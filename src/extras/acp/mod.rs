pub mod config;

use std::collections::HashMap;
use std::sync::Arc;

use agent_client_protocol::on_receive_request;
use agent_client_protocol::schema::v1::*;
use agent_client_protocol::{
    Agent, ByteStreams, Client, ConnectTo, ConnectionTo, Dispatch, Responder, Role, Stdio,
};
use compact_str::CompactString;
use tokio::sync::Mutex;

use crate::cli::Cli;
use crate::config::Config;
use crate::context::ContextFiles;
use crate::event::AgentEvent;
use crate::permission::SecurityMode;
use crate::permission::ask::AskSender;
use crate::permission::checker::{PermCheck, PermissionChecker};
use crate::sandbox::{SandboxSettings, SandboxSetup};

const AGENT_VERSION: &str = "1.0.5";

struct SessionState {
    messages: Vec<(String, String)>,
}

struct AcpState {
    cli: Cli,
    cfg: Config,
    context: ContextFiles,
    sessions: Mutex<HashMap<SessionId, SessionState>>,
}

/// The session sandbox and the warnings building it produced, from this
/// server's resolved settings. Shared by `handle_new_session` (which logs the
/// warnings once) and `run_prompt` (which needs the sandbox on every prompt),
/// so the two can never disagree about what is masked or exposed.
fn sandbox_setup(state: &AcpState) -> SandboxSetup {
    crate::sandbox::build_sandbox(&SandboxSettings {
        enabled: state.cli.resolve_sandbox(&state.cfg),
        required: state.cli.resolve_sandbox_required(&state.cfg),
        backend: &state.cli.resolve_sandbox_backend(&state.cfg),
        shell: &state.cli.resolve_shell(&state.cfg),
        expose: &state.cli.resolve_sandbox_expose(&state.cfg),
        network: state.cli.resolve_sandbox_network(&state.cfg),
    })
}

// --- TCP Transport ---

struct TcpTransport {
    host: String,
    port: u16,
}

impl<Counterpart: Role> ConnectTo<Counterpart> for TcpTransport {
    async fn connect_to(
        self,
        client: impl ConnectTo<Counterpart::Counterpart>,
    ) -> Result<(), agent_client_protocol::Error> {
        use std::net::TcpListener;

        let addr = format!("{}:{}", self.host, self.port);
        let listener = TcpListener::bind(&addr).map_err(|e| {
            agent_client_protocol::util::internal_error(format!("TCP bind {}: {}", addr, e))
        })?;

        tracing::info!("ACP TCP listening on {}", addr);

        let (stream, peer_addr) = listener.accept().map_err(|e| {
            agent_client_protocol::util::internal_error(format!("TCP accept: {}", e))
        })?;

        tracing::info!("ACP client connected from {}", peer_addr);

        let read_half = stream.try_clone().map_err(|e| {
            agent_client_protocol::util::internal_error(format!("TCP clone: {}", e))
        })?;
        let write_half = stream;

        let read_unblock = blocking::Unblock::new(read_half);
        let write_unblock = blocking::Unblock::new(write_half);

        ConnectTo::<Counterpart>::connect_to(ByteStreams::new(write_unblock, read_unblock), client)
            .await
    }
}

// --- Server Entry Point ---

pub async fn serve(cli: Cli, cfg: Config, context: ContextFiles) -> anyhow::Result<()> {
    let transport_mode = if cli.acp_host.is_some() {
        "tcp"
    } else {
        "stdio"
    };
    tracing::info!("ACP server starting: transport={}", transport_mode);

    // Extract transport config before moving cli into Arc
    let acp_host = cli.acp_host.clone();
    let acp_port = cli.acp_port;

    let state = Arc::new(AcpState {
        cli,
        cfg,
        context,
        sessions: Mutex::new(HashMap::new()),
    });

    let builder = Agent.builder().name("zerostack");

    let builder = builder
        .on_receive_request(
            {
                let state = state.clone();
                move |req: InitializeRequest, responder, _cx| {
                    let state = state.clone();
                    async move { handle_initialize(req, responder, &state).await }
                }
            },
            on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = state.clone();
                move |req: NewSessionRequest, responder, cx| {
                    let state = state.clone();
                    async move { handle_new_session(req, responder, cx, &state).await }
                }
            },
            on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = state.clone();
                move |req: PromptRequest, responder, cx| {
                    let state = state.clone();
                    async move { handle_prompt(req, responder, cx, state).await }
                }
            },
            on_receive_request!(),
        )
        .on_receive_dispatch(
            |dispatch: Dispatch<AgentRequest, AgentNotification>, cx: ConnectionTo<Client>| {
                async move {
                    tracing::warn!("ACP unhandled dispatch message");
                    dispatch.respond_with_error(
                        agent_client_protocol::util::internal_error("Unhandled ACP message"),
                        cx,
                    )
                }
            },
            agent_client_protocol::on_receive_dispatch!(),
        );

    // Choose transport: TCP if host is set, otherwise stdio
    if let Some(host) = acp_host {
        let port = acp_port.unwrap_or(7243);
        builder
            .connect_to(TcpTransport { host, port })
            .await
            .map_err(|e| anyhow::anyhow!("ACP TCP server error: {}", e))?;
    } else {
        builder
            .connect_to(Stdio::new())
            .await
            .map_err(|e| anyhow::anyhow!("ACP stdio server error: {}", e))?;
    }

    Ok(())
}

// --- Request Handlers ---

async fn handle_initialize(
    req: InitializeRequest,
    responder: Responder<InitializeResponse>,
    _state: &AcpState,
) -> Result<(), agent_client_protocol::Error> {
    let caps = AgentCapabilities::new();

    let resp = InitializeResponse::new(req.protocol_version)
        .agent_capabilities(caps)
        .agent_info(Implementation::new("zerostack", AGENT_VERSION));

    responder.respond(resp)
}

async fn handle_new_session(
    req: NewSessionRequest,
    responder: Responder<NewSessionResponse>,
    _cx: ConnectionTo<Client>,
    state: &AcpState,
) -> Result<(), agent_client_protocol::Error> {
    let session_id = SessionId::new(uuid::Uuid::new_v4().to_string());

    tracing::info!(
        "ACP new session: {} (cwd: {})",
        session_id,
        req.cwd.display()
    );

    if state.cli.sandbox_setting_conflict(&state.cfg) {
        tracing::warn!(
            "sandbox is set to false but sandbox-required is set, enabling the sandbox anyway"
        );
    }
    // Sandbox warnings are emitted once per session, so they live here and not
    // in run_prompt (which runs on every prompt). The sandbox itself is rebuilt
    // per prompt from the same settings; building it here is what makes the
    // warnings describe the sandbox that will actually run, including the
    // directory it binds, which is this process's working directory and not
    // `req.cwd` (the ACP server never chdirs to it).
    for warning in &sandbox_setup(state).warnings {
        tracing::warn!("{warning}");
    }

    state.sessions.lock().await.insert(
        session_id.clone(),
        SessionState {
            messages: Vec::new(),
        },
    );

    let resp = NewSessionResponse::new(session_id);
    responder.respond(resp)
}

async fn handle_prompt(
    req: PromptRequest,
    responder: Responder<PromptResponse>,
    cx: ConnectionTo<Client>,
    state: Arc<AcpState>,
) -> Result<(), agent_client_protocol::Error> {
    let session_id = req.session_id.clone();

    tracing::info!("ACP prompt for session {}", session_id);

    let prompt_text = req
        .prompt
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");

    // Append user message to session history
    {
        let mut sessions = state.sessions.lock().await;
        if let Some(sess) = sessions.get_mut(&session_id) {
            sess.messages
                .push(("user".to_string(), prompt_text.clone()));
        }
    }

    cx.spawn({
        let cx = cx.clone();
        async move { run_prompt(&state, &prompt_text, session_id, responder, cx).await }
    })
}

// --- Prompt Execution ---

async fn run_prompt(
    state: &AcpState,
    prompt_text: &str,
    session_id: SessionId,
    responder: Responder<PromptResponse>,
    cx: ConnectionTo<Client>,
) -> Result<(), agent_client_protocol::Error> {
    let provider_str = state.cli.resolve_provider(&state.cfg);
    let mut model_str = state.cli.resolve_model(&state.cfg);

    tracing::debug!(
        "ACP run_prompt: provider={}, model={}, prompt_len={}",
        provider_str,
        model_str,
        prompt_text.len(),
    );

    // Custom provider model override (if no explicit model set)
    if (model_str.as_str() == "deepseek/deepseek-v4-pro" || state.cli.model.is_none())
        && let Some(custom) = state.cfg.custom_providers_map().get(provider_str.as_str())
        && let Some(ref custom_model) = custom.model
    {
        model_str = custom_model.clone();
    }

    let client = crate::provider::create_client(
        &provider_str,
        None,
        &state.cfg.custom_providers_map(),
        state.cfg.api_keys.as_ref(),
    )
    .map_err(|e| agent_client_protocol::Error::new(-32603, e.to_string()))?;

    let model = client.completion_model(model_str.to_string());

    let (permission, ask_tx) = build_acp_permission(state);
    // Warnings are dropped here on purpose: handle_new_session already logged
    // them once for this session.
    let sandbox = sandbox_setup(state).sandbox;

    // Track session history for future context persistence
    let _extra_messages = {
        let sessions = state.sessions.lock().await;
        sessions
            .get(&session_id)
            .map(|s| s.messages.clone())
            .unwrap_or_default()
    };

    let temperature = crate::config::resolve_temperature(&state.cli, &state.cfg, &model_str);
    let extra_body = crate::config::resolve_extra_body(&state.cfg, &model_str);
    let agent = crate::provider::build_agent(
        model,
        &state.cli,
        &state.cfg,
        &state.context,
        permission,
        ask_tx,
        sandbox,
        false,
        temperature,
        extra_body,
        #[cfg(feature = "mcp")]
        None::<&crate::extras::mcp::McpClientManager>,
    )
    .await;

    let runner = agent
        .spawn_runner(
            prompt_text.to_string(),
            vec![],
            crate::retry::RetryConfig::default(),
            #[cfg(feature = "hooks")]
            None,
        )
        .await;
    let mut rx = runner.event_rx;

    // In-flight main-agent calls by `AgentEvent` id (rig's
    // `internal_call_id`) to the ACP ToolCallId announced for them. A map,
    // not a single slot: a parallel batch streams every `ToolCall` before
    // the first `ToolResult`.
    let mut tool_call_ids: HashMap<CompactString, ToolCallId> = HashMap::new();
    let mut final_response = String::new();

    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::Token(text) => {
                final_response.push_str(&text);
                let chunk =
                    ContentChunk::new(ContentBlock::Text(TextContent::new(text.to_string())));
                let notif = SessionNotification::new(
                    session_id.clone(),
                    SessionUpdate::AgentMessageChunk(chunk),
                );
                if let Err(e) = cx.send_notification(notif) {
                    tracing::warn!("ACP failed to send token notification: {}", e);
                }
            }
            AgentEvent::Reasoning(text) => {
                let chunk =
                    ContentChunk::new(ContentBlock::Text(TextContent::new(text.to_string())));
                let notif = SessionNotification::new(
                    session_id.clone(),
                    SessionUpdate::AgentThoughtChunk(chunk),
                );
                if let Err(e) = cx.send_notification(notif) {
                    tracing::warn!("ACP failed to send reasoning notification: {}", e);
                }
            }
            AgentEvent::ToolCall {
                call_id: event_id,
                name,
                args,
            } => {
                let id = ToolCallId::new(uuid::Uuid::new_v4().to_string());
                tool_call_ids.insert(event_id, id.clone());
                let args_str = args.to_string();
                let tool_call = ToolCall::new(id.clone(), name.to_string())
                    .raw_input(serde_json::from_str(&args_str).ok());
                let notif = SessionNotification::new(
                    session_id.clone(),
                    SessionUpdate::ToolCall(tool_call),
                );
                if let Err(e) = cx.send_notification(notif) {
                    tracing::warn!("ACP failed to send tool call notification: {}", e);
                }
            }
            AgentEvent::SubagentToolCall { name, args } => {
                // Announce-only: subagent calls carry no correlating id, so
                // they never receive a ToolCallUpdate. (Previously they
                // hijacked the single pending slot, so the enclosing `task`
                // call's result got attached to the subagent's entry.)
                // Announced as already Completed, since nothing will ever
                // update it out of the default Pending status.
                let id = ToolCallId::new(uuid::Uuid::new_v4().to_string());
                let args_str = args.to_string();
                let tool_call = ToolCall::new(id.clone(), format!("[subagent] {}", name))
                    .status(ToolCallStatus::Completed)
                    .raw_input(serde_json::from_str(&args_str).ok());
                let notif = SessionNotification::new(
                    session_id.clone(),
                    SessionUpdate::ToolCall(tool_call),
                );
                if let Err(e) = cx.send_notification(notif) {
                    tracing::warn!("ACP failed to send subagent tool call notification: {}", e);
                }
            }
            AgentEvent::ToolResult {
                call_id: event_id,
                output,
                ..
            } => {
                // No announced ToolCall to update: an update carrying a
                // ToolCallId the client was never told about is worse than
                // silence, so drop it.
                let Some(id) = tool_call_ids.remove(&event_id) else {
                    tracing::warn!(
                        "ACP tool result with no announced tool call (id={}); \
                         skipping update",
                        event_id.escape_debug(),
                    );
                    continue;
                };
                let fields = ToolCallUpdateFields::new()
                    .status(ToolCallStatus::Completed)
                    .content(vec![ToolCallContent::from(ContentBlock::Text(
                        TextContent::new(output.to_string()),
                    ))]);
                let update = ToolCallUpdate::new(id, fields);
                let notif = SessionNotification::new(
                    session_id.clone(),
                    SessionUpdate::ToolCallUpdate(update),
                );
                if let Err(e) = cx.send_notification(notif) {
                    tracing::warn!("ACP failed to send tool result notification: {}", e);
                }
            }
            AgentEvent::Retrying { attempt, max } => {
                // ACP has no status bar, so surface the retry as an agent
                // thought. This keeps the client from going silent during the
                // backoff delay and mirrors how `Reasoning` is forwarded.
                let text = format!("retrying... ({}/{})", attempt, max);
                let chunk = ContentChunk::new(ContentBlock::Text(TextContent::new(text)));
                let notif = SessionNotification::new(
                    session_id.clone(),
                    SessionUpdate::AgentThoughtChunk(chunk),
                );
                if let Err(e) = cx.send_notification(notif) {
                    tracing::warn!("ACP failed to send retry notification: {}", e);
                }
            }
            AgentEvent::CompletionCall { .. } => {
                // Mid-stream provider usage; ACP has no status bar to update, so
                // there is nothing to surface for this event.
            }
            AgentEvent::Done { .. } => {
                break;
            }
            AgentEvent::Error(err) => {
                // Surface the error to the client instead of silently
                // reporting EndTurn.
                let chunk = ContentChunk::new(ContentBlock::Text(TextContent::new(format!(
                    "[error: {}]",
                    err
                ))));
                let notif = SessionNotification::new(
                    session_id.clone(),
                    SessionUpdate::AgentMessageChunk(chunk),
                );
                let _ = cx.send_notification(notif);
                let _ = responder.respond(PromptResponse::new(StopReason::Refusal));
                return Ok(());
            }
        }
    }

    // Store assistant response in session history
    if !final_response.is_empty() {
        let mut sessions = state.sessions.lock().await;
        if let Some(sess) = sessions.get_mut(&session_id) {
            sess.messages
                .push(("assistant".to_string(), final_response));
        }
    }

    let _ = responder.respond(PromptResponse::new(StopReason::EndTurn));
    Ok(())
}

// --- Permission ---

fn build_acp_permission(state: &AcpState) -> (Option<PermCheck>, Option<AskSender>) {
    use std::sync::Mutex as StdMutex;

    let no_tools = state.cli.resolve_no_tools(&state.cfg);
    if no_tools || state.cli.dangerously_skip_permissions {
        return (None, None);
    }

    let perm_config = state.cfg.build_permission_config();

    let mode = resolve_acp_mode(&state.cli, &state.cfg);
    let permission_modes = state.cfg.permission_modes.clone();
    let checker = PermissionChecker::new(&perm_config, mode, None, permission_modes);
    let perm: PermCheck = Arc::new(StdMutex::new(checker));

    let (ask_tx, mut ask_rx) = tokio::sync::mpsc::channel::<crate::permission::ask::AskRequest>(64);
    // ACP is headless — there is no interactive user to prompt. Auto-approve
    // Ask requests so tools don't fail with "Permission system unavailable".
    // Log a warning so the auto-approval is visible in logs.
    tokio::spawn(async move {
        while let Some(req) = ask_rx.recv().await {
            tracing::warn!(
                "ACP auto-approving tool call: tool={}, input_len={}",
                req.tool,
                req.input.len()
            );
            let _ = req
                .reply
                .send(crate::permission::ask::UserDecision::AllowOnce);
        }
    });

    (Some(perm), Some(ask_tx))
}

pub(crate) fn resolve_acp_mode(cli: &Cli, cfg: &Config) -> SecurityMode {
    if cli.dangerously_skip_permissions {
        SecurityMode::Standard
    } else if cli.yolo || cfg.yolo.unwrap_or(false) {
        SecurityMode::Yolo
    } else if cli.accept_all || cfg.accept_all.unwrap_or(false) {
        SecurityMode::Standard
    } else if cli.restrictive || cfg.restrictive.unwrap_or(false) {
        SecurityMode::Restrictive
    } else if let Some(m) = &cfg.default_permission_mode {
        match m.as_str() {
            "yolo" => SecurityMode::Yolo,
            "accept" | "standard" => SecurityMode::Standard,
            "guarded" => SecurityMode::Guarded,
            "readonly" => SecurityMode::ReadOnly,
            "restrictive" => SecurityMode::Restrictive,
            _ => SecurityMode::Standard,
        }
    } else {
        SecurityMode::Standard
    }
}

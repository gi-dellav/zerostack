pub mod config;

use std::sync::Arc;

use agent_client_protocol::on_receive_request;
use agent_client_protocol::schema::*;
use agent_client_protocol::{Agent, Client, ConnectionTo, Dispatch, Responder, Stdio};

use crate::cli::Cli;
use crate::config::Config;
use crate::context::ContextFiles;
use crate::event::AgentEvent;
use crate::permission::ask::AskSender;
use crate::permission::checker::{PermCheck, PermissionChecker};
use crate::permission::{PermissionConfig, SecurityMode};
use crate::sandbox::Sandbox;

struct AcpState {
    cli: Cli,
    cfg: Config,
    context: ContextFiles,
}

pub async fn serve(cli: Cli, cfg: Config, context: ContextFiles) -> anyhow::Result<()> {
    let state = Arc::new(AcpState { cli, cfg, context });

    Agent
        .builder()
        .name("zerostack")
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
                    dispatch.respond_with_error(
                        agent_client_protocol::util::internal_error("Unhandled ACP message"),
                        cx,
                    )
                }
            },
            agent_client_protocol::on_receive_dispatch!(),
        )
        .connect_to(Stdio::new())
        .await
        .map_err(|e| anyhow::anyhow!("ACP server error: {}", e))?;

    Ok(())
}

async fn handle_initialize(
    req: InitializeRequest,
    responder: Responder<InitializeResponse>,
    state: &AcpState,
) -> Result<(), agent_client_protocol::Error> {
    let _ = state;

    let caps = AgentCapabilities::new();

    let resp = InitializeResponse::new(req.protocol_version)
        .agent_capabilities(caps)
        .agent_info(Implementation::new("zerostack", "1.0.4"));

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

    let _ = state;

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

    cx.spawn({
        let cx = cx.clone();
        async move { run_prompt(&state, &prompt_text, session_id, responder, cx).await }
    })
}

async fn run_prompt(
    state: &AcpState,
    prompt_text: &str,
    session_id: SessionId,
    responder: Responder<PromptResponse>,
    cx: ConnectionTo<Client>,
) -> Result<(), agent_client_protocol::Error> {
    let provider_str = state.cli.resolve_provider(&state.cfg);
    let model_str = state.cli.resolve_model(&state.cfg);

    let client =
        crate::provider::create_client(&provider_str, None, &state.cfg.custom_providers_map())
            .map_err(|e| agent_client_protocol::Error::new(-32603, e.to_string()))?;

    let model = client.completion_model(model_str.to_string());

    let (permission, ask_tx) = build_acp_permission(state);
    let sandbox = Sandbox::new(state.cli.resolve_sandbox(&state.cfg));

    let agent = crate::provider::build_agent(
        model,
        &state.cli,
        &state.cfg,
        &state.context,
        permission,
        ask_tx,
        sandbox,
        #[cfg(feature = "mcp")]
        None::<&crate::extras::mcp::McpClientManager>,
    )
    .await;

    let runner = agent.spawn_runner(prompt_text.to_string(), vec![]);
    let mut rx = runner.event_rx;

    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::Token(text) => {
                let chunk =
                    ContentChunk::new(ContentBlock::Text(TextContent::new(text.to_string())));
                let notif = SessionNotification::new(
                    session_id.clone(),
                    SessionUpdate::AgentMessageChunk(chunk),
                );
                let _ = cx.send_notification(notif);
            }
            AgentEvent::Reasoning(text) => {
                let chunk =
                    ContentChunk::new(ContentBlock::Text(TextContent::new(text.to_string())));
                let notif = SessionNotification::new(
                    session_id.clone(),
                    SessionUpdate::AgentThoughtChunk(chunk),
                );
                let _ = cx.send_notification(notif);
            }
            AgentEvent::ToolCall { name, args } => {
                let args_str = args.to_string();
                let tool_call = ToolCall::new(
                    ToolCallId::new(uuid::Uuid::new_v4().to_string()),
                    name.to_string(),
                )
                .raw_input(serde_json::from_str(&args_str).ok());
                let notif = SessionNotification::new(
                    session_id.clone(),
                    SessionUpdate::ToolCall(tool_call),
                );
                let _ = cx.send_notification(notif);
            }
            AgentEvent::ToolResult { output } => {
                let fields = ToolCallUpdateFields::new()
                    .status(ToolCallStatus::Completed)
                    .content(vec![ToolCallContent::from(ContentBlock::Text(
                        TextContent::new(output.to_string()),
                    ))]);
                let update = ToolCallUpdate::new(ToolCallId::new(String::new()), fields);
                let notif = SessionNotification::new(
                    session_id.clone(),
                    SessionUpdate::ToolCallUpdate(update),
                );
                let _ = cx.send_notification(notif);
            }
            AgentEvent::Done { .. } => {
                break;
            }
            AgentEvent::Error(_) => {
                break;
            }
        }
    }

    let _ = responder.respond(PromptResponse::new(StopReason::EndTurn));
    Ok(())
}

fn build_acp_permission(state: &AcpState) -> (Option<PermCheck>, Option<AskSender>) {
    use std::sync::Mutex;

    let no_tools = state.cli.resolve_no_tools(&state.cfg);
    if no_tools {
        return (None, None);
    }

    let perm_config: PermissionConfig = state
        .cfg
        .permission
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    let mode = resolve_acp_mode(&state.cli, &state.cfg);
    let checker = PermissionChecker::new(&perm_config, mode, None);
    let perm: PermCheck = Arc::new(Mutex::new(checker));

    let (ask_tx, _ask_rx) = tokio::sync::mpsc::channel(64);

    (Some(perm), Some(ask_tx))
}

fn resolve_acp_mode(cli: &Cli, cfg: &Config) -> SecurityMode {
    if cli.yolo || cfg.yolo.unwrap_or(false) {
        SecurityMode::Yolo
    } else if cli.accept_all || cfg.accept_all.unwrap_or(false) {
        SecurityMode::Accept
    } else if cli.restrictive || cfg.restrictive.unwrap_or(false) {
        SecurityMode::Restrictive
    } else if let Some(m) = &cfg.default_permission_mode {
        match m.as_str() {
            "yolo" => SecurityMode::Yolo,
            "accept" => SecurityMode::Accept,
            "restrictive" => SecurityMode::Restrictive,
            _ => SecurityMode::Standard,
        }
    } else {
        SecurityMode::Standard
    }
}

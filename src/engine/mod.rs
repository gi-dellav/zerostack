//! Headless agent execution core: [`Engine`] + `run_string`.
//!
//! `Engine` owns everything an agent turn needs — config, session, context,
//! provider client, permission checker, sandbox — so a plain message or a
//! slash command can be executed without a TUI. The interactive `App`
//! (`src/ui/app.rs`) keeps its own event loop; this module is the
//! programmatic counterpart used by tests, embeddings, and future library
//! callers.
//!
//! ## Semantics of `run_string`
//!
//! - `""` (empty/whitespace) → [`RunOutput`] with `kind: Ignored`, no session
//!   mutation.
//! - `"/..."` → slash command, dispatched through the same per-command
//!   handlers the TUI uses. Local commands (help, model, prompt, sessions,
//!   add, mode, ...) run inline and return their transcript. Spawning
//!   commands (`/compress`, `/init`, `/review`) resolve through the same
//!   deferred outcomes as the TUI and then drive an agent turn.
//! - `".prompt msg"` / `".prompt"` → one-shot or sticky prompt switch,
//!   mirroring `App::handle_dot_command`.
//! - `"!cmd"` → shell via `bash -c`, recorded as User+Assistant like the TUI.
//! - anything else → main agent run: `convert_history` → `spawn_runner` →
//!   drain `AgentEvent`s into the session (tool calls/results, cost,
//!   calibration, compaction), mirroring `event_handler::handle_agent_event`
//!   + `handle_agent_done` without any terminal rendering.
//!
//! Interactive-only surface (pickers, `$EDITOR` suspend, MCP OAuth browser
//! flow) reports a friendly error instead of blocking: headless runs never
//! read stdin.

pub mod sink;

pub use sink::{EventSink, StringSink};

use compact_str::CompactString;
use smallvec::SmallVec;

use crate::agent::runner::{self, AgentRunner};
use crate::cli::Cli;
use crate::config::{self, Config};
use crate::context::ContextFiles;
use crate::event::AgentEvent;
use crate::permission::SecurityMode;
use crate::permission::checker::PermCheck;
use crate::provider::{AnyAgent, AnyClient};
use crate::sandbox::Sandbox;
use crate::session::{MessageRole, Session};
use crate::ui::state::TurnUsage;

/// What `run_string` did with the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunKind {
    /// Plain message (or `!` command) that ran the agent / shell.
    Agent,
    /// Slash command handled locally, no agent run.
    Command,
    /// Empty input; nothing happened.
    Ignored,
}

/// Result of one [`Engine::run_string`] call.
#[derive(Debug, Clone)]
pub struct RunOutput {
    /// How the input was handled.
    pub kind: RunKind,
    /// Human-readable transcript: slash output lines, or the assistant's
    /// final response for agent runs (`!` returns the shell output).
    pub text: String,
    /// Token usage of the agent turn that ran, if any.
    pub usage: Option<TurnUsage>,
}

impl RunOutput {
    fn agent(text: String, usage: TurnUsage) -> Self {
        Self {
            kind: RunKind::Agent,
            text,
            usage: Some(usage),
        }
    }

    fn command(text: impl Into<String>) -> Self {
        Self {
            kind: RunKind::Command,
            text: text.into(),
            usage: None,
        }
    }

    fn ignored() -> Self {
        Self {
            kind: RunKind::Ignored,
            text: String::new(),
            usage: None,
        }
    }
}

/// Owned headless execution context. Construct with [`Engine::new`] (fresh
/// in-process state, no disk writes unless a command needs them) or
/// [`Engine::with_agent`] to inject a scripted agent in tests.
pub struct Engine {
    cli: Cli,
    cfg: Config,
    session: Session,
    context: ContextFiles,
    client: AnyClient,
    agent: Option<AnyAgent>,
    permission: Option<PermCheck>,
    sandbox: Sandbox,
    /// Mirrors `SlashState`: toggles owned by slash commands.
    show_reasoning: bool,
    reasoning_enabled: bool,
    /// Scratch for one in-flight turn: pending tool-call ids (rig
    /// `internal_call_id` → session call id), streamed text, turn trace.
    pending_tool_calls: Vec<(CompactString, u64)>,
    response_buf: String,
    turn_trace: Vec<CompactString>,
    /// One-shot prompt restore for `.prompt msg` (see `ChainState`).
    dot_prompt_restore: Option<String>,
}

impl Engine {
    /// Build an engine from owned state. The agent is built lazily on the
    /// first run that needs it (mirrors `ensure_agent`).
    pub fn new(
        cli: Cli,
        cfg: Config,
        session: Session,
        context: ContextFiles,
        client: AnyClient,
        permission: Option<PermCheck>,
        sandbox: Sandbox,
    ) -> Self {
        let show_reasoning = cfg.resolve_show_reasoning();
        Self {
            cli,
            cfg,
            session,
            context,
            client,
            agent: None,
            permission,
            sandbox,
            show_reasoning,
            reasoning_enabled: true,
            pending_tool_calls: Vec::new(),
            response_buf: String::new(),
            turn_trace: Vec::new(),
            dot_prompt_restore: None,
        }
    }

    /// Build an engine with a pre-built agent (tests inject `AnyAgent::Mock`).
    pub fn with_agent(
        cli: Cli,
        cfg: Config,
        session: Session,
        context: ContextFiles,
        client: AnyClient,
        permission: Option<PermCheck>,
        sandbox: Sandbox,
        agent: AnyAgent,
    ) -> Self {
        let mut engine = Self::new(cli, cfg, session, context, client, permission, sandbox);
        engine.agent = Some(agent);
        engine
    }

    /// Borrow the session (assertions, persistence).
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Mutably borrow the session.
    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }

    /// Borrow the context files (extra files, prompts).
    pub fn context(&self) -> &ContextFiles {
        &self.context
    }

    /// Run one user input string: plain message, `/` slash command, `.`
    /// dot-prompt command, or `!` shell command.
    pub async fn run_string(&mut self, input: &str) -> anyhow::Result<RunOutput> {
        let text = input.trim();
        if text.is_empty() {
            return Ok(RunOutput::ignored());
        }
        if text.starts_with('/') {
            // `/queue` / `/btw` are TUI-loop concepts (queue management,
            // parallel side questions); headless runs resolve them inline.
            let t = text.trim_start();
            if t == "/queue" || t.starts_with("/queue ") {
                let mut sink = StringSink::new();
                self.handle_queue(t, &mut sink);
                return Ok(RunOutput::command(sink.transcript()));
            }
            if t == "/btw" || t.starts_with("/btw ") {
                return Ok(self.run_btw(t).await);
            }
            return Ok(self.run_slash(text).await);
        }
        if let Some(out) = self.handle_dot_command(text).await? {
            return Ok(out);
        }
        if text.starts_with('!') {
            return Ok(self.run_bang(text).await);
        }
        Ok(self.run_agent_text(text).await)
    }

    // ── main agent run ────────────────────────────────────────────────

    async fn run_agent_text(&mut self, text: &str) -> RunOutput {
        let mut sink = StringSink::new();
        self.sink_echo_user(text, &mut sink);
        match self.start_agent_run(text.to_string()).await {
            Ok((response, usage)) => RunOutput::agent(response, usage),
            Err(e) => {
                sink.write_error(&e.to_string());
                // Roll back the optimistic user message like the TUI does on
                // a failed send (`App::finalize_turn`).
                let len = self.session.messages.len();
                if len > 0 && self.session.messages[len - 1].role == MessageRole::User {
                    self.session.truncate_to(len - 1);
                }
                RunOutput::agent(sink.transcript(), TurnUsage::default())
            }
        }
    }

    /// Spawn the agent for `prompt` and drain its events into the session.
    /// Returns the assistant's final response text plus the turn's usage.
    async fn start_agent_run(&mut self, prompt: String) -> anyhow::Result<(String, TurnUsage)> {
        self.ensure_agent().await;
        let history = runner::convert_history(&self.session);
        #[cfg(feature = "multimodal")]
        let history = {
            let media = self.session.drain_media();
            if media.is_empty() {
                history
            } else {
                let mut h = history;
                h.extend(runner::media_to_messages(&media));
                h
            }
        };
        let agent = self.agent.clone().expect("ensure_agent built one");
        let runner = agent
            .spawn_runner(
                prompt.clone(),
                history,
                self.cfg.retry.clone(),
                #[cfg(feature = "hooks")]
                None,
            )
            .await;
        self.session.add_message(MessageRole::User, &prompt);
        #[cfg(feature = "advisor")]
        crate::extras::advisor::set_session_messages(self.session.messages.clone());
        if !self.cli.no_session
            && let Err(e) = crate::session::chat_history::append_entry(
                &crate::session::chat_history::ChatHistoryEntry {
                    content: prompt,
                    timestamp: self.session.updated_at.clone(),
                },
            )
        {
            tracing::warn!("failed to append chat history entry: {e}");
        }
        self.drain_runner(runner).await
    }

    /// Drive an `AgentRunner` to completion, folding every event into the
    /// session. Mirrors `event_handler::handle_agent_event` +
    /// `handle_agent_done`, minus rendering. Returns the assistant's final
    /// response plus the turn's usage.
    async fn drain_runner(
        &mut self,
        mut runner: AgentRunner,
    ) -> anyhow::Result<(String, TurnUsage)> {
        self.response_buf.clear();
        self.pending_tool_calls.clear();
        let mut usage = TurnUsage::default();
        let mut final_response = String::new();
        let mut turn_error: Option<String> = None;

        while let Some(event) = runner.event_rx.recv().await {
            match event {
                AgentEvent::Reasoning(text) => {
                    if self.show_reasoning {
                        self.response_buf.push_str(&text);
                    }
                }
                AgentEvent::Token(text) => {
                    let safe = crate::ui::events::sanitize_output(&text);
                    self.response_buf.push_str(safe.as_str());
                }
                AgentEvent::ToolCall {
                    call_id: event_id,
                    name,
                    args,
                } => {
                    self.response_buf.clear();
                    let summary = crate::ui::utils::format_tool_call_summary(&name, &args);
                    if self.turn_trace.len() < 64 {
                        self.turn_trace
                            .push(CompactString::from(format!("→ {summary}")));
                    }
                    let call_id = self.session.add_tool_call(&name, &args);
                    self.push_pending_tool_call(event_id, call_id);
                    self.save_session_best_effort();
                }
                #[cfg(any(feature = "subagents", feature = "acp"))]
                AgentEvent::SubagentToolCall { name, args } => {
                    let parent = self.pending_tool_calls.last().map(|&(_, id)| id);
                    self.session.add_subagent_tool_call(parent, &name, &args);
                    self.save_session_best_effort();
                }
                AgentEvent::ToolResult {
                    call_id: event_id,
                    name,
                    output,
                } => {
                    if self.turn_trace.len() < 64 {
                        self.turn_trace.push(CompactString::from(format!(
                            "← {}",
                            crate::extras::truncate_cjk(&output, 500, "…")
                        )));
                    }
                    let call_id = self.resolve_tool_result_call_id(&event_id, &name);
                    self.session.add_tool_result(call_id, &name, &output);
                    self.save_session_best_effort();
                }
                AgentEvent::CompletionCall {
                    input_tokens,
                    output_tokens,
                    cached_input_tokens,
                    cache_creation_input_tokens,
                } => {
                    self.apply_completion_usage(
                        input_tokens,
                        output_tokens,
                        cached_input_tokens,
                        cache_creation_input_tokens,
                        &mut usage,
                    );
                }
                AgentEvent::Retrying { .. } => {
                    self.response_buf.clear();
                }
                AgentEvent::Done {
                    response,
                    input_tokens,
                    output_tokens,
                    cached_input_tokens,
                    cache_creation_input_tokens,
                } => {
                    usage = TurnUsage {
                        input_tokens,
                        output_tokens,
                        cached_input_tokens,
                        cache_creation_input_tokens,
                    };
                    final_response = response.to_string();
                    self.finish_turn(&response, usage).await;
                    break;
                }
                AgentEvent::Error(e) => {
                    turn_error = Some(e.to_string());
                    self.pending_tool_calls.clear();
                    self.save_session_best_effort();
                    break;
                }
            }
        }
        self.turn_trace.clear();
        self.pending_tool_calls.clear();

        if let Some(e) = turn_error {
            anyhow::bail!("{e}");
        }
        Ok((final_response, usage))
    }

    /// Fold one finished turn into the session: assistant message, token
    /// totals, cost, calibration anchor, auto-compaction. Mirrors the
    /// non-rendering half of `handle_agent_done`.
    async fn finish_turn(&mut self, response: &str, usage: TurnUsage) {
        self.session.add_message(MessageRole::Assistant, response);
        self.session.total_input_tokens = self
            .session
            .total_input_tokens
            .saturating_add(usage.input_tokens);
        self.session.total_output_tokens = self
            .session
            .total_output_tokens
            .saturating_add(usage.output_tokens);
        self.session.total_cached_input_tokens = self
            .session
            .total_cached_input_tokens
            .saturating_add(usage.cached_input_tokens);
        self.session.total_cache_creation_input_tokens = self
            .session
            .total_cache_creation_input_tokens
            .saturating_add(usage.cache_creation_input_tokens);
        self.session.total_cost += crate::pricing::estimate_cost(
            crate::pricing::billable_input_tokens(
                self.cfg.is_anthropic_native(&self.session.provider),
                usage.input_tokens,
                usage.cached_input_tokens,
                usage.cache_creation_input_tokens,
            ),
            usage.output_tokens,
            self.session.input_token_cost,
            self.session.output_token_cost,
        );
        let context_input_tokens = Session::real_input_tokens(
            self.cfg.is_anthropic_native(&self.session.provider),
            usage.input_tokens,
            usage.cached_input_tokens,
            usage.cache_creation_input_tokens,
        );
        self.session
            .set_calibration(context_input_tokens, usage.output_tokens);
        self.response_buf.clear();

        if self.cfg.resolve_compact_enabled() && self.session.needs_compaction(self.reserve()) {
            let mut sink = StringSink::new();
            if let Err(e) = self.compress(None, true, &mut sink).await {
                sink.write_error(&format!("auto-compact error: {e}"));
            }
        }
        self.save_session_best_effort();
        // One-shot prompt restore, mirroring `App::finalize_turn`.
        if let Some(restore_name) = self.dot_prompt_restore.take() {
            self.restore_prompt(&restore_name);
        }
    }

    /// Intermediate-call accounting: cost for every tool round trip (the
    /// `Done` event only carries the final call's usage). Mirrors the
    /// `CompletionCall` arm of `handle_agent_event`.
    fn apply_completion_usage(
        &mut self,
        input_tokens: u64,
        output_tokens: u64,
        cached_input_tokens: u64,
        cache_creation_input_tokens: u64,
        usage: &mut TurnUsage,
    ) {
        let real = Session::real_input_tokens(
            self.cfg.is_anthropic_native(&self.session.provider),
            input_tokens,
            cached_input_tokens,
            cache_creation_input_tokens,
        )
        .saturating_add(output_tokens);
        if real > self.session.total_estimated_tokens {
            self.session.total_estimated_tokens = real;
        }
        self.session.total_input_tokens =
            self.session.total_input_tokens.saturating_add(input_tokens);
        self.session.total_output_tokens = self
            .session
            .total_output_tokens
            .saturating_add(output_tokens);
        self.session.total_cost += crate::pricing::estimate_cost(
            crate::pricing::billable_input_tokens(
                self.cfg.is_anthropic_native(&self.session.provider),
                input_tokens,
                cached_input_tokens,
                cache_creation_input_tokens,
            ),
            output_tokens,
            self.session.input_token_cost,
            self.session.output_token_cost,
        );
        usage.input_tokens = usage.input_tokens.saturating_add(input_tokens);
        usage.output_tokens = usage.output_tokens.saturating_add(output_tokens);
        usage.cached_input_tokens = usage
            .cached_input_tokens
            .saturating_add(cached_input_tokens);
        usage.cache_creation_input_tokens = usage
            .cache_creation_input_tokens
            .saturating_add(cache_creation_input_tokens);
    }

    // ── agent construction ────────────────────────────────────────────

    /// Lazily build the main agent (mirrors `ensure_agent`).
    async fn ensure_agent(&mut self) {
        if self.agent.is_some() {
            return;
        }
        #[cfg(feature = "mcp")]
        {
            // Headless engines never connect MCP lazily: pass `None` like
            // `dispatch_print` does. Callers needing MCP build the agent
            // up front via `with_agent`.
        }
        let model = self.client.completion_model(self.session.model.to_string());
        let temperature = config::resolve_temperature(&self.cli, &self.cfg, &self.session.model);
        let extra_body = config::resolve_extra_body(&self.cfg, &self.session.model);
        let agent = crate::provider::build_agent(
            model,
            &self.cli,
            &self.cfg,
            &self.context,
            self.permission.clone(),
            // Headless: no one drains the ask channel, so an `Ask` verdict
            // must fail closed (same as `dispatch_print`'s `None`).
            None,
            self.sandbox.clone(),
            self.reasoning_enabled,
            temperature,
            extra_body,
            #[cfg(feature = "mcp")]
            None,
        )
        .await;
        self.session.overhead_tokens =
            crate::agent::builder::estimate_overhead(&self.context, self.reasoning_enabled);
        self.agent = Some(agent);
    }

    fn agent_build_parts(
        &self,
    ) -> (
        &Cli,
        &Config,
        &ContextFiles,
        &AnyClient,
        &Option<PermCheck>,
        &Sandbox,
    ) {
        (
            &self.cli,
            &self.cfg,
            &self.context,
            &self.client,
            &self.permission,
            &self.sandbox,
        )
    }

    async fn rebuild_agent(&mut self, model_id: &str) {
        #[cfg(feature = "advisor")]
        {
            crate::extras::advisor::update_client(self.client.clone());
            crate::extras::advisor::set_session_messages(self.session.messages.clone());
        }
        let (cli, cfg, context, client, permission, sandbox) = self.agent_build_parts();
        let model = client.completion_model(model_id.to_string());
        let temperature = config::resolve_temperature(cli, cfg, model_id);
        let extra_body = config::resolve_extra_body(cfg, model_id);
        let agent = crate::provider::build_agent(
            model,
            cli,
            cfg,
            context,
            permission.clone(),
            None,
            sandbox.clone(),
            self.reasoning_enabled,
            temperature,
            extra_body,
            #[cfg(feature = "mcp")]
            None,
        )
        .await;
        self.agent = Some(agent);
    }

    async fn rebuild_agent_with_client(&mut self, provider: &str) -> anyhow::Result<()> {
        self.client = crate::provider::create_client(
            provider,
            self.cli.api_key.as_deref(),
            &self.cfg.custom_providers_map(),
            self.cfg.api_keys.as_ref(),
        )?;
        #[cfg(feature = "advisor")]
        {
            crate::extras::advisor::update_client(self.client.clone());
            crate::extras::advisor::set_session_messages(self.session.messages.clone());
        }
        let model_id = self.session.model.to_string();
        self.rebuild_agent(&model_id).await;
        Ok(())
    }

    // ── slash dispatch ────────────────────────────────────────────────

    /// Dispatch `/...` through the same per-command handlers as the TUI.
    /// Deferred outcomes (`DeferCompress/DeferInit/DeferReview`) resolve
    /// inline by driving agent turns; interactive-only ones (editor suspend,
    /// MCP OAuth browser flow, quit) report a friendly error.
    async fn run_slash(&mut self, text: &str) -> RunOutput {
        let mut sink = StringSink::new();
        self.sink_echo_user(text, &mut sink);
        let parts: SmallVec<[&str; 3]> = text.trim().splitn(3, ' ').collect();
        if parts.is_empty() {
            return RunOutput::command(sink.transcript());
        }
        let result = match parts[0] {
            "/provider" | "/model" | "/models" | "/models-add" | "/model-subagent"
            | "/models-subagent" => self.slash_providers(&parts, &mut sink).await,
            "/prompt" | "/theme" | "/regen-prompts" | "/regen-themes" => {
                self.slash_content(&parts, &mut sink).await
            }
            "/reasoning" | "/thinking" | "/mode" | "/toggle" | "/editsys" | "/advisor" => {
                self.slash_settings(&parts, &mut sink).await
            }
            "/sessions" | "/rename" | "/clear" | "/new" | "/undo" | "/redo" | "/rewind"
            | "/retry" | "/quit" | "/exit" | "/history" => {
                self.slash_session(&parts, &mut sink).await
            }
            #[cfg(feature = "export")]
            "/export" | "/import" | "/share" => self.slash_session(&parts, &mut sink).await,
            "/help" => {
                self.slash_help(&parts, &mut sink);
                Ok(SlashFlow::Done)
            }
            "/welcome" | "/tutorial" => {
                self.slash_help(&parts, &mut sink);
                Ok(SlashFlow::Done)
            }
            "/tutor" => {
                sink.write_error("tutor needs an interactive terminal (opens GET_STARTED.md)");
                Ok(SlashFlow::Done)
            }
            "/add" | "/drop" | "/drop-all" => self.slash_add(&parts, &mut sink).await,
            "/init" => self.slash_init(&parts, &mut sink).await,
            "/review" => self.slash_review(&parts, &mut sink).await,
            "/memory" => self.slash_memory(&parts, &mut sink).await,
            "/compress" | "/compact" | "/loop" | "/worktree" | "/wt-merge" | "/wt-exit" => {
                self.slash_features(&parts, &mut sink).await
            }
            #[cfg(feature = "hooks")]
            "/hooks" => {
                self.slash_hooks(&mut sink);
                Ok(SlashFlow::Done)
            }
            _ => {
                sink.write_error(format!("unknown command: {} (try /help)", parts[0]));
                Ok(SlashFlow::Done)
            }
        };
        match result {
            Ok(SlashFlow::Done) => RunOutput::command(sink.transcript()),
            Ok(SlashFlow::DeferCompress { instructions }) => {
                let r = self
                    .compress(instructions.as_deref(), false, &mut sink)
                    .await;
                if let Err(e) = r {
                    sink.write_error(format!("compress error: {e}"));
                }
                self.save_session_best_effort();
                RunOutput::command(sink.transcript())
            }
            Ok(SlashFlow::DeferInit) => {
                let prompt = crate::ui::slash::init::AGENTS_CREATION_PROMPT.to_string();
                match self.start_agent_run(prompt).await {
                    Ok((response, _)) => {
                        sink.write_ok(&response);
                        RunOutput::command(sink.transcript())
                    }
                    Err(e) => {
                        sink.write_error(e.to_string());
                        RunOutput::command(sink.transcript())
                    }
                }
            }
            Ok(SlashFlow::DeferReview { message }) => {
                self.dot_prompt_restore = self.context.one_shot_restore.take();
                match self.start_agent_run(message.clone()).await {
                    Ok((response, _)) => {
                        sink.write_ok(&response);
                        RunOutput::command(sink.transcript())
                    }
                    Err(e) => {
                        sink.write_error(e.to_string());
                        RunOutput::command(sink.transcript())
                    }
                }
            }
            Err(e) => {
                // `/quit` + worktree-merge/exit surface as typed errors so the
                // TUI can intercept them; headless reports them as text.
                sink.write_error(e.to_string());
                RunOutput::command(sink.transcript())
            }
        }
    }

    // ── `!` shell ─────────────────────────────────────────────────────

    async fn run_bang(&mut self, text: &str) -> RunOutput {
        let mut sink = StringSink::new();
        let cmd = text.strip_prefix('!').map(|s| s.trim()).unwrap_or("");
        if cmd.is_empty() {
            sink.write_error("empty command after '!'");
            return RunOutput::command(sink.transcript());
        }
        self.sink_echo_user(text, &mut sink);
        let cmd_owned = cmd.to_string();
        let output = tokio::task::spawn_blocking(move || {
            std::process::Command::new("bash")
                .arg("-c")
                .arg(&cmd_owned)
                .output()
        })
        .await;
        let output = match output {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => {
                sink.write_error(format!("command error: {e}"));
                return RunOutput::command(sink.transcript());
            }
            Err(e) => {
                sink.write_error(format!("spawn error: {e}"));
                return RunOutput::command(sink.transcript());
            }
        };
        let mut result = String::new();
        if !output.stdout.is_empty() {
            result.push_str(&String::from_utf8_lossy(&output.stdout));
        }
        if !output.stderr.is_empty() {
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str(&String::from_utf8_lossy(&output.stderr));
        }
        let result = result.trim().to_string();
        if output.status.success() {
            sink.write_ok(&result);
        } else {
            sink.write_error(&result);
        }
        self.session.add_message(MessageRole::User, text);
        self.session.add_message(MessageRole::Assistant, &result);
        self.save_session_best_effort();
        RunOutput::agent(result, TurnUsage::default())
    }

    // ── `/btw` ────────────────────────────────────────────────────────

    async fn run_btw(&mut self, text: &str) -> RunOutput {
        let mut sink = StringSink::new();
        let btw_text = text
            .trim_start()
            .strip_prefix("/btw")
            .map(|s| s.trim())
            .unwrap_or("");
        if btw_text.is_empty() {
            sink.write_ok("usage: /btw <message>");
            return RunOutput::command(sink.transcript());
        }
        self.sink_echo_user(text, &mut sink);
        let snapshot = runner::build_btw_snapshot(&self.session, &self.turn_trace, false);
        let model = self.client.completion_model(self.session.model.to_string());
        let temperature = config::resolve_temperature(&self.cli, &self.cfg, &self.session.model);
        let extra_body = config::resolve_extra_body(&self.cfg, &self.session.model);
        let btw_agent = crate::provider::build_btw_agent(
            model,
            &self.cli,
            &self.cfg,
            &self.context,
            &self.permission,
            &None,
            self.reasoning_enabled,
            temperature,
            extra_body,
        );
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(32);
        let _runner = btw_agent.spawn_btw(
            btw_text.to_string(),
            snapshot,
            event_tx,
            0,
            self.cfg.retry.clone(),
        );
        // Await the single terminal event (no session mutation: `BtwEvent`
        // never touches history, enforced by the type).
        let mut answer = String::new();
        while let Some(ev) = event_rx.recv().await {
            match ev {
                crate::event::BtwEvent::Done { response, .. } => {
                    answer = response.to_string();
                    break;
                }
                crate::event::BtwEvent::Error { message, .. } => {
                    sink.write_error(message.to_string());
                    return RunOutput::command(sink.transcript());
                }
            }
        }
        sink.write_ok(&answer);
        RunOutput::command(sink.transcript())
    }

    // ── dot commands ──────────────────────────────────────────────────

    /// Mirror `App::handle_dot_command`: returns `None` when `text` is not a
    /// dot command (caller falls through to slash/bang/agent paths), else the
    /// output. `.prompt msg` rewrites the input into a plain message run.
    async fn handle_dot_command(&mut self, text: &str) -> anyhow::Result<Option<RunOutput>> {
        if !text.starts_with('.') {
            return Ok(None);
        }
        let mut sink = StringSink::new();
        self.sink_echo_user(text, &mut sink);
        let after_dot = text[1..].trim_start();
        if after_dot.is_empty() {
            sink.write_error("usage: .<prompt> [message]");
            return Ok(Some(RunOutput::command(sink.transcript())));
        }
        if let Some((prompt_name, msg)) = after_dot.split_once(char::is_whitespace) {
            let prompt_name = prompt_name.trim();
            let msg = msg.trim();
            if !prompt_name.is_empty() && self.context.prompts.contains_key(prompt_name) {
                self.dot_prompt_restore = self.context.current_prompt_name.clone();
                crate::ui::apply_prompt_mode(
                    prompt_name,
                    &mut self.context,
                    &mut self.session,
                    &self.permission,
                );
                self.apply_prompt_model(prompt_name, &mut sink).await;
                self.agent = None;
                // One-shot: run the remainder as a plain message.
                let mut out = self.run_agent_text(msg).await;
                let transcript = format!("{}\n{}", sink.transcript(), out.text);
                out.text = transcript;
                return Ok(Some(out));
            }
            sink.write_error(format!("unknown prompt '{prompt_name}'"));
            return Ok(Some(RunOutput::command(sink.transcript())));
        }
        let prompt_name = after_dot.trim();
        if self.context.prompts.contains_key(prompt_name) {
            crate::ui::apply_prompt_mode(
                prompt_name,
                &mut self.context,
                &mut self.session,
                &self.permission,
            );
            self.apply_prompt_model(prompt_name, &mut sink).await;
            self.agent = None;
            sink.write_ok(format!("switched to prompt '{prompt_name}'"));
            self.save_session_best_effort();
            Ok(Some(RunOutput::command(sink.transcript())))
        } else {
            sink.write_error(format!("unknown prompt '{prompt_name}'"));
            Ok(Some(RunOutput::command(sink.transcript())))
        }
    }

    // ── shared helpers ────────────────────────────────────────────────

    fn sink_echo_user(&self, text: &str, sink: &mut StringSink) {
        for line in text.lines() {
            sink.write_line(&format!("> {line}"));
        }
        sink.write_line("");
    }

    fn save_session_best_effort(&self) {
        if !self.cli.no_session
            && let Err(e) = crate::session::storage::save_session(&self.session)
        {
            tracing::warn!("failed to save session: {e}");
        }
    }

    fn reserve(&self) -> u64 {
        let qm = config::quick_models_map(&self.cfg);
        #[cfg(feature = "memory")]
        let reserve = crate::extras::memory::effective_reserve(
            self.cfg.resolve_reserve_tokens(&self.session.model, &qm),
            self.context.memory.as_deref(),
        );
        #[cfg(not(feature = "memory"))]
        let reserve = self.cfg.resolve_reserve_tokens(&self.session.model, &qm);
        reserve
    }

    /// Summarize `messages[..cut]` via the model and replace them with the
    /// summary. Mirrors `ui::slash::handle_compress`.
    async fn compress(
        &mut self,
        instructions: Option<&str>,
        auto: bool,
        sink: &mut StringSink,
    ) -> anyhow::Result<()> {
        let keep_recent = self.cfg.resolve_keep_recent_tokens();
        let max_tokens = self.session.context_window.saturating_sub(self.reserve());
        if auto && self.session.effective_context_tokens() <= max_tokens {
            return Ok(());
        }
        let cut_idx = Session::select_compaction_cut(&self.session.messages, keep_recent);
        if cut_idx == 0 {
            if !auto {
                sink.write_ok("not enough conversation history to compact yet");
            }
            return Ok(());
        }
        if auto {
            sink.write_result("auto-compacting...");
        } else {
            sink.write_ok("compressing...");
        }
        sink.write_line("");
        let messages_to_summarize = &self.session.messages[..cut_idx];
        let previous_summary = self.session.compactions.last().map(|c| c.summary.as_str());
        let summary = self
            .client
            .compress_messages(
                &self.session.model,
                messages_to_summarize,
                previous_summary,
                instructions,
            )
            .await?;
        let tokens_before: u64 = messages_to_summarize
            .iter()
            .map(|m| m.estimated_tokens)
            .sum();
        #[cfg(feature = "memory")]
        crate::extras::memory::flush_compaction_summary(
            &crate::extras::memory::Mem::open(),
            &summary,
            Some(cut_idx),
        );
        self.session.compress(summary, cut_idx, tokens_before);
        let model_id = self.session.model.to_string();
        self.rebuild_agent(&model_id).await;
        sink.write_ok(format!(
            "compressed {} messages (saved ~{} tokens)",
            cut_idx, tokens_before,
        ));
        Ok(())
    }

    fn push_pending_tool_call(&mut self, id: CompactString, session_call_id: u64) {
        if let Some(pos) = self.pending_tool_calls.iter().position(|(k, _)| *k == id) {
            self.pending_tool_calls.remove(pos);
        }
        self.pending_tool_calls.push((id, session_call_id));
    }

    fn take_pending_tool_call(&mut self, id: &str) -> Option<u64> {
        let pos = self.pending_tool_calls.iter().position(|(k, _)| k == id)?;
        Some(self.pending_tool_calls.remove(pos).1)
    }

    /// Pair a `ToolResult` with its call. Synthesizes an orphan call when rig
    /// streams a result with no matching `ToolCall` (mirrors
    /// `resolve_tool_result_call_id`).
    fn resolve_tool_result_call_id(&mut self, id: &str, name: &str) -> u64 {
        if let Some(call_id) = self.take_pending_tool_call(id) {
            return call_id;
        }
        tracing::warn!("ToolResult for {name} with no matching ToolCall; synthesizing orphan");
        self.session.add_tool_call(name, &serde_json::Value::Null)
    }

    fn restore_prompt(&mut self, restore_name: &str) {
        self.context.current_prompt = self.context.prompts.get(restore_name).cloned();
        self.context.current_prompt_name = if self.context.current_prompt.is_some() {
            Some(restore_name.to_string())
        } else {
            None
        };
        let extra = self.context.extra_prompts_dirs.clone();
        self.session.prompt =
            self.context
                .current_prompt_name
                .as_deref()
                .map(|name| crate::session::PromptRef {
                    name: name.into(),
                    source: crate::context::prompts::source_of_with_extra(name, &extra),
                });
        if let Some(perm) = &self.permission {
            perm.lock()
                .unwrap_or_else(|e| e.into_inner())
                .restore_user_mode();
        }
    }

    /// Switch to the quick-model mapped for `prompt_name` (mirrors
    /// `SlashCtx::switch_to_prompt_model`).
    async fn apply_prompt_model(&mut self, prompt_name: &str, sink: &mut StringSink) -> bool {
        let qm_name_owned: Option<String> = self
            .cfg
            .resolve_prompt_model(prompt_name)
            .map(|s| s.to_string());
        let Some(qm_name) = qm_name_owned else {
            return false;
        };
        let qm = config::quick_models_map(&self.cfg);
        let Some(qmc) = qm.get(qm_name.as_str()) else {
            return false;
        };
        // Clone out of the config map before any `&mut self` call.
        let qm_provider = qmc.provider.to_string();
        let qm_model = qmc.model.to_string();
        let qm_in = qmc.input_token_cost;
        let qm_out = qmc.output_token_cost;
        let new_model = CompactString::from(&qm_model);
        let provider_changed = qm_provider != self.session.provider.as_str();
        self.session.model = new_model.clone();
        if provider_changed {
            match self.rebuild_agent_with_client(&qm_provider).await {
                Ok(()) => {
                    self.session.provider = CompactString::from(&qm_provider);
                }
                Err(e) => {
                    sink.write_error(format!(
                        "failed to switch provider for prompt '{prompt_name}': {e}"
                    ));
                    return false;
                }
            }
        } else {
            self.rebuild_agent(&qm_model).await;
        }
        self.session.input_token_cost = qm_in;
        self.session.output_token_cost = qm_out;
        let qm2 = config::quick_models_map(&self.cfg);
        self.session
            .update_context_window(self.cfg.resolve_context_window(
                &self.session.provider,
                &self.session.model,
                &qm2,
            ));
        sink.write_ok(format!(
            "switched to model: {qm_name} (from prompt '{prompt_name}')"
        ));
        true
    }

    // ── slash command groups (mirrors ui::slash/*) ────────────────────

    async fn slash_providers(
        &mut self,
        parts: &[&str],
        sink: &mut StringSink,
    ) -> anyhow::Result<SlashFlow> {
        match parts[0] {
            "/provider" => self.cmd_provider(parts, sink).await,
            "/model" => self.cmd_model(parts, sink).await,
            "/models" => self.cmd_models(parts, sink).await,
            "/models-add" => self.cmd_models_add(parts, sink),
            #[cfg(feature = "subagents")]
            "/model-subagent" | "/models-subagent" => {
                sink.write_error("subagent model switching needs the TUI runtime");
                Ok(SlashFlow::Done)
            }
            _ => Ok(SlashFlow::Done),
        }
    }

    async fn cmd_provider(
        &mut self,
        parts: &[&str],
        sink: &mut StringSink,
    ) -> anyhow::Result<SlashFlow> {
        if parts.len() < 2 {
            sink.write_ok(format!("current provider: {}", self.session.provider));
            return Ok(SlashFlow::Done);
        }
        let new_provider = parts[1].trim();
        if crate::provider::parse_provider(new_provider).is_none()
            && !self.cfg.custom_providers_map().contains_key(new_provider)
        {
            sink.write_error(format!("unknown provider: '{new_provider}'"));
            return Ok(SlashFlow::Done);
        }
        if let Some((model, costs)) =
            crate::provider::default_model_for_provider(new_provider, &self.cfg)
        {
            self.session.model = CompactString::new(&model);
            if let Some((inc, outc)) = costs {
                self.session.input_token_cost = inc;
                self.session.output_token_cost = outc;
            }
        }
        self.rebuild_agent_with_client(new_provider).await?;
        self.session.provider = CompactString::new(new_provider);
        let qm = config::quick_models_map(&self.cfg);
        self.session
            .update_context_window(self.cfg.resolve_context_window(
                new_provider,
                &self.session.model,
                &qm,
            ));
        sink.write_ok(format!(
            "switched to provider: {} (model: {})",
            new_provider, self.session.model
        ));
        Ok(SlashFlow::Done)
    }

    async fn cmd_model(
        &mut self,
        parts: &[&str],
        sink: &mut StringSink,
    ) -> anyhow::Result<SlashFlow> {
        if parts.len() < 2 {
            sink.write_ok(format!("current model: {}", self.session.model));
            return Ok(SlashFlow::Done);
        }
        let new_model = CompactString::new(parts[1].trim());
        let new_model_str = new_model.to_string();
        self.rebuild_agent(&new_model_str).await;
        self.session.model = new_model.clone();
        self.session.provider = self.cli.resolve_provider(&self.cfg);
        let qm = config::quick_models_map(&self.cfg);
        self.session
            .update_context_window(self.cfg.resolve_context_window(
                &self.session.provider,
                &new_model,
                &qm,
            ));
        sink.write_ok(format!("switched to model: {new_model}"));
        Ok(SlashFlow::Done)
    }

    async fn cmd_models(
        &mut self,
        parts: &[&str],
        sink: &mut StringSink,
    ) -> anyhow::Result<SlashFlow> {
        let qm = config::quick_models_map(&self.cfg);
        // `/models <name>`: quick-model switch, else raw model id.
        if parts.len() >= 2 && parts.get(1).map(|s| s.trim()) != Some("refresh") {
            let arg = parts[1].trim();
            if let Some(q) = qm.get(arg) {
                let provider = q.provider.to_string();
                let model = q.model.to_string();
                let in_cost = q.input_token_cost;
                let out_cost = q.output_token_cost;
                self.rebuild_agent_with_client(&provider).await?;
                self.session.provider = CompactString::from(&provider);
                self.rebuild_agent(&model).await;
                self.session.model = CompactString::from(&model);
                let qm2 = config::quick_models_map(&self.cfg);
                self.session
                    .update_context_window(self.cfg.resolve_context_window(
                        &self.session.provider,
                        &self.session.model,
                        &qm2,
                    ));
                self.session.input_token_cost = in_cost;
                self.session.output_token_cost = out_cost;
                sink.write_ok(format!("switched to model: {model}"));
                return Ok(SlashFlow::Done);
            }
            let arg_owned = arg.to_string();
            self.rebuild_agent(&arg_owned).await;
            self.session.model = CompactString::new(arg);
            sink.write_ok(format!("switched to model: {arg}"));
            return Ok(SlashFlow::Done);
        }
        // List mode.
        let mut sorted: Vec<&String> = qm.keys().collect();
        sorted.sort();
        sink.write_ok(format!(
            "quick models (current: {} | {}):",
            self.session.provider, self.session.model
        ));
        if sorted.is_empty() {
            sink.write_result("  (none — add with /models-add)");
        }
        for name in &sorted {
            let q = &qm[name.as_str()];
            sink.write_result(format!(
                "  {}  ({} / {})  ${:.4}/M in  ${:.4}/M out",
                name, q.provider, q.model, q.input_token_cost, q.output_token_cost
            ));
        }
        Ok(SlashFlow::Done)
    }

    fn cmd_models_add(
        &mut self,
        parts: &[&str],
        sink: &mut StringSink,
    ) -> anyhow::Result<SlashFlow> {
        if parts.len() < 3 {
            sink.write_ok(
                "usage: /models-add <name> <provider> <model> [input_cost_per_M output_cost_per_M]",
            );
            return Ok(SlashFlow::Done);
        }
        let name = parts[1].trim().to_string();
        let rest = parts[2].trim();
        let (provider, model, input_cost, output_cost) = match rest.split_once(' ') {
            Some((p, m)) if parts.len() >= 5 => (
                p.trim().to_string(),
                m.trim().to_string(),
                parts[3].trim().parse::<f64>().unwrap_or(0.0),
                parts[4].trim().parse::<f64>().unwrap_or(0.0),
            ),
            Some((p, m)) => (p.trim().to_string(), m.trim().to_string(), 0.0, 0.0),
            None => {
                sink.write_ok(
                    "usage: /models-add <name> <provider> <model> [input_cost_per_M output_cost_per_M]",
                );
                return Ok(SlashFlow::Done);
            }
        };
        if name.is_empty() || provider.is_empty() || model.is_empty() {
            sink.write_ok(
                "usage: /models-add <name> <provider> <model> [input_cost_per_M output_cost_per_M]",
            );
            return Ok(SlashFlow::Done);
        }
        match config::save_quick_model(&name, &provider, &model, input_cost, output_cost) {
            Ok(()) => sink.write_ok(format!("saved quick model: {name} ({provider} / {model})")),
            Err(e) => sink.write_error(format!("failed to save quick model: {e}")),
        }
        Ok(SlashFlow::Done)
    }

    async fn slash_content(
        &mut self,
        parts: &[&str],
        sink: &mut StringSink,
    ) -> anyhow::Result<SlashFlow> {
        match parts[0] {
            "/prompt" => self.cmd_prompt(parts, sink).await,
            "/theme" => self.cmd_theme(parts, sink),
            "/regen-prompts" => {
                match crate::context::prompts::regen() {
                    Ok(()) => {
                        self.context.prompts = crate::context::prompts::load_with_extra(
                            &self.context.extra_prompts_dirs.clone(),
                        );
                        sink.write_ok("default prompts regenerated");
                    }
                    Err(e) => sink.write_error(format!("failed to regenerate prompts: {e}")),
                }
                Ok(SlashFlow::Done)
            }
            "/regen-themes" => {
                match crate::context::themes::regen() {
                    Ok(()) => {
                        self.context.themes = crate::context::themes::load();
                        sink.write_ok("default themes regenerated");
                    }
                    Err(e) => sink.write_error(format!("failed to regenerate themes: {e}")),
                }
                Ok(SlashFlow::Done)
            }
            _ => Ok(SlashFlow::Done),
        }
    }

    async fn cmd_prompt(
        &mut self,
        parts: &[&str],
        sink: &mut StringSink,
    ) -> anyhow::Result<SlashFlow> {
        let mut sorted: Vec<&String> = self.context.prompts.keys().collect();
        sorted.sort();
        if parts.len() < 2 {
            if sorted.is_empty() {
                sink.write_ok("no prompts available");
            } else {
                let current = self
                    .context
                    .current_prompt_name
                    .as_deref()
                    .unwrap_or("(none)");
                sink.write_ok(format!("available prompts (current: {current}):"));
                for name in &sorted {
                    sink.write_result(format!("  {name}"));
                }
                sink.write_result("usage: /prompt <name>  |  /prompt default");
            }
        } else if parts[1] == "default" {
            if self.context.current_prompt.is_none() {
                sink.write_ok("no active prompt to clear");
            } else {
                self.context.current_prompt = None;
                self.context.current_prompt_name = None;
                self.session.prompt = None;
                let default_model = self.cli.resolve_model(&self.cfg);
                let default_provider = self.cli.resolve_provider(&self.cfg);
                let provider_changed = default_provider != self.session.provider;
                self.session.model = default_model;
                if provider_changed {
                    let provider_str = default_provider.to_string();
                    if let Err(e) = self.rebuild_agent_with_client(&provider_str).await {
                        sink.write_error(format!("failed to revert provider: {e}"));
                    } else {
                        self.session.provider = default_provider;
                    }
                } else {
                    let model_id = self.session.model.to_string();
                    self.rebuild_agent(&model_id).await;
                }
                sink.write_ok("prompt cleared (back to default)");
            }
        } else {
            let name = parts[1].trim().to_string();
            if self.context.prompts.contains_key(&name) {
                match crate::ui::apply_prompt_mode(
                    &name,
                    &mut self.context,
                    &mut self.session,
                    &self.permission,
                ) {
                    crate::ui::PromptModeOutcome::RestoredUserMode => {
                        if let Some(perm) = &self.permission {
                            let current = perm
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .mode()
                                .to_string();
                            sink.write_ok(format!("restored user mode: {current}"));
                        }
                    }
                    crate::ui::PromptModeOutcome::Applied(mode) => {
                        sink.write_ok(format!("security mode: {mode} (from prompt)"));
                    }
                    crate::ui::PromptModeOutcome::None => {}
                }
                let model_switched = self.apply_prompt_model(&name, sink).await;
                if !model_switched {
                    let model_id = self.session.model.to_string();
                    self.rebuild_agent(&model_id).await;
                }
                sink.write_ok(format!("active prompt: {name}"));
            } else {
                sink.write_error(format!("unknown prompt: '{name}'"));
            }
        }
        Ok(SlashFlow::Done)
    }

    fn cmd_theme(&mut self, parts: &[&str], sink: &mut StringSink) -> anyhow::Result<SlashFlow> {
        let mut sorted: Vec<&String> = self.context.themes.keys().collect();
        sorted.sort();
        if parts.len() < 2 {
            if sorted.is_empty() {
                sink.write_ok("no themes available");
            } else {
                let current = self
                    .context
                    .current_theme_name
                    .as_deref()
                    .unwrap_or("(none)");
                sink.write_ok(format!("available themes (current: {current}):"));
                for name in &sorted {
                    sink.write_result(format!("  {name}"));
                }
            }
        } else if parts[1] == "default" {
            if self.context.current_theme_name.is_none() {
                sink.write_ok("no active theme to clear");
            } else {
                self.context.current_theme_name = None;
                if let Err(e) = crate::session::storage::save_theme_name(None) {
                    sink.write_error(format!("warning: theme choice not saved: {e}"));
                }
                sink.write_ok("theme cleared (using config colors)");
            }
        } else {
            let name = parts[1].trim();
            if self.context.themes.contains_key(name) {
                self.context.current_theme_name = Some(name.to_string());
                if let Err(e) = crate::session::storage::save_theme_name(Some(name)) {
                    sink.write_error(format!("warning: theme choice not saved: {e}"));
                }
                sink.write_ok(format!("active theme: {name}"));
            } else {
                sink.write_error(format!("unknown theme: '{name}'"));
            }
        }
        Ok(SlashFlow::Done)
    }

    async fn slash_settings(
        &mut self,
        parts: &[&str],
        sink: &mut StringSink,
    ) -> anyhow::Result<SlashFlow> {
        match parts[0] {
            "/reasoning" | "/thinking" => {
                self.reasoning_enabled = !self.reasoning_enabled;
                self.show_reasoning = self.reasoning_enabled;
                let model_id = self.session.model.to_string();
                self.rebuild_agent(&model_id).await;
                sink.write_ok(format!(
                    "reasoning: {}",
                    if self.reasoning_enabled { "on" } else { "off" }
                ));
                Ok(SlashFlow::Done)
            }
            "/mode" => {
                if parts.len() < 2 {
                    let current = self
                        .permission
                        .as_ref()
                        .map(|p| p.lock().unwrap_or_else(|e| e.into_inner()).mode())
                        .unwrap_or(SecurityMode::Standard);
                    sink.write_ok(format!("current security mode: {current}"));
                    return Ok(SlashFlow::Done);
                }
                let mode = match parts[1] {
                    "standard" => Some(SecurityMode::Standard),
                    "restrictive" => Some(SecurityMode::Restrictive),
                    "readonly" => Some(SecurityMode::ReadOnly),
                    "guarded" => Some(SecurityMode::Guarded),
                    "yolo" => Some(SecurityMode::Yolo),
                    _ => None,
                };
                match (mode, &self.permission) {
                    (Some(m), Some(p)) => {
                        p.lock().unwrap_or_else(|e| e.into_inner()).set_mode(m);
                        sink.write_ok(format!("security mode: {m}"));
                    }
                    (None, _) => sink.write_error(format!("unknown mode: {}", parts[1])),
                    (_, None) => sink.write_error("permission system not active"),
                }
                Ok(SlashFlow::Done)
            }
            "/toggle" => {
                sink.write_error("usage: /toggle is TUI-only (todo tools stay as configured)");
                Ok(SlashFlow::Done)
            }
            "/editsys" => {
                if parts.len() < 2 {
                    sink.write_ok(format!(
                        "edit system: {}",
                        crate::agent::tools::edit_system()
                    ));
                    return Ok(SlashFlow::Done);
                }
                match parts[1] {
                    "similarity" => {
                        crate::agent::tools::set_edit_system(
                            crate::config::types::EditSystem::Similarity,
                        );
                        sink.write_ok("edit system: similarity (SEARCH/REPLACE)");
                    }
                    "hashedit" => {
                        crate::agent::tools::set_edit_system(
                            crate::config::types::EditSystem::Hashedit,
                        );
                        sink.write_ok("edit system: hashedit (tag-based)");
                    }
                    _ => sink.write_error(format!("unknown: '{}' (similarity|hashedit)", parts[1])),
                }
                Ok(SlashFlow::Done)
            }
            "/advisor" => {
                #[cfg(feature = "advisor")]
                {
                    self.cmd_advisor(parts, sink).await
                }
                #[cfg(not(feature = "advisor"))]
                {
                    sink.write_error("Advisor support not enabled (build with --features advisor)");
                    Ok(SlashFlow::Done)
                }
            }
            #[cfg(feature = "mcp")]
            "/mcp" => {
                sink.write_error("/mcp needs the TUI runtime (connection notices render there)");
                Ok(SlashFlow::Done)
            }
            _ => Ok(SlashFlow::Done),
        }
    }

    #[cfg(feature = "advisor")]
    async fn cmd_advisor(
        &mut self,
        parts: &[&str],
        sink: &mut StringSink,
    ) -> anyhow::Result<SlashFlow> {
        use crate::extras::advisor;
        let current = advisor::with_config(|c| c.clone());
        if parts.len() < 2 {
            sink.write_ok(format!(
                "advisor: {} (model: {})",
                if current.enabled { "on" } else { "off" },
                current.advisor_model
            ));
            return Ok(SlashFlow::Done);
        }
        match parts[1] {
            "on" => {
                let mut cfg = current;
                cfg.enabled = true;
                advisor::init_config(cfg);
                let model_id = self.session.model.to_string();
                self.rebuild_agent(&model_id).await;
                sink.write_ok("advisor: on");
            }
            "off" => {
                let mut cfg = current;
                cfg.enabled = false;
                advisor::init_config(cfg);
                let model_id = self.session.model.to_string();
                self.rebuild_agent(&model_id).await;
                sink.write_ok("advisor: off");
            }
            _ => sink.write_error(format!("unknown: '{}' (on|off)", parts[1])),
        }
        Ok(SlashFlow::Done)
    }

    async fn slash_session(
        &mut self,
        parts: &[&str],
        sink: &mut StringSink,
    ) -> anyhow::Result<SlashFlow> {
        match parts[0] {
            "/sessions" => self.cmd_sessions(parts, sink),
            "/rename" => self.cmd_rename(parts, sink),
            "/clear" | "/new" => {
                #[cfg(feature = "hooks")]
                crate::extras::hooks::dispatch_session_end("clear").await;
                self.session.messages.clear();
                self.session.total_estimated_tokens = 0;
                self.session.reset_calibration();
                self.session.compactions.clear();
                self.context.chain_declined.clear();
                #[cfg(feature = "hooks")]
                crate::extras::hooks::dispatch_session_start("clear").await;
                sink.write_ok("session cleared");
                Ok(SlashFlow::Done)
            }
            "/undo" => {
                let removed = crate::ui::slash::undo_last(&mut self.session);
                if removed == 0 {
                    sink.write_ok("nothing to undo");
                } else {
                    sink.write_ok(format!("removed {removed} message(s)"));
                }
                Ok(SlashFlow::Done)
            }
            "/redo" => {
                if !self.session.redo() {
                    sink.write_ok("nothing to redo");
                } else {
                    sink.write_ok("restored the last rewind");
                }
                Ok(SlashFlow::Done)
            }
            "/rewind" => {
                sink.write_error(
                    "/rewind needs the TUI picker (no interactive selection headless)",
                );
                Ok(SlashFlow::Done)
            }
            "/retry" => {
                match self
                    .session
                    .messages
                    .iter()
                    .rev()
                    .find(|m| m.role == MessageRole::User)
                    .cloned()
                {
                    Some(msg) => {
                        sink.write_ok("retrying last message...");
                        let out = self.run_agent_text(&msg.content).await;
                        sink.write_ok(&out.text);
                        self.save_session_best_effort();
                        Ok(SlashFlow::Done)
                    }
                    None => {
                        sink.write_ok("no previous message to retry");
                        Ok(SlashFlow::Done)
                    }
                }
            }
            "/quit" | "/exit" => {
                anyhow::bail!("quit requested (headless engines do not exit the process)");
            }
            "/history" => {
                match crate::session::chat_history::load_history() {
                    Ok(entries) if entries.is_empty() => sink.write_ok("no chat history"),
                    Ok(entries) => {
                        sink.write_ok(format!("global chat history ({} entries):", entries.len()));
                        for entry in entries.iter().rev().take(10).rev() {
                            let preview: String = entry.content.chars().take(80).collect();
                            sink.write_result(format!("  {preview}"));
                        }
                    }
                    Err(e) => sink.write_error(format!("failed to load chat history: {e}")),
                }
                Ok(SlashFlow::Done)
            }
            #[cfg(feature = "export")]
            "/export" => self.cmd_export(parts, sink),
            #[cfg(feature = "export")]
            "/import" => self.cmd_import(parts, sink),
            #[cfg(feature = "export")]
            "/share" => {
                let filename = format!(
                    "zerostack-session-{}.html",
                    &self.session.id[..8.min(self.session.id.len())]
                );
                let html = crate::extras::export::session_to_html(&self.session);
                let description = if self.session.name.is_empty() {
                    "zerostack session".to_string()
                } else {
                    format!("zerostack session: {}", self.session.name)
                };
                match crate::extras::export::share_gist(&filename, &html, &description).await {
                    Ok(url) => sink.write_ok(format!("shared as secret gist: {url}")),
                    Err(e) => sink.write_error(format!("share failed: {e}")),
                }
                Ok(SlashFlow::Done)
            }
            _ => Ok(SlashFlow::Done),
        }
    }

    fn cmd_rename(&mut self, parts: &[&str], sink: &mut StringSink) -> anyhow::Result<SlashFlow> {
        if parts.len() < 2 || parts[1].is_empty() {
            sink.write_ok("usage: /rename <name>");
            return Ok(SlashFlow::Done);
        }
        let new_name = parts[1..].join(" ").trim().to_string();
        self.session.name = CompactString::new(&new_name);
        if let Err(e) = crate::session::storage::save_session(&self.session) {
            sink.write_error(format!("failed to save session: {e}"));
        } else {
            sink.write_ok(format!("session renamed to \"{new_name}\""));
        }
        Ok(SlashFlow::Done)
    }

    fn cmd_sessions(&mut self, parts: &[&str], sink: &mut StringSink) -> anyhow::Result<SlashFlow> {
        if parts.len() < 2 {
            match crate::session::storage::find_recent_sessions(20) {
                Ok(sessions) if sessions.is_empty() => sink.write_ok("no saved sessions"),
                Ok(sessions) => {
                    sink.write_ok(format!("recent sessions ({}):", sessions.len()));
                    for s in &sessions {
                        sink.write_result(Self::format_session_line(s));
                    }
                }
                Err(e) => sink.write_error(format!("failed to list sessions: {e}")),
            }
            return Ok(SlashFlow::Done);
        }
        if parts[1] == "delete" && parts.len() >= 3 {
            let prefix = parts[2].trim();
            match crate::session::storage::find_sessions_by_prefix(prefix) {
                Ok(sessions) if sessions.is_empty() => {
                    sink.write_ok(format!("no session matching '{prefix}'"));
                }
                Ok(sessions) if sessions.len() == 1 => {
                    let id = sessions.into_iter().next().unwrap().id.to_string();
                    match crate::session::storage::delete_session(&id) {
                        Ok(()) => sink.write_ok(format!("deleted session {}", &id[..8])),
                        Err(e) => sink.write_error(format!("failed to delete: {e}")),
                    }
                }
                Ok(sessions) => {
                    sink.write_ok(format!(
                        "multiple sessions match '{prefix}', be more specific"
                    ));
                    for s in &sessions {
                        sink.write_result(Self::format_session_line(s));
                    }
                }
                Err(e) => sink.write_error(format!("failed to list sessions: {e}")),
            }
            return Ok(SlashFlow::Done);
        }
        let prefix = parts[1].trim();
        match crate::session::storage::find_sessions_by_prefix(prefix) {
            Ok(sessions) if sessions.is_empty() => {
                sink.write_ok(format!("no session matching '{prefix}'"));
            }
            Ok(sessions) if sessions.len() == 1 => {
                let s = sessions.into_iter().next().unwrap();
                let msg_count = s.messages.len();
                self.session = s;
                sink.write_ok(format!("loaded session ({msg_count} msgs)"));
            }
            Ok(sessions) => {
                sink.write_ok(format!("multiple sessions match '{prefix}':"));
                for s in &sessions {
                    sink.write_result(Self::format_session_line(s));
                }
            }
            Err(e) => sink.write_error(format!("failed to list sessions: {e}")),
        }
        Ok(SlashFlow::Done)
    }

    fn format_session_line(s: &Session) -> String {
        let last = s
            .messages
            .last()
            .map(|m| format!("...{}", m.content.chars().take(30).collect::<String>()))
            .unwrap_or_default();
        format!(
            "  {}  {}msgs  {}  {}{}",
            &s.id[..8.min(s.id.len())],
            s.messages.len(),
            s.model,
            last,
            if s.name.is_empty() {
                String::new()
            } else {
                format!("  [{}]", s.name)
            }
        )
    }

    #[cfg(feature = "export")]
    fn cmd_export(&mut self, parts: &[&str], sink: &mut StringSink) -> anyhow::Result<SlashFlow> {
        let default_name = format!(
            "zerostack-session-{}.html",
            &self.session.id[..8.min(self.session.id.len())]
        );
        let path = parts
            .get(1)
            .map(|p| p.trim())
            .filter(|p| !p.is_empty())
            .unwrap_or(&default_name);
        let (content, kind) = if path.ends_with(".jsonl") {
            (
                crate::extras::export::session_to_jsonl(&self.session),
                "JSONL",
            )
        } else {
            (
                crate::extras::export::session_to_html(&self.session),
                "HTML",
            )
        };
        match std::fs::write(path, content) {
            Ok(()) => sink.write_ok(format!("exported {kind} to {path}")),
            Err(e) => sink.write_error(format!("export failed: {e}")),
        }
        Ok(SlashFlow::Done)
    }

    #[cfg(feature = "export")]
    fn cmd_import(&mut self, parts: &[&str], sink: &mut StringSink) -> anyhow::Result<SlashFlow> {
        let Some(path) = parts.get(1).map(|p| p.trim()).filter(|p| !p.is_empty()) else {
            sink.write_error("usage: /import <file.jsonl|session.json>");
            return Ok(SlashFlow::Done);
        };
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                sink.write_error(format!("failed to read {path}: {e}"));
                return Ok(SlashFlow::Done);
            }
        };
        let mut session = if content.trim_start().starts_with('{') {
            match serde_json::from_str::<Session>(&content) {
                Ok(s) => s,
                Err(e) => {
                    sink.write_error(format!("invalid session file: {e}"));
                    return Ok(SlashFlow::Done);
                }
            }
        } else {
            let messages = match crate::extras::export::parse_jsonl_import(&content) {
                Ok(m) => m,
                Err(e) => {
                    sink.write_error(format!("invalid JSONL session: {e}"));
                    return Ok(SlashFlow::Done);
                }
            };
            let name = std::path::Path::new(path)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "imported".to_string());
            let mut session = Session::new(
                self.session.provider.as_str(),
                self.session.model.as_str(),
                self.session.context_window,
                &name,
            );
            for msg in messages {
                session.add_message(msg.role, &msg.content);
            }
            session
        };
        if session.name.is_empty() {
            session.name = CompactString::new("imported");
        }
        let msg_count = session.messages.len();
        if let Err(e) = crate::session::storage::save_session(&session) {
            sink.write_error(format!("failed to save session: {e}"));
            return Ok(SlashFlow::Done);
        }
        self.session = session;
        sink.write_ok(format!("imported session from {path} ({msg_count} msgs)"));
        Ok(SlashFlow::Done)
    }

    fn slash_help(&self, _parts: &[&str], sink: &mut StringSink) {
        sink.write_ok("commands:");
        for line in [
            "  /add [path]            add file(s) to context",
            "  /drop <path>           remove file from context",
            "  /clear                 clear session",
            "  /provider [name]       show or switch provider",
            "  /model [name]          show or switch model",
            "  /models                list quick models",
            "  /sessions              list recent sessions",
            "  /rename <name>         rename the session",
            "  /undo /redo            undo / restore the last exchange",
            "  /retry                 retry the last message",
            "  /history               show global chat history",
            "  /prompt <name>         activate a prompt",
            "  /theme <name>          activate a theme",
            "  /reasoning             toggle reasoning",
            "  /mode <name>           switch security mode",
            "  /compress              compact the context",
            "  /init [force]          create AGENTS.md via the agent",
            "  /review [msg]          review via the agent",
            "  /help                  show this message",
            "  !<cmd>                 run a shell command",
            "  .<prompt> [msg]        switch prompt / one-shot run",
        ] {
            sink.write_result(line);
        }
    }

    async fn slash_add(
        &mut self,
        parts: &[&str],
        sink: &mut StringSink,
    ) -> anyhow::Result<SlashFlow> {
        match parts[0] {
            "/add" => self.cmd_add(parts, sink).await,
            "/drop" => self.cmd_drop(parts, sink).await,
            "/drop-all" => {
                let file_count = self.context.extra_files.len();
                #[cfg(feature = "multimodal")]
                let media_count = self.session.pending_media.len();
                #[cfg(not(feature = "multimodal"))]
                let media_count = 0;
                if file_count == 0 && media_count == 0 {
                    sink.write_ok("no files or media to drop");
                    return Ok(SlashFlow::Done);
                }
                if file_count > 0 {
                    self.context.extra_files.clear();
                    let model_id = self.session.model.to_string();
                    self.rebuild_agent(&model_id).await;
                }
                #[cfg(feature = "multimodal")]
                self.session.pending_media.clear();
                sink.write_ok(format!("dropped {file_count} file(s)"));
                Ok(SlashFlow::Done)
            }
            _ => Ok(SlashFlow::Done),
        }
    }

    async fn cmd_add(
        &mut self,
        parts: &[&str],
        sink: &mut StringSink,
    ) -> anyhow::Result<SlashFlow> {
        if parts.len() < 2 {
            if self.context.extra_files.is_empty() {
                sink.write_ok("no files added (use /add <path>)");
            } else {
                sink.write_ok("added files:");
                for f in &self.context.extra_files.clone() {
                    let size = std::fs::metadata(f).map(|m| m.len()).unwrap_or(0);
                    sink.write_result(format!("  {} ({size}B)", f.display()));
                }
            }
            return Ok(SlashFlow::Done);
        }
        let path = Self::resolve_path(parts[1]);
        if !path.exists() {
            sink.write_error(format!("file not found: {}", path.display()));
            return Ok(SlashFlow::Done);
        }
        if !path.is_file() {
            sink.write_error(format!("not a file: {}", path.display()));
            return Ok(SlashFlow::Done);
        }
        #[cfg(feature = "multimodal")]
        if crate::extras::multimodal::detect_media(&path).is_some() {
            match crate::extras::multimodal::load_attachment(&path) {
                Ok(attachment) => {
                    let size = attachment.size();
                    self.session.pending_media.push(attachment);
                    sink.write_ok(format!("attached: {} ({size}B)", path.display()));
                }
                Err(e) => sink.write_error(format!("failed to load media: {e}")),
            }
            return Ok(SlashFlow::Done);
        }
        let canonical = path.canonicalize().unwrap_or(path);
        if self.context.extra_files.contains(&canonical) {
            sink.write_ok(format!("already added: {}", canonical.display()));
            return Ok(SlashFlow::Done);
        }
        let size = std::fs::metadata(&canonical).map(|m| m.len()).unwrap_or(0);
        self.context.extra_files.push(canonical.clone());
        let model_id = self.session.model.to_string();
        self.rebuild_agent(&model_id).await;
        sink.write_ok(format!("added: {} ({size}B)", canonical.display()));
        Ok(SlashFlow::Done)
    }

    async fn cmd_drop(
        &mut self,
        parts: &[&str],
        sink: &mut StringSink,
    ) -> anyhow::Result<SlashFlow> {
        if parts.len() < 2 {
            sink.write_error("usage: /drop <path-or-index>");
            return Ok(SlashFlow::Done);
        }
        let path = Self::resolve_path(parts[1]);
        let canonical = path.canonicalize().unwrap_or(path);
        if let Some(i) = self
            .context
            .extra_files
            .iter()
            .position(|f| f == &canonical)
        {
            self.context.extra_files.remove(i);
            let model_id = self.session.model.to_string();
            self.rebuild_agent(&model_id).await;
            sink.write_ok(format!("dropped: {}", canonical.display()));
            return Ok(SlashFlow::Done);
        }
        sink.write_error(format!(
            "not in context: {} (use /add to see)",
            canonical.display()
        ));
        Ok(SlashFlow::Done)
    }

    fn resolve_path(s: &str) -> std::path::PathBuf {
        let p = std::path::PathBuf::from(s);
        if p.is_absolute() {
            p
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."))
                .join(p)
        }
    }

    async fn slash_init(
        &mut self,
        parts: &[&str],
        sink: &mut StringSink,
    ) -> anyhow::Result<SlashFlow> {
        let force = parts.len() >= 2 && parts[1] == "force";
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let agents_path = cwd.join("AGENTS.md");
        let arch_path = cwd.join("ARCHITECTURE.md");
        let agents_exists = agents_path.exists();
        let arch_exists = arch_path.exists();
        // Headless: no stdin prompt — require `force`, else explain.
        let (create_agents, create_arch) = if force {
            (true, true)
        } else {
            sink.write_ok("AGENTS.md / ARCHITECTURE.md check (headless: pass `force` to create):");
            sink.write_result(format!(
                "  AGENTS.md: {}",
                if agents_exists { "exists" } else { "missing" }
            ));
            sink.write_result(format!(
                "  ARCHITECTURE.md: {}",
                if arch_exists { "exists" } else { "missing" }
            ));
            sink.write_result("  usage: /init force");
            return Ok(SlashFlow::Done);
        };
        if create_arch {
            #[cfg(feature = "archmd")]
            {
                if arch_exists {
                    let _ = std::fs::remove_file(&arch_path);
                }
                match crate::extras::archmd::create_architecture_template(&cwd) {
                    Ok(()) => sink.write_ok(format!(
                        "Created {}/ARCHITECTURE.md — edit it to describe the codebase architecture.",
                        cwd.display()
                    )),
                    Err(e) => {
                        sink.write_error(format!("Failed to create ARCHITECTURE.md: {e}"));
                    }
                }
            }
            #[cfg(not(feature = "archmd"))]
            {
                sink.write_error("ARCHITECTURE.md creation requires the 'archmd' feature.");
            }
        }
        if create_agents {
            if !self.context.prompts.contains_key("code") {
                sink.write_error("no 'code' prompt found. Run /regen-prompts first.");
                return Ok(SlashFlow::Done);
            }
            sink.write_ok("delegating AGENTS.md creation to agent...");
            return Ok(SlashFlow::DeferInit);
        }
        Ok(SlashFlow::Done)
    }

    async fn slash_review(
        &mut self,
        parts: &[&str],
        sink: &mut StringSink,
    ) -> anyhow::Result<SlashFlow> {
        if !self.context.prompts.contains_key("review") {
            sink.write_error("no 'review' prompt found. Run /regen-prompts first.");
            return Ok(SlashFlow::Done);
        }
        let msg = if parts.len() > 1 {
            parts[1..].join(" ")
        } else {
            let session_empty = !self
                .session
                .messages
                .iter()
                .any(|m| m.role == MessageRole::User);
            if session_empty {
                "Review the current codebase for correctness, design, testing, and security."
                    .to_string()
            } else {
                "Review the changes discussed in this session for correctness, design, testing, and security.".to_string()
            }
        };
        self.context.one_shot_restore = self.context.current_prompt_name.clone();
        crate::ui::apply_prompt_mode(
            "review",
            &mut self.context,
            &mut self.session,
            &self.permission,
        );
        let model_switched = self.apply_prompt_model("review", sink).await;
        if !model_switched {
            let model_id = self.session.model.to_string();
            self.rebuild_agent(&model_id).await;
        }
        sink.write_ok(format!("review: {msg}"));
        Ok(SlashFlow::DeferReview { message: msg })
    }

    async fn slash_memory(
        &mut self,
        #[cfg_attr(not(feature = "memory"), allow(unused_variables))] parts: &[&str],
        sink: &mut StringSink,
    ) -> anyhow::Result<SlashFlow> {
        #[cfg(not(feature = "memory"))]
        {
            sink.write_error("/memory is not available in this build");
            return Ok(SlashFlow::Done);
        }
        #[cfg(feature = "memory")]
        {
            use crate::extras::memory::{Mem, WriteMode, WriteTarget};
            match parts.get(1).copied() {
                None | Some("status") => {
                    let mem = Mem::open();
                    sink.write_ok("memory status:");
                    let long_term = mem.memory_md();
                    sink.write_result(format!(
                        "  MEMORY.md: {}",
                        if long_term.exists() {
                            "exists"
                        } else {
                            "(not created)"
                        }
                    ));
                }
                Some("search") => {
                    if parts.len() < 3 {
                        sink.write_error("usage: /memory search <query>");
                        return Ok(SlashFlow::Done);
                    }
                    let query = parts[2..].join(" ");
                    let rendered = Mem::open().search(&query).render(4000);
                    sink.write_ok("search results:");
                    for line in rendered.lines() {
                        sink.write_result(line);
                    }
                }
                Some("read") => {
                    if parts.len() < 3 {
                        sink.write_error("usage: /memory read <source> [name]");
                        return Ok(SlashFlow::Done);
                    }
                    let mem = Mem::open();
                    let source = parts[2].to_lowercase();
                    let path = match source.as_str() {
                        "long_term" | "long" => Some(mem.memory_md()),
                        "scratchpad" => Some(mem.scratchpad()),
                        "daily" => Some(mem.daily_file(&mem.today)),
                        "note" => parts.get(3).and_then(|n| mem.note_path(n)),
                        _ => {
                            sink.write_error(format!("unknown source: {source}"));
                            None
                        }
                    };
                    if let Some(p) = path {
                        match std::fs::read_to_string(&p) {
                            Ok(s) => {
                                sink.write_ok(format!("{} ({source}):", p.display()));
                                for line in s.lines().take(200) {
                                    sink.write_result(line);
                                }
                            }
                            Err(e) => sink.write_error(format!("read error: {e}")),
                        }
                    }
                }
                Some("write") => {
                    if parts.len() < 4 {
                        sink.write_error("usage: /memory write <target> <content>");
                        return Ok(SlashFlow::Done);
                    }
                    let mem = Mem::open();
                    let target_str = parts[2].to_lowercase();
                    let content = parts[3..].join(" ");
                    let (target, name) = if let Some(note_name) = target_str.strip_prefix("note:") {
                        (WriteTarget::Note, Some(note_name))
                    } else {
                        match target_str.as_str() {
                            "long_term" | "long" => (WriteTarget::LongTerm, None),
                            "scratchpad" => (WriteTarget::Scratchpad, None),
                            "daily" => (WriteTarget::Daily, None),
                            _ => {
                                sink.write_error(format!("unknown target: {target_str}"));
                                return Ok(SlashFlow::Done);
                            }
                        }
                    };
                    match mem.write(target, &content, WriteMode::Append, name) {
                        Ok(msg) => sink.write_ok(msg),
                        Err(e) => sink.write_error(format!("write error: {e}")),
                    }
                }
                Some("editor") => {
                    sink.write_error("/memory editor needs an interactive terminal");
                }
                Some("clear") => {
                    if parts.len() < 3 {
                        sink.write_error("usage: /memory clear scratchpad|daily");
                        return Ok(SlashFlow::Done);
                    }
                    let mem = Mem::open();
                    let target = parts[2].to_lowercase();
                    let wt = match target.as_str() {
                        "scratchpad" => Some(WriteTarget::Scratchpad),
                        "daily" => Some(WriteTarget::Daily),
                        _ => {
                            sink.write_error("clear only supports: scratchpad, daily");
                            None
                        }
                    };
                    if let Some(wt) = wt {
                        match mem.write(wt, "", WriteMode::Overwrite, None) {
                            Ok(msg) => sink.write_ok(msg),
                            Err(e) => sink.write_error(format!("clear error: {e}")),
                        }
                    }
                }
                _ => sink.write_error("usage: /memory [status|search|read|write|editor|clear]"),
            }
            Ok(SlashFlow::Done)
        }
    }

    async fn slash_features(
        &mut self,
        parts: &[&str],
        sink: &mut StringSink,
    ) -> anyhow::Result<SlashFlow> {
        match parts[0] {
            "/compress" | "/compact" => {
                let instructions = if parts.len() > 1 {
                    let s = parts[1..].join(" ");
                    let trimmed = s.trim();
                    if trimmed.is_empty() || trimmed == "(none)" {
                        None
                    } else {
                        Some(trimmed.to_string())
                    }
                } else {
                    None
                };
                Ok(SlashFlow::DeferCompress { instructions })
            }
            "/loop" => {
                #[cfg(feature = "loop")]
                {
                    if parts.len() < 2 || parts[1] == "status" {
                        sink.write_error("loop mode needs the TUI event loop in this build");
                    } else if parts[1] == "stop" {
                        sink.write_ok("no active loop");
                    } else {
                        sink.write_error("/loop iterations run in the TUI; headless uses --loop");
                    }
                    Ok(SlashFlow::Done)
                }
                #[cfg(not(feature = "loop"))]
                {
                    sink.write_error("/loop requires the 'loop' feature");
                    Ok(SlashFlow::Done)
                }
            }
            "/worktree" | "/wt-merge" | "/wt-exit" => {
                #[cfg(feature = "git-worktree")]
                {
                    self.cmd_worktree(parts, sink).await
                }
                #[cfg(not(feature = "git-worktree"))]
                {
                    sink.write_error("worktree support not enabled in this build");
                    Ok(SlashFlow::Done)
                }
            }
            _ => Ok(SlashFlow::Done),
        }
    }

    #[cfg(feature = "git-worktree")]
    async fn cmd_worktree(
        &mut self,
        parts: &[&str],
        sink: &mut StringSink,
    ) -> anyhow::Result<SlashFlow> {
        match parts[0] {
            "/worktree" => {
                if parts.len() < 2 {
                    sink.write_error("usage: /worktree <name>");
                    return Ok(SlashFlow::Done);
                }
                let name = parts[1].trim();
                if let Err(e) = crate::extras::git_worktree::validate_branch_name(name) {
                    sink.write_error(e);
                    return Ok(SlashFlow::Done);
                }
                let wt_base_dir = self.cli.resolve_wt_base_dir(&self.cfg);
                match crate::extras::git_worktree::create(name, wt_base_dir.as_deref()) {
                    Ok((path, _info)) => {
                        if let Err(e) = std::env::set_current_dir(&path) {
                            sink.write_error(format!("failed to change directory: {e}"));
                            return Ok(SlashFlow::Done);
                        }
                        self.session.working_dir = CompactString::new(path.to_string_lossy());
                        #[cfg(feature = "git-worktree")]
                        self.context.reload();
                        crate::ui::apply_current_prompt_mode(&mut self.context, &self.permission);
                        let model_id = self.session.model.to_string();
                        self.rebuild_agent(&model_id).await;
                        sink.write_ok(format!(
                            "worktree created: branch '{name}' at {}",
                            path.display()
                        ));
                    }
                    Err(e) => sink.write_error(format!("failed: {e}")),
                }
                Ok(SlashFlow::Done)
            }
            "/wt-merge" | "/wt-exit" => {
                sink.write_error(format!(
                    "{} spawns a merge agent on the TUI loop; headless cannot run it",
                    parts[0]
                ));
                Ok(SlashFlow::Done)
            }
            _ => Ok(SlashFlow::Done),
        }
    }

    #[cfg(feature = "hooks")]
    fn slash_hooks(&self, sink: &mut StringSink) {
        match crate::extras::hooks::get_dispatcher() {
            None => sink.write_ok("hooks: no dispatcher installed"),
            Some(dispatcher) => {
                let summary = dispatcher.summary();
                if summary.is_empty() {
                    sink.write_ok("hooks: enabled, no hooks configured");
                } else {
                    sink.write_ok("hooks: configured events");
                    for (event, count) in summary {
                        sink.write_result(format!(
                            "  {event}: {count} handler{}",
                            if count == 1 { "" } else { "s" }
                        ));
                    }
                }
            }
        }
    }

    fn handle_queue(&mut self, text: &str, sink: &mut StringSink) {
        // Headless engines run one input at a time; there is never a queue.
        let arg = text.strip_prefix("/queue").unwrap_or("").trim();
        match arg {
            "" | "ls" | "list" => sink.write_ok("queue is empty"),
            "clear" | "pop" => sink.write_ok("queue is empty"),
            _ => sink.write_error("usage: /queue [ls|clear|pop]"),
        }
    }
}

/// Control flow out of a slash handler: either finished, or a deferred
/// agent-spawning outcome (mirrors `SlashOutcome` minus the TUI-only ones).
#[derive(Debug)]
enum SlashFlow {
    Done,
    DeferCompress { instructions: Option<String> },
    DeferInit,
    DeferReview { message: String },
}

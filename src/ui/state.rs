//! Grouped TUI state. The `App` used to carry ~40 flat fields and pass 10-20
//! of them into every helper; these structs group that state by lifetime and
//! purpose so helpers take a handful of coherent bundles instead.

use std::collections::VecDeque;

use compact_str::CompactString;
use tokio::sync::mpsc;

use crate::cli::Cli;
use crate::config::Config;
use crate::context::ContextFiles;
use crate::event::AgentEvent;
#[cfg(feature = "mcp")]
use crate::extras::mcp::McpClientManager;
use crate::extras::status_signals::StatusSignals;
use crate::permission::ask::AskSender;
use crate::permission::checker::PermCheck;
use crate::provider::{AnyAgent, AnyClient};
use crate::sandbox::Sandbox;
use crate::session::Session;

/// Shared resources every part of the TUI reaches for: static config, the
/// session, context files, the provider client, and the capability handles
/// needed to (re)build agents.
pub(crate) struct UiContext<'a> {
    pub cli: &'a Cli,
    pub cfg: &'a Config,
    pub session: &'a mut Session,
    pub context: &'a mut ContextFiles,
    pub client: AnyClient,
    pub permission: Option<PermCheck>,
    pub ask_tx: Option<AskSender>,
    pub sandbox: Sandbox,
    pub status_signals: Option<StatusSignals>,
    #[cfg(feature = "mcp")]
    pub mcp_manager: Option<McpClientManager>,
}

impl<'a> UiContext<'a> {
    /// Borrow the pieces [`AgentBuildCtx::rebuild_agent`] needs.
    pub(crate) fn agent_build_ctx(&self) -> AgentBuildCtx<'_> {
        AgentBuildCtx {
            cli: self.cli,
            cfg: self.cfg,
            context: self.context,
            client: &self.client,
            permission: &self.permission,
            ask_tx: &self.ask_tx,
            sandbox: &self.sandbox,
            #[cfg(feature = "mcp")]
            mcp_manager: self.mcp_manager.as_ref(),
        }
    }

    /// Composition root: built once in `main` and threaded through the TUI.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        cli: &'a Cli,
        cfg: &'a Config,
        session: &'a mut Session,
        context: &'a mut ContextFiles,
        client: AnyClient,
        permission: Option<PermCheck>,
        ask_tx: Option<AskSender>,
        sandbox: Sandbox,
        status_signals: Option<StatusSignals>,
    ) -> Self {
        Self {
            cli,
            cfg,
            session,
            context,
            client,
            permission,
            ask_tx,
            sandbox,
            status_signals,
            #[cfg(feature = "mcp")]
            mcp_manager: None,
        }
    }
}

/// Everything needed to (re)build the main agent, borrowed from whichever
/// state bundle the caller has: [`UiContext`] in the main loop and mid-turn
/// compaction, `SlashCtx` in slash commands, or owned clones in the startup
/// prebuild task. Centralizes the per-model resolution (completion model,
/// temperature, extra_body) and the `build_agent` call itself so every
/// rebuild path stays in sync.
pub(crate) struct AgentBuildCtx<'a> {
    pub cli: &'a Cli,
    pub cfg: &'a Config,
    pub context: &'a ContextFiles,
    pub client: &'a AnyClient,
    pub permission: &'a Option<PermCheck>,
    pub ask_tx: &'a Option<AskSender>,
    pub sandbox: &'a Sandbox,
    #[cfg(feature = "mcp")]
    pub mcp_manager: Option<&'a McpClientManager>,
}

impl AgentBuildCtx<'_> {
    /// Build the main agent for `model_id` (usually `session.model`; model
    /// switches pass the not-yet-committed new id).
    pub(crate) async fn rebuild_agent(&self, model_id: &str, reasoning_enabled: bool) -> AnyAgent {
        let model = self.client.completion_model(model_id.to_string());
        let temperature = crate::config::resolve_temperature(self.cli, self.cfg, model_id);
        let extra_body = crate::config::resolve_extra_body(self.cfg, model_id);
        crate::provider::build_agent(
            model,
            self.cli,
            self.cfg,
            self.context,
            self.permission.clone(),
            self.ask_tx.clone(),
            self.sandbox.clone(),
            reasoning_enabled,
            temperature,
            extra_body,
            #[cfg(feature = "mcp")]
            self.mcp_manager,
        )
        .await
    }
}

/// Transient state of the main agent run: the agent handle, its event
/// stream and abort handle, queued user input, and streaming-response scratch.
#[derive(Default)]
pub(crate) struct AgentRunState {
    pub agent: Option<AnyAgent>,
    pub is_running: bool,
    pub agent_rx: Option<mpsc::Receiver<AgentEvent>>,
    pub main_abort: Option<tokio::task::AbortHandle>,
    pub pending_inputs: VecDeque<String>,
    pub agent_line_started: bool,
    pub response_buf: String,
    pub response_start_block: Option<usize>,
    pub pending_send: Option<String>,
    pub was_reasoning: bool,
    pub turn_trace: Vec<CompactString>,
    pub awaiting_compaction_relief: bool,
    /// In-flight `ToolCall` events in arrival order: each event's id (rig's
    /// `internal_call_id`) paired with the id `Session::add_tool_call` stamped
    /// for it, removed when the matching `ToolResult` event arrives. Several
    /// are pending at once because providers may emit a whole batch of
    /// parallel tool calls (every `ToolCall` event first, then their results),
    /// so this cannot be a single slot; arrival order is kept so
    /// [`newest_pending_tool_call`](AgentRunState::newest_pending_tool_call)
    /// can answer. Reach it only through the methods below, which maintain
    /// both invariants (unique ids, newest last).
    pub pending_tool_calls: Vec<(CompactString, u64)>,
}

impl AgentRunState {
    /// Record `session_call_id` as the pending call for event id `id`. A
    /// repeated id violates rig's uniqueness guarantee; the newer call is the
    /// live one, so it replaces the stale entry and becomes the newest.
    pub(crate) fn push_pending_tool_call(&mut self, id: CompactString, session_call_id: u64) {
        if let Some(pos) = self.pending_tool_calls.iter().position(|(k, _)| *k == id) {
            tracing::warn!(
                "ToolCall id={escaped} was already in flight; replacing its pending entry \
                 (rig internal_call_ids are expected to be unique)",
                escaped = id.escape_debug(),
            );
            self.pending_tool_calls.remove(pos);
        }
        self.pending_tool_calls.push((id, session_call_id));
    }

    /// Consume the session call id recorded for event id `id`, if any.
    pub(crate) fn take_pending_tool_call(&mut self, id: &str) -> Option<u64> {
        let pos = self.pending_tool_calls.iter().position(|(k, _)| k == id)?;
        Some(self.pending_tool_calls.remove(pos).1)
    }

    /// Session call id of the newest call still awaiting its result, or `None`
    /// when nothing is in flight. `SubagentToolCall` events attach to it: the
    /// subagent side channel carries no call id, so this is a best-effort
    /// heuristic — under a parallel batch containing a `task` call it can
    /// attribute a subagent call to a sibling. (`run_print` records the
    /// opposite heuristic, attaching queued subagent calls to the batch's
    /// first-arriving result; both stay best-effort until the side channel
    /// carries a call id.)
    #[cfg(any(feature = "subagents", feature = "acp"))]
    pub(crate) fn newest_pending_tool_call(&self) -> Option<u64> {
        self.pending_tool_calls.last().map(|&(_, call_id)| call_id)
    }

    /// Drop every in-flight entry. Called from run teardown: a clean turn
    /// consumes them all, but an abort or a mid-stream error strands some, and
    /// a stranded entry must not be visible to the next run.
    pub(crate) fn clear_pending_tool_calls(&mut self) {
        self.pending_tool_calls.clear();
    }
}

/// What happens when the current run finishes: chained prompts, dot-prompt
/// restore, /loop iterations, and worktree-merge returns.
#[derive(Default)]
pub(crate) struct ChainState {
    pub pending: Option<crate::extras::chain::ChainPhase>,
    pub label_msg: Option<String>,
    pub dot_prompt_restore: Option<String>,
    pub loop_label: Option<String>,
    #[cfg(feature = "loop")]
    pub loop_state: Option<crate::extras::r#loop::LoopState>,
    #[cfg(feature = "git-worktree")]
    pub wt_return_path: Option<(String, String, String, bool)>,
}

/// User-facing feature toggles owned by slash commands.
pub(crate) struct SlashState {
    pub show_reasoning: bool,
    pub reasoning_enabled: bool,
    pub todo_tools_enabled: bool,
}

/// Provider-reported token usage for one finished turn.
#[derive(Clone, Copy, Default)]
pub(crate) struct TurnUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
}

/// /btw aggregate stats shown in the statusline.
#[derive(Clone, Copy, Default)]
pub(crate) struct BtwStats {
    pub cost: f64,
    pub input: u64,
    pub output: u64,
}

/// Parameters for a worktree merge-and-return run.
#[cfg(feature = "git-worktree")]
pub(crate) struct MergeRequest<'a> {
    pub branch: &'a str,
    pub target: &'a str,
    pub main_path: &'a str,
    pub wt_path: &'a str,
    pub force: bool,
}

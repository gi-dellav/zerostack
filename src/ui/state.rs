//! Grouped TUI state. The `App` used to carry ~40 flat fields and pass 10-20
//! of them into every helper; these structs group that state by lifetime and
//! purpose so helpers take a handful of coherent bundles instead.

use std::collections::VecDeque;

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
    pub turn_trace: Vec<compact_str::CompactString>,
    pub awaiting_compaction_relief: bool,
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

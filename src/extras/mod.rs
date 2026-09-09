/// Iterative --loop mode: plan-driven autonomous agent iterations.
#[cfg(feature = "loop")]
pub mod r#loop;

/// Git worktree isolation for parallel agent sessions.
#[cfg(feature = "git-worktree")]
pub mod git_worktree;

/// MCP client: external tool servers over stdio/HTTP.
#[cfg(feature = "mcp")]
pub mod mcp;

/// ACP server mode for editor/agent-protocol integration.
#[cfg(feature = "acp")]
pub mod acp;

/// Persistent cross-session memory tools and compaction hooks.
#[cfg(feature = "memory")]
pub mod memory;

/// Parallel subagent delegation via the task tool.
#[cfg(feature = "subagents")]
pub mod subagents;

/// ARCHITECTURE.md detection, prompting, and template creation.
#[cfg(feature = "archmd")]
pub mod archmd;

/// Session export to HTML/JSONL, import, and sharing.
#[cfg(feature = "export")]
pub mod export;

/// External-model advisor consultation tool (/advisor).
#[cfg(feature = "advisor")]
pub mod advisor;

/// Lifecycle hooks: Pre/PostToolUse, Stop, and session events.
#[cfg(feature = "hooks")]
pub mod hooks;

/// Brainstorm-plan-code prompt chaining state machine.
pub mod chain;
/// Image/PDF ingestion for multimodal-capable models.
#[cfg(feature = "multimodal")]
pub mod multimodal;

/// Unix-socket start/stop/git-conflict signals for status bars.
pub mod status_signals;

/// LSP diagnostics integration for edited files.
#[cfg(feature = "lsp")]
pub mod lsp;

/// rtk shell-output compression proxy for bash commands.
#[cfg(feature = "rtk")]
pub mod rtk;

/// Char-boundary-safe truncation helpers (CJK-aware).
pub(crate) mod truncate;

// Re-exported for the headless engine's turn-trace summaries.
pub(crate) use truncate::truncate_cjk;

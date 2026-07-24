#[cfg(feature = "loop")]
pub mod r#loop;

#[cfg(feature = "git-worktree")]
pub mod git_worktree;

#[cfg(feature = "mcp")]
pub mod mcp;

#[cfg(feature = "acp")]
pub mod acp;

#[cfg(feature = "memory")]
pub mod memory;

#[cfg(feature = "subagents")]
pub mod subagents;

#[cfg(feature = "archmd")]
pub mod archmd;

#[cfg(feature = "export")]
pub mod export;

#[cfg(feature = "advisor")]
pub mod advisor;

#[cfg(feature = "hooks")]
pub mod hooks;

pub mod chain;
#[cfg(feature = "multimodal")]
pub mod multimodal;

pub mod status_signals;

pub(crate) mod truncate;

//! Integration with rtk (https://github.com/rtk-ai/rtk), an external CLI proxy
//! that filters and compresses shell command output before it reaches the LLM
//! context.
//!
//! When enabled via `[rtk] enabled = true`, bash commands are passed through
//! `rtk rewrite <cmd>` before execution. rtk is the single source of truth for
//! which commands have a compact equivalent: it prints the rewritten command
//! on stdout (exit code 0 or 3 depending on version), or nothing (exit code 1)
//! when the command has no rtk filter. Everything here is fail-open — any
//! error (binary missing, timeout, empty output) runs the original command
//! unchanged.

use std::process::Stdio;
use std::time::Duration;

use crate::config::types::RtkConfig;

/// rtk advertises <10ms overhead; this bound only guards against a hung
/// binary. On expiry the original command runs unfiltered.
const RTK_TIMEOUT: Duration = Duration::from_secs(5);

/// A detected, working rtk binary. Construct via [`Rtk::detect`].
#[derive(Debug, Clone)]
pub struct Rtk {
    path: String,
}

impl Rtk {
    /// Returns `Some` when rtk is enabled in config and the binary responds to
    /// `<path> --version`. Logs a warning and returns `None` when the binary
    /// is enabled but unusable, so misconfiguration never blocks the agent.
    pub async fn detect(cfg: Option<&RtkConfig>) -> Option<Self> {
        let cfg = cfg.filter(|c| c.enabled)?;
        let path = cfg.path.as_deref().unwrap_or("rtk").to_string();
        let probe = tokio::process::Command::new(&path)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        match tokio::time::timeout(RTK_TIMEOUT, probe).await {
            Ok(Ok(status)) if status.success() => {
                tracing::info!("rtk output filtering enabled (binary: {})", path);
                Some(Self { path })
            }
            _ => {
                tracing::warn!(
                    "rtk.enabled is set but '{}' is not a working rtk binary; \
                     commands will run unfiltered",
                    path
                );
                None
            }
        }
    }

    /// Rewrites `command` via `rtk rewrite`. Returns `None` — meaning "run the
    /// original" — when rtk has no filter for the command or anything fails.
    /// The rewritten form only ever wraps the command in `rtk ...` calls, so
    /// permission checking stays on the original command text.
    pub async fn rewrite(&self, command: &str) -> Option<String> {
        let child = tokio::process::Command::new(&self.path)
            .arg("rewrite")
            .arg(command)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        let output = tokio::time::timeout(RTK_TIMEOUT, child).await.ok()?.ok()?;
        let rewritten = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if rewritten.is_empty() {
            None
        } else {
            Some(rewritten)
        }
    }
}

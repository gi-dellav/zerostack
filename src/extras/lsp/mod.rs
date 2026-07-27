//! LSP (Language Server Protocol) integration: spawns language servers for
//! files the agent edits and feeds diagnostics back into tool results.
//!
//! Enabled via `[lsp] enabled = true` (requires the `lsp` cargo feature).
//! Everything is fail-open: a missing server binary, a hung handshake, or a
//! crashed server only means "no diagnostics", never a failed edit.

pub(crate) mod client;
pub(crate) mod registry;
pub mod rpc;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use lsp_types::DiagnosticSeverity;
use tokio::sync::Notify;

use crate::config::types::LspConfig;
use client::{DiagStore, LspClient};

/// How long to wait for the `publishDiagnostics` that follows an edit before
/// falling back to whatever is already stored. Kept short so clean edits
/// stay fast; servers that don't republish identical diagnostics just time
/// out and reuse the previous (identical) set.
const DIAG_WAIT: Duration = Duration::from_millis(1000);

/// Max diagnostics lines appended to a tool result.
const MAX_DIAG_LINES: usize = 20;

#[derive(Clone)]
pub struct LspManager {
    inner: Arc<Inner>,
}

struct Inner {
    root: PathBuf,
    servers: Vec<(String, crate::config::types::LspServerConfig)>,
    /// server name → client, `None` caching a failed spawn so a missing
    /// binary isn't retried on every edit.
    clients: tokio::sync::Mutex<HashMap<String, Option<Arc<LspClient>>>>,
    diags: DiagStore,
    diag_notify: Arc<Notify>,
}

impl LspManager {
    pub fn new(cfg: &LspConfig, root: PathBuf) -> Self {
        let servers = registry::resolve_servers(&cfg.servers);
        tracing::debug!(
            "lsp: {} server definitions resolved (root {})",
            servers.len(),
            root.display()
        );
        Self {
            inner: Arc::new(Inner {
                root,
                servers,
                clients: tokio::sync::Mutex::new(HashMap::new()),
                diags: DiagStore::default(),
                diag_notify: Arc::new(Notify::new()),
            }),
        }
    }

    /// Whether any configured server claims this path's extension.
    pub fn handles(&self, path: &Path) -> bool {
        registry::server_for_path(&self.inner.servers, path).is_some()
    }

    /// Client for the server claiming `path`, spawning it on first use.
    /// `None` when no server matches or the spawn failed (cached).
    async fn client_for(&self, path: &Path) -> Option<Arc<LspClient>> {
        let (name, cfg) = registry::server_for_path(&self.inner.servers, path)?;
        let mut clients = self.inner.clients.lock().await;
        if let Some(cached) = clients.get(name) {
            return cached.clone();
        }
        let spawned = LspClient::spawn(name, cfg, &self.inner.root, self.inner.diags.clone(), {
            self.inner.diag_notify.clone()
        })
        .await;
        clients.insert(name.clone(), spawned.clone());
        spawned
    }

    /// Syncs a file's disk content with its language server (no-op when no
    /// server handles the extension or the server failed to start).
    pub async fn notify_changed(&self, path: &Path) {
        if let Some(client) = self.client_for(path).await {
            client.sync_file(path).await;
        }
    }

    /// Diagnostics block for one file, formatted for appending to a tool
    /// result. Waits up to `wait` for the publish following the last sync.
    /// `None` when the file is clean or has no server.
    pub async fn diagnostics_block(&self, path: &Path, wait: Duration) -> Option<String> {
        if !self.handles(path) {
            return None;
        }
        let uri = client::file_uri(path)?;
        let v0 = self
            .inner
            .diags
            .lock()
            .unwrap()
            .get(&uri)
            .map(|d| d.version)
            .unwrap_or(0);
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let current = self
                .inner
                .diags
                .lock()
                .unwrap()
                .get(&uri)
                .map(|d| d.version)
                .unwrap_or(0);
            if current > v0 {
                break;
            }
            if tokio::time::timeout_at(deadline, self.inner.diag_notify.notified())
                .await
                .is_err()
            {
                break; // timeout: use whatever is stored
            }
        }
        let store = self.inner.diags.lock().unwrap();
        let file = store.get(&uri)?;
        format_file_diags(&file.server, &file.diagnostics)
    }

    /// Compact diagnostics block for one file. Errors and warnings only,
    /// capped at [`MAX_DIAG_LINES`]. `None` when the file is clean or has no
    /// server.
    pub async fn diagnostics_block_for_edit(&self, path: &Path) -> Option<String> {
        self.diagnostics_block(path, DIAG_WAIT).await
    }

    /// All files that currently have diagnostics, formatted for the
    /// `lsp_diagnostics` tool. `None` when everything is clean.
    pub fn all_diagnostics_block(&self) -> Option<String> {
        let store = self.inner.diags.lock().unwrap();
        let mut out = String::new();
        let mut lines = 0usize;
        let mut entries: Vec<_> = store.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        for (uri, file) in entries {
            let interesting: Vec<_> = file
                .diagnostics
                .iter()
                .filter(|d| d.severity <= Some(DiagnosticSeverity::WARNING))
                .collect();
            if interesting.is_empty() {
                continue;
            }
            let display = uri
                .strip_prefix("file://")
                .and_then(|p| p.strip_prefix(self.inner.root.to_str()?))
                .map(|p| p.trim_start_matches('/').to_string())
                .unwrap_or_else(|| uri.clone());
            for d in interesting {
                if lines >= MAX_DIAG_LINES {
                    out.push_str("  … (truncated)\n");
                    return Some(out);
                }
                out.push_str(&format_diag_line(&display, d));
                lines += 1;
            }
        }
        if out.is_empty() { None } else { Some(out) }
    }

    /// Test hook: inject diagnostics as if a server had published them.
    #[cfg(test)]
    pub(crate) fn inject_diagnostics(
        &self,
        uri: &str,
        server: &str,
        diagnostics: Vec<lsp_types::Diagnostic>,
    ) {
        self.inner.diags.lock().unwrap().insert(
            uri.to_string(),
            client::FileDiags {
                server: server.to_string(),
                version: 1,
                diagnostics,
            },
        );
    }
}

/// "LSP diagnostics (server):" header + one line per error/warning, capped.
/// `None` when there is nothing worth reporting (clean edits stay silent).
fn format_file_diags(server: &str, diags: &[lsp_types::Diagnostic]) -> Option<String> {
    let mut sorted: Vec<_> = diags
        .iter()
        .filter(|d| d.severity <= Some(DiagnosticSeverity::WARNING))
        .collect();
    if sorted.is_empty() {
        return None;
    }
    sorted.sort_by_key(|d| d.severity);
    let mut out = format!("\n\nLSP diagnostics ({server}):");
    for (i, d) in sorted.iter().enumerate() {
        if i >= MAX_DIAG_LINES {
            out.push_str("\n  … (truncated)");
            break;
        }
        out.push_str(&format_diag_line("", d));
    }
    Some(out)
}

fn format_diag_line(location_prefix: &str, d: &lsp_types::Diagnostic) -> String {
    let severity = match d.severity {
        Some(DiagnosticSeverity::ERROR) => "error",
        Some(DiagnosticSeverity::WARNING) => "warning",
        Some(DiagnosticSeverity::INFORMATION) => "info",
        _ => "hint",
    };
    let line = d.range.start.line + 1;
    let col = d.range.start.character + 1;
    let message = d.message.lines().next().unwrap_or_default();
    let message = message.chars().take(200).collect::<String>();
    let where_ = if location_prefix.is_empty() {
        format!("{line}:{col}")
    } else {
        format!("{location_prefix}:{line}:{col}")
    };
    format!("\n  {where_} {severity}: {message}")
}

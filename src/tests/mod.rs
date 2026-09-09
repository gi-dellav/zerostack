#![allow(unsafe_code)]

/// ACP server protocol tests.
#[cfg(all(test, feature = "acp"))]
mod acp_tests;
/// Advisor tool conversation formatting tests.
#[cfg(all(test, feature = "advisor"))]
mod advisor_tests;
/// ARCHITECTURE.md detection and creation tests.
#[cfg(all(test, feature = "archmd"))]
mod archmd_tests;
/// Atomic file write helper tests.
#[cfg(test)]
mod atomic_write_tests;
/// Auth resolver and provider-kind tests.
#[cfg(test)]
mod auth_tests;
/// Bash command splitting tests.
#[cfg(test)]
mod bash_tests;
/// Parallel /btw side-question snapshot tests.
#[cfg(test)]
mod btw_tests;
/// Brainstorm-plan-code chain phase tests.
#[cfg(test)]
mod chain_tests;
/// Permission checker rule evaluation tests.
#[cfg(test)]
mod checker_tests;
/// --custom-mcp/--prompts-dir CLI flag resolution tests.
#[cfg(test)]
mod cli_custom_flags_tests;
/// Config loading and merging tests.
#[cfg(test)]
mod config_tests;
/// Context file loading tests.
#[cfg(test)]
mod context_tests;
/// Session-to-model history conversion tests.
#[cfg(test)]
mod convert_history_tests;
/// CRC32 checksum helper tests.
#[cfg(test)]
mod crc_tests;
/// Edit tool matching and application tests.
#[cfg(test)]
mod edit_tests;
/// Headless Engine run_string tests.
#[cfg(test)]
mod engine_tests;
/// Shared scripted fake CompletionModel carrier.
#[cfg(test)]
mod fake_model;
/// Conversation feed block tests.
#[cfg(test)]
mod feed_tests;
/// Grep tool pattern matching tests.
#[cfg(test)]
mod grep_tests;
/// Headless Ask-fails-closed permission tests.
#[cfg(test)]
mod headless_ask_tests;
/// Headless subagent call recording tests.
#[cfg(all(test, feature = "subagents"))]
mod headless_subagent_record_tests;
/// Headless tool call/result recording tests.
#[cfg(test)]
mod headless_tool_record_tests;
/// Hooks subsystem test suite.
#[cfg(all(test, feature = "hooks"))]
mod hooks;
/// Tool-result image relay tests.
#[cfg(test)]
mod image_relay_tests;
/// Input editor key handling tests.
#[cfg(test)]
mod input_tests;
/// List-dir formatting tests.
#[cfg(test)]
mod list_dir_tests;
/// Logging init and file-output tests.
#[cfg(test)]
mod logging_tests;
/// --loop state and plan-file tests.
#[cfg(all(test, feature = "loop"))]
mod loop_tests;
/// LSP manager and diagnostics tests.
#[cfg(all(test, feature = "lsp"))]
mod lsp_tests;
/// Markdown rendering tests.
#[cfg(test)]
mod markdown_tests;
/// MCP tool-result content rendering tests.
#[cfg(all(test, feature = "mcp"))]
mod mcp_content_tests;
/// MCP OAuth flow tests.
#[cfg(all(test, feature = "mcp"))]
mod mcp_oauth_tests;
/// MCP timeout and reconnect tests.
#[cfg(all(test, feature = "mcp"))]
mod mcp_timeout_tests;
/// Persistent memory feature tests.
#[cfg(all(test, feature = "memory"))]
mod memory_tests;
/// Static model catalog tests.
#[cfg(test)]
mod models_catalog_tests;
/// Media attachment detection tests.
#[cfg(all(test, feature = "multimodal"))]
mod multimodal_tests;
/// Whitespace normalization tests.
#[cfg(test)]
mod normalize_tests;
/// Parallel tool-call pairing regression tests.
#[cfg(test)]
mod parallel_tool_call_tests;
/// Paste-burst key handling tests.
#[cfg(test)]
mod paste_burst_tests;
/// Fuzzy picker matching tests.
#[cfg(test)]
mod picker_tests;
/// --print-config output tests.
#[cfg(test)]
mod print_config_tests;
/// Prompt %%mode= parsing tests.
#[cfg(test)]
mod prompt_mode_tests;
/// Provider client creation tests.
#[cfg(test)]
mod provider_tests;
/// TUI renderer viewport tests.
#[cfg(test)]
mod renderer_tests;
/// Resumed-session history replay tests.
#[cfg(test)]
mod resumed_history_tests;
/// rtk proxy integration tests.
#[cfg(all(test, feature = "rtk"))]
mod rtk_tests;
/// Sandbox agent env cutoff tests.
#[cfg(test)]
mod sandbox_agent_cutoff_tests;
/// Sandbox expose-path partitioning tests.
#[cfg(test)]
mod sandbox_expose_tests;
/// Sandbox mask-hint message tests.
#[cfg(test)]
mod sandbox_hint_tests;
/// Sandbox credential masking tests.
#[cfg(test)]
mod sandbox_mask_tests;
/// Sandbox network-namespace flag tests.
#[cfg(test)]
mod sandbox_network_tests;
/// Sandbox-required enforcement tests.
#[cfg(test)]
mod sandbox_required_tests;
/// Shared sandbox test scaffolding.
#[cfg(test)]
mod sandbox_support;
/// Session HTML/JSONL export tests.
#[cfg(all(test, feature = "export"))]
mod session_export_tests;
/// Session persistence tests.
#[cfg(test)]
mod session_storage_tests;
/// Session token accounting tests.
#[cfg(test)]
mod session_tests;
/// Shell-mode execution tests.
#[cfg(test)]
mod shell_mode_tests;
/// Single-flight submission policy tests.
#[cfg(test)]
mod singleflight_tests;
/// /add path resolution tests.
#[cfg(test)]
mod slash_add_tests;
/// /init prompt content tests.
#[cfg(test)]
mod slash_init_tests;
/// Startup prompt-mode resolution tests.
#[cfg(test)]
mod startup_prompt_mode_tests;
/// Unix-socket status signal tests.
#[cfg(all(test, unix))]
mod status_signals_tests;
/// Statusline segment parsing tests.
#[cfg(test)]
mod statusline_tests;
/// Subagent delegation tests.
#[cfg(all(test, feature = "subagents"))]
mod subagents_tests;
/// Todo-list tool tests.
#[cfg(test)]
mod todo_tests;
/// Tool allowlist filtering tests.
#[cfg(test)]
mod tools_filter_tests;
/// Tool read-tracking state tests.
#[cfg(test)]
mod tools_mod_tests;
/// Headless TUI main-loop integration tests.
#[cfg(test)]
mod tui_loop_tests;
/// Git worktree create/merge tests.
#[cfg(all(test, feature = "git-worktree"))]
mod worktree_tests;

/// Process-global CWD serialisation for tests.
///
/// `std::env::set_current_dir` is process-global: concurrent tests that
/// mutate CWD observe each other's directories, including ones already
/// deleted — which makes even `current_dir()` itself fail. Any test that
/// mutates CWD must hold this lock for its whole body; readers that only
/// *assert* on CWD (`slash_add`) must hold it too while computing the
/// expectation. Declared here (rather than per-file) so every such test
/// shares one lock.
///
/// Acquiring also repairs a deleted cwd: see [`acquire_cwd`].
///
/// NOTE: this deliberately does NOT cover the TUI loop tests
/// (`tui_loop_tests`, `headless_*`, `parallel_tool_call_tests`):
/// those never chdir, so they don't need it.
#[cfg(test)]
static CWD_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();

#[cfg(test)]
pub(crate) fn acquire_cwd() -> std::sync::MutexGuard<'static, ()> {
    let lock = CWD_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    // A previous holder may have left the process in a directory it then
    // deleted (a TempRepo dropped before its restore ran, or the binary was
    // started from one). Park in a directory that exists so child
    // processes such as `git` can read their cwd. Not restored: the next
    // holder that cares chdirs to where it wants to be anyway.
    if std::env::current_dir().is_err() {
        let _ = std::env::set_current_dir(std::env::temp_dir());
    }
    lock
}

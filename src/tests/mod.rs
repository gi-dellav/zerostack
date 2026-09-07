#![allow(unsafe_code)]

#[cfg(all(test, feature = "acp"))]
mod acp_tests;
#[cfg(all(test, feature = "advisor"))]
mod advisor_tests;
#[cfg(all(test, feature = "archmd"))]
mod archmd_tests;
#[cfg(test)]
mod atomic_write_tests;
#[cfg(test)]
mod auth_tests;
#[cfg(test)]
mod bash_tests;
#[cfg(test)]
mod btw_tests;
#[cfg(test)]
mod chain_tests;
#[cfg(test)]
mod checker_tests;
#[cfg(test)]
mod cli_custom_flags_tests;
#[cfg(test)]
mod config_tests;
#[cfg(test)]
mod context_tests;
#[cfg(test)]
mod convert_history_tests;
#[cfg(test)]
mod crc_tests;
#[cfg(test)]
mod edit_tests;
#[cfg(test)]
mod fake_model;
#[cfg(test)]
mod feed_tests;
#[cfg(test)]
mod grep_tests;
#[cfg(test)]
mod headless_ask_tests;
#[cfg(all(test, feature = "subagents"))]
mod headless_subagent_record_tests;
#[cfg(test)]
mod headless_tool_record_tests;
#[cfg(all(test, feature = "hooks"))]
mod hooks;
#[cfg(test)]
mod image_relay_tests;
#[cfg(test)]
mod input_tests;
#[cfg(test)]
mod list_dir_tests;
#[cfg(test)]
mod logging_tests;
#[cfg(all(test, feature = "loop"))]
mod loop_tests;
#[cfg(all(test, feature = "lsp"))]
mod lsp_tests;
#[cfg(test)]
mod markdown_tests;
#[cfg(all(test, feature = "mcp"))]
mod mcp_content_tests;
#[cfg(all(test, feature = "mcp"))]
mod mcp_oauth_tests;
#[cfg(all(test, feature = "mcp"))]
mod mcp_timeout_tests;
#[cfg(all(test, feature = "memory"))]
mod memory_tests;
#[cfg(test)]
mod models_catalog_tests;
#[cfg(all(test, feature = "multimodal"))]
mod multimodal_tests;
#[cfg(test)]
mod normalize_tests;
#[cfg(test)]
mod parallel_tool_call_tests;
#[cfg(test)]
mod paste_burst_tests;
#[cfg(test)]
mod picker_tests;
#[cfg(test)]
mod print_config_tests;
#[cfg(test)]
mod prompt_mode_tests;
#[cfg(test)]
mod provider_tests;
#[cfg(test)]
mod renderer_tests;
#[cfg(test)]
mod resumed_history_tests;
#[cfg(all(test, feature = "rtk"))]
mod rtk_tests;
#[cfg(test)]
mod sandbox_agent_cutoff_tests;
#[cfg(test)]
mod sandbox_expose_tests;
#[cfg(test)]
mod sandbox_hint_tests;
#[cfg(test)]
mod sandbox_mask_tests;
#[cfg(test)]
mod sandbox_network_tests;
#[cfg(test)]
mod sandbox_required_tests;
#[cfg(test)]
mod sandbox_support;
#[cfg(all(test, feature = "export"))]
mod session_export_tests;
#[cfg(test)]
mod session_storage_tests;
#[cfg(test)]
mod session_tests;
#[cfg(test)]
mod shell_mode_tests;
#[cfg(test)]
mod singleflight_tests;
#[cfg(test)]
mod slash_add_tests;
#[cfg(test)]
mod slash_init_tests;
#[cfg(test)]
mod startup_prompt_mode_tests;
#[cfg(all(test, unix))]
mod status_signals_tests;
#[cfg(test)]
mod statusline_tests;
#[cfg(all(test, feature = "subagents"))]
mod subagents_tests;
#[cfg(test)]
mod todo_tests;
#[cfg(test)]
mod tools_filter_tests;
#[cfg(test)]
mod tools_mod_tests;
#[cfg(test)]
mod tui_loop_tests;
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

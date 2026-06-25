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
mod config_tests;
#[cfg(test)]
mod crc_tests;
#[cfg(test)]
mod edit_tests;
#[cfg(test)]
mod grep_tests;
#[cfg(test)]
mod input_tests;
#[cfg(test)]
mod list_dir_tests;
#[cfg(all(test, feature = "loop"))]
mod loop_tests;
#[cfg(test)]
mod markdown_tests;
#[cfg(all(test, feature = "mcp"))]
mod mcp_oauth_tests;
#[cfg(all(test, feature = "memory"))]
mod memory_tests;
#[cfg(test)]
mod models_catalog_tests;
#[cfg(all(test, feature = "multimodal"))]
mod multimodal_tests;
#[cfg(test)]
mod normalize_tests;
#[cfg(test)]
mod picker_tests;
#[cfg(test)]
mod provider_tests;
#[cfg(test)]
mod renderer_tests;
#[cfg(test)]
mod session_storage_tests;
#[cfg(test)]
mod session_tests;
#[cfg(test)]
mod singleflight_tests;
#[cfg(test)]
mod slash_add_tests;
#[cfg(test)]
mod slash_init_tests;
#[cfg(all(test, unix))]
mod status_signals_tests;
#[cfg(test)]
mod statusline_tests;
#[cfg(all(test, feature = "subagents"))]
mod subagents_tests;
#[cfg(test)]
mod todo_tests;
#[cfg(test)]
mod tools_mod_tests;
#[cfg(all(test, feature = "git-worktree"))]
mod worktree_tests;

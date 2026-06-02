#[cfg(all(test, feature = "acp"))]
mod acp_tests;
#[cfg(all(test, feature = "archmd"))]
mod archmd_tests;
#[cfg(test)]
mod auth_tests;
#[cfg(test)]
mod btw_tests;
#[cfg(test)]
mod checker_tests;
#[cfg(test)]
mod crc_tests;
#[cfg(test)]
mod edit_tests;
#[cfg(test)]
mod input_tests;
#[cfg(test)]
mod markdown_tests;
#[cfg(all(test, feature = "memory"))]
mod memory_tests;
#[cfg(test)]
mod normalize_tests;
#[cfg(test)]
mod picker_tests;
#[cfg(test)]
mod provider_tests;
#[cfg(test)]
mod renderer_tests;
#[cfg(test)]
mod singleflight_tests;
#[cfg(test)]
mod slash_add_tests;
#[cfg(test)]
mod slash_init_tests;
#[cfg(all(test, feature = "subagents"))]
mod subagents_tests;
#[cfg(all(test, feature = "git-worktree"))]
mod worktree_tests;

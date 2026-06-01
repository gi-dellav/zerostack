#[cfg(all(test, feature = "acp"))]
mod acp_tests;
#[cfg(all(test, feature = "archmd"))]
mod archmd_tests;
#[cfg(test)]
mod auth_tests;
#[cfg(test)]
mod checker_tests;
#[cfg(test)]
mod edit_tests;
#[cfg(test)]
mod input_tests;
#[cfg(all(test, feature = "memory"))]
mod memory_tests;
#[cfg(test)]
mod picker_tests;
#[cfg(test)]
mod singleflight_tests;
#[cfg(all(test, feature = "subagents"))]
mod subagents_tests;
#[cfg(all(test, feature = "git-worktree"))]
mod worktree_tests;

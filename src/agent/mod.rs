/// Rig agent construction with tool injection and prompt assembly.
pub mod builder;
/// Image relay from tool results into follow-up user messages.
pub mod image_relay;
/// System prompt text and prompt-building helpers.
pub mod prompt;
/// Agent streaming lifecycle: spawn, retry, and event channel.
pub mod runner;
/// Built-in agent tools (read, write, edit, bash, grep, ...).
pub mod tools;

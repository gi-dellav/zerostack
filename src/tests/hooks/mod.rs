//! Tests for the `hooks` subsystem. The `corpus/` subdirectory holds the
//! data-driven contract cases exercised by `corpus_tests`.

/// Exit-code/stdout-JSON channel interpretation tests.
mod channel_tests;
/// Decision/Verdict ordering unit tests.
mod core_tests;
/// Data-driven corpus contract tests.
mod corpus_tests;
/// Tool decorator decision-application tests.
mod decorator_tests;
/// Dispatcher matching and verdict-merging tests.
mod dispatcher_tests;
/// Stdin envelope construction tests.
mod envelope_tests;
/// Tool-name canonicalization tests.
mod normalize_tests;
/// UserPromptSubmit gate tests.
mod prompt_gate_tests;
/// SessionStart/SessionEnd lifecycle tests.
mod session_lifecycle_tests;
/// settings.json config parsing tests.
mod settings_tests;
/// Stop gate continuation tests.
mod stop_gate_tests;
/// SubagentStart/SubagentStop lifecycle tests.
mod subagent_lifecycle_tests;
/// Hook subprocess execution tests.
mod subprocess_tests;
/// --hooks-test dry-run tests.
mod test_dry_run_tests;
/// Trust-hash confirmation tests.
mod trust_tests;

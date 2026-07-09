//! Data-driven contract tests: each subdirectory of `src/tests/hooks/corpus/`
//! is one case with three files: `envelope.json` (the event/tool_name/
//! tool_input to dispatch), `hook.sh` (the hook command, run for real via a
//! subprocess), and `expected.json` (the decision the dispatcher must
//! produce). Adding a new compatibility case needs no new Rust code, just a
//! new corpus directory.

use std::collections::HashMap;
use std::path::Path;

use crate::extras::hooks::dispatcher::HookDispatcher;
use crate::extras::hooks::envelope::EventFields;
use crate::extras::hooks::settings::{HookGroup, HookHandler, HooksConfig};
use crate::extras::hooks::{Decision, HookCtx, Verdict};

fn corpus_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/tests/hooks/corpus"
    ))
}

fn ctx() -> HookCtx {
    HookCtx {
        session_id: "corpus-sess".into(),
        session_path: "".into(),
        cwd: "/repo".into(),
        permission_mode: "standard".into(),
    }
}

fn dispatcher_for(event: &str, hook_path: &Path) -> HookDispatcher {
    let handler = HookHandler {
        kind: "command".to_string(),
        command: Some(format!("sh {}", hook_path.display())),
        args: None,
        timeout: Some(5),
        is_async: false,
        condition: None,
        once: false,
    };
    let mut config: HooksConfig = HashMap::new();
    config.insert(
        event.to_string(),
        vec![HookGroup {
            matcher: None,
            hooks: vec![handler],
        }],
    );
    HookDispatcher::from_config(&config).unwrap()
}

#[tokio::test]
async fn corpus_cases_match_expected_decisions() {
    let root = corpus_dir();
    let mut cases: Vec<std::path::PathBuf> = std::fs::read_dir(&root)
        .unwrap_or_else(|e| panic!("failed to read corpus dir {}: {e}", root.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    cases.sort();
    assert!(!cases.is_empty(), "corpus at {} is empty", root.display());

    for case_dir in cases {
        let case_name = case_dir.file_name().unwrap().to_string_lossy().to_string();

        let envelope: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(case_dir.join("envelope.json"))
                .unwrap_or_else(|e| panic!("[{case_name}] read envelope.json: {e}")),
        )
        .unwrap_or_else(|e| panic!("[{case_name}] parse envelope.json: {e}"));

        let expected: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(case_dir.join("expected.json"))
                .unwrap_or_else(|e| panic!("[{case_name}] read expected.json: {e}")),
        )
        .unwrap_or_else(|e| panic!("[{case_name}] parse expected.json: {e}"));

        let event = envelope["event"]
            .as_str()
            .unwrap_or_else(|| panic!("[{case_name}] envelope.json missing \"event\""));
        let hook_path = case_dir.join("hook.sh");
        let dispatcher = dispatcher_for(event, &hook_path);

        let kind = expected["kind"]
            .as_str()
            .unwrap_or_else(|| panic!("[{case_name}] expected.json missing \"kind\""));

        match kind {
            "pre_tool_use" => {
                let tool_name = envelope["tool_name"]
                    .as_str()
                    .unwrap_or_else(|| panic!("[{case_name}] envelope.json missing \"tool_name\""));
                let tool_input = envelope["tool_input"].clone();
                let decision = dispatcher
                    .dispatch_pre_tool_use(&ctx(), tool_name, tool_input)
                    .await;

                let expected_verdict = match expected["verdict"].as_str() {
                    Some("Deny") => Verdict::Deny,
                    Some("Ask") => Verdict::Ask,
                    Some("Allow") => Verdict::Allow,
                    Some("Defer") => Verdict::Defer,
                    other => panic!("[{case_name}] unknown expected verdict: {other:?}"),
                };
                assert_eq!(
                    decision.verdict, expected_verdict,
                    "[{case_name}] verdict mismatch"
                );
                // stderr/stdout from a real subprocess carries whatever
                // trailing newline the hook's `echo` produced; the corpus's
                // expected.json fixtures store the trimmed text for
                // readability, so compare trimmed on both sides.
                assert_eq!(
                    decision.reason.as_deref().map(str::trim),
                    expected["reason"].as_str(),
                    "[{case_name}] reason mismatch"
                );
                if let Some(expected_input) = expected.get("updated_input") {
                    assert_eq!(
                        decision.updated_input.as_ref(),
                        Some(expected_input),
                        "[{case_name}] updated_input mismatch"
                    );
                }
            }
            "generic" => {
                // The harness only knows how to build a Stop-shaped envelope
                // today; a case testing another lifecycle event needs this
                // match extended first.
                assert_eq!(
                    event, "Stop",
                    "[{case_name}] the \"generic\" harness path only supports event \"Stop\" today"
                );
                let decision = dispatcher
                    .dispatch(
                        event,
                        None,
                        &ctx(),
                        EventFields::Stop {
                            stop_hook_active: false,
                            loop_iteration: None,
                            loop_active: None,
                        },
                    )
                    .await;
                match expected["decision"].as_str() {
                    Some("Continue") => {
                        assert_eq!(
                            decision,
                            Decision::Continue,
                            "[{case_name}] decision mismatch"
                        )
                    }
                    Some("Block") => {
                        let expected_reason =
                            expected["reason"].as_str().unwrap_or_default().to_string();
                        assert_eq!(
                            decision,
                            Decision::Block {
                                reason: expected_reason
                            },
                            "[{case_name}] decision mismatch"
                        );
                    }
                    other => panic!("[{case_name}] unknown expected decision: {other:?}"),
                }
            }
            other => panic!("[{case_name}] unknown expected.json \"kind\": {other:?}"),
        }
    }
}

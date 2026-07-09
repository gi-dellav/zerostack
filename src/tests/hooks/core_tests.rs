use crate::extras::hooks::{Decision, HookCtx, PreDecision, Verdict};

#[test]
fn verdict_orders_most_severe_last() {
    assert!(Verdict::Deny > Verdict::Ask);
    assert!(Verdict::Ask > Verdict::Defer);
    assert!(Verdict::Defer > Verdict::Allow);
}

#[test]
fn verdict_max_of_allow_and_deny_is_deny() {
    let verdicts = [Verdict::Allow, Verdict::Deny];
    assert_eq!(verdicts.into_iter().max(), Some(Verdict::Deny));
}

#[test]
fn pre_decision_defer_carries_no_rewrite() {
    let decision = PreDecision {
        verdict: Verdict::Defer,
        reason: None,
        updated_input: None,
    };
    assert_eq!(decision.verdict, Verdict::Defer);
    assert!(decision.updated_input.is_none());
}

#[test]
fn decision_continue_is_distinct_from_block() {
    assert_ne!(Decision::Continue, Decision::Block { reason: "x".into() });
}

#[test]
fn decision_rewrite_carries_replacement_content() {
    let decision = Decision::Rewrite {
        content: "redacted".into(),
    };
    assert_eq!(
        decision,
        Decision::Rewrite {
            content: "redacted".into()
        }
    );
    assert_ne!(decision, Decision::Continue);
}

#[test]
fn hook_ctx_holds_all_common_envelope_fields() {
    let ctx = HookCtx {
        session_id: "abc".into(),
        session_path: "/tmp/session.json".into(),
        cwd: "/tmp".into(),
        permission_mode: "yolo".into(),
    };
    assert_eq!(ctx.session_id, "abc");
    assert_eq!(ctx.session_path, "/tmp/session.json");
    assert_eq!(ctx.cwd, "/tmp");
    assert_eq!(ctx.permission_mode, "yolo");
}

use crate::config::Config;

#[test]
fn mid_turn_threshold_unset_by_default() {
    let cfg = Config::default();
    assert_eq!(cfg.resolve_mid_turn_compact_threshold(), None);
}

#[test]
fn mid_turn_threshold_valid_value_passes_through() {
    let cfg = Config {
        mid_turn_compact_threshold: Some(0.80),
        ..Config::default()
    };
    assert_eq!(cfg.resolve_mid_turn_compact_threshold(), Some(0.80));
}

#[test]
fn mid_turn_threshold_upper_bound_inclusive() {
    let cfg = Config {
        mid_turn_compact_threshold: Some(1.0),
        ..Config::default()
    };
    assert_eq!(cfg.resolve_mid_turn_compact_threshold(), Some(1.0));
}

#[test]
fn mid_turn_threshold_out_of_range_treated_as_unset() {
    // Zero would compact constantly; negatives and >1 are nonsense. All map to
    // "unset" so a misconfigured value silently disables the feature rather
    // than wedging the agent.
    for bad in [0.0, -0.1, 1.5, 2.0] {
        let cfg = Config {
            mid_turn_compact_threshold: Some(bad),
            ..Config::default()
        };
        assert_eq!(
            cfg.resolve_mid_turn_compact_threshold(),
            None,
            "threshold {bad} should be treated as unset"
        );
    }
}

#[test]
fn compact_enabled_default_true() {
    // Master switch defaults on; mid-turn compaction layers on top of it.
    assert!(Config::default().resolve_compact_enabled());
}

#[test]
fn context_exhausted_report_math() {
    // window 20000, threshold 0.80 -> ceiling 16000.
    // prompt 18000 -> 90% of window, overflow 18000 - 16000 = 2000.
    let lines = crate::ui::context_exhausted_report(18_000, 0.80, 20_000, 8_192, 6_000);
    let joined = lines.join("\n");
    assert!(joined.contains("context window .............. 20000 tokens"), "{joined}");
    assert!(joined.contains("16000 tokens  (80% of window)"), "{joined}");
    assert!(joined.contains("18000 tokens  (90% of window)"), "{joined}");
    assert!(joined.contains("overflow above ceiling ...... 2000 tokens"), "{joined}");
    assert!(joined.contains("reserved for response ....... 8192 tokens"), "{joined}");
    assert!(joined.contains("kept-recent budget .......... 6000 tokens"), "{joined}");
    // Guidance references the actual pressure and the floor the KV cache must hold.
    assert!(joined.contains("raise mid_turn_compact_threshold above 90%"), "{joined}");
    assert!(joined.contains("hold 18000+ tokens"), "{joined}");
}

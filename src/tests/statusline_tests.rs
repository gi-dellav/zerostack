use crate::config::types::{StatusLineConfig, StatusLineLine, StatusLineSegment};
use crate::session::{GitStatus, Session};
use crate::ui::statusline::{self, StatusContext, StatusSpan};

fn ctx() -> StatusContext<'static> {
    StatusContext {
        loop_label: None,
        prompt_name: None,
        perm_mode: None,
        chain_label: None,
        btw_cost: 0.0,
        btw_in: 0,
        btw_out: 0,
    }
}

fn seg(item: &str) -> StatusLineSegment {
    StatusLineSegment {
        item: item.into(),
        color: None,
        bg: None,
        text: None,
    }
}

fn line_text(spans: &[StatusSpan]) -> String {
    spans
        .iter()
        .map(|s| match s {
            StatusSpan::Text { text, .. } => text.clone(),
            StatusSpan::Flex => "\u{0}FLEX\u{0}".to_string(),
        })
        .collect()
}

#[test]
fn default_statusline_shows_core_items() {
    let session = Session::new("openrouter", "deepseek/deepseek-v4-pro", 1_048_576);
    let lines = statusline::build_lines(&statusline::default_spec(), &session, &ctx());
    assert_eq!(lines.len(), 1);
    let text = line_text(&lines[0]);
    assert!(text.contains("deepseek/deepseek-v4-pro"), "{text}");
    assert!(text.contains("/1.0M"), "{text}"); // context max
}

#[test]
fn flex_separator_is_preserved() {
    let spec = StatusLineConfig {
        lines: vec![StatusLineLine {
            segments: vec![seg("model"), seg("flex_separator"), seg("context_max")],
        }],
    };
    let session = Session::new("openrouter", "m", 1000);
    let lines = statusline::build_lines(&spec, &session, &ctx());
    assert!(matches!(lines[0][1], StatusSpan::Flex));
}

#[test]
fn skipped_optional_item_drops_adjacent_separator() {
    // `cost` resolves to nothing (0 and not always-shown), so the separator
    // around it should be trimmed rather than left dangling.
    let spec = StatusLineConfig {
        lines: vec![StatusLineLine {
            segments: vec![
                seg("model"),
                StatusLineSegment {
                    item: "separator".into(),
                    color: None,
                    bg: None,
                    text: Some(" | ".into()),
                },
                seg("cost"),
            ],
        }],
    };
    let mut session = Session::new("openrouter", "m", 1000);
    session.total_cost = 0.0;
    let lines = statusline::build_lines(&spec, &session, &ctx());
    assert_eq!(
        line_text(&lines[0]),
        "m",
        "trailing separator should be dropped"
    );
}

#[test]
fn cost_shown_when_always_flag_set() {
    let spec = StatusLineConfig {
        lines: vec![StatusLineLine {
            segments: vec![seg("cost")],
        }],
    };
    let mut session = Session::new("openrouter", "m", 1000);
    session.show_cost_always = true;
    let lines = statusline::build_lines(&spec, &session, &ctx());
    assert_eq!(line_text(&lines[0]), "$0.0000");
}

#[test]
fn format_changes_lists_nonzero_parts() {
    let g = GitStatus {
        staged: 2,
        modified: 3,
        deleted: 0,
        untracked: 1,
        ahead: 0,
        behind: 0,
    };
    assert_eq!(statusline::format_changes(&g).as_deref(), Some("+2 ~3 ?1"));
    assert_eq!(statusline::format_changes(&GitStatus::default()), None);
}

#[test]
fn format_status_shows_sync_and_dirty() {
    let g = GitStatus {
        modified: 1,
        ahead: 2,
        behind: 1,
        ..Default::default()
    };
    assert_eq!(statusline::format_status(&g), "\u{2191}2 \u{2193}1 *");
    assert_eq!(statusline::format_status(&GitStatus::default()), "\u{2713}");
}

#[test]
fn fmt_tokens_scales() {
    assert_eq!(statusline::fmt_tokens(0), "0");
    assert_eq!(statusline::fmt_tokens(12_000), "12k");
    assert_eq!(statusline::fmt_tokens(1_048_576), "1.0M");
}

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
        ..Default::default()
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
    let session = Session::new("openrouter", "deepseek/deepseek-v4-pro", 1_048_576, "");
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
    let session = Session::new("openrouter", "m", 1000, "");
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
                    text: Some(" | ".into()),
                    ..Default::default()
                },
                seg("cost"),
            ],
        }],
    };
    let mut session = Session::new("openrouter", "m", 1000, "");
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
    let mut session = Session::new("openrouter", "m", 1000, "");
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

#[test]
fn powerline_glyph_resolves_names_and_passthrough() {
    assert_eq!(statusline::powerline_glyph("pl_right"), "\u{e0b0}");
    assert_eq!(statusline::powerline_glyph("round_left"), "\u{e0b6}");
    // Unknown names pass through unchanged so any literal glyph works.
    assert_eq!(statusline::powerline_glyph("\u{e0b4}"), "\u{e0b4}");
    assert_eq!(statusline::powerline_glyph(">"), ">");
}

#[test]
fn left_right_caps_wrap_the_item() {
    let spec = StatusLineConfig {
        lines: vec![StatusLineLine {
            segments: vec![StatusLineSegment {
                item: "model".into(),
                color: Some("white".into()),
                bg: Some("blue".into()),
                left: Some("pl_round_left".into()),
                right: Some("pl_round_right".into()),
                ..Default::default()
            }],
        }],
    };
    let session = Session::new("openrouter", "m", 1000, "");
    let lines = statusline::build_lines(&spec, &session, &ctx());
    assert_eq!(lines[0].len(), 3); // left cap, model, right cap
    assert_eq!(line_text(&lines[0]), "\u{e0b6}m\u{e0b4}");
}

#[test]
fn caps_skipped_when_item_is_hidden() {
    // cost resolves to nothing, so its caps must not appear either.
    let spec = StatusLineConfig {
        lines: vec![StatusLineLine {
            segments: vec![StatusLineSegment {
                item: "cost".into(),
                left: Some("pl_round_left".into()),
                right: Some("pl_round_right".into()),
                ..Default::default()
            }],
        }],
    };
    let session = Session::new("openrouter", "m", 1000, "");
    let lines = statusline::build_lines(&spec, &session, &ctx());
    assert!(lines[0].is_empty());
}

#[test]
fn light_blue_color_parses_to_rgb() {
    use crate::ui::utils::parse_color;
    assert_eq!(
        parse_color("light_blue"),
        Some(crossterm::style::Color::Rgb {
            r: 0x5f,
            g: 0xaf,
            b: 0xff
        })
    );
}

#[test]
fn auto_icon_prepends_item_glyph() {
    let spec = StatusLineConfig {
        lines: vec![StatusLineLine {
            segments: vec![StatusLineSegment {
                item: "git_branch".into(),
                icon: Some(crate::config::types::IconSpec::Auto(true)),
                ..Default::default()
            }],
        }],
    };
    let mut session = Session::new("openrouter", "m", 1000, "");
    session.git_branch = Some("main".into());
    let lines = statusline::build_lines(&spec, &session, &ctx());
    assert_eq!(line_text(&lines[0]), "\u{e0a0} main");
}

#[test]
fn custom_icon_name_resolves() {
    let spec = StatusLineConfig {
        lines: vec![StatusLineLine {
            segments: vec![StatusLineSegment {
                item: "cwd".into(),
                icon: Some(crate::config::types::IconSpec::Custom("folder".into())),
                ..Default::default()
            }],
        }],
    };
    let session = Session::new("openrouter", "m", 1000, "");
    let text = line_text(&statusline::build_lines(&spec, &session, &ctx())[0]);
    assert!(text.starts_with("\u{f07b} "), "{text}");
}

#[test]
fn item_icon_known_and_unknown() {
    assert_eq!(statusline::item_icon("git_branch"), Some("\u{e0a0}"));
    assert_eq!(statusline::item_icon("tokens_input"), None);
}

#[test]
fn cwd_full_vs_cwd_folder() {
    let spec = StatusLineConfig {
        lines: vec![StatusLineLine {
            segments: vec![seg("cwd"), seg("separator"), seg("cwd_full")],
        }],
    };
    let mut session = Session::new("openrouter", "m", 1000, "");
    session.working_dir = "/var/lib/zerostack-demo/project".into();
    let lines = statusline::build_lines(&spec, &session, &ctx());
    // cwd = folder name, cwd_full = full path (no $HOME prefix here, unchanged)
    assert_eq!(
        line_text(&lines[0]),
        "project /var/lib/zerostack-demo/project"
    );
}

#[test]
fn always_shows_zero_tokens() {
    let hidden = StatusLineConfig {
        lines: vec![StatusLineLine {
            segments: vec![seg("tokens_input")],
        }],
    };
    let forced = StatusLineConfig {
        lines: vec![StatusLineLine {
            segments: vec![StatusLineSegment {
                item: "tokens_input".into(),
                always: Some(true),
                ..Default::default()
            }],
        }],
    };
    let session = Session::new("openrouter", "m", 1000, ""); // total_input_tokens == 0
    assert!(line_text(&statusline::build_lines(&hidden, &session, &ctx())[0]).is_empty());
    assert_eq!(
        line_text(&statusline::build_lines(&forced, &session, &ctx())[0]),
        "0"
    );
}

#[test]
fn provider_model_short_message_count() {
    let spec = StatusLineConfig {
        lines: vec![StatusLineLine {
            segments: vec![
                seg("provider"),
                seg("separator"),
                seg("model_short"),
                seg("separator"),
                seg("message_count"),
            ],
        }],
    };
    let mut session = Session::new("openrouter", "deepseek/deepseek-v4-pro", 1000, "");
    session.add_message(crate::session::MessageRole::User, "hi");
    let text = line_text(&statusline::build_lines(&spec, &session, &ctx())[0]);
    assert_eq!(text, "openrouter deepseek-v4-pro 1");
}

#[test]
fn reasoning_shows_only_when_enabled() {
    let spec = StatusLineConfig {
        lines: vec![StatusLineLine {
            segments: vec![seg("reasoning")],
        }],
    };
    let mut session = Session::new("openrouter", "m", 1000, "");
    assert!(line_text(&statusline::build_lines(&spec, &session, &ctx())[0]).is_empty());
    session.reasoning_enabled = true;
    assert_eq!(
        line_text(&statusline::build_lines(&spec, &session, &ctx())[0]),
        "reasoning"
    );
}

fn first_fg(spans: &[StatusSpan]) -> Option<crossterm::style::Color> {
    spans.iter().find_map(|s| match s {
        StatusSpan::Text { fg, .. } => Some(*fg),
        StatusSpan::Flex => None,
    })?
}

#[test]
fn context_percentage_is_not_clamped() {
    // Past the window is a real over-budget state, so it must be reported
    // honestly rather than capped at 100%.
    let spec = StatusLineConfig {
        lines: vec![StatusLineLine {
            segments: vec![seg("context_percentage")],
        }],
    };
    let mut session = Session::new("openrouter", "m", 1000, "");
    session.set_calibration(1100, 0); // 110% of the window
    assert_eq!(
        line_text(&statusline::build_lines(&spec, &session, &ctx())[0]),
        "110%"
    );
}

#[test]
fn context_percentage_color_warns_as_it_fills() {
    use crossterm::style::Color;
    let spec = StatusLineConfig {
        lines: vec![StatusLineLine {
            segments: vec![seg("context_percentage")],
        }],
    };
    // Green tier keeps the configured (here: default/none) color.
    let mut session = Session::new("openrouter", "m", 1000, "");
    session.set_calibration(500, 0); // 50%
    assert_eq!(
        first_fg(&statusline::build_lines(&spec, &session, &ctx())[0]),
        None
    );

    // Amber as it approaches full.
    session.set_calibration(950, 0); // 95%
    assert_eq!(
        first_fg(&statusline::build_lines(&spec, &session, &ctx())[0]),
        Some(Color::Yellow)
    );

    // Red once it crosses the window.
    session.set_calibration(1100, 0); // 110%
    assert_eq!(
        first_fg(&statusline::build_lines(&spec, &session, &ctx())[0]),
        Some(Color::Red)
    );
}

#[test]
fn session_age_is_short_duration() {
    let spec = StatusLineConfig {
        lines: vec![StatusLineLine {
            segments: vec![seg("session_age")],
        }],
    };
    let session = Session::new("openrouter", "m", 1000, ""); // created_at = now
    let text = line_text(&statusline::build_lines(&spec, &session, &ctx())[0]);
    assert!(text.ends_with('s') || text.ends_with('m'), "got {text:?}");
}

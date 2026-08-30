use crate::ui::renderer::{base64_encode, copy_to_clipboard, is_safe_url};

#[test]
fn base64_encode_empty() {
    assert_eq!(base64_encode(b""), "");
}

#[test]
fn base64_encode_single_byte() {
    assert_eq!(base64_encode(b"f"), "Zg==");
}

#[test]
fn base64_encode_two_bytes() {
    assert_eq!(base64_encode(b"fo"), "Zm8=");
}

#[test]
fn base64_encode_three_bytes() {
    assert_eq!(base64_encode(b"foo"), "Zm9v");
}

#[test]
fn base64_encode_known_values() {
    assert_eq!(base64_encode(b"Hello"), "SGVsbG8=");
    assert_eq!(base64_encode(b"Hi!"), "SGkh");
    assert_eq!(base64_encode(b"ab"), "YWI=");
    assert_eq!(base64_encode(b"abc"), "YWJj");
    assert_eq!(base64_encode(b"Man"), "TWFu");
}

#[test]
fn base64_encode_long_input() {
    let input = "The quick brown fox jumps over the lazy dog. ".repeat(10);
    let encoded = base64_encode(input.as_bytes());
    assert!(encoded.len() > input.len());
    assert!(encoded.ends_with('=') || !encoded.contains('='));
}

#[test]
fn copy_to_clipboard_does_not_panic() {
    // Succeeds via an external tool or the OSC 52 fallback.
    copy_to_clipboard("test text").expect("copy should succeed");
}

#[test]
fn copy_to_clipboard_empty_string() {
    copy_to_clipboard("").expect("copy should succeed");
}

#[test]
fn safe_url_accepts_http_and_https() {
    assert!(is_safe_url("https://example.com"));
    assert!(is_safe_url("http://example.com/path?q=1#frag"));
    assert!(is_safe_url("https://user@example.com:8080/x"));
}

#[test]
fn safe_url_rejects_non_http_schemes() {
    assert!(!is_safe_url("file:///etc/passwd"));
    assert!(!is_safe_url("javascript:alert(1)"));
    assert!(!is_safe_url("ftp://example.com"));
    assert!(!is_safe_url("example.com/no-scheme"));
    assert!(!is_safe_url(""));
}

#[test]
fn safe_url_rejects_missing_host() {
    assert!(!is_safe_url("https://"));
    assert!(!is_safe_url("http:///path"));
}

#[test]
fn safe_url_rejects_whitespace_and_control_chars() {
    assert!(!is_safe_url("https://example.com/a b"));
    assert!(!is_safe_url("https://example.com/\nevil"));
    assert!(!is_safe_url("https://example.com/\x07"));
}

#[test]
fn safe_url_rejects_overlong_urls() {
    let long = format!("https://example.com/{}", "a".repeat(2100));
    assert!(!is_safe_url(&long));
}

#[test]
fn chat_margin_reduces_content_width() {
    let mut r = crate::ui::renderer::Renderer::new().unwrap();
    let full = r.line_width();
    r.set_chat_margin(4);
    assert_eq!(r.line_width(), full.saturating_sub(4));
    // Zero margin leaves the width unchanged.
    r.set_chat_margin(0);
    assert_eq!(r.line_width(), full);
}

mod dirty {
    use crate::ui::feed::BlockStyle;
    use crate::ui::renderer::{BottomRedrawPlan, BottomSnapshot, PromptSnapshot, Renderer};
    use crate::ui::statusline::StatusSpan;

    fn bottom_snapshot() -> BottomSnapshot {
        BottomSnapshot {
            cols: 80,
            rows: 24,
            statusline_height: 1,
            input: String::new(),
            cursor_pos: 0,
            is_running: false,
            spinner_frame: 0,
            input_vscroll_offset: 0,
            prompt: PromptSnapshot::Input,
            statusline: vec![vec![StatusSpan::Text {
                text: "model".to_string(),
                fg: None,
                bg: None,
            }]],
            scroll_indicator: false,
            monochrome: false,
            input_bg: None,
            status_bg: None,
        }
    }

    #[test]
    fn fresh_renderer_needs_chat_redraw() {
        let r = Renderer::new().unwrap();
        assert!(r.chat_needs_redraw());
    }

    #[test]
    fn chat_clean_after_mark_clean() {
        let mut r = Renderer::new().unwrap();
        r.mark_chat_clean();
        assert!(!r.chat_needs_redraw());
    }

    #[test]
    fn feed_mut_mutation_triggers_chat_redraw() {
        let mut r = Renderer::new().unwrap();
        r.mark_chat_clean();
        r.feed_mut().push_block(BlockStyle::Plain, "hello");
        assert!(r.chat_needs_redraw());
    }

    #[test]
    fn scroll_triggers_chat_redraw() {
        let mut r = Renderer::new().unwrap();
        // Enough lines to overflow the (fallback 80x24) viewport.
        for i in 0..40 {
            r.feed_mut()
                .push_line(BlockStyle::Plain, format!("line {i}"));
        }
        r.mark_chat_clean();
        assert!(!r.chat_needs_redraw());
        r.scroll_line_up();
        assert!(r.chat_needs_redraw());
    }

    #[test]
    fn resize_marks_chat_dirty() {
        let mut r = Renderer::new().unwrap();
        r.mark_chat_clean();
        r.resize();
        assert!(r.chat_needs_redraw());
    }

    #[test]
    fn selection_change_triggers_chat_redraw() {
        let mut r = Renderer::new().unwrap();
        r.feed_mut().push_line(BlockStyle::Plain, "selectable");
        r.mark_chat_clean();
        assert!(!r.chat_needs_redraw());
        // Selection fields are public and mutated directly by callers.
        r.selection_active = true;
        r.selection_start = Some(0);
        r.selection_end = Some(0);
        assert!(r.chat_needs_redraw());
        r.mark_chat_clean();
        r.clear_selection();
        assert!(r.chat_needs_redraw());
    }

    #[test]
    fn invalidate_marks_chat_dirty() {
        let mut r = Renderer::new().unwrap();
        r.mark_chat_clean();
        r.invalidate();
        assert!(r.chat_needs_redraw());
    }

    #[test]
    fn bottom_plan_full_when_no_previous() {
        let next = bottom_snapshot();
        assert_eq!(
            Renderer::bottom_redraw_plan(None, &next, false),
            BottomRedrawPlan::Full
        );
    }

    #[test]
    fn bottom_plan_skip_when_unchanged() {
        let prev = bottom_snapshot();
        let next = bottom_snapshot();
        assert_eq!(
            Renderer::bottom_redraw_plan(Some(&prev), &next, false),
            BottomRedrawPlan::Skip
        );
    }

    #[test]
    fn bottom_plan_force_full() {
        let prev = bottom_snapshot();
        let next = bottom_snapshot();
        assert_eq!(
            Renderer::bottom_redraw_plan(Some(&prev), &next, true),
            BottomRedrawPlan::Full
        );
    }

    #[test]
    fn bottom_plan_statusline_only_on_statusline_change() {
        let prev = bottom_snapshot();
        let mut next = bottom_snapshot();
        next.statusline = vec![vec![StatusSpan::Text {
            text: "other model".to_string(),
            fg: None,
            bg: None,
        }]];
        assert_eq!(
            Renderer::bottom_redraw_plan(Some(&prev), &next, false),
            BottomRedrawPlan::StatuslineOnly
        );
    }

    #[test]
    fn bottom_plan_statusline_only_on_scroll_indicator_change() {
        let prev = bottom_snapshot();
        let mut next = bottom_snapshot();
        next.scroll_indicator = true;
        assert_eq!(
            Renderer::bottom_redraw_plan(Some(&prev), &next, false),
            BottomRedrawPlan::StatuslineOnly
        );
    }

    #[test]
    fn bottom_plan_full_on_input_change() {
        let prev = bottom_snapshot();
        let mut next = bottom_snapshot();
        next.input = "typed".to_string();
        assert_eq!(
            Renderer::bottom_redraw_plan(Some(&prev), &next, false),
            BottomRedrawPlan::Full
        );
    }

    #[test]
    fn bottom_plan_full_on_cursor_change() {
        let prev = bottom_snapshot();
        let mut next = bottom_snapshot();
        next.cursor_pos = 3;
        assert_eq!(
            Renderer::bottom_redraw_plan(Some(&prev), &next, false),
            BottomRedrawPlan::Full
        );
    }

    #[test]
    fn bottom_plan_full_on_prompt_mode_change() {
        let prev = bottom_snapshot();
        let mut next = bottom_snapshot();
        next.prompt = PromptSnapshot::Chain {
            question: "continue?".into(),
            but_mode: false,
        };
        assert_eq!(
            Renderer::bottom_redraw_plan(Some(&prev), &next, false),
            BottomRedrawPlan::Full
        );
    }

    #[test]
    fn bottom_plan_full_on_geometry_change() {
        let prev = bottom_snapshot();
        let mut next = bottom_snapshot();
        next.rows = 40;
        assert_eq!(
            Renderer::bottom_redraw_plan(Some(&prev), &next, false),
            BottomRedrawPlan::Full
        );
    }

    #[test]
    fn bottom_plan_full_on_spinner_frame_change() {
        let prev = bottom_snapshot();
        let mut next = bottom_snapshot();
        next.is_running = true;
        next.spinner_frame = 1;
        assert_eq!(
            Renderer::bottom_redraw_plan(Some(&prev), &next, false),
            BottomRedrawPlan::Full
        );
    }

    #[test]
    fn bottom_plan_full_on_input_scroll_change() {
        let prev = bottom_snapshot();
        let mut next = bottom_snapshot();
        next.input_vscroll_offset = 1;
        assert_eq!(
            Renderer::bottom_redraw_plan(Some(&prev), &next, false),
            BottomRedrawPlan::Full
        );
    }

    #[test]
    fn bottom_plan_full_when_statusline_and_input_change() {
        let prev = bottom_snapshot();
        let mut next = bottom_snapshot();
        next.input = "typed".to_string();
        next.statusline = Vec::new();
        assert_eq!(
            Renderer::bottom_redraw_plan(Some(&prev), &next, false),
            BottomRedrawPlan::Full
        );
    }
}

/// Drive `draw_bottom` through a `FakeBackend` and read back the emitted
/// caret (row, col). Content-agnostic: works for any input string.
mod cursor_positioning {
    use crate::ui::renderer::{FakeBackend, Renderer};
    use regex::Regex;

    const COLS: u16 = 80;
    const PROMPT_WIDTH: usize = 2;
    const VISIBLE_WIDTH: usize = COLS as usize - PROMPT_WIDTH;

    fn emitted_cursor(input: &str, cursor: usize) -> (u16, u16) {
        let mut r = Renderer::with_backend(Box::new(FakeBackend::new(COLS, 24)));
        r.set_statusline_height(1);
        r.draw_bottom(input, cursor, &[], false).unwrap();
        let out = r.captured_output();
        // ANSI Cursor Position: ESC[row;colH (1-based), as emitted by crossterm's MoveTo.
        let re = Regex::new(r"\x1b\[(\d+);(\d+)H").unwrap();
        re.captures_iter(&out)
            .last()
            .map(|c| {
                (
                    c[1].parse::<u16>().unwrap() - 1,
                    c[2].parse::<u16>().unwrap() - 1,
                )
            })
            .expect("renderer must emit a cursor MoveTo")
    }

    #[test]
    fn empty_buffer_cursor_after_prompt() {
        assert_eq!(emitted_cursor("", 0).1, 2);
    }

    #[test]
    fn cursor_after_one_ascii_char() {
        assert_eq!(emitted_cursor("a", 1).1, 3);
    }

    #[test]
    fn cursor_after_three_ascii_chars() {
        assert_eq!(emitted_cursor("abc", 3).1, 5);
    }

    #[test]
    fn cursor_mid_line() {
        assert_eq!(emitted_cursor("abc", 1).1, 3);
    }

    fn repeat_ascii(n: usize) -> String {
        "a".repeat(n)
    }

    #[test]
    fn cursor_at_end_of_short_line() {
        // Line fits: caret is one past the last char.
        let line = repeat_ascii(5);
        assert_eq!(emitted_cursor(&line, 5).1, (PROMPT_WIDTH + 5) as u16);
    }

    #[test]
    fn cursor_at_end_of_full_width_line() {
        // Line exactly fills the row; it scrolls one column left so the caret
        // stays on-screen ONE PAST the last visible char — at the final column
        // (COLS-1), not on it (COLS-2) and not off-screen (COLS).
        let line = repeat_ascii(VISIBLE_WIDTH);
        assert_eq!(emitted_cursor(&line, VISIBLE_WIDTH).1, COLS - 1);
    }

    #[test]
    fn cursor_at_end_of_overflowing_line() {
        let line = repeat_ascii(VISIBLE_WIDTH + 12);
        assert_eq!(emitted_cursor(&line, VISIBLE_WIDTH + 12).1, COLS - 1);
    }
}

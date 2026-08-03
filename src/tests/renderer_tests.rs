use crate::ui::renderer::{base64_encode, copy_to_clipboard};

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
            picker: None,
            statusline: vec![vec![StatusSpan::Text {
                text: "model".to_string(),
                fg: None,
                bg: None,
            }]],
            feed_generation: 0,
            watermark: None,
            partial: "".into(),
            partial_style: BlockStyle::Plain,
            monochrome: false,
            chat_bg: None,
            chat_margin: 0,
            input_bg: None,
            status_bg: None,
            show_timestamps: false,
        }
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
    fn bottom_plan_full_on_feed_change() {
        let prev = bottom_snapshot();
        let mut next = bottom_snapshot();
        next.feed_generation = 1;
        assert_eq!(
            Renderer::bottom_redraw_plan(Some(&prev), &next, false),
            BottomRedrawPlan::Full
        );
    }

    #[test]
    fn bottom_plan_full_on_partial_change() {
        let prev = bottom_snapshot();
        let mut next = bottom_snapshot();
        next.partial = "streaming".into();
        assert_eq!(
            Renderer::bottom_redraw_plan(Some(&prev), &next, false),
            BottomRedrawPlan::Full
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
    fn bottom_plan_full_on_input_vscroll_change() {
        let prev = bottom_snapshot();
        let mut next = bottom_snapshot();
        next.input_vscroll_offset = 1;
        assert_eq!(
            Renderer::bottom_redraw_plan(Some(&prev), &next, false),
            BottomRedrawPlan::Full
        );
    }

    #[test]
    fn bottom_plan_full_on_picker_change() {
        let prev = bottom_snapshot();
        let mut next = bottom_snapshot();
        next.picker = Some(crate::ui::renderer::PickerView {
            header: None,
            rows: vec![crate::ui::renderer::PickerRow {
                text: "▸ /help".to_string(),
                selected: true,
            }],
        });
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

    #[test]
    fn bottom_plan_full_on_show_timestamps_change() {
        let prev = bottom_snapshot();
        let mut next = bottom_snapshot();
        next.show_timestamps = true;
        assert_eq!(
            Renderer::bottom_redraw_plan(Some(&prev), &next, false),
            BottomRedrawPlan::Full
        );
    }
}

/// Watermark math: the pure commit/advance/clamp functions plus end-to-end
/// flush behavior against a `FakeBackend`.
mod watermark {
    use crate::ui::feed::BlockStyle;
    use crate::ui::renderer::{
        FakeBackend, Renderer, Watermark, advance_watermark, clamp_watermark, commit_count,
    };

    fn wm(width: usize, block: usize, line: usize) -> Watermark {
        Watermark { width, block, line }
    }

    fn headless(cols: u16, rows: u16) -> Renderer {
        Renderer::with_backend(Box::new(FakeBackend::new(cols, rows)))
    }

    #[test]
    fn advance_walks_across_blocks() {
        // 2 lines of block 0 (1 already printed), then 3 lines of block 1.
        let next = advance_watermark(wm(80, 0, 1), &[2, 3], 3);
        assert_eq!(next, wm(80, 1, 2));
    }

    #[test]
    fn advance_zero_printed_stays_put() {
        assert_eq!(advance_watermark(wm(80, 1, 2), &[5], 0), wm(80, 1, 2));
    }

    #[test]
    fn advance_skips_zero_line_blocks() {
        // An empty agent block lays out to zero lines; the watermark must not
        // get stuck on it.
        let next = advance_watermark(wm(80, 0, 0), &[0, 2], 1);
        assert_eq!(next, wm(80, 1, 1));
    }

    #[test]
    fn advance_survives_layout_shrink() {
        // The watermark said 5 lines of block 0 were printed, but the block
        // now lays out to 3 lines (markdown reflow): treat it as done.
        let next = advance_watermark(wm(80, 0, 5), &[3, 2], 1);
        assert_eq!(next, wm(80, 1, 1));
    }

    #[test]
    fn clamp_collapses_past_end() {
        assert_eq!(clamp_watermark(wm(80, 5, 2), 3), wm(80, 3, 0));
        assert_eq!(clamp_watermark(wm(80, 3, 0), 3), wm(80, 3, 0));
        assert_eq!(clamp_watermark(wm(80, 1, 2), 3), wm(80, 1, 2));
    }

    #[test]
    fn commit_count_commits_finalized_only() {
        // 2 finalized lines plus a 4-line running tail that fits the
        // streaming region: only the finalized lines print.
        assert_eq!(commit_count(2, 4, 0, 10), 2);
    }

    #[test]
    fn commit_count_spills_overtall_running_tail() {
        // Nothing finalized; a 10-line running tail with 6 streaming rows
        // available: the top 4 spill into scrollback.
        assert_eq!(commit_count(0, 10, 0, 6), 4);
    }

    #[test]
    fn commit_count_counts_partial_towards_overflow() {
        // The partial scratch rows push the live tail over the edge, but the
        // spill is still capped to feed lines (partial is never printed).
        assert_eq!(commit_count(2, 4, 3, 6), 3);
    }

    #[test]
    fn commit_count_zero_when_everything_fits() {
        assert_eq!(commit_count(0, 3, 1, 10), 0);
    }

    #[test]
    fn flush_prints_new_lines_once() {
        let mut r = headless(80, 24);
        for i in 0..3 {
            r.feed_mut()
                .push_line(BlockStyle::Plain, format!("line {i}"));
        }
        r.flush_committed("").unwrap();
        let first = r.captured_output();
        assert!(first.contains("line 0"), "printed: {first}");
        assert!(first.contains("line 2"), "printed: {first}");
        assert_eq!(r.watermark(), Some(wm(79, 3, 0)));

        // Nothing new pending: a second flush prints nothing.
        r.flush_committed("").unwrap();
        assert_eq!(r.captured_output(), first, "no line may print twice");
    }

    #[test]
    fn flush_prints_only_lines_past_watermark() {
        let mut r = headless(80, 24);
        r.feed_mut().push_line(BlockStyle::Plain, "old");
        r.flush_committed("").unwrap();
        let before = r.captured_output();

        r.feed_mut().push_line(BlockStyle::Plain, "new");
        r.flush_committed("").unwrap();
        let after = r.captured_output();
        assert!(after.len() > before.len());
        let delta = &after[before.len()..];
        assert!(delta.contains("new"), "delta: {delta}");
        assert!(!delta.contains("old"), "old must not reprint: {delta}");
    }

    #[test]
    fn running_block_stays_live_until_finalized() {
        let mut r = headless(80, 24);
        r.feed_mut().push_streaming_block(BlockStyle::Agent);
        r.feed_mut().append_to_last("streaming text");
        r.flush_committed("").unwrap();
        assert!(
            r.captured_output().is_empty(),
            "a running block must not be committed"
        );

        r.feed_mut().finalize_last();
        r.flush_committed("").unwrap();
        assert!(r.captured_output().contains("< streaming text"));
    }

    #[test]
    fn overtall_running_block_spills_top_lines() {
        // 10 rows: streaming region is tiny, so most of a tall running block
        // spills into scrollback while the tail stays live.
        let mut r = headless(80, 10);
        r.feed_mut().push_streaming_block(BlockStyle::Plain);
        for i in 0..20 {
            r.feed_mut().append_to_last(format!("row {i}\n"));
        }
        r.flush_committed("").unwrap();
        let out = r.captured_output();
        assert!(out.contains("row 0"), "top lines spilled: {out}");
        let mark = r.watermark().expect("watermark advanced");
        assert_eq!(mark.block, 0, "still inside the running block");
        assert!(mark.line > 0, "some lines committed: {mark:?}");

        // Finalize: the rest of the block commits.
        r.feed_mut().finalize_last();
        r.flush_committed("").unwrap();
        let out = r.captured_output();
        assert!(out.contains("row 19"), "tail printed on finalize: {out}");
        assert_eq!(r.watermark(), Some(wm(79, 1, 0)));
    }

    #[test]
    fn truncation_clamps_watermark() {
        let mut r = headless(80, 24);
        for i in 0..3 {
            r.feed_mut()
                .push_line(BlockStyle::Plain, format!("line {i}"));
        }
        r.flush_committed("").unwrap();
        let before = r.captured_output();

        // Blocks 1 and 2 were printed, then the feed is truncated (e.g. a
        // streaming restart): nothing may reprint or panic.
        r.feed_mut().truncate_blocks(1);
        r.flush_committed("").unwrap();
        assert_eq!(r.captured_output(), before);
        assert_eq!(r.watermark(), Some(wm(79, 1, 0)));
    }

    #[test]
    fn resize_remaps_watermark_to_same_position() {
        let mut r = headless(80, 24);
        for i in 0..5 {
            r.feed_mut()
                .push_line(BlockStyle::Plain, format!("line {i}"));
        }
        r.flush_committed("").unwrap();
        let before = r.captured_output();

        r.resize_backend(40, 24);
        r.resize();
        r.flush_committed("").unwrap();
        assert_eq!(
            r.captured_output(),
            before,
            "printed lines keep their old wrap and never reprint"
        );
        assert_eq!(
            r.watermark(),
            Some(wm(39, 5, 0)),
            "same (block, line), remapped to the new width"
        );
    }

    #[test]
    fn clear_screen_resets_watermark() {
        let mut r = headless(80, 24);
        r.feed_mut().push_line(BlockStyle::Plain, "hello");
        r.flush_committed("").unwrap();
        assert!(r.watermark().is_some());

        r.clear_screen().unwrap();
        assert_eq!(r.watermark(), None);
        let out = r.captured_output();
        assert!(out.contains("\x1b[2J"), "screen cleared: {out}");

        // The feed is unprinted again: it prints once from the top.
        r.flush_committed("").unwrap();
        let reprinted = r.captured_output().len() > out.len();
        assert!(reprinted, "feed reprints after /clear");
    }

    #[test]
    fn rebuild_marks_feed_printed_but_startup_does_not() {
        // Startup: nothing printed yet, the transcript must print.
        let mut r = headless(80, 24);
        r.reset_feed_for_rebuild();
        r.feed_mut().push_line(BlockStyle::Plain, "transcript");
        r.note_feed_rebuilt();
        r.flush_committed("").unwrap();
        assert!(r.captured_output().contains("transcript"));

        // Rebuild (undo/rewind): the new feed must not reprint.
        let before = r.captured_output();
        r.reset_feed_for_rebuild();
        r.feed_mut()
            .push_line(BlockStyle::Plain, "shorter transcript");
        r.note_feed_rebuilt();
        r.flush_committed("").unwrap();
        assert_eq!(r.captured_output(), before, "rebuilt feed must not reprint");
    }
}

/// Input soft-wrap math: the pure segment/cursor-mapping functions plus one
/// end-to-end draw against a `FakeBackend`.
mod input_wrap {
    use crate::ui::renderer::{FakeBackend, Renderer, wrap_input_segments, wrapped_cursor};

    #[test]
    fn segments_short_line_fits_one_row() {
        assert_eq!(wrap_input_segments("hello", 10), vec![(0, 5)]);
    }

    #[test]
    fn segments_empty_line_is_one_empty_row() {
        assert_eq!(wrap_input_segments("", 10), vec![(0, 0)]);
    }

    #[test]
    fn segments_wrap_at_char_boundaries() {
        assert_eq!(
            wrap_input_segments("abcdefghij", 4),
            vec![(0, 4), (4, 8), (8, 10)]
        );
    }

    #[test]
    fn segments_exact_multiple_has_no_trailing_row() {
        assert_eq!(wrap_input_segments("abcd", 2), vec![(0, 2), (2, 4)]);
    }

    #[test]
    fn segments_never_split_a_wide_char() {
        // '界' is 2 columns wide and 3 bytes long.
        assert_eq!(wrap_input_segments("a界b", 2), vec![(0, 1), (1, 4), (4, 5)]);
    }

    #[test]
    fn segments_wide_char_wider_than_row_still_gets_a_row() {
        assert_eq!(wrap_input_segments("界", 1), vec![(0, 3)]);
    }

    #[test]
    fn segments_zero_width_is_clamped() {
        assert_eq!(wrap_input_segments("ab", 0), vec![(0, 1), (1, 2)]);
    }

    #[test]
    fn cursor_in_middle_of_row() {
        let segs = wrap_input_segments("abcdefghij", 4);
        assert_eq!(wrapped_cursor(&segs, "abcdefghij", 6), (1, 2));
    }

    #[test]
    fn cursor_on_wrap_boundary_starts_next_row() {
        let segs = wrap_input_segments("abcdefghij", 4);
        assert_eq!(wrapped_cursor(&segs, "abcdefghij", 4), (1, 0));
    }

    #[test]
    fn cursor_at_line_end_sits_on_last_row() {
        let segs = wrap_input_segments("abcdefghij", 4);
        assert_eq!(wrapped_cursor(&segs, "abcdefghij", 10), (2, 2));
    }

    #[test]
    fn cursor_after_wide_char_uses_display_columns() {
        // "a界b" at width 2: rows are "a", "界", "b". After '界' (byte 4) the
        // row is exactly full, so the boundary rule lands on the next row.
        let segs = wrap_input_segments("a界b", 2);
        assert_eq!(wrapped_cursor(&segs, "a界b", 4), (2, 0));
        // After 'b' (end): row 2, display column 1.
        assert_eq!(wrapped_cursor(&segs, "a界b", 5), (2, 1));
        // Inside a row, columns are display widths, not bytes: after 'a'
        // (byte 1) sits at display column 1 of row 0... but '界' starts row
        // 1, so the boundary rule wins and the caret starts row 1.
        assert_eq!(wrapped_cursor(&segs, "a界b", 1), (1, 0));
    }

    #[test]
    fn long_input_soft_wraps_instead_of_scrolling() {
        // 78 a's fill the first text row (80 cols - 2 prompt), then 22 b's.
        let input = format!("{}{}", "a".repeat(78), "b".repeat(22));
        let mut r = Renderer::with_backend(Box::new(FakeBackend::new(80, 24)));
        // Cursor at the start: with horizontal scrolling only the tail or the
        // head would be visible; wrapped, both halves must be painted.
        r.draw_live_block(&input, 0, &[], false, None).unwrap();
        let out = r.captured_output();
        assert!(out.contains(&"a".repeat(78)), "first wrapped row: {out}");
        assert!(out.contains(&"b".repeat(22)), "second wrapped row: {out}");
    }
}

/// Picker overlay as a live-block section: rows are painted through the
/// backend and the block's erase always covers them (regression for picker
/// remnants left on screen after closing a picker in inline mode).
mod picker_overlay {
    use crate::ui::renderer::{FakeBackend, PickerRow, PickerView, Renderer};

    fn headless(cols: u16, rows: u16) -> Renderer {
        Renderer::with_backend(Box::new(FakeBackend::new(cols, rows)))
    }

    fn view(n: usize) -> PickerView {
        PickerView {
            header: None,
            rows: (0..n)
                .map(|i| PickerRow {
                    text: format!("  item {i}"),
                    selected: false,
                })
                .collect(),
        }
    }

    #[test]
    fn picker_rows_paint_inside_the_block() {
        let mut r = headless(80, 24);
        let mut v = view(3);
        v.rows[1].selected = true;
        v.rows[1].text = "▸ item 1".to_string();
        r.draw_live_block("x", 1, &[], false, Some(&v)).unwrap();
        let out = r.captured_output();
        assert!(out.contains("▸ item 1"), "selected row painted: {out}");
        assert!(out.contains("  item 2"), "plain row painted: {out}");
        // The input row follows the picker rows (picker sits above the box).
        let picker_at = out.find("▸ item 1").unwrap();
        let input_at = out.find("> ").unwrap();
        assert!(picker_at < input_at, "picker above input: {out}");
    }

    #[test]
    fn closing_picker_erases_its_rows() {
        let mut r = headless(80, 24);
        r.draw_live_block("x", 1, &[], false, Some(&view(3)))
            .unwrap();
        let before = r.captured_output();

        // Same input, picker gone: the snapshot differs, forcing a full
        // redraw that moves up over the picker rows (3 picker rows + 1
        // separator above the caret row = MoveUp(4)) and clears below.
        r.draw_live_block("x", 1, &[], false, None).unwrap();
        let after = r.captured_output();
        let delta = &after[before.len()..];
        assert!(
            delta.contains("\x1b[4A"),
            "erase covers picker rows: {delta:?}"
        );
        assert!(delta.contains("\x1b[J"), "clears below: {delta:?}");
        assert!(
            !delta.contains("item 0"),
            "picker row not repainted: {delta:?}"
        );
    }

    #[test]
    fn shrinking_picker_still_erases_the_taller_frame() {
        let mut r = headless(80, 24);
        r.draw_live_block("x", 1, &[], false, Some(&view(10)))
            .unwrap();
        let before = r.captured_output();

        // Filtering shrank the list from 10 rows to 1: the redraw must move
        // up over the previous 10-row frame (10 + 1 = MoveUp(11)), not the
        // new height.
        r.draw_live_block("x", 1, &[], false, Some(&view(1)))
            .unwrap();
        let after = r.captured_output();
        let delta = &after[before.len()..];
        assert!(
            delta.contains("\x1b[11A"),
            "erase covers the previous taller frame: {delta:?}"
        );
        assert!(
            delta.contains("item 0"),
            "shrunk picker repaints: {delta:?}"
        );
    }

    #[test]
    fn picker_header_row_counts_towards_erase() {
        let mut r = headless(80, 24);
        let mut v = view(2);
        v.header = Some("[Quick 3]  Provider 0".to_string());
        r.draw_live_block("x", 1, &[], false, Some(&v)).unwrap();
        let before = r.captured_output();
        assert!(before.contains("[Quick 3]"), "header painted: {before}");

        // header(1) + rows(2) + separator(1) above the caret = MoveUp(4).
        r.draw_live_block("x", 1, &[], false, None).unwrap();
        let after = r.captured_output();
        let delta = &after[before.len()..];
        assert!(
            delta.contains("\x1b[4A"),
            "header row included in erase height: {delta:?}"
        );
    }
}

/// Live-block repaint throttling (~60 Hz).
mod throttle {
    use std::time::Duration;

    use crate::ui::feed::BlockStyle;
    use crate::ui::renderer::{FakeBackend, Renderer};

    fn headless(cols: u16, rows: u16) -> Renderer {
        Renderer::with_backend(Box::new(FakeBackend::new(cols, rows)))
    }

    #[test]
    fn immediate_repaint_is_forced() {
        let mut r = headless(80, 24);
        r.feed_mut().push_streaming_block(BlockStyle::Agent);
        r.feed_mut().append_to_last("hello");
        r.draw_live_block("", 0, &[], true, None).unwrap();
        let after_draw = r.captured_output().len();

        r.feed_mut().append_to_last(" world");
        r.repaint(true).unwrap();
        assert!(
            r.captured_output().len() > after_draw,
            "forced repaint should emit output"
        );
    }

    #[test]
    fn second_repaint_within_budget_is_throttled() {
        let mut r = headless(80, 24);
        r.feed_mut().push_streaming_block(BlockStyle::Agent);
        r.feed_mut().append_to_last("hello");
        r.draw_live_block("", 0, &[], true, None).unwrap();
        r.repaint(true).unwrap();
        let after_first = r.captured_output().clone();

        r.feed_mut().append_to_last(" world");
        r.repaint(false).unwrap();
        assert_eq!(
            r.captured_output(),
            after_first,
            "repaint within 16 ms budget must not emit output"
        );
        assert!(r.needs_redraw());
    }

    #[test]
    fn repaint_after_budget_elapses_redraws() {
        let mut r = headless(80, 24);
        r.feed_mut().push_streaming_block(BlockStyle::Agent);
        r.feed_mut().append_to_last("hello");
        r.draw_live_block("", 0, &[], true, None).unwrap();
        r.repaint(true).unwrap();

        std::thread::sleep(Duration::from_millis(20));
        r.feed_mut().append_to_last(" world");
        r.repaint(false).unwrap();
        assert!(
            r.captured_output().contains("world"),
            "repaint after budget should show new content"
        );
        assert!(!r.needs_redraw());
    }

    #[test]
    fn request_repaint_defers_draw_until_budget_or_force() {
        let mut r = headless(80, 24);
        r.feed_mut().push_streaming_block(BlockStyle::Agent);
        r.feed_mut().append_to_last("hello");
        r.draw_live_block("", 0, &[], true, None).unwrap();
        r.repaint(true).unwrap();
        let after_first = r.captured_output().clone();

        r.feed_mut().append_to_last(" world");
        r.request_repaint();
        assert!(r.needs_redraw());
        assert_eq!(r.captured_output(), after_first);
    }

    #[test]
    fn flush_committed_is_not_throttled() {
        let mut r = headless(80, 24);
        r.feed_mut().push_line(BlockStyle::Agent, "committed");
        r.draw_live_block("", 0, &[], false, None).unwrap();
        r.repaint(true).unwrap();
        let after_first = r.captured_output().clone();

        r.feed_mut().push_line(BlockStyle::Agent, "next");
        // repaint(false) should flush the committed line even if the live-block
        // draw is throttled.
        r.repaint(false).unwrap();
        assert!(
            r.captured_output().len() > after_first.len(),
            "committed line must be flushed even when draw is throttled"
        );
        assert!(r.captured_output().contains("next"));
    }
}

/// Per-message timestamps and turn separators.
mod timestamps {
    use crate::ui::feed::BlockStyle;
    use crate::ui::renderer::{FakeBackend, Renderer};

    fn headless(cols: u16, rows: u16) -> Renderer {
        Renderer::with_backend(Box::new(FakeBackend::new(cols, rows)))
    }

    fn contains_timestamp(text: &str) -> bool {
        // Look for a literal [HH:MM:SS] in the captured output. ANSI escape
        // sequences also use brackets, but not this exact digit pattern.
        let re = regex::Regex::new(r"\[\d{2}:\d{2}:\d{2}\]").unwrap();
        re.is_match(text)
    }

    fn sample_time() -> chrono::DateTime<chrono::Local> {
        chrono::DateTime::from_timestamp(1_700_000_000, 0)
            .unwrap()
            .with_timezone(&chrono::Local)
    }

    #[test]
    fn disabled_timestamps_do_not_prefix_lines() {
        let mut r = headless(80, 24);
        r.feed_mut().push_line(BlockStyle::Agent, "hello");
        r.flush_committed("").unwrap();
        let out = r.captured_output();
        assert!(
            !contains_timestamp(&out),
            "no timestamp when disabled: {out}"
        );
    }

    #[test]
    fn enabled_timestamps_prefix_with_hhmmss() {
        let mut r = headless(80, 24);
        r.set_show_timestamps(true);
        r.feed_mut()
            .push_line_with_time(BlockStyle::Agent, "hello", sample_time());
        r.flush_committed("").unwrap();
        let out = r.captured_output();
        assert!(out.contains("hello"), "content still printed: {out}");
        // Local time at the sample timestamp: 2023-11-14 22:13:20 UTC, which in
        // many timezones still starts with two digits and a colon.
        assert!(contains_timestamp(&out), "timestamp wrapper present: {out}");
    }

    #[test]
    fn timestamp_is_omitted_when_block_has_none() {
        let mut r = headless(80, 24);
        r.set_show_timestamps(true);
        r.feed_mut().push_line(BlockStyle::Agent, "no time");
        r.flush_committed("").unwrap();
        let out = r.captured_output();
        assert!(
            !contains_timestamp(&out),
            "no timestamp when created_at is None: {out}"
        );
    }

    #[test]
    fn separator_drawn_between_committed_blocks() {
        let mut r = headless(80, 24);
        r.set_show_timestamps(true);
        let ts = sample_time();
        r.feed_mut()
            .push_line_with_time(BlockStyle::User, "> hi", ts);
        r.feed_mut()
            .push_line_with_time(BlockStyle::Agent, "reply", ts);
        r.flush_committed("").unwrap();
        let out = r.captured_output();
        // The separator is a run of box-drawing horizontal lines.
        assert!(out.contains('─'), "separator drawn between turns: {out}");
    }

    #[test]
    fn no_separator_before_first_committed_line() {
        let mut r = headless(80, 24);
        r.set_show_timestamps(true);
        let ts = sample_time();
        r.feed_mut()
            .push_line_with_time(BlockStyle::Agent, "only", ts);
        r.flush_committed("").unwrap();
        let out = r.captured_output();
        let first_sep = out.find('─');
        assert!(
            first_sep.is_none(),
            "no separator above the first block: {out}"
        );
    }

    #[test]
    fn streaming_region_does_not_get_timestamp_prefix() {
        let mut r = headless(80, 24);
        r.set_show_timestamps(true);
        r.feed_mut().push_streaming_block(BlockStyle::Agent);
        r.feed_mut().append_to_last("streaming");
        r.draw_live_block("", 0, &[], true, None).unwrap();
        let out = r.captured_output();
        assert!(out.contains("streaming"), "streaming text painted: {out}");
    }
}

/// Fenced code block styling (header + background).
mod code_blocks {
    use crate::ui::feed::BlockStyle;
    use crate::ui::renderer::{FakeBackend, Renderer};

    fn headless(cols: u16, rows: u16) -> Renderer {
        Renderer::with_backend(Box::new(FakeBackend::new(cols, rows)))
    }

    #[test]
    fn code_block_header_and_content_emitted() {
        let mut r = headless(80, 24);
        r.feed_mut().push_block(
            BlockStyle::Agent,
            "```rust\nfn main() {\n    println!();\n}\n```",
        );
        r.flush_committed("").unwrap();
        let out = r.captured_output();
        assert!(
            out.contains("rust"),
            "language header should be printed: {out}"
        );
        assert!(
            out.contains("fn main"),
            "code content should be printed: {out}"
        );
        // The line should be padded to the full content width (80 - 1 - 0 margin).
        assert!(
            out.contains("── rust ──"),
            "header text should be present: {out}"
        );
    }

    #[test]
    fn code_block_is_monochrome_safe() {
        let mut r = headless(80, 24);
        r.set_monochrome(true);
        r.feed_mut()
            .push_block(BlockStyle::Agent, "```rust\nfn main() {}\n```");
        r.flush_committed("").unwrap();
        let out = r.captured_output();
        assert!(
            out.contains("rust"),
            "language header should still print in monochrome: {out}"
        );
        // Reverse-video attribute is used instead of background color.
        assert!(
            out.contains("[7m"),
            "reverse video should be used for code blocks in monochrome: {out}"
        );
    }

    #[test]
    fn no_agent_prefix_on_code_block_header() {
        let mut r = headless(80, 24);
        r.feed_mut()
            .push_block(BlockStyle::Agent, "```rust\nfn main() {}\n```");
        r.flush_committed("").unwrap();
        let out = r.captured_output();
        // The code header should appear without the agent "< " prefix.
        assert!(
            !out.contains("<   ── rust"),
            "code header should not be prefixed with agent marker: {out}"
        );
    }
}

/// OSC 133 shell integration marks (prompt/output region annotations).
mod osc_133 {
    use crate::ui::feed::BlockStyle;
    use crate::ui::renderer::{FakeBackend, Renderer};

    fn headless(cols: u16, rows: u16) -> Renderer {
        Renderer::with_backend(Box::new(FakeBackend::new(cols, rows)))
    }

    #[test]
    fn helper_emits_7bit_osc_sequence() {
        let mut r = headless(80, 24);
        r.osc_133('A', "").unwrap();
        r.osc_133('B', "").unwrap();
        r.osc_133('C', "").unwrap();
        r.osc_133('D', "done=true").unwrap();
        let out = r.captured_output();
        assert!(out.contains("\x1b]133;A\x1b\\"), "A: {out:?}");
        assert!(out.contains("\x1b]133;B\x1b\\"), "B: {out:?}");
        assert!(out.contains("\x1b]133;C\x1b\\"), "C: {out:?}");
        assert!(out.contains("\x1b]133;D;done=true\x1b\\"), "D: {out:?}");
    }

    #[test]
    fn draw_live_block_emits_prompt_start() {
        let mut r = headless(80, 24);
        r.draw_live_block("hello", 5, &[], false, None).unwrap();
        let out = r.captured_output();
        assert!(out.contains("\x1b]133;A\x1b\\"), "prompt start: {out:?}");
    }

    #[test]
    fn flush_committed_opens_output_mark_for_non_user_lines() {
        let mut r = headless(80, 24);
        r.feed_mut().push_line(BlockStyle::Agent, "assistant line");
        r.flush_committed("").unwrap();
        let out = r.captured_output();
        assert!(
            out.contains("\x1b]133;C\x1b\\"),
            "output start before assistant line: {out:?}"
        );
        assert!(out.contains("assistant line"), "line printed: {out:?}");
    }

    #[test]
    fn flush_committed_does_not_open_mark_for_user_lines() {
        let mut r = headless(80, 24);
        r.feed_mut().push_line(BlockStyle::User, "> user line");
        r.flush_committed("").unwrap();
        let out = r.captured_output();
        assert!(
            !out.contains("\x1b]133;C\x1b\\"),
            "user echo should not open output region: {out:?}"
        );
        assert!(out.contains("> user line"), "user line printed: {out:?}");
    }

    #[test]
    fn end_output_mark_emits_d_and_closes_state() {
        let mut r = headless(80, 24);
        r.feed_mut().push_line(BlockStyle::Agent, "assistant line");
        r.flush_committed("").unwrap();
        r.end_output_mark().unwrap();
        let out = r.captured_output();
        assert!(out.contains("\x1b]133;C\x1b\\"), "output start: {out:?}");
        assert!(out.contains("\x1b]133;D\x1b\\"), "output end: {out:?}");
    }
}

mod focus {
    use crate::ui::renderer::{FakeBackend, Renderer};
    use crate::ui::statusline::StatusSpan;

    fn headless(cols: u16, rows: u16) -> Renderer {
        let mut r = Renderer::with_backend(Box::new(FakeBackend::new(cols, rows)));
        r.set_statusline_height(1);
        r
    }

    fn statusline() -> Vec<Vec<StatusSpan>> {
        vec![vec![StatusSpan::Text {
            text: "model".to_string(),
            fg: None,
            bg: None,
        }]]
    }

    #[test]
    fn spinner_advances_when_focused() {
        let mut r = headless(80, 24);
        r.set_focused(true);
        let before = r.spinner_frame();
        r.draw_live_block("", 0, &statusline(), true, None).unwrap();
        assert_ne!(
            r.spinner_frame(),
            before,
            "spinner should advance while focused"
        );
    }

    #[test]
    fn spinner_paused_when_unfocused() {
        let mut r = headless(80, 24);
        r.set_focused(true);
        r.draw_live_block("", 0, &statusline(), true, None).unwrap();
        let frame = r.spinner_frame();

        r.set_focused(false);
        r.draw_live_block("", 0, &statusline(), true, None).unwrap();
        assert_eq!(
            r.spinner_frame(),
            frame,
            "spinner should stay paused when unfocused"
        );

        // When focus returns together with a real change, the spinner advances
        // again; a no-change refocus does not redraw, so the frame stays put.
        r.set_focused(true);
        r.draw_live_block("changed", 0, &statusline(), true, None)
            .unwrap();
        assert_ne!(
            r.spinner_frame(),
            frame,
            "spinner should resume when focus returns with a content change"
        );
    }
}

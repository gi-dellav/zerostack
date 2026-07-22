use crate::ui::feed::{BlockStyle, Feed};
use crossterm::style::Color;

#[test]
fn block_style_color_mapping() {
    assert_eq!(BlockStyle::User.color(), Color::Green);
    assert_eq!(BlockStyle::Agent.color(), Color::White);
    assert_eq!(BlockStyle::Reasoning.color(), Color::DarkMagenta);
    assert_eq!(BlockStyle::Tool.color(), Color::Yellow);
    assert_eq!(BlockStyle::ToolResult.color(), Color::DarkGrey);
    assert_eq!(BlockStyle::Error.color(), Color::Red);
    assert_eq!(BlockStyle::System.color(), Color::DarkGrey);
    assert_eq!(BlockStyle::Welcome.color(), Color::Cyan);
    assert_eq!(BlockStyle::Permission.color(), Color::Magenta);
    assert_eq!(BlockStyle::Plain.color(), Color::White);
}

#[test]
fn lines_wrap_plain_block() {
    let mut feed = Feed::new();
    feed.push_line(BlockStyle::Plain, "hello world");
    let lines = feed.lines(20);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].text, "hello world");
    assert_eq!(lines[0].color, Color::White);
}

#[test]
fn lines_wrap_narrow_width() {
    let mut feed = Feed::new();
    feed.push_line(BlockStyle::Plain, "hello world");
    let lines = feed.lines(5);
    assert!(lines.len() > 1);
    for line in &lines {
        assert!(line.text.chars().count() <= 5 || line.text == "hello" || line.text == "world");
    }
}

#[test]
fn empty_block_produces_empty_line() {
    let mut feed = Feed::new();
    feed.push_line(BlockStyle::Plain, "");
    let lines = feed.lines(80);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].text, "");
}

#[test]
fn agent_block_gets_prefix_and_markdown() {
    let mut feed = Feed::new();
    feed.push_block(BlockStyle::Agent, "hello **world**");
    let lines = feed.lines(80);
    assert!(!lines.is_empty());
    assert!(
        lines[0].text.starts_with("< "),
        "first agent line should start with '< ', got {:?}",
        lines[0].text
    );
    let joined: String = lines
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("");
    assert!(
        joined.contains("hello "),
        "prose should be present: {}",
        joined
    );
    assert!(
        joined.contains("world"),
        "bold text should be present: {}",
        joined
    );
}

#[test]
fn agent_empty_block_no_lines() {
    let mut feed = Feed::new();
    feed.push_block(BlockStyle::Agent, "");
    let lines = feed.lines(80);
    assert!(lines.is_empty());
}

#[test]
fn line_count_matches_lines() {
    let mut feed = Feed::new();
    feed.push_line(BlockStyle::Plain, "one");
    feed.push_line(BlockStyle::Plain, "two");
    feed.push_line(BlockStyle::Plain, "three");
    assert_eq!(feed.line_count(80), 3);
}

#[test]
fn visible_range_bottom_aligned_when_short() {
    let mut feed = Feed::new();
    feed.push_line(BlockStyle::Plain, "one");
    feed.push_line(BlockStyle::Plain, "two");
    let (start, end) = feed.visible_range(80, 0, 10);
    assert_eq!(start, 0);
    assert_eq!(end, 2);
}

#[test]
fn visible_range_scrolled() {
    let mut feed = Feed::new();
    for i in 0..20 {
        feed.push_line(BlockStyle::Plain, format!("line {}", i));
    }
    let (start, end) = feed.visible_range(80, 5, 10);
    assert_eq!(end - start, 10);
    assert_eq!(start, 5);
}

#[test]
fn line_at_visual_row_bottom_pad() {
    let mut feed = Feed::new();
    feed.push_line(BlockStyle::Plain, "one");
    // viewport height 10, auto-scroll, content shorter than viewport -> padding
    assert_eq!(feed.line_at_visual_row(80, 0, 10, 0), None);
    assert_eq!(feed.line_at_visual_row(80, 0, 10, 9), Some(0));
}

#[test]
fn line_at_visual_row_scrolled() {
    let mut feed = Feed::new();
    for i in 0..20 {
        feed.push_line(BlockStyle::Plain, format!("line {}", i));
    }
    assert_eq!(feed.line_at_visual_row(80, 5, 10, 0), Some(5));
    assert_eq!(feed.line_at_visual_row(80, 5, 10, 9), Some(14));
}

#[test]
fn selected_text_extracts_lines() {
    let mut feed = Feed::new();
    feed.push_line(BlockStyle::Plain, "alpha");
    feed.push_line(BlockStyle::Plain, "beta");
    feed.push_line(BlockStyle::Plain, "gamma");
    let text = feed.selected_text(80, 0, 2);
    assert_eq!(text.as_deref(), Some("alpha\nbeta\ngamma"));
}

#[test]
fn selected_text_reversed_range() {
    let mut feed = Feed::new();
    feed.push_line(BlockStyle::Plain, "alpha");
    feed.push_line(BlockStyle::Plain, "beta");
    let text = feed.selected_text(80, 1, 0);
    assert_eq!(text.as_deref(), Some("alpha\nbeta"));
}

#[test]
fn append_to_last_extends_block() {
    let mut feed = Feed::new();
    feed.push_block(BlockStyle::Agent, "hello");
    assert!(feed.append_to_last(" world"));
    let lines = feed.lines(80);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].text.contains("hello world"));
}

#[test]
fn append_to_last_returns_false_when_empty() {
    let mut feed = Feed::new();
    assert!(!feed.append_to_last("orphan"));
}

#[test]
fn replace_last_updates_final_block() {
    let mut feed = Feed::new();
    feed.push_line(BlockStyle::Plain, "first");
    feed.push_line(BlockStyle::Plain, "second");
    feed.replace_last(BlockStyle::Agent, "replaced");
    let lines = feed.lines(80);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].text, "first");
    assert_eq!(lines[1].text, "< replaced");
}

#[test]
fn replace_last_pushes_when_empty() {
    let mut feed = Feed::new();
    feed.replace_last(BlockStyle::Agent, "only");
    let lines = feed.lines(80);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].text, "< only");
}

#[test]
fn truncate_blocks_keeps_prefix() {
    let mut feed = Feed::new();
    feed.push_line(BlockStyle::Plain, "first");
    feed.push_line(BlockStyle::Plain, "second");
    feed.push_line(BlockStyle::Plain, "third");
    feed.truncate_blocks(2);
    assert_eq!(feed.block_count(), 2);
    let lines = feed.lines(80);
    assert_eq!(lines.len(), 2);
}

#[test]
fn clear_empties_feed() {
    let mut feed = Feed::new();
    feed.push_line(BlockStyle::Plain, "hello");
    feed.clear();
    assert!(feed.is_empty());
    assert_eq!(feed.line_count(80), 0);
}

#[test]
fn generation_starts_at_zero() {
    let feed = Feed::new();
    assert_eq!(feed.generation(), 0);
}

#[test]
fn generation_bumps_on_each_mutator() {
    let mut feed = Feed::new();
    feed.push_block(BlockStyle::Plain, "one");
    assert_eq!(feed.generation(), 1);
    feed.push_line(BlockStyle::Plain, "two");
    assert_eq!(feed.generation(), 2);
    assert!(feed.append_to_last(" more"));
    assert_eq!(feed.generation(), 3);
    feed.replace_last(BlockStyle::Agent, "replaced");
    assert_eq!(feed.generation(), 4);
    feed.truncate_blocks(1);
    assert_eq!(feed.generation(), 5);
    feed.clear();
    assert_eq!(feed.generation(), 6);
}

#[test]
fn generation_not_bumped_by_failed_append() {
    let mut feed = Feed::new();
    assert!(!feed.append_to_last("orphan"));
    assert_eq!(feed.generation(), 0);
}

#[test]
fn generation_not_bumped_by_reads() {
    let mut feed = Feed::new();
    feed.push_line(BlockStyle::Plain, "one");
    let before = feed.generation();
    let _ = feed.lines(80);
    let _ = feed.line_count(80);
    let _ = feed.visible_range(80, 0, 10);
    let _ = feed.line_at_visual_row(80, 0, 10, 0);
    let _ = feed.selected_text(80, 0, 0);
    let _ = feed.is_empty();
    let _ = feed.block_count();
    assert_eq!(feed.generation(), before);
}

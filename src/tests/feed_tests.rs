use crate::ui::feed::{BlockStyle, Feed};
use crossterm::style::Color;

fn sample_time() -> chrono::DateTime<chrono::Local> {
    chrono::DateTime::from_timestamp(1_700_000_000, 0)
        .unwrap()
        .with_timezone(&chrono::Local)
}

#[test]
fn block_created_at_defaults_to_none() {
    let feed = Feed::new();
    assert!(feed.block_lines(0, 80).is_empty());
}

#[test]
fn push_line_with_time_carries_timestamp() {
    let mut feed = Feed::new();
    let ts = sample_time();
    feed.push_line_with_time(BlockStyle::User, "hello", ts);
    let lines = feed.lines(80);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].created_at, Some(ts));
    assert!(lines[0].block_start);
}

#[test]
fn push_streaming_block_with_time_carries_timestamp() {
    let mut feed = Feed::new();
    let ts = sample_time();
    feed.push_streaming_block_with_time(BlockStyle::Agent, ts);
    feed.append_to_last("hi");
    feed.finalize_last();
    let lines = feed.lines(80);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].created_at, Some(ts));
    assert!(lines[0].block_start);
}

#[test]
fn set_last_block_timestamp_stamps_only_when_empty() {
    let mut feed = Feed::new();
    feed.push_block(BlockStyle::Agent, "hi");
    assert!(feed.block_lines(0, 80)[0].created_at.is_none());
    feed.set_last_block_timestamp();
    let ts = feed.block_lines(0, 80)[0].created_at;
    assert!(ts.is_some());
    // Second call is a no-op: the timestamp stays the same.
    feed.set_last_block_timestamp();
    assert_eq!(feed.block_lines(0, 80)[0].created_at, ts);
}

#[test]
fn block_start_marked_on_first_wrapped_row_only() {
    let mut feed = Feed::new();
    feed.push_line(BlockStyle::Plain, "one two three four five six seven eight");
    let lines = feed.lines(10);
    assert!(lines.len() > 1);
    assert!(lines[0].block_start);
    assert!(!lines[1].block_start);
}

#[test]
fn agent_block_start_marked_after_prefix() {
    let mut feed = Feed::new();
    feed.push_block(BlockStyle::Agent, "hello");
    let lines = feed.lines(80);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].block_start);
    assert!(lines[0].text.starts_with("< "));
}

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
    assert_eq!(BlockStyle::Code.color(), Color::DarkYellow);
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
fn fenced_code_block_gets_header_and_code_style() {
    let mut feed = Feed::new();
    feed.push_block(
        BlockStyle::Agent,
        "```rust\nfn main() {\n    println!();\n}\n```",
    );
    let lines = feed.lines(80);
    assert!(
        lines.iter().any(|l| l.style == BlockStyle::Code),
        "code block lines should be BlockStyle::Code"
    );
    assert!(
        lines.iter().any(|l| l.text.contains("rust")),
        "language header should mention rust: {:?}",
        lines.iter().map(|l| &l.text).collect::<Vec<_>>()
    );
    assert!(
        lines.iter().any(|l| l.text.contains("fn main")),
        "code content should be present"
    );
    // Agent blocks that start with a code block should not get the '< ' prefix
    // on the code header.
    assert!(
        !lines[0].text.starts_with("< "),
        "code header should not get agent prefix: {:?}",
        lines[0].text
    );
}

#[test]
fn indented_code_block_uses_plain_code_header() {
    let mut feed = Feed::new();
    feed.push_block(BlockStyle::Agent, "    fn hi() {}\n    fn bye() {}");
    let lines = feed.lines(80);
    assert!(
        lines.iter().any(|l| l.style == BlockStyle::Code),
        "indented code block lines should be styled as code"
    );
    assert!(
        lines.iter().any(|l| l.text.contains("code")),
        "indented code block should have generic header"
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
fn block_lines_matches_flat_layout() {
    let mut feed = Feed::new();
    feed.push_line(BlockStyle::Plain, "one");
    feed.push_block(BlockStyle::Agent, "two **bold**");
    feed.push_line(BlockStyle::Plain, "three");

    let flat = feed.lines(80);
    let mut per_block = feed.block_lines(0, 80);
    per_block.extend(feed.block_lines(1, 80));
    per_block.extend(feed.block_lines(2, 80));
    assert_eq!(
        flat.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
        per_block
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
    );
    // Out-of-range blocks lay out to nothing.
    assert!(feed.block_lines(3, 80).is_empty());
}

#[test]
fn last_block_running_tracks_streaming_state() {
    let mut feed = Feed::new();
    feed.push_line(BlockStyle::Plain, "done");
    assert!(!feed.last_block_running());
    feed.push_streaming_block(BlockStyle::Agent);
    assert!(feed.last_block_running());
    feed.finalize_last();
    assert!(!feed.last_block_running());
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
    let _ = feed.block_lines(0, 80);
    let _ = feed.last_block_running();
    let _ = feed.is_empty();
    let _ = feed.block_count();
    assert_eq!(feed.generation(), before);
}

#[test]
fn running_agent_block_renders_tail_as_plain_text() {
    let mut feed = Feed::new();
    feed.push_streaming_block(BlockStyle::Agent);
    assert!(feed.append_to_last("hello **wor"));
    let lines = feed.lines(80);
    assert_eq!(lines.len(), 1);
    // No markdown parsing while the line is unfinished: markers stay literal.
    assert_eq!(lines[0].text, "< hello **wor");
    assert_eq!(lines[0].color, Color::White);
}

#[test]
fn running_agent_block_parses_only_completed_lines() {
    let mut feed = Feed::new();
    feed.push_streaming_block(BlockStyle::Agent);
    assert!(feed.append_to_last("first **bold**\nsecond **par"));
    let lines = feed.lines(80);
    assert_eq!(lines.len(), 2);
    // The completed line is parsed as markdown: bold markers are gone.
    assert_eq!(lines[0].text, "< first bold");
    // The unfinished tail line stays plain: markers remain literal.
    assert_eq!(lines[1].text, "second **par");
}

#[test]
fn running_agent_block_appends_grow_tail() {
    let mut feed = Feed::new();
    feed.push_streaming_block(BlockStyle::Agent);
    assert!(feed.append_to_last("hello"));
    assert!(feed.append_to_last(" world"));
    let lines = feed.lines(80);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].text, "< hello world");
}

#[test]
fn finalize_last_parses_full_text() {
    let mut feed = Feed::new();
    feed.push_streaming_block(BlockStyle::Agent);
    assert!(feed.append_to_last("hello **world**"));
    feed.finalize_last();
    let lines = feed.lines(80);
    assert_eq!(lines.len(), 1);
    // After finalizing, the former tail line is parsed as markdown.
    assert_eq!(lines[0].text, "< hello world");
}

#[test]
fn finalize_last_bumps_generation_once() {
    let mut feed = Feed::new();
    feed.push_streaming_block(BlockStyle::Agent);
    let before = feed.generation();
    feed.finalize_last();
    assert_eq!(feed.generation(), before + 1);
    // Second call is a no-op: the block is no longer running.
    feed.finalize_last();
    assert_eq!(feed.generation(), before + 1);
}

#[test]
fn finalize_last_on_complete_block_is_noop() {
    let mut feed = Feed::new();
    feed.push_block(BlockStyle::Agent, "done");
    let before = feed.generation();
    feed.finalize_last();
    assert_eq!(feed.generation(), before);
}

#[test]
fn replace_last_invalidates_cached_layout() {
    let mut feed = Feed::new();
    feed.push_block(BlockStyle::Agent, "aaaa **old**");
    let _ = feed.lines(80); // populate the layout cache
    // Same length, different content: the cached layout must not leak through.
    feed.replace_last(BlockStyle::Agent, "bbbb **new**");
    let lines = feed.lines(80);
    let joined: String = lines
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("");
    assert!(joined.contains("new"), "expected new content: {joined}");
    assert!(!joined.contains("old"), "stale cached content: {joined}");
}

#[test]
fn agent_layout_recomputes_on_width_change() {
    let mut feed = Feed::new();
    feed.push_block(
        BlockStyle::Agent,
        "one two three four five six seven eight nine ten eleven twelve",
    );
    let wide = feed.lines(120);
    let narrow = feed.lines(20);
    assert!(
        narrow.len() > wide.len(),
        "narrow width should wrap into more lines: {} vs {}",
        narrow.len(),
        wide.len()
    );
}

#[test]
fn layout_queries_reuse_prewrapped_rows() {
    let mut feed = Feed::new();
    feed.push_line(BlockStyle::Plain, "hello");
    let _ = feed.lines(80);
    let _ = feed.line_count(80);
    let _ = feed.block_lines(0, 80);
    assert_eq!(
        feed.layout_computes(),
        1,
        "layout queries should reuse the pre-wrapped rows"
    );

    feed.push_line(BlockStyle::Plain, "world");
    let _ = feed.lines(80);
    assert_eq!(feed.layout_computes(), 2, "mutation should invalidate");

    let _ = feed.lines(40);
    assert_eq!(feed.layout_computes(), 3, "resize should invalidate");

    // Alternating back to a previously seen width still re-lays out once
    // (single-slot cache), then reuses.
    let _ = feed.lines(80);
    let _ = feed.lines(80);
    assert_eq!(feed.layout_computes(), 4);
}

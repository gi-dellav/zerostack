use crate::ui::renderer::{LineColor, base64_encode, copy_to_clipboard};
use crate::ui::utils::UiColors;
use crossterm::style::Color;

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
    copy_to_clipboard("test text");
}

#[test]
fn copy_to_clipboard_empty_string() {
    copy_to_clipboard("");
}

#[test]
fn line_color_resolve_default_colors() {
    let colors = UiColors::default_colors();
    assert_eq!(LineColor::AgentText.resolve(&colors), Color::White);
    assert_eq!(LineColor::Error.resolve(&colors), Color::Red);
    assert_eq!(LineColor::ToolCall.resolve(&colors), Color::Yellow);
    assert_eq!(LineColor::Permission.resolve(&colors), Color::Magenta);
    assert_eq!(LineColor::ByTheWay.resolve(&colors), Color::Cyan);
    assert_eq!(LineColor::Reasoning.resolve(&colors), Color::DarkGrey);
    assert_eq!(LineColor::Secondary.resolve(&colors), Color::DarkGrey);
    assert_eq!(LineColor::Success.resolve(&colors), Color::Green);
    assert_eq!(LineColor::Heading.resolve(&colors), Color::Cyan);
    assert_eq!(LineColor::CodeBlock.resolve(&colors), Color::DarkYellow);
    assert_eq!(LineColor::LinkText.resolve(&colors), Color::DarkCyan);
    assert_eq!(LineColor::PromptMarker.resolve(&colors), Color::Green);
}

#[test]
fn line_color_resolve_custom_colors() {
    let mut colors = UiColors::default_colors();
    colors.agent_text = Color::Cyan;
    colors.error = Color::Magenta;
    assert_eq!(LineColor::AgentText.resolve(&colors), Color::Cyan);
    assert_eq!(LineColor::Error.resolve(&colors), Color::Magenta);
}

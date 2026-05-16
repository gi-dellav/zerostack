use compact_str::CompactString;
use crossterm::style::Color;
use pulldown_cmark::{Event, Tag, TagEnd};

use super::renderer::LineEntry;

fn word_wrap(text: &str, max_width: usize) -> Vec<CompactString> {
    if text.is_empty() {
        return vec![CompactString::new("")];
    }
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_width {
        return vec![CompactString::from(text)];
    }
    let mut lines = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let end = (start + max_width).min(chars.len());
        if end < chars.len() {
            let mut break_at = end;
            for i in (start..end).rev() {
                if chars[i] == ' ' {
                    break_at = i + 1;
                    break;
                }
            }
            if break_at == start {
                break_at = end;
            }
            lines.push(CompactString::from(chars[start..break_at].iter().collect::<String>()));
            start = break_at;
        } else {
            lines.push(CompactString::from(chars[start..].iter().collect::<String>()));
            break;
        }
    }
    lines
}

fn flush_acc(acc: &str, color: Color, max_width: usize, out: &mut Vec<LineEntry>) {
    if acc.is_empty() {
        return;
    }
    for line in acc.split('\n') {
        let trimmed = line.trim_end_matches('\r');
        if trimmed.is_empty() {
            out.push(LineEntry {
                text: CompactString::new(""),
                color,
            });
        } else {
            for chunk in word_wrap(trimmed, max_width) {
                out.push(LineEntry {
                    text: chunk,
                    color,
                });
            }
        }
    }
}

fn bullet_prefix(col: Color) -> &'static str {
    match col {
        Color::DarkGrey => "  ┊ ",
        _ => "  • ",
    }
}

pub fn markdown_to_styled(text: &str, max_width: usize) -> Vec<LineEntry> {
    if text.is_empty() {
        return Vec::new();
    }

    let parser = pulldown_cmark::Parser::new(text);
    let mut result = Vec::new();
    let mut acc = String::new();

    let mut in_heading = false;
    let mut in_code_block = false;
    let mut in_blockquote = false;
    let mut ordered_list = false;
    let mut list_item_count: u64 = 0;

    for event in parser {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {}
                Tag::Heading { level: _, .. } => {
                    flush_acc(&acc, Color::White, max_width, &mut result);
                    acc.clear();
                    in_heading = true;
                }
                Tag::CodeBlock(_kind) => {
                    flush_acc(&acc, Color::White, max_width, &mut result);
                    acc.clear();
                    in_code_block = true;
                }
                Tag::BlockQuote(_) => {
                    flush_acc(&acc, Color::White, max_width, &mut result);
                    acc.clear();
                    in_blockquote = true;
                }
                Tag::List(t) => {
                    ordered_list = t.is_some();
                    list_item_count = 0;
                }
                Tag::Item => {
                    flush_acc(&acc, Color::White, max_width, &mut result);
                    acc.clear();
                    list_item_count += 1;
                }
                Tag::FootnoteDefinition(_) => {}
                Tag::Table(_) => {}
                Tag::TableHead => {}
                Tag::TableRow => {}
                Tag::TableCell => {}
                _ => {}
            },
            Event::End(tag_end) => match tag_end {
                TagEnd::Paragraph => {
                    let color = if in_blockquote {
                        Color::DarkGrey
                    } else {
                        Color::White
                    };
                    flush_acc(&acc, color, max_width, &mut result);
                    acc.clear();
                }
                TagEnd::Heading(_) => {
                    flush_acc(&acc, Color::Cyan, max_width, &mut result);
                    acc.clear();
                    in_heading = false;
                    result.push(LineEntry {
                        text: CompactString::new(""),
                        color: Color::White,
                    });
                }
                TagEnd::CodeBlock => {
                    for line in acc.split('\n') {
                        let trimmed = line.trim_end_matches('\r');
                        if trimmed.is_empty() {
                            result.push(LineEntry {
                                text: CompactString::new(""),
                                color: Color::DarkYellow,
                            });
                        } else {
                            result.push(LineEntry {
                                text: CompactString::from(trimmed),
                                color: Color::DarkYellow,
                            });
                        }
                    }
                    acc.clear();
                    in_code_block = false;
                    result.push(LineEntry {
                        text: CompactString::new(""),
                        color: Color::White,
                    });
                }
                TagEnd::BlockQuote(_) => {
                    let mut quoted = Vec::new();
                    for line in acc.split('\n') {
                        let trimmed = line.trim_end_matches('\r');
                        if trimmed.is_empty() {
                            quoted.push(LineEntry {
                                text: CompactString::new(""),
                                color: Color::DarkGrey,
                            });
                        } else {
                            let prefixed = format!("│ {}", trimmed);
                            for chunk in word_wrap(&prefixed, max_width) {
                                quoted.push(LineEntry {
                                    text: chunk,
                                    color: Color::DarkGrey,
                                });
                            }
                        }
                    }
                    result.extend(quoted);
                    acc.clear();
                    in_blockquote = false;
                    result.push(LineEntry {
                        text: CompactString::new(""),
                        color: Color::White,
                    });
                }
                TagEnd::Item => {
                    let color = if in_blockquote {
                        Color::DarkGrey
                    } else {
                        Color::White
                    };
                    let bullet = if ordered_list {
                        format!(" {}. ", list_item_count)
                    } else {
                        format!("{}", bullet_prefix(color))
                    };
                    let mut item_lines = Vec::new();
                    let mut first = true;
                    for line in acc.split('\n') {
                        let trimmed = line.trim_end_matches('\r');
                        if trimmed.is_empty() {
                            item_lines.push(LineEntry {
                                text: CompactString::new(""),
                                color,
                            });
                        } else if first {
                            let prefixed = format!("{}{}", bullet, trimmed);
                            for chunk in word_wrap(&prefixed, max_width) {
                                item_lines.push(LineEntry {
                                    text: chunk,
                                    color,
                                });
                            }
                            first = false;
                        } else {
                            for chunk in word_wrap(trimmed, max_width) {
                                item_lines.push(LineEntry {
                                    text: chunk,
                                    color,
                                });
                            }
                        }
                    }
                    result.extend(item_lines);
                    acc.clear();
                }
                TagEnd::List(_) => {
                    ordered_list = false;
                    list_item_count = 0;
                    result.push(LineEntry {
                        text: CompactString::new(""),
                        color: Color::White,
                    });
                }
                TagEnd::FootnoteDefinition => {}
                TagEnd::Table => {}
                TagEnd::TableHead => {}
                TagEnd::TableRow => {}
                TagEnd::TableCell => {}
                _ => {}
            },
            Event::Text(t) => {
                if in_code_block {
                    acc.push_str(&t);
                } else {
                    acc.push_str(&t);
                }
            }
            Event::Code(t) => {
                if in_code_block {
                    acc.push_str(&t);
                } else {
                    acc.push_str(&t);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if in_code_block {
                    acc.push('\n');
                } else {
                    acc.push('\n');
                }
            }
            Event::Rule => {
                flush_acc(&acc, Color::White, max_width, &mut result);
                acc.clear();
                let rule: String = std::iter::repeat('─')
                    .take(max_width.min(40))
                    .collect();
                result.push(LineEntry {
                    text: CompactString::from(rule),
                    color: Color::DarkGrey,
                });
                result.push(LineEntry {
                    text: CompactString::new(""),
                    color: Color::White,
                });
            }
            Event::Html(t) => {
                acc.push_str(&t);
            }
            Event::InlineHtml(t) => {
                acc.push_str(&t);
            }
            Event::FootnoteReference(t) => {
                acc.push_str(&t);
            }
            Event::TaskListMarker(checked) => {
                if checked {
                    acc.push_str("[x]");
                } else {
                    acc.push_str("[ ]");
                }
            }
            _ => {}
        }
    }

    if !acc.is_empty() {
        let color = if in_blockquote {
            Color::DarkGrey
        } else if in_code_block {
            Color::DarkYellow
        } else if in_heading {
            Color::Cyan
        } else {
            Color::White
        };
        flush_acc(&acc, color, max_width, &mut result);
    }

    result
}

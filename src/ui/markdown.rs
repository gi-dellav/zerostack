use compact_str::CompactString;
use crossterm::style::Color;
use pulldown_cmark::{Alignment, Event, Options, Tag, TagEnd};
use smallvec::{SmallVec, smallvec};

use super::renderer::LineEntry;
use super::utils::display_width;

pub(crate) fn word_wrap(text: &str, max_width: usize) -> SmallVec<[CompactString; 4]> {
    if text.is_empty() || max_width == 0 {
        return smallvec![CompactString::from(text)];
    }
    if display_width(text) <= max_width {
        return smallvec![CompactString::from(text)];
    }

    let mut lines: SmallVec<[CompactString; 4]> = SmallVec::new();
    let mut line = String::new();
    let mut line_width: usize = 0;

    for word in text.split_inclusive(char::is_whitespace) {
        let word_trimmed = word.trim_end_matches(char::is_whitespace);
        let word_w = display_width(word);
        let trimmed_w = display_width(word_trimmed);

        if word_trimmed.is_empty() {
            if line_width > 0 && line_width < max_width {
                line.push(' ');
                line_width += 1;
            }
            continue;
        }

        if line_width + word_w <= max_width {
            line.push_str(word);
            line_width += word_w;
        } else if !line.is_empty() && line_width + 1 + trimmed_w <= max_width {
            line.push(' ');
            line.push_str(word_trimmed);
            line_width += 1 + trimmed_w;
            if word.ends_with(char::is_whitespace) {
                line.push(' ');
                line_width += 1;
            }
        } else {
            if !line.is_empty() {
                lines.push(CompactString::from(line.trim_end()));
            }
            line.clear();
            line_width = 0;

            if trimmed_w > max_width {
                for ch in word_trimmed.chars() {
                    let cw = super::utils::char_display_width(ch);
                    if line_width + cw > max_width && !line.is_empty() {
                        lines.push(CompactString::from(&line));
                        line.clear();
                        line_width = 0;
                    }
                    line.push(ch);
                    line_width += cw;
                }
            } else {
                line.push_str(word_trimmed);
                line_width += trimmed_w;
            }
            if word.ends_with(char::is_whitespace) {
                line.push(' ');
                line_width += 1;
            }
        }
    }

    let trimmed = line.trim_end();
    if !trimmed.is_empty() {
        lines.push(CompactString::from(trimmed));
    }

    if lines.is_empty() {
        lines.push(CompactString::from(text));
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
                out.push(LineEntry { text: chunk, color });
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

    let parser = pulldown_cmark::Parser::new_ext(
        text,
        Options::ENABLE_TABLES
            | Options::ENABLE_FOOTNOTES
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TASKLISTS,
    );
    let mut result = Vec::new();
    let mut acc = String::new();

    let mut in_heading = false;
    let mut in_code_block = false;
    let mut in_blockquote = false;
    let mut ordered_list = false;
    let mut list_item_count: u64 = 0;
    let mut table_rows: Vec<Vec<String>> = Vec::new();
    let mut table_row: Vec<String> = Vec::new();
    let mut table_cell = String::new();
    let mut table_alignments: Vec<Alignment> = Vec::new();
    let mut link_url = String::new();
    let mut in_table_cell = false;

    for event in parser {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {}
                Tag::Heading { .. } => {
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
                Tag::Table(alignments) => {
                    flush_acc(&acc, Color::White, max_width, &mut result);
                    acc.clear();
                    table_rows.clear();
                    table_row.clear();
                    table_cell.clear();
                    table_alignments = alignments;
                }
                Tag::TableHead => {
                    table_rows.clear();
                }
                Tag::TableRow => {
                    table_row.clear();
                }
                Tag::TableCell => {
                    table_cell.clear();
                    in_table_cell = true;
                }
                Tag::Link {
                    link_type: _,
                    dest_url,
                    title: _,
                    id: _,
                } => {
                    link_url = dest_url.to_string();
                }
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
                        bullet_prefix(color).to_string()
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
                                item_lines.push(LineEntry { text: chunk, color });
                            }
                            first = false;
                        } else {
                            for chunk in word_wrap(trimmed, max_width) {
                                item_lines.push(LineEntry { text: chunk, color });
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
                TagEnd::Link => {
                    if !link_url.is_empty() {
                        if in_table_cell {
                            table_cell.push_str(&format!(" ({})", link_url));
                        } else if !acc.is_empty() {
                            flush_acc(&acc, Color::DarkCyan, max_width, &mut result);
                            let note = format!("  ↪ {}", link_url);
                            for chunk in word_wrap(&note, max_width) {
                                result.push(LineEntry {
                                    text: chunk,
                                    color: Color::DarkGrey,
                                });
                            }
                            acc.clear();
                        }
                    }
                    link_url.clear();
                }
                TagEnd::Table => {
                    flush_table(&table_rows, &table_alignments, max_width, &mut result);
                    table_rows.clear();
                    table_alignments.clear();
                    result.push(LineEntry {
                        text: CompactString::new(""),
                        color: Color::White,
                    });
                }
                TagEnd::TableHead => {
                    let cells = std::mem::take(&mut table_row);
                    let cell_text: Vec<String> =
                        cells.into_iter().map(|c| c.trim().to_string()).collect();
                    if !cell_text.iter().all(|c| c.is_empty()) {
                        table_rows.push(cell_text);
                    }
                }
                TagEnd::TableRow => {
                    let cells = std::mem::take(&mut table_row);
                    let cell_text: Vec<String> =
                        cells.into_iter().map(|c| c.trim().to_string()).collect();
                    if !cell_text.iter().all(|c| c.is_empty()) {
                        table_rows.push(cell_text);
                    }
                }
                TagEnd::TableCell => {
                    in_table_cell = false;
                    table_row.push(std::mem::take(&mut table_cell));
                }
                TagEnd::FootnoteDefinition => {}
                _ => {}
            },
            Event::Text(t) => {
                if in_table_cell {
                    table_cell.push_str(&t);
                } else {
                    acc.push_str(&t);
                }
            }
            Event::Code(t) => {
                if in_table_cell {
                    table_cell.push_str(&format!("`{}`", t));
                } else {
                    acc.push_str(&format!("`{}`", t));
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if in_table_cell {
                    table_cell.push('\n');
                } else {
                    acc.push('\n');
                }
            }
            Event::Rule => {
                flush_acc(&acc, Color::White, max_width, &mut result);
                acc.clear();
                let rule: String = "\u{2500}".repeat(max_width.min(40));
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
                if in_table_cell {
                    table_cell.push_str(&t);
                } else {
                    acc.push_str(&t);
                }
            }
            Event::InlineHtml(t) => {
                if in_table_cell {
                    table_cell.push_str(&t);
                } else {
                    acc.push_str(&t);
                }
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

fn flush_table(
    rows: &[Vec<String>],
    alignments: &[Alignment],
    max_width: usize,
    out: &mut Vec<LineEntry>,
) {
    if rows.is_empty() {
        return;
    }

    let col_count = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if col_count == 0 {
        return;
    }

    let mut col_widths: Vec<usize> = vec![0; col_count];
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < col_count {
                col_widths[i] = col_widths[i].max(display_width(cell));
            }
        }
    }

    let overhead = 3 * col_count + 1;
    let available = max_width.saturating_sub(overhead);
    if available == 0 {
        return;
    }

    let mut total_req: usize = col_widths.iter().sum();
    const MIN_COL_WIDTH: usize = 4;
    while total_req > available {
        let mut widest_idx = 0;
        let mut widest_w = 0;
        for (i, w) in col_widths.iter().enumerate() {
            if *w > widest_w {
                widest_w = *w;
                widest_idx = i;
            }
        }
        if widest_w <= MIN_COL_WIDTH {
            break;
        }
        col_widths[widest_idx] -= 1;
        total_req -= 1;
    }

    let top = format_table_rule(&col_widths, '\u{250c}', '\u{252c}', '\u{2510}');
    let sep = format_table_rule(&col_widths, '\u{251c}', '\u{253c}', '\u{2524}');
    let bot = format_table_rule(&col_widths, '\u{2514}', '\u{2534}', '\u{2518}');

    push_table_line(&top, Color::DarkGrey, out);
    for (i, row) in rows.iter().enumerate() {
        for line in format_table_row(row, &col_widths, alignments) {
            push_table_line(&line, Color::White, out);
        }
        if i == 0 && rows.len() > 1 {
            push_table_line(&sep, Color::DarkGrey, out);
        }
    }
    push_table_line(&bot, Color::DarkGrey, out);
}

fn format_table_rule(widths: &[usize], left: char, mid: char, right: char) -> String {
    let mut s = String::with_capacity(widths.iter().sum::<usize>() + widths.len() * 3);
    s.push(left);
    for (i, w) in widths.iter().enumerate() {
        for _ in 0..*w + 2 {
            s.push('\u{2500}');
        }
        if i + 1 < widths.len() {
            s.push(mid);
        }
    }
    s.push(right);
    s
}

fn push_table_line(text: &str, color: Color, out: &mut Vec<LineEntry>) {
    out.push(LineEntry {
        text: CompactString::from(text),
        color,
    });
}

fn format_table_row(cells: &[String], widths: &[usize], alignments: &[Alignment]) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut cell_wrapped: Vec<Vec<String>> = Vec::new();
    let mut max_subrows = 0usize;

    for (i, cell) in cells.iter().enumerate() {
        let width = widths.get(i).copied().unwrap_or(10);
        let wrapped = if display_width(cell) <= width {
            vec![cell.clone()]
        } else {
            let mut chunks = Vec::new();
            for chunk in word_wrap(cell, width) {
                chunks.push(chunk.to_string());
            }
            chunks
        };
        max_subrows = max_subrows.max(wrapped.len());
        cell_wrapped.push(wrapped);
    }

    for subrow in 0..max_subrows {
        let mut line = String::new();
        line.push('\u{2502}');
        for (i, cw) in cell_wrapped.iter().enumerate() {
            let width = widths.get(i).copied().unwrap_or(10);
            let text = cw.get(subrow).map(|s| s.as_str()).unwrap_or("");
            let text_w = display_width(text);
            let align = alignments.get(i).copied().unwrap_or(Alignment::None);
            let padding = width.saturating_sub(text_w);
            line.push(' ');
            match align {
                Alignment::Center => {
                    let left_pad = padding / 2;
                    let right_pad = padding - left_pad;
                    for _ in 0..left_pad {
                        line.push(' ');
                    }
                    line.push_str(text);
                    for _ in 0..right_pad {
                        line.push(' ');
                    }
                }
                Alignment::Right => {
                    for _ in 0..padding {
                        line.push(' ');
                    }
                    line.push_str(text);
                }
                Alignment::None | Alignment::Left => {
                    line.push_str(text);
                    for _ in 0..padding {
                        line.push(' ');
                    }
                }
            }
            line.push(' ');
            if i + 1 < cell_wrapped.len() {
                line.push('\u{2502}');
            }
        }
        line.push('\u{2502}');
        lines.push(line);
    }

    lines
}

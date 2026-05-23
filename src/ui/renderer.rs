use std::io::{self, Write};

use compact_str::CompactString;
use crossterm::ExecutableCommand;
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::style::{
    Attribute, Color, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
};
use crossterm::terminal::{Clear, ClearType, ScrollUp};
use smallvec::{smallvec, SmallVec};

use super::markdown::word_wrap;
use super::resolve_color;

#[derive(Clone)]
pub struct LineEntry {
    pub text: CompactString,
    pub color: Color,
}

pub struct Renderer {
    lines: u16,
    col: u16,
    spinner_tick: bool,
    buffer: Vec<LineEntry>,
    partial: CompactString,
    partial_color: Color,
    scroll_offset: usize,
    input_scroll_offset: usize,
    monochrome: bool,
    chat_bg: Option<Color>,
    input_bg: Option<Color>,
    status_bg: Option<Color>,
    pub selection_active: bool,
    pub selection_start: Option<usize>,
    pub selection_end: Option<usize>,
}

impl Renderer {
    pub fn new() -> io::Result<Self> {
        Ok(Renderer {
            lines: 0,
            col: 0,
            spinner_tick: false,
            buffer: Vec::new(),
            partial: CompactString::new(""),
            partial_color: Color::White,
            scroll_offset: 0,
            input_scroll_offset: 0,
            monochrome: false,
            chat_bg: None,
            input_bg: None,
            status_bg: None,
            selection_active: false,
            selection_start: None,
            selection_end: None,
        })
    }

    pub fn set_monochrome(&mut self, monochrome: bool) {
        self.monochrome = monochrome;
    }

    pub fn set_background_colors(
        &mut self,
        chat_bg: Option<Color>,
        input_bg: Option<Color>,
        status_bg: Option<Color>,
    ) {
        self.chat_bg = chat_bg;
        self.input_bg = input_bg;
        self.status_bg = status_bg;
    }

    fn color(&self, color: Color) -> Color {
        resolve_color(color, self.monochrome)
    }

    fn terminal_size(&self) -> (u16, u16) {
        crossterm::terminal::size().unwrap_or((80, 24))
    }

    fn max_line_width(&self) -> usize {
        let (cols, _) = self.terminal_size();
        cols.saturating_sub(1) as usize
    }

    pub fn line_width(&self) -> usize {
        self.max_line_width()
    }

    pub fn buffer_len(&self) -> usize {
        self.buffer.len()
    }

    pub fn replace_from(&mut self, start: usize, lines: Vec<LineEntry>) {
        self.commit_partial();
        self.buffer.truncate(start);
        self.buffer.extend(lines);
        self.lines = self.buffer.len() as u16;
        self.col = 0;
        self.partial.clear();
        let visible = self.visible_lines();
        let max_offset = self.buffer.len().saturating_sub(visible);
        if self.scroll_offset > max_offset {
            self.scroll_offset = max_offset;
        }
    }

    pub fn visible_lines(&self) -> usize {
        let (_, rows) = self.terminal_size();
        rows.saturating_sub(2) as usize
    }

    pub fn buffer_line_at_row(&self, row: u16) -> Option<usize> {
        let (cols, rows) = self.terminal_size();
        let max_width = cols.saturating_sub(1) as usize;
        let visible = rows.saturating_sub(2) as usize;
        let total = self.buffer.len();
        if total == 0 {
            return None;
        }
        let start = if self.scroll_offset == 0 {
            total.saturating_sub(visible)
        } else {
            total.saturating_sub(self.scroll_offset + visible)
        };
        let start = start.min(total.saturating_sub(visible));

        let mut visual_row: u16 = 0;
        let mut buf_idx = start;

        while buf_idx < total {
            let entry = &self.buffer[buf_idx];
            let text = &entry.text;

            let wrapped_rows = if text.chars().count() > max_width {
                word_wrap(text, max_width).len() as u16
            } else {
                1
            };

            if visual_row + wrapped_rows > row {
                return Some(buf_idx);
            }

            visual_row += wrapped_rows;
            buf_idx += 1;
        }

        None
    }

    pub fn clear_selection(&mut self) {
        self.selection_active = false;
        self.selection_start = None;
        self.selection_end = None;
    }

    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = match (self.selection_start, self.selection_end) {
            (Some(s), Some(e)) if s <= e => (s, e),
            (Some(s), Some(e)) => (e, s),
            _ => return None,
        };
        let mut result = String::new();
        for i in start..=end {
            if let Some(entry) = self.buffer.get(i) {
                if !result.is_empty() {
                    result.push('\n');
                }
                result.push_str(&entry.text);
            }
        }
        if result.is_empty() {
            None
        } else {
            Some(result)
        }
    }

    fn wrap_line(&self, line: &str, max_width: usize) -> SmallVec<[CompactString; 4]> {
        word_wrap(line, max_width)
    }

    fn commit_partial(&mut self) {
        if !self.partial.is_empty() {
            let max_width = self.max_line_width();
            let c = self.partial_color;
            for chunk in self.wrap_line(&self.partial, max_width) {
                self.buffer.push(LineEntry {
                    text: chunk,
                    color: c,
                });
            }
            self.partial.clear();
        }
    }

    pub fn is_scrolling(&self) -> bool {
        self.scroll_offset > 0
    }

    pub fn scroll_line_up(&mut self) {
        let visible = self.visible_lines();
        let max_offset = self.buffer.len().saturating_sub(visible);
        if self.scroll_offset < max_offset {
            self.scroll_offset += 1;
        }
    }

    pub fn scroll_line_down(&mut self) {
        if self.scroll_offset > 0 {
            self.scroll_offset -= 1;
        }
    }

    pub fn scroll_page_up(&mut self) {
        let visible = self.visible_lines();
        let page = visible.saturating_sub(2).max(1);
        let max_offset = self.buffer.len().saturating_sub(visible);
        self.scroll_offset = (self.scroll_offset + page).min(max_offset);
    }

    pub fn scroll_page_down(&mut self) {
        let visible = self.visible_lines();
        let page = visible.saturating_sub(2).max(1);
        if self.scroll_offset <= page {
            self.scroll_offset = 0;
        } else {
            self.scroll_offset = self.scroll_offset.saturating_sub(page);
        }
    }

    pub fn scroll_to_top(&mut self) {
        let visible = self.visible_lines();
        self.scroll_offset = self.buffer.len().saturating_sub(visible);
    }

    pub fn scroll_to_bottom(&mut self) -> io::Result<()> {
        self.scroll_offset = 0;
        self.sync_to_buffer()
    }

    fn sync_to_buffer(&mut self) -> io::Result<()> {
        self.commit_partial();
        self.col = 0;
        self.lines = self.buffer.len() as u16;
        self.render_viewport()
    }

    pub fn render_viewport(&mut self) -> io::Result<()> {
        let (cols, rows) = self.terminal_size();
        let max_width = cols.saturating_sub(1) as usize;
        let visible = rows.saturating_sub(2) as usize;
        let total = self.buffer.len();
        let mut stdout = io::stdout();
        write!(stdout, "{}", Hide)?;

        let start = if self.scroll_offset == 0 {
            total.saturating_sub(visible)
        } else {
            total.saturating_sub(self.scroll_offset + visible)
        };
        let start = start.min(total.saturating_sub(visible));

        let mut visual_row: u16 = 0;
        let mut buf_idx = start;

        while (visual_row as usize) < visible && buf_idx < total {
            let entry = &self.buffer[buf_idx];
            let text = &entry.text;

            let wrapped = if text.chars().count() > max_width {
                word_wrap(text, max_width)
            } else {
                smallvec![text.clone()]
            };

            for chunk in &wrapped {
                if (visual_row as usize) >= visible {
                    break;
                }

                stdout.execute(MoveTo(0, visual_row))?;

                let is_selected = self.selection_active
                    && self.selection_start.is_some()
                    && self.selection_end.is_some()
                    && {
                        let s = self.selection_start.unwrap();
                        let e = self.selection_end.unwrap();
                        let lo = s.min(e);
                        let hi = s.max(e);
                        buf_idx >= lo && buf_idx <= hi
                    };

                if let Some(bg) = self.chat_bg {
                    write!(stdout, "{}", SetBackgroundColor(self.color(bg)))?;
                }
                if is_selected {
                    write!(stdout, "{}", SetAttribute(Attribute::Reverse))?;
                }
                write!(stdout, "{}", SetForegroundColor(self.color(entry.color)))?;
                write!(stdout, "{}", chunk)?;
                if is_selected {
                    write!(stdout, "{}", SetAttribute(Attribute::NoReverse))?;
                }
                write!(stdout, "{}", Clear(ClearType::UntilNewLine))?;
                write!(stdout, "{}", ResetColor)?;

                visual_row += 1;
            }

            buf_idx += 1;
        }

        while (visual_row as usize) < visible {
            stdout.execute(MoveTo(0, visual_row))?;
            if let Some(bg) = self.chat_bg {
                write!(stdout, "{}", SetBackgroundColor(self.color(bg)))?;
            }
            write!(stdout, "{}", Clear(ClearType::UntilNewLine))?;
            write!(stdout, "{}", ResetColor)?;
            visual_row += 1;
        }

        if self.scroll_offset > 0 {
            let pct = if total > visible {
                ((total - self.scroll_offset - visible) * 100 / (total - visible)).min(100)
            } else {
                0
            };
            let indicator = format!(" SCROLL {}% ", pct);
            let x = cols.saturating_sub(indicator.len() as u16);
            stdout.execute(MoveTo(x, 0))?;
            if let Some(bg) = self.chat_bg {
                write!(stdout, "{}", SetBackgroundColor(self.color(bg)))?;
            }
            write!(
                stdout,
                "{}",
                SetForegroundColor(self.color(Color::DarkYellow))
            )?;
            write!(stdout, "{}", indicator)?;
            write!(stdout, "{}", ResetColor)?;
        }

        stdout.flush()?;
        Ok(())
    }

    fn ensure_room(&mut self) {
        if self.scroll_offset > 0 {
            return;
        }
        let (cols, rows) = self.terminal_size();
        if rows < 3 {
            return;
        }
        let max_content = rows.saturating_sub(2);
        if self.lines >= max_content {
            let mut stdout = io::stdout();
            let _ = stdout.execute(ScrollUp(1));
            self.lines = self.lines.saturating_sub(1);
            for &r in &[max_content.saturating_sub(1), max_content] {
                let _ = stdout.execute(MoveTo(0, r));
                if let Some(bg) = self.chat_bg {
                    let _ = write!(stdout, "{}", SetBackgroundColor(self.color(bg)));
                }
                let _ = write!(stdout, "{}", " ".repeat(cols as usize));
                let _ = write!(stdout, "{}", ResetColor);
            }
            let _ = stdout.flush();
        }
    }

    fn content_row(&self) -> u16 {
        let (_, rows) = self.terminal_size();
        self.lines.min(rows.saturating_sub(3))
    }

    pub fn resize(&mut self) {
        let visible = self.visible_lines();
        let max_offset = self.buffer.len().saturating_sub(visible);
        if self.scroll_offset > max_offset {
            self.scroll_offset = max_offset;
        }
    }

    pub fn write_line(&mut self, text: &str, color: Color) -> io::Result<()> {
        self.commit_partial();
        let max_width = self.max_line_width();
        for segment in text.split('\n') {
            let wrapped = self.wrap_line(segment, max_width);
            for chunk in &wrapped {
                self.buffer.push(LineEntry {
                    text: chunk.clone(),
                    color,
                });
                if self.scroll_offset == 0 {
                    self.ensure_room();
                    let mut stdout = io::stdout();
                    let r = self.content_row();
                    stdout.execute(MoveTo(0, r))?;
                    if let Some(bg) = self.chat_bg {
                        write!(stdout, "{}", SetBackgroundColor(self.color(bg)))?;
                    }
                    write!(stdout, "{}", Clear(ClearType::CurrentLine))?;
                    write!(stdout, "{}", SetForegroundColor(self.color(color)))?;
                    writeln!(stdout, "{}", chunk)?;
                    write!(stdout, "{}", ResetColor)?;
                    self.lines = self.lines.saturating_add(1);
                    self.col = 0;
                }
            }
        }
        if self.scroll_offset == 0 {
            io::stdout().flush()?;
        }
        Ok(())
    }

    pub fn write(&mut self, text: &str, color: Color) -> io::Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        let max_width = self.max_line_width();
        if max_width == 0 {
            return Ok(());
        }
        let parts: SmallVec<[&str; 4]> = text.split('\n').collect();
        let last = parts.len() - 1;
        for (i, segment) in parts.iter().enumerate() {
            if i < last {
                let len_before = self.buffer.len();
                self.commit_partial();
                let had_content = len_before < self.buffer.len();
                if !segment.is_empty() {
                    self.partial_color = color;
                    self.partial.push_str(segment);
                    self.commit_partial();
                } else if !had_content {
                    self.buffer.push(LineEntry {
                        text: CompactString::new(""),
                        color,
                    });
                }
                if self.scroll_offset == 0 {
                    self.ensure_room();
                    let mut stdout = io::stdout();
                    let r = self.content_row();
                    stdout.execute(MoveTo(self.col, r))?;
                    if let Some(bg) = self.chat_bg {
                        write!(stdout, "{}", SetBackgroundColor(self.color(bg)))?;
                    }
                    if !segment.is_empty() {
                        write!(stdout, "{}", SetForegroundColor(self.color(color)))?;
                        write!(stdout, "{}", segment)?;
                        write!(stdout, "{}", ResetColor)?;
                    }
                    writeln!(stdout)?;
                    self.lines = self.lines.saturating_add(1);
                    self.col = 0;
                }
            } else if !segment.is_empty() {
                let chars: SmallVec<[char; 64]> = segment.chars().collect();
                let mut idx = 0;
                while idx < chars.len() {
                    let avail = max_width.saturating_sub(self.col as usize);
                    if avail == 0 {
                        self.commit_partial();
                        if self.scroll_offset == 0 {
                            self.lines = self.lines.saturating_add(1);
                            self.col = 0;
                        }
                        continue;
                    }
                    let mut end = (idx + avail).min(chars.len());
                    if end < chars.len() {
                        let mut break_at = end;
                        for i in (idx..end).rev() {
                            if chars[i] == ' ' {
                                break_at = i + 1;
                                break;
                            }
                        }
                        if break_at != idx {
                            end = break_at;
                        }
                    }
                    let chunk: String = chars[idx..end].iter().collect();
                    self.partial_color = color;
                    self.partial.push_str(&chunk);
                    if self.scroll_offset == 0 {
                        self.ensure_room();
                        let mut stdout = io::stdout();
                        let r = self.content_row();
                        stdout.execute(MoveTo(self.col, r))?;
                        if let Some(bg) = self.chat_bg {
                            write!(stdout, "{}", SetBackgroundColor(self.color(bg)))?;
                        }
                        write!(stdout, "{}", SetForegroundColor(self.color(color)))?;
                        write!(stdout, "{}", chunk)?;
                        write!(stdout, "{}", ResetColor)?;

                        self.col = self.col.saturating_add(chunk.chars().count() as u16);
                    }
                    idx = end;
                    if idx < chars.len() {
                        self.commit_partial();
                        if self.scroll_offset == 0 {
                            self.lines = self.lines.saturating_add(1);
                            self.col = 0;
                        }
                    }
                }
            }
        }
        if self.scroll_offset == 0 {
            io::stdout().flush()?;
        }
        Ok(())
    }

    pub fn clear_content(&mut self) -> io::Result<()> {
        self.buffer.clear();
        self.partial.clear();
        self.scroll_offset = 0;
        self.clear_selection();
        let mut stdout = io::stdout();
        if let Some(bg) = self.chat_bg {
            write!(stdout, "{}", SetBackgroundColor(self.color(bg)))?;
        }
        stdout.execute(Clear(ClearType::All))?;
        write!(stdout, "{}", ResetColor)?;
        stdout.execute(MoveTo(0, 0))?;
        stdout.flush()?;
        self.lines = 0;
        self.col = 0;
        Ok(())
    }

    pub fn draw_bottom(
        &mut self,
        input_line: &str,
        cursor_pos: usize,
        status: &str,
        is_running: bool,
    ) -> io::Result<()> {
        let (cols, rows) = crossterm::terminal::size()?;
        let mut stdout = io::stdout();

        let status_row = rows.saturating_sub(1);

        let lines: SmallVec<[&str; 4]> = input_line.split('\n').collect();
        let line_count = lines.len();

        let last_line = rows.saturating_sub(2) as usize - 1;
        let available_rows = last_line + 1;
        let need_scroll = line_count > available_rows;
        let first_visible = if need_scroll {
            line_count - available_rows
        } else {
            0
        };

        let prompt = if is_running {
            self.spinner_tick = !self.spinner_tick;
            if self.spinner_tick { ". " } else { ": " }
        } else {
            "> "
        };
        let prompt_width = prompt.chars().count();

        let (cursor_line, cursor_col) =
            crate::ui::input::cursor_to_line_col(input_line, cursor_pos);

        let visible_width = cols.saturating_sub(prompt_width as u16) as usize;
        let cursor_line_text = lines.get(cursor_line).unwrap_or(&"");
        let cursor_line_len = cursor_line_text.chars().count();
        let mut h_scroll = 0usize;
        if cursor_line_len > visible_width {
            if cursor_col < self.input_scroll_offset {
                self.input_scroll_offset = cursor_col;
            } else if cursor_col >= self.input_scroll_offset + visible_width {
                self.input_scroll_offset = cursor_col - visible_width + 1;
            }
            let max_h_scroll = cursor_line_len.saturating_sub(visible_width);
            h_scroll = self.input_scroll_offset.min(max_h_scroll);
        } else {
            self.input_scroll_offset = 0;
        }

        // Clear and draw input area
        let visible_line_count = if need_scroll {
            available_rows
        } else {
            line_count
        };
        for (i, line) in lines
            .iter()
            .enumerate()
            .take(line_count)
            .skip(first_visible)
        {
            let render_row = (rows.saturating_sub(2) - visible_line_count as u16 + 1)
                + (i - first_visible) as u16;
            stdout.execute(MoveTo(0, render_row))?;

            if let Some(bg) = self.input_bg {
                write!(stdout, "{}", SetBackgroundColor(self.color(bg)))?;
            }

            if i == first_visible {
                write!(stdout, "{}", SetForegroundColor(self.color(Color::Cyan)))?;
                write!(stdout, "{}", prompt)?;
                write!(stdout, "{}", SetForegroundColor(Color::Reset))?;
            } else {
                write!(stdout, "{}", " ".repeat(prompt_width))?;
            }

            let line_chars: SmallVec<[char; 64]> = line.chars().collect();
            let h_offset = if i == cursor_line { h_scroll } else { 0 };
            let display: String = line_chars
                .iter()
                .skip(h_offset)
                .take(visible_width)
                .collect();
            write!(stdout, "{}", display)?;
            write!(stdout, "{}", Clear(ClearType::UntilNewLine))?;
            write!(stdout, "{}", ResetColor)?;
        }

        // Status line
        stdout.execute(MoveTo(0, status_row))?;
        if let Some(bg) = self.status_bg {
            write!(stdout, "{}", SetBackgroundColor(self.color(bg)))?;
        }
        write!(stdout, "{}", Clear(ClearType::CurrentLine))?;
        stdout.execute(MoveTo(0, status_row))?;
        if let Some(bg) = self.status_bg {
            write!(stdout, "{}", SetBackgroundColor(self.color(bg)))?;
        }
        write!(
            stdout,
            "{}",
            SetForegroundColor(self.color(Color::DarkGrey))
        )?;
        let status_display = if self.scroll_offset > 0 {
            format!("-- SCROLL -- {}", status)
        } else {
            status.to_string()
        };
        let truncated: String = status_display.chars().take(cols as usize).collect();
        write!(stdout, "{}", truncated)?;
        write!(stdout, "{}", Clear(ClearType::UntilNewLine))?;
        write!(stdout, "{}", ResetColor)?;

        // Cursor
        let cursor_render_idx = cursor_line.saturating_sub(first_visible);
        let cursor_row =
            (rows.saturating_sub(2) - visible_line_count as u16 + 1) + cursor_render_idx as u16;
        let cursor_x = (prompt_width + cursor_col.saturating_sub(h_scroll)) as u16;
        stdout.execute(MoveTo(cursor_x, cursor_row))?;
        write!(stdout, "{}", Show)?;
        stdout.flush()?;
        Ok(())
    }
}

pub fn copy_to_clipboard(text: &str) {
    let cmds: &[(&str, &[&str])] = &[
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("pbcopy", &[]),
        ("clip.exe", &[]),
    ];
    for &(cmd, args) in cmds {
        if let Ok(mut child) = std::process::Command::new(cmd)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
                let _ = stdin.flush();
            }
            let _ = child.wait();
            return;
        }
    }

    // OSC 52 escape sequence — clipboard access via terminal emulator.
    // Supported by Kitty, Alacritty, WezTerm, foot, iTerm2, Windows Terminal,
    // and most other modern terminals. No external tools needed.
    let encoded = base64_encode(text.as_bytes());
    let mut stdout = std::io::stdout().lock();
    let _ = write!(stdout, "\x1b]52;c;{encoded}\x07");
    let _ = stdout.flush();
}

/// Minimal base64 encoder — avoids pulling in a crate just for clipboard support.
fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = chunk.get(1).copied().unwrap_or(0) as usize;
        let b2 = chunk.get(2).copied().unwrap_or(0) as usize;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(triple >> 18) & 63] as char);
        out.push(ALPHABET[(triple >> 12) & 63] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(triple >> 6) & 63] } else { b'=' } as char);
        out.push(if chunk.len() > 2 { ALPHABET[triple & 63] } else { b'=' } as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

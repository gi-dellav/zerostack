use std::io::{self, Write};

use compact_str::CompactString;
use crossterm::ExecutableCommand;
use crossterm::cursor::MoveTo;
use crossterm::style::{Attribute, Color, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::terminal::{Clear, ClearType, ScrollUp};

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
            selection_active: false,
            selection_start: None,
            selection_end: None,
        })
    }

    pub fn set_monochrome(&mut self, monochrome: bool) {
        self.monochrome = monochrome;
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
        let (_, rows) = self.terminal_size();
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
        let idx = start + row as usize;
        if idx < total { Some(idx) } else { None }
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

    fn wrap_line(&self, line: &str, max_width: usize) -> Vec<CompactString> {
        let chars: Vec<char> = line.chars().collect();
        if chars.len() <= max_width {
            return vec![CompactString::new(line)];
        }
        chars
            .chunks(max_width)
            .map(|c| CompactString::new(c.iter().collect::<String>()))
            .collect()
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
        let visible = rows.saturating_sub(2) as usize;
        let total = self.buffer.len();
        let mut stdout = io::stdout();

        let start = if self.scroll_offset == 0 {
            total.saturating_sub(visible)
        } else {
            total.saturating_sub(self.scroll_offset + visible)
        };
        let start = start.min(total.saturating_sub(visible));
        let end = (start + visible).min(total);

        for i in 0..visible {
            stdout.execute(MoveTo(0, i as u16))?;
            if start + i < end {
                let entry = &self.buffer[start + i];
                let line_idx = start + i;
                let text: String = entry
                    .text
                    .chars()
                    .take(cols.saturating_sub(1) as usize)
                    .collect();

                let is_selected = self.selection_active
                    && self.selection_start.is_some()
                    && self.selection_end.is_some()
                    && {
                        let s = self.selection_start.unwrap();
                        let e = self.selection_end.unwrap();
                        let lo = s.min(e);
                        let hi = s.max(e);
                        line_idx >= lo && line_idx <= hi
                    };

                if is_selected {
                    write!(stdout, "{}", SetAttribute(Attribute::Reverse))?;
                }
                write!(stdout, "{}", SetForegroundColor(self.color(entry.color)))?;
                write!(stdout, "{}", text)?;
                if is_selected {
                    write!(stdout, "{}", SetAttribute(Attribute::NoReverse))?;
                }
                write!(stdout, "{}", ResetColor)?;
            }
            write!(stdout, "{}", Clear(ClearType::UntilNewLine))?;
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
            write!(stdout, "{}", SetForegroundColor(self.color(Color::DarkYellow)))?;
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
                let _ = write!(stdout, "{}", " ".repeat(cols as usize));
            }
            let _ = stdout.flush();
        }
    }

    fn content_row(&self) -> u16 {
        let (_, rows) = self.terminal_size();
        self.lines.min(rows.saturating_sub(3))
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
                    stdout.execute(Clear(ClearType::CurrentLine))?;
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
        let parts: Vec<&str> = text.split('\n').collect();
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
                let chars: Vec<char> = segment.chars().collect();
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
                    let end = (idx + avail).min(chars.len());
                    let chunk: String = chars[idx..end].iter().collect();
                    self.partial_color = color;
                    self.partial.push_str(&chunk);
                    if self.scroll_offset == 0 {
                        self.ensure_room();
                        let mut stdout = io::stdout();
                        let r = self.content_row();
                        stdout.execute(MoveTo(self.col, r))?;
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
        stdout.execute(Clear(ClearType::All))?;
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

        let input_row = rows.saturating_sub(2);
        let status_row = rows.saturating_sub(1);
        let prompt = if is_running {
            self.spinner_tick = !self.spinner_tick;
            if self.spinner_tick { ". " } else { ": " }
        } else {
            "> "
        };

        stdout.execute(MoveTo(0, input_row))?;
        write!(stdout, "{}", " ".repeat(cols as usize))?;
        stdout.execute(MoveTo(0, input_row))?;
        write!(stdout, "{}", SetForegroundColor(self.color(Color::Cyan)))?;
        write!(stdout, "{}", prompt)?;
        write!(stdout, "{}", ResetColor)?;
        let visible_width = cols.saturating_sub(2) as usize;
        let input_len = input_line.len();

        if cursor_pos < self.input_scroll_offset {
            self.input_scroll_offset = cursor_pos;
        } else if cursor_pos >= self.input_scroll_offset + visible_width {
            self.input_scroll_offset = cursor_pos - visible_width + 1;
        }
        let max_scroll = input_len.saturating_sub(visible_width);
        self.input_scroll_offset = self.input_scroll_offset.min(max_scroll);

        let visible: String = input_line
            .chars()
            .skip(self.input_scroll_offset)
            .take(visible_width)
            .collect();
        write!(stdout, "{}", visible)?;

        stdout.execute(MoveTo(0, status_row))?;
        write!(stdout, "{}", " ".repeat(cols as usize))?;
        stdout.execute(MoveTo(0, status_row))?;
        write!(stdout, "{}", SetForegroundColor(self.color(Color::DarkGrey)))?;
        let status_display = if self.scroll_offset > 0 {
            format!("-- SCROLL -- {}", status)
        } else {
            status.to_string()
        };
        let truncated: String = status_display.chars().take(cols as usize).collect();
        write!(stdout, "{}", truncated)?;
        write!(stdout, "{}", ResetColor)?;

        let cursor_x = (2 + cursor_pos.saturating_sub(self.input_scroll_offset)) as u16;
        stdout.execute(MoveTo(cursor_x, input_row))?;
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
}

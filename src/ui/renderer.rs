use std::io::{self, Write};

use compact_str::CompactString;
use crossterm::cursor::MoveTo;
use crossterm::style::{Color, ResetColor, SetForegroundColor};
use crossterm::terminal::{Clear, ClearType, ScrollUp};
use crossterm::ExecutableCommand;

#[derive(Clone)]
struct LineEntry {
    text: CompactString,
    color: Color,
}

pub struct Renderer {
    lines: u16,
    col: u16,
    spinner_tick: bool,
    buffer: Vec<LineEntry>,
    partial: CompactString,
    partial_color: Color,
    scroll_offset: usize,
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
        })
    }

    fn terminal_size(&self) -> (u16, u16) {
        crossterm::terminal::size().unwrap_or((80, 24))
    }

    fn max_line_width(&self) -> usize {
        let (cols, _) = self.terminal_size();
        cols.saturating_sub(1) as usize
    }

    fn visible_lines(&self) -> usize {
        let (_, rows) = self.terminal_size();
        rows.saturating_sub(2) as usize
    }

    fn wrap_line(&self, line: &str, max_width: usize) -> Vec<CompactString> {
        let chars: Vec<char> = line.chars().collect();
        if chars.len() <= max_width {
            return vec![CompactString::new(line)];
        }
        chars.chunks(max_width).map(|c| CompactString::new(c.iter().collect::<String>())).collect()
    }

    fn commit_partial(&mut self) {
        if !self.partial.is_empty() {
            let max_width = self.max_line_width();
            let c = self.partial_color;
            for chunk in self.wrap_line(&self.partial, max_width) {
                self.buffer.push(LineEntry { text: chunk, color: c });
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
                write!(stdout, "{}", SetForegroundColor(entry.color))?;
                let truncated: String = entry
                    .text
                    .chars()
                    .take(cols.saturating_sub(1) as usize)
                    .collect();
                write!(stdout, "{}", truncated)?;
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
            write!(stdout, "{}", SetForegroundColor(Color::DarkYellow))?;
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
                self.buffer.push(LineEntry { text: chunk.clone(), color });
                if self.scroll_offset == 0 {
                    self.ensure_room();
                    let mut stdout = io::stdout();
                    let r = self.content_row();
                    stdout.execute(MoveTo(0, r))?;
                    stdout.execute(Clear(ClearType::CurrentLine))?;
                    write!(stdout, "{}", SetForegroundColor(color))?;
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
                    self.buffer.push(LineEntry { text: CompactString::new(""), color });
                }
                if self.scroll_offset == 0 {
                    self.ensure_room();
                    let mut stdout = io::stdout();
                    let r = self.content_row();
                    stdout.execute(MoveTo(self.col, r))?;
                    if !segment.is_empty() {
                        write!(stdout, "{}", SetForegroundColor(color))?;
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
                        write!(stdout, "{}", SetForegroundColor(color))?;
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
            if self.spinner_tick {
                ". "
            } else {
                ": "
            }
        } else {
            "> "
        };

        stdout.execute(MoveTo(0, input_row))?;
        write!(stdout, "{}", " ".repeat(cols as usize))?;
        stdout.execute(MoveTo(0, input_row))?;
        write!(stdout, "{}", SetForegroundColor(Color::Cyan))?;
        write!(stdout, "{}", prompt)?;
        write!(stdout, "{}", ResetColor)?;
        let truncated: String = input_line
            .chars()
            .take(cols.saturating_sub(2) as usize)
            .collect();
        write!(stdout, "{}", truncated)?;

        stdout.execute(MoveTo(0, status_row))?;
        write!(stdout, "{}", " ".repeat(cols as usize))?;
        stdout.execute(MoveTo(0, status_row))?;
        write!(stdout, "{}", SetForegroundColor(Color::DarkGrey))?;
        let status_display = if self.scroll_offset > 0 {
            format!("-- SCROLL -- {}", status)
        } else {
            status.to_string()
        };
        let truncated: String = status_display.chars().take(cols as usize).collect();
        write!(stdout, "{}", truncated)?;
        write!(stdout, "{}", ResetColor)?;

        let cursor_x = (2 + cursor_pos.min(input_line.len())) as u16;
        stdout.execute(MoveTo(cursor_x, input_row))?;
        stdout.flush()?;
        Ok(())
    }
}

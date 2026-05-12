use std::io::{self, Write};

use crossterm::cursor::MoveTo;
use crossterm::style::{Color, ResetColor, SetForegroundColor};
use crossterm::terminal::{Clear, ClearType, ScrollUp};
use crossterm::ExecutableCommand;

pub struct Renderer {
    lines: u16,
    col: u16,
    spinner_tick: bool,
}

impl Renderer {
    pub fn new() -> io::Result<Self> {
        Ok(Renderer { lines: 0, col: 0, spinner_tick: false })
    }

    fn terminal_size(&self) -> (u16, u16) {
        crossterm::terminal::size().unwrap_or((80, 24))
    }

    fn ensure_room(&mut self) {
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
        let mut stdout = io::stdout();
        for line in text.lines() {
            self.ensure_room();
            let r = self.content_row();
            stdout.execute(MoveTo(0, r))?;
            stdout.execute(Clear(ClearType::CurrentLine))?;
            write!(stdout, "{}", SetForegroundColor(color))?;
            let truncated: String = {
                let (cols, _) = self.terminal_size();
                line.chars().take(cols.saturating_sub(1) as usize).collect()
            };
            writeln!(stdout, "{}", truncated)?;
            write!(stdout, "{}", ResetColor)?;
            self.lines = self.lines.saturating_add(1);
            self.col = 0;
        }
        stdout.flush()?;
        Ok(())
    }

    pub fn write(&mut self, text: &str, color: Color) -> io::Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        let mut stdout = io::stdout();
        let (cols, _) = self.terminal_size();
        let parts: Vec<&str> = text.split('\n').collect();
        let max_cols = cols.saturating_sub(1) as usize;
        for (i, line) in parts.iter().enumerate() {
            if i < parts.len() - 1 {
                self.ensure_room();
                let r = self.content_row();
                stdout.execute(MoveTo(self.col, r))?;
                write!(stdout, "{}", SetForegroundColor(color))?;
                let truncated: String = line.chars().take(max_cols.saturating_sub(self.col as usize)).collect();
                writeln!(stdout, "{}", truncated)?;
                write!(stdout, "{}", ResetColor)?;
                self.lines = self.lines.saturating_add(1);
                self.col = 0;
            } else if !line.is_empty() {
                self.ensure_room();
                let r = self.content_row();
                stdout.execute(MoveTo(self.col, r))?;
                write!(stdout, "{}", SetForegroundColor(color))?;
                let truncated: String = line.chars().take(max_cols.saturating_sub(self.col as usize)).collect();
                write!(stdout, "{}", truncated)?;
                write!(stdout, "{}", ResetColor)?;
                self.col = self.col.saturating_add(truncated.len() as u16);
            }
        }
        stdout.flush()?;
        Ok(())
    }

    pub fn clear_content(&mut self) -> io::Result<()> {
        let mut stdout = io::stdout();
        stdout.execute(Clear(ClearType::All))?;
        stdout.execute(MoveTo(0, 0))?;
        stdout.flush()?;
        self.lines = 0;
        self.col = 0;
        Ok(())
    }

    pub fn draw_bottom(&mut self, input_line: &str, cursor_pos: usize, status: &str, is_running: bool) -> io::Result<()> {
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
        write!(stdout, "{}", SetForegroundColor(Color::Cyan))?;
        write!(stdout, "{}", prompt)?;
        write!(stdout, "{}", ResetColor)?;
        let truncated: String = input_line.chars().take(cols.saturating_sub(2) as usize).collect();
        write!(stdout, "{}", truncated)?;

        stdout.execute(MoveTo(0, status_row))?;
        write!(stdout, "{}", " ".repeat(cols as usize))?;
        stdout.execute(MoveTo(0, status_row))?;
        write!(stdout, "{}", SetForegroundColor(Color::DarkGrey))?;
        let truncated: String = status.chars().take(cols as usize).collect();
        write!(stdout, "{}", truncated)?;
        write!(stdout, "{}", ResetColor)?;

        let cursor_x = (2 + cursor_pos.min(input_line.len())) as u16;
        stdout.execute(MoveTo(cursor_x, input_row))?;
        stdout.flush()?;
        Ok(())
    }
}

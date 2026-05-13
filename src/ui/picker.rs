use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crossterm::cursor::MoveTo;
use crossterm::style::{Color, ResetColor, SetForegroundColor};
use crossterm::terminal::Clear;
use crossterm::ExecutableCommand;

pub struct FilePicker {
    pub active: bool,
    pub query: String,
    pub cursor: usize,
    pub matches: Vec<PathBuf>,
    pub selected: usize,
    file_cache: Arc<Mutex<Vec<PathBuf>>>,
    cache_loading: bool,
}

impl FilePicker {
    pub fn new() -> Self {
        FilePicker {
            active: false,
            query: String::new(),
            cursor: 0,
            matches: Vec::new(),
            selected: 0,
            file_cache: Arc::new(Mutex::new(Vec::new())),
            cache_loading: false,
        }
    }

    pub fn activate(&mut self) {
        self.active = true;
        self.query.clear();
        self.cursor = 0;
        self.matches.clear();
        self.selected = 0;
        if !self.file_cache.lock().unwrap().is_empty() {
            self.filter();
        } else if !self.cache_loading {
            self.load_files();
        }
    }

    pub fn deactivate(&mut self) {
        self.active = false;
    }

    fn load_files(&mut self) {
        if self.cache_loading {
            return;
        }
        self.cache_loading = true;

        let cache = self.file_cache.clone();
        std::thread::spawn(move || {
            let mut files = Vec::new();
            let walker = ignore::WalkBuilder::new(".")
                .hidden(false)
                .git_ignore(true)
                .max_depth(Some(8))
                .sort_by_file_name(|a, b| a.cmp(b))
                .build();

            for entry in walker.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                if path.components().any(|c| {
                    c.as_os_str().to_string_lossy().starts_with('.')
                }) {
                    continue;
                }
                let rel = path
                    .strip_prefix(".")
                    .unwrap_or(path)
                    .to_string_lossy()
                    .to_string();
                let rel = rel.trim_start_matches('/').to_string();
                files.push(PathBuf::from(rel));
                if files.len() >= 10_000 {
                    break;
                }
            }

            let mut cache = cache.lock().unwrap();
            *cache = files;
        });
    }

    pub fn char_input(&mut self, c: char) {
        self.query.insert(self.cursor, c);
        self.cursor += 1;
        self.filter();
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.query.remove(self.cursor);
            self.filter();
        }
    }

    fn filter(&mut self) {
        let cache = self.file_cache.lock().unwrap();
        if cache.is_empty() {
            self.matches.clear();
            return;
        }
        let query_lower = self.query.to_lowercase();
        self.matches = cache
            .iter()
            .filter(|p| {
                let lower = p.to_string_lossy().to_lowercase();
                lower.contains(&query_lower)
            })
            .take(50)
            .cloned()
            .collect();
        self.selected = 0;
    }

    pub fn select_next(&mut self) {
        if !self.matches.is_empty() {
            self.selected = (self.selected + 1) % self.matches.len();
        }
    }

    pub fn select_prev(&mut self) {
        if !self.matches.is_empty() {
            self.selected = if self.selected == 0 {
                self.matches.len() - 1
            } else {
                self.selected - 1
            };
        }
    }

    pub fn selected_path(&self) -> Option<&PathBuf> {
        self.matches.get(self.selected)
    }

    pub fn draw(&self) -> std::io::Result<()> {
        if !self.active {
            return Ok(());
        }
        let (cols, rows) = crossterm::terminal::size()?;
        let mut stdout = std::io::stdout();

        let max_items = (rows.saturating_sub(4)).min(10) as usize;

        if self.matches.is_empty() {
            let r = rows.saturating_sub(3);
            stdout.execute(MoveTo(0, r))?;
            write!(stdout, "{}", SetForegroundColor(Color::DarkGrey))?;
            let cache_empty = self.file_cache.lock().unwrap().is_empty();
            let msg = if cache_empty && self.cache_loading {
                "loading files..."
            } else {
                "no matches"
            };
            write!(stdout, "{}", msg)?;
            write!(stdout, "{}", ResetColor)?;
            stdout.flush()?;
            return Ok(());
        }

        let list_height = max_items.min(self.matches.len());
        let start_idx = self
            .selected
            .saturating_sub(list_height / 2)
            .min(self.matches.len().saturating_sub(list_height));
        let end_idx = (start_idx + list_height).min(self.matches.len());

        let top_row = rows.saturating_sub(3).saturating_sub(list_height as u16);

        for i in start_idx..end_idx {
            let render_row = top_row + (i - start_idx) as u16;
            stdout.execute(MoveTo(0, render_row))?;
            write!(stdout, "{}", Clear(crossterm::terminal::ClearType::CurrentLine))?;

            let path = &self.matches[i];
            let display = path.to_string_lossy();
            let truncated: String = display
                .chars()
                .take(cols.saturating_sub(3) as usize)
                .collect();

            if i == self.selected {
                write!(stdout, "{}", SetForegroundColor(Color::Green))?;
                write!(stdout, "▸ {}", truncated)?;
            } else {
                write!(stdout, "{}", SetForegroundColor(Color::DarkGrey))?;
                write!(stdout, "  {}", truncated)?;
            }
            write!(stdout, "{}", ResetColor)?;
        }
        stdout.flush()?;
        Ok(())
    }
}

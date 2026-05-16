use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crossterm::ExecutableCommand;
use crossterm::cursor::MoveTo;
use crossterm::style::{Color, ResetColor, SetForegroundColor};
use crossterm::terminal::Clear;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backspace_empty_query() {
        let mut picker = FilePicker::new();
        picker.test_set_cache(vec![PathBuf::from("test.rs")]);
        // backspace on empty query should not panic
        picker.backspace();
        assert!(picker.query.is_empty());
        assert_eq!(picker.cursor, 0);
    }

    #[test]
    fn test_char_input_and_backspace_ascii() {
        let mut picker = FilePicker::new();
        picker.test_set_cache(vec![PathBuf::from("test.rs")]);
        picker.char_input('a');
        picker.char_input('b');
        picker.char_input('c');
        assert_eq!(picker.query, "abc");
        assert_eq!(picker.cursor, 3);

        picker.backspace();
        assert_eq!(picker.query, "ab");
        assert_eq!(picker.cursor, 2);

        picker.backspace();
        assert_eq!(picker.query, "a");
        assert_eq!(picker.cursor, 1);

        picker.backspace();
        assert_eq!(picker.query, "");
        assert_eq!(picker.cursor, 0);

        // backspace at empty should be no-op
        picker.backspace();
        assert_eq!(picker.query, "");
        assert_eq!(picker.cursor, 0);
    }

    #[test]
    fn test_char_input_and_backspace_unicode() {
        let mut picker = FilePicker::new();
        picker.test_set_cache(vec![PathBuf::from("test.rs")]);

        // Multi-byte UTF-8: é is 2 bytes, ñ is 2 bytes
        picker.char_input('é');
        assert_eq!(picker.query, "é");
        assert_eq!(picker.cursor, 1);

        picker.char_input('ñ');
        assert_eq!(picker.query, "éñ");
        assert_eq!(picker.cursor, 2);

        // Backspace should remove 'ñ' (multi-byte) without panicking
        picker.backspace();
        assert_eq!(picker.query, "é");
        assert_eq!(picker.cursor, 1);

        // Backspace should remove 'é' (multi-byte) without panicking
        picker.backspace();
        assert_eq!(picker.query, "");
        assert_eq!(picker.cursor, 0);

        // Mix ASCII and multi-byte
        picker.char_input('a');
        picker.char_input('é');
        picker.char_input('b');
        assert_eq!(picker.query, "aéb");
        assert_eq!(picker.cursor, 3);

        picker.backspace();
        assert_eq!(picker.query, "aé");
        assert_eq!(picker.cursor, 2);

        picker.backspace();
        assert_eq!(picker.query, "a");
        assert_eq!(picker.cursor, 1);

        picker.backspace();
        assert_eq!(picker.query, "");
        assert_eq!(picker.cursor, 0);
    }

    #[test]
    fn test_mid_query_insertion_unicode() {
        let mut picker = FilePicker::new();
        picker.test_set_cache(vec![PathBuf::from("test.rs")]);

        // Build "ab"
        picker.char_input('a');
        picker.char_input('b');
        assert_eq!(picker.query, "ab");
        assert_eq!(picker.cursor, 2);

        // Move cursor back with backspace to simulate left-arrow + insert
        picker.backspace();
        assert_eq!(picker.query, "a");
        assert_eq!(picker.cursor, 1);

        // Insert multi-byte char in middle (simulating cursor positioning)
        // This tests that nth(1) on "a" gives byte_pos = 1 (end of string)
        picker.char_input('é');
        assert_eq!(picker.query, "aé");
        assert_eq!(picker.cursor, 2);

        // Insert 'c' at the end
        picker.char_input('c');
        assert_eq!(picker.query, "aéc");
        assert_eq!(picker.cursor, 3);

        // Remove from the end (multi-byte in middle)
        picker.backspace();
        assert_eq!(picker.query, "aé");
        assert_eq!(picker.cursor, 2);

        picker.backspace();
        assert_eq!(picker.query, "a");
        assert_eq!(picker.cursor, 1);
    }

    #[test]
    fn test_deactivate_and_reactivate() {
        let mut picker = FilePicker::new();
        picker.test_set_cache(vec![PathBuf::from("test.rs")]);
        picker.char_input('h');
        picker.char_input('i');
        assert_eq!(picker.query, "hi");

        picker.deactivate();
        assert!(!picker.active);

        picker.activate();
        assert!(picker.active);
        assert_eq!(picker.query, "");
        assert_eq!(picker.cursor, 0);
    }

    #[test]
    fn test_backspace_cursor_never_negative() {
        let mut picker = FilePicker::new();
        picker.test_set_cache(vec![PathBuf::from("test.rs")]);
        // Multiple backspaces on empty should keep cursor at 0
        for _ in 0..10 {
            picker.backspace();
        }
        assert_eq!(picker.cursor, 0);
        assert!(picker.query.is_empty());
    }

    #[test]
    fn test_emoji_handling() {
        let mut picker = FilePicker::new();
        picker.test_set_cache(vec![PathBuf::from("test.rs")]);

        // Emoji: "🔥" is 4 bytes in UTF-8
        picker.char_input('🔥');
        assert_eq!(picker.query, "🔥");
        assert_eq!(picker.cursor, 1);

        picker.char_input('x');
        assert_eq!(picker.query, "🔥x");
        assert_eq!(picker.cursor, 2);

        picker.backspace();
        assert_eq!(picker.query, "🔥");
        assert_eq!(picker.cursor, 1);

        picker.backspace();
        assert_eq!(picker.query, "");
        assert_eq!(picker.cursor, 0);
    }
}

pub struct FilePicker {
    pub active: bool,
    pub query: String,
    pub cursor: usize,
    pub matches: Vec<PathBuf>,
    pub selected: usize,
    file_cache: Arc<Mutex<Vec<PathBuf>>>,
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
        }
    }

    pub fn activate(&mut self) {
        self.active = true;
        self.query.clear();
        self.cursor = 0;
        self.matches.clear();
        self.selected = 0;
        self.load_files();
        self.filter();
    }

    pub fn deactivate(&mut self) {
        self.active = false;
    }

    fn load_files(&mut self) {
        let files = walk_files(".");
        *self.file_cache.lock().unwrap() = files;
    }

    pub fn char_input(&mut self, c: char) {
        // Convert char-index cursor to byte index for String::insert
        let byte_pos = self
            .query
            .char_indices()
            .nth(self.cursor)
            .map(|(i, _)| i)
            .unwrap_or(self.query.len());
        self.query.insert(byte_pos, c);
        self.cursor += 1;
        self.filter();
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 && !self.query.is_empty() {
            self.cursor -= 1;
            // Convert char-index cursor to byte index for String::remove
            let byte_pos = self
                .query
                .char_indices()
                .nth(self.cursor)
                .map(|(i, _)| i)
                .unwrap_or(self.query.len());
            self.query.remove(byte_pos);
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

    #[cfg(test)]
    pub fn test_set_cache(&mut self, files: Vec<PathBuf>) {
        *self.file_cache.lock().unwrap() = files;
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
            write!(stdout, "{}", "no matches")?;
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
            write!(
                stdout,
                "{}",
                Clear(crossterm::terminal::ClearType::CurrentLine)
            )?;

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

fn walk_files(root: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
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
        if path
            .components()
            .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
        {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();
        let rel = rel.trim_start_matches('/').to_string();
        files.push(PathBuf::from(rel));
        if files.len() >= 20 {
            break;
        }
    }
    files
}

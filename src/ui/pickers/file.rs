use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crossterm::ExecutableCommand;
use crossterm::cursor::MoveTo;
use crossterm::style::{Color, ResetColor, SetForegroundColor};
use crossterm::terminal::Clear;

use super::super::utils::{UiColors, resolve_color};

pub struct FilePicker {
    pub active: bool,
    pub query: String,
    pub cursor: usize,
    pub matches: Vec<PathBuf>,
    pub selected: usize,
    file_cache: Arc<Mutex<Vec<PathBuf>>>,
    monochrome: bool,
    loading: bool,
    walk_done: Arc<AtomicBool>,
    colors: UiColors,
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
            monochrome: false,
            colors: UiColors::default_colors(),
            loading: false,
            walk_done: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn set_monochrome(&mut self, monochrome: bool) {
        self.monochrome = monochrome;
    }

    pub fn set_colors(&mut self, colors: UiColors) {
        self.colors = colors;
    }

    fn color(&self, color: Color) -> Color {
        resolve_color(color, self.monochrome)
    }

    pub fn activate(&mut self) {
        self.active = true;
        self.query.clear();
        self.cursor = 0;
        self.matches.clear();
        self.selected = 0;

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            self.loading = true;
            self.walk_done.store(false, Ordering::Relaxed);
            let cache = self.file_cache.clone();
            let done = self.walk_done.clone();
            handle.spawn_blocking(move || {
                let files = walk_files(".");
                *cache.lock().unwrap_or_else(|e| e.into_inner()) = files;
                done.store(true, Ordering::Relaxed);
            });
        } else {
            self.load_files_sync();
        }
    }

    fn load_files_sync(&mut self) {
        let files = walk_files(".");
        *self.file_cache.lock().unwrap_or_else(|e| e.into_inner()) = files;
        self.filter();
    }

    pub fn deactivate(&mut self) {
        self.active = false;
    }

    pub fn try_finish_loading(&mut self) -> bool {
        if self.loading && self.walk_done.load(Ordering::Relaxed) {
            self.loading = false;
            self.filter();
            true
        } else {
            false
        }
    }

    pub fn char_input(&mut self, c: char) {
        let byte_pos = self
            .query
            .char_indices()
            .nth(self.cursor)
            .map(|(i, _)| i)
            .unwrap_or(self.query.len());
        self.query.insert(byte_pos, c);
        self.cursor += 1;
        if !self.loading {
            self.filter();
        }
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 && !self.query.is_empty() {
            self.cursor -= 1;
            let byte_pos = self
                .query
                .char_indices()
                .nth(self.cursor)
                .map(|(i, _)| i)
                .unwrap_or(self.query.len());
            self.query.remove(byte_pos);
            if !self.loading {
                self.filter();
            }
        }
    }

    fn filter(&mut self) {
        let cache = self.file_cache.lock().unwrap_or_else(|e| e.into_inner());
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
        *self.file_cache.lock().unwrap_or_else(|e| e.into_inner()) = files;
        self.loading = false;
    }

    pub fn draw(&mut self) -> std::io::Result<()> {
        if !self.active {
            return Ok(());
        }

        self.try_finish_loading();

        let (cols, rows) = crossterm::terminal::size()?;
        let mut stdout = std::io::stdout();

        let max_items = (rows.saturating_sub(4)).min(10) as usize;

        if self.loading {
            let r = rows.saturating_sub(3);
            stdout.execute(MoveTo(0, r))?;
            write!(
                stdout,
                "{}",
                SetForegroundColor(self.color(self.colors.picker_secondary))
            )?;
            write!(stdout, "scanning files...")?;
            write!(stdout, "{}", ResetColor)?;
            stdout.flush()?;
            return Ok(());
        }

        if self.matches.is_empty() {
            let r = rows.saturating_sub(3);
            stdout.execute(MoveTo(0, r))?;
            write!(
                stdout,
                "{}",
                SetForegroundColor(self.color(self.colors.picker_secondary))
            )?;
            write!(stdout, "no matches")?;
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
            let mut display = path.to_string_lossy().to_string();
            if Path::new(&path).is_dir() {
                display.push('/');
            }
            let truncated: String = display
                .chars()
                .take(cols.saturating_sub(3) as usize)
                .collect();

            if i == self.selected {
                write!(
                    stdout,
                    "{}",
                    SetForegroundColor(self.color(self.colors.picker_selected))
                )?;
                write!(stdout, "▸ {}", truncated)?;
            } else {
                write!(
                    stdout,
                    "{}",
                    SetForegroundColor(self.color(self.colors.picker_secondary))
                )?;
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
        if !path.is_file() && !path.is_dir() {
            continue;
        }
        if path
            .components()
            .any(|c| matches!(c, Component::Normal(n) if n.to_string_lossy().starts_with('.')))
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
        if files.len() >= 200 {
            break;
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn with_temp_dir<F>(f: F)
    where
        F: FnOnce(&Path),
    {
        let dir = std::env::temp_dir().join(format!("zerostack_test_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let canonical = dir.canonicalize().unwrap();
        f(&canonical);
        let _ = fs::remove_dir_all(&canonical);
    }

    #[test]
    fn test_walk_files_includes_directories() {
        with_temp_dir(|root| {
            fs::create_dir(root.join("subdir")).unwrap();
            fs::write(root.join("file.txt"), b"hello").unwrap();

            let files = walk_files(&root.to_string_lossy());
            let names: Vec<&str> = files.iter().map(|p| p.to_str().unwrap()).collect();

            assert!(
                names.contains(&"file.txt"),
                "walk_files should include files"
            );
            assert!(
                names.contains(&"subdir"),
                "walk_files should include directories, got: {:?}",
                names
            );
        });
    }

    #[test]
    fn test_walk_files_includes_nested_dirs() {
        with_temp_dir(|root| {
            fs::create_dir_all(root.join("a").join("b")).unwrap();
            fs::write(root.join("a").join("b").join("deep.txt"), b"deep").unwrap();

            let files = walk_files(&root.to_string_lossy());
            let names: Vec<&str> = files.iter().map(|p| p.to_str().unwrap()).collect();

            assert!(names.contains(&"a"));
            assert!(names.contains(&"a/b"));
            assert!(names.contains(&"a/b/deep.txt"));
        });
    }
}

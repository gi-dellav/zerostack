use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use crossterm::ExecutableCommand;
use crossterm::cursor::MoveTo;
use crossterm::style::{Color, ResetColor, SetForegroundColor};
use crossterm::terminal::Clear;

use super::super::utils::resolve_color;

/// Paths per batch sent from the background walk to the picker. Small
/// batches keep the first matches visible quickly on large trees.
const WALK_BATCH_SIZE: usize = 25;

/// Hard cap on walked paths, same bound the picker always had.
const MAX_WALK_FILES: usize = 200;

pub struct FilePicker {
    pub active: bool,
    pub query: String,
    pub cursor: usize,
    pub matches: Vec<PathBuf>,
    pub selected: usize,
    file_cache: Vec<PathBuf>,
    monochrome: bool,
    loading: bool,
    walk_rx: Option<mpsc::Receiver<Vec<PathBuf>>>,
    walk_cancel: Arc<AtomicBool>,
}

impl FilePicker {
    pub fn new() -> Self {
        FilePicker {
            active: false,
            query: String::new(),
            cursor: 0,
            matches: Vec::new(),
            selected: 0,
            file_cache: Vec::new(),
            monochrome: false,
            loading: false,
            walk_rx: None,
            walk_cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn set_monochrome(&mut self, monochrome: bool) {
        self.monochrome = monochrome;
    }

    fn color(&self, color: Color) -> Color {
        resolve_color(color, self.monochrome)
    }

    pub fn activate(&mut self) {
        // Cancel any walk still running from a previous activation before
        // arming a fresh flag for the new one.
        self.walk_cancel.store(true, Ordering::Relaxed);
        self.walk_rx = None;

        self.active = true;
        self.query.clear();
        self.cursor = 0;
        self.matches.clear();
        self.selected = 0;
        self.file_cache.clear();

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            self.loading = true;
            self.walk_cancel = Arc::new(AtomicBool::new(false));
            let cancel = self.walk_cancel.clone();
            let (tx, rx) = mpsc::channel();
            self.walk_rx = Some(rx);
            handle.spawn_blocking(move || {
                walk_files_streaming(".", &cancel, |batch| tx.send(batch).is_ok());
            });
        } else {
            self.load_files_sync();
        }
    }

    fn load_files_sync(&mut self) {
        self.file_cache = walk_files(".");
        self.filter();
    }

    pub fn deactivate(&mut self) {
        self.active = false;
        // Stop the background walk (if any): the flag makes it exit early,
        // and dropping the receiver makes its next batch send fail.
        self.walk_cancel.store(true, Ordering::Relaxed);
        self.walk_rx = None;
        self.loading = false;
    }

    /// Drain walk batches that arrived since the last call. Returns true
    /// when new files arrived or the walk just finished.
    pub fn try_finish_loading(&mut self) -> bool {
        if !self.loading {
            return false;
        }
        let mut changed = false;
        let mut done = false;
        if let Some(rx) = &self.walk_rx {
            loop {
                match rx.try_recv() {
                    Ok(batch) => {
                        self.file_cache.extend(batch);
                        changed = true;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        done = true;
                        break;
                    }
                }
            }
        }
        if done {
            self.loading = false;
            self.walk_rx = None;
        }
        if changed || done {
            self.filter();
        }
        changed || done
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
        // Filter even while the walk is streaming so matches update live.
        self.filter();
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
            self.filter();
        }
    }

    fn filter(&mut self) {
        if self.file_cache.is_empty() {
            self.matches.clear();
            return;
        }
        let query_lower = self.query.to_lowercase();
        self.matches = self
            .file_cache
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
        self.file_cache = files;
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

        if self.loading && self.matches.is_empty() {
            let r = rows.saturating_sub(3);
            stdout.execute(MoveTo(0, r))?;
            write!(
                stdout,
                "{}",
                SetForegroundColor(self.color(Color::DarkGrey))
            )?;
            write!(stdout, "scanning files...")?;
            write!(stdout, "{}", ResetColor)?;
            stdout.flush()?;
            return Ok(());
        }

        if self.matches.is_empty() {
            let r = rows.saturating_sub(4);
            stdout.execute(MoveTo(0, r))?;
            write!(
                stdout,
                "{}",
                SetForegroundColor(self.color(Color::DarkGrey))
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
                write!(stdout, "{}", SetForegroundColor(self.color(Color::Green)))?;
                write!(stdout, "▸ {}", truncated)?;
            } else {
                write!(
                    stdout,
                    "{}",
                    SetForegroundColor(self.color(Color::DarkGrey))
                )?;
                write!(stdout, "  {}", truncated)?;
            }
            write!(stdout, "{}", ResetColor)?;
        }
        stdout.flush()?;
        Ok(())
    }
}

/// Walk `root`, invoking `emit` with batches of paths as they are found so
/// the picker can show matches incrementally. Stops early when `cancel` is
/// set (Esc/deactivate) or when `emit` returns false (receiver dropped).
pub(crate) fn walk_files_streaming(
    root: &str,
    cancel: &AtomicBool,
    mut emit: impl FnMut(Vec<PathBuf>) -> bool,
) {
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .max_depth(Some(8))
        .sort_by_file_name(|a, b| a.cmp(b))
        .build();

    let mut batch = Vec::with_capacity(WALK_BATCH_SIZE);
    let mut total = 0usize;
    for entry in walker.flatten() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
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
        batch.push(PathBuf::from(rel));
        total += 1;
        if batch.len() >= WALK_BATCH_SIZE && !emit(std::mem::take(&mut batch)) {
            return;
        }
        if total >= MAX_WALK_FILES {
            break;
        }
    }
    if !batch.is_empty() {
        let _ = emit(batch);
    }
}

/// Collect the full walk into a `Vec` (used by the synchronous fallback and
/// tests).
pub(crate) fn walk_files(root: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();
    walk_files_streaming(root, &AtomicBool::new(false), |batch| {
        files.extend(batch);
        true
    });
    files
}

pub(crate) mod cursor;
mod pickers;

pub use cursor::cursor_to_line_col;
pub use cursor::{
    count_lines, line_col_to_cursor, line_end, line_start, next_char_boundary, prev_char_boundary,
};
pub use pickers::Picker;

use compact_str::CompactString;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::io::Write;

use crate::ui::pickers::file::FilePicker;
use crate::ui::pickers::list::ListPicker;
use crate::ui::pickers::models::ModelsPicker;
use crate::ui::utils::UiColors;

const MAX_KILL_RING: usize = 30;

pub struct InputEditor {
    pub buffer: CompactString,
    pub cursor: usize,
    history: Vec<CompactString>,
    history_pos: Option<usize>,
    draft: Option<CompactString>,
    pub picker: Option<Picker>,
    monochrome: bool,
    colors: UiColors,
    prompt_names: Vec<String>,
    theme_names: Vec<String>,
    quick_model_names: Vec<String>,
    live_model_names: Vec<String>,
    provider_names: Vec<String>,
    editor: Option<String>,
    kill_ring: Vec<CompactString>,
    yank_pos: Option<usize>,
    yank_len: usize,
}

impl InputEditor {
    pub fn new() -> Self {
        InputEditor {
            buffer: CompactString::new(""),
            cursor: 0,
            history: Vec::new(),
            history_pos: None,
            draft: None,
            picker: None,
            monochrome: false,
            colors: UiColors::default_colors(),
            prompt_names: Vec::new(),
            theme_names: Vec::new(),
            quick_model_names: Vec::new(),
            live_model_names: Vec::new(),
            provider_names: Vec::new(),
            editor: None,
            kill_ring: Vec::with_capacity(MAX_KILL_RING),
            yank_pos: None,
            yank_len: 0,
        }
    }

    pub fn set_quick_model_names(&mut self, names: Vec<String>) {
        self.quick_model_names = names;
    }

    pub fn set_live_model_names(&mut self, names: Vec<String>) {
        self.live_model_names = names;
    }

    pub fn set_provider_names(&mut self, names: Vec<String>) {
        self.provider_names = names;
    }

    pub fn set_editor(&mut self, editor: String) {
        self.editor = Some(editor);
    }

    pub fn set_monochrome(&mut self, monochrome: bool) {
        self.monochrome = monochrome;
        if let Some(ref mut picker) = self.picker {
            picker.set_monochrome(monochrome);
        }
    }

    pub fn set_colors(&mut self, colors: UiColors) {
        self.colors = colors.clone();
        if let Some(ref mut picker) = self.picker {
            picker.set_colors(colors);
        }
    }

    pub fn set_prompt_names(&mut self, names: Vec<String>) {
        self.prompt_names = names;
    }

    pub fn set_theme_names(&mut self, names: Vec<String>) {
        self.theme_names = names;
    }

    pub fn load_global_history(&mut self) {
        if let Ok(entries) = crate::session::chat_history::load_history() {
            self.history = entries
                .into_iter()
                .map(|e| CompactString::new(e.content))
                .collect();
            self.history_pos = None;
        }
    }

    pub fn start_file_picker(&mut self) {
        let mut picker = FilePicker::new();
        picker.set_monochrome(self.monochrome);
        picker.set_colors(self.colors.clone());
        picker.activate();
        self.picker = Some(Picker::File(picker));
    }

    pub fn start_command_picker(&mut self) {
        let mut picker = ListPicker::with_static_commands();
        picker.set_monochrome(self.monochrome);
        picker.set_colors(self.colors.clone());
        picker.activate();
        self.picker = Some(Picker::Command(picker));
    }

    pub fn start_models_picker(&mut self) {
        let mut picker = ModelsPicker::new();
        picker.set_monochrome(self.monochrome);
        picker.set_colors(self.colors.clone());
        picker.set_groups(
            self.quick_model_names.clone(),
            self.live_model_names.clone(),
        );
        picker.activate();
        self.picker = Some(Picker::Models(picker));
    }

    pub fn start_provider_picker(&mut self) {
        let mut picker = ListPicker::new();
        picker.set_monochrome(self.monochrome);
        picker.set_colors(self.colors.clone());
        if !self.provider_names.is_empty() {
            picker.set_items(self.provider_names.clone());
        }
        picker.activate();
        self.picker = Some(Picker::Prefixed(picker, "/provider "));
    }

    pub fn start_prompt_picker(&mut self) {
        let mut picker = ListPicker::new();
        picker.set_monochrome(self.monochrome);
        picker.set_colors(self.colors.clone());
        if !self.prompt_names.is_empty() {
            picker.set_items(self.prompt_names.clone());
        }
        picker.activate();
        self.picker = Some(Picker::Prefixed(picker, "/prompt "));
    }

    pub fn start_dot_picker(&mut self) {
        let mut picker = ListPicker::new();
        picker.set_monochrome(self.monochrome);
        picker.set_colors(self.colors.clone());
        if !self.prompt_names.is_empty() {
            picker.set_items(self.prompt_names.clone());
        }
        picker.activate();
        self.picker = Some(Picker::Prefixed(picker, "."));
    }

    pub fn start_theme_picker(&mut self) {
        let mut picker = ListPicker::new();
        picker.set_monochrome(self.monochrome);
        picker.set_colors(self.colors.clone());
        if !self.theme_names.is_empty() {
            picker.set_items(self.theme_names.clone());
        }
        picker.activate();
        self.picker = Some(Picker::Prefixed(picker, "/theme "));
    }

    pub fn open_in_editor(&mut self) {
        let editor = self
            .editor
            .clone()
            .or_else(|| std::env::var("EDITOR").ok())
            .unwrap_or_else(|| "editor".to_string());

        let tmp = std::env::temp_dir().join(format!("zerostack-{}.md", std::process::id()));

        let _ = std::fs::write(&tmp, self.buffer.as_bytes());

        let _ = crossterm::terminal::disable_raw_mode();
        let mut stdout = std::io::stdout();
        let _ = crossterm::ExecutableCommand::execute(
            &mut stdout,
            crossterm::event::DisableMouseCapture,
        );
        let _ = crossterm::ExecutableCommand::execute(
            &mut stdout,
            crossterm::terminal::LeaveAlternateScreen,
        );
        let _ = stdout.flush();

        let _ = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("{} \"$1\"", editor))
            .arg("sh")
            .arg(&tmp)
            .status();

        let _ = crossterm::ExecutableCommand::execute(
            &mut stdout,
            crossterm::terminal::EnterAlternateScreen,
        );
        let _ = crossterm::ExecutableCommand::execute(
            &mut stdout,
            crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
        );
        let _ = crossterm::ExecutableCommand::execute(
            &mut stdout,
            crossterm::event::EnableMouseCapture,
        );
        let _ = crossterm::terminal::enable_raw_mode();

        if let Ok(content) = std::fs::read_to_string(&tmp) {
            self.buffer = CompactString::new(content.trim_end());
            self.cursor = self.buffer.len();
        }

        let _ = std::fs::remove_file(&tmp);
    }

    pub fn handle_paste(&mut self, data: String) {
        self.buffer.insert_str(self.cursor, &data);
        self.cursor += data.len();
        self.history_pos = None;
        self.draft = None;
        self.yank_pos = None;
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<CompactString> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);

        if ctrl {
            match key.code {
                KeyCode::Char('a') => {
                    let current = line_start(&self.buffer, self.cursor);
                    if self.cursor == current {
                        let (line, _) = cursor_to_line_col(&self.buffer, self.cursor);
                        if line > 0 {
                            self.cursor = line_end(&self.buffer, current - 1);
                        }
                    } else {
                        self.cursor = current;
                    }
                    self.yank_pos = None;
                    return None;
                }
                KeyCode::Char('e') => {
                    let current = line_end(&self.buffer, self.cursor);
                    if self.cursor == current {
                        let (line, _) = cursor_to_line_col(&self.buffer, self.cursor);
                        let total = count_lines(&self.buffer);
                        if line + 1 < total {
                            self.cursor = line_start(&self.buffer, self.cursor + 1);
                        }
                    } else {
                        self.cursor = current;
                    }
                    self.yank_pos = None;
                    return None;
                }
                KeyCode::Char('b') => {
                    if self.cursor > 0 {
                        self.cursor = prev_char_boundary(&self.buffer, self.cursor);
                    }
                    self.yank_pos = None;
                    return None;
                }
                KeyCode::Char('f') => {
                    if self.cursor < self.buffer.len() {
                        self.cursor = next_char_boundary(&self.buffer, self.cursor);
                    }
                    self.yank_pos = None;
                    return None;
                }
                KeyCode::Char('p') => {
                    return self.cursor_up();
                }
                KeyCode::Char('n') => {
                    return self.cursor_down();
                }
                KeyCode::Char('w') => {
                    let deleted = self.delete_prev_word();
                    if !deleted.is_empty() {
                        self.push_kill(deleted);
                    }
                    self.yank_pos = None;
                    return None;
                }
                KeyCode::Char('u') => {
                    if self.cursor > 0 {
                        let deleted: String = self.buffer.chars().take(self.cursor).collect();
                        let remaining: String = self.buffer.chars().skip(self.cursor).collect();
                        self.buffer = CompactString::new(&remaining);
                        self.cursor = 0;
                        if !deleted.is_empty() {
                            self.push_kill(CompactString::new(&deleted));
                        }
                    }
                    self.yank_pos = None;
                    return None;
                }
                KeyCode::Char('k') => {
                    if self.cursor < self.buffer.len() {
                        let deleted: String = self.buffer.chars().skip(self.cursor).collect();
                        let before: String = self.buffer.chars().take(self.cursor).collect();
                        self.buffer = CompactString::new(&before);
                        if !deleted.is_empty() {
                            self.push_kill(CompactString::new(&deleted));
                        }
                    }
                    self.yank_pos = None;
                    return None;
                }
                KeyCode::Char('d') => {
                    if self.cursor < self.buffer.len() {
                        self.buffer.remove(self.cursor);
                    }
                    self.yank_pos = None;
                    return None;
                }
                KeyCode::Char('y') => {
                    if self.kill_ring.is_empty() {
                        return None;
                    }
                    let pos = self.yank_pos.unwrap_or(0);
                    let text = &self.kill_ring[pos];
                    self.buffer.insert_str(self.cursor, text);
                    self.yank_len = text.len();
                    self.cursor += text.len();
                    self.yank_pos = Some(pos);
                    return None;
                }
                _ => {}
            }
        }

        if alt {
            match key.code {
                KeyCode::Char('b') => {
                    self.cursor = self.prev_word_start();
                    self.yank_pos = None;
                    return None;
                }
                KeyCode::Char('f') => {
                    self.cursor = self.next_word_end();
                    self.yank_pos = None;
                    return None;
                }
                KeyCode::Char('d') => {
                    let deleted = self.delete_next_word();
                    if !deleted.is_empty() {
                        self.push_kill(deleted);
                    }
                    self.yank_pos = None;
                    return None;
                }
                KeyCode::Char('y') => {
                    if let Some(pos) = self.yank_pos
                        && self.kill_ring.len() > 1
                    {
                        let start = self.cursor.saturating_sub(self.yank_len);
                        if start <= self.cursor {
                            let before: String = self.buffer.chars().take(start).collect();
                            let after: String = self.buffer.chars().skip(self.cursor).collect();
                            self.buffer = CompactString::new(format!("{}{}", before, after));
                            self.cursor = start;
                        }
                        let new_pos = if pos == 0 {
                            self.kill_ring.len() - 1
                        } else {
                            pos - 1
                        };
                        self.yank_pos = Some(new_pos);
                        let text = &self.kill_ring[new_pos];
                        self.buffer.insert_str(self.cursor, text);
                        self.yank_len = text.len();
                        self.cursor += text.len();
                    }
                    return None;
                }
                _ => {}
            }
        }

        match key.code {
            KeyCode::Enter
                if key.modifiers.contains(KeyModifiers::SHIFT)
                    || key.modifiers.contains(KeyModifiers::ALT) =>
            {
                if self.picker.as_ref().is_some_and(|p| p.active()) {
                    return None;
                }
                self.buffer.insert(self.cursor, '\n');
                self.cursor += 1;
                None
            }
            KeyCode::Enter => {
                if self.picker.as_ref().is_some_and(|p| p.active()) {
                    return None;
                }
                let text = self.buffer.clone();
                let is_blank = text.trim().is_empty();
                if !is_blank {
                    self.history.push(text.clone());
                }
                self.history_pos = None;
                self.draft = None;
                self.buffer.clear();
                self.cursor = 0;
                self.yank_pos = None;
                if text.is_empty() { None } else { Some(text) }
            }
            KeyCode::Char(c)
                if c == '\x08' || (c == 'h' && key.modifiers.contains(KeyModifiers::CONTROL)) =>
            {
                if self.cursor > 0 {
                    self.cursor = prev_char_boundary(&self.buffer, self.cursor);
                    self.buffer.remove(self.cursor);
                }
                None
            }
            KeyCode::Char(c) => {
                if c == '@' {
                    let at_word_start = self.cursor == 0
                        || self.buffer[..self.cursor]
                            .chars()
                            .next_back()
                            .is_some_and(|prev| prev == ' ');
                    if at_word_start {
                        self.start_file_picker();
                    }
                }
                if c == '/' && self.cursor == 0 {
                    self.start_command_picker();
                }
                if c == '.' && self.cursor == 0 {
                    self.buffer.insert(self.cursor, c);
                    self.cursor += c.len_utf8();
                    self.start_dot_picker();
                    self.yank_pos = None;
                    return None;
                }
                self.buffer.insert(self.cursor, c);
                self.cursor += c.len_utf8();
                self.history_pos = None;
                self.draft = None;
                self.yank_pos = None;

                if (self.picker.is_none() || !self.picker.as_ref().is_some_and(|p| p.active()))
                    && self.buffer.starts_with("/prompt ")
                {
                    let after_prefix: String = self.buffer.chars().skip("/prompt ".len()).collect();
                    if !after_prefix.is_empty() && c != ' ' {
                        let query_len = after_prefix.len();
                        if query_len == 1 {
                            self.start_prompt_picker();
                            if let Some(Picker::Prefixed(ref mut pp, _)) = self.picker {
                                pp.char_input(c);
                            }
                        }
                    }
                }
                if (self.picker.is_none() || !self.picker.as_ref().is_some_and(|p| p.active()))
                    && self.buffer.starts_with("/models ")
                {
                    let after_prefix: String = self.buffer.chars().skip("/models ".len()).collect();
                    if !after_prefix.is_empty() && c != ' ' {
                        let query_len = after_prefix.len();
                        if query_len == 1 {
                            self.start_models_picker();
                            if let Some(Picker::Models(ref mut mp)) = self.picker {
                                mp.char_input(c);
                            }
                        }
                    }
                }
                if (self.picker.is_none() || !self.picker.as_ref().is_some_and(|p| p.active()))
                    && self.buffer.starts_with("/theme ")
                {
                    let after_prefix: String = self.buffer.chars().skip("/theme ".len()).collect();
                    if !after_prefix.is_empty() && c != ' ' {
                        let query_len = after_prefix.len();
                        if query_len == 1 {
                            self.start_theme_picker();
                            if let Some(Picker::Prefixed(ref mut tp, _)) = self.picker {
                                tp.char_input(c);
                            }
                        }
                    }
                }
                if (self.picker.is_none() || !self.picker.as_ref().is_some_and(|p| p.active()))
                    && self.buffer.starts_with("/provider ")
                {
                    let after_prefix: String =
                        self.buffer.chars().skip("/provider ".len()).collect();
                    if !after_prefix.is_empty() && c != ' ' {
                        let query_len = after_prefix.len();
                        if query_len == 1 {
                            self.start_provider_picker();
                            if let Some(Picker::Prefixed(ref mut pp, _)) = self.picker {
                                pp.char_input(c);
                            }
                        }
                    }
                }

                None
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    self.cursor = prev_char_boundary(&self.buffer, self.cursor);
                    self.buffer.remove(self.cursor);
                }
                self.yank_pos = None;
                None
            }
            KeyCode::Delete => {
                if self.cursor < self.buffer.len() {
                    self.buffer.remove(self.cursor);
                }
                self.yank_pos = None;
                None
            }
            KeyCode::Left => {
                if self.cursor > 0 {
                    self.cursor = prev_char_boundary(&self.buffer, self.cursor);
                }
                self.yank_pos = None;
                None
            }
            KeyCode::Right => {
                if self.cursor < self.buffer.len() {
                    self.cursor = next_char_boundary(&self.buffer, self.cursor);
                }
                self.yank_pos = None;
                None
            }
            KeyCode::Up => {
                self.yank_pos = None;
                self.cursor_up()
            }
            KeyCode::Down => {
                self.yank_pos = None;
                self.cursor_down()
            }
            KeyCode::Home => {
                self.cursor = 0;
                self.yank_pos = None;
                None
            }
            KeyCode::End => {
                self.cursor = self.buffer.len();
                self.yank_pos = None;
                None
            }
            KeyCode::Tab => {
                self.buffer.insert_str(self.cursor, "  ");
                self.cursor += 2;
                self.yank_pos = None;
                None
            }
            _ => None,
        }
    }

    fn history_up(&mut self) -> Option<CompactString> {
        let hist_len = self.history.len();
        if hist_len == 0 || self.history_pos == Some(0) {
            return None;
        }
        if self.history_pos.is_none() {
            self.draft = Some(self.buffer.clone());
        }
        let pos = match self.history_pos {
            Some(p) if p > 0 => p - 1,
            Some(_) => unreachable!(),
            None => hist_len - 1,
        };
        self.history_pos = Some(pos);
        self.buffer = self.history[pos].clone();
        self.cursor = 0;
        None
    }

    fn history_down(&mut self) -> Option<CompactString> {
        match self.history_pos {
            Some(pos) if pos + 1 < self.history.len() => {
                let new_pos = pos + 1;
                self.history_pos = Some(new_pos);
                self.buffer = self.history[new_pos].clone();
                self.cursor = self.buffer.len();
            }
            Some(_) => {
                self.history_pos = None;
                if let Some(draft) = self.draft.take() {
                    self.buffer = draft.clone();
                    self.cursor = self.buffer.len();
                } else {
                    self.buffer.clear();
                    self.cursor = 0;
                }
            }
            None => {}
        }
        None
    }

    fn cursor_up(&mut self) -> Option<CompactString> {
        let (line, col) = cursor_to_line_col(&self.buffer, self.cursor);
        if line > 0 {
            let line_len =
                line_end(&self.buffer, self.cursor) - line_start(&self.buffer, self.cursor);
            let target = line_col_to_cursor(
                &self.buffer,
                line - 1,
                if col >= line_len { usize::MAX } else { col },
            );
            self.cursor = target;
            None
        } else {
            self.history_up()
        }
    }

    fn cursor_down(&mut self) -> Option<CompactString> {
        let (line, col) = cursor_to_line_col(&self.buffer, self.cursor);
        let total = count_lines(&self.buffer);
        if line + 1 < total {
            let line_len =
                line_end(&self.buffer, self.cursor) - line_start(&self.buffer, self.cursor);
            let target = line_col_to_cursor(
                &self.buffer,
                line + 1,
                if col >= line_len { usize::MAX } else { col },
            );
            self.cursor = target;
            None
        } else {
            self.history_down()
        }
    }

    fn push_kill(&mut self, text: CompactString) {
        if text.is_empty() {
            return;
        }
        if self.kill_ring.first() == Some(&text) {
            return;
        }
        self.kill_ring.insert(0, text);
        if self.kill_ring.len() > MAX_KILL_RING {
            self.kill_ring.pop();
        }
    }

    fn prev_word_start(&self) -> usize {
        if self.cursor == 0 {
            return 0;
        }
        let pairs: Vec<(usize, char)> = self.buffer.char_indices().collect();
        if pairs.is_empty() {
            return 0;
        }
        let char_idx = pairs
            .iter()
            .position(|&(bi, _)| bi >= self.cursor)
            .unwrap_or(pairs.len());
        let mut pos = char_idx;
        while pos > 0 && pairs[pos - 1].1 == ' ' {
            pos -= 1;
        }
        while pos > 0 && pairs[pos - 1].1 != ' ' {
            pos -= 1;
        }
        if pos < pairs.len() {
            pairs[pos].0
        } else {
            self.buffer.len()
        }
    }

    fn next_word_end(&self) -> usize {
        let pairs: Vec<(usize, char)> = self.buffer.char_indices().collect();
        let len = pairs.len();
        if len == 0 {
            return 0;
        }
        let char_idx = pairs
            .iter()
            .position(|&(bi, _)| bi >= self.cursor)
            .unwrap_or(len);
        let mut pos = char_idx;
        while pos < len && pairs[pos].1 == ' ' {
            pos += 1;
        }
        while pos < len && pairs[pos].1 != ' ' {
            pos += 1;
        }
        if pos < len {
            pairs[pos].0
        } else {
            self.buffer.len()
        }
    }

    fn delete_prev_word(&mut self) -> CompactString {
        if self.cursor == 0 || self.buffer.is_empty() {
            return CompactString::new("");
        }
        let start = self.prev_word_start();
        let deleted: CompactString = self.buffer[start..self.cursor].into();
        let before = &self.buffer[..start];
        let after = &self.buffer[self.cursor..];
        let mut new_buf = String::with_capacity(before.len() + after.len());
        new_buf.push_str(before);
        new_buf.push_str(after);
        self.buffer = CompactString::new(&new_buf);
        self.cursor = start;
        deleted
    }

    fn delete_next_word(&mut self) -> CompactString {
        if self.cursor >= self.buffer.len() {
            return CompactString::new("");
        }
        let end = self.next_word_end();
        let deleted: CompactString = self.buffer[self.cursor..end].into();
        let before = &self.buffer[..self.cursor];
        let after = &self.buffer[end..];
        let mut new_buf = String::with_capacity(before.len() + after.len());
        new_buf.push_str(before);
        new_buf.push_str(after);
        self.buffer = CompactString::new(&new_buf);
        deleted
    }
}

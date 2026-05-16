use compact_str::CompactString;
use crossterm::event::{KeyCode, KeyEvent};

use crate::ui::picker::FilePicker;

pub struct InputEditor {
    pub buffer: CompactString,
    pub cursor: usize,
    history: Vec<CompactString>,
    history_pos: Option<usize>,
    pub picker: Option<FilePicker>,
}

impl InputEditor {
    pub fn new() -> Self {
        InputEditor {
            buffer: CompactString::new(""),
            cursor: 0,
            history: Vec::new(),
            history_pos: None,
            picker: None,
        }
    }

    pub fn start_picker(&mut self) {
        let picker = self.picker.get_or_insert_with(FilePicker::new);
        picker.activate();
    }

    pub fn handle_picker_key(&mut self, key: KeyEvent) -> bool {
        let picker = match self.picker.as_mut() {
            Some(p) if p.active => p,
            _ => return false,
        };

        match key.code {
            KeyCode::Char(c) => {
                picker.char_input(c);
                self.buffer.insert(self.cursor, c);
                self.cursor += 1;
                true
            }
            KeyCode::Backspace => {
                if picker.cursor > 0 {
                    picker.backspace();
                    self.cursor -= 1;
                    self.buffer.remove(self.cursor);
                    true
                } else {
                    // backspace at start of query: cancel picker, remove @
                    let at_pos = self.buffer.rfind('@');
                    if let Some(at) = at_pos {
                        let before: String = self.buffer.chars().take(at).collect();
                        let after: String = self.buffer.chars().skip(at + 1).collect();
                        self.buffer = format!("{}{}", before, after).into();
                        self.cursor = at;
                    }
                    picker.deactivate();
                    true
                }
            }
            KeyCode::Tab => {
                if key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::SHIFT)
                {
                    picker.select_prev();
                } else {
                    picker.select_next();
                }
                true
            }
            KeyCode::Up => {
                picker.select_prev();
                true
            }
            KeyCode::Down => {
                picker.select_next();
                true
            }
            KeyCode::Enter => {
                if let Some(path) = picker.selected_path() {
                    let path_str = path.to_string_lossy().to_string();
                    let at_pos = self.buffer.rfind('@');
                    if let Some(at) = at_pos {
                        let before: String = self.buffer.chars().take(at).collect();
                        let after_offset = at + 1 + picker.query.len();
                        let after: String = self.buffer.chars().skip(after_offset).collect();
                        let new_len = before.len() + path_str.len();
                        self.buffer = format!("{}{}{}", before, path_str, after).into();
                        self.cursor = new_len;
                    }
                }
                picker.deactivate();
                true
            }
            KeyCode::Esc => {
                let at_pos = self.buffer.rfind('@');
                if let Some(at) = at_pos {
                    let before: String = self.buffer.chars().take(at).collect();
                    let after: String = self
                        .buffer
                        .chars()
                        .skip(at + 1 + picker.query.len())
                        .collect();
                    self.buffer = format!("{}{}", before, after).into();
                    self.cursor = at;
                }
                picker.deactivate();
                true
            }
            _ => false,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<CompactString> {
        match key.code {
            KeyCode::Enter => {
                // Don't submit if picker is active
                if self.picker.as_ref().is_some_and(|p| p.active) {
                    return None;
                }
                let text = self.buffer.clone();
                if !text.is_empty() {
                    self.history.push(text.clone());
                }
                self.history_pos = None;
                self.buffer.clear();
                self.cursor = 0;
                if text.is_empty() { None } else { Some(text) }
            }
            KeyCode::Char(c) => {
                if c == '@' {
                    let at_word_start =
                        self.cursor == 0 || self.buffer.chars().nth(self.cursor - 1) == Some(' ');
                    if at_word_start {
                        self.start_picker();
                    }
                }
                self.buffer.insert(self.cursor, c);
                self.cursor += 1;
                self.history_pos = None;
                None
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.buffer.remove(self.cursor);
                }
                None
            }
            KeyCode::Delete => {
                if self.cursor < self.buffer.len() {
                    self.buffer.remove(self.cursor);
                }
                None
            }
            KeyCode::Left => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                None
            }
            KeyCode::Right => {
                if self.cursor < self.buffer.len() {
                    self.cursor += 1;
                }
                None
            }
            KeyCode::Home => {
                self.cursor = 0;
                None
            }
            KeyCode::End => {
                self.cursor = self.buffer.len();
                None
            }
            KeyCode::Up => {
                let hist_len = self.history.len();
                if hist_len == 0 {
                    return None;
                }
                let pos = match self.history_pos {
                    Some(p) if p > 0 => p - 1,
                    Some(_) => 0,
                    None => hist_len - 1,
                };
                self.history_pos = Some(pos);
                self.buffer = self.history[pos].clone();
                self.cursor = self.buffer.len();
                None
            }
            KeyCode::Down => {
                match self.history_pos {
                    Some(pos) if pos + 1 < self.history.len() => {
                        let new_pos = pos + 1;
                        self.history_pos = Some(new_pos);
                        self.buffer = self.history[new_pos].clone();
                        self.cursor = self.buffer.len();
                    }
                    Some(_) => {
                        self.history_pos = None;
                        self.buffer.clear();
                        self.cursor = 0;
                    }
                    None => {}
                }
                None
            }
            KeyCode::Tab => {
                self.buffer.insert_str(self.cursor, "  ");
                self.cursor += 2;
                None
            }
            _ => None,
        }
    }
}

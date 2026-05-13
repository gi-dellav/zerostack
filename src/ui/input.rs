use compact_str::CompactString;
use crossterm::event::{KeyCode, KeyEvent};

pub struct InputEditor {
    pub buffer: CompactString,
    pub cursor: usize,
    history: Vec<CompactString>,
    history_pos: Option<usize>,
}

impl InputEditor {
    pub fn new() -> Self {
        InputEditor {
            buffer: CompactString::new(""),
            cursor: 0,
            history: Vec::new(),
            history_pos: None,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<CompactString> {
        match key.code {
            KeyCode::Enter => {
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

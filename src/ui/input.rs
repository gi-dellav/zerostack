use compact_str::CompactString;
use crossterm::event::{KeyCode, KeyEvent};

use crate::ui::picker::FilePicker;

// `cursor` er en byte-offset inn i `buffer` (som er UTF-8). Hjelperne under
// flytter cursoren med ett tegn (én char-grense) i hver retning slik at vi
// aldri lander midt i en multibyte-sekvens — det ville panic-et på neste
// insert/remove i `CompactString`/`String`.
fn prev_char_boundary(s: &str, idx: usize) -> usize {
    let mut i = idx.saturating_sub(1);
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn next_char_boundary(s: &str, idx: usize) -> usize {
    let len = s.len();
    let mut i = (idx + 1).min(len);
    while i < len && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

pub struct InputEditor {
    pub buffer: CompactString,
    pub cursor: usize,
    history: Vec<CompactString>,
    history_pos: Option<usize>,
    pub picker: Option<FilePicker>,
    monochrome: bool,
}

impl InputEditor {
    pub fn new() -> Self {
        InputEditor {
            buffer: CompactString::new(""),
            cursor: 0,
            history: Vec::new(),
            history_pos: None,
            picker: None,
            monochrome: false,
        }
    }

    pub fn set_monochrome(&mut self, monochrome: bool) {
        self.monochrome = monochrome;
        if let Some(picker) = self.picker.as_mut() {
            picker.set_monochrome(monochrome);
        }
    }

    pub fn start_picker(&mut self) {
        let picker = self.picker.get_or_insert_with(FilePicker::new);
        picker.set_monochrome(self.monochrome);
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
                self.cursor += c.len_utf8();
                true
            }
            KeyCode::Backspace => {
                if picker.cursor > 0 {
                    picker.backspace();
                    self.cursor = prev_char_boundary(&self.buffer, self.cursor);
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
                    // ' ' er ASCII (1 byte), så sjekken kan gjøres byte-vis.
                    let at_word_start = self.cursor == 0
                        || self.buffer.as_bytes().get(self.cursor - 1) == Some(&b' ');
                    if at_word_start {
                        self.start_picker();
                    }
                }
                self.buffer.insert(self.cursor, c);
                self.cursor += c.len_utf8();
                self.history_pos = None;
                None
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    self.cursor = prev_char_boundary(&self.buffer, self.cursor);
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
                    self.cursor = prev_char_boundary(&self.buffer, self.cursor);
                }
                None
            }
            KeyCode::Right => {
                if self.cursor < self.buffer.len() {
                    self.cursor = next_char_boundary(&self.buffer, self.cursor);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::empty())
    }

    fn type_str(editor: &mut InputEditor, s: &str) {
        for c in s.chars() {
            editor.handle_key(press(KeyCode::Char(c)));
        }
    }

    #[test]
    fn typing_ascii_keeps_cursor_in_sync() {
        let mut editor = InputEditor::new();
        type_str(&mut editor, "hello");
        assert_eq!(editor.buffer.as_str(), "hello");
        assert_eq!(editor.cursor, 5);
    }

    #[test]
    fn typing_multibyte_chars_does_not_panic() {
        // Regresjon for bug der `cursor += 1` (char-trinn) ble brukt mot
        // `CompactString::insert(byte_idx, ch)` (byte-grense påkrevd).
        // To norske bokstaver etter hverandre var nok til å trigge panic.
        let mut editor = InputEditor::new();
        type_str(&mut editor, "på "); // hadde panic-et på mellomrommet etter 'å'
        assert_eq!(editor.buffer.as_str(), "på ");
        assert_eq!(editor.cursor, editor.buffer.len()); // cursor i bytes
    }

    #[test]
    fn typing_mixed_ascii_and_multibyte() {
        let mut editor = InputEditor::new();
        type_str(&mut editor, "hei på deg så fin dag æøå");
        assert_eq!(editor.buffer.as_str(), "hei på deg så fin dag æøå");
        assert_eq!(editor.cursor, editor.buffer.len());
    }

    #[test]
    fn backspace_after_multibyte_does_not_panic() {
        let mut editor = InputEditor::new();
        type_str(&mut editor, "å");
        editor.handle_key(press(KeyCode::Backspace));
        assert_eq!(editor.buffer.as_str(), "");
        assert_eq!(editor.cursor, 0);
    }

    #[test]
    fn left_arrow_steps_one_char_not_one_byte() {
        let mut editor = InputEditor::new();
        type_str(&mut editor, "aåb");
        // cursor er bak 'b', byte-idx 4 (a=1 + å=2 + b=1)
        assert_eq!(editor.cursor, 4);
        editor.handle_key(press(KeyCode::Left));
        // bak 'å' → byte-idx 3
        assert_eq!(editor.cursor, 3);
        editor.handle_key(press(KeyCode::Left));
        // bak 'a' → byte-idx 1 (hopper over de 2 byte i 'å')
        assert_eq!(editor.cursor, 1);
    }

    #[test]
    fn right_arrow_steps_one_char_not_one_byte() {
        let mut editor = InputEditor::new();
        type_str(&mut editor, "aåb");
        editor.cursor = 0;
        editor.handle_key(press(KeyCode::Right));
        assert_eq!(editor.cursor, 1); // bak 'a'
        editor.handle_key(press(KeyCode::Right));
        assert_eq!(editor.cursor, 3); // bak 'å' (hoppet 2 byte)
    }

    #[test]
    fn enter_returns_buffer_and_resets() {
        let mut editor = InputEditor::new();
        type_str(&mut editor, "hei på");
        let out = editor.handle_key(press(KeyCode::Enter)).unwrap();
        assert_eq!(out.as_str(), "hei på");
        assert_eq!(editor.cursor, 0);
        assert_eq!(editor.buffer.as_str(), "");
    }
}

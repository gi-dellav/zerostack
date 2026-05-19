use compact_str::CompactString;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::ui::cmd_picker::{CommandPicker, PromptPicker};
use crate::ui::picker::FilePicker;

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

pub fn cursor_to_line_col(buffer: &str, cursor: usize) -> (usize, usize) {
    let mut line = 0usize;
    let mut col = 0usize;
    for (i, ch) in buffer.char_indices() {
        if i >= cursor {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (line, col)
}

pub enum Picker {
    File(FilePicker),
    Command(CommandPicker),
    Prompt(PromptPicker),
}

impl Picker {
    pub fn active(&self) -> bool {
        match self {
            Picker::File(p) => p.active,
            Picker::Command(p) => p.active,
            Picker::Prompt(p) => p.active,
        }
    }

    pub fn set_monochrome(&mut self, monochrome: bool) {
        match self {
            Picker::File(p) => p.set_monochrome(monochrome),
            Picker::Command(p) => p.set_monochrome(monochrome),
            Picker::Prompt(p) => p.set_monochrome(monochrome),
        }
    }

    pub fn draw(&self) -> std::io::Result<()> {
        match self {
            Picker::File(p) => p.draw(),
            Picker::Command(p) => p.draw(),
            Picker::Prompt(p) => p.draw(),
        }
    }
}

pub struct InputEditor {
    pub buffer: CompactString,
    pub cursor: usize,
    history: Vec<CompactString>,
    history_pos: Option<usize>,
    pub picker: Option<Picker>,
    monochrome: bool,
    prompt_names: Vec<String>,
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
            prompt_names: Vec::new(),
        }
    }

    pub fn set_monochrome(&mut self, monochrome: bool) {
        self.monochrome = monochrome;
        if let Some(ref mut picker) = self.picker {
            picker.set_monochrome(monochrome);
        }
    }

    pub fn set_prompt_names(&mut self, names: Vec<String>) {
        self.prompt_names = names;
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
        picker.activate();
        self.picker = Some(Picker::File(picker));
    }

    pub fn start_command_picker(&mut self) {
        let mut picker = CommandPicker::new();
        picker.set_monochrome(self.monochrome);
        picker.activate();
        self.picker = Some(Picker::Command(picker));
    }

    pub fn start_prompt_picker(&mut self) {
        let mut picker = PromptPicker::new();
        picker.set_monochrome(self.monochrome);
        if !self.prompt_names.is_empty() {
            picker.set_items(self.prompt_names.clone());
        }
        picker.activate();
        self.picker = Some(Picker::Prompt(picker));
    }

    pub fn handle_picker_key(&mut self, key: KeyEvent) -> bool {
        match self.picker.as_mut() {
            Some(Picker::File(p)) => {
                handle_file_picker_key(&mut self.buffer, &mut self.cursor, p, key)
            }
            Some(Picker::Command(p)) => {
                let (handled, replacement) = handle_command_picker_key(
                    &mut self.buffer,
                    &mut self.cursor,
                    &self.prompt_names,
                    p,
                    key,
                );
                if let Some(new_picker) = replacement {
                    self.picker = Some(new_picker);
                }
                handled
            }
            Some(Picker::Prompt(p)) => {
                handle_prompt_picker_key(&mut self.buffer, &mut self.cursor, p, key)
            }
            _ => false,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<CompactString> {
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
                if !text.is_empty() {
                    self.history.push(text.clone());
                }
                self.history_pos = None;
                self.buffer.clear();
                self.cursor = 0;
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
                        || self.buffer.as_bytes().get(self.cursor - 1) == Some(&b' ');
                    if at_word_start {
                        self.start_file_picker();
                    }
                }
                if c == '/' && self.cursor == 0 {
                    self.start_command_picker();
                }
                self.buffer.insert(self.cursor, c);
                self.cursor += c.len_utf8();
                self.history_pos = None;

                // Check if we should activate prompt picker after typing /prompt
                if (self.picker.is_none() || !self.picker.as_ref().is_some_and(|p| p.active()))
                    && self.buffer.starts_with("/prompt ")
                {
                    let after_prefix: String = self.buffer.chars().skip("/prompt ".len()).collect();
                    if !after_prefix.is_empty() && c != ' ' {
                        let query_len = after_prefix.len();
                        if query_len == 1 {
                            self.start_prompt_picker();
                            if let Some(Picker::Prompt(ref mut pp)) = self.picker {
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

fn handle_file_picker_key(
    buffer: &mut CompactString,
    cursor: &mut usize,
    picker: &mut FilePicker,
    key: KeyEvent,
) -> bool {
    match key.code {
        KeyCode::Char(c)
            if c == '\x08' || (c == 'h' && key.modifiers.contains(KeyModifiers::CONTROL)) =>
        {
            if picker.cursor > 0 {
                picker.backspace();
                *cursor = prev_char_boundary(buffer, *cursor);
                buffer.remove(*cursor);
            } else {
                let at_pos = buffer.rfind('@');
                if let Some(at) = at_pos {
                    let before: String = buffer.chars().take(at).collect();
                    let after: String = buffer.chars().skip(at + 1).collect();
                    *buffer = format!("{}{}", before, after).into();
                    *cursor = at;
                }
                picker.deactivate();
            }
            true
        }
        KeyCode::Char(c) => {
            picker.char_input(c);
            buffer.insert(*cursor, c);
            *cursor += c.len_utf8();
            true
        }
        KeyCode::Backspace => {
            if picker.cursor > 0 {
                picker.backspace();
                *cursor = prev_char_boundary(buffer, *cursor);
                buffer.remove(*cursor);
                true
            } else {
                let at_pos = buffer.rfind('@');
                if let Some(at) = at_pos {
                    let before: String = buffer.chars().take(at).collect();
                    let after: String = buffer.chars().skip(at + 1).collect();
                    *buffer = format!("{}{}", before, after).into();
                    *cursor = at;
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
                let at_pos = buffer.rfind('@');
                if let Some(at) = at_pos {
                    let before: String = buffer.chars().take(at).collect();
                    let after_offset = at + 1 + picker.query.len();
                    let after: String = buffer.chars().skip(after_offset).collect();
                    let new_len = before.len() + path_str.len();
                    *buffer = format!("{}{}{}", before, path_str, after).into();
                    *cursor = new_len;
                }
            }
            picker.deactivate();
            true
        }
        KeyCode::Esc => {
            let at_pos = buffer.rfind('@');
            if let Some(at) = at_pos {
                let before: String = buffer.chars().take(at).collect();
                let after: String = buffer.chars().skip(at + 1 + picker.query.len()).collect();
                *buffer = format!("{}{}", before, after).into();
                *cursor = at;
            }
            picker.deactivate();
            true
        }
        _ => false,
    }
}

fn handle_command_picker_key(
    buffer: &mut CompactString,
    cursor: &mut usize,
    prompt_names: &[String],
    picker: &mut CommandPicker,
    key: KeyEvent,
) -> (bool, Option<Picker>) {
    match key.code {
        KeyCode::Char(c)
            if c == '\x08' || (c == 'h' && key.modifiers.contains(KeyModifiers::CONTROL)) =>
        {
            if picker.cursor > 0 {
                picker.backspace();
                *cursor = prev_char_boundary(buffer, *cursor);
                buffer.remove(*cursor);
            } else {
                if buffer.starts_with('/') {
                    let after: String = buffer.chars().skip(1 + picker.query.len()).collect();
                    *buffer = format!("/{}", after).into();
                    *cursor = 1;
                }
                picker.deactivate();
            }
            (true, None)
        }
        KeyCode::Char(c) => {
            picker.char_input(c);
            let pos = 1 + picker.cursor.saturating_sub(1);
            buffer.insert(pos, c);
            *cursor += c.len_utf8();
            (true, None)
        }
        KeyCode::Backspace => {
            if picker.cursor > 0 {
                picker.backspace();
                let remove_pos = 1 + picker.cursor;
                if remove_pos <= buffer.len() {
                    buffer.remove(remove_pos);
                }
                *cursor = prev_char_boundary(buffer, *cursor);
                (true, None)
            } else {
                if buffer.starts_with('/') {
                    let after: String = buffer.chars().skip(1 + picker.query.len()).collect();
                    *buffer = format!("/{}", after).into();
                    *cursor = 1;
                }
                picker.deactivate();
                (true, None)
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
            (true, None)
        }
        KeyCode::Up => {
            picker.select_prev();
            (true, None)
        }
        KeyCode::Down => {
            picker.select_next();
            (true, None)
        }
        KeyCode::Enter => {
            if let Some(cmd) = picker.selected_command() {
                let selected = cmd.to_string();
                let slash_pos = buffer.find('/').unwrap_or(0);
                let before: String = buffer.chars().take(slash_pos).collect();
                let after_offset = slash_pos + 1 + picker.query.len();
                let after: String = buffer.chars().skip(after_offset).collect();
                let insertion = if after.is_empty() || after.starts_with(' ') {
                    format!("{} ", selected)
                } else {
                    format!("{}{}", selected, after)
                };
                *buffer = format!("{}{}", before, insertion).into();
                *cursor = before.len() + selected.len() + 1;

                if selected == "/prompt" && !prompt_names.is_empty() {
                    picker.deactivate();
                    let mut pp = PromptPicker::new();
                    pp.set_items(prompt_names.to_vec());
                    pp.activate();
                    return (true, Some(Picker::Prompt(pp)));
                }
            }
            picker.deactivate();
            (true, None)
        }
        KeyCode::Esc => {
            let slash_pos = buffer.find('/').unwrap_or(0);
            let before: String = buffer.chars().take(slash_pos).collect();
            let after: String = buffer
                .chars()
                .skip(slash_pos + 1 + picker.query.len())
                .collect();
            *buffer = format!("{}/{}", before, after).into();
            *cursor = slash_pos + 1;
            picker.deactivate();
            (true, None)
        }
        _ => (false, None),
    }
}

fn handle_prompt_picker_key(
    buffer: &mut CompactString,
    cursor: &mut usize,
    picker: &mut PromptPicker,
    key: KeyEvent,
) -> bool {
    match key.code {
        KeyCode::Char(c)
            if c == '\x08' || (c == 'h' && key.modifiers.contains(KeyModifiers::CONTROL)) =>
        {
            if picker.cursor > 0 {
                picker.backspace();
                *cursor = prev_char_boundary(buffer, *cursor);
                buffer.remove(*cursor);
            } else {
                let prompt_prefix = "/prompt ";
                let prefix_len = prompt_prefix.len();
                let after_offset = prefix_len + picker.query.len();
                if buffer.len() >= after_offset {
                    let before: String = buffer.chars().take(prefix_len).collect();
                    let after: String = buffer.chars().skip(after_offset).collect();
                    *buffer = format!("{}{}", before, after).into();
                    *cursor = prefix_len;
                }
                picker.deactivate();
            }
            true
        }
        KeyCode::Char(c) => {
            picker.char_input(c);
            let insert_pos = "/prompt ".len() + picker.cursor.saturating_sub(1);
            buffer.insert(insert_pos, c);
            *cursor += c.len_utf8();
            true
        }
        KeyCode::Backspace => {
            if picker.cursor > 0 {
                picker.backspace();
                let prompt_prefix = "/prompt ";
                let prefix_len = prompt_prefix.len();
                let remove_pos = prefix_len + picker.cursor;
                if remove_pos < buffer.len() {
                    buffer.remove(remove_pos);
                }
                *cursor = prev_char_boundary(buffer, *cursor);
                true
            } else {
                let prompt_prefix = "/prompt ";
                let prefix_len = prompt_prefix.len();
                let after_offset = prefix_len + picker.query.len();
                if buffer.len() >= after_offset {
                    let before: String = buffer.chars().take(prefix_len).collect();
                    let after: String = buffer.chars().skip(after_offset).collect();
                    *buffer = format!("{}{}", before, after).into();
                    *cursor = prefix_len;
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
            if let Some(name) = picker.selected_name() {
                let prompt_prefix = "/prompt ";
                let prefix_len = prompt_prefix.len();
                let after_offset = prefix_len + picker.query.len();
                let before: String = buffer.chars().take(prefix_len).collect();
                let after: String = buffer.chars().skip(after_offset).collect();
                *buffer = format!("{}{}{}", before, name, after).into();
                *cursor = prefix_len + name.len();
            }
            picker.deactivate();
            true
        }
        KeyCode::Esc => {
            let prompt_prefix = "/prompt ";
            let prefix_len = prompt_prefix.len();
            let after_offset = prefix_len + picker.query.len();
            if buffer.len() >= after_offset {
                let before: String = buffer.chars().take(prefix_len).collect();
                let after: String = buffer.chars().skip(after_offset).collect();
                *buffer = format!("{}{}", before, after).into();
                *cursor = prefix_len;
            }
            picker.deactivate();
            true
        }
        _ => false,
    }
}

use compact_str::CompactString;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::ui::cmd_picker::{CommandPicker, ModelsPicker, PromptPicker, ThemePicker};
use crate::ui::input::cursor::prev_char_boundary;
use crate::ui::picker::FilePicker;

pub enum Picker {
    File(FilePicker),
    Command(CommandPicker),
    Prompt(PromptPicker),
    Models(ModelsPicker),
    Theme(ThemePicker),
}

impl Picker {
    pub fn active(&self) -> bool {
        match self {
            Picker::File(p) => p.active,
            Picker::Command(p) => p.active,
            Picker::Prompt(p) => p.active,
            Picker::Models(p) => p.active,
            Picker::Theme(p) => p.active,
        }
    }

    pub fn set_monochrome(&mut self, monochrome: bool) {
        match self {
            Picker::File(p) => p.set_monochrome(monochrome),
            Picker::Command(p) => p.set_monochrome(monochrome),
            Picker::Prompt(p) => p.set_monochrome(monochrome),
            Picker::Models(p) => p.set_monochrome(monochrome),
            Picker::Theme(p) => p.set_monochrome(monochrome),
        }
    }

    pub fn draw(&self) -> std::io::Result<()> {
        match self {
            Picker::File(p) => p.draw(),
            Picker::Command(p) => p.draw(),
            Picker::Prompt(p) => p.draw(),
            Picker::Models(p) => p.draw(),
            Picker::Theme(p) => p.draw(),
        }
    }
}

pub fn handle_file_picker_key(
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

pub fn handle_command_picker_key(
    buffer: &mut CompactString,
    cursor: &mut usize,
    prompt_names: &[String],
    theme_names: &[String],
    quick_model_names: &[String],
    picker: &mut CommandPicker,
    key: KeyEvent,
) -> (bool, Option<Picker>) {
    match key.code {
        KeyCode::Char(c)
            if c == '\x08' || (c == 'h' && key.modifiers.contains(KeyModifiers::CONTROL)) =>
        {
            if picker.cursor > 0 {
                picker.backspace();
                let byte_in_query = picker
                    .query
                    .char_indices()
                    .nth(picker.cursor)
                    .map(|(i, _)| i)
                    .unwrap_or(picker.query.len());
                let remove_pos = 1 + byte_in_query;
                if remove_pos < buffer.len() {
                    buffer.remove(remove_pos);
                }
                *cursor = prev_char_boundary(buffer, *cursor);
            } else {
                if buffer.starts_with('/') {
                    let after: String = buffer
                        .chars()
                        .skip(1 + picker.query.chars().count())
                        .collect();
                    *buffer = format!("/{}", after).into();
                    *cursor = 1;
                }
                picker.deactivate();
            }
            (true, None)
        }
        KeyCode::Char(c) => {
            picker.char_input(c);
            let byte_in_query = picker
                .query
                .char_indices()
                .nth(picker.cursor.saturating_sub(1))
                .map(|(i, _)| i)
                .unwrap_or(picker.query.len());
            let pos = 1 + byte_in_query;
            buffer.insert(pos, c);
            *cursor += c.len_utf8();
            (true, None)
        }
        KeyCode::Backspace => {
            if picker.cursor > 0 {
                picker.backspace();
                let byte_in_query = picker
                    .query
                    .char_indices()
                    .nth(picker.cursor)
                    .map(|(i, _)| i)
                    .unwrap_or(picker.query.len());
                let remove_pos = 1 + byte_in_query;
                if remove_pos < buffer.len() {
                    buffer.remove(remove_pos);
                }
                *cursor = prev_char_boundary(buffer, *cursor);
                (true, None)
            } else {
                if buffer.starts_with('/') {
                    let after: String = buffer
                        .chars()
                        .skip(1 + picker.query.chars().count())
                        .collect();
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
                let after_offset = slash_pos + 1 + picker.query.chars().count();
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
                if selected == "/models" && !quick_model_names.is_empty() {
                    picker.deactivate();
                    let mut mp = ModelsPicker::new();
                    mp.set_items(quick_model_names.to_vec());
                    mp.activate();
                    return (true, Some(Picker::Models(mp)));
                }
                if selected == "/theme" && !theme_names.is_empty() {
                    picker.deactivate();
                    let mut tp = ThemePicker::new();
                    tp.set_items(theme_names.to_vec());
                    tp.activate();
                    return (true, Some(Picker::Theme(tp)));
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
                .skip(slash_pos + 1 + picker.query.chars().count())
                .collect();
            *buffer = format!("{}/{}", before, after).into();
            *cursor = slash_pos + 1;
            picker.deactivate();
            (true, None)
        }
        _ => (false, None),
    }
}

pub fn handle_models_picker_key(
    buffer: &mut CompactString,
    cursor: &mut usize,
    picker: &mut ModelsPicker,
    key: KeyEvent,
) -> bool {
    let prefix = "/models ";
    let prefix_len = prefix.len();
    match key.code {
        KeyCode::Char(c)
            if c == '\x08' || (c == 'h' && key.modifiers.contains(KeyModifiers::CONTROL)) =>
        {
            if picker.cursor > 0 {
                picker.backspace();
                let byte_in_query = picker
                    .query
                    .char_indices()
                    .nth(picker.cursor)
                    .map(|(i, _)| i)
                    .unwrap_or(picker.query.len());
                let remove_pos = prefix_len + byte_in_query;
                if remove_pos < buffer.len() {
                    buffer.remove(remove_pos);
                }
                *cursor = prev_char_boundary(buffer, *cursor);
            } else {
                let after_offset = prefix_len + picker.query.chars().count();
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
            let byte_in_query = picker
                .query
                .char_indices()
                .nth(picker.cursor.saturating_sub(1))
                .map(|(i, _)| i)
                .unwrap_or(picker.query.len());
            let insert_pos = prefix_len + byte_in_query;
            buffer.insert(insert_pos, c);
            *cursor += c.len_utf8();
            true
        }
        KeyCode::Backspace => {
            if picker.cursor > 0 {
                picker.backspace();
                let byte_in_query = picker
                    .query
                    .char_indices()
                    .nth(picker.cursor)
                    .map(|(i, _)| i)
                    .unwrap_or(picker.query.len());
                let remove_pos = prefix_len + byte_in_query;
                if remove_pos < buffer.len() {
                    buffer.remove(remove_pos);
                }
                *cursor = prev_char_boundary(buffer, *cursor);
                true
            } else {
                let after_offset = prefix_len + picker.query.chars().count();
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
                let after_offset = prefix_len + picker.query.chars().count();
                let before: String = buffer.chars().take(prefix_len).collect();
                let after: String = buffer.chars().skip(after_offset).collect();
                *buffer = format!("{}{}{}", before, name, after).into();
                *cursor = prefix_len + name.len();
            }
            picker.deactivate();
            true
        }
        KeyCode::Esc => {
            let after_offset = prefix_len + picker.query.chars().count();
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

pub fn handle_theme_picker_key(
    buffer: &mut CompactString,
    cursor: &mut usize,
    picker: &mut ThemePicker,
    key: KeyEvent,
) -> bool {
    let prefix = "/theme ";
    let prefix_len = prefix.len();
    match key.code {
        KeyCode::Char(c)
            if c == '\x08' || (c == 'h' && key.modifiers.contains(KeyModifiers::CONTROL)) =>
        {
            if picker.cursor > 0 {
                picker.backspace();
                let byte_in_query = picker
                    .query
                    .char_indices()
                    .nth(picker.cursor)
                    .map(|(i, _)| i)
                    .unwrap_or(picker.query.len());
                let remove_pos = prefix_len + byte_in_query;
                if remove_pos < buffer.len() {
                    buffer.remove(remove_pos);
                }
                *cursor = prev_char_boundary(buffer, *cursor);
            } else {
                let after_offset = prefix_len + picker.query.chars().count();
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
            let byte_in_query = picker
                .query
                .char_indices()
                .nth(picker.cursor.saturating_sub(1))
                .map(|(i, _)| i)
                .unwrap_or(picker.query.len());
            let insert_pos = prefix_len + byte_in_query;
            buffer.insert(insert_pos, c);
            *cursor += c.len_utf8();
            true
        }
        KeyCode::Backspace => {
            if picker.cursor > 0 {
                picker.backspace();
                let byte_in_query = picker
                    .query
                    .char_indices()
                    .nth(picker.cursor)
                    .map(|(i, _)| i)
                    .unwrap_or(picker.query.len());
                let remove_pos = prefix_len + byte_in_query;
                if remove_pos < buffer.len() {
                    buffer.remove(remove_pos);
                }
                *cursor = prev_char_boundary(buffer, *cursor);
                true
            } else {
                let after_offset = prefix_len + picker.query.chars().count();
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
                let after_offset = prefix_len + picker.query.chars().count();
                let before: String = buffer.chars().take(prefix_len).collect();
                let after: String = buffer.chars().skip(after_offset).collect();
                *buffer = format!("{}{}{}", before, name, after).into();
                *cursor = prefix_len + name.len();
            }
            picker.deactivate();
            true
        }
        KeyCode::Esc => {
            let after_offset = prefix_len + picker.query.chars().count();
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

pub fn handle_prompt_picker_key(
    buffer: &mut CompactString,
    cursor: &mut usize,
    picker: &mut PromptPicker,
    key: KeyEvent,
) -> bool {
    let prefix = "/prompt ";
    let prefix_len = prefix.len();
    match key.code {
        KeyCode::Char(c)
            if c == '\x08' || (c == 'h' && key.modifiers.contains(KeyModifiers::CONTROL)) =>
        {
            if picker.cursor > 0 {
                picker.backspace();
                let byte_in_query = picker
                    .query
                    .char_indices()
                    .nth(picker.cursor)
                    .map(|(i, _)| i)
                    .unwrap_or(picker.query.len());
                let remove_pos = prefix_len + byte_in_query;
                if remove_pos < buffer.len() {
                    buffer.remove(remove_pos);
                }
                *cursor = prev_char_boundary(buffer, *cursor);
            } else {
                let after_offset = prefix_len + picker.query.chars().count();
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
            let byte_in_query = picker
                .query
                .char_indices()
                .nth(picker.cursor.saturating_sub(1))
                .map(|(i, _)| i)
                .unwrap_or(picker.query.len());
            let insert_pos = prefix_len + byte_in_query;
            buffer.insert(insert_pos, c);
            *cursor += c.len_utf8();
            true
        }
        KeyCode::Backspace => {
            if picker.cursor > 0 {
                picker.backspace();
                let byte_in_query = picker
                    .query
                    .char_indices()
                    .nth(picker.cursor)
                    .map(|(i, _)| i)
                    .unwrap_or(picker.query.len());
                let remove_pos = prefix_len + byte_in_query;
                if remove_pos < buffer.len() {
                    buffer.remove(remove_pos);
                }
                *cursor = prev_char_boundary(buffer, *cursor);
                true
            } else {
                let after_offset = prefix_len + picker.query.chars().count();
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
                let after_offset = prefix_len + picker.query.chars().count();
                let before: String = buffer.chars().take(prefix_len).collect();
                let after: String = buffer.chars().skip(after_offset).collect();
                *buffer = format!("{}{}{}", before, name, after).into();
                *cursor = prefix_len + name.len();
            }
            picker.deactivate();
            true
        }
        KeyCode::Esc => {
            let after_offset = prefix_len + picker.query.chars().count();
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

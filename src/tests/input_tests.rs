use crate::ui::input::InputEditor;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::empty())
}

fn press_mod(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, mods)
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
    // Regression for bug where `cursor += 1` (char step) was used with
    // `CompactString::insert(byte_idx, ch)` (byte boundary required).
    // Two Norwegian characters in a row were enough to trigger a panic.
    let mut editor = InputEditor::new();
    type_str(&mut editor, "på "); // used to panic on the space after 'å'
    assert_eq!(editor.buffer.as_str(), "på ");
    assert_eq!(editor.cursor, editor.buffer.len()); // cursor in bytes
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
    // cursor is after 'b', byte-idx 4 (a=1 + å=2 + b=1)
    assert_eq!(editor.cursor, 4);
    editor.handle_key(press(KeyCode::Left));
    // after 'å' → byte-idx 3
    assert_eq!(editor.cursor, 3);
    editor.handle_key(press(KeyCode::Left));
    // after 'a' → byte-idx 1 (skips the 2 bytes of 'å')
    assert_eq!(editor.cursor, 1);
}

#[test]
fn right_arrow_steps_one_char_not_one_byte() {
    let mut editor = InputEditor::new();
    type_str(&mut editor, "aåb");
    editor.cursor = 0;
    editor.handle_key(press(KeyCode::Right));
    assert_eq!(editor.cursor, 1); // after 'a'
    editor.handle_key(press(KeyCode::Right));
    assert_eq!(editor.cursor, 3); // after 'å' (skipped 2 bytes)
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

#[test]
fn alt_left_jumps_to_previous_word_start() {
    let mut editor = InputEditor::new();
    type_str(&mut editor, "foo bar baz");
    editor.handle_key(press_mod(KeyCode::Left, KeyModifiers::ALT));
    assert_eq!(editor.cursor, 8); // start of "baz"
    editor.handle_key(press_mod(KeyCode::Left, KeyModifiers::ALT));
    assert_eq!(editor.cursor, 4); // start of "bar"
}

#[test]
fn alt_right_jumps_to_next_word_end() {
    let mut editor = InputEditor::new();
    type_str(&mut editor, "foo bar baz");
    editor.cursor = 0;
    editor.handle_key(press_mod(KeyCode::Right, KeyModifiers::ALT));
    assert_eq!(editor.cursor, 3); // end of "foo"
    editor.handle_key(press_mod(KeyCode::Right, KeyModifiers::ALT));
    assert_eq!(editor.cursor, 7); // end of "bar"
}

#[test]
fn ctrl_arrows_jump_words_like_alt_arrows() {
    let mut editor = InputEditor::new();
    type_str(&mut editor, "foo bar");
    editor.handle_key(press_mod(KeyCode::Left, KeyModifiers::CONTROL));
    assert_eq!(editor.cursor, 4); // start of "bar"
    editor.handle_key(press_mod(KeyCode::Right, KeyModifiers::CONTROL));
    assert_eq!(editor.cursor, 7); // end of "bar"
}

#[test]
fn shift_alt_left_still_jumps_words() {
    let mut editor = InputEditor::new();
    type_str(&mut editor, "foo bar");
    editor.handle_key(press_mod(
        KeyCode::Left,
        KeyModifiers::SHIFT | KeyModifiers::ALT,
    ));
    assert_eq!(editor.cursor, 4);
}

#[test]
fn shift_arrows_alone_move_one_char() {
    let mut editor = InputEditor::new();
    type_str(&mut editor, "foo bar");
    editor.handle_key(press_mod(KeyCode::Left, KeyModifiers::SHIFT));
    assert_eq!(editor.cursor, 6); // one char back, not a word jump
    editor.handle_key(press_mod(KeyCode::Right, KeyModifiers::SHIFT));
    assert_eq!(editor.cursor, 7);
}

#[test]
fn plain_arrows_move_one_char() {
    let mut editor = InputEditor::new();
    type_str(&mut editor, "foo bar");
    editor.handle_key(press(KeyCode::Left));
    assert_eq!(editor.cursor, 6);
    editor.handle_key(press(KeyCode::Right));
    assert_eq!(editor.cursor, 7);
}

#[test]
fn ctrl_backspace_deletes_previous_word_into_kill_ring() {
    let mut editor = InputEditor::new();
    type_str(&mut editor, "foo bar");
    editor.handle_key(press_mod(KeyCode::Backspace, KeyModifiers::CONTROL));
    assert_eq!(editor.buffer.as_str(), "foo ");
    assert_eq!(editor.cursor, 4);
    // The deleted word went to the kill ring: Ctrl+Y yanks it back.
    editor.handle_key(press_mod(KeyCode::Char('y'), KeyModifiers::CONTROL));
    assert_eq!(editor.buffer.as_str(), "foo bar");
    assert_eq!(editor.cursor, 7);
}

#[test]
fn ctrl_backspace_at_start_is_noop() {
    let mut editor = InputEditor::new();
    type_str(&mut editor, "foo");
    editor.cursor = 0;
    editor.handle_key(press_mod(KeyCode::Backspace, KeyModifiers::CONTROL));
    assert_eq!(editor.buffer.as_str(), "foo");
    assert_eq!(editor.cursor, 0);
}

#[test]
fn plain_backspace_still_deletes_one_char() {
    let mut editor = InputEditor::new();
    type_str(&mut editor, "foo bar");
    editor.handle_key(press(KeyCode::Backspace));
    assert_eq!(editor.buffer.as_str(), "foo ba");
}

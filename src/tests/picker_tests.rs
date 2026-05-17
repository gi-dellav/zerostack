use crate::ui::picker::FilePicker;
use std::path::PathBuf;

#[test]
fn test_backspace_empty_query() {
    let mut picker = FilePicker::new();
    picker.test_set_cache(vec![PathBuf::from("test.rs")]);
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

    picker.backspace();
    assert_eq!(picker.query, "");
    assert_eq!(picker.cursor, 0);
}

#[test]
fn test_char_input_and_backspace_unicode() {
    let mut picker = FilePicker::new();
    picker.test_set_cache(vec![PathBuf::from("test.rs")]);

    picker.char_input('é');
    assert_eq!(picker.query, "é");
    assert_eq!(picker.cursor, 1);

    picker.char_input('ñ');
    assert_eq!(picker.query, "éñ");
    assert_eq!(picker.cursor, 2);

    picker.backspace();
    assert_eq!(picker.query, "é");
    assert_eq!(picker.cursor, 1);

    picker.backspace();
    assert_eq!(picker.query, "");
    assert_eq!(picker.cursor, 0);

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

    picker.char_input('a');
    picker.char_input('b');
    assert_eq!(picker.query, "ab");
    assert_eq!(picker.cursor, 2);

    picker.backspace();
    assert_eq!(picker.query, "a");
    assert_eq!(picker.cursor, 1);

    picker.char_input('é');
    assert_eq!(picker.query, "aé");
    assert_eq!(picker.cursor, 2);

    picker.char_input('c');
    assert_eq!(picker.query, "aéc");
    assert_eq!(picker.cursor, 3);

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

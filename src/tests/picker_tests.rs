use crate::ui::pickers::file::{FilePicker, walk_files};
use crate::ui::pickers::list::ListPicker;
use crate::ui::pickers::models::ModelsPicker;
use std::path::PathBuf;

#[test]
fn test_models_picker_starts_on_quick_group() {
    let mut picker = ModelsPicker::new();
    picker.set_groups(
        vec!["fast".to_string()],
        vec!["claude-opus-4-7".to_string()],
    );
    picker.activate();
    assert_eq!(picker.matches, vec!["fast".to_string()]);
}

#[test]
fn test_models_picker_tab_toggles_to_provider_group() {
    let mut picker = ModelsPicker::new();
    picker.set_groups(
        vec!["fast".to_string()],
        vec!["claude-opus-4-7".to_string()],
    );
    picker.activate();
    picker.toggle_group();
    assert_eq!(picker.matches, vec!["claude-opus-4-7".to_string()]);
}

#[test]
fn test_models_picker_starts_on_provider_when_quick_empty() {
    let mut picker = ModelsPicker::new();
    picker.set_groups(Vec::new(), vec!["claude-opus-4-7".to_string()]);
    picker.activate();
    assert_eq!(picker.matches, vec!["claude-opus-4-7".to_string()]);
}

#[test]
fn test_models_picker_fuzzy_subsequence_match() {
    let mut picker = ModelsPicker::new();
    picker.set_groups(
        Vec::new(),
        vec!["claude-opus-4-7".to_string(), "gpt-4o-mini".to_string()],
    );
    picker.activate();
    for c in "o47".chars() {
        picker.char_input(c);
    }
    assert_eq!(picker.selected_name(), Some("claude-opus-4-7"));
    assert!(!picker.matches.iter().any(|m| m == "gpt-4o-mini"));
}

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

// ── ListPicker tests ───────────────────────────────────────────────

#[test]
fn test_list_picker_filter() {
    let mut picker = ListPicker::new();
    picker.set_items(vec![
        "alpha".to_string(),
        "beta".to_string(),
        "gamma".to_string(),
    ]);
    picker.activate();
    assert_eq!(picker.matches.len(), 3);

    picker.char_input('a');
    assert_eq!(picker.matches, vec!["alpha", "beta", "gamma"]);

    picker.char_input('l');
    assert_eq!(picker.matches, vec!["alpha"]);
}

#[test]
fn test_list_picker_navigation() {
    let mut picker = ListPicker::new();
    picker.set_items(vec!["a".to_string(), "b".to_string(), "c".to_string()]);
    picker.activate();
    assert_eq!(picker.selected, 0);

    picker.select_next();
    assert_eq!(picker.selected, 1);

    picker.select_prev();
    assert_eq!(picker.selected, 0);

    picker.select_prev();
    assert_eq!(picker.selected, 2);
}

#[test]
fn test_list_picker_backspace_and_char_unicode() {
    let mut picker = ListPicker::new();
    picker.set_items(vec!["test".to_string()]);

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

    picker.backspace();
    assert_eq!(picker.query, "");
    assert_eq!(picker.cursor, 0);
}

#[test]
fn test_list_picker_reactivate_resets_state() {
    let mut picker = ListPicker::new();
    picker.set_items(vec!["a".to_string(), "b".to_string()]);
    picker.char_input('a');
    picker.char_input('b');
    assert_eq!(picker.query, "ab");

    picker.deactivate();
    assert!(!picker.active);

    picker.activate();
    assert!(picker.active);
    assert_eq!(picker.query, "");
    assert_eq!(picker.cursor, 0);
    assert_eq!(picker.selected, 0);
}

#[test]
fn test_static_commands_prepopulated() {
    let mut picker = ListPicker::with_static_commands();
    picker.activate();
    assert!(picker.matches.len() > 5);

    picker.char_input('m');
    picker.char_input('o');
    picker.char_input('d');
    assert!(picker.matches.contains(&"/model".to_string()));
}

#[test]
fn test_list_picker_annotations_not_in_filter_or_selection() {
    let mut picker = ListPicker::new();
    picker.set_items(vec!["alpha".to_string(), "beta".to_string()]);
    picker.set_annotations(std::collections::HashMap::from([
        ("alpha".to_string(), "built-in".to_string()),
        ("beta".to_string(), ".zerostack".to_string()),
    ]));
    picker.activate();

    // Selection returns the bare name, never the decorated display string.
    assert_eq!(picker.selected_name(), Some("alpha"));

    // Filtering matches item names only: the annotation text is not searched.
    picker.char_input('z');
    assert!(picker.matches.is_empty());

    picker.backspace();
    picker.char_input('a');
    assert_eq!(picker.matches, vec!["alpha", "beta"]);
}

// ── walk_files tests ────────────────────────────────────────────────

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn with_temp_dir<F>(f: F)
where
    F: FnOnce(&Path),
{
    let n = TEMP_COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("zerostack_test_{}_{}", std::process::id(), n));
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

#[test]
fn test_walk_files_skips_dotfiles() {
    with_temp_dir(|root| {
        fs::write(root.join(".hidden"), b"secret").unwrap();
        fs::write(root.join("visible.txt"), b"hello").unwrap();

        let files = walk_files(&root.to_string_lossy());
        let names: Vec<&str> = files.iter().map(|p| p.to_str().unwrap()).collect();

        assert!(!names.contains(&".hidden"));
        assert!(names.contains(&"visible.txt"));
    });
}

#[test]
fn test_walk_files_skips_files_in_dot_dirs() {
    with_temp_dir(|root| {
        fs::create_dir_all(root.join(".secret").join("nested")).unwrap();
        fs::write(
            root.join(".secret").join("nested").join("file.txt"),
            b"hidden",
        )
        .unwrap();
        fs::write(root.join(".secret").join("secret_file.txt"), b"hidden").unwrap();
        fs::write(root.join("public.txt"), b"visible").unwrap();

        let files = walk_files(&root.to_string_lossy());
        let names: Vec<&str> = files.iter().map(|p| p.to_str().unwrap()).collect();

        assert!(!names.contains(&".secret"));
        assert!(!names.contains(&".secret/nested"));
        assert!(!names.contains(&".secret/nested/file.txt"));
        assert!(!names.contains(&".secret/secret_file.txt"));
        assert!(names.contains(&"public.txt"));
    });
}

#[test]
fn test_walk_files_root_is_sorted_and_stripped() {
    with_temp_dir(|root| {
        fs::write(root.join("z.txt"), b"z").unwrap();
        fs::write(root.join("c.txt"), b"c").unwrap();
        fs::write(root.join("a.txt"), b"a").unwrap();

        let files = walk_files(&root.to_string_lossy());
        let names: Vec<&str> = files.iter().map(|p| p.to_str().unwrap()).collect();

        let root_idx = names.iter().position(|n| n.is_empty());
        assert!(
            root_idx.is_some(),
            "root entry (empty string) should be present"
        );

        let file_indices: Vec<usize> = names
            .iter()
            .enumerate()
            .filter(|(_, n)| n.ends_with(".txt"))
            .map(|(i, _)| i)
            .collect();
        assert!(
            file_indices.windows(2).all(|w| w[0] < w[1]),
            "files should be sorted"
        );
    });
}

#[test]
fn test_walk_files_empty_directory() {
    with_temp_dir(|root| {
        let files = walk_files(&root.to_string_lossy());
        let names: Vec<&str> = files.iter().map(|p| p.to_str().unwrap()).collect();

        assert_eq!(names.len(), 1, "only root entry expected in empty dir");
        assert!(names.contains(&""), "root entry should be present");
    });
}

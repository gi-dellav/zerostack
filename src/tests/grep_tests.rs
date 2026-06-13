use crate::agent::tools::GrepTool;

#[test]
fn glob_literal_chars() {
    assert_eq!(GrepTool::glob_to_regex("hello"), "hello");
}

#[test]
fn glob_dot() {
    assert_eq!(GrepTool::glob_to_regex("file.txt"), "file\\.txt");
}

#[test]
fn glob_star() {
    assert_eq!(GrepTool::glob_to_regex("*.rs"), ".*\\.rs");
}

#[test]
fn glob_question_mark() {
    assert_eq!(GrepTool::glob_to_regex("file.?"), "file\\..");
}

#[test]
fn glob_brace_alternation() {
    assert_eq!(GrepTool::glob_to_regex("*.{ts,tsx}"), ".*\\.(?:ts|tsx)");
}

#[test]
fn glob_complex_pattern() {
    assert_eq!(
        GrepTool::glob_to_regex("src/**/test_*.{rs,toml}"),
        "src/.*.*/test_.*\\.(?:rs|toml)"
    );
}

#[test]
fn glob_empty() {
    assert_eq!(GrepTool::glob_to_regex(""), "");
}

#[test]
fn is_binary_null_byte_in_first_8k() {
    let mut data = vec![b'a'; 100];
    data[50] = 0;
    assert!(GrepTool::is_binary(&data));
}

#[test]
fn is_binary_no_null_byte() {
    let data = vec![b'a'; 100];
    assert!(!GrepTool::is_binary(&data));
}

#[test]
fn is_binary_empty() {
    assert!(!GrepTool::is_binary(&[]));
}

#[test]
fn is_binary_null_at_start() {
    let data = vec![0, b'a', b'b'];
    assert!(GrepTool::is_binary(&data));
}

#[test]
fn is_binary_null_at_end() {
    let mut data = vec![b'a'; 8192];
    data[8191] = 0;
    assert!(GrepTool::is_binary(&data));
}

#[test]
fn is_binary_all_text() {
    let data = b"hello world\nline 2\nline 3\n";
    assert!(!GrepTool::is_binary(data));
}

#[test]
fn is_binary_non_utf8_no_null() {
    let data = vec![0xFF, 0xFE, 0xFD];
    assert!(!GrepTool::is_binary(&data));
}

use crate::agent::tools::bash::split_bash_commands;

#[test]
fn split_simple_semicolon() {
    let cmds = split_bash_commands("ls; pwd");
    assert_eq!(cmds, vec!["ls", "pwd"]);
}

#[test]
fn split_double_ampersand() {
    let cmds = split_bash_commands("cargo build && cargo test");
    assert_eq!(cmds, vec!["cargo build", "cargo test"]);
}

#[test]
fn split_double_pipe() {
    let cmds = split_bash_commands("false || echo fail");
    assert_eq!(cmds, vec!["false", "echo fail"]);
}

#[test]
fn split_mixed_separators() {
    let cmds = split_bash_commands("make; cargo build && cargo test || echo fail");
    assert_eq!(cmds, vec!["make", "cargo build", "cargo test", "echo fail"]);
}

#[test]
fn split_single_command_no_separator() {
    let cmds = split_bash_commands("echo hello");
    assert_eq!(cmds, vec!["echo hello"]);
}

#[test]
fn split_empty_input() {
    let cmds: Vec<String> = split_bash_commands("");
    assert!(cmds.is_empty());
}

#[test]
fn split_semicolons_only() {
    let cmds: Vec<String> = split_bash_commands(";");
    assert!(cmds.is_empty());
}

#[test]
fn split_leading_semicolon() {
    let cmds = split_bash_commands("; ls");
    assert_eq!(cmds, vec!["ls"]);
}

#[test]
fn split_trailing_semicolon() {
    let cmds = split_bash_commands("ls;");
    assert_eq!(cmds, vec!["ls"]);
}

#[test]
fn split_single_quoted_separators() {
    let cmds = split_bash_commands("echo 'hello;world'");
    assert_eq!(cmds, vec!["echo 'hello;world'"]);
}

#[test]
fn split_double_quoted_separators() {
    let cmds = split_bash_commands("echo \"a && b\"");
    assert_eq!(cmds, vec!["echo \"a && b\""]);
}

#[test]
fn split_escaped_single_quote_inside_single_quotes() {
    let cmds = split_bash_commands("echo 'it\\'s working'");
    assert_eq!(cmds, vec!["echo 'it\\'s working'"]);
}

#[test]
fn split_escaped_double_quote_inside_double_quotes() {
    let cmds = split_bash_commands("echo \"she said \\\"hi\\\"\"");
    assert_eq!(cmds, vec!["echo \"she said \\\"hi\\\"\""]);
}

#[test]
fn split_pipe_not_double_is_inline() {
    let cmds = split_bash_commands("cat file | sort");
    assert_eq!(cmds, vec!["cat file | sort"]);
}

#[test]
fn split_single_ampersand_inline() {
    let cmds = split_bash_commands("sleep 1 & echo done");
    assert_eq!(cmds, vec!["sleep 1 & echo done"]);
}

#[test]
fn split_append_redirect_is_separator() {
    let cmds = split_bash_commands("echo foo >> log.txt");
    assert_eq!(cmds, vec!["echo foo", "log.txt"]);
}

#[test]
fn split_quoted_string_with_mixed_content() {
    let cmds = split_bash_commands("grep '; && ||' file.txt; echo done");
    assert_eq!(cmds, vec!["grep '; && ||' file.txt", "echo done"]);
}

#[test]
fn split_escaped_backslash_before_quote() {
    let cmds = split_bash_commands("echo \\\\'; echo two");
    assert_eq!(cmds, vec!["echo \\\\'; echo two"]);
}

#[test]
fn split_newline_is_separator() {
    // Newlines must be treated as command separators so that newline-separated
    // commands are individually permission-checked (security fix).
    let cmds = split_bash_commands("ls\npwd");
    assert_eq!(cmds, vec!["ls", "pwd"]);
}

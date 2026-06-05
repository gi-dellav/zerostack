use crate::extras::workflows::{load, load_all, parse_workflow};

#[test]
fn parse_empty() {
    let wf = parse_workflow(String::new());
    assert!(wf.slash_commands.is_empty());
    assert!(wf.message.is_empty());
}

#[test]
fn parse_message_only() {
    let wf = parse_workflow("Hello world\nDo another thing\n".to_string());
    assert!(wf.slash_commands.is_empty());
    assert_eq!(wf.message, "Hello world\nDo another thing");
}

#[test]
fn parse_slash_commands_only() {
    let wf = parse_workflow("/model gpt\n/prompt ask\n".to_string());
    assert_eq!(wf.slash_commands, vec!["/model gpt", "/prompt ask"]);
    assert!(wf.message.is_empty());
}

#[test]
fn parse_mixed() {
    let wf = parse_workflow("/model claude\n\nReview the diff\nSuggest fixes\n".to_string());
    assert_eq!(wf.slash_commands, vec!["/model claude"]);
    assert_eq!(wf.message, "Review the diff\nSuggest fixes");
}

#[test]
fn parse_comments() {
    let content =
        "# header comment\n/model claude\n# inline comment\nReview this\n# footer\n".to_string();
    let wf = parse_workflow(content);
    assert_eq!(wf.slash_commands, vec!["/model claude"]);
    assert_eq!(wf.message, "Review this");
}

#[test]
fn parse_only_comments() {
    let wf = parse_workflow("# comment 1\n# comment 2\n".to_string());
    assert!(wf.slash_commands.is_empty());
    assert!(wf.message.is_empty());
}

#[test]
fn parse_blank_lines_ignored() {
    let wf = parse_workflow("\n\n/model claude\n\n\nReview\n".to_string());
    assert_eq!(wf.slash_commands, vec!["/model claude"]);
    assert_eq!(wf.message, "Review");
}

#[test]
fn parse_slash_in_message_not_cmd() {
    let wf = parse_workflow("Run git diff\nThen /review the output\n".to_string());
    assert!(wf.slash_commands.is_empty());
    assert_eq!(wf.message, "Run git diff\nThen /review the output");
}

#[test]
fn parse_multiple_slash_commands() {
    let wf = parse_workflow("/model claude\n/prompt code\n/mode standard\n".to_string());
    assert_eq!(wf.slash_commands.len(), 3);
    assert!(wf.message.is_empty());
}

#[test]
fn parse_slash_commands_before_message() {
    let content =
        "/model claude\n/prompt code\n# now review\nReview changes\nFix bugs\n".to_string();
    let wf = parse_workflow(content);
    assert_eq!(wf.slash_commands, vec!["/model claude", "/prompt code"]);
    assert_eq!(wf.message, "Review changes\nFix bugs");
}

#[test]
fn load_non_existent_workflow() {
    assert!(load("non_existent_workflow_xyz").is_none());
}

#[test]
fn load_all_returns_sorted_vec() {
    let wfs = load_all();
    for pair in wfs.windows(2) {
        assert!(pair[0].name <= pair[1].name);
    }
}

#[test]
fn parse_message_preserves_multiple_blanks_in_content() {
    let wf = parse_workflow("First\n\nSecond\n\nThird\n".to_string());
    assert_eq!(wf.message, "First\nSecond\nThird");
}

#[test]
fn parse_workflow_struct_fields() {
    let wf = parse_workflow("Some message\n".to_string());
    assert_eq!(wf.slash_commands.len(), 0);
    assert_eq!(wf.message, "Some message");
    assert!(wf.name.is_empty());
    assert!(wf.path.as_os_str().is_empty());
}

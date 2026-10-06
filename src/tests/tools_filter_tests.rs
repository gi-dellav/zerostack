use rig::tool::DynamicTool;

use crate::agent::builder::filter_tools_by_allowlist;

fn named_tool(name: &'static str) -> DynamicTool {
    DynamicTool::new(
        name,
        String::new(),
        serde_json::json!({}),
        move |_args: serde_json::Value| {
            Box::pin(async move {
                Ok::<_, rig::tool::ToolExecutionError>(rig::tool::ToolOutput::text(""))
            })
        },
    )
}

fn make_tools(names: &[&'static str]) -> Vec<DynamicTool> {
    names.iter().map(|n| named_tool(n)).collect()
}

fn tool_names(tools: &[DynamicTool]) -> Vec<String> {
    tools.iter().map(|t| t.name().to_string()).collect()
}

#[test]
fn empty_allowlist_passes_all_tools_through() {
    let tools = make_tools(&["read", "write", "bash"]);
    let filtered = filter_tools_by_allowlist(tools, &[]);
    assert_eq!(tool_names(&filtered), vec!["read", "write", "bash"]);
}

#[test]
fn allowlist_retains_only_matching_tools() {
    let tools = make_tools(&["read", "write", "bash", "grep"]);
    let allowlist = vec!["read".to_string(), "grep".to_string()];
    let filtered = filter_tools_by_allowlist(tools, &allowlist);
    assert_eq!(tool_names(&filtered), vec!["read", "grep"]);
}

#[test]
fn unknown_names_in_allowlist_are_ignored() {
    let tools = make_tools(&["read", "write"]);
    let allowlist = vec!["read".to_string(), "bogus".to_string()];
    let filtered = filter_tools_by_allowlist(tools, &allowlist);
    assert_eq!(tool_names(&filtered), vec!["read"]);
}

#[test]
fn allowlist_with_no_matches_returns_empty() {
    let tools = make_tools(&["read", "write"]);
    let allowlist = vec!["bash".to_string(), "grep".to_string()];
    let filtered = filter_tools_by_allowlist(tools, &allowlist);
    assert!(filtered.is_empty());
}

#[test]
fn single_tool_allowlist() {
    let tools = make_tools(&["read", "write", "bash"]);
    let allowlist = vec!["bash".to_string()];
    let filtered = filter_tools_by_allowlist(tools, &allowlist);
    assert_eq!(tool_names(&filtered), vec!["bash"]);
}

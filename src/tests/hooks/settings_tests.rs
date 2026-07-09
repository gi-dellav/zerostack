use crate::extras::hooks::settings::parse_hooks_config;
use serde_json::json;

#[test]
fn parses_cc_shaped_config() {
    let value = json!({
        "PreToolUse": [
            {
                "matcher": "Bash|Write",
                "hooks": [
                    {"type": "command", "command": "echo hi", "timeout": 60}
                ]
            }
        ]
    });

    let config = parse_hooks_config(&value).expect("valid config should parse");
    let groups = config.get("PreToolUse").expect("PreToolUse group present");
    assert_eq!(groups.len(), 1);
    let group = &groups[0];
    assert_eq!(group.matcher.as_deref(), Some("Bash|Write"));
    assert_eq!(group.hooks.len(), 1);
    let handler = &group.hooks[0];
    assert_eq!(handler.kind, "command");
    assert_eq!(handler.command.as_deref(), Some("echo hi"));
    assert_eq!(handler.timeout, Some(60));
    assert!(!handler.is_async);
    assert!(handler.condition.is_none());
    assert!(!handler.once);
}

#[test]
fn if_and_once_fields_still_parse_and_are_preserved() {
    let value = json!({
        "Stop": [
            {
                "hooks": [
                    {
                        "type": "command",
                        "command": "run-tests.sh",
                        "if": "test -f ./Cargo.toml",
                        "once": true
                    }
                ]
            }
        ]
    });

    let config = parse_hooks_config(&value).expect("if/once should not fail parsing");
    let handler = &config.get("Stop").unwrap()[0].hooks[0];
    assert_eq!(handler.condition.as_deref(), Some("test -f ./Cargo.toml"));
    assert!(handler.once);
}

#[test]
fn unsupported_handler_type_is_a_parse_error() {
    let value = json!({
        "PreToolUse": [
            {
                "hooks": [
                    {"type": "http", "command": "irrelevant"}
                ]
            }
        ]
    });

    let err = parse_hooks_config(&value).expect_err("non-command type must fail in v1");
    assert!(err.contains("http") || err.to_lowercase().contains("unsupported"));
}

#[test]
fn args_field_parses_as_string_list() {
    let value = json!({
        "PreToolUse": [
            {
                "hooks": [
                    {"type": "command", "command": "guard", "args": ["--strict", "--verbose"]}
                ]
            }
        ]
    });

    let config = parse_hooks_config(&value).unwrap();
    let handler = &config.get("PreToolUse").unwrap()[0].hooks[0];
    assert_eq!(
        handler.args.as_deref(),
        Some(&["--strict".to_string(), "--verbose".to_string()][..])
    );
}

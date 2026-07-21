use std::collections::HashMap;

use crate::permission::{SecurityMode, resolve_startup_prompt_mode};

fn prompts(entries: &[(&str, &str)]) -> HashMap<String, String> {
    entries
        .iter()
        .map(|(name, content)| (name.to_string(), content.to_string()))
        .collect()
}

#[test]
fn mode_directive_resolves_to_security_mode() {
    let map = prompts(&[
        ("review", "%%mode=readonly\nReview the code."),
        ("plan", "%%mode=planwrite\nPlan the work."),
    ]);

    assert_eq!(
        resolve_startup_prompt_mode(&map, "review"),
        Some(SecurityMode::ReadOnly)
    );
    assert_eq!(
        resolve_startup_prompt_mode(&map, "plan"),
        Some(SecurityMode::PlanWrite)
    );
}

#[test]
fn last_user_mode_yields_none_at_startup() {
    let map = prompts(&[("code", "%%mode=last_user_mode\nYou are a coder.")]);

    assert_eq!(resolve_startup_prompt_mode(&map, "code"), None);
}

#[test]
fn prompt_without_directive_yields_none() {
    let map = prompts(&[("code", "You are a coder.")]);

    assert_eq!(resolve_startup_prompt_mode(&map, "code"), None);
}

#[test]
fn unknown_prompt_yields_none() {
    let map = prompts(&[("code", "%%mode=readonly\nYou are a coder.")]);

    assert_eq!(resolve_startup_prompt_mode(&map, "missing"), None);
}

#[test]
fn unrecognized_mode_name_yields_none() {
    let map = prompts(&[("weird", "%%mode=bogus\nBody.")]);

    assert_eq!(resolve_startup_prompt_mode(&map, "weird"), None);
}

#[test]
fn directive_is_only_read_from_the_first_line() {
    let map = prompts(&[("code", "You are a coder.\n%%mode=readonly")]);

    assert_eq!(resolve_startup_prompt_mode(&map, "code"), None);
}

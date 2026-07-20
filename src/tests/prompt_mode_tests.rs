use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::context::ContextFiles;
use crate::permission::checker::{PermCheck, PermissionChecker};
use crate::permission::{PermissionConfigs, SecurityMode};
use crate::ui::{PromptModeOutcome, apply_prompt_mode};

fn make_context(prompts: &[(&str, &str)]) -> ContextFiles {
    ContextFiles {
        agents: None,
        prompts: prompts
            .iter()
            .map(|(name, content)| (name.to_string(), content.to_string()))
            .collect::<HashMap<_, _>>(),
        current_prompt: None,
        current_prompt_name: None,
        themes: HashMap::new(),
        current_theme_name: None,
        extra_files: Vec::new(),
        one_shot_restore: None,
        chain_declined: Vec::new(),
        #[cfg(feature = "memory")]
        memory: None,
        #[cfg(feature = "archmd")]
        architecture: None,
    }
}

fn make_perm(mode: SecurityMode) -> PermCheck {
    Arc::new(Mutex::new(PermissionChecker::new(
        &PermissionConfigs::default(),
        mode,
        None,
        None,
    )))
}

fn current_mode(perm: &PermCheck) -> SecurityMode {
    perm.lock().unwrap_or_else(|e| e.into_inner()).mode()
}

#[test]
fn prompt_without_directive_keeps_content_and_mode_untouched() {
    let mut context = make_context(&[("code", "You are a coder.")]);
    let perm = make_perm(SecurityMode::Standard);

    let outcome = apply_prompt_mode("code", &mut context, &Some(perm.clone()));

    assert_eq!(outcome, PromptModeOutcome::None);
    assert_eq!(context.current_prompt.as_deref(), Some("You are a coder."));
    assert_eq!(context.current_prompt_name.as_deref(), Some("code"));
    assert_eq!(current_mode(&perm), SecurityMode::Standard);
}

#[test]
fn mode_directive_is_stripped_and_applied() {
    let mut context = make_context(&[("review", "%%mode=readonly\nReview the code.")]);
    let perm = make_perm(SecurityMode::Standard);

    let outcome = apply_prompt_mode("review", &mut context, &Some(perm.clone()));

    assert_eq!(outcome, PromptModeOutcome::Applied(SecurityMode::ReadOnly));
    assert_eq!(context.current_prompt.as_deref(), Some("Review the code."));
    assert_eq!(context.current_prompt_name.as_deref(), Some("review"));
    assert_eq!(current_mode(&perm), SecurityMode::ReadOnly);
}

#[test]
fn last_user_mode_restores_the_user_selected_mode() {
    let mut context = make_context(&[
        ("review", "%%mode=readonly\nReview the code."),
        ("code", "%%mode=last_user_mode\nYou are a coder."),
    ]);
    let perm = make_perm(SecurityMode::Guarded);

    // A prompt-imposed mode must not clobber the user-selected mode...
    apply_prompt_mode("review", &mut context, &Some(perm.clone()));
    assert_eq!(current_mode(&perm), SecurityMode::ReadOnly);

    // ...so a `last_user_mode` prompt restores it afterwards.
    let outcome = apply_prompt_mode("code", &mut context, &Some(perm.clone()));

    assert_eq!(outcome, PromptModeOutcome::RestoredUserMode);
    assert_eq!(current_mode(&perm), SecurityMode::Guarded);
    assert_eq!(context.current_prompt.as_deref(), Some("You are a coder."));
    assert_eq!(context.current_prompt_name.as_deref(), Some("code"));
}

#[test]
fn unknown_prompt_is_a_noop() {
    let mut context = make_context(&[("code", "You are a coder.")]);
    let perm = make_perm(SecurityMode::Standard);

    let outcome = apply_prompt_mode("missing", &mut context, &Some(perm.clone()));

    assert_eq!(outcome, PromptModeOutcome::None);
    assert!(context.current_prompt.is_none());
    assert!(context.current_prompt_name.is_none());
    assert_eq!(current_mode(&perm), SecurityMode::Standard);
}

#[test]
fn unrecognized_mode_strips_directive_but_keeps_current_mode() {
    let mut context = make_context(&[("weird", "%%mode=bogus\nBody.")]);
    let perm = make_perm(SecurityMode::Standard);

    let outcome = apply_prompt_mode("weird", &mut context, &Some(perm.clone()));

    assert_eq!(outcome, PromptModeOutcome::None);
    assert_eq!(context.current_prompt.as_deref(), Some("Body."));
    assert_eq!(context.current_prompt_name.as_deref(), Some("weird"));
    assert_eq!(current_mode(&perm), SecurityMode::Standard);
}

#[test]
fn without_permission_checker_prompt_is_still_selected() {
    let mut context = make_context(&[("review", "%%mode=readonly\nReview the code.")]);

    let outcome = apply_prompt_mode("review", &mut context, &None);

    assert_eq!(outcome, PromptModeOutcome::None);
    assert_eq!(context.current_prompt.as_deref(), Some("Review the code."));
    assert_eq!(context.current_prompt_name.as_deref(), Some("review"));
}

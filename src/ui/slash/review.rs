use crate::session::MessageRole;
use crate::ui::apply_prompt_mode;
use crate::ui::slash::{SlashCtx, write_error, write_ok};

fn is_session_empty(ctx: &SlashCtx<'_>) -> bool {
    !ctx.session
        .messages
        .iter()
        .any(|m| m.role == MessageRole::User)
}

fn is_in_worktree() -> bool {
    #[cfg(feature = "git-worktree")]
    {
        crate::extras::git_worktree::detect().is_some()
    }
    #[cfg(not(feature = "git-worktree"))]
    {
        false
    }
}

fn build_default_review_message(session_empty: bool, in_worktree: bool) -> String {
    match (session_empty, in_worktree) {
        (true, true) => "Review the current worktree state. Check the diff from the base branch \
                         for correctness, design, testing, and security."
            .to_string(),
        (true, false) => "Review the current codebase for correctness, design, testing, and \
                          security."
            .to_string(),
        (false, true) => "Review the changes in this worktree session. Consider the diff from \
                          main since the branch was created. Check for correctness, design, \
                          testing, and security."
            .to_string(),
        (false, false) => "Review the changes discussed in this session for correctness, \
                           design, testing, and security."
            .to_string(),
    }
}

pub async fn handle(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    if !ctx.context.prompts.contains_key("review") {
        write_error(
            ctx.renderer,
            "no 'review' prompt found. Run /regen-prompts first.",
        );
        return Ok(());
    }

    let msg = if parts.len() > 1 {
        parts[1..].join(" ")
    } else {
        let session_empty = is_session_empty(ctx);
        let in_worktree = is_in_worktree();
        build_default_review_message(session_empty, in_worktree)
    };

    // Save current prompt for one-shot restoration
    ctx.context.one_shot_restore = ctx.context.current_prompt_name.clone();

    // Switch to review prompt
    apply_prompt_mode("review", ctx.context, ctx.permission);

    let model_switched = ctx.switch_to_prompt_model("review").await;
    if !model_switched {
        ctx.rebuild_agent().await;
    }
    write_ok(ctx.renderer, format!("review: {}", msg));

    Err(anyhow::anyhow!("DEFER_REVIEW:{}", msg))
}

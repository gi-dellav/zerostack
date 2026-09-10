use std::collections::VecDeque;

use crate::engine::pal;
use crate::ui::renderer::Renderer;

/// Load `path` and arm the PAL queue on `chain`: each executable script line
/// becomes one queued step. Steps drain through `pending_inputs` so agent
/// messages run one full turn at a time and `/`/`!` lines go through the
/// normal typed-input dispatch on submission.
///
/// Pure enough for unit tests: takes the file `content` plus only the two
/// `ChainState` fields it sets, leaving all renderer/input work to [`handle`].
/// Returns an error string when the content holds no executable steps or
/// contains a nested `/pal`; otherwise the queued steps.
pub fn plan_pal_content(content: &str) -> Result<Vec<String>, String> {
    let entries = pal::parse_content(content);
    if entries.is_empty() {
        return Err("no executable steps".to_string());
    }
    // Nested `/pal` inside a script would recurse (scripts drain as queued
    // submissions, and a `/pal` submission re-arms the queue). Reject the
    // whole script up front with a friendly message instead.
    if entries.iter().any(|e| pal::is_nested_pal(&e.text)) {
        return Err("nested /pal is not supported".to_string());
    }
    Ok(entries.into_iter().map(|e| e.text).collect())
}

/// Arm the PAL queue from an already-read `content`. Split from [`handle`] so
/// the borrow of `chain` never overlaps the `SlashCtx` borrow of
/// `chain.loop_state` in `handle_slash`: this takes only the renderer, the
/// input editor, and the run-active flag, never a full `SlashCtx`.
pub fn arm_pal(
    renderer: &mut Renderer,
    input: &mut crate::ui::input::InputEditor,
    chain: &mut crate::ui::state::ChainState,
    arg: &str,
    content: &str,
) -> Result<usize, String> {
    let steps = plan_pal_content(content).map_err(|e| format!("{arg}: {e}"))?;
    chain.pal_total = steps.len();
    chain.pal_source = Some(arg.to_string());
    chain.pal_queue = VecDeque::from(steps);
    // The arming `/pal` itself was submitted as user input; the first step
    // must not sit behind it in the queue mechanics — `finalize_turn` drains
    // one `pending_inputs` entry per idle turn, so seed it with step one now
    // and leave the rest in `pal_queue`.
    if let Some(first) = chain.pal_queue.pop_front() {
        input.buffer = first.into();
        input.cursor = input.buffer.len();
        let _ = renderer.write_line(
            &format!("pal: running 1/{} from {arg}", chain.pal_total),
            crate::ui::slash::C_AGENT,
        );
    }
    Ok(chain.pal_total)
}

pub async fn handle(
    parts: &[&str],
    renderer: &mut Renderer,
    input: &mut crate::ui::input::InputEditor,
    is_running: bool,
    chain: &mut crate::ui::state::ChainState,
) -> anyhow::Result<()> {
    let Some(arg) = parts.get(1).map(|s| s.trim()).filter(|s| !s.is_empty()) else {
        let _ = renderer.write_line("usage: /pal <file.pal|file.txt>", crate::ui::slash::C_AGENT);
        return Ok(());
    };
    if is_running {
        let _ = renderer.write_line(
            "agent is running — wait for it to finish or press Ctrl-C before running /pal",
            crate::ui::slash::C_ERROR,
        );
        return Ok(());
    }
    let path = pal::resolve_script_path(arg);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            let _ = renderer.write_line(
                &format!("cannot read {arg}: {e}"),
                crate::ui::slash::C_ERROR,
            );
            return Ok(());
        }
    };
    if let Err(e) = arm_pal(renderer, input, chain, arg, &content) {
        let _ = renderer.write_line(&e.to_string(), crate::ui::slash::C_ERROR);
    }
    Ok(())
}

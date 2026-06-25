use crossterm::style::Color;
use tokio::sync::mpsc;

use crate::cli::Cli;
use crate::event::UserEvent;
use crate::session::Session;
use crate::ui::renderer::Renderer;
use crate::ui::utils::suggest_pattern;

use super::C_PERM;

#[allow(clippy::too_many_arguments)]
pub async fn handle_permission_request(
    ask_req: crate::permission::ask::AskRequest,
    renderer: &mut Renderer,
    session: &mut Session,
    cli: &Cli,
    user_rx: &mut mpsc::Receiver<UserEvent>,
    agent_line_started: &mut bool,
    was_reasoning: &mut bool,
) -> anyhow::Result<()> {
    *was_reasoning = false;
    if *agent_line_started {
        renderer.write_line("", Color::White)?;
        *agent_line_started = false;
    }

    renderer.write_line(
        &format!("[permission] {}: {}", ask_req.tool, ask_req.input),
        C_PERM,
    )?;
    renderer.write_line(
        "  (y) allow once  (a) allow always  (n) deny  (ESC) abort",
        C_PERM,
    )?;

    renderer.permission_prompt = Some(super::renderer::PermissionPrompt {
        tool: format!("[permission] {}: {}", ask_req.tool, ask_req.input).into(),
        options: "  (y) allow once  (a) allow always  (n) deny  (ESC) abort".into(),
    });
    renderer.render_viewport()?;
    renderer.draw_bottom("", 0, &[], false)?;

    let decision = loop {
        tokio::select! {
            Some(ev) = user_rx.recv() => {
                if let crate::event::UserEvent::Key(key) = ev {
                    match key.code {
                        crossterm::event::KeyCode::Char('y') => break crate::permission::ask::UserDecision::AllowOnce,
                        crossterm::event::KeyCode::Char('a') => {
                            let pattern = suggest_pattern(&ask_req.tool, &ask_req.input);
                            renderer.write_line(
                                &format!("  -> will allow: {}", pattern),
                                Color::Green,
                            )?;
                            break crate::permission::ask::UserDecision::AllowAlways(pattern);
                        }
                        crossterm::event::KeyCode::Char('n') | crossterm::event::KeyCode::Esc => break crate::permission::ask::UserDecision::Deny,
                        _ => {}
                    }
                }
            }
        }
    };

    renderer.permission_prompt = None;

    let allow_pattern = match &decision {
        crate::permission::ask::UserDecision::AllowAlways(p) => Some(p.clone()),
        _ => None,
    };
    let _ = ask_req.reply.send(decision);

    if let Some(pattern) = allow_pattern {
        renderer.write_line(
            &format!("  allowed {} {} (saved to session)", ask_req.tool, pattern),
            Color::Green,
        )?;
        session
            .permission_allowlist
            .push(crate::session::PermissionAllowEntry {
                tool: ask_req.tool.clone(),
                pattern: pattern.into(),
            });
        if !cli.no_session {
            let _ = crate::session::storage::save_session(session);
        }
    }

    Ok(())
}

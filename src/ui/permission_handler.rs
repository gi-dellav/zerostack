use tokio::sync::mpsc;

use crate::cli::Cli;
use crate::event::UserEvent;
use crate::session::Session;
use crate::ui::renderer::{LineColor, Renderer};
use crate::ui::utils::suggest_pattern;

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
        renderer.write_line("", LineColor::AgentText)?;
        *agent_line_started = false;
    }

    renderer.write_line(
        &format!("[permission] {}: {}", ask_req.tool, ask_req.input),
        LineColor::Permission,
    )?;
    renderer.write_line(
        "  (y) allow once  (a) allow always  (n) deny  (ESC) abort",
        LineColor::Permission,
    )?;

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
                                LineColor::Success,
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

    let allow_pattern = match &decision {
        crate::permission::ask::UserDecision::AllowAlways(p) => Some(p.clone()),
        _ => None,
    };
    let _ = ask_req.reply.send(decision);

    if let Some(pattern) = allow_pattern {
        renderer.write_line(
            &format!("  allowed {} {} (saved to session)", ask_req.tool, pattern),
            LineColor::Success,
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

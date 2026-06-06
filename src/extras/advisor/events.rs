use crossterm::style::Color;

use crate::event::AgentEvent;
use crate::ui::events::sanitize_output;
use crate::ui::renderer::Renderer;

const C_TOOL: Color = Color::Yellow;
const C_ERROR: Color = Color::Red;

pub fn handle(
    event: &AgentEvent,
    renderer: &mut Renderer,
    was_reasoning: &mut bool,
    agent_line_started: &mut bool,
    response_buf: &mut String,
    response_start_line: &mut Option<usize>,
) -> Option<anyhow::Result<()>> {
    match event {
        AgentEvent::AdvisorConsulting => {
            *was_reasoning = false;
            if *agent_line_started {
                renderer.write_line("", Color::White).ok()?;
                *agent_line_started = false;
            }
            response_buf.clear();
            *response_start_line = None;
            renderer
                .write_line("⬢ consulting advisor...", C_TOOL)
                .ok()?;
            Some(Ok(()))
        }
        AgentEvent::AdvisorResult { text } => {
            renderer
                .write_line(&sanitize_output(text), Color::DarkMagenta)
                .ok()?;
            Some(Ok(()))
        }
        AgentEvent::AdvisorError { error } => {
            renderer
                .write_line(
                    &format!("advisor error: {}", sanitize_output(error)),
                    C_ERROR,
                )
                .ok()?;
            Some(Ok(()))
        }
        _ => None,
    }
}

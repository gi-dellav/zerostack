use crate::session::Session;

pub struct StatusLine;

impl StatusLine {
    pub fn render(session: &Session, is_running: bool) -> String {
        let state = if is_running { "running" } else { "ready" };
        let dir = session
            .working_dir
            .split('/')
            .next_back()
            .unwrap_or(&session.working_dir);
        let cost_str = if session.total_cost > 0.0 {
            format!(" ${:.4}", session.total_cost)
        } else {
            String::new()
        };
        let tokens_str = if session.total_tokens > 0 {
            format!(" {}tok", session.total_tokens)
        } else {
            String::new()
        };
        format!(
            "{}{}{} | {} | {} | {}msgs | {}",
            dir,
            tokens_str,
            cost_str,
            session.provider,
            session.model,
            session.messages.len(),
            state,
        )
    }
}

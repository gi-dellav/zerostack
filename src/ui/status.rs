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
        format!(
            "{} | {} | {} | {}msgs | status: {}",
            dir,
            session.provider,
            session.model,
            session.messages.len(),
            state,
        )
    }
}

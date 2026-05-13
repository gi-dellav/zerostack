use crate::session::Session;

pub struct StatusLine;

fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{}k", n / 1000)
    } else {
        n.to_string()
    }
}

impl StatusLine {
    pub fn render(session: &Session, is_running: bool, _spinner_tick: u64, loop_label: Option<&str>) -> String {
        let state = if is_running { "running" } else { "ready" };
        let dir = session
            .working_dir
            .split('/')
            .next_back()
            .unwrap_or(&session.working_dir);

        let ctx = session.context_window;
        let used = session.total_estimated_tokens;
        let pct = if ctx > 0 { (used * 100) / ctx } else { 0 };

        let cost_str = if session.total_cost > 0.0 {
            format!(" ${:.4}", session.total_cost)
        } else {
            String::new()
        };

        let compact_badge = if session.compactions.is_empty() {
            String::new()
        } else {
            format!(" cmp:{}", session.compactions.len())
        };

        let loop_badge = match loop_label {
            Some(label) => format!(" [{}]", label),
            None => String::new(),
        };

        format!(
            "{}{} | {}{} | {}/{} ({}%) | {}msgs | {}{}",
            dir,
            cost_str,
            session.model,
            loop_badge,
            fmt_tokens(used),
            fmt_tokens(ctx),
            pct,
            session.messages.len(),
            state,
            compact_badge,
        )
    }
}

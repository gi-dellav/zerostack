use std::path::Path;

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
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        session: &Session,
        _is_running: bool,
        _spinner_tick: u64,
        loop_label: Option<&str>,
        prompt_name: Option<&str>,
        perm_mode: Option<&str>,
        chain_label: Option<&str>,
        btw_cost: f64,
        btw_in: u64,
        btw_out: u64,
    ) -> (String, Option<String>) {
        let state = if let Some(name) = prompt_name {
            format!("prompt:{}", name)
        } else {
            String::new()
        };
        let dir = Path::new(&session.working_dir)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&session.working_dir);

        let ctx = session.context_window;
        let pct = (session.effective_context_tokens() * 100)
            .checked_div(ctx)
            .unwrap_or(0);

        let cost_str = if session.total_cost > 0.0 {
            format!(" ${:.4}", session.total_cost)
        } else {
            String::new()
        };

        // Side-question (`/btw`) usage is tracked and shown separately so it
        // never pollutes the main session totals. Shown once `/btw` is used;
        // cost is added only when the model has per-token pricing configured.
        let btw_badge = if btw_in > 0 || btw_out > 0 {
            if btw_cost > 0.0 {
                format!(
                    " btw:${:.4} ({}/{})",
                    btw_cost,
                    fmt_tokens(btw_in),
                    fmt_tokens(btw_out)
                )
            } else {
                format!(" btw:{}/{}", fmt_tokens(btw_in), fmt_tokens(btw_out))
            }
        } else {
            String::new()
        };

        let token_detail = if session.total_input_tokens > 0 || session.total_output_tokens > 0 {
            format!(
                " \u{21D1}{} \u{21D3}{}",
                fmt_tokens(session.total_input_tokens),
                fmt_tokens(session.total_output_tokens),
            )
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

        let perm_badge = match perm_mode {
            Some(m) if m != "standard" => format!(" | mode:{}", m),
            _ => String::new(),
        };

        let chain_badge = chain_label.map(|label| format!(" | {}", label));

        let status = format!(
            "{}{}{} | {}{} | {}%/{}{}{} | {}{}",
            dir,
            cost_str,
            btw_badge,
            session.model,
            loop_badge,
            pct,
            fmt_tokens(ctx),
            token_detail,
            compact_badge,
            state,
            perm_badge,
        );
        (status, chain_badge)
    }
}

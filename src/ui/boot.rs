//! Startup progress log rendered into the chat feed.
//!
//! Startup phases record their steps (`config loaded from …`, `MCP redmine
//! connected`, …) into a [`BootState`] via the [`Boot`] handle threaded
//! through `startup.rs`. When the chat UI opens, the collected lines are
//! pushed into the feed below the banner, so the information stays visible
//! as scrollable history instead of flashing by on a separate screen.
//!
//! `BootState` is the pure, unit-testable core; `Boot` is the `Option`
//! wrapper so disabled (non-interactive) runs collect nothing and call sites
//! stay unconditional.

use compact_str::CompactString;
use crossterm::style::Color;

/// Format a step duration for the startup log: milliseconds under a second,
/// one decimal of seconds above. Durations under 1 ms render as `0ms` and
/// are suppressed by the caller.
pub(crate) fn fmt_duration(d: std::time::Duration) -> String {
    if d.as_secs() >= 1 {
        format!("{:.1}s", d.as_secs_f64())
    } else {
        format!("{}ms", d.as_millis())
    }
}

/// ASCII wordmark drawn at the top of the chat banner, one gradient color
/// per line (plain text when monochrome). Figlet "standard" style.
pub(crate) const LOGO: &[&str] = &[
    "███████╗███████╗██████╗  ██████╗ ███████╗████████╗ █████╗  ██████╗██╗  ██╗",
    "╚══███╔╝██╔════╝██╔══██╗██╔═══██╗██╔════╝╚══██╔══╝██╔══██╗██╔════╝██║ ██╔╝",
    "  ███╔╝ █████╗  ██████╔╝██║   ██║███████╗   ██║   ███████║██║     █████╔╝ ",
    " ███╔╝  ██╔══╝  ██╔══██╗██║   ██║╚════██║   ██║   ██╔══██║██║     ██╔═██╗ ",
    "███████╗███████╗██║  ██║╚██████╔╝███████║   ██║   ██║  ██║╚██████╗██║  ██╗",
    "╚══════╝╚══════╝╚═╝  ╚═╝ ╚═════╝ ╚══════╝   ╚═╝   ╚═╝  ╚═╝ ╚═════╝╚═╝  ╚═╝",
];

/// Per-line logo colors (top to bottom).
pub(crate) const LOGO_COLORS: &[Color] = &[
    Color::Cyan,
    Color::Cyan,
    Color::Blue,
    Color::Blue,
    Color::Magenta,
    Color::Magenta,
];

/// Truncate `s` to at most `max` chars, appending '…' when cut. Used to keep
/// failure messages (e.g. MCP connect errors) on one line.
pub(crate) fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

/// Render a path with the user's home directory abbreviated to `~`, keeping
/// detail lines (config location, directories) short.
pub(crate) fn abbreviate_home(p: &std::path::Path) -> String {
    let s = p.display().to_string();
    if let Some(home) = std::env::var_os("HOME") {
        let home = home.to_string_lossy();
        if let Some(rest) = s.strip_prefix(home.as_ref())
            && (rest.is_empty() || rest.starts_with('/'))
        {
            return format!("~{}", rest);
        }
    }
    s
}

/// Compile-time cargo features enabled in this build, alphabetically, shown
/// in the startup log as `✓ Features — …`.
pub(crate) fn active_features() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    #[cfg(feature = "acp")]
    out.push("acp");
    #[cfg(feature = "advisor")]
    out.push("advisor");
    #[cfg(feature = "archmd")]
    out.push("archmd");
    #[cfg(feature = "export")]
    out.push("export");
    #[cfg(feature = "git-worktree")]
    out.push("git-worktree");
    #[cfg(feature = "hooks")]
    out.push("hooks");
    #[cfg(feature = "loop")]
    out.push("loop");
    #[cfg(feature = "mcp")]
    out.push("mcp");
    #[cfg(feature = "memory")]
    out.push("memory");
    #[cfg(feature = "multimodal")]
    out.push("multimodal");
    #[cfg(feature = "multithread")]
    out.push("multithread");
    #[cfg(feature = "pdf")]
    out.push("pdf");
    #[cfg(feature = "status-signals")]
    out.push("status-signals");
    #[cfg(feature = "subagents")]
    out.push("subagents");
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StepStatus {
    Active,
    /// Finished; carries an optional detail shown after the step name and an
    /// optional elapsed time (`✓ Model pricing — 180ms`).
    Done {
        detail: Option<CompactString>,
        elapsed: Option<std::time::Duration>,
    },
    Warn(CompactString),
}

/// Ordered startup steps and their statuses. Pure state, no I/O.
#[derive(Default)]
pub(crate) struct BootState {
    steps: Vec<(CompactString, StepStatus, Option<std::time::Instant>)>,
}

impl BootState {
    /// Mark `name` as the step currently running. Re-activates an existing
    /// step of the same name instead of duplicating it.
    pub(crate) fn start(&mut self, name: &str) {
        let now = std::time::Instant::now();
        if let Some(step) = self.steps.iter_mut().find(|(n, _, _)| n.as_str() == name) {
            step.1 = StepStatus::Active;
            step.2 = Some(now);
        } else {
            self.steps
                .push((CompactString::new(name), StepStatus::Active, Some(now)));
        }
    }

    /// Elapsed since the step was started, if it was.
    fn elapsed_of(&self, name: &str) -> Option<std::time::Duration> {
        self.steps
            .iter()
            .find(|(n, _, _)| n.as_str() == name)
            .and_then(|(_, _, started)| started.map(|t| t.elapsed()))
    }

    /// Mark `name` as finished. Unknown names are ignored (a phase may skip
    /// a step it started only conditionally).
    pub(crate) fn done(&mut self, name: &str) {
        let elapsed = self.elapsed_of(name);
        if let Some(step) = self.steps.iter_mut().find(|(n, _, _)| n.as_str() == name) {
            step.1 = StepStatus::Done {
                detail: None,
                elapsed,
            };
        }
    }

    /// Mark `name` as finished with a detail suffix, creating the step when
    /// it was never started (for work that completed before the log line was
    /// recorded, like reading the config file — no elapsed time then).
    pub(crate) fn loaded(&mut self, name: &str, detail: &str) {
        let status = StepStatus::Done {
            detail: Some(CompactString::new(detail)),
            elapsed: self.elapsed_of(name),
        };
        if let Some(step) = self.steps.iter_mut().find(|(n, _, _)| n.as_str() == name) {
            step.1 = status;
        } else {
            self.steps.push((CompactString::new(name), status, None));
        }
    }

    /// Mark `name` as finished with a warning worth surfacing.
    pub(crate) fn warn(&mut self, name: &str, msg: &str) {
        if let Some(step) = self.steps.iter_mut().find(|(n, _, _)| n.as_str() == name) {
            step.1 = StepStatus::Warn(CompactString::new(msg));
        }
    }

    /// One `(text, color)` per step, e.g. `✓ Loading context files`.
    pub(crate) fn render_lines(&self) -> Vec<(String, Color)> {
        self.steps
            .iter()
            .map(|(name, status, _)| match status {
                StepStatus::Active => (format!("… {}", name), Color::Yellow),
                StepStatus::Done { detail, elapsed } => {
                    let mut line = format!("✓ {}", name);
                    if let Some(detail) = detail {
                        line.push_str(&format!(" — {}", detail));
                    }
                    if let Some(elapsed) = elapsed
                        && *elapsed >= std::time::Duration::from_millis(1)
                    {
                        line.push_str(&format!(" ({})", fmt_duration(*elapsed)));
                    }
                    (line, Color::Green)
                }
                StepStatus::Warn(msg) => (format!("! {}: {}", name, msg), Color::Red),
            })
            .collect()
    }
}

/// Startup-log handle threaded through startup. All methods are no-ops when
/// disabled (non-interactive runs, or `show_boot_screen = false`), so startup
/// code never has to branch.
pub(crate) struct Boot(Option<BootState>);

impl Boot {
    pub(crate) fn new(enabled: bool) -> Self {
        Boot(enabled.then(BootState::default))
    }

    pub(crate) fn step(&mut self, name: &str) {
        if let Some(s) = self.0.as_mut() {
            s.start(name);
        }
    }

    pub(crate) fn done(&mut self, name: &str) {
        if let Some(s) = self.0.as_mut() {
            s.done(name);
        }
    }

    /// Mark a step finished with a detail suffix (see [`BootState::loaded`]).
    pub(crate) fn loaded(&mut self, name: &str, detail: &str) {
        if let Some(s) = self.0.as_mut() {
            s.loaded(name, detail);
        }
    }

    pub(crate) fn warn(&mut self, name: &str, msg: &str) {
        if let Some(s) = self.0.as_mut() {
            s.warn(name, msg);
        }
    }

    /// Hand the collected steps to the chat UI for replay into the feed.
    /// `None` when the log was disabled (nothing to replay).
    pub(crate) fn into_state(self) -> Option<BootState> {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_track_activation_and_completion() {
        let mut state = BootState::default();
        state.start("Loading context files");
        state.start("Resolving provider & model");
        state.done("Loading context files");

        let lines = state.render_lines();
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0],
            ("✓ Loading context files".to_string(), Color::Green)
        );
        assert_eq!(
            lines[1],
            ("… Resolving provider & model".to_string(), Color::Yellow)
        );
    }

    #[test]
    fn restart_reactivates_existing_step_without_duplicating() {
        let mut state = BootState::default();
        state.start("step");
        state.done("step");
        state.start("step");

        assert_eq!(state.steps.len(), 1);
        assert_eq!(state.steps[0].1, StepStatus::Active);
    }

    #[test]
    fn loaded_upserts_a_finished_step_with_detail() {
        let mut state = BootState::default();
        state.loaded("Loading configuration", "~/.config/zerostack/config.toml");
        state.start("Resolving provider & model");
        state.loaded("Resolving provider & model", "anthropic/claude-opus-4");

        let lines = state.render_lines();
        assert_eq!(
            lines[0],
            (
                "✓ Loading configuration — ~/.config/zerostack/config.toml".to_string(),
                Color::Green
            )
        );
        assert_eq!(
            lines[1],
            (
                "✓ Resolving provider & model — anthropic/claude-opus-4".to_string(),
                Color::Green
            )
        );
    }

    #[test]
    fn warn_renders_message_in_red() {
        let mut state = BootState::default();
        state.start("Fetching model pricing");
        state.warn("Fetching model pricing", "request failed");

        let lines = state.render_lines();
        assert_eq!(
            lines,
            vec![(
                "! Fetching model pricing: request failed".to_string(),
                Color::Red
            )]
        );
    }

    #[test]
    fn done_on_unknown_step_is_ignored() {
        let mut state = BootState::default();
        state.done("never started");
        assert!(state.render_lines().is_empty());
    }

    #[test]
    fn disabled_boot_collects_nothing() {
        let mut boot = Boot::new(false);
        boot.step("step");
        boot.loaded("other", "detail");
        boot.warn("step", "msg");
        assert!(boot.into_state().is_none());
    }

    #[test]
    fn done_step_shows_elapsed_when_measurable() {
        let mut state = BootState::default();
        state.start("Model pricing");
        // Simulate a step that took measurable time.
        state.steps[0].2 = Some(std::time::Instant::now() - std::time::Duration::from_millis(50));
        state.done("Model pricing");

        let lines = state.render_lines();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].0.starts_with("✓ Model pricing ("));
        assert!(lines[0].0.ends_with("ms)"));
    }

    #[test]
    fn sub_millisecond_steps_hide_elapsed() {
        let mut state = BootState::default();
        state.start("Tools & permissions");
        state.done("Tools & permissions");

        let lines = state.render_lines();
        assert_eq!(lines[0].0, "✓ Tools & permissions");
    }

    #[test]
    fn fmt_duration_picks_unit() {
        assert_eq!(fmt_duration(std::time::Duration::from_millis(180)), "180ms");
        assert_eq!(fmt_duration(std::time::Duration::from_millis(1200)), "1.2s");
    }

    #[test]
    fn logo_is_six_equal_width_lines_with_matching_colors() {
        assert_eq!(LOGO.len(), LOGO_COLORS.len());
        let width = LOGO[0].chars().count();
        assert!(LOGO.iter().all(|l| l.chars().count() == width));
    }

    #[test]
    fn truncate_leaves_short_strings_untouched() {
        assert_eq!(truncate("connection refused", 80), "connection refused");
        assert_eq!(truncate("exact", 5), "exact");
    }

    #[test]
    fn truncate_cuts_long_strings_with_ellipsis() {
        let out = truncate(&"x".repeat(200), 80);
        assert_eq!(out.chars().count(), 80);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn abbreviate_home_shortens_home_prefix_only() {
        let home = std::env::var_os("HOME").expect("HOME is set in test env");
        let home = std::path::PathBuf::from(home);

        let inside = abbreviate_home(&home.join("zerostack/config.toml"));
        assert_eq!(inside, "~/zerostack/config.toml");

        let outside = abbreviate_home(std::path::Path::new("/var/tmp/x"));
        assert_eq!(outside, "/var/tmp/x");

        // A directory that merely shares a textual prefix is not home.
        let lookalike = format!("{}-backup/x", home.display());
        assert_eq!(abbreviate_home(std::path::Path::new(&lookalike)), lookalike);
    }

    #[test]
    fn active_features_is_alphabetical_and_matches_build() {
        let features = active_features();
        let mut sorted = features.clone();
        sorted.sort();
        assert_eq!(features, sorted);

        #[cfg(feature = "mcp")]
        assert!(features.contains(&"mcp"));
        #[cfg(not(feature = "mcp"))]
        assert!(!features.contains(&"mcp"));
        #[cfg(feature = "hooks")]
        assert!(features.contains(&"hooks"));
        #[cfg(not(feature = "hooks"))]
        assert!(!features.contains(&"hooks"));
    }
}

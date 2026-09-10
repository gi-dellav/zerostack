//! PAL (Programmable Agents Language): a minimal line-based workflow language.
//!
//! Each line of a `.pal`/`.txt` file is classified independently:
//!
//! - empty/whitespace → skipped
//! - `#...` → comment, skipped
//! - `/...` → slash command, passed to [`crate::engine::Engine::run_string`]
//! - `!...` → shell command, passed to `Engine::run_string`
//! - anything else → user message, passed to `Engine::run_string`
//!
//! Classification is pure (no I/O, no agent) so both the headless [`Engine`]
//! and the interactive TUI share it. Execution lives where the run state
//! lives: `Engine::run_pal_*` (headless, sequential `run_string` loop) and
//! the TUI `/pal` slash command (queue-drain via `pending_inputs`).

/// What a single PAL source line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PalLineKind {
    /// Empty or whitespace-only: skipped.
    Empty,
    /// Starts with `#` (after leading whitespace): skipped.
    Comment,
    /// Starts with `/`: slash command.
    Slash,
    /// Starts with `!`: shell command.
    Shell,
    /// Anything else: user message for the agent.
    Message,
}

/// Classify one source line. Leading whitespace is ignored so indented
/// scripts behave the same as flush-left ones; the check is on the first
/// non-whitespace byte.
pub fn classify_line(line: &str) -> PalLineKind {
    let t = line.trim_start();
    if t.is_empty() {
        PalLineKind::Empty
    } else if t.starts_with('#') {
        PalLineKind::Comment
    } else if t.starts_with('/') {
        PalLineKind::Slash
    } else if t.starts_with('!') {
        PalLineKind::Shell
    } else {
        PalLineKind::Message
    }
}

/// True when the line should be executed (slash, shell, or message).
/// Empty lines and comments are skipped.
pub fn is_executable(kind: PalLineKind) -> bool {
    matches!(
        kind,
        PalLineKind::Slash | PalLineKind::Shell | PalLineKind::Message
    )
}

/// One executable PAL step: 1-based source line number plus the trimmed text
/// handed to `Engine::run_string` (or the TUI dispatcher).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PalEntry {
    /// 1-based line number in the source file/content.
    pub lineno: usize,
    /// Trimmed line text (leading/trailing whitespace removed).
    pub text: String,
    /// Classification of the line.
    pub kind: PalLineKind,
}

/// Parse PAL source into its executable steps, skipping empty lines and
/// comments. Line numbers are preserved from the source for error messages.
pub fn parse_content(content: &str) -> Vec<PalEntry> {
    content
        .lines()
        .enumerate()
        .filter_map(|(idx, line)| {
            let kind = classify_line(line);
            if !is_executable(kind) {
                return None;
            }
            let text = line.trim().to_string();
            // `classify_line` guarantees non-empty after trim_start for
            // executable kinds, but `trim` (both ends) is what gets executed.
            if text.is_empty() {
                return None;
            }
            Some(PalEntry {
                lineno: idx + 1,
                text,
                kind,
            })
        })
        .collect()
}

/// A nested `/pal` invocation inside a PAL script. Executing it inline would
/// recurse (`run_pal` → `run_string("/pal …")` → `run_pal` …); both the
/// headless engine and the TUI reject nesting with a friendly message instead.
pub fn is_nested_pal(text: &str) -> bool {
    let t = text.trim_start();
    t == "/pal" || t.starts_with("/pal ")
}

/// Resolve a `/pal` argument to a script path: leading `~` expands to the
/// home directory and relative paths resolve against the current directory.
pub fn resolve_script_path(arg: &str) -> std::path::PathBuf {
    let expanded: std::borrow::Cow<'_, str> = if let Some(after) = arg.strip_prefix('~')
        && (after.is_empty() || after.starts_with('/'))
        && let Some(home) = dirs::home_dir()
    {
        std::borrow::Cow::Owned(format!("{}{after}", home.to_string_lossy()))
    } else {
        std::borrow::Cow::Borrowed(arg)
    };
    let p = std::path::PathBuf::from(expanded.as_ref());
    if p.is_absolute() {
        p
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join(p)
    }
}

// ── headless execution (Engine::run_string as the core) ───────────────────

use super::sink::{EventSink, StringSink};

impl super::Engine {
    /// Execute PAL `content` line by line through [`super::Engine::run_string`]:
    /// empty lines and `#` comments are skipped (by [`parse_content`]), `/`
    /// slash commands and `!` shell commands go through the same handlers as
    /// typed input, and anything else runs as an agent message.
    ///
    /// Continues through errors: a failing line is reported into `sink` and
    /// the script carries on. Returns `(completed_steps, total_steps)`.
    ///
    /// Boxed (rather than a plain `async fn`) because `run_string` can reach
    /// back here via `/pal`, which would otherwise be an infinitely-sized
    /// self-recursive future.
    pub fn run_pal_content<'a>(
        &'a mut self,
        content: &'a str,
        sink: &'a mut StringSink,
    ) -> std::pin::Pin<Box<dyn Future<Output = (usize, usize)> + 'a>> {
        Box::pin(async move {
            let entries = parse_content(content);
            let total = entries.len();
            let mut done = 0usize;
            for entry in &entries {
                if is_nested_pal(&entry.text) {
                    sink.write_error(format!(
                        "line {}: nested /pal is not supported (skipped)",
                        entry.lineno
                    ));
                    continue;
                }
                match self.run_string(&entry.text).await {
                    Ok(out) => {
                        if !out.text.is_empty() {
                            sink.write_line(out.text.as_str());
                        }
                        done += 1;
                    }
                    Err(e) => {
                        sink.write_error(format!("line {}: {:#}", entry.lineno, e));
                    }
                }
            }
            (done, total)
        })
    }

    /// Read the script file at `path` and execute it via [`Self::run_pal_content`].
    /// An unreadable file is reported into `sink` as `(0, 0)` instead of
    /// propagating: callers (slash dispatch, `--pal`) render the sink either way.
    ///
    /// Boxed for the same `/pal`-recursion reason as [`Self::run_pal_content`].
    pub fn run_pal_file<'a>(
        &'a mut self,
        path: &'a std::path::Path,
        sink: &'a mut StringSink,
    ) -> std::pin::Pin<Box<dyn Future<Output = (usize, usize)> + 'a>> {
        Box::pin(async move {
            match std::fs::read_to_string(path) {
                Ok(content) => self.run_pal_content(&content, sink).await,
                Err(e) => {
                    sink.write_error(format!("cannot read {}: {e}", path.display()));
                    (0, 0)
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_empty_lines() {
        assert_eq!(classify_line(""), PalLineKind::Empty);
        assert_eq!(classify_line("   "), PalLineKind::Empty);
        assert_eq!(classify_line("\t  \n"), PalLineKind::Empty);
    }

    #[test]
    fn classify_comment_lines() {
        assert_eq!(classify_line("# hello"), PalLineKind::Comment);
        assert_eq!(classify_line("   # indented"), PalLineKind::Comment);
        assert_eq!(classify_line("#/not-a-command"), PalLineKind::Comment);
    }

    #[test]
    fn classify_slash_shell_message() {
        assert_eq!(classify_line("/help"), PalLineKind::Slash);
        assert_eq!(classify_line("  /model gpt"), PalLineKind::Slash);
        assert_eq!(classify_line("!ls -la"), PalLineKind::Shell);
        assert_eq!(classify_line("  !echo hi"), PalLineKind::Shell);
        assert_eq!(classify_line("hello agent"), PalLineKind::Message);
        assert_eq!(classify_line(".ask what?"), PalLineKind::Message);
    }

    #[test]
    fn parse_skips_empty_and_comments() {
        let entries = parse_content("# top\n\n/hello\n\n  \n!echo hi\nfix bug\n");
        let texts: Vec<&str> = entries.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, vec!["/hello", "!echo hi", "fix bug"]);
        assert_eq!(entries[0].lineno, 3);
        assert_eq!(entries[1].lineno, 6);
        assert_eq!(entries[2].lineno, 7);
    }
}

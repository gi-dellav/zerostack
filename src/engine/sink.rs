//! Output abstraction for agent execution.
//!
//! The TUI renders through [`Renderer`](crate::ui::renderer::Renderer); the
//! headless [`Engine`](super::Engine) collects into a [`StringSink`]. Both
//! implement [`EventSink`] so future slash/agent code can target the trait
//! instead of a concrete renderer.

/// Minimal output surface shared by the TUI renderer and headless runs.
///
/// Deliberately small: plain lines plus semantic helpers (`ok`, `result`,
/// `error`) mirroring `ui::slash::{write_ok, write_result, write_error}`.
/// Streaming tokens go through [`EventSink::token`]; sinks that do not render
/// partial output (e.g. [`StringSink`]) may buffer them until the turn ends.
///
/// Methods take `impl AsRef<str>` so callers can pass `&str`, `String`, or
/// `format!(...)` without borrowing gymnastics.
pub trait EventSink {
    /// Ordinary output line.
    fn write_line(&mut self, text: impl AsRef<str>);
    /// Success-styled line (white in the TUI).
    fn write_ok(&mut self, text: impl AsRef<str>);
    /// Dim/secondary line (dark grey in the TUI).
    fn write_result(&mut self, text: impl AsRef<str>);
    /// Error line (red in the TUI; `StringSink` prefixes `error: `).
    fn write_error(&mut self, text: impl AsRef<str>);
    /// One streamed assistant token. Default: ignore (sinks override when
    /// they render partial output).
    fn token(&mut self, _text: &str) {}
}

/// In-memory [`EventSink`] for [`Engine`](super::Engine) runs.
///
/// Captures every line plus the streamed assistant text so `run_string`
/// can return both a human-readable transcript and the final response.
#[derive(Debug, Default)]
pub struct StringSink {
    lines: Vec<String>,
    response_buf: String,
}

impl StringSink {
    /// Empty sink.
    pub fn new() -> Self {
        Self::default()
    }

    /// All captured lines joined with `\n` (no trailing newline).
    pub fn transcript(&self) -> String {
        self.lines.join("\n")
    }

    /// Streamed assistant tokens accumulated via [`EventSink::token`].
    pub fn response_text(&self) -> &str {
        &self.response_buf
    }

    /// Captured lines (one entry per visual line).
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    fn push(&mut self, text: &str) {
        // Keep one entry per visual line so assertions stay stable even when
        // callers pass multi-line strings.
        if text.is_empty() {
            self.lines.push(String::new());
        } else {
            for line in text.split('\n') {
                self.lines.push(line.to_string());
            }
        }
    }
}

impl EventSink for StringSink {
    fn write_line(&mut self, text: impl AsRef<str>) {
        self.push(text.as_ref());
    }

    fn write_ok(&mut self, text: impl AsRef<str>) {
        self.push(text.as_ref());
    }

    fn write_result(&mut self, text: impl AsRef<str>) {
        self.push(text.as_ref());
    }

    fn write_error(&mut self, text: impl AsRef<str>) {
        self.push(&format!("error: {}", text.as_ref()));
    }

    fn token(&mut self, text: &str) {
        self.response_buf.push_str(text);
    }
}

// Adapter so TUI code can target `EventSink` while the slash handlers are
// migrated off `&mut Renderer` one command at a time. Colors mirror the
// `C_AGENT` / `C_RESULT` / `C_ERROR` convention in `ui::slash`.
impl EventSink for crate::ui::renderer::Renderer {
    fn write_line(&mut self, text: impl AsRef<str>) {
        let _ = self.write_line(text.as_ref(), crossterm::style::Color::White);
    }

    fn write_ok(&mut self, text: impl AsRef<str>) {
        let _ = self.write_line(text.as_ref(), crossterm::style::Color::White);
    }

    fn write_result(&mut self, text: impl AsRef<str>) {
        let _ = self.write_line(text.as_ref(), crossterm::style::Color::DarkGrey);
    }

    fn write_error(&mut self, text: impl AsRef<str>) {
        let _ = self.write_line(
            &format!("error: {}", text.as_ref()),
            crossterm::style::Color::Red,
        );
    }

    fn token(&mut self, text: &str) {
        let _ = self.write(text, crossterm::style::Color::White);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_sink_joins_lines_and_tokens() {
        let mut sink = StringSink::new();
        sink.write_ok("hello");
        sink.write_error("boom");
        sink.token("a");
        sink.token("b");
        assert_eq!(sink.transcript(), "hello\nerror: boom");
        assert_eq!(sink.response_text(), "ab");
    }

    #[test]
    fn string_sink_splits_multiline_writes() {
        let mut sink = StringSink::new();
        sink.write_line("a\nb");
        assert_eq!(sink.lines(), &["a".to_string(), "b".to_string()]);
    }
}

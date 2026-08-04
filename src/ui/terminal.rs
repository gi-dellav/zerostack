use std::env;
use std::io::Write;

use crossterm::ExecutableCommand;
use crossterm::cursor::MoveTo;
use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, EnableBracketedPaste, EnableFocusChange,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal;

/// Scoped terminal setup for the inline UI: raw mode, bracketed paste, and
/// keyboard enhancement flags. No alternate screen — output prints inline so
/// the terminal's native scrollback and text selection keep working.
pub struct TerminalGuard;

impl TerminalGuard {
    pub fn new() -> std::io::Result<Self> {
        let mut stdout = std::io::stdout();
        stdout.execute(EnableBracketedPaste)?;
        if env::var_os("ZEROSTACK_DISABLE_FOCUS").is_none() {
            stdout.execute(EnableFocusChange)?;
        }
        let _ = stdout.execute(PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES,
        ));
        terminal::enable_raw_mode()?;
        // Anchor the UI to the bottom of the screen unless the user opts into
        // pure inline mode. In inline mode the UI prints below the shell prompt
        // and relies on the terminal's native scrollback, which some terminals
        // (e.g. Ghostty) handle more robustly.
        if env::var_os("ZEROSTACK_NO_ANCHOR").is_none() {
            let (_, rows) = terminal::size().unwrap_or((80, 24));
            stdout.execute(MoveTo(0, rows.saturating_sub(1)))?;
        }
        Ok(TerminalGuard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let mut stdout = std::io::stdout();
        let _ = stdout.execute(PopKeyboardEnhancementFlags);
        let _ = stdout.execute(DisableBracketedPaste);
        if env::var_os("ZEROSTACK_DISABLE_FOCUS").is_none() {
            let _ = stdout.execute(DisableFocusChange);
        }
        let _ = stdout.flush();
    }
}

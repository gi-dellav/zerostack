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
        stdout.execute(EnableFocusChange)?;
        let _ = stdout.execute(PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES,
        ));
        terminal::enable_raw_mode()?;
        // Anchor the UI to the bottom of the screen: committed lines scroll up
        // into native scrollback from here, and the live block stays on the
        // bottom rows where the pickers expect it.
        let (_, rows) = terminal::size().unwrap_or((80, 24));
        stdout.execute(MoveTo(0, rows.saturating_sub(1)))?;
        Ok(TerminalGuard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let mut stdout = std::io::stdout();
        let _ = stdout.execute(PopKeyboardEnhancementFlags);
        let _ = stdout.execute(DisableBracketedPaste);
        let _ = stdout.execute(DisableFocusChange);
        let _ = stdout.flush();
    }
}

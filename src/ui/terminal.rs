use std::io::Write;

use crossterm::ExecutableCommand;
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};

pub struct TerminalGuard;

impl TerminalGuard {
    pub fn new() -> std::io::Result<Self> {
        let mut stdout = std::io::stdout();
        stdout.execute(EnterAlternateScreen)?;
        stdout.execute(Clear(ClearType::All))?;
        stdout.execute(EnableBracketedPaste)?;
        let _ = stdout.execute(PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES,
        ));
        terminal::enable_raw_mode()?;
        Ok(TerminalGuard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let mut stdout = std::io::stdout();
        let _ = stdout.execute(PopKeyboardEnhancementFlags);
        let _ = stdout.execute(DisableBracketedPaste);
        let _ = stdout.execute(LeaveAlternateScreen);
        let _ = stdout.flush();
    }
}

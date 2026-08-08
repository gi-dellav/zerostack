use std::io::Write;

use crossterm::ExecutableCommand;
use crossterm::cursor::Show;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};

pub struct TerminalGuard {
    mouse_capture: bool,
}

impl TerminalGuard {
    pub fn new(mouse_capture: bool) -> std::io::Result<Self> {
        let mut stdout = std::io::stdout();
        stdout.execute(EnterAlternateScreen)?;
        stdout.execute(Clear(ClearType::All))?;
        if mouse_capture {
            stdout.execute(EnableMouseCapture)?;
        }
        stdout.execute(EnableBracketedPaste)?;
        let _ = stdout.execute(PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES,
        ));
        terminal::enable_raw_mode()?;
        Ok(TerminalGuard { mouse_capture })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let mut stdout = std::io::stdout();
        let _ = stdout.execute(PopKeyboardEnhancementFlags);
        let _ = stdout.execute(DisableBracketedPaste);
        if self.mouse_capture {
            let _ = stdout.execute(DisableMouseCapture);
        }
        let _ = stdout.execute(Show);
        let _ = stdout.execute(LeaveAlternateScreen);
        let _ = stdout.flush();
    }
}

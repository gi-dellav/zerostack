use std::io::Write;

use crossterm::ExecutableCommand;
use crossterm::cursor::Show;
use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};

/// RAII guard that suspends the TUI (leaves alternate screen, disables raw mode)
/// and restores it on drop. Ensures the alternate screen is re-entered and raw mode
/// re-enabled even if the editor exits abnormally.
pub struct SuspendGuard {
    mouse_capture: bool,
}

impl SuspendGuard {
    pub fn suspend(mouse_capture: bool) -> Self {
        let _ = terminal::disable_raw_mode();
        let mut stdout = std::io::stdout();
        if mouse_capture {
            let _ = stdout.execute(DisableMouseCapture);
        }
        let _ = stdout.execute(LeaveAlternateScreen);
        let _ = stdout.flush();
        Self { mouse_capture }
    }
}

impl Drop for SuspendGuard {
    fn drop(&mut self) {
        let mut stdout = std::io::stdout();
        let _ = stdout.execute(EnterAlternateScreen);
        let _ = stdout.execute(Clear(ClearType::All));
        if self.mouse_capture {
            let _ = stdout.execute(EnableMouseCapture);
        }
        let _ = terminal::enable_raw_mode();
        let _ = stdout.flush();
    }
}

/// Suspend the TUI, run `f`, then restore it. Single shared helper to avoid
/// copy-pasted `disable_raw_mode` / `LeaveAlternateScreen` sequences.
pub fn suspend_tui<F, R>(mouse_capture: bool, f: F) -> R
where
    F: FnOnce() -> R,
{
    let _guard = SuspendGuard::suspend(mouse_capture);
    f()
}

/// Split an editor command string (e.g. `"code --wait"`) into program + args,
/// respecting shell quoting. Falls back to treating the whole string as a single
/// program name if splitting fails or yields empty.
pub fn parse_editor_command(editor: &str) -> (String, Vec<String>) {
    match shell_words::split(editor) {
        Ok(mut parts) if !parts.is_empty() => {
            let prog = parts.remove(0);
            (prog, parts)
        }
        _ => (editor.to_string(), Vec::new()),
    }
}

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
        stdout.execute(EnableFocusChange)?;
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
        let _ = stdout.execute(DisableFocusChange);
        if self.mouse_capture {
            let _ = stdout.execute(DisableMouseCapture);
        }
        let _ = stdout.execute(Show);
        let _ = stdout.execute(LeaveAlternateScreen);
        let _ = stdout.flush();
    }
}

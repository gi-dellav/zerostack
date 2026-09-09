#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use std::os::unix::net::UnixStream;

#[derive(Clone)]
pub struct StatusSignals {
    path: String,
}

/// Reasons zerostack can be blocked waiting on something external.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockedReason {
    Permission,
}

impl BlockedReason {
    fn line(self) -> &'static str {
        match self {
            BlockedReason::Permission => "blocked:permission\n",
        }
    }
}

/// Run states zerostack can report once a block has ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunState {
    Working,
}

impl RunState {
    fn line(self) -> &'static str {
        match self {
            RunState::Working => "state:working\n",
        }
    }
}

/// RAII guard that reports `blocked:<reason>` on construction and
/// `state:working` on drop, keeping the pair balanced across every exit
/// path (including early returns via `?`).
#[must_use = "dropping the guard immediately sends state:working right after blocked:permission; bind it to a local"]
pub struct BlockedScope<'a> {
    signals: &'a StatusSignals,
}

impl Drop for BlockedScope<'_> {
    fn drop(&mut self) {
        self.signals.send_state(RunState::Working);
    }
}

impl StatusSignals {
    #[allow(dead_code)]
    pub fn new(path: String) -> Self {
        Self { path }
    }

    #[cfg(unix)]
    fn send_line(&self, line: &str) {
        let _ = (|| -> std::io::Result<()> {
            let mut stream = UnixStream::connect(&self.path)?;
            stream.write_all(line.as_bytes())?;
            Ok(())
        })();
    }

    #[cfg(not(unix))]
    fn send_line(&self, _line: &str) {}

    pub fn send_start(&self) {
        self.send_line("start\n");
    }

    pub fn send_stop(&self) {
        self.send_line("stop\n");
    }

    #[allow(dead_code)]
    pub fn send_git_conflict(&self) {
        self.send_line("git-conflict\n");
    }

    /// Raw sender for `blocked:<reason>`. TUI code must go through
    /// `blocked_scope` instead so the `blocked:`/`state:` pair stays
    /// balanced; this exists as a raw sender because later waits (chain)
    /// span several event-loop turns and cannot hold a guard across them.
    pub fn send_blocked(&self, reason: BlockedReason) {
        self.send_line(reason.line());
    }

    /// Raw sender for `state:<state>`. TUI code must go through
    /// `blocked_scope` instead so the `blocked:`/`state:` pair stays
    /// balanced; this exists as a raw sender because later waits (chain)
    /// span several event-loop turns and cannot hold a guard across them.
    pub fn send_state(&self, state: RunState) {
        self.send_line(state.line());
    }

    /// Permission waits always resume the same run, which is why the guard
    /// releases with `RunState::Working` while `send_state` stays
    /// parameterised for the reserved `idle` state.
    pub fn blocked_scope(&self, reason: BlockedReason) -> BlockedScope<'_> {
        self.send_blocked(reason);
        BlockedScope { signals: self }
    }
}

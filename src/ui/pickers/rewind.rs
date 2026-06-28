use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::ui::pickers::list::ListPicker;

/// What the user settled on after driving the rewind picker to completion.
pub enum RewindOutcome {
    /// Rewind the conversation to just before the message at this index.
    Confirmed(usize),
    /// Back out without changing anything.
    Cancelled,
}

enum Stage {
    /// Choosing which earlier user turn to rewind to.
    PickMessage,
    /// Confirming the rewind for the chosen turn.
    Confirm,
}

const CONFIRM_REWIND: &str = "Rewind and start from here";
const CONFIRM_CANCEL: &str = "Cancel";

/// A two-level modal picker for the double-Esc rewind: first pick an earlier
/// user turn, then confirm the (destructive) rewind. Reuses [`ListPicker`] for
/// rendering and selection at each level; the outcome is read back by the event
/// loop, which owns the session and performs the actual rewind.
pub struct RewindPicker {
    active: bool,
    stage: Stage,
    list: ListPicker,
    /// `(message_index, preview)` for each rewind-able user turn, in order. The
    /// list shows the previews, so `list.selected` indexes straight into this.
    targets: Vec<(usize, String)>,
    chosen: Option<usize>,
    outcome: Option<RewindOutcome>,
}

impl RewindPicker {
    pub fn new(targets: Vec<(usize, String)>) -> Self {
        let mut list = ListPicker::new();
        list.set_items(targets.iter().map(|(_, p)| p.clone()).collect());
        RewindPicker {
            active: false,
            stage: Stage::PickMessage,
            list,
            targets,
            chosen: None,
            outcome: None,
        }
    }

    pub fn set_monochrome(&mut self, monochrome: bool) {
        self.list.set_monochrome(monochrome);
    }

    pub fn activate(&mut self) {
        self.active = true;
        self.stage = Stage::PickMessage;
        self.list.activate();
    }

    pub fn active(&self) -> bool {
        self.active
    }

    pub fn take_outcome(&mut self) -> Option<RewindOutcome> {
        self.outcome.take()
    }

    pub fn draw(&self) -> std::io::Result<()> {
        self.list.draw(None)
    }

    /// Handle a key while the picker is open. Always returns `true`: the picker
    /// is modal, so it swallows every keystroke until it resolves.
    pub fn handle(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Up => self.list.select_prev(),
            KeyCode::Down => self.list.select_next(),
            KeyCode::Tab => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.list.select_prev();
                } else {
                    self.list.select_next();
                }
            }
            KeyCode::BackTab => self.list.select_prev(),
            KeyCode::Enter => self.confirm(),
            KeyCode::Esc => self.back_or_cancel(),
            _ => {}
        }
        true
    }

    fn confirm(&mut self) {
        match self.stage {
            Stage::PickMessage => {
                if let Some(&(idx, _)) = self.targets.get(self.list.selected) {
                    self.chosen = Some(idx);
                    self.stage = Stage::Confirm;
                    self.list
                        .set_items(vec![CONFIRM_REWIND.to_string(), CONFIRM_CANCEL.to_string()]);
                    self.list.activate();
                }
            }
            Stage::Confirm => {
                self.outcome = Some(if self.list.selected == 0 {
                    RewindOutcome::Confirmed(self.chosen.unwrap_or(0))
                } else {
                    RewindOutcome::Cancelled
                });
                self.finish();
            }
        }
    }

    fn back_or_cancel(&mut self) {
        match self.stage {
            // Esc at the confirm step backs up to the message list rather than
            // bailing out entirely, so a mis-step is cheap to correct.
            Stage::Confirm => {
                self.stage = Stage::PickMessage;
                self.list
                    .set_items(self.targets.iter().map(|(_, p)| p.clone()).collect());
                self.list.activate();
                if let Some(pos) = self
                    .chosen
                    .and_then(|idx| self.targets.iter().position(|(i, _)| *i == idx))
                {
                    self.list.selected = pos;
                }
            }
            Stage::PickMessage => {
                self.outcome = Some(RewindOutcome::Cancelled);
                self.finish();
            }
        }
    }

    fn finish(&mut self) {
        self.active = false;
        self.list.deactivate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn picker() -> RewindPicker {
        let mut p = RewindPicker::new(vec![(1, "first".to_string()), (3, "second".to_string())]);
        p.activate();
        p
    }

    #[test]
    fn enter_then_confirm_yields_the_chosen_message_index() {
        let mut p = picker();
        p.handle(key(KeyCode::Down)); // select second target (message index 3)
        p.handle(key(KeyCode::Enter)); // open confirm step
        assert!(matches!(p.stage, Stage::Confirm));
        p.handle(key(KeyCode::Enter)); // confirm "Rewind and start from here"
        assert!(matches!(
            p.take_outcome(),
            Some(RewindOutcome::Confirmed(3))
        ));
        assert!(!p.active());
    }

    #[test]
    fn confirm_step_cancel_option_backs_out() {
        let mut p = picker();
        p.handle(key(KeyCode::Enter)); // confirm step for first target
        p.handle(key(KeyCode::Down)); // move to "Cancel"
        p.handle(key(KeyCode::Enter));
        assert!(matches!(p.take_outcome(), Some(RewindOutcome::Cancelled)));
        assert!(!p.active());
    }

    #[test]
    fn esc_at_confirm_returns_to_message_list_without_resolving() {
        let mut p = picker();
        p.handle(key(KeyCode::Enter)); // into confirm
        p.handle(key(KeyCode::Esc)); // back to message list
        assert!(matches!(p.stage, Stage::PickMessage));
        assert!(p.take_outcome().is_none());
        assert!(p.active());
    }

    #[test]
    fn esc_at_message_list_cancels() {
        let mut p = picker();
        p.handle(key(KeyCode::Esc));
        assert!(matches!(p.take_outcome(), Some(RewindOutcome::Cancelled)));
        assert!(!p.active());
    }
}

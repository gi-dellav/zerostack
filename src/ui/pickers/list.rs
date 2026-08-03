use super::{picker_list_view, truncate_item};
use crate::ui::renderer::PickerView;

/// Slash commands that are always available, regardless of which optional
/// features were compiled in. Feature-gated commands are appended by
/// [`available_commands`].
///
/// Kept in alphabetical order for ease of maintenance.
const BASE_COMMANDS: &[&str] = &[
    "/add",
    "/btw",
    "/clear",
    "/compact",
    "/compress",
    "/drop",
    "/drop-all",
    "/editsys",
    "/exit",
    "/help",
    "/history",
    "/init",
    "/mode",
    "/model",
    "/models",
    "/models-add",
    "/new",
    "/prompt",
    "/provider",
    "/queue",
    "/quit",
    "/reasoning",
    "/redo",
    "/rename",
    "/regen-prompts",
    "/regen-themes",
    "/retry",
    "/review",
    "/rewind",
    "/sessions",
    "/theme",
    "/thinking",
    "/toggle",
    "/tutor",
    "/tutorial",
    "/undo",
    "/welcome",
];

/// Build the autocomplete command list, including only the commands whose
/// backing feature was actually compiled in.
///
/// `#[cfg]` cannot be attached to elements of an array literal on stable Rust
/// (that requires the unstable `stmt_expr_attributes` feature), so the
/// feature-gated commands are appended via conditionally-compiled statements
/// instead. Feature blocks are ordered alphabetically by feature name, and the
/// commands within each block are likewise alphabetical. Keep this in sync with
/// the dispatcher in `crate::ui::slash`.
fn available_commands() -> Vec<&'static str> {
    #[allow(unused_mut)]
    let mut cmds: Vec<&'static str> = BASE_COMMANDS.to_vec();

    #[cfg(feature = "advisor")]
    cmds.push("/advisor");

    #[cfg(feature = "export")]
    {
        cmds.push("/export");
        cmds.push("/import");
        cmds.push("/share");
    }

    #[cfg(feature = "git-worktree")]
    {
        cmds.push("/worktree");
        cmds.push("/wt-exit");
        cmds.push("/wt-merge");
    }

    #[cfg(feature = "hooks")]
    cmds.push("/hooks");

    #[cfg(feature = "loop")]
    cmds.push("/loop");

    #[cfg(feature = "mcp")]
    cmds.push("/mcp");

    #[cfg(feature = "memory")]
    cmds.push("/memory");

    #[cfg(feature = "subagents")]
    {
        cmds.push("/model-subagent");
        cmds.push("/models-subagent");
    }

    cmds
}

pub struct ListPicker {
    pub active: bool,
    pub query: String,
    pub cursor: usize,
    pub matches: Vec<String>,
    pub selected: usize,
    items: Vec<String>,
}

impl ListPicker {
    pub fn new() -> Self {
        ListPicker {
            active: false,
            query: String::new(),
            cursor: 0,
            matches: Vec::new(),
            selected: 0,
            items: Vec::new(),
        }
    }

    pub fn with_static_commands() -> Self {
        let mut picker = ListPicker::new();
        picker.items = available_commands().iter().map(|s| s.to_string()).collect();
        picker
    }

    pub fn set_items(&mut self, items: Vec<String>) {
        self.items = items;
    }

    pub fn activate(&mut self) {
        self.active = true;
        self.query.clear();
        self.cursor = 0;
        self.matches.clear();
        self.selected = 0;
        self.filter();
    }

    pub fn deactivate(&mut self) {
        self.active = false;
    }

    pub fn char_input(&mut self, c: char) {
        let byte_pos = self
            .query
            .char_indices()
            .nth(self.cursor)
            .map(|(i, _)| i)
            .unwrap_or(self.query.len());
        self.query.insert(byte_pos, c);
        self.cursor += 1;
        self.filter();
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 && !self.query.is_empty() {
            self.cursor -= 1;
            let byte_pos = self
                .query
                .char_indices()
                .nth(self.cursor)
                .map(|(i, _)| i)
                .unwrap_or(self.query.len());
            self.query.remove(byte_pos);
            self.filter();
        }
    }

    fn filter(&mut self) {
        let query_lower = self.query.to_lowercase();
        self.matches = self
            .items
            .iter()
            .filter(|name| name.to_lowercase().contains(&query_lower))
            .take(50)
            .cloned()
            .collect();
        self.selected = 0;
    }

    pub fn select_next(&mut self) {
        if !self.matches.is_empty() {
            self.selected = (self.selected + 1) % self.matches.len();
        }
    }

    pub fn select_prev(&mut self) {
        if !self.matches.is_empty() {
            self.selected = if self.selected == 0 {
                self.matches.len() - 1
            } else {
                self.selected - 1
            };
        }
    }

    pub fn selected_name(&self) -> Option<&str> {
        self.matches.get(self.selected).map(|s| s.as_str())
    }

    /// The picker overlay as live-block rows, or `None` when inactive.
    /// `reserved` counts the rows below the picker (input + separators +
    /// statusline) so the list fits the remaining screen.
    pub fn view(&self, empty_message: Option<&str>, reserved: u16) -> Option<PickerView> {
        if !self.active {
            return None;
        }
        let (cols, rows) = crossterm::terminal::size().ok()?;
        let max_items = (rows.saturating_sub(reserved)).min(10) as usize;
        let matches: Vec<String> = self
            .matches
            .iter()
            .map(|m| truncate_item(m, cols))
            .collect();
        Some(picker_list_view(
            &matches,
            self.selected,
            max_items,
            empty_message,
        ))
    }
}

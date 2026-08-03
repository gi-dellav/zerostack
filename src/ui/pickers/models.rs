use super::{fuzzy_score, picker_window, truncate_item};
use crate::ui::renderer::PickerView;

pub struct ModelsPicker {
    pub active: bool,
    pub query: String,
    pub cursor: usize,
    pub matches: Vec<String>,
    pub selected: usize,
    quick: Vec<String>,
    provider: Vec<String>,
    pub group: usize,
}

impl ModelsPicker {
    pub fn new() -> Self {
        ModelsPicker {
            active: false,
            query: String::new(),
            cursor: 0,
            matches: Vec::new(),
            selected: 0,
            quick: Vec::new(),
            provider: Vec::new(),
            group: 0,
        }
    }

    pub fn set_groups(&mut self, quick: Vec<String>, provider: Vec<String>) {
        self.quick = quick;
        self.provider = provider;
    }

    pub fn activate(&mut self) {
        self.active = true;
        self.query.clear();
        self.cursor = 0;
        self.matches.clear();
        self.selected = 0;
        self.group = if self.quick.is_empty() && !self.provider.is_empty() {
            1
        } else {
            0
        };
        self.filter();
    }

    pub fn deactivate(&mut self) {
        self.active = false;
    }

    pub fn toggle_group(&mut self) {
        self.group = 1 - self.group;
        self.selected = 0;
        self.filter();
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
        let src = if self.group == 0 {
            &self.quick
        } else {
            &self.provider
        };
        let mut scored: Vec<(i32, &String)> = src
            .iter()
            .filter_map(|n| fuzzy_score(n, &self.query).map(|s| (s, n)))
            .collect();
        scored.sort_by_key(|b| std::cmp::Reverse(b.0));
        self.matches = scored
            .into_iter()
            .take(50)
            .map(|(_, n)| n.clone())
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

    /// The picker overlay as live-block rows (group-tab header above the
    /// list), or `None` when inactive.
    pub fn view(&self, reserved: u16) -> Option<PickerView> {
        if !self.active {
            return None;
        }
        let (cols, rows) = crossterm::terminal::size().ok()?;

        let header = (rows >= 8).then(|| {
            let tab = |label: &str, count: usize, active: bool| {
                if active {
                    format!("[{} {}]", label, count)
                } else {
                    format!(" {} {} ", label, count)
                }
            };
            format!(
                "{}  {}   (Tab to switch · /models refresh for the latest)",
                tab("Quick", self.quick.len(), self.group == 0),
                tab("Provider", self.provider.len(), self.group == 1)
            )
        });

        // One extra row for the tab header above the list.
        let max_items = (rows.saturating_sub(reserved + 1)).min(10) as usize;
        let matches: Vec<String> = self
            .matches
            .iter()
            .map(|m| truncate_item(m, cols))
            .collect();
        let rows_out = if matches.is_empty() {
            vec![crate::ui::renderer::PickerRow {
                text: "no matches".to_string(),
                selected: false,
            }]
        } else {
            picker_window(&matches, self.selected, max_items)
        };
        Some(PickerView {
            header,
            rows: rows_out,
        })
    }
}

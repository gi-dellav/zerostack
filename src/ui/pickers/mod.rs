pub(crate) mod file;
pub(crate) mod handlers;
pub(crate) mod list;
pub(crate) mod models;
pub(crate) mod rewind;

use super::renderer::{PickerRow, PickerView};

/// Truncate a picker item to the display width the old direct-stdout painting
/// used (three columns of slack for the selection marker).
pub(crate) fn truncate_item(s: &str, cols: u16) -> String {
    s.chars().take(cols.saturating_sub(3) as usize).collect()
}

/// Window `matches` around `selected`, capped at `max_items`, as picker rows
/// with the selection marker baked into the text. Pure so the geometry is
/// unit-testable without a terminal.
pub(crate) fn picker_window(
    matches: &[String],
    selected: usize,
    max_items: usize,
) -> Vec<PickerRow> {
    let list_height = max_items.min(matches.len());
    let start_idx = selected
        .saturating_sub(list_height / 2)
        .min(matches.len().saturating_sub(list_height));
    let end_idx = (start_idx + list_height).min(matches.len());
    matches[start_idx..end_idx]
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let selected = start_idx + i == selected;
            PickerRow {
                text: if selected {
                    format!("▸ {item}")
                } else {
                    format!("  {item}")
                },
                selected,
            }
        })
        .collect()
}

/// Build a list view: the windowed matches, or a single dim message row when
/// there is nothing to show.
pub(crate) fn picker_list_view(
    matches: &[String],
    selected: usize,
    max_items: usize,
    empty_message: Option<&str>,
) -> PickerView {
    if matches.is_empty() {
        return PickerView {
            header: None,
            rows: vec![PickerRow {
                text: empty_message.unwrap_or("no matches").to_string(),
                selected: false,
            }],
        };
    }
    PickerView {
        header: None,
        rows: picker_window(matches, selected, max_items),
    }
}

pub(crate) fn fuzzy_score(item: &str, query: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let item_l = item.to_lowercase();
    let query_l = query.to_lowercase();
    let is_boundary = |bytes: &[u8], pos: usize| -> bool {
        pos == 0
            || matches!(
                bytes.get(pos - 1),
                Some(b'-' | b'.' | b'/' | b'_' | b' ' | b':')
            )
    };

    if let Some(pos) = item_l.find(&query_l) {
        let mut score = 1000;
        if is_boundary(item_l.as_bytes(), pos) {
            score += 200;
        }
        if pos == 0 {
            score += 100;
        }
        score -= pos as i32;
        score -= (item_l.chars().count() / 4) as i32;
        return Some(score);
    }

    let chars: Vec<char> = item_l.chars().collect();
    let mut score = 0i32;
    let mut idx = 0usize;
    let mut last: Option<usize> = None;
    for qc in query_l.chars() {
        let mut pos = None;
        while idx < chars.len() {
            if chars[idx] == qc {
                pos = Some(idx);
                break;
            }
            idx += 1;
        }
        let pos = pos?;
        if last == Some(pos.wrapping_sub(1)) {
            score += 5;
        }
        if pos == 0 || matches!(chars.get(pos - 1), Some('-' | '.' | '/' | '_' | ' ' | ':')) {
            score += 3;
        }
        last = Some(pos);
        idx = pos + 1;
    }
    score -= (chars.len() / 20) as i32;
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("item {i}")).collect()
    }

    #[test]
    fn window_shows_all_when_under_cap() {
        let rows = picker_window(&matches(3), 1, 10);
        assert_eq!(rows.len(), 3);
        assert!(rows[1].selected);
        assert_eq!(rows[1].text, "▸ item 1");
        assert_eq!(rows[0].text, "  item 0");
    }

    #[test]
    fn window_caps_at_max_items() {
        let rows = picker_window(&matches(50), 0, 10);
        assert_eq!(rows.len(), 10);
        assert!(rows[0].selected);
    }

    #[test]
    fn window_centers_the_selection() {
        let rows = picker_window(&matches(50), 25, 10);
        // Selected item sits in the middle of the window.
        let pos = rows.iter().position(|r| r.selected).unwrap();
        assert_eq!(rows[pos].text, "▸ item 25");
        assert!(pos >= 4 && pos <= 5, "centered: pos {pos}");
    }

    #[test]
    fn window_clamps_at_the_end() {
        let rows = picker_window(&matches(12), 11, 10);
        assert_eq!(rows.len(), 10);
        assert!(rows[9].selected);
        assert_eq!(rows[9].text, "▸ item 11");
    }

    #[test]
    fn empty_matches_yield_a_single_message_row() {
        let view = picker_list_view(&[], 0, 10, Some("nothing here"));
        assert_eq!(view.height(), 1);
        assert_eq!(view.rows[0].text, "nothing here");
        assert!(!view.rows[0].selected);
    }

    #[test]
    fn empty_matches_default_message() {
        let view = picker_list_view(&[], 0, 10, None);
        assert_eq!(view.rows[0].text, "no matches");
    }

    #[test]
    fn view_height_counts_header() {
        let mut view = picker_list_view(&matches(2), 0, 10, None);
        assert_eq!(view.height(), 2);
        view.header = Some("tabs".to_string());
        assert_eq!(view.height(), 3);
    }
}

pub(crate) mod file;
pub(crate) mod handlers;
pub(crate) mod list;
pub(crate) mod models;

use std::io::Write;

use crossterm::ExecutableCommand;
use crossterm::cursor::MoveTo;
use crossterm::style::{ResetColor, SetForegroundColor};
use crossterm::terminal::Clear;

use super::utils::{UiColors, resolve_color};

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

pub(crate) fn draw_picker_list(
    matches: &[String],
    selected: usize,
    monochrome: bool,
    empty_message: Option<&str>,
    bottom_reserved: u16,
    colors: &UiColors,
) -> std::io::Result<()> {
    let (cols, rows) = crossterm::terminal::size()?;
    let mut stdout = std::io::stdout();

    let max_items = (rows.saturating_sub(bottom_reserved)).min(10) as usize;

    if matches.is_empty() {
        let r = rows.saturating_sub(3);
        stdout.execute(MoveTo(0, r))?;
        let color = resolve_color(colors.picker_secondary, monochrome);
        write!(stdout, "{}", SetForegroundColor(color))?;
        write!(stdout, "{}", empty_message.unwrap_or("no matches"))?;
        write!(stdout, "{}", ResetColor)?;
        stdout.flush()?;
        return Ok(());
    }

    let list_height = max_items.min(matches.len());
    let start_idx = selected
        .saturating_sub(list_height / 2)
        .min(matches.len().saturating_sub(list_height));
    let end_idx = (start_idx + list_height).min(matches.len());

    let top_row = rows.saturating_sub(3).saturating_sub(list_height as u16);

    for (i, item) in matches
        .iter()
        .enumerate()
        .skip(start_idx)
        .take(end_idx - start_idx)
    {
        let render_row = top_row + (i - start_idx) as u16;
        stdout.execute(MoveTo(0, render_row))?;
        write!(
            stdout,
            "{}",
            Clear(crossterm::terminal::ClearType::CurrentLine)
        )?;

        let truncated: String = item.chars().take(cols.saturating_sub(3) as usize).collect();

        if i == selected {
            write!(
                stdout,
                "{}",
                SetForegroundColor(resolve_color(colors.picker_selected, monochrome))
            )?;
            write!(stdout, "▸ {}", truncated)?;
        } else {
            write!(
                stdout,
                "{}",
                SetForegroundColor(resolve_color(colors.picker_secondary, monochrome))
            )?;
            write!(stdout, "  {}", truncated)?;
        }
        write!(stdout, "{}", ResetColor)?;
    }
    stdout.flush()?;
    Ok(())
}

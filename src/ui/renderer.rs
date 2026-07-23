use std::io::{self, Write};
use std::sync::LazyLock;

use compact_str::CompactString;
use crossterm::ExecutableCommand;
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::style::{
    Attribute, Color, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
};
use crossterm::terminal::{Clear, ClearType};
use regex::Regex;
use smallvec::SmallVec;

use super::feed::{BlockStyle, Feed, style_from_color};
use super::markdown::word_wrap;
use super::statusline::StatusSpan;
use super::utils::{char_display_width, display_width, resolve_color};

static URL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"https?://[^\x00-\x1f\x7f\s<>]+").expect("compile URL regex"));

fn wrap_urls_osc8(text: &str) -> String {
    let mut result = String::with_capacity(text.len() + 64);
    let mut last = 0;
    for m in URL_RE.find_iter(text) {
        result.push_str(&text[last..m.start()]);
        result.push_str("\x1b]8;;");
        result.push_str(m.as_str());
        result.push_str("\x1b\\");
        result.push_str(m.as_str());
        result.push_str("\x1b]8;;\x1b\\");
        last = m.end();
    }
    result.push_str(&text[last..]);
    result
}

#[derive(Clone, Debug)]
pub struct LineEntry {
    pub text: CompactString,
    pub color: Color,
}

pub struct PermissionPrompt {
    pub tool: CompactString,
    pub options: CompactString,
}

pub struct ChainPrompt {
    pub question: CompactString,
}

/// Everything that affects what the chat viewport paints. Compared between
/// frames to decide whether `render_viewport` can skip drawing.
#[derive(Clone, PartialEq)]
struct ChatSnapshot {
    feed_generation: u64,
    width: usize,
    visible_rows: usize,
    scroll_offset: usize,
    selection_active: bool,
    selection_start: Option<usize>,
    selection_end: Option<usize>,
    partial: CompactString,
    partial_style: BlockStyle,
    chat_bg: Option<Color>,
    monochrome: bool,
}

/// Which prompt mode `draw_bottom` paints in the input area.
#[derive(Clone, PartialEq)]
pub(crate) enum PromptSnapshot {
    Input,
    Permission {
        tool: CompactString,
        options: CompactString,
    },
    Chain {
        question: CompactString,
        but_mode: bool,
    },
}

/// Everything `draw_bottom` paints, compared between frames to decide how much
/// of the bottom region (input area + statusline) needs repainting.
#[derive(Clone, PartialEq)]
pub(crate) struct BottomSnapshot {
    pub(crate) cols: u16,
    pub(crate) rows: u16,
    pub(crate) statusline_height: usize,
    pub(crate) input: String,
    pub(crate) cursor_pos: usize,
    pub(crate) is_running: bool,
    pub(crate) spinner_frame: u8,
    pub(crate) input_vscroll_offset: usize,
    pub(crate) prompt: PromptSnapshot,
    pub(crate) statusline: Vec<Vec<StatusSpan>>,
    pub(crate) scroll_indicator: bool,
    pub(crate) monochrome: bool,
    pub(crate) input_bg: Option<Color>,
    pub(crate) status_bg: Option<Color>,
}

/// How much of the bottom region a `draw_bottom` call must repaint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BottomRedrawPlan {
    /// Nothing changed since the last frame; draw nothing.
    Skip,
    /// Only the statusline content changed; redraw just the statusline rows.
    StatuslineOnly,
    /// Input area (or the geometry it sits in) changed; full bottom redraw.
    Full,
}

pub struct Renderer {
    spinner_frame: u8,
    feed: Feed,
    partial: CompactString,
    partial_style: BlockStyle,
    scroll_offset: usize,
    input_scroll_offset: usize,
    input_vscroll_offset: usize,
    input_max_vscroll: usize,
    last_input_cursor: usize,
    // Geometry of the last-rendered input area, used to map a mouse click to a
    // cursor position inside the input buffer.
    input_base_row: u16,
    input_prompt_width: usize,
    input_first_visible: usize,
    input_visible_line_count: usize,
    input_h_scroll: usize,
    input_cursor_line: usize,
    monochrome: bool,
    chat_bg: Option<Color>,
    input_bg: Option<Color>,
    status_bg: Option<Color>,
    pub selection_active: bool,
    pub selection_start: Option<usize>,
    pub selection_end: Option<usize>,
    prev_input_height: usize,
    /// Number of statusline rows (1-3), fixed by the statusline config at startup.
    statusline_height: usize,
    /// Left padding (columns) for the chat buffer area only. Input and status
    /// rows are unaffected.
    chat_margin: u16,
    pub permission_prompt: Option<PermissionPrompt>,
    pub chain_prompt: Option<ChainPrompt>,
    pub chain_but_mode: bool,
    /// Dirty-region tracking: explicit invalidation flags plus snapshots of
    /// the state recorded after the last successful draw of each region.
    chat_dirty: bool,
    last_chat_snapshot: Option<ChatSnapshot>,
    bottom_dirty: bool,
    last_bottom_snapshot: Option<BottomSnapshot>,
    /// Screen position of the input caret after the last full bottom draw;
    /// `None` when the caret is hidden (permission/chain prompts).
    bottom_cursor: Option<(u16, u16)>,
}

impl Renderer {
    pub fn new() -> io::Result<Self> {
        Ok(Renderer {
            spinner_frame: 0,
            feed: Feed::new(),
            partial: CompactString::new(""),
            partial_style: BlockStyle::Plain,
            scroll_offset: 0,
            input_scroll_offset: 0,
            input_vscroll_offset: 0,
            input_max_vscroll: 0,
            last_input_cursor: 0,
            input_base_row: 0,
            input_prompt_width: 0,
            input_first_visible: 0,
            input_visible_line_count: 0,
            input_h_scroll: 0,
            input_cursor_line: 0,
            monochrome: false,
            chat_bg: None,
            input_bg: None,
            status_bg: None,
            selection_active: false,
            selection_start: None,
            selection_end: None,
            prev_input_height: 0,
            statusline_height: 1,
            chat_margin: 0,
            permission_prompt: None,
            chain_prompt: None,
            chain_but_mode: false,
            chat_dirty: true,
            last_chat_snapshot: None,
            bottom_dirty: true,
            last_bottom_snapshot: None,
            bottom_cursor: None,
        })
    }

    /// Set the number of statusline rows (1-3). Call once at startup.
    pub fn set_statusline_height(&mut self, h: usize) {
        self.statusline_height = h.clamp(1, 3);
    }

    /// Rows reserved at the bottom: statusline lines + separator + input baseline.
    fn statusline_reserve(&self) -> u16 {
        self.statusline_height as u16 + 2
    }

    pub fn set_monochrome(&mut self, monochrome: bool) {
        self.monochrome = monochrome;
    }

    /// Set the chat buffer's left padding in columns. Clamped so content keeps
    /// at least a few usable columns.
    pub fn set_chat_margin(&mut self, margin: u16) {
        let (cols, _) = self.terminal_size();
        self.chat_margin = margin.min(cols.saturating_sub(8));
    }

    /// Emit the chat left-margin gutter (spaces in the chat background) at the
    /// current cursor position. Caller has already positioned to column 0 and
    /// set the background.
    fn write_chat_margin(&self, stdout: &mut impl Write) -> io::Result<()> {
        if self.chat_margin > 0 {
            write!(stdout, "{}", " ".repeat(self.chat_margin as usize))?;
        }
        Ok(())
    }

    pub fn set_background_colors(
        &mut self,
        chat_bg: Option<Color>,
        input_bg: Option<Color>,
        status_bg: Option<Color>,
    ) {
        self.chat_bg = chat_bg;
        self.input_bg = input_bg;
        self.status_bg = status_bg;
    }

    fn color(&self, color: Color) -> Color {
        resolve_color(color, self.monochrome)
    }

    fn terminal_size(&self) -> (u16, u16) {
        crossterm::terminal::size().unwrap_or((80, 24))
    }

    fn max_line_width(&self) -> usize {
        let (cols, _) = self.terminal_size();
        cols.saturating_sub(1 + self.chat_margin) as usize
    }

    #[cfg(test)]
    pub fn line_width(&self) -> usize {
        self.max_line_width()
    }

    fn chat_lines(&self, width: usize) -> Vec<LineEntry> {
        let mut lines = self.feed.lines(width);
        if !self.partial.is_empty() {
            let color = self.partial_style.color();
            for chunk in word_wrap(&self.partial, width) {
                lines.push(LineEntry { text: chunk, color });
            }
        }
        lines
    }

    pub fn buffer_len(&self) -> usize {
        self.chat_lines(self.max_line_width()).len()
    }

    /// Access the underlying feed for callers that want to push semantic blocks
    /// directly (e.g., session rendering or streaming agent responses).
    pub fn feed(&self) -> &Feed {
        &self.feed
    }

    pub fn feed_mut(&mut self) -> &mut Feed {
        &mut self.feed
    }

    /// Snapshot of everything the chat viewport currently paints.
    fn chat_snapshot(&self) -> ChatSnapshot {
        ChatSnapshot {
            feed_generation: self.feed.generation(),
            width: self.max_line_width(),
            visible_rows: self.visible_lines(),
            scroll_offset: self.scroll_offset,
            selection_active: self.selection_active,
            selection_start: self.selection_start,
            selection_end: self.selection_end,
            partial: self.partial.clone(),
            partial_style: self.partial_style,
            chat_bg: self.chat_bg,
            monochrome: self.monochrome,
        }
    }

    /// Whether the chat viewport needs repainting: either a renderer-internal
    /// mutation marked it dirty, or the tracked state changed since the last
    /// recorded draw. The state comparison also catches feed mutations made
    /// through `feed_mut()` and direct writes to the public selection fields.
    pub fn chat_needs_redraw(&self) -> bool {
        if self.chat_dirty {
            return true;
        }
        match &self.last_chat_snapshot {
            Some(prev) => *prev != self.chat_snapshot(),
            None => true,
        }
    }

    /// Record the current chat state as freshly drawn.
    fn record_chat_drawn(&mut self) {
        self.last_chat_snapshot = Some(self.chat_snapshot());
        self.chat_dirty = false;
    }

    /// Test helper: mark the chat viewport clean without drawing, as if a
    /// `render_viewport` had just completed.
    #[cfg(test)]
    pub fn mark_chat_clean(&mut self) {
        self.record_chat_drawn();
    }

    /// Mark both regions dirty, forcing a full repaint on the next frame.
    /// Used when something painted over the screen outside the tracked paths
    /// (e.g. an active picker overlay).
    pub fn invalidate(&mut self) {
        self.chat_dirty = true;
        self.bottom_dirty = true;
    }

    pub fn visible_lines(&self) -> usize {
        let (_, rows) = self.terminal_size();
        let input_height = self.prev_input_height.max(1);
        rows.saturating_sub(input_height as u16 + self.statusline_reserve()) as usize
    }

    /// Number of rows the input area will occupy for the given content. Kept in
    /// sync with the height logic used while drawing the input in `draw_bottom`.
    fn input_visible_height(&self, input_line: &str, rows: u16) -> usize {
        if self.permission_prompt.is_some() || self.chain_prompt.is_some() {
            return 2;
        }
        let available_rows = rows.saturating_sub(self.statusline_reserve()) as usize;
        let max_input_rows = available_rows.min((available_rows * 3 / 10).max(5));
        input_line.split('\n').count().min(max_input_rows).max(1)
    }

    /// Recompute the input height and reconcile `prev_input_height` before the
    /// chat viewport is drawn, so the chat is sized against the height the input
    /// is about to use. Without this, a height change (e.g. clearing or pasting
    /// text) leaves the viewport drawn for the old size until the next redraw.
    pub fn sync_input_height(&mut self, input_line: &str) -> io::Result<()> {
        let (_, rows) = self.terminal_size();
        let new_height = self.input_visible_height(input_line, rows);
        self.clear_shrunk_rows(self.prev_input_height, new_height)?;
        self.prev_input_height = new_height;
        Ok(())
    }

    pub fn buffer_line_at_row(&self, row: u16) -> Option<usize> {
        let width = self.max_line_width();
        let total = self.chat_lines(width).len();
        if total == 0 {
            return None;
        }

        let visible = self.visible_lines();
        let auto_scroll = self.scroll_offset == 0;
        let pad = if auto_scroll && total < visible {
            visible - total
        } else {
            0
        };
        if (row as usize) < pad {
            return None;
        }

        let start = if auto_scroll {
            total.saturating_sub(visible)
        } else {
            total.saturating_sub((self.scroll_offset + visible).min(total))
        };
        let start = start.min(total.saturating_sub(visible));

        Some(start + (row as usize) - pad)
    }

    pub fn clear_selection(&mut self) {
        self.chat_dirty = true;
        self.selection_active = false;
        self.selection_start = None;
        self.selection_end = None;
    }

    pub fn link_url_at(&self, buf_idx: usize, col: u16) -> Option<String> {
        let lines = self.chat_lines(self.max_line_width());
        let entry = lines.get(buf_idx)?;
        let text: &str = &entry.text;
        let click_col = col.saturating_sub(self.chat_margin) as usize;
        for m in URL_RE.find_iter(text) {
            let prefix = &text[..m.start()];
            let url_start = display_width(prefix);
            let url_end = url_start + display_width(m.as_str());
            if click_col >= url_start && click_col < url_end {
                return Some(m.as_str().to_string());
            }
        }
        None
    }

    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = match (self.selection_start, self.selection_end) {
            (Some(s), Some(e)) if s <= e => (s, e),
            (Some(s), Some(e)) => (e, s),
            _ => return None,
        };
        let lines = self.chat_lines(self.max_line_width());
        let mut result = String::new();
        for i in start..=end {
            if let Some(entry) = lines.get(i) {
                if !result.is_empty() {
                    result.push('\n');
                }
                result.push_str(&entry.text);
            }
        }
        if result.is_empty() {
            None
        } else {
            Some(result)
        }
    }

    fn commit_partial(&mut self) {
        if !self.partial.is_empty() {
            self.feed
                .push_block(self.partial_style, self.partial.as_str());
            self.partial.clear();
            self.chat_dirty = true;
        }
    }

    pub fn is_scrolling(&self) -> bool {
        self.scroll_offset > 0
    }

    /// Map a mouse click at `(row, col)` to a cursor byte offset inside the
    /// input buffer, or `None` if the click falls outside the input area.
    pub fn input_cursor_for_click(&self, row: u16, col: u16, input_line: &str) -> Option<usize> {
        let vlc = self.input_visible_line_count;
        if vlc == 0 {
            return None;
        }
        if row < self.input_base_row || row >= self.input_base_row + vlc as u16 {
            return None;
        }
        let visible_idx = (row - self.input_base_row) as usize;
        let line_idx = self.input_first_visible + visible_idx;
        let lines: SmallVec<[&str; 4]> = input_line.split('\n').collect();
        let line_text = lines.get(line_idx)?;

        // Display column the click lands on, within the line's text. Clicks on
        // the prompt (or to its left) snap to the start of the line.
        let click_col = col as usize;
        let mut target_display = click_col.saturating_sub(self.input_prompt_width);
        if line_idx == self.input_cursor_line {
            target_display += self.input_h_scroll;
        }

        // Walk the line accumulating display width until we pass the target,
        // landing on the nearest character boundary.
        let mut width = 0usize;
        let mut col_chars = 0usize;
        for ch in line_text.chars() {
            let cw = char_display_width(ch);
            if width + cw > target_display {
                break;
            }
            width += cw;
            col_chars += 1;
        }
        Some(crate::ui::input::line_col_to_cursor(
            input_line, line_idx, col_chars,
        ))
    }

    /// Scroll the multi-line input viewport up one line (toward earlier lines).
    /// Returns false when the input is already showing its top line, so the
    /// caller can fall through to scrolling the chat history instead.
    pub fn input_scroll_up(&mut self) -> bool {
        if self.input_vscroll_offset > 0 {
            self.input_vscroll_offset -= 1;
            true
        } else {
            false
        }
    }

    /// Scroll the multi-line input viewport down one line (toward the end).
    /// Returns false when the input is already at the bottom.
    pub fn input_scroll_down(&mut self) -> bool {
        if self.input_vscroll_offset < self.input_max_vscroll {
            self.input_vscroll_offset += 1;
            true
        } else {
            false
        }
    }

    pub fn scroll_line_up(&mut self) {
        let visible = self.visible_lines();
        let max_offset = self.buffer_len().saturating_sub(visible);
        if self.scroll_offset < max_offset {
            self.scroll_offset += 1;
            self.chat_dirty = true;
        }
    }

    pub fn scroll_line_down(&mut self) {
        if self.scroll_offset > 0 {
            self.scroll_offset -= 1;
            self.chat_dirty = true;
        }
    }

    pub fn scroll_page_up(&mut self) {
        let visible = self.visible_lines();
        let page = visible.saturating_sub(2).max(1);
        let max_offset = self.buffer_len().saturating_sub(visible);
        let new_offset = (self.scroll_offset + page).min(max_offset);
        if new_offset != self.scroll_offset {
            self.scroll_offset = new_offset;
            self.chat_dirty = true;
        }
    }

    pub fn scroll_page_down(&mut self) {
        let visible = self.visible_lines();
        let page = visible.saturating_sub(2).max(1);
        let new_offset = if self.scroll_offset <= page {
            0
        } else {
            self.scroll_offset.saturating_sub(page)
        };
        if new_offset != self.scroll_offset {
            self.scroll_offset = new_offset;
            self.chat_dirty = true;
        }
    }

    pub fn scroll_to_top(&mut self) {
        let visible = self.visible_lines();
        let new_offset = self.buffer_len().saturating_sub(visible);
        if new_offset != self.scroll_offset {
            self.scroll_offset = new_offset;
            self.chat_dirty = true;
        }
    }

    pub fn scroll_to_bottom(&mut self) -> io::Result<()> {
        if self.scroll_offset != 0 {
            self.scroll_offset = 0;
            self.chat_dirty = true;
        }
        self.sync_to_buffer()
    }

    fn sync_to_buffer(&mut self) -> io::Result<()> {
        self.commit_partial();
        self.render_viewport()
    }

    pub fn render_viewport(&mut self) -> io::Result<()> {
        if !self.chat_needs_redraw() {
            return Ok(());
        }
        let (cols, _rows) = self.terminal_size();
        let max_width = cols.saturating_sub(1 + self.chat_margin) as usize;
        let visible = self.visible_lines();
        let buffer = self.chat_lines(max_width);
        let total = buffer.len();
        let mut stdout = io::stdout();
        write!(stdout, "{}", Hide)?;

        let auto_scroll = self.scroll_offset == 0;
        let start = if auto_scroll {
            total.saturating_sub(visible)
        } else {
            total.saturating_sub(self.scroll_offset + visible)
        };
        let start = start.min(total.saturating_sub(visible));

        let mut visual_row: u16 = 0;
        let mut buf_idx = start;

        // Bottom-align: when auto-scrolling and content is shorter than viewport,
        // render empty rows first so content hugs the input area.
        if auto_scroll && total < visible {
            let pad = visible - total;
            for _ in 0..pad {
                stdout.execute(MoveTo(0, visual_row))?;
                if let Some(bg) = self.chat_bg {
                    write!(stdout, "{}", SetBackgroundColor(self.color(bg)))?;
                }
                write!(stdout, "{}", Clear(ClearType::UntilNewLine))?;
                write!(stdout, "{}", ResetColor)?;
                visual_row += 1;
            }
        }

        while (visual_row as usize) < visible && buf_idx < total {
            let entry = &buffer[buf_idx];
            let chunk = &entry.text;

            if (visual_row as usize) >= visible {
                break;
            }

            stdout.execute(MoveTo(0, visual_row))?;

            let is_selected = self.selection_active
                && if let (Some(s), Some(e)) = (self.selection_start, self.selection_end) {
                    let lo = s.min(e);
                    let hi = s.max(e);
                    buf_idx >= lo && buf_idx <= hi
                } else {
                    false
                };

            if let Some(bg) = self.chat_bg {
                write!(stdout, "{}", SetBackgroundColor(self.color(bg)))?;
            }
            self.write_chat_margin(&mut stdout)?;
            if is_selected {
                write!(stdout, "{}", SetAttribute(Attribute::Reverse))?;
            }
            write!(stdout, "{}", SetForegroundColor(self.color(entry.color)))?;
            write!(stdout, "{}", wrap_urls_osc8(chunk))?;
            if is_selected {
                write!(stdout, "{}", SetAttribute(Attribute::NoReverse))?;
            }
            write!(stdout, "{}", Clear(ClearType::UntilNewLine))?;
            write!(stdout, "{}", ResetColor)?;

            visual_row += 1;
            buf_idx += 1;
        }

        while (visual_row as usize) < visible {
            stdout.execute(MoveTo(0, visual_row))?;
            if let Some(bg) = self.chat_bg {
                write!(stdout, "{}", SetBackgroundColor(self.color(bg)))?;
            }
            write!(stdout, "{}", Clear(ClearType::UntilNewLine))?;
            write!(stdout, "{}", ResetColor)?;
            visual_row += 1;
        }

        if self.scroll_offset > 0 {
            let pct = if total > visible {
                ((total - self.scroll_offset - visible) * 100 / (total - visible)).min(100)
            } else {
                0
            };
            let indicator = format!(" SCROLL {}% ", pct);
            let x = cols.saturating_sub(indicator.len() as u16);
            stdout.execute(MoveTo(x, 0))?;
            if let Some(bg) = self.chat_bg {
                write!(stdout, "{}", SetBackgroundColor(self.color(bg)))?;
            }
            write!(
                stdout,
                "{}",
                SetForegroundColor(self.color(Color::DarkYellow))
            )?;
            write!(stdout, "{}", indicator)?;
            write!(stdout, "{}", ResetColor)?;
        }

        stdout.flush()?;
        self.record_chat_drawn();
        Ok(())
    }

    pub fn write_line(&mut self, text: &str, color: Color) -> io::Result<()> {
        self.commit_partial();
        let style = style_from_color(color);
        self.feed.push_block(style, text);
        self.chat_dirty = true;
        if self.scroll_offset == 0 {
            self.render_viewport()?;
        }
        Ok(())
    }

    pub fn write(&mut self, text: &str, color: Color) -> io::Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        let style = style_from_color(color);
        let parts: SmallVec<[&str; 4]> = text.split('\n').collect();
        let last = parts.len() - 1;
        for (i, segment) in parts.iter().enumerate() {
            if i < last {
                // Complete line segment: finalize any partial and push it.
                if !self.partial.is_empty() {
                    self.commit_partial();
                }
                self.feed.push_block(style, *segment);
            } else {
                // Last segment may still be incomplete; accumulate in partial.
                self.partial_style = style;
                self.partial.push_str(segment);
            }
        }
        self.chat_dirty = true;
        if self.scroll_offset == 0 {
            self.render_viewport()?;
        }
        Ok(())
    }

    pub fn clear_content(&mut self) -> io::Result<()> {
        self.chat_dirty = true;
        self.feed.clear();
        self.partial.clear();
        self.scroll_offset = 0;
        self.clear_selection();
        let mut stdout = io::stdout();
        if let Some(bg) = self.chat_bg {
            write!(stdout, "{}", SetBackgroundColor(self.color(bg)))?;
        }
        stdout.execute(Clear(ClearType::All))?;
        write!(stdout, "{}", ResetColor)?;
        stdout.execute(MoveTo(0, 0))?;
        stdout.flush()?;
        Ok(())
    }

    pub fn resize(&mut self) {
        self.chat_dirty = true;
        let visible = self.visible_lines();
        let max_offset = self.buffer_len().saturating_sub(visible);
        if self.scroll_offset > max_offset {
            self.scroll_offset = max_offset;
        }
    }

    fn clear_shrunk_rows(&self, old_height: usize, new_height: usize) -> io::Result<()> {
        if new_height >= old_height {
            return Ok(());
        }
        let (_, rows) = self.terminal_size();
        let reserve = self.statusline_reserve();
        let avail = rows.saturating_sub(reserve);
        let old_start = avail.saturating_sub(old_height as u16).saturating_add(1);
        let new_start = avail.saturating_sub(new_height as u16).saturating_add(1);
        let mut stdout = io::stdout();
        for row in old_start..new_start {
            stdout.execute(MoveTo(0, row))?;
            if let Some(bg) = self.input_bg {
                write!(stdout, "{}", SetBackgroundColor(self.color(bg)))?;
            }
            write!(stdout, "{}", Clear(ClearType::UntilNewLine))?;
            write!(stdout, "{}", ResetColor)?;
        }
        Ok(())
    }

    fn draw_separator(&self, row: u16, cols: u16) -> io::Result<()> {
        let mut stdout = io::stdout();
        stdout.execute(MoveTo(0, row))?;
        if let Some(bg) = self.input_bg {
            write!(stdout, "{}", SetBackgroundColor(self.color(bg)))?;
        }
        write!(
            stdout,
            "{}",
            SetForegroundColor(self.color(Color::DarkGrey))
        )?;
        let sep: String = "─".repeat(cols as usize);
        write!(stdout, "{}", sep)?;
        write!(stdout, "{}", ResetColor)?;
        Ok(())
    }

    /// Draw the statusline (1-3 lines) at the bottom rows. Each line's `Flex` spans
    /// expand to fill remaining width. Fewer lines than `statusline_height` leaves
    /// the upper statusline rows blank.
    fn draw_statusline(
        &self,
        statusline: &[Vec<StatusSpan>],
        cols: u16,
        is_scrolling: bool,
    ) -> io::Result<()> {
        let (_, rows) = self.terminal_size();
        let h = self.statusline_height as u16;
        for row_idx in 0..h {
            let screen_row = rows.saturating_sub(h - row_idx);
            let empty: Vec<StatusSpan> = Vec::new();
            let spans = statusline.get(row_idx as usize).unwrap_or(&empty);
            // Scroll indicator on the top statusline row only.
            let prefix = if is_scrolling && row_idx == 0 {
                "-- SCROLL -- "
            } else {
                ""
            };
            self.draw_statusline_row(screen_row, spans, prefix, cols)?;
        }
        Ok(())
    }

    fn draw_statusline_row(
        &self,
        screen_row: u16,
        spans: &[StatusSpan],
        prefix: &str,
        cols: u16,
    ) -> io::Result<()> {
        let mut stdout = io::stdout();
        stdout.execute(MoveTo(0, screen_row))?;
        if let Some(bg) = self.status_bg {
            write!(stdout, "{}", SetBackgroundColor(self.color(bg)))?;
        }
        write!(stdout, "{}", Clear(ClearType::CurrentLine))?;
        stdout.execute(MoveTo(0, screen_row))?;
        if let Some(bg) = self.status_bg {
            write!(stdout, "{}", SetBackgroundColor(self.color(bg)))?;
        }

        let total = cols as usize;
        let mut budget = total;

        if !prefix.is_empty() {
            write!(
                stdout,
                "{}",
                SetForegroundColor(self.color(Color::DarkYellow))
            )?;
            let take = prefix.chars().take(budget).collect::<String>();
            budget -= display_width(&take);
            write!(stdout, "{}", take)?;
        }

        // Fixed width of all text spans; flex shares what is left.
        let fixed: usize = spans
            .iter()
            .map(|s| match s {
                StatusSpan::Text { text, .. } => display_width(text),
                StatusSpan::Flex => 0,
            })
            .sum();
        let flex_count = spans
            .iter()
            .filter(|s| matches!(s, StatusSpan::Flex))
            .count();
        let mut flex_left = budget.saturating_sub(fixed);
        let mut flex_seen = 0usize;

        for span in spans {
            if budget == 0 {
                break;
            }
            match span {
                StatusSpan::Text { text, fg, bg } => {
                    let bgc = bg.or(self.status_bg);
                    if let Some(c) = bgc {
                        write!(stdout, "{}", SetBackgroundColor(self.color(c)))?;
                    }
                    let fgc = fg.unwrap_or(Color::DarkGrey);
                    write!(stdout, "{}", SetForegroundColor(self.color(fgc)))?;
                    let piece: String = text.chars().take(budget).collect();
                    budget = budget.saturating_sub(display_width(&piece));
                    write!(stdout, "{}", piece)?;
                    write!(stdout, "{}", ResetColor)?;
                    if let Some(bg) = self.status_bg {
                        write!(stdout, "{}", SetBackgroundColor(self.color(bg)))?;
                    }
                }
                StatusSpan::Flex => {
                    flex_seen += 1;
                    if flex_count == 0 {
                        continue;
                    }
                    // Distribute leftover evenly; earliest flex absorbs the remainder.
                    let base = flex_left / flex_count;
                    let extra = if flex_seen <= flex_left % flex_count {
                        1
                    } else {
                        0
                    };
                    let width = (base + extra).min(budget);
                    flex_left = flex_left.saturating_sub(width);
                    budget = budget.saturating_sub(width);
                    write!(stdout, "{}", " ".repeat(width))?;
                }
            }
        }

        write!(stdout, "{}", Clear(ClearType::UntilNewLine))?;
        write!(stdout, "{}", ResetColor)?;
        Ok(())
    }

    /// Snapshot of everything `draw_bottom` would paint for these arguments.
    fn bottom_snapshot(
        &self,
        input_line: &str,
        cursor_pos: usize,
        statusline: &[Vec<StatusSpan>],
        is_running: bool,
        cols: u16,
        rows: u16,
    ) -> BottomSnapshot {
        let prompt = if let Some(ref pp) = self.permission_prompt {
            PromptSnapshot::Permission {
                tool: pp.tool.clone(),
                options: pp.options.clone(),
            }
        } else if let Some(ref cp) = self.chain_prompt {
            PromptSnapshot::Chain {
                question: cp.question.clone(),
                but_mode: self.chain_but_mode,
            }
        } else {
            PromptSnapshot::Input
        };
        BottomSnapshot {
            cols,
            rows,
            statusline_height: self.statusline_height,
            input: input_line.to_string(),
            cursor_pos,
            is_running,
            spinner_frame: self.spinner_frame,
            input_vscroll_offset: self.input_vscroll_offset,
            scroll_indicator: matches!(prompt, PromptSnapshot::Input) && self.scroll_offset > 0,
            prompt,
            statusline: statusline.to_vec(),
            monochrome: self.monochrome,
            input_bg: self.input_bg,
            status_bg: self.status_bg,
        }
    }

    /// Pure redraw decision for the bottom region: compare the state recorded
    /// after the last draw with the state about to be drawn. When only the
    /// statusline content (or the scroll indicator painted inside it) differs,
    /// the input area is untouched and only the statusline rows need a repaint.
    pub(crate) fn bottom_redraw_plan(
        prev: Option<&BottomSnapshot>,
        next: &BottomSnapshot,
        force_full: bool,
    ) -> BottomRedrawPlan {
        if force_full {
            return BottomRedrawPlan::Full;
        }
        let Some(prev) = prev else {
            return BottomRedrawPlan::Full;
        };
        if prev == next {
            return BottomRedrawPlan::Skip;
        }
        let mut status_only = next.clone();
        status_only.statusline = prev.statusline.clone();
        status_only.scroll_indicator = prev.scroll_indicator;
        if status_only == *prev {
            BottomRedrawPlan::StatuslineOnly
        } else {
            BottomRedrawPlan::Full
        }
    }

    /// Record the bottom region as freshly drawn.
    fn record_bottom_drawn(&mut self, snapshot: BottomSnapshot) {
        self.last_bottom_snapshot = Some(snapshot);
        self.bottom_dirty = false;
    }

    /// Re-place the terminal caret where the last full bottom draw left it.
    /// Needed after a statusline-only redraw, which moves the cursor.
    fn restore_bottom_cursor(&self) -> io::Result<()> {
        let mut stdout = io::stdout();
        match self.bottom_cursor {
            Some((x, row)) => {
                stdout.execute(MoveTo(x, row))?;
                write!(stdout, "{}", Show)?;
            }
            None => {
                write!(stdout, "{}", Hide)?;
            }
        }
        stdout.flush()
    }

    pub fn draw_bottom(
        &mut self,
        input_line: &str,
        cursor_pos: usize,
        statusline: &[Vec<StatusSpan>],
        is_running: bool,
    ) -> io::Result<()> {
        let (cols, rows) = crossterm::terminal::size()?;
        let snapshot =
            self.bottom_snapshot(input_line, cursor_pos, statusline, is_running, cols, rows);
        match Self::bottom_redraw_plan(
            self.last_bottom_snapshot.as_ref(),
            &snapshot,
            self.bottom_dirty,
        ) {
            BottomRedrawPlan::Skip => return Ok(()),
            BottomRedrawPlan::StatuslineOnly => {
                self.draw_statusline(statusline, cols, snapshot.scroll_indicator)?;
                self.restore_bottom_cursor()?;
                self.record_bottom_drawn(snapshot);
                return Ok(());
            }
            BottomRedrawPlan::Full => {}
        }
        let reserve = self.statusline_reserve();
        let mut stdout = io::stdout();

        if let Some(ref pp) = self.permission_prompt {
            let perm_lines = [pp.tool.as_str(), pp.options.as_str()];
            let line_count = 2usize;
            let input_top = rows
                .saturating_sub(reserve)
                .saturating_sub(line_count as u16)
                .saturating_add(1);
            let sep_above = input_top.saturating_sub(1);

            self.clear_shrunk_rows(self.prev_input_height, line_count)?;
            self.prev_input_height = line_count;

            if sep_above < input_top {
                self.draw_separator(sep_above, cols)?;
            }

            let perm_color = self.color(Color::DarkYellow);
            for (i, line) in perm_lines.iter().enumerate() {
                let render_row = input_top + i as u16;
                stdout.execute(MoveTo(0, render_row))?;
                if let Some(bg) = self.input_bg {
                    write!(stdout, "{}", SetBackgroundColor(self.color(bg)))?;
                }
                write!(stdout, "{}", SetForegroundColor(perm_color))?;
                write!(stdout, "{}", line)?;
                write!(stdout, "{}", Clear(ClearType::UntilNewLine))?;
                write!(stdout, "{}", ResetColor)?;
            }

            let sep_below = rows.saturating_sub(reserve - 1);
            if sep_below < rows.saturating_sub(1) {
                self.draw_separator(sep_below, cols)?;
            }

            self.draw_statusline(statusline, cols, false)?;
            write!(stdout, "{}", Hide)?;
            stdout.flush()?;
            self.bottom_cursor = None;
            self.record_bottom_drawn(snapshot);
            return Ok(());
        }

        if let Some(ref cp) = self.chain_prompt {
            let question = cp.question.as_str();
            let options = if self.chain_but_mode {
                "[Enter] send  [Esc] cancel"
            } else {
                "[Y] Yes  [N] No  [B] yes, But (add instruction)"
            };
            let line_count = 2usize;
            let input_top = rows
                .saturating_sub(reserve)
                .saturating_sub(line_count as u16)
                .saturating_add(1);
            let sep_above = input_top.saturating_sub(1);

            self.clear_shrunk_rows(self.prev_input_height, line_count)?;
            self.prev_input_height = line_count;

            if sep_above < input_top {
                self.draw_separator(sep_above, cols)?;
            }

            let chain_color = self.color(Color::DarkYellow);
            let render_lines = [question, options];
            for (i, line) in render_lines.iter().enumerate() {
                let render_row = input_top + i as u16;
                stdout.execute(MoveTo(0, render_row))?;
                if let Some(bg) = self.input_bg {
                    write!(stdout, "{}", SetBackgroundColor(self.color(bg)))?;
                }
                write!(stdout, "{}", SetForegroundColor(chain_color))?;
                write!(stdout, "{}", line)?;
                write!(stdout, "{}", Clear(ClearType::UntilNewLine))?;
                write!(stdout, "{}", ResetColor)?;
            }

            let sep_below = rows.saturating_sub(reserve - 1);
            if sep_below < rows.saturating_sub(1) {
                self.draw_separator(sep_below, cols)?;
            }

            self.draw_statusline(statusline, cols, false)?;
            write!(stdout, "{}", Hide)?;
            stdout.flush()?;
            self.bottom_cursor = None;
            self.record_bottom_drawn(snapshot);
            return Ok(());
        }

        let lines: SmallVec<[&str; 4]> = input_line.split('\n').collect();
        let line_count = lines.len();

        let available_rows = (rows.saturating_sub(reserve) as usize).max(1);
        // Cap the input height to roughly 30% of the area so the chat history
        // stays visible (and therefore scrollable) above a tall input instead
        // of being squeezed to nothing.
        let max_input_rows = available_rows.min((available_rows * 3 / 10).max(5));
        let need_scroll = line_count > max_input_rows;

        const SPINNER: &[&str] = &["⠋ ", "⠙ ", "⠹ ", "⠸ ", "⠼ ", "⠴ ", "⠦ ", "⠧ ", "⠇ ", "⠏ "];
        let prompt = if is_running {
            let frame = SPINNER[self.spinner_frame as usize];
            self.spinner_frame = (self.spinner_frame + 1) % SPINNER.len() as u8;
            frame
        } else {
            "> "
        };
        let prompt_width = display_width(prompt);

        let (cursor_line, cursor_col) =
            crate::ui::input::cursor_to_line_col(input_line, cursor_pos);

        // Vertical scroll: keep the cursor's line within the visible window so
        // pressing Up/Down can reveal lines that don't fit on screen at once.
        // Only follow the cursor when it actually moved, so mouse-wheel scrolling
        // (which leaves the cursor put) is not snapped back every frame.
        let cursor_moved = self.last_input_cursor != cursor_pos;
        self.last_input_cursor = cursor_pos;
        let first_visible = if need_scroll {
            self.input_max_vscroll = line_count - max_input_rows;
            if cursor_moved {
                if cursor_line < self.input_vscroll_offset {
                    self.input_vscroll_offset = cursor_line;
                } else if cursor_line >= self.input_vscroll_offset + max_input_rows {
                    self.input_vscroll_offset = cursor_line - max_input_rows + 1;
                }
            }
            self.input_vscroll_offset = self.input_vscroll_offset.min(self.input_max_vscroll);
            self.input_vscroll_offset
        } else {
            self.input_vscroll_offset = 0;
            self.input_max_vscroll = 0;
            0
        };

        let visible_width = cols.saturating_sub(prompt_width as u16) as usize;
        let cursor_line_text = lines.get(cursor_line).unwrap_or(&"");

        // Convert cursor char-index to display column
        let cursor_byte = cursor_line_text
            .char_indices()
            .nth(cursor_col)
            .map(|(i, _)| i)
            .unwrap_or(cursor_line_text.len());
        let cursor_display_col = display_width(&cursor_line_text[..cursor_byte]);

        let cursor_line_len = display_width(cursor_line_text);
        let mut h_scroll = 0usize;
        if cursor_line_len > visible_width {
            if cursor_display_col < self.input_scroll_offset {
                self.input_scroll_offset = cursor_display_col;
            } else if cursor_display_col >= self.input_scroll_offset + visible_width {
                self.input_scroll_offset = cursor_display_col - visible_width + 1;
            }
            let max_h_scroll = cursor_line_len.saturating_sub(visible_width);
            h_scroll = self.input_scroll_offset.min(max_h_scroll);
        } else {
            self.input_scroll_offset = 0;
        }

        // Clear and draw input area
        let visible_line_count = if need_scroll {
            max_input_rows
        } else {
            line_count
        };

        self.clear_shrunk_rows(self.prev_input_height, visible_line_count)?;
        self.prev_input_height = visible_line_count;

        // Thin separator line above input
        let input_top = rows
            .saturating_sub(reserve)
            .saturating_sub(visible_line_count as u16)
            .saturating_add(1);
        let sep_above = input_top.saturating_sub(1);
        if sep_above < input_top {
            self.draw_separator(sep_above, cols)?;
        }

        // Remember the input layout so a mouse click can be mapped back to a
        // cursor position inside the input buffer.
        self.input_base_row = input_top;
        self.input_prompt_width = prompt_width;
        self.input_first_visible = first_visible;
        self.input_visible_line_count = visible_line_count;
        self.input_h_scroll = h_scroll;
        self.input_cursor_line = cursor_line;

        for (i, line) in lines
            .iter()
            .enumerate()
            .skip(first_visible)
            .take(visible_line_count)
        {
            let render_row = (rows.saturating_sub(reserve) - visible_line_count as u16 + 1)
                + (i - first_visible) as u16;
            stdout.execute(MoveTo(0, render_row))?;

            if let Some(bg) = self.input_bg {
                write!(stdout, "{}", SetBackgroundColor(self.color(bg)))?;
            }

            if i == first_visible {
                write!(
                    stdout,
                    "{}",
                    SetForegroundColor(self.color(Color::DarkYellow))
                )?;
                write!(stdout, "{}", prompt)?;
                write!(stdout, "{}", SetForegroundColor(Color::Reset))?;
            } else {
                write!(stdout, "{}", " ".repeat(prompt_width))?;
            }

            let line_chars: SmallVec<[char; 64]> = line.chars().collect();
            // Skip chars to reach display column h_scroll, then take enough to fill visible_width
            let skip_chars: usize = if i == cursor_line {
                let mut w = 0usize;
                let mut skip = 0usize;
                for &ch in &line_chars {
                    let cw = char_display_width(ch);
                    if w + cw > h_scroll {
                        break;
                    }
                    w += cw;
                    skip += 1;
                }
                skip
            } else {
                0
            };
            let display: String = line_chars
                .iter()
                .skip(skip_chars)
                .take(visible_width)
                .collect();
            write!(stdout, "{}", display)?;
            write!(stdout, "{}", Clear(ClearType::UntilNewLine))?;
            write!(stdout, "{}", ResetColor)?;
        }

        // Thin separator line below input
        let sep_below = rows.saturating_sub(reserve - 1);
        if sep_below < rows.saturating_sub(1) {
            self.draw_separator(sep_below, cols)?;
        }

        // Status line
        self.draw_statusline(statusline, cols, self.scroll_offset > 0)?;

        // Cursor. Clamp to the visible input rows so that when the viewport is
        // scrolled away from the cursor line, the terminal caret stays inside
        // the input box instead of spilling onto the separator or status bar.
        let cursor_render_idx = cursor_line
            .saturating_sub(first_visible)
            .min(visible_line_count.saturating_sub(1));
        let cursor_row = (rows.saturating_sub(reserve) - visible_line_count as u16 + 1)
            + cursor_render_idx as u16;
        let cursor_x = (prompt_width + cursor_display_col.saturating_sub(h_scroll)) as u16;
        stdout.execute(MoveTo(cursor_x, cursor_row))?;
        write!(stdout, "{}", Show)?;
        stdout.flush()?;
        self.bottom_cursor = Some((cursor_x, cursor_row));
        // The draw itself settles `input_vscroll_offset` (cursor follow /
        // clamping); record the settled value so the next identical frame is
        // recognized as unchanged. `spinner_frame` deliberately keeps the
        // pre-draw value so a running spinner still differs next frame.
        let mut drawn = snapshot;
        drawn.input_vscroll_offset = self.input_vscroll_offset;
        self.record_bottom_drawn(drawn);
        Ok(())
    }
}

pub fn open_url(url: &str) {
    let openers: &[(&str, &[&str])] = &[
        ("xdg-open", &[url]),
        ("open", &[url]),               // macOS
        ("cmd", &["/c", "start", url]), // Windows
    ];
    for &(cmd, args) in openers {
        if std::process::Command::new(cmd)
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .is_ok()
        {
            return;
        }
    }
}

pub fn copy_to_clipboard(text: &str) {
    let cmds: &[(&str, &[&str])] = &[
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("pbcopy", &[]),
        ("clip.exe", &[]),
    ];
    for &(cmd, args) in cmds {
        if let Ok(mut child) = std::process::Command::new(cmd)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
                let _ = stdin.flush();
            }
            let _ = child.wait();
            return;
        }
    }

    // OSC 52 escape sequence — clipboard access via terminal emulator.
    // Supported by Kitty, Alacritty, WezTerm, foot, iTerm2, Windows Terminal,
    // and most other modern terminals. No external tools needed.
    let encoded = base64_encode(text.as_bytes());
    let mut stdout = std::io::stdout().lock();
    let _ = write!(stdout, "\x1b]52;c;{encoded}\x07");
    let _ = stdout.flush();
}

/// Minimal base64 encoder — avoids pulling in a crate just for clipboard support.
pub(crate) fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = chunk.get(1).copied().unwrap_or(0) as usize;
        let b2 = chunk.get(2).copied().unwrap_or(0) as usize;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(triple >> 18) & 63] as char);
        out.push(ALPHABET[(triple >> 12) & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(triple >> 6) & 63]
        } else {
            b'='
        } as char);
        out.push(if chunk.len() > 2 {
            ALPHABET[triple & 63]
        } else {
            b'='
        } as char);
    }
    out
}

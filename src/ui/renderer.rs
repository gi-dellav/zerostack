use std::io::{self, Write};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use compact_str::CompactString;
use crossterm::QueueableCommand;
use crossterm::cursor::{Hide, MoveDown, MoveRight, MoveTo, MoveUp, Show};
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

/// Terminal output sink for [`Renderer`]: every ANSI write and terminal-size
/// read goes through this trait. Production uses [`CrosstermBackend`] (stdout
/// plus the real terminal size); tests swap in [`FakeBackend`] to capture the
/// emitted frames and pin a fixed geometry — this is what lets the main loop
/// run headless in integration tests.
pub(crate) trait RenderBackend: io::Write + Send {
    fn size(&self) -> io::Result<(u16, u16)>;

    /// Everything written so far, for test assertions. `None` on real backends.
    #[cfg(test)]
    fn captured(&self) -> Option<String> {
        None
    }

    /// Resize the pinned geometry (test backends only).
    #[cfg(test)]
    fn set_size(&mut self, _cols: u16, _rows: u16) {}
}

/// Production backend: plain stdout plus `crossterm::terminal::size`.
pub(crate) struct CrosstermBackend {
    stdout: io::Stdout,
}

impl io::Write for CrosstermBackend {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stdout.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stdout.flush()
    }
}

impl RenderBackend for CrosstermBackend {
    fn size(&self) -> io::Result<(u16, u16)> {
        crossterm::terminal::size()
    }
}

/// Test backend: fixed terminal geometry, records every byte written so tests
/// can assert on the frames the renderer emitted.
#[cfg(test)]
pub(crate) struct FakeBackend {
    buf: Vec<u8>,
    cols: u16,
    rows: u16,
}

#[cfg(test)]
impl FakeBackend {
    pub(crate) fn new(cols: u16, rows: u16) -> Self {
        Self {
            buf: Vec::new(),
            cols,
            rows,
        }
    }
}

#[cfg(test)]
impl io::Write for FakeBackend {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
impl RenderBackend for FakeBackend {
    fn size(&self) -> io::Result<(u16, u16)> {
        Ok((self.cols, self.rows))
    }

    fn captured(&self) -> Option<String> {
        Some(String::from_utf8_lossy(&self.buf).into_owned())
    }

    fn set_size(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
    }
}

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
    pub style: BlockStyle,
    /// Wall-clock time of the block this line belongs to, for timestamped output.
    pub created_at: Option<chrono::DateTime<chrono::Local>>,
    /// True for the first visible row of a block, so the renderer can draw a
    /// faint separator above turns when timestamps are enabled.
    pub block_start: bool,
}

impl LineEntry {
    pub fn new(text: CompactString, color: Color, style: BlockStyle) -> Self {
        Self {
            text,
            color,
            style,
            created_at: None,
            block_start: false,
        }
    }

    pub(crate) fn with_created_at(
        mut self,
        created_at: Option<chrono::DateTime<chrono::Local>>,
    ) -> Self {
        self.created_at = created_at;
        self
    }
}

pub struct PermissionPrompt {
    pub tool: CompactString,
    pub options: CompactString,
}

pub struct ChainPrompt {
    pub question: CompactString,
}

/// Print watermark: the position in the feed up to which lines have been
/// committed (printed) to the terminal's native scrollback. Tracked per block
/// (`block` = first block with unprinted lines, `line` = lines of that block
/// already printed) so it survives feed truncation and width remapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Watermark {
    /// Feed width the printed lines were laid out at.
    pub(crate) width: usize,
    pub(crate) block: usize,
    pub(crate) line: usize,
}

/// Clamp a watermark after the feed was truncated: positions past the new end
/// collapse to "everything printed".
pub(crate) fn clamp_watermark(wm: Watermark, block_count: usize) -> Watermark {
    if wm.block >= block_count {
        Watermark {
            block: block_count,
            line: 0,
            ..wm
        }
    } else {
        wm
    }
}

/// Advance a watermark after `printed` lines were committed, given the line
/// counts of the blocks from `wm.block` onward in the current layout.
pub(crate) fn advance_watermark(wm: Watermark, counts: &[usize], printed: usize) -> Watermark {
    let mut block = wm.block;
    let mut line = wm.line;
    let mut left = printed;
    for (i, &count) in counts.iter().enumerate() {
        if line >= count {
            // Already fully printed (including zero-line blocks such as an
            // empty agent block), or the layout shrank underneath the
            // watermark: move to the next block.
            block = wm.block + i + 1;
            line = 0;
        }
        if left == 0 {
            break;
        }
        let take = (count - line).min(left);
        line += take;
        left -= take;
        if line == count {
            block = wm.block + i + 1;
            line = 0;
        }
    }
    Watermark { block, line, ..wm }
}

/// How many pending feed lines to commit (print) this frame: all `committed`
/// finalized lines, plus the part of the live tail (the still-running
/// streaming block's `live_feed` lines) — counting the partial scratch rows
/// too — that overflows the live block's streaming region. The partial
/// scratch itself is never printed; `live_feed` caps the spill.
pub(crate) fn commit_count(
    committed: usize,
    live_feed: usize,
    partial_rows: usize,
    stream_rows: usize,
) -> usize {
    let spill = (live_feed + partial_rows)
        .saturating_sub(stream_rows)
        .min(live_feed);
    committed + spill
}

/// Soft-wrap one hard input line at `width` display columns, returning the
/// `(start_byte, end_byte)` slice of each display row. Wraps at character
/// boundaries (no word logic — the input is raw text); a row always holds at
/// least one character, even one wider than `width`. An empty line yields one
/// empty segment.
pub(crate) fn wrap_input_segments(line: &str, width: usize) -> Vec<(usize, usize)> {
    let width = width.max(1);
    let mut segments = Vec::new();
    let mut start = 0usize;
    let mut col = 0usize;
    for (i, ch) in line.char_indices() {
        let cw = char_display_width(ch);
        if col > 0 && col + cw > width {
            segments.push((start, i));
            start = i;
            col = 0;
        }
        col += cw;
    }
    segments.push((start, line.len()));
    segments
}

/// Map a cursor byte offset within a hard line to its `(wrapped_row,
/// display_col)` across the segments from [`wrap_input_segments`]. A cursor
/// sitting exactly on a wrap boundary lands at the start of the next row.
pub(crate) fn wrapped_cursor(
    segments: &[(usize, usize)],
    line: &str,
    cursor_byte: usize,
) -> (usize, usize) {
    let mut row = 0usize;
    for (i, &(start, _)) in segments.iter().enumerate() {
        if start > cursor_byte {
            break;
        }
        row = i;
    }
    let col = display_width(&line[segments[row].0..cursor_byte]);
    (row, col)
}

/// Which prompt mode `draw_live_block` paints in the input area.
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

/// One row of a picker overlay, as painted inside the live block. `text` is
/// the final display string (prefix and truncation applied by the picker);
/// `selected` only chooses the highlight color.
#[derive(Clone, Debug, PartialEq)]
pub struct PickerRow {
    pub text: String,
    pub selected: bool,
}

/// A picker overlay rendered as a section of the live block (between the
/// streaming region and the input box) so the block's erase/redraw always
/// covers it — no row a picker painted can be left behind.
#[derive(Clone, Debug, PartialEq)]
pub struct PickerView {
    /// Optional header row above the list (e.g. the models group tabs).
    pub header: Option<String>,
    pub rows: Vec<PickerRow>,
}

impl PickerView {
    /// Display rows the view occupies (header + list).
    pub fn height(&self) -> usize {
        self.rows.len() + usize::from(self.header.is_some())
    }
}

/// Everything `draw_live_block` paints, compared between frames to decide how
/// much of the live block (streaming region + input area + statusline) needs
/// repainting.
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
    pub(crate) picker: Option<PickerView>,
    pub(crate) statusline: Vec<Vec<StatusSpan>>,
    pub(crate) feed_generation: u64,
    pub(crate) watermark: Option<Watermark>,
    pub(crate) partial: CompactString,
    pub(crate) partial_style: BlockStyle,
    pub(crate) monochrome: bool,
    pub(crate) chat_bg: Option<Color>,
    pub(crate) chat_margin: u16,
    pub(crate) input_bg: Option<Color>,
    pub(crate) status_bg: Option<Color>,
    pub(crate) show_timestamps: bool,
}

/// How much of the live block a `draw_live_block` call must repaint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BottomRedrawPlan {
    /// Nothing changed since the last frame; draw nothing.
    Skip,
    /// Only the statusline content changed; redraw just the statusline rows.
    StatuslineOnly,
    /// Anything else changed; full live-block redraw.
    Full,
}

/// Pending (unprinted) feed content, split for a frame.
struct PendingSplit {
    /// Finalized lines past the watermark, ready to commit.
    committed: Vec<LineEntry>,
    /// Lines shown in the live block's streaming region: the still-running
    /// streaming block's unprinted lines plus the partial scratch.
    live: Vec<LineEntry>,
    /// How many of `live` come from the feed (the rest is partial scratch,
    /// which must never be printed).
    live_feed: usize,
    /// Full line counts of the blocks from the watermark onward, in the
    /// current layout; drives [`advance_watermark`].
    counts: Vec<usize>,
}

/// Last arguments passed to [`Renderer::draw_live_block`], cached so internal
/// repaints (streaming flushes) can redraw the live block without the
/// caller's input/statusline state.
#[derive(Clone)]
struct LiveArgs {
    input: String,
    cursor: usize,
    statusline: Vec<Vec<StatusSpan>>,
    is_running: bool,
    picker: Option<PickerView>,
}

pub struct Renderer {
    backend: Box<dyn RenderBackend>,
    spinner_frame: u8,
    feed: Feed,
    partial: CompactString,
    partial_style: BlockStyle,
    input_vscroll_offset: usize,
    input_max_vscroll: usize,
    monochrome: bool,
    chat_bg: Option<Color>,
    input_bg: Option<Color>,
    status_bg: Option<Color>,
    /// Number of statusline rows (1-3), fixed by the statusline config at startup.
    statusline_height: usize,
    /// Left padding (columns) for the chat output only. Input and status
    /// rows are unaffected.
    chat_margin: u16,
    /// Whether to prepend `[HH:MM:SS]` to committed chat rows and draw faint
    /// separators between turns. Mirrors `Session::show_timestamps`.
    show_timestamps: bool,
    pub permission_prompt: Option<PermissionPrompt>,
    pub chain_prompt: Option<ChainPrompt>,
    pub chain_but_mode: bool,
    /// Print watermark: feed lines before it are committed to scrollback.
    /// `None` means nothing has been printed yet.
    watermark: Option<Watermark>,
    /// Rows occupied by the live block after the last draw (0 = never drawn).
    live_height: usize,
    /// Offset of the post-draw cursor within the live block, counted from the
    /// block's first row. Moving up this many rows reaches the block start.
    live_cursor_off: usize,
    /// Screen column + block-row offset of the input caret after the last
    /// full draw; `None` when the caret is hidden (permission/chain prompts).
    live_caret: Option<(u16, usize)>,
    /// Streaming-region and input rows of the last full draw, used by the
    /// statusline-only redraw to locate the statusline rows relatively.
    live_stream_rows: usize,
    live_input_rows: usize,
    /// Picker-overlay rows of the last full draw (0 = no picker painted).
    live_picker_rows: usize,
    /// Whether an OSC 133 output region (C...D) is currently open.
    output_mark_open: bool,
    /// Set when the live block was erased externally (`suspend`/`finish` or a
    /// flush that printed over it): the cursor already sits where the next
    /// draw must start, so no upward move is needed.
    live_erased: bool,
    /// Dirty flag plus snapshot of the state recorded after the last
    /// successful live-block draw.
    bottom_dirty: bool,
    last_bottom_snapshot: Option<BottomSnapshot>,
    last_live_args: Option<LiveArgs>,
    /// Last time the live block was actually redrawn. Used to throttle
    /// streaming repaints to ~60 Hz and reduce flicker on fast token streams.
    last_repaint: std::time::Instant,
    /// True when a streaming repaint was requested but deferred because the
    /// 16 ms frame budget had not elapsed yet.
    repaint_pending: bool,
    /// Whether the terminal window currently has focus. The spinner is paused
    /// while focus is lost to avoid purely cosmetic redraws.
    focused: bool,
}

impl Renderer {
    pub fn new() -> io::Result<Self> {
        Ok(Self::with_backend(Box::new(CrosstermBackend {
            stdout: io::stdout(),
        })))
    }

    pub(crate) fn with_backend(backend: Box<dyn RenderBackend>) -> Self {
        Renderer {
            backend,
            spinner_frame: 0,
            feed: Feed::new(),
            partial: CompactString::new(""),
            partial_style: BlockStyle::Plain,
            input_vscroll_offset: 0,
            input_max_vscroll: 0,
            monochrome: false,
            chat_bg: None,
            input_bg: None,
            status_bg: None,
            statusline_height: 1,
            chat_margin: 0,
            show_timestamps: false,
            permission_prompt: None,
            chain_prompt: None,
            chain_but_mode: false,
            watermark: None,
            live_height: 0,
            live_cursor_off: 0,
            live_caret: None,
            live_stream_rows: 0,
            live_input_rows: 0,
            live_picker_rows: 0,
            output_mark_open: false,
            live_erased: false,
            bottom_dirty: true,
            last_bottom_snapshot: None,
            last_live_args: None,
            last_repaint: std::time::Instant::now(),
            repaint_pending: false,
            focused: true,
        }
    }

    /// Queue a crossterm command to the backend and flush, mirroring
    /// `ExecutableCommand::execute` semantics.
    fn exec(&mut self, command: impl crossterm::Command) -> io::Result<()> {
        self.backend.queue(command)?;
        self.backend.flush()
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

    /// Set the chat output's left padding in columns. Clamped so content keeps
    /// at least a few usable columns.
    pub fn set_chat_margin(&mut self, margin: u16) {
        let (cols, _) = self.terminal_size();
        self.chat_margin = margin.min(cols.saturating_sub(8));
    }

    /// Enable or disable per-message timestamps and turn separators.
    pub fn set_show_timestamps(&mut self, show: bool) {
        self.show_timestamps = show;
    }

    /// Emit the chat left-margin gutter (spaces in the chat background) at the
    /// current cursor position. Caller has already positioned to column 0 and
    /// set the background.
    fn write_chat_margin(margin: u16, stdout: &mut impl Write) -> io::Result<()> {
        if margin > 0 {
            write!(stdout, "{}", " ".repeat(margin as usize))?;
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
        self.backend.size().unwrap_or((80, 24))
    }

    fn max_line_width(&self) -> usize {
        let (cols, _) = self.terminal_size();
        cols.saturating_sub(1 + self.chat_margin) as usize
    }

    #[cfg(test)]
    pub fn line_width(&self) -> usize {
        self.max_line_width()
    }

    /// Access the underlying feed for callers that want to push semantic blocks
    /// directly (e.g., session rendering or streaming agent responses).
    pub fn feed(&self) -> &Feed {
        &self.feed
    }

    pub fn feed_mut(&mut self) -> &mut Feed {
        &mut self.feed
    }

    /// Test helper: everything written to a [`FakeBackend`] so far (empty on
    /// real backends).
    #[cfg(test)]
    pub(crate) fn captured_output(&self) -> String {
        self.backend.captured().unwrap_or_default()
    }

    /// Test helper: the current print watermark.
    #[cfg(test)]
    pub(crate) fn watermark(&self) -> Option<Watermark> {
        self.watermark
    }

    /// Test helper: resize the pinned test backend geometry.
    #[cfg(test)]
    pub(crate) fn resize_backend(&mut self, cols: u16, rows: u16) {
        self.backend.set_size(cols, rows);
    }

    /// Whether the renderer drives a headless test backend instead of a real
    /// terminal. Used to skip code paths that bypass the backend abstraction
    /// and write straight to stdout (e.g. the pickers), which break on CI
    /// where stdout is a non-blocking pipe.
    pub(crate) fn is_headless(&self) -> bool {
        #[cfg(test)]
        {
            self.backend.captured().is_some()
        }
        #[cfg(not(test))]
        {
            false
        }
    }

    /// Number of rows the input area will occupy for the given content. Kept in
    /// sync with the height logic used while drawing the input in
    /// `draw_live_block`.
    fn input_visible_height(&self, input_line: &str, rows: u16) -> usize {
        if self.permission_prompt.is_some() || self.chain_prompt.is_some() {
            return 2;
        }
        let (cols, _) = self.terminal_size();
        // The prompt ("> " or a spinner frame) takes two columns; the text
        // soft-wraps at the rest.
        let text_width = (cols.saturating_sub(2) as usize).max(1);
        let available_rows = rows.saturating_sub(self.statusline_reserve()) as usize;
        let max_input_rows = available_rows.min((available_rows * 3 / 10).max(5));
        let wrapped_rows: usize = input_line
            .split('\n')
            .map(|line| wrap_input_segments(line, text_width).len())
            .sum();
        wrapped_rows.min(max_input_rows).max(1)
    }

    /// Rows available above the input area for the streaming region (tail of
    /// the running block + partial scratch).
    fn streaming_rows(&self, input_line: &str, rows: u16) -> usize {
        let input_h = self.input_visible_height(input_line, rows);
        (rows as usize).saturating_sub(input_h + self.statusline_height + 2)
    }

    /// Rows below the picker overlay section: input area + separators +
    /// statusline. Picker view builders use it to cap their list height.
    pub(crate) fn rows_below_picker(&self, input_line: &str) -> usize {
        let (_, rows) = self.terminal_size();
        self.input_visible_height(input_line, rows) + self.statusline_height + 2
    }

    /// Emit an OSC 133 shell-integration sequence through the backend.
    /// `code` is the single-letter phase (A/B/C/D); `args` is appended after
    /// a semicolon when non-empty. The 7-bit ST (`ESC \\`) terminator is used.
    pub(crate) fn osc_133(&mut self, code: char, args: &str) -> io::Result<()> {
        if args.is_empty() {
            write!(self.backend, "\x1b]133;{}\x1b\\", code)
        } else {
            write!(self.backend, "\x1b]133;{};{}\x1b\\", code, args)
        }
    }

    /// Open an OSC 133 output region (C) if one is not already open.
    pub(crate) fn start_output_mark(&mut self) -> io::Result<()> {
        if !self.output_mark_open {
            self.osc_133('C', "")?;
            self.output_mark_open = true;
        }
        Ok(())
    }

    /// Close the current OSC 133 output region (D).
    pub(crate) fn end_output_mark(&mut self) -> io::Result<()> {
        if self.output_mark_open {
            self.osc_133('D', "")?;
            self.output_mark_open = false;
        }
        Ok(())
    }

    pub(crate) fn commit_partial(&mut self) {
        if !self.partial.is_empty() {
            self.feed
                .push_block(self.partial_style, self.partial.as_str());
            self.partial.clear();
        }
    }

    /// The partial scratch (an incomplete line accumulating via
    /// [`Renderer::write`]) laid out at `width`; shown in the live block's
    /// streaming region but never committed.
    fn partial_lines(&self, width: usize) -> Vec<LineEntry> {
        if self.partial.is_empty() {
            return Vec::new();
        }
        let color = self.partial_style.color();
        word_wrap(&self.partial, width)
            .into_iter()
            .map(|text| LineEntry::new(text, color, self.partial_style))
            .collect()
    }

    /// The watermark clamped to the current feed, defaulting to the start of
    /// the feed when nothing was printed yet.
    fn current_watermark(&self, width: usize) -> Watermark {
        let wm = self.watermark.unwrap_or(Watermark {
            width,
            block: 0,
            line: 0,
        });
        clamp_watermark(wm, self.feed.block_count())
    }

    /// Split the pending (unprinted) content into committed candidates and
    /// the live tail at the current layout.
    fn pending_split(&self, width: usize, wm: Watermark) -> PendingSplit {
        let mut feed_lines: Vec<LineEntry> = Vec::new();
        let mut counts = Vec::new();
        let mut pushed = Vec::new();
        for b in wm.block..self.feed.block_count() {
            let lines = self.feed.block_lines(b, width);
            counts.push(lines.len());
            let start = if b == wm.block {
                wm.line.min(lines.len())
            } else {
                0
            };
            pushed.push(lines.len() - start);
            feed_lines.extend(lines.into_iter().skip(start));
        }
        let running_tail = if self.feed.last_block_running() {
            pushed.last().copied().unwrap_or(0)
        } else {
            0
        };
        let running_tail = running_tail.min(feed_lines.len());
        let mut live = feed_lines.split_off(feed_lines.len() - running_tail);
        let live_feed = live.len();
        live.extend(self.partial_lines(width));
        PendingSplit {
            committed: feed_lines,
            live,
            live_feed,
            counts,
        }
    }

    /// Move the cursor to the first row of the live block, relatively (never
    /// above it). No-op when the block was never drawn or was just erased.
    fn move_to_block_start(&mut self) -> io::Result<()> {
        if self.live_height == 0 || self.live_erased {
            return Ok(());
        }
        if self.live_cursor_off > 0 {
            write!(self.backend, "{}", MoveUp(self.live_cursor_off as u16))?;
        }
        write!(self.backend, "\r")?;
        Ok(())
    }

    /// Format a timestamp as `[HH:MM:SS] ` when timestamps are enabled and the
    /// line carries one; otherwise returns `None`.
    fn timestamp_prefix(
        &self,
        created_at: Option<chrono::DateTime<chrono::Local>>,
    ) -> Option<String> {
        if !self.show_timestamps {
            return None;
        }
        created_at.map(|t| format!("[{}] ", t.format("%H:%M:%S")))
    }

    /// Draw a faint horizontal rule across the full terminal width at the
    /// current cursor position; no trailing newline.
    fn write_separator_row(&mut self) -> io::Result<()> {
        let (cols, _) = self.terminal_size();
        write!(self.backend, "\r")?;
        if let Some(bg) = self.chat_bg {
            let bg = self.color(bg);
            write!(self.backend, "{}", SetBackgroundColor(bg))?;
        }
        Self::write_chat_margin(self.chat_margin, &mut self.backend)?;
        let fg = self.color(Color::DarkGrey);
        write!(self.backend, "{}", SetForegroundColor(fg))?;
        let sep: String = "─".repeat((cols as usize).saturating_sub(self.chat_margin as usize));
        write!(self.backend, "{}", sep)?;
        write!(self.backend, "{}", ResetColor)?;
        Ok(())
    }

    /// Write one chat-style row (chat background, left margin, role color,
    /// OSC8-wrapped text) at the current cursor position; no trailing newline.
    /// When `prefix` is provided it is painted in dark grey before the row text.
    fn write_chat_row(&mut self, entry: &LineEntry, prefix: Option<&str>) -> io::Result<()> {
        let is_code = entry.style == BlockStyle::Code;
        write!(self.backend, "\r")?;
        if is_code {
            if self.monochrome {
                write!(self.backend, "{}", SetAttribute(Attribute::Reverse))?;
            } else {
                let bg = self.color(Color::DarkGrey);
                write!(self.backend, "{}", SetBackgroundColor(bg))?;
            }
        } else if let Some(bg) = self.chat_bg {
            let bg = self.color(bg);
            write!(self.backend, "{}", SetBackgroundColor(bg))?;
        }
        Self::write_chat_margin(self.chat_margin, &mut self.backend)?;
        if let Some(prefix) = prefix {
            if is_code {
                // Timestamps should not appear inside code blocks; ignore the
                // prefix so the header/code lines stay visually clean.
            } else {
                let prefix_color = self.color(Color::DarkGrey);
                write!(self.backend, "{}", SetForegroundColor(prefix_color))?;
                write!(self.backend, "{}", prefix)?;
                write!(self.backend, "{}", ResetColor)?;
                if let Some(bg) = self.chat_bg {
                    let bg = self.color(bg);
                    write!(self.backend, "{}", SetBackgroundColor(bg))?;
                }
            }
        }
        let fg = self.color(entry.color);
        write!(self.backend, "{}", SetForegroundColor(fg))?;
        write!(self.backend, "{}", wrap_urls_osc8(&entry.text))?;
        if is_code {
            // Fill the rest of the content area so the background spans the
            // full line width.
            let pad = self
                .max_line_width()
                .saturating_sub(display_width(&entry.text));
            if pad > 0 {
                write!(self.backend, "{}", " ".repeat(pad))?;
            }
            write!(self.backend, "{}", Clear(ClearType::UntilNewLine))?;
            if self.monochrome {
                write!(self.backend, "{}", SetAttribute(Attribute::Reset))?;
            }
        } else {
            write!(self.backend, "{}", Clear(ClearType::UntilNewLine))?;
        }
        write!(self.backend, "{}", ResetColor)?;
        Ok(())
    }

    /// Commit pending feed lines to the terminal's native scrollback: print
    /// the finalized lines past the watermark (plus any live-tail overflow)
    /// once, starting at the live block's top edge. The caller redraws the
    /// live block right after.
    pub fn flush_committed(&mut self, input_line: &str) -> io::Result<()> {
        let (_, rows) = self.terminal_size();
        let width = self.max_line_width();
        let stream_rows = self.streaming_rows(input_line, rows);
        let wm = self.current_watermark(width);
        let split = self.pending_split(width, wm);
        let partial_rows = split.live.len() - split.live_feed;
        let print = commit_count(
            split.committed.len(),
            split.live_feed,
            partial_rows,
            stream_rows,
        );
        if print > 0 {
            write!(self.backend, "{}", Hide)?;
            self.move_to_block_start()?;
            let spill = print - split.committed.len();
            let mut lines = split.committed;
            lines.extend(split.live.iter().take(spill).cloned());
            let mut first_line = true;
            for entry in &lines {
                if self.show_timestamps && entry.block_start && !first_line {
                    self.write_separator_row()?;
                    writeln!(self.backend)?;
                }
                first_line = false;
                if !self.output_mark_open && entry.style != BlockStyle::User {
                    self.start_output_mark()?;
                }
                let prefix = self.timestamp_prefix(entry.created_at);
                self.write_chat_row(entry, prefix.as_deref())?;
                writeln!(self.backend)?;
            }
            self.backend.flush()?;
            self.watermark = Some(advance_watermark(wm, &split.counts, print));
            // The printed lines overwrote (or scrolled past) the old live
            // block; the next draw starts right where the cursor is.
            self.live_erased = true;
        } else {
            self.watermark = Some(Watermark { width, ..wm });
        }
        Ok(())
    }

    /// Request a live-block redraw on the next frame budget tick. Cheap to
    /// call on every streamed token; the actual paint is throttled to ~60 Hz.
    pub fn request_repaint(&mut self) {
        self.repaint_pending = true;
    }

    /// Set whether the terminal window has focus. When focus is lost the
    /// spinner frame is frozen to avoid cosmetic redraws.
    pub(crate) fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// Current spinner frame, for tests that verify focus pauses the spinner.
    #[cfg(test)]
    pub(crate) fn spinner_frame(&self) -> u8 {
        self.spinner_frame
    }

    /// True when a streaming repaint was requested but not yet painted.
    pub fn needs_redraw(&self) -> bool {
        self.repaint_pending
    }

    fn repaint_budget_available(&self) -> bool {
        self.last_repaint.elapsed() >= Duration::from_millis(16)
    }

    /// Flush committed lines and redraw the live block with the most recent
    /// draw arguments; used by streaming, which mutates the feed between
    /// frames. The live-block draw is throttled to ~60 Hz unless `force` is
    /// true; committed lines are always flushed immediately.
    pub fn repaint(&mut self, force: bool) -> io::Result<()> {
        let Some(args) = self.last_live_args.clone() else {
            return self.flush_committed("");
        };
        self.flush_committed(&args.input)?;
        if !force && !self.repaint_budget_available() {
            self.repaint_pending = true;
            return Ok(());
        }
        self.repaint_pending = false;
        self.last_repaint = Instant::now();
        self.draw_live_block(
            &args.input,
            args.cursor,
            &args.statusline,
            args.is_running,
            args.picker.as_ref(),
        )
    }

    pub fn write_line(&mut self, text: &str, color: Color) -> io::Result<()> {
        self.commit_partial();
        let style = style_from_color(color);
        self.feed.push_block(style, text);
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
        Ok(())
    }

    /// Terminal resized: printed lines keep their old wrap; the watermark
    /// keeps the same `(block, line)` position and is remapped to the new
    /// width on the next flush; only the live block is redrawn against the
    /// new geometry.
    pub fn resize(&mut self) {
        self.bottom_dirty = true;
    }

    /// `/clear`: erase the live block, wipe the visible screen shell-style
    /// (native scrollback is kept), and reset the print watermark so the
    /// rebuilt feed prints once from the top.
    pub fn clear_screen(&mut self) -> io::Result<()> {
        write!(self.backend, "{}", Hide)?;
        self.move_to_block_start()?;
        write!(self.backend, "{}", Clear(ClearType::FromCursorDown))?;
        self.exec(Clear(ClearType::All))?;
        self.exec(MoveTo(0, 0))?;
        self.backend.flush()?;
        self.watermark = None;
        self.live_height = 0;
        self.live_cursor_off = 0;
        self.live_caret = None;
        self.live_picker_rows = 0;
        self.output_mark_open = false;
        self.live_erased = false;
        self.bottom_dirty = true;
        Ok(())
    }

    /// Drop all feed content and the partial scratch in preparation for a
    /// rebuild from the session (see `events::render_session`).
    pub fn reset_feed_for_rebuild(&mut self) {
        self.feed.clear();
        self.partial.clear();
    }

    /// Reconcile the watermark after the feed was rebuilt from the session
    /// (resume/undo/redo/rewind). Already-printed content cannot be unprinted
    /// and must not print twice, so when anything was committed the whole
    /// rebuilt feed is marked as printed; the caller is expected to print a
    /// banner explaining the change. At startup (nothing printed yet) the
    /// watermark stays unset so the transcript prints once.
    pub fn note_feed_rebuilt(&mut self) {
        if let Some(wm) = self.watermark {
            self.watermark = Some(Watermark {
                block: self.feed.block_count(),
                line: 0,
                ..wm
            });
        }
    }

    /// Suspend the inline UI to run a full-screen external program ($EDITOR,
    /// lazygit, less): erase the live block and leave raw mode. Pair with
    /// [`Renderer::resume`].
    pub fn suspend(&mut self) -> io::Result<()> {
        self.move_to_block_start()?;
        write!(self.backend, "{}", Clear(ClearType::FromCursorDown))?;
        write!(self.backend, "{}", Show)?;
        self.backend.flush()?;
        self.live_erased = true;
        crossterm::terminal::disable_raw_mode()
    }

    /// Re-enter raw mode after [`Renderer::suspend`]; the next
    /// `draw_live_block` repaints the live block where the cursor sits
    /// (external full-screen programs restore the cursor to where they
    /// started, i.e. the live block's former top edge).
    pub fn resume(&mut self) -> io::Result<()> {
        crossterm::terminal::enable_raw_mode()?;
        self.bottom_dirty = true;
        Ok(())
    }

    /// Erase the live block and leave the cursor on a fresh line below the
    /// UI, for program exit (before the terminal guard restores the screen).
    pub fn finish(&mut self) -> io::Result<()> {
        self.move_to_block_start()?;
        write!(self.backend, "{}", Clear(ClearType::FromCursorDown))?;
        write!(self.backend, "{}", Show)?;
        self.backend.flush()?;
        self.live_height = 0;
        self.live_erased = true;
        Ok(())
    }

    /// Write the thin separator row at the current cursor position; no
    /// trailing newline.
    fn draw_separator_line(&mut self, cols: u16) -> io::Result<()> {
        write!(self.backend, "\r")?;
        if let Some(bg) = self.input_bg {
            let bg = self.color(bg);
            write!(self.backend, "{}", SetBackgroundColor(bg))?;
        }
        let fg = self.color(Color::DarkGrey);
        write!(self.backend, "{}", SetForegroundColor(fg))?;
        let sep: String = "─".repeat(cols as usize);
        write!(self.backend, "{}", sep)?;
        write!(self.backend, "{}", ResetColor)?;
        Ok(())
    }

    /// Draw the statusline (1-3 rows) at the current cursor position. Each
    /// `Flex` span expands to fill the remaining width. Every row except the
    /// last is followed by a newline. Fewer lines than `statusline_height`
    /// leaves the upper statusline rows blank.
    fn draw_statusline_rows(
        &mut self,
        statusline: &[Vec<StatusSpan>],
        cols: u16,
    ) -> io::Result<()> {
        let h = self.statusline_height;
        for row_idx in 0..h {
            let empty: Vec<StatusSpan> = Vec::new();
            let spans = statusline.get(row_idx).unwrap_or(&empty);
            self.write_statusline_row(spans, cols)?;
            if row_idx + 1 < h {
                writeln!(self.backend)?;
            }
        }
        Ok(())
    }

    /// Write one statusline row at the current cursor position; no trailing
    /// newline.
    fn write_statusline_row(&mut self, spans: &[StatusSpan], cols: u16) -> io::Result<()> {
        write!(self.backend, "\r")?;
        if let Some(bg) = self.status_bg {
            let bg = self.color(bg);
            write!(self.backend, "{}", SetBackgroundColor(bg))?;
        }

        let total = cols as usize;
        let mut budget = total;

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
                        let c = self.color(c);
                        write!(self.backend, "{}", SetBackgroundColor(c))?;
                    }
                    let fgc = fg.unwrap_or(Color::DarkGrey);
                    let fgc = self.color(fgc);
                    write!(self.backend, "{}", SetForegroundColor(fgc))?;
                    let piece: String = text.chars().take(budget).collect();
                    budget = budget.saturating_sub(display_width(&piece));
                    write!(self.backend, "{}", piece)?;
                    write!(self.backend, "{}", ResetColor)?;
                    if let Some(bg) = self.status_bg {
                        let bg = self.color(bg);
                        write!(self.backend, "{}", SetBackgroundColor(bg))?;
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
                    write!(self.backend, "{}", " ".repeat(width))?;
                }
            }
        }

        write!(self.backend, "{}", Clear(ClearType::UntilNewLine))?;
        write!(self.backend, "{}", ResetColor)?;
        Ok(())
    }

    /// Repaint only the statusline rows. Valid right after a full draw whose
    /// geometry matches the current frame (enforced by `bottom_redraw_plan`).
    fn redraw_statusline_only(
        &mut self,
        statusline: &[Vec<StatusSpan>],
        cols: u16,
    ) -> io::Result<()> {
        let status_start =
            self.live_stream_rows + self.live_picker_rows + 1 + self.live_input_rows + 1;
        // Move from the post-draw cursor position to the first statusline row.
        if status_start > self.live_cursor_off {
            write!(
                self.backend,
                "{}",
                MoveDown((status_start - self.live_cursor_off) as u16)
            )?;
        } else if status_start < self.live_cursor_off {
            write!(
                self.backend,
                "{}",
                MoveUp((self.live_cursor_off - status_start) as u16)
            )?;
        }
        write!(self.backend, "\r{}", Hide)?;
        self.draw_statusline_rows(statusline, cols)?;
        Ok(())
    }

    /// Re-place the terminal caret where the last full draw left it. Needed
    /// after a statusline-only redraw, which moves the cursor.
    fn restore_live_cursor(&mut self) -> io::Result<()> {
        match self.live_caret {
            Some((x, off)) => {
                let up = self.live_height.saturating_sub(1).saturating_sub(off);
                if up > 0 {
                    write!(self.backend, "{}", MoveUp(up as u16))?;
                }
                write!(self.backend, "\r")?;
                if x > 0 {
                    write!(self.backend, "{}", MoveRight(x))?;
                }
                write!(self.backend, "{}", Show)?;
            }
            None => {
                write!(self.backend, "{}", Hide)?;
            }
        }
        self.backend.flush()
    }

    /// Snapshot of everything `draw_live_block` would paint for these arguments.
    #[allow(clippy::too_many_arguments)]
    fn bottom_snapshot(
        &self,
        input_line: &str,
        cursor_pos: usize,
        statusline: &[Vec<StatusSpan>],
        is_running: bool,
        picker: Option<&PickerView>,
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
            prompt,
            picker: picker.cloned(),
            statusline: statusline.to_vec(),
            feed_generation: self.feed.generation(),
            watermark: self.watermark,
            partial: self.partial.clone(),
            partial_style: self.partial_style,
            monochrome: self.monochrome,
            chat_bg: self.chat_bg,
            chat_margin: self.chat_margin,
            input_bg: self.input_bg,
            status_bg: self.status_bg,
            show_timestamps: self.show_timestamps,
        }
    }

    /// Pure redraw decision for the live block: compare the state recorded
    /// after the last draw with the state about to be drawn. When only the
    /// statusline content differs, the rest of the block is untouched and
    /// only the statusline rows need a repaint.
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
        if status_only == *prev {
            BottomRedrawPlan::StatuslineOnly
        } else {
            BottomRedrawPlan::Full
        }
    }

    /// Record the live block as freshly drawn.
    fn record_bottom_drawn(&mut self, snapshot: BottomSnapshot) {
        self.last_bottom_snapshot = Some(snapshot);
        self.bottom_dirty = false;
    }

    /// Draw the live block — the only region ever redrawn: streaming region
    /// (tail of the running feed block + partial scratch), then the picker
    /// overlay (when one is active), then the input box (or a permission/chain
    /// prompt) between two separators, then the statusline. All positioning is
    /// relative to the block's previous position: the cursor never moves above
    /// the block's start, so committed lines above it are left untouched in
    /// native scrollback. Because the picker is a block section, its rows are
    /// always covered by the block's erase/redraw — nothing it painted can be
    /// left behind when it closes or shrinks.
    pub fn draw_live_block(
        &mut self,
        input_line: &str,
        cursor_pos: usize,
        statusline: &[Vec<StatusSpan>],
        is_running: bool,
        picker: Option<&PickerView>,
    ) -> io::Result<()> {
        self.last_live_args = Some(LiveArgs {
            input: input_line.to_string(),
            cursor: cursor_pos,
            statusline: statusline.to_vec(),
            is_running,
            picker: picker.cloned(),
        });
        let (cols, rows) = self.backend.size()?;
        let snapshot = self.bottom_snapshot(
            input_line, cursor_pos, statusline, is_running, picker, cols, rows,
        );
        match Self::bottom_redraw_plan(
            self.last_bottom_snapshot.as_ref(),
            &snapshot,
            self.bottom_dirty,
        ) {
            BottomRedrawPlan::Skip => return Ok(()),
            BottomRedrawPlan::StatuslineOnly => {
                self.redraw_statusline_only(statusline, cols)?;
                self.restore_live_cursor()?;
                self.record_bottom_drawn(snapshot);
                return Ok(());
            }
            BottomRedrawPlan::Full => {}
        }

        write!(self.backend, "{}", Hide)?;
        self.move_to_block_start()?;
        self.live_erased = false;
        write!(self.backend, "{}", Clear(ClearType::FromCursorDown))?;

        let width = cols.saturating_sub(1 + self.chat_margin) as usize;
        let picker_rows = picker.map(PickerView::height).unwrap_or(0);
        // The picker overlay takes rows from the streaming region, not from
        // the input area.
        let stream_rows = self
            .streaming_rows(input_line, rows)
            .saturating_sub(picker_rows);
        let wm = self.current_watermark(width);
        let split = self.pending_split(width, wm);
        let skip = split.live.len().saturating_sub(stream_rows);
        let stream = &split.live[skip..];

        // Streaming region: tail of the running block plus the partial scratch.
        for entry in stream {
            self.write_chat_row(entry, None)?;
            writeln!(self.backend)?;
        }
        let stream_len = stream.len();

        // Picker overlay: a block section like any other, so the next erase
        // covers it (close, shrink and resize all redraw the whole block).
        if let Some(view) = picker {
            if let Some(ref header) = view.header {
                write!(self.backend, "\r")?;
                let fg = self.color(Color::DarkGrey);
                write!(self.backend, "{}", SetForegroundColor(fg))?;
                write!(self.backend, "{}", header)?;
                write!(self.backend, "{}", Clear(ClearType::UntilNewLine))?;
                write!(self.backend, "{}", ResetColor)?;
                writeln!(self.backend)?;
            }
            for row in &view.rows {
                write!(self.backend, "\r")?;
                let fg = self.color(if row.selected {
                    Color::Green
                } else {
                    Color::DarkGrey
                });
                write!(self.backend, "{}", SetForegroundColor(fg))?;
                write!(self.backend, "{}", row.text)?;
                write!(self.backend, "{}", Clear(ClearType::UntilNewLine))?;
                write!(self.backend, "{}", ResetColor)?;
                writeln!(self.backend)?;
            }
        }

        let prompt_lines: Option<[String; 2]> = if let Some(ref pp) = self.permission_prompt {
            Some([pp.tool.to_string(), pp.options.to_string()])
        } else {
            self.chain_prompt.as_ref().map(|cp| {
                let options = if self.chain_but_mode {
                    "[Enter] send  [Esc] cancel"
                } else {
                    "[Y] Yes  [N] No  [B] yes, But (add instruction)"
                };
                [cp.question.to_string(), options.to_string()]
            })
        };

        if let Some(perm_lines) = prompt_lines {
            let prompt_color = self.color(Color::DarkYellow);
            self.draw_separator_line(cols)?;
            writeln!(self.backend)?;
            self.osc_133('A', "")?;
            for line in &perm_lines {
                write!(self.backend, "\r")?;
                if let Some(bg) = self.input_bg {
                    let bg = self.color(bg);
                    write!(self.backend, "{}", SetBackgroundColor(bg))?;
                }
                write!(self.backend, "{}", SetForegroundColor(prompt_color))?;
                write!(self.backend, "{}", line)?;
                write!(self.backend, "{}", Clear(ClearType::UntilNewLine))?;
                write!(self.backend, "{}", ResetColor)?;
                writeln!(self.backend)?;
            }
            self.draw_separator_line(cols)?;
            writeln!(self.backend)?;
            self.draw_statusline_rows(statusline, cols)?;
            self.backend.flush()?;
            let total = stream_len + 1 + 2 + 1 + self.statusline_height;
            self.live_height = total;
            self.live_stream_rows = stream_len;
            self.live_input_rows = 2;
            self.live_picker_rows = 0;
            self.live_cursor_off = total - 1;
            self.live_caret = None;
            self.record_bottom_drawn(snapshot);
            return Ok(());
        }

        let lines: SmallVec<[&str; 4]> = input_line.split('\n').collect();

        let reserve = self.statusline_reserve();
        let available_rows = (rows.saturating_sub(reserve) as usize).max(1);
        // Cap the input height to roughly 30% of the area so the streaming
        // region stays visible above a tall input instead of being squeezed
        // to nothing.
        let max_input_rows = available_rows.min((available_rows * 3 / 10).max(5));

        const SPINNER: &[&str] = &["⠋ ", "⠙ ", "⠹ ", "⠸ ", "⠼ ", "⠴ ", "⠦ ", "⠧ ", "⠇ ", "⠏ "];
        let prompt = if is_running {
            let frame = SPINNER[self.spinner_frame as usize];
            if self.focused {
                self.spinner_frame = (self.spinner_frame + 1) % SPINNER.len() as u8;
            }
            frame
        } else {
            "> "
        };
        let prompt_width = display_width(prompt);
        let text_width = (cols.saturating_sub(prompt_width as u16) as usize).max(1);

        let (cursor_line, cursor_col) =
            crate::ui::input::cursor_to_line_col(input_line, cursor_pos);

        // Soft-wrap each hard line at the text width: long lines occupy extra
        // display rows instead of scrolling horizontally. Locate the cursor's
        // wrapped row and column-in-row while flattening.
        let mut wrapped: Vec<(&str, usize, usize)> = Vec::new();
        let mut cursor_row = 0usize;
        let mut cursor_col_in_row = 0usize;
        for (i, line) in lines.iter().enumerate() {
            let segments = wrap_input_segments(line, text_width);
            if i == cursor_line {
                // Convert the cursor's char-index within the hard line to a
                // byte offset, then to a wrapped row + display column.
                let cursor_byte = line
                    .char_indices()
                    .nth(cursor_col)
                    .map(|(b, _)| b)
                    .unwrap_or(line.len());
                let (row, col) = wrapped_cursor(&segments, line, cursor_byte);
                cursor_row = wrapped.len() + row;
                cursor_col_in_row = col;
            }
            for &(start, end) in &segments {
                wrapped.push((line, start, end));
            }
        }
        let row_count = wrapped.len();
        let need_scroll = row_count > max_input_rows;

        // Vertical scroll: keep the cursor's display row within the visible
        // window so pressing Up/Down can reveal rows that don't fit at once.
        let first_visible = if need_scroll {
            self.input_max_vscroll = row_count - max_input_rows;
            if cursor_row < self.input_vscroll_offset {
                self.input_vscroll_offset = cursor_row;
            } else if cursor_row >= self.input_vscroll_offset + max_input_rows {
                self.input_vscroll_offset = cursor_row - max_input_rows + 1;
            }
            self.input_vscroll_offset = self.input_vscroll_offset.min(self.input_max_vscroll);
            self.input_vscroll_offset
        } else {
            self.input_vscroll_offset = 0;
            self.input_max_vscroll = 0;
            0
        };

        let visible_line_count = if need_scroll {
            max_input_rows
        } else {
            row_count
        };

        // Thin separator line above input
        self.draw_separator_line(cols)?;
        writeln!(self.backend)?;

        // OSC 133 prompt start: right before the input prompt row.
        self.osc_133('A', "")?;

        for (i, &(line, start, end)) in wrapped
            .iter()
            .enumerate()
            .skip(first_visible)
            .take(visible_line_count)
        {
            write!(self.backend, "\r")?;

            if let Some(bg) = self.input_bg {
                let bg = self.color(bg);
                write!(self.backend, "{}", SetBackgroundColor(bg))?;
            }

            if i == first_visible {
                let fg = self.color(Color::DarkYellow);
                write!(self.backend, "{}", SetForegroundColor(fg))?;
                write!(self.backend, "{}", prompt)?;
                write!(self.backend, "{}", SetForegroundColor(Color::Reset))?;
            } else {
                write!(self.backend, "{}", " ".repeat(prompt_width))?;
            }

            write!(self.backend, "{}", &line[start..end])?;
            write!(self.backend, "{}", Clear(ClearType::UntilNewLine))?;
            write!(self.backend, "{}", ResetColor)?;
            writeln!(self.backend)?;
        }

        // Thin separator line below input
        self.draw_separator_line(cols)?;
        writeln!(self.backend)?;

        // Statusline rows (no trailing newline after the last one).
        self.draw_statusline_rows(statusline, cols)?;

        // Caret. Clamp to the visible input rows so that when the input is
        // scrolled away from the cursor row, the terminal caret stays inside
        // the input box instead of spilling onto the separator or status bar.
        let cursor_render_idx = cursor_row
            .saturating_sub(first_visible)
            .min(visible_line_count.saturating_sub(1));
        let caret_off = stream_len + picker_rows + 1 + cursor_render_idx;
        let cursor_x = (prompt_width + cursor_col_in_row) as u16;
        let cursor_x = cursor_x.min(cols.saturating_sub(1));
        let total = stream_len + picker_rows + 1 + visible_line_count + 1 + self.statusline_height;
        // The cursor sits at the end of the last statusline row; move it up
        // to the caret row.
        let up = (total - 1).saturating_sub(caret_off);
        if up > 0 {
            write!(self.backend, "{}", MoveUp(up as u16))?;
        }
        write!(self.backend, "\r")?;
        if cursor_x > 0 {
            write!(self.backend, "{}", MoveRight(cursor_x))?;
        }
        write!(self.backend, "{}", Show)?;
        self.backend.flush()?;
        self.live_height = total;
        self.live_stream_rows = stream_len;
        self.live_input_rows = visible_line_count;
        self.live_picker_rows = picker_rows;
        self.live_cursor_off = caret_off;
        self.live_caret = Some((cursor_x, caret_off));
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

/// Copy `text` to the system clipboard. Tries external tools (checking
/// their exit status, so a tool that starts but fails — e.g. xclip without
/// an X display — falls through) and finally the OSC 52 terminal escape.
/// Errors only when even the escape cannot be written.
///
/// Only the MCP OAuth login flow copies to the clipboard today, so this is
/// compiled just for that feature (and its tests).
#[cfg(any(feature = "mcp", test))]
pub fn copy_to_clipboard(text: &str) -> anyhow::Result<()> {
    let cmds: &[(&str, &[&str])] = &[
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("pbcopy", &[]),
        ("clip.exe", &[]),
    ];
    for &(cmd, args) in cmds {
        let Ok(mut child) = std::process::Command::new(cmd)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        else {
            continue; // tool not installed
        };
        let wrote = match child.stdin.take() {
            Some(mut stdin) => {
                let ok = stdin.write_all(text.as_bytes()).is_ok();
                drop(stdin); // close stdin so the tool sees EOF
                ok
            }
            None => false,
        };
        if wrote && matches!(child.wait(), Ok(status) if status.success()) {
            return Ok(());
        }
    }

    // OSC 52 escape sequence — clipboard access via terminal emulator.
    // Supported by Kitty, Alacritty, WezTerm, foot, iTerm2, Windows Terminal,
    // and most other modern terminals. No external tools needed.
    let encoded = base64_encode(text.as_bytes());
    let mut stdout = std::io::stdout().lock();
    write!(stdout, "\x1b]52;c;{encoded}\x07")?;
    stdout.flush()?;
    Ok(())
}

/// Minimal base64 encoder — avoids pulling in a crate just for clipboard support.
#[cfg(any(feature = "mcp", test))]
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

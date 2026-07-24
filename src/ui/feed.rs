use std::cell::RefCell;

use compact_str::CompactString;
use crossterm::style::Color;

use super::markdown::{markdown_to_styled, word_wrap};
use super::renderer::LineEntry;
use super::{C_AGENT, C_ERROR, C_PERM, C_TOOL};

/// Semantic role of a conversation block in the feed.
///
/// Roles are independent of terminal colors; `BlockStyle::color()` maps each
/// role to the color used by the custom renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockStyle {
    User,
    Agent,
    Reasoning,
    Tool,
    ToolResult,
    Error,
    System,
    Welcome,
    Permission,
    Plain,
}

impl BlockStyle {
    pub fn color(self) -> Color {
        match self {
            BlockStyle::User => Color::Green,
            BlockStyle::Agent => C_AGENT,
            BlockStyle::Reasoning => Color::DarkMagenta,
            BlockStyle::Tool => C_TOOL,
            BlockStyle::ToolResult => Color::DarkGrey,
            BlockStyle::Error => C_ERROR,
            BlockStyle::System => Color::DarkGrey,
            BlockStyle::Welcome => Color::Cyan,
            BlockStyle::Permission => C_PERM,
            BlockStyle::Plain => Color::White,
        }
    }
}

/// Map a legacy terminal color to the closest semantic block style.
///
/// This is used while migrating callers from `Renderer::write_line(text, color)`
/// to the feed model. New code should prefer `BlockStyle` directly.
pub fn style_from_color(color: Color) -> BlockStyle {
    match color {
        Color::Green => BlockStyle::User,
        Color::DarkMagenta => BlockStyle::Reasoning,
        Color::Yellow => BlockStyle::Tool,
        Color::DarkGrey => BlockStyle::System,
        Color::Cyan => BlockStyle::Welcome,
        Color::Red => BlockStyle::Error,
        Color::Magenta => BlockStyle::Permission,
        Color::White => BlockStyle::Plain,
        _ => BlockStyle::Plain,
    }
}

/// A single structured conversation block.
///
/// Blocks store raw text; layout (word-wrap, markdown parsing) happens when
/// `Feed::lines(width)` is called. This keeps the feed independent of terminal
/// geometry and makes layout math testable without a terminal.
#[derive(Clone, Debug)]
pub struct Block {
    pub style: BlockStyle,
    pub text: String,
    /// True while a producer is still appending to this block (e.g. streaming
    /// agent tokens). A running agent block parses markdown only for its
    /// completed lines and renders the unfinished tail line as plain text.
    running: bool,
    /// Memoized markdown layout. Interior mutability keeps `Feed::lines` a
    /// `&self` read; `Feed` mutators that rewrite block text invalidate it.
    md_cache: RefCell<Option<MdCache>>,
}

/// Memoized markdown layout of an agent block's completed text at a width.
#[derive(Clone, Debug)]
struct MdCache {
    width: usize,
    /// Byte length of the parsed prefix: up to the last completed line for
    /// running blocks, the full text once finalized.
    parsed_len: usize,
    lines: Vec<LineEntry>,
}

impl Block {
    pub fn new(style: BlockStyle, text: impl Into<String>) -> Self {
        Self {
            style,
            text: text.into(),
            running: false,
            md_cache: RefCell::new(None),
        }
    }
}

/// Conversation feed: a sequence of semantic blocks that can be laid out at
/// any width.
#[derive(Clone, Debug, Default)]
pub struct Feed {
    blocks: Vec<Block>,
    /// Bumped by every content mutation. The renderer compares generations to
    /// know whether the chat viewport needs a redraw, which also catches
    /// mutations made through `Renderer::feed_mut()`.
    generation: u64,
    /// Pre-wrapped visual rows for the last requested width. Scroll and
    /// selection queries reuse these rows instead of re-laying out the whole
    /// feed each time; invalidated by any content mutation (generation bump)
    /// or a width change.
    layout_cache: RefCell<Option<LayoutCache>>,
    /// Number of full layout passes; test-only proof that queries reuse the
    /// pre-wrapped rows.
    #[cfg(test)]
    layout_computes: std::cell::Cell<usize>,
}

/// Memoized layout of the whole feed at a viewport width and generation.
#[derive(Clone, Debug)]
struct LayoutCache {
    width: usize,
    generation: u64,
    lines: Vec<LineEntry>,
}

// Several helpers exist primarily for unit testing layout/scroll math without
// a terminal; allow them even when not yet wired into the production path.
#[allow(dead_code)]
impl Feed {
    pub fn new() -> Self {
        Self {
            blocks: Vec::new(),
            generation: 0,
            layout_cache: RefCell::new(None),
            #[cfg(test)]
            layout_computes: std::cell::Cell::new(0),
        }
    }

    /// Monotonic counter bumped on every content mutation.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn clear(&mut self) {
        self.generation += 1;
        self.blocks.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    pub fn push_block(&mut self, style: BlockStyle, text: impl Into<String>) {
        self.generation += 1;
        self.blocks.push(Block::new(style, text));
    }

    /// Push an empty block that a producer will append to incrementally
    /// (e.g. streaming agent tokens). While running, agent blocks parse
    /// markdown only for completed lines and render the unfinished tail line
    /// as plain text. Call `finalize_last` when the stream ends.
    pub fn push_streaming_block(&mut self, style: BlockStyle) {
        self.generation += 1;
        let mut block = Block::new(style, "");
        block.running = true;
        self.blocks.push(block);
    }

    /// Mark the last block as complete: its full text (including the former
    /// tail line) is parsed as markdown on the next layout. No-op when the
    /// last block is not running.
    pub fn finalize_last(&mut self) {
        if let Some(last) = self.blocks.last_mut()
            && last.running
        {
            self.generation += 1;
            last.running = false;
            // Force one full re-parse now that the text is complete.
            *last.md_cache.borrow_mut() = None;
        }
    }

    pub fn push_line(&mut self, style: BlockStyle, text: impl Into<String>) {
        self.push_block(style, text);
    }

    /// Append text to the most recent block. Returns `false` when the feed is
    /// empty and there is no block to append to.
    pub fn append_to_last(&mut self, text: impl AsRef<str>) -> bool {
        if let Some(last) = self.blocks.last_mut() {
            self.generation += 1;
            last.text.push_str(text.as_ref());
            true
        } else {
            false
        }
    }

    /// Replace the last block, or push a new one if the feed is empty.
    pub fn replace_last(&mut self, style: BlockStyle, text: impl Into<String>) {
        self.generation += 1;
        if let Some(last) = self.blocks.last_mut() {
            last.style = style;
            last.text = text.into();
            last.running = false;
            *last.md_cache.borrow_mut() = None;
        } else {
            self.blocks.push(Block::new(style, text));
        }
    }

    pub fn truncate_blocks(&mut self, len: usize) {
        self.generation += 1;
        self.blocks.truncate(len);
    }

    /// Return the fully laid-out chat lines for the given width.
    ///
    /// The result is a list of `LineEntry` values, one per visible row, that the
    /// renderer can draw directly. Markdown is parsed for agent blocks; all
    /// other blocks are word-wrapped and colored by their semantic role.
    /// Running agent blocks parse markdown only for their completed lines and
    /// render the unfinished tail line as plain text; parsed layouts are
    /// memoized per block so repeated layouts at the same width don't re-parse.
    ///
    /// The laid-out rows are pre-wrapped and memoized per `(width,
    /// generation)`, so scroll and selection queries (`line_count`,
    /// `visible_range`, `line_at_visual_row`, `selected_text`) operate on the
    /// cached visual rows instead of re-laying out the feed on every call.
    pub fn lines(&self, width: usize) -> Vec<LineEntry> {
        {
            let cache = self.layout_cache.borrow();
            if let Some(c) = cache.as_ref()
                && c.width == width
                && c.generation == self.generation
            {
                return c.lines.clone();
            }
        }
        let lines = self.compute_lines(width);
        #[cfg(test)]
        self.layout_computes.set(self.layout_computes.get() + 1);
        *self.layout_cache.borrow_mut() = Some(LayoutCache {
            width,
            generation: self.generation,
            lines: lines.clone(),
        });
        lines
    }

    /// Number of full layout passes so far (test-only).
    #[cfg(test)]
    pub(crate) fn layout_computes(&self) -> usize {
        self.layout_computes.get()
    }

    /// Lay out every block at `width`. Called by `lines` on a cache miss.
    fn compute_lines(&self, width: usize) -> Vec<LineEntry> {
        let mut result = Vec::new();
        for block in &self.blocks {
            match block.style {
                BlockStyle::Agent => {
                    let mut styled = agent_block_lines(block, width);
                    if !styled.is_empty() {
                        styled[0].text = CompactString::from(format!("< {}", styled[0].text));
                    }
                    result.extend(styled);
                }
                _ => {
                    let color = block.style.color();
                    for line in block.text.split('\n') {
                        let trimmed = line.trim_end_matches('\r');
                        if trimmed.is_empty() {
                            result.push(LineEntry {
                                text: CompactString::new(""),
                                color,
                            });
                        } else {
                            for chunk in word_wrap(trimmed, width) {
                                result.push(LineEntry { text: chunk, color });
                            }
                        }
                    }
                }
            }
        }
        result
    }

    /// Total number of visible rows for the given width.
    pub fn line_count(&self, width: usize) -> usize {
        self.lines(width).len()
    }

    /// Return the index of the first and last `LineEntry` that would be visible
    /// in a viewport of `viewport_height` rows with `scroll_offset`.
    ///
    /// `scroll_offset == 0` means "stick to the bottom" (auto-scroll).
    pub fn visible_range(
        &self,
        width: usize,
        scroll_offset: usize,
        viewport_height: usize,
    ) -> (usize, usize) {
        let total = self.line_count(width);
        let visible = viewport_height.min(total);
        let auto_scroll = scroll_offset == 0;

        let start = if auto_scroll {
            total.saturating_sub(visible)
        } else {
            total.saturating_sub((scroll_offset + visible).min(total))
        };
        let end = (start + visible).min(total);
        (start, end)
    }

    /// Map a screen row (relative to the top of the viewport) to a `LineEntry`
    /// index in `lines(width)`.
    ///
    /// Returns `None` when the row is padding above bottom-aligned content or
    /// falls past the last visible line.
    pub fn line_at_visual_row(
        &self,
        width: usize,
        scroll_offset: usize,
        viewport_height: usize,
        row: u16,
    ) -> Option<usize> {
        let total = self.line_count(width);
        if total == 0 {
            return None;
        }
        let visible = viewport_height.min(total);
        let auto_scroll = scroll_offset == 0;
        let pad = if auto_scroll && total < viewport_height {
            viewport_height - total
        } else {
            0
        };

        let row = row as usize;
        if row < pad {
            return None;
        }

        let start = if auto_scroll {
            total.saturating_sub(visible)
        } else {
            total.saturating_sub((scroll_offset + visible).min(total))
        };

        let lines = self.lines(width);
        let mut visual_row = pad;
        let mut idx = start;
        while idx < lines.len() {
            if visual_row == row {
                return Some(idx);
            }
            visual_row += 1;
            idx += 1;
        }
        None
    }

    /// Concatenate the text of all visible lines in the given range.
    pub fn selected_text(&self, width: usize, start: usize, end: usize) -> Option<String> {
        let lines = self.lines(width);
        let (lo, hi) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        let mut result = String::new();
        for i in lo..=hi {
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
}

/// Lay out an agent block: markdown for completed lines, plain text for the
/// unfinished tail line of a still-streaming block.
///
/// The markdown parse of the completed prefix is memoized in the block's
/// `MdCache`. The cache key is `(width, parsed_len)`; this is valid because
/// block text grows by appends only while running, so an unchanged prefix
/// length means an unchanged prefix. Mutators that rewrite text
/// (`replace_last`, `finalize_last`) clear the cache explicitly.
fn agent_block_lines(block: &Block, width: usize) -> Vec<LineEntry> {
    // Text parsed as markdown: the whole block once finalized, or only the
    // completed lines (up to the last newline) while streaming.
    let completed_len = if block.running {
        match block.text.rfind('\n') {
            Some(idx) => idx + 1,
            None => 0,
        }
    } else {
        block.text.len()
    };

    let mut lines = match cached_agent_lines(block, width, completed_len) {
        Some(lines) => lines,
        None => {
            let parsed = markdown_to_styled(&block.text[..completed_len], width);
            *block.md_cache.borrow_mut() = Some(MdCache {
                width,
                parsed_len: completed_len,
                lines: parsed.clone(),
            });
            parsed
        }
    };

    // The unfinished tail line of a running block is rendered as plain text:
    // its markdown markers are not parsed until the line completes. This
    // avoids re-parsing the whole response on every streamed token.
    if block.running && completed_len < block.text.len() {
        let tail = block.text[completed_len..].trim_end_matches('\r');
        if !tail.is_empty() {
            let color = BlockStyle::Agent.color();
            for chunk in word_wrap(tail, width) {
                lines.push(LineEntry { text: chunk, color });
            }
        }
    }
    lines
}

/// Return the memoized markdown layout when it matches `(width, parsed_len)`.
fn cached_agent_lines(block: &Block, width: usize, parsed_len: usize) -> Option<Vec<LineEntry>> {
    let cache = block.md_cache.borrow();
    let cache = cache.as_ref()?;
    if cache.width == width && cache.parsed_len == parsed_len {
        Some(cache.lines.clone())
    } else {
        None
    }
}

use std::cell::RefCell;
use std::collections::HashMap;

use compact_str::CompactString;
use crossterm::style::Color;

use super::markdown::{markdown_to_styled, word_wrap};
use super::renderer::LineEntry;

/// Semantic role of a conversation block in the feed.
///
/// Roles are independent of terminal colors; `BlockStyle::color()` maps each
/// role to a color, honoring `[colors.roles]` overrides from the active
/// theme or config (see `ui::roles`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
    Code,
    Plain,
}

impl BlockStyle {
    /// The color this role renders in, honoring `[colors.roles]` overrides
    /// from the active theme or config (see `ui::roles`).
    pub fn color(self) -> Color {
        super::roles::color(self)
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
    /// Optional wall-clock time when the block was created. Set by callers
    /// when the turn starts (user submit) or finalizes (assistant done).
    pub created_at: Option<chrono::DateTime<chrono::Local>>,
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
            created_at: None,
            running: false,
            md_cache: RefCell::new(None),
        }
    }

    pub fn with_created_at(
        style: BlockStyle,
        text: impl Into<String>,
        created_at: chrono::DateTime<chrono::Local>,
    ) -> Self {
        Self {
            style,
            text: text.into(),
            created_at: Some(created_at),
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
    /// know whether the live block needs a redraw, which also catches
    /// mutations made through `Renderer::feed_mut()`.
    generation: u64,
    /// Memoized layout for each `(block_index, width)`. Invalidated when the
    /// corresponding block is mutated, removed, or the whole feed is cleared.
    block_layout_cache: RefCell<HashMap<(usize, usize), Vec<LineEntry>>>,
    /// Pre-wrapped visual rows for the last requested width; invalidated by
    /// any content mutation (generation bump) or a width change. Only the
    /// test-only `lines` flat-layout helper uses it (production walks the
    /// feed block by block via `block_lines`).
    #[cfg(test)]
    layout_cache: RefCell<Option<LayoutCache>>,
    /// Number of full layout passes; test-only proof that queries reuse the
    /// pre-wrapped rows.
    #[cfg(test)]
    layout_computes: std::cell::Cell<usize>,
    /// Number of per-block layout computations. Used by tests to prove that
    /// unchanged blocks are not re-laid out.
    #[cfg(test)]
    block_layout_computes: std::cell::Cell<usize>,
}

/// Memoized layout of the whole feed at a width and generation (test-only,
/// see `Feed::lines`).
#[cfg(test)]
#[derive(Clone, Debug)]
struct LayoutCache {
    width: usize,
    generation: u64,
    lines: Vec<LineEntry>,
}

impl Feed {
    pub fn new() -> Self {
        Self {
            blocks: Vec::new(),
            generation: 0,
            block_layout_cache: RefCell::new(HashMap::new()),
            #[cfg(test)]
            layout_cache: RefCell::new(None),
            #[cfg(test)]
            layout_computes: std::cell::Cell::new(0),
            #[cfg(test)]
            block_layout_computes: std::cell::Cell::new(0),
        }
    }

    /// Monotonic counter bumped on every content mutation.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn clear(&mut self) {
        self.generation += 1;
        self.blocks.clear();
        self.block_layout_cache.borrow_mut().clear();
        #[cfg(test)]
        {
            self.layout_cache.borrow_mut().take();
        }
    }

    #[cfg(test)]
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

    /// Push a streaming block stamped with the current wall-clock time.
    #[cfg(test)]
    pub fn push_streaming_block_with_time(
        &mut self,
        style: BlockStyle,
        created_at: chrono::DateTime<chrono::Local>,
    ) {
        self.generation += 1;
        let mut block = Block::with_created_at(style, "", created_at);
        block.running = true;
        self.blocks.push(block);
    }

    /// Mark the last block as complete: its full text (including the former
    /// tail line) is parsed as markdown on the next layout. No-op when the
    /// last block is not running.
    pub fn finalize_last(&mut self) {
        let idx = self.blocks.len().saturating_sub(1);
        let finalized = if let Some(last) = self.blocks.last_mut()
            && last.running
        {
            self.generation += 1;
            last.running = false;
            // Force one full re-parse now that the text is complete.
            *last.md_cache.borrow_mut() = None;
            true
        } else {
            false
        };
        if finalized {
            self.invalidate_block_layout(idx);
        }
    }

    pub fn push_line(&mut self, style: BlockStyle, text: impl Into<String>) {
        self.push_block(style, text);
    }

    /// Push a single-line block with an explicit creation timestamp.
    pub fn push_line_with_time(
        &mut self,
        style: BlockStyle,
        text: impl Into<String>,
        created_at: chrono::DateTime<chrono::Local>,
    ) {
        self.generation += 1;
        self.blocks
            .push(Block::with_created_at(style, text, created_at));
    }

    /// Append text to the most recent block. Returns `false` when the feed is
    /// empty and there is no block to append to.
    pub fn append_to_last(&mut self, text: impl AsRef<str>) -> bool {
        let appended = if let Some(last) = self.blocks.last_mut() {
            self.generation += 1;
            last.text.push_str(text.as_ref());
            true
        } else {
            false
        };
        if appended {
            let idx = self.blocks.len() - 1;
            self.invalidate_block_layout(idx);
        }
        appended
    }

    /// Stamp the most recent block with the current local time, if it does not
    /// already have one. No-op when the feed is empty.
    pub fn set_last_block_timestamp(&mut self) {
        let idx = self.blocks.len().saturating_sub(1);
        let changed = if let Some(last) = self.blocks.last_mut()
            && last.created_at.is_none()
        {
            last.created_at = Some(chrono::Local::now());
            true
        } else {
            false
        };
        if changed {
            self.invalidate_block_layout(idx);
            #[cfg(test)]
            {
                self.layout_cache.borrow_mut().take();
            }
        }
    }

    /// Replace the last block, or push a new one if the feed is empty
    /// (test-only).
    #[cfg(test)]
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
        let idx = self.blocks.len().saturating_sub(1);
        self.invalidate_block_layout(idx);
    }

    pub fn truncate_blocks(&mut self, len: usize) {
        self.generation += 1;
        self.blocks.truncate(len);
        self.block_layout_cache
            .borrow_mut()
            .retain(|(idx, _), _| *idx < len);
    }

    /// Return the fully laid-out chat lines for the given width (test-only:
    /// production walks the feed block by block via `block_lines`).
    ///
    /// The result is a list of `LineEntry` values, one per visible row, that the
    /// renderer can draw directly. Markdown is parsed for agent blocks; all
    /// other blocks are word-wrapped and colored by their semantic role.
    /// Running agent blocks parse markdown only for their completed lines and
    /// render the unfinished tail line as plain text; parsed layouts are
    /// memoized per block so repeated layouts at the same width don't re-parse.
    ///
    /// The laid-out rows are pre-wrapped and memoized per `(width,
    /// generation)`, so repeated queries reuse the cached visual rows instead
    /// of re-laying out the feed on every call.
    #[cfg(test)]
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

    /// Lay out a single block at `width`, with per-block markdown
    /// memoization. The renderer uses this to walk the feed block by block
    /// from its print watermark.
    pub fn block_lines(&self, idx: usize, width: usize) -> Vec<LineEntry> {
        {
            let cache = self.block_layout_cache.borrow();
            if let Some(lines) = cache.get(&(idx, width)) {
                return lines.clone();
            }
        }
        let lines = match self.blocks.get(idx) {
            Some(block) => self.compute_block_layout(block, width),
            None => Vec::new(),
        };
        self.block_layout_cache
            .borrow_mut()
            .insert((idx, width), lines.clone());
        lines
    }

    /// Remove all cached layouts for `block_index`.
    fn invalidate_block_layout(&self, block_index: usize) {
        self.block_layout_cache
            .borrow_mut()
            .retain(|(idx, _), _| *idx != block_index);
    }

    /// Lay out one block and count the computation for tests.
    #[cfg(test)]
    fn compute_block_layout(&self, block: &Block, width: usize) -> Vec<LineEntry> {
        self.block_layout_computes
            .set(self.block_layout_computes.get() + 1);
        block_layout(block, width)
    }

    /// Lay out one block (production builds have no counter).
    #[cfg(not(test))]
    fn compute_block_layout(&self, block: &Block, width: usize) -> Vec<LineEntry> {
        block_layout(block, width)
    }

    /// Whether the last block is still running (a producer is appending to
    /// it). The renderer keeps a running tail in the live block instead of
    /// committing it to scrollback.
    pub fn last_block_running(&self) -> bool {
        self.blocks.last().is_some_and(|b| b.running)
    }

    /// Number of full layout passes so far (test-only).
    #[cfg(test)]
    pub(crate) fn layout_computes(&self) -> usize {
        self.layout_computes.get()
    }

    /// Number of per-block layout computations so far (test-only).
    #[cfg(test)]
    pub(crate) fn block_layout_computes(&self) -> usize {
        self.block_layout_computes.get()
    }

    /// Lay out every block at `width`. Called by `lines` on a cache miss
    /// (test-only).
    #[cfg(test)]
    fn compute_lines(&self, width: usize) -> Vec<LineEntry> {
        let mut result = Vec::new();
        for idx in 0..self.blocks.len() {
            result.extend(self.block_lines(idx, width));
        }
        result
    }

    /// Total number of visible rows for the given width (test-only).
    #[cfg(test)]
    pub fn line_count(&self, width: usize) -> usize {
        self.lines(width).len()
    }
}

/// Lay out one block at `width`: markdown for agent blocks, word-wrapped
/// role-colored text for everything else. Agent blocks get a `< ` prefix on
/// their first line.
fn block_layout(block: &Block, width: usize) -> Vec<LineEntry> {
    let mut result = match block.style {
        BlockStyle::Agent => {
            let mut styled = agent_block_lines(block, width);
            if !styled.is_empty() && styled[0].style != BlockStyle::Code {
                styled[0].text = CompactString::from(format!("< {}", styled[0].text));
            }
            styled
        }
        style => {
            let color = style.color();
            let mut result = Vec::new();
            for line in block.text.split('\n') {
                let trimmed = line.trim_end_matches('\r');
                if trimmed.is_empty() {
                    result.push(
                        LineEntry::new(CompactString::new(""), color, style)
                            .with_created_at(block.created_at),
                    );
                } else {
                    for chunk in word_wrap(trimmed, width) {
                        result.push(
                            LineEntry::new(chunk, color, style).with_created_at(block.created_at),
                        );
                    }
                }
            }
            result
        }
    };
    if let Some(first) = result.first_mut() {
        first.block_start = true;
    }
    result
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
        Some(lines) => lines
            .into_iter()
            .map(|l| l.with_created_at(block.created_at))
            .collect(),
        None => {
            let parsed: Vec<LineEntry> = markdown_to_styled(&block.text[..completed_len], width)
                .into_iter()
                .map(|l| l.with_created_at(block.created_at))
                .collect();
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
                lines.push(
                    LineEntry::new(chunk, color, BlockStyle::Agent)
                        .with_created_at(block.created_at),
                );
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

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
}

impl Block {
    pub fn new(style: BlockStyle, text: impl Into<String>) -> Self {
        Self {
            style,
            text: text.into(),
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
}

// Several helpers exist primarily for unit testing layout/scroll math without
// a terminal; allow them even when not yet wired into the production path.
#[allow(dead_code)]
impl Feed {
    pub fn new() -> Self {
        Self {
            blocks: Vec::new(),
            generation: 0,
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
    pub fn lines(&self, width: usize) -> Vec<LineEntry> {
        let mut result = Vec::new();
        for block in &self.blocks {
            match block.style {
                BlockStyle::Agent => {
                    let mut styled = markdown_to_styled(&block.text, width);
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

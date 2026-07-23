pub mod chat_history;
pub mod export;
pub mod storage;

use std::path::Path;

use compact_str::CompactString;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const TOOL_RESULT_SAVE_THRESHOLD: usize = 12_000;
pub const TOOL_RESULT_HEAD_CHARS: usize = 2_000;
pub const TOOL_RESULT_TAIL_CHARS: usize = 8_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    User,
    Assistant,
    System,
    ToolCall,
    ToolResult,
    SubagentToolCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessage {
    pub role: MessageRole,
    pub content: CompactString,
    pub estimated_tokens: u64,
}

/// A single-step restore point captured before a conversation rewind, so the
/// destructive truncation can be undone with `/unrewind`. Holds the full
/// message list and the calibration/estimate fields that `truncate_to` mutates,
/// which is everything needed to put the session back exactly as it was. Not
/// persisted: a rewind is only undoable within the session that made it.
#[derive(Debug, Clone)]
pub struct RewindUndo {
    messages: Vec<SessionMessage>,
    total_estimated_tokens: u64,
    calibrated_tokens: u64,
    calibrated_msg_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Compaction {
    pub summary: CompactString,
    pub first_kept_index: usize,
    pub summarized_count: usize,
    pub token_savings: u64,
    pub created_at: CompactString,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionAllowEntry {
    pub tool: CompactString,
    pub pattern: CompactString,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: CompactString,
    pub name: CompactString,
    pub messages: Vec<SessionMessage>,
    pub compactions: Vec<Compaction>,
    pub created_at: CompactString,
    pub updated_at: CompactString,
    #[serde(default)]
    pub total_input_tokens: u64,
    #[serde(default)]
    pub total_output_tokens: u64,
    #[serde(default)]
    pub total_cached_input_tokens: u64,
    #[serde(default)]
    pub total_cache_creation_input_tokens: u64,
    pub total_cost: f64,
    pub total_estimated_tokens: u64,
    #[serde(default)]
    pub calibrated_tokens: u64,
    #[serde(default)]
    pub calibrated_msg_count: usize,
    #[serde(default)]
    pub input_token_cost: f64,
    #[serde(default)]
    pub output_token_cost: f64,
    pub context_window: u64,
    pub model: CompactString,
    pub provider: CompactString,
    pub working_dir: CompactString,
    #[serde(default)]
    pub permission_allowlist: Vec<PermissionAllowEntry>,
    #[cfg(feature = "multimodal")]
    #[serde(skip)]
    pub pending_media: Vec<crate::extras::multimodal::MediaAttachment>,
    /// Display preference (set from config at startup, not persisted): show the
    /// session cost in the status bar even when it is $0.0000.
    #[serde(skip)]
    pub show_cost_always: bool,
    /// Current git branch of `working_dir`, for the status bar. Refreshed at
    /// runtime, not persisted.
    #[serde(skip)]
    pub git_branch: Option<CompactString>,
    /// Working-tree change counts and upstream sync, for the status bar.
    /// Computed only when the statusline uses a git change/status item. Not persisted.
    #[serde(skip)]
    pub git_status: Option<GitStatus>,
    /// Whether reasoning is currently enabled, for the status bar. Synced from
    /// the event loop, not persisted.
    #[serde(skip)]
    pub reasoning_enabled: bool,
    /// Estimated tokens for the fixed request overhead that never lives in
    /// `messages` — system prompt, tool-use preamble, context files, memory.
    /// Used only before the first real calibration (see
    /// [`effective_context_tokens`](Self::effective_context_tokens)); once the
    /// provider reports real usage, the calibration anchor already includes this
    /// overhead, so it must not be added again. Recomputed at runtime, not
    /// persisted.
    #[serde(skip)]
    pub overhead_tokens: u64,
    /// Restore point for the most recent `/unrewind`-able rewind, captured by
    /// [`rewind_to`](Self::rewind_to). Runtime only, not persisted.
    #[serde(skip)]
    pub rewind_undo: Option<RewindUndo>,
}

/// Working-tree summary parsed from `git status --porcelain=v2 --branch`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitStatus {
    pub staged: u32,
    pub modified: u32,
    pub deleted: u32,
    pub untracked: u32,
    pub ahead: u32,
    pub behind: u32,
}

impl GitStatus {
    pub fn is_dirty(&self) -> bool {
        self.staged + self.modified + self.deleted + self.untracked > 0
    }
}

impl Session {
    pub fn estimate_tokens(text: &str) -> u64 {
        let mut wide: u64 = 0;
        let mut narrow: u64 = 0;
        for ch in text.chars() {
            if Self::is_wide_char(ch) {
                wide += 1;
            } else {
                narrow += 1;
            }
        }
        // wide * 0.9 + narrow / 4, min 1
        ((wide * 9 / 10) + narrow / 4).max(1)
    }

    fn is_wide_char(ch: char) -> bool {
        matches!(ch as u32,
            0x1100..=0x11FF |   // Hangul Jamo
            0x2E80..=0x9FFF |   // CJK radicals/Kangxi/punctuation/kana/Unified+ExtA
            0xA000..=0xA4CF |   // Yi
            0xAC00..=0xD7A3 |   // Hangul Syllables
            0xF900..=0xFAFF |   // CJK Compatibility Ideographs
            0xFF00..=0xFFEF |   // Halfwidth/Fullwidth Forms
            0x20000..=0x3FFFF   // Supplementary Ideographic Plane (Ext B–F)
        )
    }

    pub fn new(provider: &str, model: &str, context_window: u64, name: &str) -> Self {
        let now = CompactString::new(chrono::Utc::now().to_rfc3339());
        Session {
            id: CompactString::new(Uuid::new_v4().to_string()),
            name: CompactString::new(name),
            messages: Vec::new(),
            compactions: Vec::new(),
            created_at: now.clone(),
            updated_at: now,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cached_input_tokens: 0,
            total_cache_creation_input_tokens: 0,
            total_cost: 0.0,
            total_estimated_tokens: 0,
            calibrated_tokens: 0,
            calibrated_msg_count: 0,
            input_token_cost: 0.0,
            output_token_cost: 0.0,
            context_window,
            model: CompactString::new(model),
            provider: CompactString::new(provider),
            working_dir: std::env::current_dir()
                .map(|p| CompactString::new(p.to_string_lossy()))
                .unwrap_or_default(),
            permission_allowlist: Vec::new(),
            #[cfg(feature = "multimodal")]
            pending_media: Vec::new(),
            show_cost_always: false,
            git_branch: None,
            git_status: None,
            reasoning_enabled: false,
            overhead_tokens: 0,
            rewind_undo: None,
        }
    }

    /// Read the current git branch for `dir`, or `None` outside a repo / on a
    /// detached HEAD (then a short commit hash is returned instead). Reads
    /// `.git/HEAD` directly (cheap) rather than spawning git, and follows the
    /// `.git` file pointer used by worktrees and submodules.
    pub fn detect_git_branch(dir: &str) -> Option<CompactString> {
        use std::path::{Path, PathBuf};
        let dir_path = Path::new(dir);
        let dot_git = dir_path.join(".git");
        let gitdir = if dot_git.is_dir() {
            dot_git
        } else if dot_git.is_file() {
            let content = std::fs::read_to_string(&dot_git).ok()?;
            let rel = content.strip_prefix("gitdir:")?.trim();
            let p = PathBuf::from(rel);
            if p.is_absolute() { p } else { dir_path.join(p) }
        } else {
            return None;
        };
        let head = std::fs::read_to_string(gitdir.join("HEAD")).ok()?;
        let head = head.trim();
        if let Some(rest) = head.strip_prefix("ref:") {
            let r = rest.trim();
            Some(CompactString::new(
                r.strip_prefix("refs/heads/").unwrap_or(r),
            ))
        } else if !head.is_empty() {
            // Detached HEAD: show a short commit hash (char-boundary-safe).
            let mut end = head.len().min(8);
            while end > 0 && !head.is_char_boundary(end) {
                end -= 1;
            }
            Some(CompactString::new(&head[..end]))
        } else {
            None
        }
    }

    /// Refresh [`git_branch`](Self::git_branch) from the current `working_dir`.
    pub fn refresh_git_branch(&mut self) {
        self.git_branch = Self::detect_git_branch(&self.working_dir);
    }

    /// Refresh [`git_status`](Self::git_status) by running `git status` in
    /// `working_dir`. Only call this when the statusline actually shows a git
    /// change/status item: it spawns a subprocess (throttled by the caller).
    pub fn refresh_git_status(&mut self) {
        self.git_status = Self::detect_git_status(&self.working_dir);
    }

    fn detect_git_status(dir: &str) -> Option<GitStatus> {
        let out = std::process::Command::new("git")
            .args(["status", "--porcelain=v2", "--branch"])
            .current_dir(dir)
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        Some(Self::parse_porcelain(&String::from_utf8_lossy(&out.stdout)))
    }

    /// Parse `git status --porcelain=v2 --branch` output into a [`GitStatus`].
    pub fn parse_porcelain(text: &str) -> GitStatus {
        let mut s = GitStatus::default();
        for line in text.lines() {
            if let Some(ab) = line.strip_prefix("# branch.ab ") {
                // Format: "+<ahead> -<behind>"
                for tok in ab.split_whitespace() {
                    if let Some(n) = tok.strip_prefix('+') {
                        s.ahead = n.parse().unwrap_or(0);
                    } else if let Some(n) = tok.strip_prefix('-') {
                        s.behind = n.parse().unwrap_or(0);
                    }
                }
            } else if let Some(rest) = line.strip_prefix("1 ").or_else(|| line.strip_prefix("2 ")) {
                // Changed/renamed entry. The XY field is the first token: index
                // status (staged) then worktree status.
                if let Some(xy) = rest.split_whitespace().next() {
                    let mut chars = xy.chars();
                    let x = chars.next().unwrap_or('.');
                    let y = chars.next().unwrap_or('.');
                    if x != '.' {
                        s.staged += 1;
                    }
                    match y {
                        'M' => s.modified += 1,
                        'D' => s.deleted += 1,
                        _ => {}
                    }
                }
            } else if line.starts_with("u ") {
                // Unmerged paths count as a working-tree modification.
                s.modified += 1;
            } else if line.starts_with("? ") {
                s.untracked += 1;
            }
        }
        s
    }

    pub fn add_message(&mut self, role: MessageRole, content: &str) {
        let tokens = Self::estimate_tokens(content);
        self.messages.push(SessionMessage {
            role,
            content: CompactString::new(content),
            estimated_tokens: tokens,
        });
        self.total_estimated_tokens = self.total_estimated_tokens.saturating_add(tokens);
        self.updated_at = CompactString::new(chrono::Utc::now().to_rfc3339());
        // The conversation has moved forward, so the last rewind's restore point
        // no longer lines up — drop it so /redo can't splice in a stale tail.
        self.rewind_undo = None;
    }

    pub fn add_tool_call(&mut self, name: &str, args: &serde_json::Value) {
        self.add_message(
            MessageRole::ToolCall,
            &crate::ui::utils::format_tool_call_summary(name, args),
        );
    }

    pub fn add_tool_result(&mut self, name: &str, output: &str) -> String {
        let content = self.tool_result_content(name, output);
        self.add_message(MessageRole::ToolResult, &content);
        content
    }

    fn tool_result_content(&self, name: &str, output: &str) -> String {
        let output_chars = output.chars().count();
        if output_chars <= TOOL_RESULT_SAVE_THRESHOLD {
            return format!("{name}:\n{output}");
        }

        match storage::save_tool_output(&self.id, name, output) {
            Ok(path) => format_truncated_tool_result(name, output, output_chars, &path),
            Err(err) => format!(
                "{name}:\n{output}\n\n[failed to save long tool output separately; kept full output in session to avoid data loss: {err}]"
            ),
        }
    }

    #[cfg(any(feature = "subagents", feature = "acp"))]
    pub fn add_subagent_tool_call(&mut self, name: &str, args: &serde_json::Value) {
        self.add_message(
            MessageRole::SubagentToolCall,
            &crate::ui::utils::format_tool_call_summary(name, args),
        );
    }

    #[cfg(feature = "multimodal")]
    pub fn drain_media(&mut self) -> Vec<crate::extras::multimodal::MediaAttachment> {
        std::mem::take(&mut self.pending_media)
    }

    /// The true prompt size occupying the context window, normalizing across
    /// providers' differing cache-usage reporting.
    ///
    /// The Anthropic-native route reports `input_tokens` counting *only* the
    /// uncached portion of the prompt; the cache-read and cache-creation tokens
    /// are reported in separate fields even though they still occupy the context
    /// window. So there the real prompt size is the sum of all three. The
    /// OpenAI, Gemini and OpenRouter shapes instead fold the cached subset into
    /// `input_tokens` and report no cache-creation, so `input_tokens` is already
    /// the full prompt size and adding the cache fields would double-count.
    ///
    /// `anthropic_native` must be the *resolved protocol route*, not a literal
    /// provider-name comparison — a custom gateway with `provider_type =
    /// "anthropic"` uses the native route under a different name, while
    /// OpenRouter serving a Claude model does not. Compute it with
    /// [`Config::is_anthropic_native`](crate::config::Config::is_anthropic_native).
    pub fn real_input_tokens(
        anthropic_native: bool,
        input_tokens: u64,
        cached_input_tokens: u64,
        cache_creation_input_tokens: u64,
    ) -> u64 {
        if anthropic_native {
            input_tokens
                .saturating_add(cached_input_tokens)
                .saturating_add(cache_creation_input_tokens)
        } else {
            input_tokens
        }
    }

    pub fn set_calibration(&mut self, input_tokens: u64, output_tokens: u64) {
        if input_tokens == 0 {
            return;
        }
        self.calibrated_tokens = input_tokens.saturating_add(output_tokens);
        self.calibrated_msg_count = self.messages.len();
    }

    pub fn reset_calibration(&mut self) {
        self.calibrated_tokens = 0;
        self.calibrated_msg_count = 0;
    }

    /// Truncate the conversation to `new_len` messages while keeping the context
    /// figure accurate (used by `/undo` and the failed-send rollback).
    ///
    /// If any removed message was part of the calibration anchor, subtract its
    /// estimated tokens from the anchor rather than discarding the whole
    /// calibration. Resetting to a cold estimate would undercount (the estimate
    /// omits tool schemas), and leaving the anchor untouched would overcount by
    /// the removed turn — subtracting keeps the figure ≈ the real remaining
    /// size. Messages beyond the anchor were never in it, so removing them only
    /// shrinks the estimated tail.
    pub fn truncate_to(&mut self, new_len: usize) {
        if new_len >= self.messages.len() {
            return;
        }
        let cal = self.calibrated_msg_count.min(self.messages.len());
        if self.calibrated_tokens > 0 && new_len < cal {
            let removed: u64 = self.messages[new_len..cal]
                .iter()
                .map(|m| m.estimated_tokens)
                .sum();
            self.calibrated_tokens = self.calibrated_tokens.saturating_sub(removed);
            self.calibrated_msg_count = new_len;
        }
        self.messages.truncate(new_len);
        self.total_estimated_tokens = self.messages.iter().map(|m| m.estimated_tokens).sum();
    }

    /// Rewind the conversation to `new_len` messages, capturing a single-step
    /// restore point first so the cut can be undone with [`redo`](Self::redo).
    ///
    /// This is the shared primitive behind both `/undo` (rewind by one turn) and
    /// the double-Esc rewind picker (rewind to a chosen earlier point): the only
    /// difference between them is which `new_len` they pass. Returns the number
    /// of messages removed (0 if `new_len` is already at or past the end, in
    /// which case no restore point is recorded).
    pub fn rewind_to(&mut self, new_len: usize) -> usize {
        if new_len >= self.messages.len() {
            return 0;
        }
        let removed = self.messages.len() - new_len;
        self.rewind_undo = Some(RewindUndo {
            messages: self.messages.clone(),
            total_estimated_tokens: self.total_estimated_tokens,
            calibrated_tokens: self.calibrated_tokens,
            calibrated_msg_count: self.calibrated_msg_count,
        });
        self.truncate_to(new_len);
        removed
    }

    /// Restore the messages removed by the most recent [`rewind_to`](Self::rewind_to)
    /// (i.e. the last `/undo` or rewind). Returns false when there is nothing to
    /// restore. The restore point is consumed, and is also invalidated as soon
    /// as the conversation moves forward again (see [`add_message`](Self::add_message)),
    /// so `/redo` only ever reaches back to the cut it directly reverses.
    pub fn redo(&mut self) -> bool {
        match self.rewind_undo.take() {
            Some(u) => {
                self.messages = u.messages;
                self.total_estimated_tokens = u.total_estimated_tokens;
                self.calibrated_tokens = u.calibrated_tokens;
                self.calibrated_msg_count = u.calibrated_msg_count;
                true
            }
            None => false,
        }
    }

    /// True while the context figure is still an estimate — no provider usage
    /// has been reported yet (or it was reset by `/clear`). The status bar marks
    /// the estimated value so the snap to the real number on the first response
    /// reads as a refinement rather than a surprise.
    pub fn ctx_is_estimated(&self) -> bool {
        self.calibrated_tokens == 0
    }

    pub fn effective_context_tokens(&self) -> u64 {
        if self.calibrated_tokens == 0 {
            // No real usage yet: per-message estimates cover only `messages`, so
            // add the fixed overhead (system prompt, tools, context files) that
            // every request also carries. After calibration this overhead is
            // already inside the anchor, so it is not added in that branch.
            return self
                .overhead_tokens
                .saturating_add(self.total_estimated_tokens);
        }
        let start = self.calibrated_msg_count.min(self.messages.len());
        let delta: u64 = self.messages[start..]
            .iter()
            .map(|m| m.estimated_tokens)
            .sum();
        self.calibrated_tokens.saturating_add(delta)
    }

    /// Pick the compaction boundary: `messages[..cut]` get summarized and
    /// `messages[cut..]` are kept as recent context. Walks backward summing
    /// per-message `estimated_tokens` until `keep_recent` is covered.
    ///
    /// This deliberately stays in the per-message estimate scale rather than
    /// the calibrated total: it is a *relative* comparison among messages (how
    /// far back does `keep_recent` reach), so any uniform estimator bias
    /// cancels out. Calibration only matters for the absolute total in
    /// `effective_context_tokens`.
    ///
    /// Returns 0 ("nothing old enough to summarize") when every message fits
    /// within `keep_recent`. The initial value is 0, not `messages.len()`, so
    /// an oversized `keep_recent` keeps the recent messages instead of
    /// summarizing the entire history, a case made reachable now that the
    /// compaction gate measures context against real usage (system prompt and
    /// context files can push the real total over budget while the messages
    /// themselves stay small).
    pub fn select_compaction_cut(messages: &[SessionMessage], keep_recent: u64) -> usize {
        let mut accumulated = 0u64;
        let mut cut_idx = 0;
        for (i, msg) in messages.iter().enumerate().rev() {
            if accumulated >= keep_recent {
                cut_idx = i + 1;
                break;
            }
            accumulated = accumulated.saturating_add(msg.estimated_tokens);
        }
        cut_idx
    }

    pub fn needs_compaction(&self, reserve_tokens: u64) -> bool {
        if self.context_window == 0 {
            return false;
        }
        self.effective_context_tokens() > self.context_window.saturating_sub(reserve_tokens)
    }

    pub fn update_context_window(&mut self, cw: u64) {
        self.context_window = cw;
    }

    pub fn compacted_context(&self) -> (Option<&str>, usize) {
        let c = match self.compactions.last() {
            Some(c) => c,
            None => return (None, 0),
        };
        // Locate the summary System message at runtime rather than trusting
        // a stored index, which drifts if messages are inserted before it.
        for (i, msg) in self.messages.iter().enumerate() {
            if msg.role == MessageRole::System && msg.content.as_str() == c.summary.as_str() {
                return (Some(c.summary.as_str()), i + 1);
            }
        }
        (None, 0)
    }

    pub fn compress(&mut self, summary: String, first_kept_index: usize, token_savings: u64) {
        let summarized_count = first_kept_index;
        let summary_tokens = Self::estimate_tokens(&summary);

        // Insert a System message with the summary at the boundary
        let summary_msg = SessionMessage {
            role: MessageRole::System,
            content: CompactString::from(summary.clone()),
            estimated_tokens: summary_tokens,
        };

        // Remove summarized messages and insert summary
        self.messages.drain(..first_kept_index);
        self.messages.insert(0, summary_msg);

        // Recompute total from remaining messages so the count is always
        // consistent — no underflow risk when token_savings is stale.
        self.total_estimated_tokens = self.messages.iter().map(|m| m.estimated_tokens).sum();

        self.compactions.push(Compaction {
            summary: CompactString::from(summary),
            first_kept_index: 1, // The summary is at index 0
            summarized_count,
            token_savings,
            created_at: CompactString::new(chrono::Utc::now().to_rfc3339()),
        });

        // Compaction reindexes messages, so the calibration anchor no longer
        // lines up. Drop it; the next completed turn re-anchors.
        self.reset_calibration();
        self.updated_at = CompactString::new(chrono::Utc::now().to_rfc3339());
    }
}

fn format_truncated_tool_result(
    name: &str,
    output: &str,
    output_chars: usize,
    path: &Path,
) -> String {
    let head: String = output.chars().take(TOOL_RESULT_HEAD_CHARS).collect();
    let tail_start = output_chars.saturating_sub(TOOL_RESULT_TAIL_CHARS);
    let tail: String = output.chars().skip(tail_start).collect();
    let omitted = output_chars.saturating_sub(TOOL_RESULT_HEAD_CHARS + TOOL_RESULT_TAIL_CHARS);

    format!(
        "{name}:\n{head}\n\n[tool output truncated: {output_chars} characters; {omitted} omitted]\n[full output saved to: {}; use the read tool on this path to inspect the complete output]\n\n{tail}",
        path.display()
    )
}

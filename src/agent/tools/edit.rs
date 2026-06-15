use rig::completion::ToolDefinition;
use rig::tool::Tool;

use crate::agent::tools::crc::crc32_hex;
use crate::agent::tools::{
    AskSender, EditArgs, EditBlock, EditOp, PermCheck, ToolError, check_perm_path, edit_system,
    levenshtein_similarity, normalize_whitespace,
};
use crate::config::types::EditSystem;

pub struct EditTool {
    pub permission: Option<PermCheck>,
    pub ask_tx: Option<AskSender>,
}

impl EditTool {
    pub fn new(permission: Option<PermCheck>, ask_tx: Option<AskSender>) -> Self {
        EditTool { permission, ask_tx }
    }
}

// ── V1: Similarity (SEARCH/REPLACE) ──────────────────────────────────────

fn parse_blocks(raw: &str) -> Result<Vec<EditBlock>, ToolError> {
    let mut blocks = Vec::new();
    let mut in_block = false;
    let mut search_lines: Vec<String> = Vec::new();
    let mut replace_lines: Vec<String> = Vec::new();
    let mut phase: u8 = 0;

    for line in raw.lines() {
        match line.trim() {
            "<<<<<<< SEARCH" => {
                if in_block {
                    return Err(ToolError::Msg(
                        "Nested SEARCH/REPLACE block detected. Close each block with >>>>>>> REPLACE before starting a new one.".to_string(),
                    ));
                }
                in_block = true;
                search_lines.clear();
                replace_lines.clear();
                phase = 1;
            }
            "=======" if phase == 1 => {
                phase = 2;
            }
            ">>>>>>> REPLACE" if phase == 2 => {
                let search = search_lines.join("\n");
                if search.is_empty() {
                    return Err(ToolError::Msg(format!(
                        "Block {} has empty search text. Each block must have a non-empty SEARCH section.",
                        blocks.len() + 1
                    )));
                }
                blocks.push(EditBlock {
                    search,
                    replace: replace_lines.join("\n"),
                });
                in_block = false;
                phase = 0;
            }
            _ if phase == 1 => {
                search_lines.push(line.to_string());
            }
            _ if phase == 2 => {
                replace_lines.push(line.to_string());
            }
            _ => {}
        }
    }

    if in_block {
        return Err(ToolError::Msg(
            "Unclosed SEARCH/REPLACE block. Each block must end with >>>>>>> REPLACE.".to_string(),
        ));
    }

    if blocks.is_empty() {
        return Err(ToolError::Msg(
            "No SEARCH/REPLACE blocks found. Use format:\n<<<<<<< SEARCH\nexisting code to find\n=======\nreplacement code\n>>>>>>> REPLACE\n\nMultiple blocks can be included for editing different parts of the same file."
                .to_string(),
        ));
    }

    Ok(blocks)
}

enum MatchResult {
    Exact(usize),
    Normalized(usize, usize),
    FuzzyApply(usize, usize, f64),
    FuzzySuggest(usize, f64, String),
    NotFound,
}

fn compute_byte_range(content: &str, norm_pos: usize, norm_len: usize) -> (usize, usize) {
    let content_norm = normalize_whitespace(content);
    let norm_end = (norm_pos + norm_len).min(content_norm.len());

    let mut orig_pos = 0usize;
    let mut norm_rem = 0usize;
    let mut byte_start = None;

    while orig_pos < content.len() && norm_rem < norm_end {
        if norm_rem >= content_norm.len() {
            break;
        }

        let b = content.as_bytes()[orig_pos];

        // Tab -> space expansion: 1 tab byte maps to 4 spaces in normalized
        if b == b'\t' {
            let tab_spaces = 4usize;
            if norm_rem < norm_pos {
                orig_pos += 1;
                norm_rem = (norm_rem + tab_spaces).min(content_norm.len());
                continue;
            }
            if byte_start.is_none() {
                byte_start = Some(orig_pos);
            }
            orig_pos += 1;
            norm_rem = (norm_rem + tab_spaces).min(content_norm.len());
            continue;
        }

        if norm_rem < norm_pos {
            orig_pos += 1;
            norm_rem += 1;
            continue;
        }

        if byte_start.is_none() {
            byte_start = Some(orig_pos);
        }

        // CRLF edge case: original \r\n maps to a single \n in normalized
        if b == b'\r' && norm_rem < content_norm.len() && content_norm.as_bytes()[norm_rem] == b'\n'
        {
            orig_pos += 1;
            continue;
        }

        orig_pos += 1;
        norm_rem += 1;
    }

    // Fallback: no match region found — return zero-length range at end of file
    let start = byte_start.unwrap_or(content.len());
    (start, orig_pos)
}

fn find_best_match(content: &str, search: &str) -> MatchResult {
    // Step 1: exact match in original content
    if let Some(pos) = content.find(search) {
        return MatchResult::Exact(pos);
    }

    // Step 2: normalized match in full text
    let content_norm = normalize_whitespace(content);
    let search_norm = normalize_whitespace(search);
    if let Some(norm_pos) = content_norm.find(&search_norm) {
        let (byte_start, byte_end) = compute_byte_range(content, norm_pos, search_norm.len());
        return MatchResult::Normalized(byte_start, byte_end);
    }

    // Step 3: fuzzy line-level matching
    let search_lines: Vec<&str> = search.lines().collect();
    let content_lines: Vec<&str> = content.lines().collect();

    if search_lines.is_empty() || content_lines.len() < search_lines.len() {
        return MatchResult::NotFound;
    }

    let search_norm_lines: Vec<String> = search_lines
        .iter()
        .map(|l| normalize_whitespace(l))
        .collect();
    let search_norm_joined = search_norm_lines.join("\n");

    let mut best_sim = 0.0f64;
    let mut best_start = 0usize;

    for start in 0..=content_lines.len() - search_lines.len() {
        let window_norm: String = content_lines[start..start + search_lines.len()]
            .iter()
            .map(|l| normalize_whitespace(l))
            .collect::<Vec<_>>()
            .join("\n");
        let sim = levenshtein_similarity(&search_norm_joined, &window_norm);
        if sim > best_sim {
            best_sim = sim;
            best_start = start;
        }
        if sim >= 0.999 {
            break;
        }
    }

    if best_sim >= 0.85 {
        let byte_start: usize = content_lines[..best_start]
            .iter()
            .map(|l| l.len() + 1)
            .sum();
        let byte_end = byte_start
            + content_lines[best_start..best_start + search_lines.len()]
                .iter()
                .map(|l| l.len() + 1)
                .sum::<usize>()
                .saturating_sub(1);
        MatchResult::FuzzyApply(byte_start, byte_end, best_sim)
    } else if best_sim >= 0.60 {
        let preview: String = search_lines
            .iter()
            .take(3)
            .copied()
            .collect::<Vec<_>>()
            .join("\n");
        MatchResult::FuzzySuggest(best_start + 1, best_sim, preview)
    } else {
        MatchResult::NotFound
    }
}

fn count_exact_matches(content: &str, search: &str) -> usize {
    content.match_indices(search).count()
}

async fn handle_similarity(
    path: &str,
    block: &str,
    content: &str,
) -> Result<(Vec<String>, Vec<(usize, usize, String)>), ToolError> {
    let blocks = parse_blocks(block)?;

    struct ResolvedSim {
        byte_start: usize,
        byte_end: usize,
        replace: String,
        note: String,
    }

    let mut resolved: Vec<ResolvedSim> = Vec::new();

    for (i, blk) in blocks.iter().enumerate() {
        let label = if blocks.len() > 1 {
            format!("Block {}: ", i + 1)
        } else {
            String::new()
        };

        match find_best_match(content, &blk.search) {
            MatchResult::Exact(pos) => {
                let count = count_exact_matches(content, &blk.search);
                if count > 1 {
                    let line_starts: Vec<usize> = std::iter::once(0)
                        .chain(content.match_indices('\n').map(|(i, _)| i + 1))
                        .collect();

                    let mut match_info = Vec::new();
                    for byte_idx in content.match_indices(&blk.search).map(|(i, _)| i) {
                        let line_num = match line_starts.binary_search(&byte_idx) {
                            Ok(i) => i + 1,
                            Err(i) => i,
                        };
                        let ls = line_starts.get(line_num - 1).copied().unwrap_or(0);
                        let le = content[ls..]
                            .find('\n')
                            .map(|e| ls + e)
                            .unwrap_or(content.len());
                        let text: String = content[ls..le].chars().take(100).collect();
                        match_info.push(format!("  Line {}: {}", line_num, text));
                    }

                    return Err(ToolError::Msg(format!(
                        "{label}search text matched {} times in {}:\n{}\n\nAdd more surrounding context to the SEARCH block to make it unique.",
                        count,
                        path,
                        match_info.join("\n"),
                    )));
                }
                resolved.push(ResolvedSim {
                    byte_start: pos,
                    byte_end: pos + blk.search.len(),
                    replace: blk.replace.clone(),
                    note: String::new(),
                });
            }
            MatchResult::Normalized(start, end) => {
                resolved.push(ResolvedSim {
                    byte_start: start,
                    byte_end: end,
                    replace: blk.replace.clone(),
                    note: "matched after whitespace normalization".to_string(),
                });
            }
            MatchResult::FuzzyApply(start, end, sim) => {
                resolved.push(ResolvedSim {
                    byte_start: start,
                    byte_end: end,
                    replace: blk.replace.clone(),
                    note: format!("fuzzy match, {:.0}% similarity", sim * 100.0),
                });
            }
            MatchResult::FuzzySuggest(line, sim, preview) => {
                return Err(ToolError::Msg(format!(
                    "{label}search text not found in '{}'. Closest match at line {}, {:.0}% similar:\n  {}\n\nRead the file around that area, copy the exact text, and retry the edit.",
                    path,
                    line,
                    sim * 100.0,
                    preview,
                )));
            }
            MatchResult::NotFound => {
                return Err(ToolError::Msg(format!(
                    "{label}search text not found in '{}'.\nRead the file and copy the exact text for the SEARCH block, ensuring whitespace and indentation match.",
                    path,
                )));
            }
        }
    }

    let mut notes = Vec::new();
    let mut ranges = Vec::new();

    for rb in &resolved {
        if !rb.note.is_empty() {
            notes.push(rb.note.clone());
        }
        ranges.push((rb.byte_start, rb.byte_end, rb.replace.clone()));
    }

    Ok((notes, ranges))
}

// ── V2: Hashedit (tag-based) ────────────────────────────────────────────

fn parse_tagged_line(raw: &str) -> Option<(usize, String)> {
    let stripped = raw.trim_start_matches([' ', '\t']);
    let (num_tag, _content) = stripped.split_once(' ')?;
    let (num_str, tag) = num_tag.split_once('|')?;
    let line_num: usize = num_str.parse().ok()?;
    if tag.len() != 8 || !tag.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some((line_num, tag.to_string()))
}

fn extract_line_info(lines_raw: &str) -> Result<Vec<(usize, String)>, ToolError> {
    let mut result = Vec::new();
    for line in lines_raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let (line_num, tag) = parse_tagged_line(line).ok_or_else(|| {
            ToolError::Msg(format!(
                "Invalid tagged line format. Expected 'N|TAG content', got: '{}'",
                trimmed
            ))
        })?;
        result.push((line_num, tag));
    }
    if result.is_empty() {
        return Err(ToolError::Msg(
            "No valid tagged lines found. Copy lines from the read output exactly.".to_string(),
        ));
    }
    Ok(result)
}

fn validate_tag(content_lines: &[&str], line_num: usize, tag: &str) -> Result<(), ToolError> {
    let idx = line_num.saturating_sub(1);
    let actual = content_lines.get(idx).ok_or_else(|| {
        ToolError::Msg(format!(
            "Line {} is out of range (file has {} lines)",
            line_num,
            content_lines.len()
        ))
    })?;
    let expected = crc32_hex(actual.as_bytes());
    if expected != tag {
        return Err(ToolError::Msg(format!(
            "Tag mismatch at line {}: expected {} but line content has tag {}. The file may have changed. Re-read and retry.",
            line_num, tag, expected
        )));
    }
    Ok(())
}

fn line_range_to_byte_range(
    content_lines: &[&str],
    start_line: usize,
    end_line: usize,
) -> (usize, usize) {
    if content_lines.is_empty() || start_line == 0 || start_line > content_lines.len() {
        return (0, 0);
    }
    let end_line = end_line.min(content_lines.len());

    // Byte position before start_line
    let byte_start: usize = content_lines[..start_line.saturating_sub(1)]
        .iter()
        .map(|l| l.len() + 1)
        .sum();

    // The +1 per line accounts for newline separators; saturating_sub(1)
    // removes the phantom newline from the last line in the range.
    let byte_end = byte_start
        + content_lines[start_line.saturating_sub(1)..end_line]
            .iter()
            .map(|l| l.len() + 1)
            .sum::<usize>()
            .saturating_sub(1);

    (byte_start, byte_end)
}

async fn handle_hashedit(
    path: &str,
    file_crc: &str,
    edits: &[EditOp],
    content: &str,
) -> Result<(Vec<String>, Vec<(usize, usize, String)>), ToolError> {
    // Validate file-level CRC
    let actual_crc = crc32_hex(content.as_bytes());
    if actual_crc != file_crc {
        return Err(ToolError::Msg(format!(
            "File CRC mismatch for '{}': expected {} but file now has {}. The file has changed since the read. Re-read and retry.",
            path, file_crc, actual_crc
        )));
    }

    let content_lines: Vec<&str> = content.lines().collect();
    let notes = Vec::new();
    let mut ranges = Vec::new();

    for (i, op) in edits.iter().enumerate() {
        let label = if edits.len() > 1 {
            format!("Edit {}: ", i + 1)
        } else {
            String::new()
        };

        match (&op.line, &op.lines) {
            (Some(single_line), None) => {
                let (line_num, tag) = parse_tagged_line(single_line).ok_or_else(|| {
                    ToolError::Msg(format!(
                        "{}invalid tagged line format. Expected 'N|TAG content', got: '{}'",
                        label, single_line
                    ))
                })?;
                validate_tag(&content_lines, line_num, &tag)
                    .map_err(|e| ToolError::Msg(format!("{}{}", label, e)))?;

                let (byte_start, byte_end) =
                    line_range_to_byte_range(&content_lines, line_num, line_num);
                ranges.push((byte_start, byte_end, op.text.clone()));
            }
            (None, Some(multi_lines)) => {
                let entries = extract_line_info(multi_lines)?;
                for &(line_num, ref tag) in &entries {
                    validate_tag(&content_lines, line_num, tag)
                        .map_err(|e| ToolError::Msg(format!("{}{}", label, e)))?;
                }
                let start_line = entries[0].0;
                let end_line = entries[entries.len() - 1].0;
                let (byte_start, byte_end) =
                    line_range_to_byte_range(&content_lines, start_line, end_line);
                ranges.push((byte_start, byte_end, op.text.clone()));
            }
            (Some(_), Some(_)) => {
                return Err(ToolError::Msg(format!(
                    "{}both 'line' and 'lines' specified — use only one",
                    label
                )));
            }
            (None, None) => {
                return Err(ToolError::Msg(format!(
                    "{}neither 'line' nor 'lines' specified — provide one",
                    label
                )));
            }
        }
    }

    Ok((notes, ranges))
}

// ── Tool implementation ──────────────────────────────────────────────────

impl Tool for EditTool {
    const NAME: &'static str = "edit";

    type Error = ToolError;
    type Args = EditArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        let (desc, params) = match edit_system() {
            EditSystem::Similarity => (
                "Edit a file using aider-style SEARCH/REPLACE blocks. Each block finds exact text and replaces it. Multiple blocks in one call are applied atomically. If the search text is not an exact match, whitespace normalization and fuzzy matching are attempted as fallbacks.".to_string(),
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path to the file (relative or absolute)" },
                        "block": { "type": "string", "description": "One or more SEARCH/REPLACE blocks:\n<<<<<<< SEARCH\nexisting code to find\n=======\nreplacement code\n>>>>>>> REPLACE\n\nInclude multiple blocks for separate edits to the same file." }
                    },
                    "required": ["path", "block"]
                }),
            ),
            EditSystem::Hashedit => (
                "Edit a file using tag-based line references. Copy tagged lines from read output. Edit is CAS-guarded via file-level CRC-32 hash. All edits in one call are applied atomically.".to_string(),
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path to the file (relative or absolute)" },
                        "file_crc": { "type": "string", "description": "8-char hex CRC-32 from the read output header [CRC: ...]" },
                        "edits": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "line": { "type": "string", "description": "For single-line edits: copy-paste the tagged line from read output. Format: 'N|TAG content'" },
                                    "lines": { "type": "string", "description": "For range edits: copy-paste multiple tagged lines from read output. Newline-separated." },
                                    "text": { "type": "string", "description": "Replacement text. Use empty string to delete." }
                                },
                                "required": ["text"]
                            },
                            "description": "Array of edit operations"
                        }
                    },
                    "required": ["path", "file_crc", "edits"]
                }),
            ),
        };

        ToolDefinition {
            name: "edit".to_string(),
            description: desc,
            parameters: params,
        }
    }

    async fn call(&self, args: EditArgs) -> Result<String, ToolError> {
        let path = crate::fs::expand_tilde(&args.path);
        let coaching = check_perm_path(&self.permission, &self.ask_tx, "edit", &path).await?;

        let bytes = tokio::fs::read(&path).await?;
        let has_crlf = bytes.windows(2).any(|w| w == b"\r\n");
        let content = String::from_utf8_lossy(&bytes).replace("\r\n", "\n");

        // Determine mode: V1 (block) or V2 (edits)
        let (notes, mut ranges) = if let Some(ref block) = args.block {
            handle_similarity(&path, block, &content).await?
        } else if let (Some(file_crc), Some(edits)) = (&args.file_crc, &args.edits) {
            handle_hashedit(&path, file_crc, edits, &content).await?
        } else if args.block.is_some() {
            // block was Some but empty or parse failed — handle_similarity already errored
            unreachable!()
        } else {
            return Err(ToolError::Msg(
                "Provide either 'block' (SEARCH/REPLACE) or 'file_crc'+'edits' (hashedit). Use /editsys to check the current mode."
                    .to_string(),
            ));
        };

        let edit_count = ranges.len();

        // Apply last-to-first so earlier byte positions remain valid
        ranges.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));

        let mut modified = content;

        for (byte_start, byte_end, replace) in &ranges {
            if *byte_end > modified.len() || *byte_start > modified.len() {
                return Err(ToolError::Msg(
                    "Internal error: edit range exceeds file bounds. The file may have changed. Re-read and retry."
                        .to_string(),
                ));
            }
            modified.replace_range(*byte_start..*byte_end, replace);
        }

        let output = if has_crlf {
            modified.replace('\n', "\r\n")
        } else {
            modified
        };

        crate::fs::atomic_write(&path, &output).await?;
        crate::agent::tools::untrack_read_path(&path);

        let mut result = format!("Applied {} edit(s) to {}", edit_count, path);
        for note in &notes {
            result.push_str(&format!("\n  Note: {}", note));
        }
        if let Some(msg) = coaching {
            result = format!("{}\n\n{}", msg, result);
        }

        Ok(result)
    }
}

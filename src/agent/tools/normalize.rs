pub struct NormalizedText {
    pub text: String,
    spans: Vec<(usize, usize)>,
}

impl NormalizedText {
    pub fn new(s: &str) -> Self {
        let mut spans = Vec::new();
        let text = normalize_impl(s, Some(&mut spans));
        Self { text, spans }
    }

    /// Map a range in normalized text back to the original text.
    ///
    /// Each normalized byte records the original byte span that produced it,
    /// so whitespace dropped before the match (trailing spaces, collapsed
    /// blank lines) no longer shifts the mapped range.
    pub fn original_range(&self, norm_start: usize, norm_len: usize) -> Option<(usize, usize)> {
        let norm_end = norm_start.checked_add(norm_len)?.min(self.spans.len());
        if norm_start >= norm_end {
            return None;
        }
        Some((self.spans[norm_start].0, self.spans[norm_end - 1].1))
    }
}

pub fn normalize_whitespace(s: &str) -> String {
    normalize_impl(s, None)
}

fn normalize_impl(s: &str, mut spans: Option<&mut Vec<(usize, usize)>>) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank_count = 0u32;
    let mut line_start = 0usize;

    if s.is_empty() {
        return out;
    }

    for segment in s.split_inclusive('\n') {
        let body = segment.strip_suffix('\n').unwrap_or(segment);
        let body = body.strip_suffix('\r').unwrap_or(body);
        let trimmed = body.trim_end();

        if trimmed.is_empty() {
            blank_count += 1;
            if blank_count <= 1 {
                out.push('\n');
                if let Some(spans) = spans.as_deref_mut() {
                    spans.push((line_start, line_start + segment.len()));
                }
            }
        } else {
            blank_count = 0;
            for (offset, ch) in trimmed.char_indices() {
                let orig_start = line_start + offset;
                let orig_end = orig_start + ch.len_utf8();
                if ch == '\t' {
                    out.push_str("    ");
                    if let Some(spans) = spans.as_deref_mut() {
                        spans.extend(std::iter::repeat_n((orig_start, orig_end), 4));
                    }
                } else {
                    out.push(ch);
                    if let Some(spans) = spans.as_deref_mut() {
                        spans.extend(std::iter::repeat_n((orig_start, orig_end), ch.len_utf8()));
                    }
                }
            }
            out.push('\n');
            if let Some(spans) = spans.as_deref_mut() {
                spans.push((line_start + body.len(), line_start + segment.len()));
            }
        }

        line_start += segment.len();
    }

    out
}

pub fn levenshtein_similarity(a: &str, b: &str) -> f64 {
    let a_len = a.chars().count();
    let b_len = b.chars().count();
    let max_len = a_len.max(b_len);
    if max_len == 0 {
        return 1.0;
    }
    let dist = levenshtein_distance(a, b);
    1.0 - (dist as f64 / max_len as f64)
}

fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let a_len = a_chars.len();
    let b_len = b_chars.len();

    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    let mut prev: Vec<usize> = (0..=b_len).collect();
    let mut curr = vec![0usize; b_len + 1];

    for i in 1..=a_len {
        curr[0] = i;
        for j in 1..=b_len {
            let cost = if a_chars[i - 1] == b_chars[j - 1] {
                0
            } else {
                1
            };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[b_len]
}

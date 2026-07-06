use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct Pattern {
    regex: OnceLock<Regex>,
    pub original: String,
    is_regex: bool,
}

impl Pattern {
    pub fn new(pattern: &str) -> Self {
        Pattern {
            regex: OnceLock::new(),
            original: pattern.to_string(),
            is_regex: false,
        }
    }

    pub fn new_regex(pattern: &str) -> Self {
        Pattern {
            regex: OnceLock::new(),
            original: pattern.to_string(),
            is_regex: true,
        }
    }

    pub fn matches(&self, input: &str) -> bool {
        let regex = self.regex.get_or_init(|| {
            let expanded = crate::fs::expand_tilde(&self.original);
            let regex_str = if self.is_regex {
                expanded
            } else {
                glob_to_regex(&expanded)
            };
            match Regex::new(&regex_str) {
                Ok(re) => re,
                Err(e) => {
                    // Fail-safe: match everything so a broken deny rule still
                    // denies (rather than silently matching nothing). Log so
                    // the user knows the pattern is invalid.
                    tracing::warn!(
                        "invalid regex pattern {:?}, falling back to match-all: {}",
                        self.original,
                        e
                    );
                    Regex::new("(?s).*").unwrap()
                }
            }
        });
        regex.is_match(input)
    }
}

fn glob_to_regex(pattern: &str) -> String {
    let mut re = String::with_capacity(pattern.len() * 2);
    re.push('^');
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' => {
                if chars.peek() == Some(&'*') {
                    chars.next();
                    if chars.peek() == Some(&'/') {
                        chars.next();
                        re.push_str("(?:.*/)?");
                    } else {
                        re.push_str(".*");
                    }
                } else {
                    re.push_str("[^/]*");
                }
            }
            '?' => re.push('.'),
            '.' => re.push_str("\\."),
            '\\' => re.push_str("\\\\"),
            '(' | ')' | '[' | ']' | '{' | '}' | '+' | '^' | '$' | '|' => {
                re.push('\\');
                re.push(c);
            }
            _ => re.push(c),
        }
    }
    re.push('$');
    re
}

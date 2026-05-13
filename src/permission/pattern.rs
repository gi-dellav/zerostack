use regex::Regex;

#[derive(Debug, Clone)]
pub struct Pattern {
    regex: Regex,
    #[allow(dead_code)]
    pub original: String,
}

impl Pattern {
    pub fn new(pattern: &str) -> Self {
        let expanded = expand_home(pattern);
        let regex_str = glob_to_regex(&expanded);
        let regex = Regex::new(&regex_str).unwrap_or_else(|_| Regex::new("^$").unwrap());
        Pattern {
            regex,
            original: pattern.to_string(),
        }
    }

    pub fn matches(&self, input: &str) -> bool {
        self.regex.is_match(input)
    }
}

fn expand_home(pattern: &str) -> String {
    if pattern == "~" || pattern == "$HOME" {
        if let Some(home) = dirs::home_dir() {
            return home.to_string_lossy().to_string();
        }
        return pattern.to_string();
    }
    if let Some(rest) = pattern.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return format!("{}/{}", home.to_string_lossy(), rest);
        }
        return pattern.to_string();
    }
    if let Some(rest) = pattern.strip_prefix("$HOME/") {
        if let Some(home) = dirs::home_dir() {
            return format!("{}/{}", home.to_string_lossy(), rest);
        }
    }
    pattern.to_string()
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

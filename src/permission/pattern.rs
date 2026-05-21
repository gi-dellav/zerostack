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
        return home_dir_string().unwrap_or_else(|| pattern.to_string());
    }

    for prefix in ["~/", "$HOME/"] {
        if let Some(rest) = pattern.strip_prefix(prefix) {
            return home_dir_string()
                .map(|home| format!("{home}/{rest}"))
                .unwrap_or_else(|| pattern.to_string());
        }
    }

    pattern.to_string()
}

fn home_dir_string() -> Option<String> {
    dirs::home_dir().map(|home| home.to_string_lossy().to_string())
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

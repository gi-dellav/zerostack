use crate::config::ColorsConfig;
use crossterm::style::Color;
use unicode_width::UnicodeWidthStr;

use crate::extras::truncate::truncate_cjk;

const TOOL_SUMMARY_MAX: usize = 200;

fn display_value(val: &str) -> String {
    if val.len() <= TOOL_SUMMARY_MAX {
        format!("\"{}\"", val)
    } else {
        format!("\"{}\"", truncate_cjk(val, TOOL_SUMMARY_MAX, "..."))
    }
}

/// Returns the display width of a string in terminal columns.
/// CJK characters typically occupy 2 columns; ASCII occupies 1.
#[inline]
pub(crate) fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Returns the display width of a single character.
#[inline]
pub(crate) fn char_display_width(c: char) -> usize {
    unicode_width::UnicodeWidthChar::width(c).unwrap_or(0)
}

/// Resolves a color based on monochrome mode.
#[inline]
pub(crate) fn resolve_color(color: Color, monochrome: bool) -> Color {
    if monochrome {
        let _ = color;
        Color::Reset
    } else {
        color
    }
}

/// Parses a color name or hex string into a crossterm Color.
pub(crate) fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim().to_lowercase();
    match s.as_str() {
        "reset" => Some(Color::Reset),
        "black" => Some(Color::Black),
        "dark_grey" | "darkgrey" | "dark_gray" | "darkgray" => Some(Color::DarkGrey),
        "red" => Some(Color::Red),
        "dark_red" | "darkred" => Some(Color::DarkRed),
        "green" => Some(Color::Green),
        "dark_green" | "darkgreen" => Some(Color::DarkGreen),
        "yellow" => Some(Color::Yellow),
        "dark_yellow" | "darkyellow" => Some(Color::DarkYellow),
        "blue" => Some(Color::Blue),
        "dark_blue" | "darkblue" => Some(Color::DarkBlue),
        "magenta" => Some(Color::Magenta),
        "dark_magenta" | "darkmagenta" => Some(Color::DarkMagenta),
        "cyan" => Some(Color::Cyan),
        "dark_cyan" | "darkcyan" => Some(Color::DarkCyan),
        "white" => Some(Color::White),
        "grey" | "gray" => Some(Color::Grey),
        _ => {
            if let Some(num) = s.strip_prefix("ansi:") {
                if let Ok(n) = num.parse::<u8>() {
                    return Some(Color::AnsiValue(n));
                }
                // Value > 255 or invalid
                return None;
            }
            if let Some(hex) = s.strip_prefix('#')
                && hex.len() == 6
                && let (Ok(r), Ok(g), Ok(b)) = (
                    u8::from_str_radix(&hex[0..2], 16),
                    u8::from_str_radix(&hex[2..4], 16),
                    u8::from_str_radix(&hex[4..6], 16),
                )
            {
                return Some(Color::Rgb { r, g, b });
            }
            None
        }
    }
}

/// Formats a tool call showing only the primary file/command parameter.
pub(crate) fn format_tool_call_summary(name: &str, args: &serde_json::Value) -> String {
    let obj = match args {
        serde_json::Value::Object(map) => map,
        _ => return name.to_string(),
    };

    if name == "task" {
        return format_task_summary(obj);
    }

    let primary_keys: &[&str] = match name {
        "read" | "write" | "edit" | "list_dir" => &["path"],
        "grep" => &["pattern", "path"],
        "find_files" => &["pattern"],
        "bash" => &["command"],
        _ => &[],
    };

    let mut shown = Vec::new();
    for key in primary_keys {
        if let Some(serde_json::Value::String(val)) = obj.get(*key) {
            let display_val = if name == "bash" {
                val.clone()
            } else {
                display_value(val)
            };
            shown.push(display_val);
        }
    }

    if shown.is_empty() {
        if let Some((_, serde_json::Value::String(val))) = obj.iter().next() {
            format!("{} {}", name, display_value(val))
        } else {
            name.to_string()
        }
    } else {
        format!("{} {}", name, shown.join(" "))
    }
}

fn format_task_summary(obj: &serde_json::Map<String, serde_json::Value>) -> String {
    let prompts = match obj.get("prompts") {
        Some(serde_json::Value::Array(arr)) => arr,
        _ => return "task".to_string(),
    };
    let parts: Vec<String> = prompts
        .iter()
        .filter_map(|v| v.as_str())
        .map(display_value)
        .collect();
    if parts.is_empty() {
        "task".to_string()
    } else {
        format!("task {}", parts.join(" "))
    }
}

/// Suggests a permission allow pattern for a tool+input combination.
pub(crate) fn suggest_pattern(tool: &str, input: &str) -> String {
    match tool {
        "bash" => {
            let first = input.split_whitespace().next().unwrap_or("*");
            format!("{} *", first)
        }
        "read" | "write" | "edit" | "list_dir" => {
            let expanded = crate::fs::expand_tilde(input);
            let path = std::path::Path::new(&expanded);
            let parent = path
                .parent()
                .map(|p| p.to_string_lossy())
                .unwrap_or(std::borrow::Cow::Borrowed("*"));
            if parent.is_empty() {
                "**".to_string()
            } else {
                format!("{}/**/*", parent)
            }
        }
        "grep" | "find_files" => {
            let first = input.split_whitespace().next().unwrap_or("*");
            format!("{}*", first)
        }
        _ => "*".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_color_ansi_valid() {
        assert_eq!(parse_color("ansi:7"), Some(Color::AnsiValue(7)));
        assert_eq!(parse_color("ansi:0"), Some(Color::AnsiValue(0)));
        assert_eq!(parse_color("ansi:255"), Some(Color::AnsiValue(255)));
    }

    #[test]
    fn parse_color_ansi_invalid() {
        assert_eq!(parse_color("ansi:256"), None);
        assert_eq!(parse_color("ansi:abc"), None);
        assert_eq!(parse_color("ansi:-1"), None);
        assert_eq!(parse_color("ansi:"), None);
    }

    #[test]
    fn parse_color_ansi_whitespace_case() {
        assert_eq!(parse_color(" ANSI:42 "), Some(Color::AnsiValue(42)));
    }
}

/// Resolved theme colors used throughout the TUI.
///
/// Each field is a concrete `Color` resolved from `ColorsConfig` string values
/// via `parse_color`, falling back to the built-in default when the config
/// field is `None` or unparseable.
#[derive(Debug, Clone)]

pub(crate) struct UiColors {
    // Background
    pub chat_background: Option<Color>,
    pub input_background: Option<Color>,
    pub status_background: Option<Color>,

    // Semantic foreground
    pub agent_text: Color,
    pub error: Color,
    pub tool_call: Color,
    pub permission: Color,
    pub by_the_way: Color,
    pub reasoning: Color,
    pub secondary: Color,
    pub success: Color,
    pub heading: Color,
    pub code_block: Color,
    pub link_text: Color,
    pub prompt_marker: Color,
    pub status_foreground: Option<Color>,
    pub scroll_indicator: Color,
    pub picker_secondary: Color,
    pub picker_selected: Color,
}

impl UiColors {
    /// Default color scheme (matches the existing hard-coded constants).
    pub(crate) const fn default_colors() -> Self {
        Self {
            chat_background: None,
            input_background: None,
            status_background: None,
            agent_text: Color::White,
            error: Color::Red,
            tool_call: Color::Yellow,
            permission: Color::Magenta,
            by_the_way: Color::Cyan,
            reasoning: Color::DarkGrey,
            secondary: Color::DarkGrey,
            success: Color::Green,
            heading: Color::Cyan,
            code_block: Color::DarkYellow,
            link_text: Color::DarkCyan,
            prompt_marker: Color::Green,
            status_foreground: None,
            scroll_indicator: Color::DarkGrey,
            picker_secondary: Color::DarkGrey,
            picker_selected: Color::Green,
        }
    }

    /// Resolve all colors from a `ColorsConfig`, falling back to defaults.
    pub fn from_config(cfg: &ColorsConfig) -> Self {
        let defaults = Self::default_colors();
        Self {
            chat_background: cfg.chat_background.as_deref().and_then(parse_color),
            input_background: cfg.input_background.as_deref().and_then(parse_color),
            status_background: cfg.status_background.as_deref().and_then(parse_color),
            agent_text: cfg
                .agent_text
                .as_deref()
                .and_then(parse_color)
                .unwrap_or(defaults.agent_text),
            error: cfg
                .error
                .as_deref()
                .and_then(parse_color)
                .unwrap_or(defaults.error),
            tool_call: cfg
                .tool_call
                .as_deref()
                .and_then(parse_color)
                .unwrap_or(defaults.tool_call),
            permission: cfg
                .permission
                .as_deref()
                .and_then(parse_color)
                .unwrap_or(defaults.permission),
            by_the_way: cfg
                .by_the_way
                .as_deref()
                .and_then(parse_color)
                .unwrap_or(defaults.by_the_way),
            reasoning: cfg
                .reasoning
                .as_deref()
                .and_then(parse_color)
                .unwrap_or(defaults.reasoning),
            secondary: cfg
                .secondary
                .as_deref()
                .and_then(parse_color)
                .unwrap_or(defaults.secondary),
            success: cfg
                .success
                .as_deref()
                .and_then(parse_color)
                .unwrap_or(defaults.success),
            heading: cfg
                .heading
                .as_deref()
                .and_then(parse_color)
                .unwrap_or(defaults.heading),
            code_block: cfg
                .code_block
                .as_deref()
                .and_then(parse_color)
                .unwrap_or(defaults.code_block),
            link_text: cfg
                .link_text
                .as_deref()
                .and_then(parse_color)
                .unwrap_or(defaults.link_text),
            prompt_marker: cfg
                .prompt_marker
                .as_deref()
                .and_then(parse_color)
                .unwrap_or(defaults.prompt_marker),
            status_foreground: cfg.status_foreground.as_deref().and_then(parse_color),
            scroll_indicator: cfg
                .scroll_indicator
                .as_deref()
                .and_then(parse_color)
                .unwrap_or(defaults.scroll_indicator),
            picker_secondary: cfg
                .picker_secondary
                .as_deref()
                .and_then(parse_color)
                .unwrap_or(defaults.picker_secondary),
            picker_selected: cfg
                .picker_selected
                .as_deref()
                .and_then(parse_color)
                .unwrap_or(defaults.picker_selected),
        }
    }
}

#[cfg(test)]
mod ui_colors_tests {
    use super::*;

    #[test]
    fn from_config_empty() {
        let cfg = ColorsConfig::default();
        let colors = UiColors::from_config(&cfg);
        let defaults = UiColors::default_colors();
        assert_eq!(colors.agent_text, defaults.agent_text);
        assert_eq!(colors.error, defaults.error);
        assert_eq!(colors.tool_call, defaults.tool_call);
        assert_eq!(colors.permission, defaults.permission);
        assert_eq!(colors.by_the_way, defaults.by_the_way);
        assert_eq!(colors.chat_background, None);
        assert_eq!(colors.status_foreground, None);
    }

    #[test]
    fn from_config_overrides() {
        let mut cfg = ColorsConfig::default();
        cfg.agent_text = Some(compact_str::CompactString::from("red"));
        cfg.error = Some(compact_str::CompactString::from("ansi:202"));
        cfg.chat_background = Some(compact_str::CompactString::from("#1a1a2e"));
        let colors = UiColors::from_config(&cfg);
        assert_eq!(colors.agent_text, Color::Red);
        assert_eq!(colors.error, Color::AnsiValue(202));
        assert_eq!(
            colors.chat_background,
            Some(Color::Rgb {
                r: 0x1a,
                g: 0x1a,
                b: 0x2e
            })
        );
    }

    #[test]
    fn from_config_invalid_falls_back() {
        let mut cfg = ColorsConfig::default();
        cfg.agent_text = Some(compact_str::CompactString::from("notacolor"));
        cfg.tool_call = Some(compact_str::CompactString::from("ansi:999"));
        let colors = UiColors::from_config(&cfg);
        assert_eq!(colors.agent_text, Color::White);
        assert_eq!(colors.tool_call, Color::Yellow);
    }
}

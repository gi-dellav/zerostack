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

/// Converts an RGB color to the nearest ANSI 256 color index (16-255).
fn rgb_to_ansi256(r: u8, g: u8, b: u8) -> u8 {
    let r = r as f32;
    let g = g as f32;
    let b = b as f32;
    // Check if grayscale: all channels within ~10% of each other
    let mean = (r + g + b) / 3.0;
    let spread = (r - mean).abs().max((g - mean).abs()).max((b - mean).abs());
    if spread < 15.0 {
        // 24 grayscale steps from 232-255
        let gs = ((mean / 255.0) * 23.0).round() as u8;
        return 232 + gs.min(23);
    }
    // 216-color cube: 6 levels per channel (0, 95, 135, 175, 215, 255)
    let levels: [f32; 6] = [0.0, 95.0, 135.0, 175.0, 215.0, 255.0];
    let nearest = |v: f32| -> u8 {
        let mut best = 0u8;
        let mut best_dist = f32::MAX;
        for (i, &l) in levels.iter().enumerate() {
            let dist = (l - v).abs();
            if dist < best_dist {
                best_dist = dist;
                best = i as u8;
            }
        }
        best
    };
    let ri = nearest(r);
    let gi = nearest(g);
    let bi = nearest(b);
    16 + 36 * ri + 6 * gi + bi
}

/// Converts any Color to its nearest ANSI 256-color equivalent.
pub(crate) fn to_ansi_256(color: Color) -> Color {
    match color {
        Color::Reset => Color::Reset,
        Color::Black => Color::AnsiValue(0),
        Color::Red => Color::AnsiValue(1),
        Color::Green => Color::AnsiValue(2),
        Color::Yellow => Color::AnsiValue(3),
        Color::Blue => Color::AnsiValue(4),
        Color::Magenta => Color::AnsiValue(5),
        Color::Cyan => Color::AnsiValue(6),
        Color::White => Color::AnsiValue(7),
        Color::Grey => Color::AnsiValue(7),
        Color::DarkGrey => Color::AnsiValue(8),
        Color::DarkRed => Color::AnsiValue(9),
        Color::DarkGreen => Color::AnsiValue(10),
        Color::DarkYellow => Color::AnsiValue(11),
        Color::DarkBlue => Color::AnsiValue(12),
        Color::DarkMagenta => Color::AnsiValue(13),
        Color::DarkCyan => Color::AnsiValue(14),
        Color::Rgb { r, g, b } => Color::AnsiValue(rgb_to_ansi256(r, g, b)),
        Color::AnsiValue(v) => Color::AnsiValue(v),
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
        "light_blue" | "lightblue" => Some(Color::Rgb {
            r: 0x5f,
            g: 0xaf,
            b: 0xff,
        }),
        "dark_blue" | "darkblue" => Some(Color::DarkBlue),
        "magenta" => Some(Color::Magenta),
        "dark_magenta" | "darkmagenta" => Some(Color::DarkMagenta),
        "cyan" => Some(Color::Cyan),
        "dark_cyan" | "darkcyan" => Some(Color::DarkCyan),
        "white" => Some(Color::White),
        "grey" | "gray" => Some(Color::Grey),
        _ => {
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

/// Byte index of the first char whose display cell starts at or after
/// display column `col` (`s.len()` when the string is narrower). Selection
/// ranges are measured in display columns; this maps them back to byte
/// offsets. A wide (CJK) char straddling `col` is excluded from the range.
pub(crate) fn byte_index_at_display_col(s: &str, col: usize) -> usize {
    if col == 0 {
        return 0;
    }
    let mut width = 0;
    for (i, c) in s.char_indices() {
        if width >= col {
            return i;
        }
        width += char_display_width(c);
    }
    s.len()
}

/// The substring of `s` covering display columns `[start_col, end_col)`.
/// Columns are clamped and may be given in any order.
pub(crate) fn display_col_slice(s: &str, start_col: usize, end_col: usize) -> String {
    let a = byte_index_at_display_col(s, start_col.min(end_col));
    let b = byte_index_at_display_col(s, start_col.max(end_col));
    s[a..b].to_string()
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
    fn byte_index_maps_display_cols() {
        assert_eq!(byte_index_at_display_col("hello", 0), 0);
        assert_eq!(byte_index_at_display_col("hello", 3), 3);
        assert_eq!(byte_index_at_display_col("hello", 99), 5);
        // Wide chars: '你' occupies cols 0-1, '好' cols 2-3.
        assert_eq!(byte_index_at_display_col("你好x", 2), 3);
        // col 1 straddles '你' — excluded, next boundary is '好' at byte 3.
        assert_eq!(byte_index_at_display_col("你好x", 1), 3);
    }

    #[test]
    fn display_col_slice_extracts_ranges() {
        assert_eq!(display_col_slice("hello world", 0, 5), "hello");
        assert_eq!(display_col_slice("hello world", 6, 11), "world");
        assert_eq!(display_col_slice("hello", 3, 1), "el");
        assert_eq!(display_col_slice("hello", 2, 99), "llo");
        assert_eq!(display_col_slice("你好世界", 0, 4), "你好");
        assert_eq!(display_col_slice("a你b", 1, 3), "你");
        assert_eq!(display_col_slice("hello", 2, 2), "");
    }
}

//! Configurable semantic role colors.
//!
//! Every feed block has a semantic [`BlockStyle`] (agent, error, tool,
//! permission, …). Historically each role mapped to a hardcoded color; this
//! module holds the optional overrides coming from a theme's or the config's
//! `[colors.roles]` map. Overrides are global process state, matching the
//! project's other one-shot UI settings (statusline, edit system, …), and
//! are applied/reset whenever a theme is selected so switching themes drops
//! the previous theme's roles.

use std::collections::HashMap;
use std::sync::RwLock;

use crossterm::style::Color;

use super::feed::BlockStyle;
use super::utils::parse_color;
use super::{C_AGENT, C_ERROR, C_PERM, C_TOOL};

static ROLE_OVERRIDES: RwLock<Option<HashMap<BlockStyle, Color>>> = RwLock::new(None);

/// The built-in palette, used for any role without a configured override.
pub(crate) fn default_color(role: BlockStyle) -> Color {
    match role {
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

/// The color a role renders in: the configured override when one was applied
/// by the current theme/config, otherwise [`default_color`].
pub(crate) fn color(role: BlockStyle) -> Color {
    ROLE_OVERRIDES
        .read()
        .ok()
        .and_then(|guard| guard.as_ref().and_then(|m| m.get(&role).copied()))
        .unwrap_or_else(|| default_color(role))
}

/// Map a `[colors.roles]` key to its semantic role. Accepts the plain names
/// (`tool_result` also as `toolresult`, `tool-result`).
fn role_from_name(name: &str) -> Option<BlockStyle> {
    match name.trim().to_lowercase().as_str() {
        "user" => Some(BlockStyle::User),
        "agent" => Some(BlockStyle::Agent),
        "reasoning" => Some(BlockStyle::Reasoning),
        "tool" => Some(BlockStyle::Tool),
        "tool_result" | "toolresult" | "tool-result" => Some(BlockStyle::ToolResult),
        "error" => Some(BlockStyle::Error),
        "system" => Some(BlockStyle::System),
        "welcome" => Some(BlockStyle::Welcome),
        "permission" => Some(BlockStyle::Permission),
        "plain" => Some(BlockStyle::Plain),
        _ => None,
    }
}

/// Apply a `[colors.roles]` map from a theme or the config: reset to the
/// default palette, then apply each known role with a parsable color.
/// Unknown role names and unparsable colors are skipped with a warning so a
/// typo never breaks rendering.
pub(crate) fn apply(roles: &HashMap<String, String>) {
    let mut overrides = HashMap::new();
    for (name, value) in roles {
        match role_from_name(name) {
            Some(role) => match parse_color(value) {
                Some(color) => {
                    overrides.insert(role, color);
                }
                None => {
                    tracing::warn!("ignoring unparsable color '{value}' for role '{name}'");
                }
            },
            None => {
                tracing::warn!("ignoring unknown color role '{name}'");
            }
        }
    }
    if let Ok(mut guard) = ROLE_OVERRIDES.write() {
        *guard = Some(overrides);
    }
}

/// Drop all overrides (a theme/config without a `roles` map restores the
/// default palette).
pub(crate) fn reset() {
    if let Ok(mut guard) = ROLE_OVERRIDES.write() {
        *guard = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_names_resolve() {
        assert_eq!(role_from_name("agent"), Some(BlockStyle::Agent));
        assert_eq!(role_from_name("TOOL_RESULT"), Some(BlockStyle::ToolResult));
        assert_eq!(role_from_name("permission"), Some(BlockStyle::Permission));
        assert_eq!(role_from_name("nope"), None);
    }

    #[test]
    fn overrides_apply_and_reset() {
        reset();
        assert_eq!(color(BlockStyle::Tool), default_color(BlockStyle::Tool));

        let mut roles = HashMap::new();
        roles.insert("tool".to_string(), "#ff5555".to_string());
        roles.insert("error".to_string(), "blue".to_string());
        roles.insert("bogus".to_string(), "red".to_string());
        roles.insert("agent".to_string(), "not-a-color".to_string());
        apply(&roles);

        assert_eq!(
            color(BlockStyle::Tool),
            Color::Rgb {
                r: 0xff,
                g: 0x55,
                b: 0x55
            }
        );
        assert_eq!(color(BlockStyle::Error), Color::Blue);
        // Unknown role / bad color leave the defaults in place.
        assert_eq!(color(BlockStyle::Agent), default_color(BlockStyle::Agent));

        reset();
        assert_eq!(color(BlockStyle::Tool), default_color(BlockStyle::Tool));
    }
}

//! User-facing plugin configuration parsed from the KDL `plugin { ... }` block.
//!
//! zellij hands the block's child key/values to the plugin as a
//! `BTreeMap<String, String>`. Parsing is **total**: every key falls back to a
//! documented default on a missing or malformed value, so a typo in the layout
//! degrades the bar to defaults rather than panicking it. Range concerns (e.g.
//! the clamp on `active_width`'s render budget) live at the render site, not
//! here — this parser preserves whatever valid value the user wrote.

use std::collections::BTreeMap;

/// Parsed plugin configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// Prefix glyph shown before a tab's switch-hint position number.
    pub shortcut_prefix: String,
    /// Column budget for the focused tab's detailed minimap.
    pub active_width: usize,
    /// Whether to draw a 1px dark separator between adjacent panes.
    pub gutter: bool,
}

impl Config {
    /// Command glyph `⌘` — default switch-hint prefix.
    pub const DEFAULT_SHORTCUT_PREFIX: &str = "⌘";
    /// Default column budget for the focused tab's minimap.
    pub const DEFAULT_ACTIVE_WIDTH: usize = 24;
    /// Default gutter state — no separator.
    pub const DEFAULT_GUTTER: bool = false;

    /// Parse the configuration map, falling back to a default for any missing or
    /// malformed value. Total: never panics on bad input.
    pub fn from_configuration(configuration: &BTreeMap<String, String>) -> Self {
        Self {
            shortcut_prefix: configuration
                .get("shortcut_prefix")
                .cloned()
                .unwrap_or_else(|| Self::DEFAULT_SHORTCUT_PREFIX.to_string()),
            active_width: configuration
                .get("active_width")
                .and_then(|raw| raw.parse().ok())
                .unwrap_or(Self::DEFAULT_ACTIVE_WIDTH),
            gutter: configuration
                .get("gutter")
                .and_then(|raw| raw.parse().ok())
                .unwrap_or(Self::DEFAULT_GUTTER),
        }
    }
}

impl Default for Config {
    /// The defaults are exactly what an empty configuration map parses to.
    fn default() -> Self {
        Self::from_configuration(&BTreeMap::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_from(pairs: &[(&str, &str)]) -> Config {
        Config::from_configuration(
            &pairs
                .iter()
                .map(|&(key, value)| (key.to_string(), value.to_string()))
                .collect(),
        )
    }

    #[test]
    fn defaults_when_empty() {
        let config = config_from(&[]);
        assert_eq!(config.shortcut_prefix, "⌘");
        assert_eq!(config.active_width, 24);
        assert!(!config.gutter);
    }

    #[test]
    fn parses_valid_overrides() {
        let config = config_from(&[
            ("shortcut_prefix", "C-"),
            ("active_width", "30"),
            ("gutter", "true"),
        ]);
        assert_eq!(config.shortcut_prefix, "C-");
        assert_eq!(config.active_width, 30);
        assert!(config.gutter);
    }

    #[test]
    fn malformed_active_width_falls_back() {
        assert_eq!(config_from(&[("active_width", "abc")]).active_width, 24);
        assert_eq!(config_from(&[("active_width", "")]).active_width, 24);
    }

    #[test]
    fn malformed_gutter_falls_back() {
        assert!(!config_from(&[("gutter", "maybe")]).gutter);
    }

    #[test]
    fn partial_config_keeps_other_defaults() {
        let config = config_from(&[("active_width", "18")]);
        assert_eq!(config.active_width, 18);
        assert_eq!(config.shortcut_prefix, "⌘");
        assert!(!config.gutter);
    }
}

//! User-facing plugin configuration parsed from the KDL `plugin { ... }` block.
//!
//! zellij hands the block's child key/values to the plugin as a
//! `BTreeMap<String, String>`. Parsing is **total**: every key falls back to a
//! documented default on a missing or malformed value, so a typo in the layout
//! degrades the bar to defaults rather than panicking it. Range concerns (e.g.
//! the clamp on `active_width`'s render budget) live at the render site, not
//! here — this parser preserves whatever valid value the user wrote.

use std::collections::BTreeMap;

use crate::line::Alignment;
use crate::minimap::{GradientMode, GradientShape, GradientSpec, RadialDirection};
use crate::scroll::ScrollMode;

/// Parsed plugin configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// Prefix glyph shown before a tab's switch-hint position number.
    pub shortcut_prefix: String,
    /// Column budget for the focused tab's detailed minimap.
    pub active_width: usize,
    /// How the all-fit tab row is anchored: `Center` re-centers the active block
    /// on each focus change (the strip slides), `Left` pins the row's left edge
    /// at the start of the tab area — just after any reserved prefix (no
    /// whole-strip slide). Governs the all-fit case only — an overflowing strip
    /// always follows the active tab. See [`Alignment`].
    pub align: Alignment,
    /// Columns of empty space left between adjacent tab blocks so the boundary
    /// between screens reads clearly. Defaults to `2`; `0` packs the blocks
    /// flush — the original v0.1.0 look. The gap renders as the cleared pane
    /// background for free: [`crate::paint::compose`] positions each block at
    /// its own column and never paints the inter-block columns.
    pub tab_gap: usize,
    /// Whether to draw a 1px dark separator between adjacent panes.
    pub gutter: bool,
    /// Whether drag-to-reorder is enabled. Off by default: the plugin then
    /// requests only the v0.1.0 permission set (`ReadApplicationState` +
    /// `ChangeApplicationState`), so existing users do not hit a
    /// `RunActionsAsUser` cache miss on auto-update (zellij#4982). On → the
    /// third permission is requested and a tab drag reorders.
    pub reorder: bool,
    /// Gradient sweep applied to each pane block's fill. Defaults to `sheen`
    /// — the polished out-of-the-box look; `off` restores the flat
    /// v0.1.0-style fills. See [`GradientMode`].
    pub gradient: GradientMode,
    /// Geometry of the gradient sweep: `linear` (a straight sweep at
    /// [`Self::gradient_angle`]) or `radial` (a circular sweep from the block's
    /// center). Defaults to `linear`. See [`GradientShape`] (#71).
    pub gradient_shape: GradientShape,
    /// Linear sweep angle in degrees, `[0, 360)` — the perceived on-screen
    /// direction: `0` left→right (the v0.5 look), `90` top→bottom, `180`
    /// right→left, `270` bottom→top. Out-of-range or non-integer values fall
    /// back to `0`. Ignored when `gradient_shape` is `radial` (#71).
    pub gradient_angle: u16,
    /// Direction of a radial sweep: `outward` (base fill at the center) or
    /// `inward` (stop at the center). Defaults to `outward`. Ignored when
    /// `gradient_shape` is `linear`. See [`RadialDirection`] (#71).
    pub gradient_radial: RadialDirection,
    /// Whether inactive tabs render dimmed (receded toward the canvas) so the
    /// active tab reads selected at a glance (#59). On by default; `false`
    /// restores the equally-vivid pre-0.6 strip.
    pub inactive_dim: bool,
    /// Whether the active tab is lifted with a depth cue: in a bar at least four
    /// rows tall every inactive tab recedes by one row (a half-row of background
    /// inset top and bottom) while the active tab fills the full height (#66). On
    /// by default; a no-op below four rows. The row count comes from the layout's
    /// `pane size=N`, so this cue only shows once `size >= 4`. `false` renders
    /// every tab at full height.
    pub perspective: bool,
    /// Whether a clickable "+" button is appended after the last visible tab,
    /// opening (and focusing) a new tab on click (#76). On by default; clicking it
    /// needs only the already-granted `ChangeApplicationState` permission, so no
    /// new permission prompt appears on auto-update. `false` hides the button and
    /// reclaims its columns for the tab strip.
    pub new_tab_button: bool,
    /// How the mouse wheel navigates over the bar (#80). `tab` (default) switches
    /// tabs, `pane` walks the focused pane across tab boundaries, `off` makes the
    /// wheel inert. zellij delivers scroll events without a position, so the
    /// gesture acts on the whole bar. Needs no permission beyond the default set
    /// (`ChangeApplicationState`). See [`ScrollMode`].
    pub scroll: ScrollMode,
}

impl Config {
    /// Command glyph `⌘` plus a trailing space — the default switch-hint prefix.
    /// The space keeps the NerdFont `⌘`, which overflows its cell, from colliding
    /// with the position digit (renders `⌘ 1`, not `⌘1`).
    pub const DEFAULT_SHORTCUT_PREFIX: &str = "⌘ ";
    /// Default column budget for the focused tab's minimap.
    pub const DEFAULT_ACTIVE_WIDTH: usize = 24;
    /// Default alignment — centered, preserving the v0.1.0 sliding behavior so
    /// existing layouts render identically on auto-update (opt into `left` to
    /// anchor the row). Same default-preserve rationale as [`Self::DEFAULT_REORDER`].
    pub const DEFAULT_ALIGN: Alignment = Alignment::Center;
    /// Default gap between tab blocks — `2` cleared columns, so adjacent
    /// screens read as separate blocks out of the box. Set `0` to pack the
    /// blocks flush (the original v0.1.0 look).
    pub const DEFAULT_TAB_GAP: usize = 2;
    /// Default gutter state — no separator.
    pub const DEFAULT_GUTTER: bool = false;
    /// Default reorder state — off, preserving the v0.1.0 permission posture.
    pub const DEFAULT_REORDER: bool = false;
    /// Default gradient mode — `Sheen`, the polished out-of-the-box look.
    /// Set `off` to restore the flat v0.1.0-style fills.
    pub const DEFAULT_GRADIENT: GradientMode = GradientMode::Sheen;
    /// Default gradient shape — `Linear`, a straight sweep. Set `radial` for a
    /// circular sweep from each block's center (#71).
    pub const DEFAULT_GRADIENT_SHAPE: GradientShape = GradientShape::Linear;
    /// Default linear sweep angle — `0` degrees (left→right), preserving the
    /// pre-#71 sheen direction byte-for-byte (#71).
    pub const DEFAULT_GRADIENT_ANGLE: u16 = 0;
    /// Default radial direction — `Outward`, base fill at the center (#71).
    pub const DEFAULT_GRADIENT_RADIAL: RadialDirection = RadialDirection::Outward;
    /// Default inactive-tab dimming — on, so the active tab reads selected
    /// out of the box (#59). Set `false` to restore the equally-vivid strip.
    pub const DEFAULT_INACTIVE_DIM: bool = true;
    /// Default perspective depth cue — on, so a tall enough bar (`pane size >= 4`)
    /// lifts the active tab out of the box (#66). A no-op below four rows. Set
    /// `false` to render every tab at full height.
    pub const DEFAULT_PERSPECTIVE: bool = true;
    /// Default new-tab button — on, so the "+" affordance is present out of the
    /// box (#76). It rides on the already-granted `ChangeApplicationState`
    /// permission, so enabling it by default costs existing users no new prompt.
    pub const DEFAULT_NEW_TAB_BUTTON: bool = true;
    /// Default wheel behaviour — `Tab`, matching zellij's stock tab-bar (scroll
    /// switches tabs). Set `pane` to walk panes, `off` to disable (#80).
    pub const DEFAULT_SCROLL: ScrollMode = ScrollMode::Tab;

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
            align: configuration
                .get("align")
                .and_then(|raw| raw.parse().ok())
                .unwrap_or(Self::DEFAULT_ALIGN),
            tab_gap: configuration
                .get("tab_gap")
                .and_then(|raw| raw.parse().ok())
                .unwrap_or(Self::DEFAULT_TAB_GAP),
            gutter: configuration
                .get("gutter")
                .and_then(|raw| raw.parse().ok())
                .unwrap_or(Self::DEFAULT_GUTTER),
            reorder: configuration
                .get("reorder")
                .and_then(|raw| raw.parse().ok())
                .unwrap_or(Self::DEFAULT_REORDER),
            gradient: configuration
                .get("gradient")
                .and_then(|raw| raw.parse().ok())
                .unwrap_or(Self::DEFAULT_GRADIENT),
            gradient_shape: configuration
                .get("gradient_shape")
                .and_then(|raw| raw.parse().ok())
                .unwrap_or(Self::DEFAULT_GRADIENT_SHAPE),
            gradient_angle: configuration
                .get("gradient_angle")
                .and_then(|raw| raw.parse::<u16>().ok())
                .filter(|degrees| *degrees < 360)
                .unwrap_or(Self::DEFAULT_GRADIENT_ANGLE),
            gradient_radial: configuration
                .get("gradient_radial")
                .and_then(|raw| raw.parse().ok())
                .unwrap_or(Self::DEFAULT_GRADIENT_RADIAL),
            inactive_dim: configuration
                .get("inactive_dim")
                .and_then(|raw| raw.parse().ok())
                .unwrap_or(Self::DEFAULT_INACTIVE_DIM),
            perspective: configuration
                .get("perspective")
                .and_then(|raw| raw.parse().ok())
                .unwrap_or(Self::DEFAULT_PERSPECTIVE),
            new_tab_button: configuration
                .get("new_tab_button")
                .and_then(|raw| raw.parse().ok())
                .unwrap_or(Self::DEFAULT_NEW_TAB_BUTTON),
            scroll: configuration
                .get("scroll")
                .and_then(|raw| raw.parse().ok())
                .unwrap_or(Self::DEFAULT_SCROLL),
        }
    }

    /// Bundle the gradient-related keys into the [`GradientSpec`] the renderer
    /// consumes — mode plus its shape, angle, and radial direction (#71).
    pub fn gradient_spec(&self) -> GradientSpec {
        GradientSpec {
            mode: self.gradient,
            shape: self.gradient_shape,
            angle: self.gradient_angle,
            radial: self.gradient_radial,
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
        assert_eq!(config.shortcut_prefix, "⌘ ");
        assert_eq!(config.active_width, 24);
        assert_eq!(config.align, Alignment::Center);
        assert_eq!(config.tab_gap, 2);
        assert!(!config.gutter);
        assert!(!config.reorder);
        assert_eq!(config.gradient, GradientMode::Sheen);
        assert_eq!(config.gradient_shape, GradientShape::Linear);
        assert_eq!(config.gradient_angle, 0);
        assert_eq!(config.gradient_radial, RadialDirection::Outward);
        assert!(config.inactive_dim);
        assert!(config.perspective);
        assert!(config.new_tab_button);
        assert_eq!(config.scroll, ScrollMode::Tab);
    }

    #[test]
    fn parses_valid_overrides() {
        let config = config_from(&[
            ("shortcut_prefix", "C-"),
            ("active_width", "30"),
            ("align", "left"),
            ("tab_gap", "1"),
            ("gutter", "true"),
            ("reorder", "true"),
            ("gradient", "sheen"),
        ]);
        assert_eq!(config.shortcut_prefix, "C-");
        assert_eq!(config.active_width, 30);
        assert_eq!(config.align, Alignment::Left);
        assert_eq!(config.tab_gap, 1);
        assert!(config.gutter);
        assert!(config.reorder);
        assert_eq!(config.gradient, GradientMode::Sheen);
    }

    #[test]
    fn parses_weave_gradient() {
        assert_eq!(
            config_from(&[("gradient", "weave")]).gradient,
            GradientMode::Weave
        );
    }

    #[test]
    fn malformed_gradient_falls_back() {
        assert_eq!(
            config_from(&[("gradient", "rainbow")]).gradient,
            GradientMode::Sheen
        );
        assert_eq!(
            config_from(&[("gradient", "")]).gradient,
            GradientMode::Sheen
        );
        // Case-sensitive: only exact "off" / "sheen" / "weave" parse — a
        // capitalized "Weave" falls back to the default instead of Weave.
        assert_eq!(
            config_from(&[("gradient", "Weave")]).gradient,
            GradientMode::Sheen
        );
    }

    #[test]
    fn parses_explicit_off_gradient() {
        assert_eq!(
            config_from(&[("gradient", "off")]).gradient,
            GradientMode::Off
        );
    }

    #[test]
    fn parses_gradient_shape_and_radial_direction() {
        let config = config_from(&[("gradient_shape", "radial"), ("gradient_radial", "inward")]);
        assert_eq!(config.gradient_shape, GradientShape::Radial);
        assert_eq!(config.gradient_radial, RadialDirection::Inward);
        // Explicit linear/outward parse too.
        let config = config_from(&[("gradient_shape", "linear"), ("gradient_radial", "outward")]);
        assert_eq!(config.gradient_shape, GradientShape::Linear);
        assert_eq!(config.gradient_radial, RadialDirection::Outward);
    }

    #[test]
    fn malformed_gradient_shape_and_radial_fall_back() {
        // Unknown / empty / wrong-case values keep the documented defaults.
        assert_eq!(
            config_from(&[("gradient_shape", "circle")]).gradient_shape,
            GradientShape::Linear
        );
        assert_eq!(
            config_from(&[("gradient_shape", "Radial")]).gradient_shape,
            GradientShape::Linear
        );
        assert_eq!(
            config_from(&[("gradient_radial", "out")]).gradient_radial,
            RadialDirection::Outward
        );
        assert_eq!(
            config_from(&[("gradient_radial", "")]).gradient_radial,
            RadialDirection::Outward
        );
    }

    #[test]
    fn parses_gradient_angle_within_range() {
        assert_eq!(config_from(&[("gradient_angle", "90")]).gradient_angle, 90);
        assert_eq!(config_from(&[("gradient_angle", "45")]).gradient_angle, 45);
        assert_eq!(config_from(&[("gradient_angle", "0")]).gradient_angle, 0);
        // 359 is the largest in-range value; 360 is not (the range is [0, 360)).
        assert_eq!(
            config_from(&[("gradient_angle", "359")]).gradient_angle,
            359
        );
    }

    #[test]
    fn out_of_range_or_malformed_gradient_angle_falls_back_to_zero() {
        // 360 and beyond are out of the half-open [0, 360) range → default 0.
        assert_eq!(config_from(&[("gradient_angle", "360")]).gradient_angle, 0);
        assert_eq!(config_from(&[("gradient_angle", "720")]).gradient_angle, 0);
        // Non-integer, empty, and negative values do not parse as u16 → 0.
        assert_eq!(config_from(&[("gradient_angle", "45.5")]).gradient_angle, 0);
        assert_eq!(
            config_from(&[("gradient_angle", "diagonal")]).gradient_angle,
            0
        );
        assert_eq!(config_from(&[("gradient_angle", "")]).gradient_angle, 0);
        assert_eq!(config_from(&[("gradient_angle", "-90")]).gradient_angle, 0);
    }

    #[test]
    fn gradient_spec_bundles_the_gradient_keys() {
        let spec = config_from(&[
            ("gradient", "weave"),
            ("gradient_shape", "radial"),
            ("gradient_angle", "135"),
            ("gradient_radial", "inward"),
        ])
        .gradient_spec();
        assert_eq!(spec.mode, GradientMode::Weave);
        assert_eq!(spec.shape, GradientShape::Radial);
        assert_eq!(spec.angle, 135);
        assert_eq!(spec.radial, RadialDirection::Inward);
    }

    #[test]
    fn parses_explicit_center_align() {
        assert_eq!(config_from(&[("align", "center")]).align, Alignment::Center);
    }

    #[test]
    fn malformed_align_falls_back() {
        assert_eq!(
            config_from(&[("align", "sideways")]).align,
            Alignment::Center
        );
        assert_eq!(config_from(&[("align", "")]).align, Alignment::Center);
        // Case-sensitive: only exact "left" / "center" parse.
        assert_eq!(config_from(&[("align", "Left")]).align, Alignment::Center);
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
    fn malformed_reorder_falls_back() {
        assert!(!config_from(&[("reorder", "yes")]).reorder);
        assert!(!config_from(&[("reorder", "")]).reorder);
    }

    #[test]
    fn malformed_tab_gap_falls_back() {
        assert_eq!(config_from(&[("tab_gap", "wide")]).tab_gap, 2);
        assert_eq!(config_from(&[("tab_gap", "")]).tab_gap, 2);
        // Negative values do not parse as `usize` — fall back to the default.
        assert_eq!(config_from(&[("tab_gap", "-1")]).tab_gap, 2);
    }

    #[test]
    fn parses_explicit_zero_tab_gap() {
        assert_eq!(config_from(&[("tab_gap", "0")]).tab_gap, 0);
    }

    #[test]
    fn parses_explicit_inactive_dim_off() {
        assert!(!config_from(&[("inactive_dim", "false")]).inactive_dim);
    }

    #[test]
    fn malformed_inactive_dim_falls_back() {
        assert!(config_from(&[("inactive_dim", "no")]).inactive_dim);
        assert!(config_from(&[("inactive_dim", "")]).inactive_dim);
    }

    #[test]
    fn parses_explicit_perspective_off() {
        // The depth cue is on by default; an explicit `false` opts out.
        assert!(!config_from(&[("perspective", "false")]).perspective);
    }

    #[test]
    fn parses_scroll_modes() {
        assert_eq!(config_from(&[("scroll", "tab")]).scroll, ScrollMode::Tab);
        assert_eq!(config_from(&[("scroll", "pane")]).scroll, ScrollMode::Pane);
        assert_eq!(config_from(&[("scroll", "off")]).scroll, ScrollMode::Off);
    }

    #[test]
    fn malformed_scroll_falls_back_to_tab() {
        // Unknown / wrong-case / empty values keep the zellij-stock default.
        assert_eq!(config_from(&[("scroll", "wheel")]).scroll, ScrollMode::Tab);
        assert_eq!(config_from(&[("scroll", "Tab")]).scroll, ScrollMode::Tab);
        assert_eq!(config_from(&[("scroll", "")]).scroll, ScrollMode::Tab);
    }

    #[test]
    fn malformed_perspective_falls_back() {
        // A malformed or empty value keeps the on-by-default cue.
        assert!(config_from(&[("perspective", "sometimes")]).perspective);
        assert!(config_from(&[("perspective", "")]).perspective);
    }

    #[test]
    fn parses_explicit_new_tab_button_off() {
        // The "+" button is on by default; an explicit `false` hides it.
        assert!(!config_from(&[("new_tab_button", "false")]).new_tab_button);
    }

    #[test]
    fn malformed_new_tab_button_falls_back() {
        // A malformed or empty value keeps the on-by-default button.
        assert!(config_from(&[("new_tab_button", "nope")]).new_tab_button);
        assert!(config_from(&[("new_tab_button", "")]).new_tab_button);
    }

    #[test]
    fn partial_config_keeps_other_defaults() {
        let config = config_from(&[("active_width", "18")]);
        assert_eq!(config.active_width, 18);
        assert_eq!(config.shortcut_prefix, "⌘ ");
        assert_eq!(config.align, Alignment::Center);
        assert_eq!(config.tab_gap, 2);
        assert!(!config.gutter);
        assert!(!config.reorder);
    }
}

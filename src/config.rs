//! User-facing plugin configuration parsed from the KDL `plugin { ... }` block.
//!
//! zellij hands the block's child key/values to the plugin as a
//! `BTreeMap<String, String>`. Parsing is **total**: every key falls back to a
//! documented default on a missing or malformed value, so a typo in the layout
//! degrades the bar to defaults rather than panicking it. Range concerns (e.g.
//! the clamp on `active_width`'s render budget) live at the render site, not
//! here — this parser preserves whatever valid value the user wrote.

use std::collections::BTreeMap;

use crate::color::Rgb;
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
    /// Whether each tab block draws a clickable close button near its top-right
    /// corner, closing that tab on click (#86). On by default (#94): the close
    /// glyph rides on the already-granted `ChangeApplicationState` permission
    /// (same family as `new_tab`, #76), so it triggers no new permission prompt on
    /// auto-update (zellij#4982) — the policy is "features on out of the box unless
    /// they need a new permission." The glyph shows on every grid-rung tab, but
    /// only while more than one tab is open, so the last tab can never be closed
    /// out from under the session. Set `false` to hide it: a misclick on an
    /// inactive tab's corner then can't close it (the corner stays a switch
    /// target). The drawn form follows the terminal — the Nerd Font glyph, or the
    /// ASCII `×` under a simplified UI (#94).
    pub close_button: bool,
    /// Foreground color of the close glyph (#94 follow-up). `Theme` (default)
    /// preserves the original behavior — the Nerd Font glyph follows the theme's
    /// alert/error red (`exit_code_error`) and the ASCII `×` stays black. The
    /// theme tie-in backfires on a theme that defines its `red`/error color as a
    /// dark shade (e.g. `sobrio`'s `red "#121212"`): the glyph then renders
    /// near-black. Set `fg` (white, matching the labels/badge), `red` (a fixed
    /// red independent of the theme), or a `#rrggbb` hex to override it. See
    /// [`CloseColor`].
    pub close_button_color: CloseColor,
    /// How the mouse wheel navigates over the tab bar (#80, restored #108):
    /// `tab` (default) switches tabs, `pane` walks the focused pane in reading
    /// order, `off` disables it. One wheel event = one step — the rate-limiter
    /// #104 removed (#83/#96/#100) is gone for good (#108), so a stepless device
    /// (trackpad, Magic Mouse) whose flick bursts events steps several at once;
    /// `off` is the opt-out. Rides the already-granted `ChangeApplicationState`
    /// (same family as click-to-switch #8 / click-to-focus #74), so it triggers
    /// no new permission prompt on auto-update (zellij#4982). See [`ScrollMode`].
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
    /// anchor the row).
    pub const DEFAULT_ALIGN: Alignment = Alignment::Center;
    /// Default gap between tab blocks — `2` cleared columns, so adjacent
    /// screens read as separate blocks out of the box. Set `0` to pack the
    /// blocks flush (the original v0.1.0 look).
    pub const DEFAULT_TAB_GAP: usize = 2;
    /// Default gutter state — no separator.
    pub const DEFAULT_GUTTER: bool = false;
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
    /// Default close-button state — on (#94), so the close affordance is present
    /// out of the box. It rides on the already-granted `ChangeApplicationState`
    /// permission (#86), so enabling it by default costs existing users no new
    /// prompt and no auto-update freeze (zellij#4982). Set `false` to hide it.
    pub const DEFAULT_CLOSE_BUTTON: bool = true;
    /// Default close-glyph color — [`CloseColor::Theme`], preserving the original
    /// theme-driven behavior so existing users see no change. Override with `fg`,
    /// `red`, or a `#rrggbb` hex when a theme's dark error color makes the glyph
    /// hard to read (#94 follow-up).
    pub const DEFAULT_CLOSE_BUTTON_COLOR: CloseColor = CloseColor::Theme;
    /// Default wheel behaviour — `Tab`, matching zellij's stock tab-bar (scroll
    /// over the bar switches tabs). Set `pane` to walk panes in reading order, or
    /// `off` to disable wheel navigation entirely (#108).
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
            close_button: configuration
                .get("close_button")
                .and_then(|raw| raw.parse().ok())
                .unwrap_or(Self::DEFAULT_CLOSE_BUTTON),
            close_button_color: configuration
                .get("close_button_color")
                .and_then(|raw| raw.parse().ok())
                .unwrap_or(Self::DEFAULT_CLOSE_BUTTON_COLOR),
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

/// How the close glyph's foreground color is chosen (#94 follow-up). Resolved to
/// a concrete [`Rgb`] at the render site via [`CloseColor::resolve`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CloseColor {
    /// Follow the theme: the Nerd Font glyph takes the theme's alert/error red
    /// (`exit_code_error`), the ASCII `×` stays black. The original behavior, and
    /// the default — but a theme whose `red`/error color is a dark shade tints
    /// the glyph toward black.
    #[default]
    Theme,
    /// The labels/badge foreground white ([`crate::minimap::ACTIVE_FG`]) — always
    /// readable on any pane fill, theme-independent, and sidesteps a red glyph
    /// reading poorly over a red pane.
    Fg,
    /// A fixed red ([`crate::color::DEFAULT_ALERT`]) independent of the theme, so
    /// the "close = danger" cue survives a theme that defines its red as a dark
    /// shade.
    Red,
    /// An explicit `#rrggbb` color.
    Custom(Rgb),
}

impl CloseColor {
    /// Resolve to the close glyph's foreground. `theme_default` is the per-glyph
    /// color used when this is [`Theme`](CloseColor::Theme): the Nerd Font glyph
    /// passes the theme's alert red, the ASCII `×` passes black. The other
    /// variants ignore it and return a fixed color.
    pub fn resolve(self, theme_default: Rgb) -> Rgb {
        match self {
            CloseColor::Theme => theme_default,
            CloseColor::Fg => crate::minimap::ACTIVE_FG,
            CloseColor::Red => crate::color::DEFAULT_ALERT,
            CloseColor::Custom(rgb) => rgb,
        }
    }
}

/// Parse a `#rrggbb` (or bare `rrggbb`) hex color. Returns `None` on any other
/// shape so [`Config::from_configuration`] stays total (never panics): the
/// ASCII-and-length guard keeps the byte slicing in range for every input.
fn parse_hex_color(raw: &str) -> Option<Rgb> {
    let hex = raw.strip_prefix('#').unwrap_or(raw);
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some((channel(0)?, channel(2)?, channel(4)?))
}

impl std::str::FromStr for CloseColor {
    type Err = ();

    /// `"theme"`, `"fg"` (aliases `"white"` / `"foreground"`), `"red"`, or a
    /// `#rrggbb` hex; any other value errors so the config parser falls back to
    /// the documented default ([`CloseColor::Theme`]) rather than panicking.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "theme" => Ok(Self::Theme),
            "fg" | "white" | "foreground" => Ok(Self::Fg),
            "red" => Ok(Self::Red),
            other => parse_hex_color(other).map(Self::Custom).ok_or(()),
        }
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
        assert_eq!(config.gradient, GradientMode::Sheen);
        assert_eq!(config.gradient_shape, GradientShape::Linear);
        assert_eq!(config.gradient_angle, 0);
        assert_eq!(config.gradient_radial, RadialDirection::Outward);
        assert!(config.inactive_dim);
        assert!(config.perspective);
        assert!(config.new_tab_button);
        assert!(config.close_button);
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
            ("gradient", "sheen"),
        ]);
        assert_eq!(config.shortcut_prefix, "C-");
        assert_eq!(config.active_width, 30);
        assert_eq!(config.align, Alignment::Left);
        assert_eq!(config.tab_gap, 1);
        assert!(config.gutter);
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
    fn parses_explicit_close_button_off() {
        // The close glyph is on by default (#94); an explicit `false` opts out.
        assert!(!config_from(&[("close_button", "false")]).close_button);
    }

    #[test]
    fn malformed_close_button_falls_back() {
        // A malformed or empty value keeps the on-by-default close button (#94).
        assert!(config_from(&[("close_button", "yes")]).close_button);
        assert!(config_from(&[("close_button", "")]).close_button);
    }

    #[test]
    fn parses_close_button_color_keywords_and_hex() {
        // The keyword forms and both hex spellings (`#rrggbb` and bare `rrggbb`)
        // all parse (#94 follow-up).
        assert_eq!(
            config_from(&[("close_button_color", "theme")]).close_button_color,
            CloseColor::Theme
        );
        assert_eq!(
            config_from(&[("close_button_color", "fg")]).close_button_color,
            CloseColor::Fg
        );
        assert_eq!(
            config_from(&[("close_button_color", "white")]).close_button_color,
            CloseColor::Fg
        );
        assert_eq!(
            config_from(&[("close_button_color", "red")]).close_button_color,
            CloseColor::Red
        );
        assert_eq!(
            config_from(&[("close_button_color", "#d70000")]).close_button_color,
            CloseColor::Custom((215, 0, 0))
        );
        assert_eq!(
            config_from(&[("close_button_color", "00ff80")]).close_button_color,
            CloseColor::Custom((0, 255, 128))
        );
    }

    #[test]
    fn malformed_close_button_color_falls_back_to_theme() {
        // Unset, empty, or any unparseable value keeps the theme-driven default,
        // and a length-or-charset-wrong hex never panics the total parser.
        assert_eq!(
            config_from(&[]).close_button_color,
            CloseColor::Theme,
            "default is theme"
        );
        for bad in ["bogus", "", "#12", "#12345g", "redd", "#1234567"] {
            assert_eq!(
                config_from(&[("close_button_color", bad)]).close_button_color,
                CloseColor::Theme,
                "malformed {bad:?} falls back to theme"
            );
        }
    }

    #[test]
    fn close_color_resolves_overrides_and_theme_passthrough() {
        // `Theme` returns the per-glyph default it is handed; the others ignore it.
        let theme_default = (10, 20, 30);
        assert_eq!(CloseColor::Theme.resolve(theme_default), theme_default);
        assert_eq!(
            CloseColor::Custom((1, 2, 3)).resolve(theme_default),
            (1, 2, 3)
        );
        assert_eq!(
            CloseColor::Fg.resolve(theme_default),
            crate::minimap::ACTIVE_FG
        );
        assert_eq!(
            CloseColor::Red.resolve(theme_default),
            crate::color::DEFAULT_ALERT
        );
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
    }

    #[test]
    fn parses_scroll_modes() {
        assert_eq!(config_from(&[("scroll", "tab")]).scroll, ScrollMode::Tab);
        assert_eq!(config_from(&[("scroll", "pane")]).scroll, ScrollMode::Pane);
        assert_eq!(config_from(&[("scroll", "off")]).scroll, ScrollMode::Off);
    }

    #[test]
    fn malformed_scroll_falls_back_to_tab() {
        // Unknown, wrong-case, and empty values keep the documented default (tab).
        assert_eq!(config_from(&[("scroll", "wheel")]).scroll, ScrollMode::Tab);
        assert_eq!(config_from(&[("scroll", "Tab")]).scroll, ScrollMode::Tab);
        assert_eq!(config_from(&[("scroll", "")]).scroll, ScrollMode::Tab);
    }
}

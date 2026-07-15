//! Theme-derived, identity-stable pane colors.
//!
//! Pure and dependency-free, so it unit-tests off-wasm. The plugin layer
//! ([`crate::State`]) reads the live theme palette from `ModeInfo.style`,
//! converts each `PaletteColor` to [`Rgb`], and hands the resulting slot
//! vector to [`Palette::new`]. This module never calls zellij — it only
//! cycles the colors it is given.
//!
//! The cardinal rule (issue #5): a color is keyed on a pane's **stable
//! identity**, not its position in any list. [`Palette::color_for`] maps a
//! `pane_id` to a slot by modulo, so a given pane keeps its color across
//! repaints even as siblings open, close, or move.

/// 24-bit color. Canonical home — [`crate::minimap`] re-exports this alias.
pub type Rgb = (u8, u8, u8);

/// The sentinel an *unset* theme color collapses to. zellij encodes a missing
/// color as `Rgb((0, 0, 0))`, `EightBit(0)`, or `EightBit(16)`, all of which
/// `rgb()` resolves to exactly this — so it doubles as the "invisible fill"
/// marker [`Palette::new`] drops from the slot cycle (issue #27).
const BLACK: Rgb = (0, 0, 0);

/// Visible stand-in when a theme leaves `accent` (`frame_highlight.base`) unset
/// and it collapses to [`BLACK`]. Matches [`Palette::fallback`]'s accent
/// (`from_eightbit(166)`, orange) so a degenerate theme still gets a sensible
/// focus color rather than an invisible one.
const DEFAULT_ACCENT: Rgb = (215, 95, 0);

/// Tokyonight background (`#1a1b26`) — the canvas every block sits on.
/// Canonical home; [`crate::minimap`] re-exposes it as its `BG`. Also the
/// target of [`Palette::dimmed`]: inactive tabs recede *toward the canvas*,
/// not toward plain black, so they sink into the bar instead of graying out.
pub(crate) const CANVAS: Rgb = (26, 27, 38);

/// Convert an xterm-256 palette index to RGB.
///
/// Mirrors zellij's own `eightbit_to_rgb`, which is not re-exported to
/// plugins (`zellij_tile::prelude` surfaces `zellij_utils::data`, not its
/// `shared` module). The 16 system colors use the common xterm defaults,
/// 16–231 the 6×6×6 color cube, and 232–255 the 24-step grayscale ramp.
/// Every color the default theme actually uses lands in the cube/grayscale
/// ranges, where this table is exact; the 0–15 system colors are
/// terminal-dependent and reproduced here at their conventional values.
pub const fn from_eightbit(c: u8) -> Rgb {
    // Conventional xterm/VGA values for the 16 ANSI system colors.
    const SYSTEM: [Rgb; 16] = [
        (0, 0, 0),
        (128, 0, 0),
        (0, 128, 0),
        (128, 128, 0),
        (0, 0, 128),
        (128, 0, 128),
        (0, 128, 128),
        (192, 192, 192),
        (128, 128, 128),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (0, 0, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    // The six cube levels: 0, then 95 + 40·k.
    const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];
    match c {
        0..=15 => SYSTEM[c as usize],
        16..=231 => {
            let n = c - 16;
            (
                CUBE[(n / 36) as usize],
                CUBE[(n / 6 % 6) as usize],
                CUBE[(n % 6) as usize],
            )
        }
        232..=255 => {
            let v = 8 + 10 * (c - 232);
            (v, v, v)
        }
    }
}

/// Perceived luminance (Rec. 601 luma) of a color, `0..=255`. Decides which way
/// a focus ring shifts: lighten a dark fill, darken a light one.
fn luma((r, g, b): Rgb) -> u8 {
    ((r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000) as u8
}

/// Blend `from` toward `to` by `percent` (`0` = `from`, `100` = `to`), per
/// channel. Overflow-safe: the lerp runs in `i32` and lands back in `0..=255`.
pub(crate) fn mixed(from: Rgb, to: Rgb, percent: u8) -> Rgb {
    let lerp = |a: u8, b: u8| (a as i32 + (b as i32 - a as i32) * percent as i32 / 100) as u8;
    (lerp(from.0, to.0), lerp(from.1, to.1), lerp(from.2, to.2))
}

/// The luma-opposite extreme of `base`: white for a dark color, black for a
/// light one. The single direction rule every "stay legible on either theme
/// polarity" shade follows — [`derived_ring`], [`gradient_at`], and the
/// new-tab button fill (#76) all sweep `base` toward this.
fn contrast_target(base: Rgb) -> Rgb {
    if luma(base) < 128 {
        (255, 255, 255)
    } else {
        (0, 0, 0)
    }
}

/// Luminance-aware focus-ring shade of `fill`: lighten a dark fill toward white,
/// darken a light one toward black, by a fixed mix. The ring stays in the same
/// hue family as the pane it surrounds (cohesive) while reading as a distinct
/// outline — it is never equal to `fill`. This is the default ring when a layout
/// does not pin one explicitly (issue #32).
fn derived_ring(fill: Rgb) -> Rgb {
    /// Mix fraction toward white/black. A visual parameter tuned for a ring that
    /// reads as a clear outline at minimap scale — high enough to stand out
    /// against the fill without washing out to pure white/black, not a
    /// correctness constant.
    const SHIFT_PERCENT: u8 = 55;
    mixed(fill, contrast_target(fill), SHIFT_PERCENT)
}

/// One sRGB channel decoded to linear light (`0.0..=1.0`), via the exact
/// piecewise sRGB EOTF (not the `^2.2` approximation). Inverse of
/// [`linear_to_channel`].
fn channel_to_linear(c: u8) -> f32 {
    let s = c as f32 / 255.0;
    if s <= 0.04045 {
        return s / 12.92;
    }
    ((s + 0.055) / 1.055).powf(2.4)
}

/// Linear-light intensity re-encoded to an sRGB channel. Inverse of
/// [`channel_to_linear`]; clamps so float drift never wraps a byte.
fn linear_to_channel(l: f32) -> u8 {
    let s = if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round().clamp(0.0, 255.0) as u8
}

/// Blend `from` toward `to` by `t` (`0.0` = `from`, `1.0` = `to`) in
/// **linear-light** space: decode sRGB → linear, lerp, re-encode. Unlike the
/// raw-sRGB [`mixed`], equal `t` steps advance perceived lightness evenly, so
/// gradients built on this don't band (#46).
fn mixed_linear(from: Rgb, to: Rgb, t: f32) -> Rgb {
    let lerp = |a: u8, b: u8| {
        let (la, lb) = (channel_to_linear(a), channel_to_linear(b));
        linear_to_channel(la + (lb - la) * t)
    };
    (lerp(from.0, to.0), lerp(from.1, to.1), lerp(from.2, to.2))
}

/// The color `percent` of the way along `fill`'s gradient sweep (#40).
///
/// The sweep runs from `fill` toward a luminance-shifted stop — the same
/// lighten-a-dark / darken-a-light direction rule as [`derived_ring`], so a
/// light theme fill never blows out to white. `0` is the base fill, `100` the
/// full stop. The mix happens in linear-light space ([`mixed_linear`]) so the
/// ramp is perceptually even instead of banded (#46). The stop's mix fraction
/// is a visual parameter tuned so the sweep reads as a sheen at minimap
/// scale; ring pixels are painted solid on top of the sweep (see
/// [`crate::minimap`]), so the focus outline stays intact even where the
/// sweep's far end approaches the ring's shade.
pub(crate) fn gradient_at(fill: Rgb, percent: u8) -> Rgb {
    /// Mix fraction toward white/black at the sweep's far end. A visual
    /// parameter (like [`derived_ring`]'s `SHIFT_PERCENT`), not a
    /// correctness constant — retune freely if the sheen reads too
    /// strong/weak after curve changes.
    const SWEEP_PERCENT: f32 = 0.60;
    // Composing the two lerps in linear space collapses to a single mix:
    // fill→stop by t equals fill→target by t·SWEEP_PERCENT.
    mixed_linear(
        fill,
        contrast_target(fill),
        SWEEP_PERCENT * percent as f32 / 100.0,
    )
}

/// The muted fill the inline new-tab `+` button block is painted with (#76):
/// the bar [`CANVAS`] lifted a little toward its [`contrast_target`] (lighter on
/// a dark bar, darker on a light one). Deliberately a *small* lift — the button
/// is a quiet affordance that reads as part of the strip, not a competing tab.
pub(crate) fn button_fill() -> Rgb {
    /// Mix fraction of the canvas toward its contrast extreme. A visual
    /// parameter (like [`derived_ring`]'s `SHIFT_PERCENT`) — small so the
    /// button stays inconspicuous; retune freely.
    const BUTTON_FILL_BLEND: u8 = 12;
    mixed(CANVAS, contrast_target(CANVAS), BUTTON_FILL_BLEND)
}

/// The `+` glyph foreground for the new-tab button (#76): the same contrast
/// direction as [`button_fill`] but a stronger mix, so the affordance is clearly
/// legible on the muted fill without pulling focus from the tab labels.
pub(crate) fn button_glyph() -> Rgb {
    /// Mix fraction of the canvas toward its contrast extreme for the glyph —
    /// stronger than the fill's so the `+` stands off it, soft enough not to
    /// compete with the labels. A visual parameter, retune freely.
    const BUTTON_GLYPH_BLEND: u8 = 60;
    mixed(CANVAS, contrast_target(CANVAS), BUTTON_GLYPH_BLEND)
}

/// A theme-derived color assignment for panes.
///
/// `slots` is cycled by pane id, so identity maps to a stable color across
/// repaints — focus does **not** repaint a pane's fill (issue #47): the pane
/// keeps its identity hue and the ring outline (plus the highlighted label)
/// is what marks focus. The ring is derived **from the pane's own fill** as a
/// luminance-shifted shade ([`derived_ring`]), so a blue pane gets a
/// slightly-different-blue outline rather than a theme-wide accent ring. The
/// accent survives only as the single-slot fallback fill and the seed of the
/// [`hint`](Self::hint) text shade.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    slots: Vec<Rgb>,
    hint: Rgb,
    accent: Rgb,
    /// The theme's alert/error red (`exit_code_error.base`), used as the close
    /// glyph foreground (#86). Defaults to zellij's own `EightBit(124)` when a
    /// theme leaves it unset, so the close affordance is always a recognizable
    /// red rather than borrowing a pane hue. Set from the live theme via
    /// [`Palette::with_alert`].
    alert: Rgb,
}

/// zellij's default error red — `EightBit(124)`, the value `exit_code_error`
/// falls back to when a theme leaves it unset. The close glyph's stand-in red
/// until [`Palette::with_alert`] supplies the live theme value.
pub(crate) const DEFAULT_ALERT: Rgb = from_eightbit(124);

impl Default for Palette {
    fn default() -> Self {
        Self::fallback()
    }
}

impl Palette {
    /// Build a palette from theme colors. Drops the [`BLACK`] sentinel that
    /// unset theme colors collapse to, so it never cycles in as an invisible
    /// pane fill (issue #27). If no slot survives — empty input, or a list that
    /// was entirely the sentinel — falls back to `[accent]` so
    /// [`color_for`](Self::color_for) never divides by zero. (A sentinel
    /// `accent` — an unset `frame_highlight.base` — is replaced with
    /// [`DEFAULT_ACCENT`] so the single-slot fallback stays visible.)
    ///
    /// The hint/glyph text shade is derived from the **post-fallback**
    /// `accent` via [`derived_ring`] — deriving after the sentinel swap means
    /// it always tracks the *visible* accent, never the invisible [`BLACK`]
    /// one. Focus rings are not stored here at all: they are derived per pane
    /// from its own fill (see [`ring_for`](Self::ring_for), issue #47).
    pub fn new(slots: Vec<Rgb>, accent: Rgb) -> Self {
        let accent = if accent == BLACK {
            DEFAULT_ACCENT
        } else {
            accent
        };
        let visible: Vec<Rgb> = slots.into_iter().filter(|&c| c != BLACK).collect();
        let slots = if visible.is_empty() {
            vec![accent]
        } else {
            visible
        };
        let hint = derived_ring(accent);
        Self {
            slots,
            hint,
            accent,
            alert: DEFAULT_ALERT,
        }
    }

    /// Override the alert/close-glyph red with the theme's `exit_code_error.base`
    /// (#86). An unset theme color collapses to the [`BLACK`] sentinel; that is
    /// ignored so the visible [`DEFAULT_ALERT`] survives rather than turning the
    /// close glyph invisible. A builder so the call site reads
    /// `Palette::new(..).with_alert(..)` without widening [`new`](Self::new).
    pub fn with_alert(mut self, alert: Rgb) -> Self {
        self.alert = if alert == BLACK { self.alert } else { alert };
        self
    }

    /// Pre-theme stopgap built from zellij's default style codes, so the
    /// first frames (before any `ModeUpdate` arrives) are already colored
    /// rather than blank. The codes are the default theme's emphasis colors:
    /// orange, cyan, green, magenta, red, white, blue, brown.
    pub fn fallback() -> Self {
        let slots = [166, 51, 154, 201, 124, 255, 45, 215]
            .into_iter()
            .map(from_eightbit)
            .collect();
        Self::new(slots, from_eightbit(166))
    }

    /// Deterministic per-identity color: the same `pane_id` always resolves
    /// to the same slot, cycling once ids exceed the slot count.
    pub fn color_for(&self, pane_id: usize) -> Rgb {
        self.slots[pane_id % self.slots.len()]
    }

    /// Accent-derived shade for degraded-rung hint/glyph text (the `⌘N` badge
    /// and the representative split glyph) — bright on a dark theme, dark on
    /// a light one, by the same luminance rule as the focus ring.
    pub fn hint(&self) -> Rgb {
        self.hint
    }

    /// The theme accent (post-sentinel-fallback, so always visible). It seeds
    /// the [`hint`](Self::hint) shade and survives as the single-slot fallback
    /// fill; the raw value is exposed so tests can pin that it never leaks
    /// into the render as a background (#59).
    pub fn accent(&self) -> Rgb {
        self.accent
    }

    /// The alert/error red used for the close glyph (#86) — the theme's
    /// `exit_code_error.base`, or [`DEFAULT_ALERT`] when the theme left it unset.
    /// Carried through [`dimmed`](Self::dimmed) unchanged so an inactive tab's
    /// close glyph reads as the same red, toned toward its fill at the call site
    /// rather than receded into the canvas like a pane hue.
    pub fn alert(&self) -> Rgb {
        self.alert
    }

    /// The inactive-tab variant of this palette (#59): every slot — and the
    /// hint/accent it seeds text from — receded toward [`CANVAS`] by
    /// [`Self::DIM_PERCENT`], so inactive blocks sink into the bar while the
    /// active tab keeps full vibrancy. A recolor, not a re-keying: slot order
    /// is preserved, so identity→color mapping survives, and rings derive
    /// from the dimmed fills automatically.
    pub fn dimmed(&self) -> Self {
        let receded = |c: Rgb| mixed(c, CANVAS, Self::DIM_PERCENT);
        Self {
            slots: self.slots.iter().map(|&c| receded(c)).collect(),
            hint: receded(self.hint),
            accent: receded(self.accent),
            // The alert red is *not* receded: the close glyph stays the same
            // recognizable red on inactive tabs, toned toward the fill at the
            // render site (`INACTIVE_LABEL_BLEND`) rather than sunk into the
            // canvas here — mirroring how the white badge was a fixed shade.
            alert: self.alert,
        }
    }

    /// Mix fraction toward the canvas for [`Self::dimmed`]. A visual parameter
    /// tuned so inactive tabs read as "not selected" at minimap scale while
    /// their pane hues stay tellable — not a correctness constant.
    const DIM_PERCENT: u8 = 45;

    /// Focus-ring color for a pane: a luminance-shifted shade of the pane's
    /// **own fill** (issue #47), so the outline stays in the pane's hue
    /// family — a blue pane is ringed in a slightly different blue. Never
    /// equal to the fill ([`derived_ring`]).
    pub fn ring_for(&self, pane_id: usize) -> Rgb {
        derived_ring(self.color_for(pane_id))
    }

    /// Blend fraction toward a float's own fill for the ring of an **un**focused
    /// float (#116). The focused float keeps the full [`ring_for`](Self::ring_for)
    /// outline; every other float's ring recedes toward its fill by this much, so
    /// the focused float reads as the prominent one among sibling floats. A
    /// visual-tuning parameter, not a correctness constant.
    const UNFOCUSED_FLOAT_RING_BLEND: u8 = 50;

    /// Boundary-ring color for a visible float (#110, #116). Every float carries a
    /// ring so it reads as floating above the tiles; the focused float keeps its
    /// full [`ring_for`](Self::ring_for) shade while an unfocused float weakens
    /// toward its own [`color_for`](Self::color_for) fill by
    /// `UNFOCUSED_FLOAT_RING_BLEND`, so only the focused float carries the strong
    /// outline. The interior fill is unaffected.
    pub fn float_ring_for(&self, pane_id: usize, focused: bool) -> Rgb {
        match focused {
            true => self.ring_for(pane_id),
            false => mixed(
                self.ring_for(pane_id),
                self.color_for(pane_id),
                Self::UNFOCUSED_FLOAT_RING_BLEND,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type R = Result<(), Box<dyn std::error::Error>>;

    fn palette() -> Palette {
        Palette::new(
            vec![(10, 20, 30), (40, 50, 60), (70, 80, 90)],
            (200, 100, 50),
        )
    }

    #[test]
    fn same_id_same_color() -> R {
        let p = palette();
        assert_eq!(p.color_for(1), p.color_for(1));
        assert_eq!(p.color_for(7), p.color_for(7));
        Ok(())
    }

    #[test]
    fn distinct_ids_distinct_colors_when_slots_differ() -> R {
        let p = palette();
        assert_ne!(p.color_for(0), p.color_for(1));
        assert_ne!(p.color_for(1), p.color_for(2));
        Ok(())
    }

    #[test]
    fn color_cycles_by_modulo() -> R {
        let p = palette();
        let len = 3;
        assert_eq!(p.color_for(0), p.color_for(len));
        assert_eq!(p.color_for(1), p.color_for(len + 1));
        assert_eq!(p.color_for(2), p.color_for(2 * len + 2));
        Ok(())
    }

    #[test]
    fn empty_slots_fall_back_to_accent() -> R {
        let p = Palette::new(vec![], (1, 2, 3));
        // No division by zero, and every id resolves to the accent.
        assert_eq!(p.color_for(0), (1, 2, 3));
        assert_eq!(p.color_for(99), (1, 2, 3));
        Ok(())
    }

    #[test]
    fn black_sentinel_slots_are_dropped() -> R {
        // Unset theme colors collapse to (0, 0, 0); that sentinel must never
        // become a pane fill, or the pane is invisible on the dark canvas.
        let p = Palette::new(
            vec![(0, 0, 0), (10, 20, 30), (0, 0, 0), (40, 50, 60)],
            (200, 100, 50),
        );
        // Only the two visible colors remain, cycled in order.
        assert_eq!(p.color_for(0), (10, 20, 30));
        assert_eq!(p.color_for(1), (40, 50, 60));
        assert_eq!(p.color_for(2), (10, 20, 30));
        // No id ever resolves to the black sentinel.
        assert!((0..32).all(|id| p.color_for(id) != (0, 0, 0)));
        Ok(())
    }

    #[test]
    fn all_black_slots_fall_back_to_accent() -> R {
        // A theme that leaves every emphasis color unset yields an all-black
        // slot list; after dropping the sentinels the modulo guard kicks in.
        let p = Palette::new(vec![(0, 0, 0), (0, 0, 0)], (1, 2, 3));
        assert_eq!(p.color_for(0), (1, 2, 3));
        assert_eq!(p.color_for(99), (1, 2, 3));
        Ok(())
    }

    #[test]
    fn sentinel_accent_falls_back_to_default() -> R {
        // An unset frame_highlight.base collapses accent to (0,0,0); the hint
        // shade must seed from the visible DEFAULT_ACCENT, not black.
        let p = Palette::new(vec![(10, 20, 30)], (0, 0, 0));
        assert_eq!(p.hint(), derived_ring(DEFAULT_ACCENT));
        assert_ne!(p.hint(), derived_ring(BLACK));
        Ok(())
    }

    #[test]
    fn all_black_slots_and_accent_fall_back_to_visible_default() -> R {
        // Worst case: every emphasis color AND frame_highlight.base unset. The
        // slot cycle drops to [accent], and accent is the visible default — so
        // color_for never yields black.
        let p = Palette::new(vec![(0, 0, 0), (0, 0, 0)], (0, 0, 0));
        assert_eq!(p.color_for(0), DEFAULT_ACCENT);
        assert_ne!(p.color_for(0), (0, 0, 0));
        Ok(())
    }

    #[test]
    fn focus_always_carries_a_ring_even_if_fill_collides() -> R {
        // The ring shade is never equal to the fill it derives from, so even
        // a one-slot palette keeps a focused pane distinguishable from a
        // same-colored neighbor.
        let collide = Palette::new(vec![(9, 9, 9)], (9, 9, 9));
        assert_ne!(collide.ring_for(0), collide.color_for(0));
        Ok(())
    }

    #[test]
    fn ring_derives_from_the_panes_own_fill() -> R {
        // The headline of the per-pane ring model: each pane's ring is the
        // luminance-shifted shade of its OWN fill, so different panes get
        // different rings (issue #47 follow-up).
        let p = palette();
        assert_eq!(p.ring_for(0), derived_ring(p.color_for(0)));
        assert_eq!(p.ring_for(1), derived_ring(p.color_for(1)));
        assert_ne!(p.ring_for(0), p.ring_for(1));
        Ok(())
    }

    #[test]
    fn dark_fill_derives_a_lighter_ring() -> R {
        // A dark focused fill gets a ring shifted toward white, so the outline
        // reads brighter than the pane.
        let p = Palette::new(vec![(10, 20, 30)], (200, 100, 50));
        assert!(luma(p.ring_for(0)) > luma((10, 20, 30)));
        Ok(())
    }

    #[test]
    fn light_fill_derives_a_darker_ring() -> R {
        // A light focused fill gets a ring shifted toward black, so the outline
        // reads darker than the pane.
        let p = Palette::new(vec![(220, 210, 200)], (200, 100, 50));
        assert!(luma(p.ring_for(0)) < luma((220, 210, 200)));
        Ok(())
    }

    #[test]
    fn float_ring_weakens_only_when_unfocused() -> R {
        // A visible float always carries a boundary ring so it reads as floating
        // above the tiles (#110). #116 keeps the FOCUSED float's ring at full
        // strength and weakens every other float's ring toward its own fill, so
        // the focused float stands out among its sibling floats.
        let p = palette();
        // Focused: unchanged — exactly the shade `ring_for` gives.
        assert_eq!(p.float_ring_for(1, true), p.ring_for(1));
        // Unfocused: blended toward the float's own fill — strictly weaker than
        // the full ring, but never collapsed into the bare fill.
        let weak = p.float_ring_for(1, false);
        assert_ne!(weak, p.ring_for(1), "unfocused ring must be weakened");
        assert_ne!(weak, p.color_for(1), "but not collapsed into the fill");
        assert_eq!(
            weak,
            mixed(
                p.ring_for(1),
                p.color_for(1),
                Palette::UNFOCUSED_FLOAT_RING_BLEND
            )
        );
        Ok(())
    }

    #[test]
    fn gradient_endpoints_are_base_fill_and_stop() -> R {
        // t=0 must be the untouched base fill; t=100 must be lighter than the
        // base for a dark fill (the sweep lightens darks).
        let fill = (10, 20, 30);
        assert_eq!(gradient_at(fill, 0), fill);
        assert!(luma(gradient_at(fill, 100)) > luma(fill));
        Ok(())
    }

    #[test]
    fn gradient_darkens_a_light_fill() -> R {
        let fill = (220, 210, 200);
        assert!(luma(gradient_at(fill, 100)) < luma(fill));
        Ok(())
    }

    #[test]
    fn gradient_channels_advance_monotonically() -> R {
        // Each channel must move toward the stop without ever stepping back —
        // a reversal would read as a visible band in the sweep.
        let fill = (10, 20, 30);
        let steps: Vec<Rgb> = (0..=100).map(|t| gradient_at(fill, t)).collect();
        assert!(
            steps
                .windows(2)
                .all(|w| { w[0].0 <= w[1].0 && w[0].1 <= w[1].1 && w[0].2 <= w[1].2 })
        );
        Ok(())
    }

    #[test]
    fn gradient_mixes_in_linear_light_not_srgb() -> R {
        // The headline of #46: the midpoint must sit at the linear-light
        // halfway, which for a dark fill is brighter than the raw-sRGB
        // average (sRGB encoding lifts dark linear values).
        let fill = (10, 20, 30);
        let stop = gradient_at(fill, 100);
        let srgb_mid = mixed(fill, stop, 50);
        let mid = gradient_at(fill, 50);
        assert!(
            luma(mid) > luma(srgb_mid),
            "linear-light midpoint {mid:?} must be brighter than sRGB midpoint {srgb_mid:?}"
        );
        Ok(())
    }

    #[test]
    fn linear_channel_codec_roundtrips_every_byte() -> R {
        // decode→encode must be the identity on all 256 channel values, or a
        // zero-width sweep would repaint the base fill.
        assert!((0..=255).all(|c| linear_to_channel(channel_to_linear(c)) == c));
        Ok(())
    }

    #[test]
    fn from_eightbit_cube_corners() -> R {
        // 16 is the cube origin (black); 231 is the cube apex (white).
        assert_eq!(from_eightbit(16), (0, 0, 0));
        assert_eq!(from_eightbit(231), (255, 255, 255));
        // Named cube colors used by the default theme.
        assert_eq!(from_eightbit(51), (0, 255, 255)); // cyan
        assert_eq!(from_eightbit(166), (215, 95, 0)); // orange
        assert_eq!(from_eightbit(201), (255, 0, 255)); // magenta
        Ok(())
    }

    #[test]
    fn from_eightbit_grayscale_ramp() -> R {
        // 232 is the darkest gray (8), 255 the lightest (238).
        assert_eq!(from_eightbit(232), (8, 8, 8));
        assert_eq!(from_eightbit(255), (238, 238, 238));
        assert_eq!(from_eightbit(238), (68, 68, 68));
        Ok(())
    }

    #[test]
    fn from_eightbit_system_colors() -> R {
        assert_eq!(from_eightbit(0), (0, 0, 0));
        assert_eq!(from_eightbit(15), (255, 255, 255));
        Ok(())
    }

    /// `value` lies on the closed per-channel segment between `from` and `to`.
    fn channel_between(from: Rgb, to: Rgb, value: Rgb) -> bool {
        let on = |a: u8, b: u8, v: u8| (a.min(b)..=a.max(b)).contains(&v);
        on(from.0, to.0, value.0) && on(from.1, to.1, value.1) && on(from.2, to.2, value.2)
    }

    #[test]
    fn accent_is_the_post_fallback_accent() -> R {
        // The raw (post-sentinel-swap) accent is exposed for the active tab's
        // badge cue (#59): a real accent comes back as-is, a sentinel one as
        // the visible default — mirroring the hint's seeding rule.
        let p = Palette::new(vec![(10, 20, 30)], (200, 100, 50));
        assert_eq!(p.accent(), (200, 100, 50));
        let q = Palette::new(vec![(10, 20, 30)], (0, 0, 0));
        assert_eq!(q.accent(), DEFAULT_ACCENT);
        Ok(())
    }

    #[test]
    fn dimmed_moves_every_slot_toward_the_canvas() -> R {
        // The inactive-tab treatment (#59): every slot recedes toward the
        // canvas color — never reaching it (the block must stay visible),
        // never moving away from it.
        let p = palette();
        let d = p.dimmed();
        for id in 0..3 {
            let orig = p.color_for(id);
            let dim = d.color_for(id);
            assert_ne!(dim, orig, "slot {id}: dimming must change the fill");
            assert_ne!(dim, CANVAS, "slot {id}: a dimmed fill must stay visible");
            assert!(
                channel_between(orig, CANVAS, dim),
                "slot {id}: {dim:?} must lie between {orig:?} and {CANVAS:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn dimmed_preserves_identity_keying() -> R {
        // Dimming is a recolor, not a re-keying: the same pane id must land on
        // the same (dimmed) slot, and the modulo cycle must be untouched.
        let p = palette();
        let d = p.dimmed();
        assert_eq!(d.color_for(0), d.color_for(3));
        assert_eq!(d.color_for(1), d.color_for(4));
        assert_ne!(d.color_for(0), d.color_for(1));
        Ok(())
    }

    #[test]
    fn dimmed_hint_recedes_with_the_slots() -> R {
        // The L3 glyph / L4 hint text must dim with its block, or a narrow
        // inactive tab would keep a full-vibrancy hint while its neighbors
        // recede.
        let p = palette();
        let d = p.dimmed();
        assert_ne!(d.hint(), p.hint());
        assert!(channel_between(p.hint(), CANVAS, d.hint()));
        Ok(())
    }

    #[test]
    fn dimmed_ring_derives_from_the_dimmed_fill() -> R {
        // Rings are derived per pane from the palette's own fill, so the
        // dimmed palette's ring must track the dimmed slot — not the
        // original one.
        let p = palette();
        let d = p.dimmed();
        assert_eq!(d.ring_for(0), derived_ring(d.color_for(0)));
        assert_ne!(d.ring_for(0), p.ring_for(0));
        Ok(())
    }

    #[test]
    fn gradient_sweeps_a_light_fill_toward_black() -> R {
        // The sweep direction follows the same rule as `derived_ring`: a light
        // fill sweeps darker (toward black), so a light-theme pane never blows
        // out to white at the sweep's far end. Every channel must move down,
        // and the sweep's base stays the fill itself.
        let light = (240, 230, 220);
        assert_eq!(gradient_at(light, 0), light);
        let far = gradient_at(light, 100);
        assert!(far.0 < light.0 && far.1 < light.1 && far.2 < light.2);
        Ok(())
    }

    #[test]
    fn contrast_target_follows_luma_polarity() -> R {
        // The shared direction rule: a dark base sweeps toward white, a light
        // one toward black — what keeps ring / sweep / button shades legible on
        // either theme polarity.
        assert_eq!(contrast_target((10, 20, 30)), (255, 255, 255));
        assert_eq!(contrast_target((230, 220, 210)), (0, 0, 0));
        Ok(())
    }

    #[test]
    fn button_fill_is_a_quiet_lift_off_the_canvas() -> R {
        // The fill must read as part of the strip: distinct from the canvas but
        // only slightly — closer to the canvas than the glyph foreground, and
        // lifted toward the contrast extreme (lighter, since the canvas is dark).
        let fill = button_fill();
        assert_ne!(fill, CANVAS, "the fill must be visible against the canvas");
        assert!(
            channel_between(CANVAS, button_glyph(), fill),
            "the fill sits between the canvas and the glyph: {fill:?}"
        );
        assert!(luma(fill) > luma(CANVAS), "a dark canvas lifts lighter");
        Ok(())
    }

    #[test]
    fn button_glyph_reads_clearly_on_the_fill() -> R {
        // The "+" must stand off the muted fill: further from the canvas than
        // the fill (brighter here), so it is legible without pulling focus.
        let glyph = button_glyph();
        let fill = button_fill();
        assert!(luma(glyph) > luma(fill), "glyph brighter than its fill");
        assert_ne!(glyph, fill);
        Ok(())
    }
}

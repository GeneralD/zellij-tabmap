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

/// Convert an xterm-256 palette index to RGB.
///
/// Mirrors zellij's own `eightbit_to_rgb`, which is not re-exported to
/// plugins (`zellij_tile::prelude` surfaces `zellij_utils::data`, not its
/// `shared` module). The 16 system colors use the common xterm defaults,
/// 16–231 the 6×6×6 color cube, and 232–255 the 24-step grayscale ramp.
/// Every color the default theme actually uses lands in the cube/grayscale
/// ranges, where this table is exact; the 0–15 system colors are
/// terminal-dependent and reproduced here at their conventional values.
pub fn from_eightbit(c: u8) -> Rgb {
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
fn mixed(from: Rgb, to: Rgb, percent: u8) -> Rgb {
    let lerp = |a: u8, b: u8| (a as i32 + (b as i32 - a as i32) * percent as i32 / 100) as u8;
    (lerp(from.0, to.0), lerp(from.1, to.1), lerp(from.2, to.2))
}

/// Luminance-aware focus-ring shade of `fill`: lighten a dark fill toward white,
/// darken a light one toward black, by a fixed mix. The ring stays in the same
/// hue family as the pane it surrounds (cohesive) while reading as a distinct
/// outline — it is never equal to `fill`. This is the default ring when a layout
/// does not pin one explicitly (issue #32).
fn derived_ring(fill: Rgb) -> Rgb {
    /// Mix fraction toward white/black. A visual parameter tuned for a ring that
    /// reads as an outline at minimap scale, not a correctness constant.
    const SHIFT_PERCENT: u8 = 18;
    let target = if luma(fill) < 128 {
        (255, 255, 255)
    } else {
        (0, 0, 0)
    };
    mixed(fill, target, SHIFT_PERCENT)
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
    const SWEEP_PERCENT: f32 = 0.35;
    let target = if luma(fill) < 128 {
        (255, 255, 255)
    } else {
        (0, 0, 0)
    };
    // Composing the two lerps in linear space collapses to a single mix:
    // fill→stop by t equals fill→target by t·SWEEP_PERCENT.
    mixed_linear(fill, target, SWEEP_PERCENT * percent as f32 / 100.0)
}

/// Resolved drawing attributes for one pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneStyle {
    /// Block fill color.
    pub fill: Rgb,
    /// Focus-outline color — `Some` only for the focused pane.
    pub ring: Option<Rgb>,
    /// Whether the pane's label should render emphasized (bold).
    pub emphasized: bool,
}

/// A theme-derived color assignment for panes.
///
/// `slots` is cycled by pane id, so identity maps to a stable color across
/// repaints — focus does **not** repaint a pane's fill (issue #47): the pane
/// keeps its identity hue and the ring outline (plus the emphasized label)
/// is what marks focus. The ring is derived **from the pane's own fill** as a
/// luminance-shifted shade ([`derived_ring`]), so a blue pane gets a
/// slightly-different-blue outline rather than a theme-wide accent ring. The
/// accent survives only as the single-slot fallback fill and the seed of the
/// [`hint`](Self::hint) text shade.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    slots: Vec<Rgb>,
    hint: Rgb,
}

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
        Self { slots, hint }
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

    /// Focus-ring color for a pane: a luminance-shifted shade of the pane's
    /// **own fill** (issue #47), so the outline stays in the pane's hue
    /// family — a blue pane is ringed in a slightly different blue. Never
    /// equal to the fill ([`derived_ring`]).
    pub fn ring_for(&self, pane_id: usize) -> Rgb {
        derived_ring(self.color_for(pane_id))
    }

    /// Drawing attributes for a pane, keyed on its stable `pane_id`. The fill
    /// is the pane's identity hue regardless of focus — focus is marked by the
    /// fill-derived ring outline and the emphasized label only (issue #47).
    pub fn style_for(&self, pane_id: usize, focused: bool) -> PaneStyle {
        PaneStyle {
            fill: self.color_for(pane_id),
            ring: focused.then(|| self.ring_for(pane_id)),
            emphasized: focused,
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
    fn focus_keeps_slot_fill_with_ring_and_emphasis() -> R {
        // Focus must not repaint the pane: identity hue stays, the ring and
        // the emphasized label are what mark focus (issue #47).
        let p = palette();
        let focused = p.style_for(0, true);
        assert_eq!(focused.fill, p.color_for(0));
        assert_eq!(focused.ring, Some(p.ring_for(0)));
        assert!(focused.emphasized);
        Ok(())
    }

    #[test]
    fn unfocused_uses_slot_color_no_ring_no_emphasis() -> R {
        let p = palette();
        let style = p.style_for(1, false);
        assert_eq!(style.fill, p.color_for(1));
        assert_eq!(style.ring, None);
        assert!(!style.emphasized);
        Ok(())
    }

    #[test]
    fn focus_is_marked_by_ring_not_fill() -> R {
        let p = palette();
        // The headline guarantee: focus never changes the fill — the ring is
        // the only structural difference between the two styles (issue #47).
        assert_eq!(p.style_for(0, true).fill, p.style_for(0, false).fill);
        assert!(p.style_for(0, true).ring.is_some());
        assert!(p.style_for(0, false).ring.is_none());
        Ok(())
    }

    #[test]
    fn focus_always_carries_a_ring_even_if_fill_collides() -> R {
        // The ring shade is never equal to the fill it derives from, so even
        // a one-slot palette keeps a focused pane distinguishable from a
        // same-colored neighbor.
        let collide = Palette::new(vec![(9, 9, 9)], (9, 9, 9));
        let focused = collide.style_for(0, true);
        let neighbor = collide.style_for(0, false);
        assert_eq!(focused.fill, neighbor.fill);
        assert!(focused.ring.is_some());
        assert_ne!(focused.ring, Some(focused.fill));
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
}

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
/// repaints. `accent` and `ring` mark the focused pane. The always-on ring
/// is the real focus disambiguator: some themes make `accent` coincide with
/// a slot color (the default theme's `frame_highlight.base` equals a
/// `text_unselected` emphasis color), so the focused fill alone can match a
/// neighbor — the ring outline is what keeps focus unambiguous. By default
/// `ring` is a luminance-shifted shade of `accent` ([`derived_ring`]), so it
/// always reads as an outline of the focused fill; a layout may pin it
/// explicitly instead (issue #32).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    slots: Vec<Rgb>,
    accent: Rgb,
    ring: Rgb,
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
    /// [`DEFAULT_ACCENT`] so the focused fill and the single-slot fallback
    /// stay visible.)
    ///
    /// `ring` is the focus-outline color: `Some` pins it explicitly (a layout
    /// override), `None` derives it from the **post-fallback** `accent` via
    /// [`derived_ring`] — a luminance-shifted shade of the focused fill. Deriving
    /// after the sentinel swap means the ring always tracks the *visible* accent,
    /// never the invisible [`BLACK`] one.
    pub fn new(slots: Vec<Rgb>, accent: Rgb, ring: Option<Rgb>) -> Self {
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
        let ring = ring.unwrap_or_else(|| derived_ring(accent));
        Self {
            slots,
            accent,
            ring,
        }
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
        Self::new(slots, from_eightbit(166), None)
    }

    /// Deterministic per-identity color: the same `pane_id` always resolves
    /// to the same slot, cycling once ids exceed the slot count.
    pub fn color_for(&self, pane_id: usize) -> Rgb {
        self.slots[pane_id % self.slots.len()]
    }

    /// The focus-ring color, drawn on the focused pane's outline pixels.
    pub fn ring(&self) -> Rgb {
        self.ring
    }

    /// Drawing attributes for a pane, keyed on its stable `pane_id`.
    pub fn style_for(&self, pane_id: usize, focused: bool) -> PaneStyle {
        if focused {
            return PaneStyle {
                fill: self.accent,
                ring: Some(self.ring),
                emphasized: true,
            };
        }
        PaneStyle {
            fill: self.color_for(pane_id),
            ring: None,
            emphasized: false,
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
            None,
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
        let p = Palette::new(vec![], (1, 2, 3), None);
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
            None,
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
        let p = Palette::new(vec![(0, 0, 0), (0, 0, 0)], (1, 2, 3), None);
        assert_eq!(p.color_for(0), (1, 2, 3));
        assert_eq!(p.color_for(99), (1, 2, 3));
        Ok(())
    }

    #[test]
    fn sentinel_accent_falls_back_to_default() -> R {
        // An unset frame_highlight.base collapses accent to (0,0,0); the focused
        // fill must stay visible, so accent is replaced with DEFAULT_ACCENT.
        let p = Palette::new(vec![(10, 20, 30)], (0, 0, 0), None);
        assert_eq!(p.style_for(0, true).fill, DEFAULT_ACCENT);
        assert_ne!(p.style_for(0, true).fill, (0, 0, 0));
        Ok(())
    }

    #[test]
    fn all_black_slots_and_accent_fall_back_to_visible_default() -> R {
        // Worst case: every emphasis color AND frame_highlight.base unset. The
        // slot cycle drops to [accent], and accent is the visible default — so
        // color_for never yields black.
        let p = Palette::new(vec![(0, 0, 0), (0, 0, 0)], (0, 0, 0), None);
        assert_eq!(p.color_for(0), DEFAULT_ACCENT);
        assert_ne!(p.color_for(0), (0, 0, 0));
        Ok(())
    }

    #[test]
    fn focus_uses_accent_and_ring_and_emphasis() -> R {
        let p = palette();
        let focused = p.style_for(0, true);
        assert_eq!(focused.fill, (200, 100, 50));
        assert_eq!(focused.ring, Some(p.ring()));
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
    fn focused_and_unfocused_styles_differ() -> R {
        let p = palette();
        // The headline guarantee: a focused pane is visually distinct from
        // the same-id unfocused pane (different fill, and a ring is present).
        assert_ne!(p.style_for(0, true).fill, p.style_for(0, false).fill);
        assert!(p.style_for(0, true).ring.is_some());
        assert!(p.style_for(0, false).ring.is_none());
        Ok(())
    }

    #[test]
    fn focus_always_carries_a_ring_even_if_fill_collides() -> R {
        // Safety net for themes where `accent` equals a slot color: the ring
        // is what keeps a focused pane distinguishable from a same-colored
        // neighbor, since the fill alone would not.
        let collide = Palette::new(vec![(9, 9, 9)], (9, 9, 9), None);
        let focused = collide.style_for(0, true);
        let neighbor = collide.style_for(0, false);
        assert_eq!(focused.fill, neighbor.fill);
        assert!(focused.ring.is_some());
        Ok(())
    }

    #[test]
    fn ring_is_distinct_from_accent_fill() -> R {
        // The derived ring must never equal the focused fill, or the outline
        // would vanish into the pane it surrounds.
        let p = palette();
        assert_ne!(p.ring(), p.style_for(0, true).fill);
        Ok(())
    }

    #[test]
    fn dark_accent_derives_a_lighter_ring() -> R {
        // A dark focused fill gets a ring shifted toward white, so the outline
        // reads brighter than the pane.
        let p = Palette::new(vec![(10, 20, 30)], (20, 30, 40), None);
        assert!(luma(p.ring()) > luma((20, 30, 40)));
        Ok(())
    }

    #[test]
    fn light_accent_derives_a_darker_ring() -> R {
        // A light focused fill gets a ring shifted toward black, so the outline
        // reads darker than the pane.
        let p = Palette::new(vec![(10, 20, 30)], (220, 210, 200), None);
        assert!(luma(p.ring()) < luma((220, 210, 200)));
        Ok(())
    }

    #[test]
    fn explicit_ring_overrides_derivation() -> R {
        // A layout-pinned ring is used verbatim, bypassing the luminance shift.
        let p = Palette::new(vec![(10, 20, 30)], (20, 30, 40), Some((1, 2, 3)));
        assert_eq!(p.ring(), (1, 2, 3));
        assert_eq!(p.style_for(0, true).ring, Some((1, 2, 3)));
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

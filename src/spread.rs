//! Adjacency-aware palette spreading for the minimap's pane fills (#111).
//!
//! The default color scheme keys a pane's fill on its **stable identity**
//! (`Palette::color_for(pane_id)`, issue #5), which lets two panes that happen
//! to sit side by side collide on the same hue whenever their ids are congruent
//! mod the slot count — the split between them disappears. This module computes
//! an alternative per-tab assignment that keeps a pane on its identity slot
//! whenever it can, and moves it to the nearest free slot only when a
//! **geometrically adjacent** pane already holds that hue.
//!
//! The trade this makes (settled in #111): within an opted-in tab, a pane's
//! color becomes a function of its neighbors, so it can shift when a sibling
//! opens, closes, or moves. The default `stable` strategy is untouched — this
//! runs only behind the `color_strategy "distinct"` config opt-in.

use crate::color::Rgb;
use crate::minimap::PaneRect;

/// How a tab's pane fills are keyed to palette slots (`color_strategy`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorStrategy {
    /// A pane's fill keys on its stable id (`color_for(id)`, issue #5): the
    /// color never changes as siblings open, close, or move. The default —
    /// adjacent panes with congruent ids can collide on one hue.
    #[default]
    Stable,
    /// Adjacency-aware spreading (#111): panes keep their identity slot when
    /// they can, but a pane whose edge-neighbor already holds that hue moves
    /// to the nearest free slot. Within an opted-in tab a pane's color can
    /// therefore shift when its neighborhood changes.
    Distinct,
}

impl std::str::FromStr for ColorStrategy {
    type Err = ();

    /// `"stable"` / `"distinct"` (exact match); any other value errors so the
    /// config parser falls back to the documented default rather than panicking.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "stable" => Ok(Self::Stable),
            "distinct" => Ok(Self::Distinct),
            _ => Err(()),
        }
    }
}

/// Per-pane color keys for `panes`, parallel to the input slice.
///
/// With `distinct` off (the `stable` strategy), every pane keys on its own
/// `id` — byte-for-byte the historical `color_for(id)` behavior.
///
/// With `distinct` on, panes are walked in reading order (top→bottom, then
/// left→right — the same `(y, x)` order as
/// [`crate::projection::pane_ids_in_reading_order`]) and each is assigned a
/// palette slot index (`0..slot_colors.len()`, so the key feeds
/// [`crate::color::Palette::color_for`] unchanged):
///
/// - **Identity anchor:** the walk starts each pane at its stable slot
///   (`id % slot_colors.len()`); a pane with no colliding neighbor keeps
///   exactly the color the `stable` strategy would give it.
/// - **De-collision:** collisions are judged by **rendered color**, not slot
///   index — a theme may repeat one RGB across slots, and two slots sharing a
///   hue are the same collision. When an already-assigned pane sharing an
///   **edge** (positive-length side contact — corner touch does not count)
///   holds the preferred hue, the probe advances (`preferred + 1, + 2, …`,
///   wrapping) to the first slot whose color no assigned neighbor uses.
/// - **Saturation fallback:** when neighbors cover every hue, a repeat is
///   unavoidable without recoloring an already-placed pane — deliberately out
///   of scope, so a conflict-free pane always keeps its stable hue (the #111
///   identity anchor). The pane takes the least recently assigned slot (ties
///   → the lower slot index) so forced repeats spread across the palette
///   instead of clustering.
///
/// The result is deterministic for a given pane set: it depends only on the
/// panes' ids and geometry, never on the input slice order. An empty
/// `slot_colors` (never produced by [`crate::color::Palette`], which floors
/// at one slot) degrades to the stable identity keys rather than dividing by
/// zero.
pub fn color_keys(panes: &[PaneRect], slot_colors: &[Rgb], distinct: bool) -> Vec<usize> {
    let slots = slot_colors.len();
    if !distinct || slots == 0 {
        return panes.iter().map(|p| p.id).collect();
    }
    // Reading-order walk over slice *indices*, so the returned keys stay
    // parallel to the input. The id tiebreaker keeps the order total even for
    // degenerate same-origin rects (a real tiled layout never produces them).
    let mut order: Vec<usize> = (0..panes.len()).collect();
    order.sort_by_key(|&i| (panes[i].y, panes[i].x, panes[i].id));
    let mut keys = vec![0usize; panes.len()];
    let mut assigned = vec![false; panes.len()];
    // Walk step at which each slot was most recently taken — the recency the
    // saturation fallback minimizes so forced repeats spread out.
    let mut last_used: Vec<Option<usize>> = vec![None; slots];
    for (step, &i) in order.iter().enumerate() {
        // Hues (not slot indices) held by already-assigned edge-neighbors: a
        // theme may repeat one RGB across slots, and two indices sharing a
        // hue are the same collision.
        let taken: Vec<Rgb> = panes
            .iter()
            .enumerate()
            .filter(|&(j, pane)| assigned[j] && shares_edge(&panes[i], pane))
            .map(|(j, _)| slot_colors[keys[j]])
            .collect();
        let preferred = panes[i].id % slots;
        let slot = (0..slots)
            .map(|k| (preferred + k) % slots)
            .find(|&s| !taken.contains(&slot_colors[s]))
            .unwrap_or_else(|| {
                // Every slot is held by a neighbor: a repeat is unavoidable.
                // Take the least recently assigned slot (ties → lower index).
                // `min_by_key` is `None` only for `slots == 0`, excluded by
                // the early return; `preferred` is a panic-free stand-in.
                (0..slots)
                    .min_by_key(|&s| (last_used[s], s))
                    .unwrap_or(preferred)
            });
        keys[i] = slot;
        assigned[i] = true;
        last_used[slot] = Some(step);
    }
    keys
}

/// Whether two panes share a positive-length edge — side contact, not a mere
/// corner touch. Tiled zellij panes butt exactly (`a.x + a.w == b.x` for a
/// vertical seam, verified in the #9 geometry notes), so touch is equality,
/// never a gap-tolerant range.
fn shares_edge(a: &PaneRect, b: &PaneRect) -> bool {
    let overlap = |p0: u32, l0: u32, p1: u32, l1: u32| (p0 + l0).min(p1 + l1) > p0.max(p1);
    let vertical_seam = a.x + a.w == b.x || b.x + b.w == a.x;
    let horizontal_seam = a.y + a.h == b.y || b.y + b.h == a.y;
    (vertical_seam && overlap(a.y, a.h, b.y, b.h))
        || (horizontal_seam && overlap(a.x, a.w, b.x, b.w))
}

#[cfg(test)]
mod tests {
    use super::*;

    type R = Result<(), Box<dyn std::error::Error>>;

    fn pane(id: usize, x: u32, y: u32, w: u32, h: u32) -> PaneRect {
        PaneRect::new(id, x, y, w, h, "", false)
    }

    /// `n` distinct dummy hues, so index-keyed expectations read unchanged.
    fn slots(n: usize) -> Vec<Rgb> {
        (0..n).map(|i| (i as u8, 0, 0)).collect()
    }

    // -- stable strategy (distinct = false) ---------------------------------

    #[test]
    fn stable_returns_ids_verbatim() -> R {
        let panes = [pane(3, 0, 0, 60, 20), pane(11, 60, 0, 60, 20)];
        assert_eq!(color_keys(&panes, &slots(8), false), vec![3, 11]);
        Ok(())
    }

    #[test]
    fn empty_input_yields_empty_keys() -> R {
        assert!(color_keys(&[], &slots(8), true).is_empty());
        Ok(())
    }

    // -- identity anchor ----------------------------------------------------

    #[test]
    fn conflict_free_panes_keep_their_identity_slots() -> R {
        // Ids 0 and 1 land on different slots already — distinct mode must
        // reproduce the stable coloring exactly (slot == id % slots).
        let panes = [pane(0, 0, 0, 60, 20), pane(1, 60, 0, 60, 20)];
        assert_eq!(color_keys(&panes, &slots(8), true), vec![0, 1]);
        Ok(())
    }

    #[test]
    fn adjacent_congruent_ids_get_distinct_slots() -> R {
        // Ids 0 and 8 are both slot 0 under 8 slots — the collision #111
        // exists to kill. The reading-order first pane keeps its identity
        // slot; the second probes forward to the next free slot.
        let panes = [pane(0, 0, 0, 60, 20), pane(8, 60, 0, 60, 20)];
        assert_eq!(color_keys(&panes, &slots(8), true), vec![0, 1]);
        Ok(())
    }

    #[test]
    fn reading_order_decides_who_keeps_the_identity_slot() -> R {
        // Same collision, but the congruent pair is listed bottom-first: the
        // walk is (y, x), so the *top* pane keeps slot 0 regardless of input
        // order, and the bottom one moves.
        let top = pane(8, 0, 0, 120, 10);
        let bottom = pane(0, 0, 10, 120, 10);
        assert_eq!(
            color_keys(&[bottom.clone(), top.clone()], &slots(8), true),
            vec![1, 0]
        );
        assert_eq!(color_keys(&[top, bottom], &slots(8), true), vec![0, 1]);
        Ok(())
    }

    #[test]
    fn non_adjacent_panes_may_share_a_slot() -> R {
        // Congruent ids separated by a middle pane: no shared edge, so both
        // keep their identity slot — the assignment is adjacency-scoped, not
        // a global uniqueness pass.
        let panes = [
            pane(0, 0, 0, 40, 20),
            pane(1, 40, 0, 40, 20),
            pane(8, 80, 0, 40, 20),
        ];
        assert_eq!(color_keys(&panes, &slots(8), true), vec![0, 1, 0]);
        Ok(())
    }

    // -- adjacency definition ------------------------------------------------

    #[test]
    fn vertical_edge_contact_counts_as_adjacent() -> R {
        // Stacked panes share a horizontal edge (top.y + top.h == bottom.y).
        let panes = [pane(0, 0, 0, 120, 10), pane(8, 0, 10, 120, 10)];
        assert_eq!(color_keys(&panes, &slots(8), true), vec![0, 1]);
        Ok(())
    }

    #[test]
    fn corner_touch_is_not_adjacent() -> R {
        // A 2x2 grid's diagonal pair meets only at the corner point — no
        // positive-length shared edge, so congruent diagonal ids may match.
        // (Their orthogonal neighbors are id 1 / id 2, no collision there.)
        let panes = [
            pane(0, 0, 0, 60, 10),
            pane(1, 60, 0, 60, 10),
            pane(2, 0, 10, 60, 10),
            pane(8, 60, 10, 60, 10),
        ];
        assert_eq!(color_keys(&panes, &slots(8), true), vec![0, 1, 2, 0]);
        Ok(())
    }

    // -- hue equality across slots -------------------------------------------

    #[test]
    fn duplicate_slot_colors_collide_as_one_hue() -> R {
        // A theme may repeat one RGB across slots ([red, red, blue]): slot 1
        // is a different index but the same rendered hue as slot 0, so the
        // probe must skip past it and land on blue — index-distinct is not
        // color-distinct.
        let colors = [(255, 0, 0), (255, 0, 0), (0, 0, 255)];
        let panes = [pane(0, 0, 0, 60, 20), pane(3, 60, 0, 60, 20)];
        assert_eq!(color_keys(&panes, &colors, true), vec![0, 2]);
        Ok(())
    }

    // -- saturation fallback -------------------------------------------------

    #[test]
    fn saturated_neighborhood_falls_back_to_least_recently_assigned() -> R {
        // Two slots, three mutually touching panes: A top-left, B top-right,
        // C spanning the bottom (edge contact with both). C's neighbors cover
        // both slots, so a repeat is forced; the least recently assigned slot
        // is A's slot 0 (assigned before B's slot 1), so C takes 0.
        let panes = [
            pane(0, 0, 0, 60, 10),
            pane(1, 60, 0, 60, 10),
            pane(2, 0, 10, 120, 10),
        ];
        assert_eq!(color_keys(&panes, &slots(2), true), vec![0, 1, 0]);
        Ok(())
    }

    // -- degenerate slot counts ---------------------------------------------

    #[test]
    fn single_slot_palette_assigns_slot_zero_everywhere() -> R {
        // A one-slot palette (the accent-only fallback) cannot distinguish
        // anything; every pane keys 0, mirroring `color_for`'s cycling.
        let panes = [pane(3, 0, 0, 60, 20), pane(4, 60, 0, 60, 20)];
        assert_eq!(color_keys(&panes, &slots(1), true), vec![0, 0]);
        Ok(())
    }

    #[test]
    fn zero_slots_degrades_to_stable_identity_keys() -> R {
        // `Palette` never yields zero slots; defend the division anyway by
        // falling back to the stable keys.
        let panes = [pane(3, 0, 0, 60, 20)];
        assert_eq!(color_keys(&panes, &slots(0), true), vec![3]);
        Ok(())
    }

    // -- determinism ---------------------------------------------------------

    #[test]
    fn input_order_does_not_change_the_assignment() -> R {
        // A 2x2 grid with two colliding pairs (0/8 and 1/9): whatever order
        // the manifest lists the panes in, each id maps to the same key.
        let a = pane(8, 0, 0, 60, 10);
        let b = pane(1, 60, 0, 60, 10);
        let c = pane(9, 0, 10, 60, 10);
        let d = pane(0, 60, 10, 60, 10);
        let forward = [a.clone(), b.clone(), c.clone(), d.clone()];
        let shuffled = [d, b, a, c];
        let key_of = |panes: &[PaneRect]| {
            let keys = color_keys(panes, &slots(8), true);
            let mut by_id: Vec<(usize, usize)> = panes.iter().map(|p| p.id).zip(keys).collect();
            by_id.sort_unstable();
            by_id
        };
        assert_eq!(key_of(&forward), key_of(&shuffled));
        Ok(())
    }
}

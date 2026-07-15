//! Dependency-free suppressed-pane awareness layer (#118): match each suppressed
//! pane to the visible tiled pane that hides it, so the bar can mark that cover
//! pane's corner. No zellij types, so the whole module is unit-tested off-wasm
//! (rule #8), exactly like `floating`/`minimap`/`scroll`.

use crate::minimap::PaneRect;

/// The marker glyph — a small quadrant marker distinct from the hidden-float
/// chip [`crate::floating::CHIP_GLYPH`] (`◲`): a different rotation, and it rides
/// an individual pane's corner rather than the tab block's, so the two
/// "hidden-thing" markers never conflate. Single display column.
pub const SUPPRESSED_MARKER_GLYPH: char = '◳';

/// The tiled pane ids that hide at least one suppressed pane in their slot (#118).
/// A tiled pane `t` **covers** suppressed `s` when `t`'s rect contains `s`'s rect
/// (same slot, or `s` nested inside `t`) — the geometry the P0 spike confirmed for
/// edit-scrollback. Returned in `tiled` order and naturally deduped (each cover is
/// yielded at most once), so the renderer marks a pane once no matter how many
/// panes hide behind it — awareness is presence, not count.
pub fn cover_ids(suppressed: &[PaneRect], tiled: &[PaneRect]) -> Vec<usize> {
    tiled
        .iter()
        .filter(|t| suppressed.iter().any(|s| covers(t, s)))
        .map(|t| t.id)
        .collect()
}

/// Whether tiled rect `t` fully contains suppressed rect `s` (half-open extents).
fn covers(t: &PaneRect, s: &PaneRect) -> bool {
    t.x <= s.x && t.y <= s.y && s.x + s.w <= t.x + t.w && s.y + s.h <= t.y + t.h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(id: usize, x: u32, y: u32, w: u32, h: u32) -> PaneRect {
        PaneRect::new(id, x, y, w, h, "p", false)
    }

    #[test]
    fn marks_the_tiled_pane_that_covers_a_suppressed_one() {
        // A suppressed pane sits in the exact slot of tiled pane 2 (same rect);
        // tiled pane 1 is elsewhere. Only pane 2 is a cover.
        let tiled = [rect(1, 0, 0, 60, 40), rect(2, 60, 0, 60, 40)];
        let suppressed = [rect(9, 60, 0, 60, 40)];
        assert_eq!(cover_ids(&suppressed, &tiled), vec![2]);
    }

    #[test]
    fn marks_each_cover_once_regardless_of_count() {
        // Two suppressed panes both nested inside tiled pane 2 → pane 2 marked once.
        let tiled = [rect(1, 0, 0, 60, 40), rect(2, 60, 0, 60, 40)];
        let suppressed = [rect(9, 60, 0, 60, 40), rect(10, 70, 5, 10, 10)];
        assert_eq!(cover_ids(&suppressed, &tiled), vec![2]);
    }

    #[test]
    fn no_cover_when_geometry_does_not_overlap() {
        let tiled = [rect(1, 0, 0, 60, 40)];
        let suppressed = [rect(9, 200, 200, 10, 10)];
        assert!(cover_ids(&suppressed, &tiled).is_empty());
    }
}

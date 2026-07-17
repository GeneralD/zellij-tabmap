//! Dependency-free floating-pane layer: config mode, hidden-float chips, and
//! (later) the visible-float overlay mapping. No zellij types, so the whole
//! module is unit-tested off-wasm (rule #8), exactly like `minimap`/`scroll`.

use crate::minimap::PaneRect;

/// The floating-pane layer handed to [`crate::minimap::render`] for one tab
/// (#110). Chosen per tab from `TabInfo.are_floating_panes_visible`:
/// - `None` — the tab has no floats, or `floating = off`: draw nothing extra.
/// - `Hidden` — the layer is hidden: draw one corner chip per float id.
/// - `Visible` — the layer is shown: overlay each float's rect on the grid (P3).
/// - `Mixed` — the layer is hidden but some floats are pinned (#119): the
///   pinned ones still overlay, the rest chip.
#[derive(Clone, Copy, Debug)]
pub enum FloatLayer<'a> {
    None,
    Hidden(&'a [usize]),
    Visible(&'a [PaneRect]),
    /// A hidden layer whose pinned floats stay on screen (#119): zellij keeps
    /// pinned floats rendered while the layer is hidden, so the bar overlays
    /// them like a visible layer and chips only the rest.
    Mixed {
        chips: &'a [usize],
        overlay: &'a [PaneRect],
    },
}

/// Per-tab floating data captured at the render site (#110), turned into a
/// borrowed [`FloatLayer`] inside `paint::bar`. Owns the float ids (hidden) or
/// rects (visible) so `lib.rs` can build it once per frame from the manifest,
/// then hand each tab's block a borrowed layer via [`FloatSpec::layer`].
#[derive(Clone, Debug)]
pub enum FloatSpec {
    None,
    Hidden(Vec<usize>),
    Visible(Vec<PaneRect>),
    /// See [`FloatLayer::Mixed`] — the owned per-frame form (#119).
    Mixed {
        chips: Vec<usize>,
        overlay: Vec<PaneRect>,
    },
}

impl FloatSpec {
    /// Borrow this spec as the layer [`crate::minimap::render`] consumes.
    pub fn layer(&self) -> FloatLayer<'_> {
        match self {
            FloatSpec::None => FloatLayer::None,
            FloatSpec::Hidden(ids) => FloatLayer::Hidden(ids),
            FloatSpec::Visible(rects) => FloatLayer::Visible(rects),
            FloatSpec::Mixed { chips, overlay } => FloatLayer::Mixed { chips, overlay },
        }
    }
}

/// How the bar depicts a tab's floating-pane layer (config key `floating`, #110).
///
/// `Hybrid` is the B/A behaviour from the design: a tab whose floating layer is
/// visible overlays each float graphically on its tiled minimap, and a tab whose
/// layer is hidden shows one corner chip per float. `Off` restores the pre-#110
/// look — floating panes are invisible on the bar, exactly as before.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FloatingMode {
    /// Overlay visible floats; chip hidden floats — the default (#110).
    #[default]
    Hybrid,
    /// Draw no floating panes at all — the pre-#110 bar.
    Off,
}

impl std::str::FromStr for FloatingMode {
    type Err = ();

    /// `"hybrid"` / `"off"` (exact match); any other value errors so the config
    /// parser falls back to the documented default rather than panicking.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "hybrid" => Ok(Self::Hybrid),
            "off" => Ok(Self::Off),
            _ => Err(()),
        }
    }
}

/// One drawn chip cell's content (#110). `Float(i)` is a chip individually
/// addressable by index for the `i`-th hidden float (its glyph is colored by
/// that float's id); `PlusK(k)` is the overflow marker standing in for `k`
/// floats that did not fit as individual chips — clicking it still resolves
/// to a float (the first one it folds, #113), just not via an `i` index.
/// Returned by [`chip_cells`] paired with the block-local column it sits in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chip {
    Float(usize),
    PlusK(usize),
}

/// The chip glyph — a small quadrant marker that reads as "a floating pane
/// docked in the corner". Single display column.
pub const CHIP_GLYPH: char = '◲';

/// The overflow marker glyph — stands in for the floats that did not fit as
/// chips. Single display column (a real "+k" needs several cells).
pub const CHIP_MORE_GLYPH: char = '⋯';

/// Lay out `count` hidden-float chips into the bottom-right corner of a
/// `cols`-wide block (#110): one selectable [`Chip::Float`] per float, packed
/// right-to-left in id order, capped so a `+k` overflow marker fits when there
/// are more floats than columns. Returns `(block_local_column, chip)` pairs in
/// ascending column order, or empty when there is nothing to draw / no width.
///
/// Budget: chips never use more than `cols` columns. When `count` fits, all
/// `count` cells are `Float`; otherwise the leftmost shown cell becomes a
/// `PlusK` marker so the total never exceeds the budget and the overflow is
/// *visible*, never silently dropped.
pub fn chip_cells(cols: usize, count: usize) -> Vec<(usize, Chip)> {
    if cols == 0 || count == 0 {
        return Vec::new();
    }
    // Everything fits: all cells are float chips, right-aligned.
    if count <= cols {
        let start = cols - count;
        return (0..count).map(|i| (start + i, Chip::Float(i))).collect();
    }
    // Overflow: show `cols - 1` float chips and a single `+k` marker cell at the
    // left of the reserved run. `k` counts every float the marker stands in for.
    let shown = cols - 1;
    let hidden = count - shown;
    std::iter::once((0usize, Chip::PlusK(hidden)))
        .chain((0..shown).map(|i| (1 + i, Chip::Float(i))))
        .collect()
}

/// The hidden-float index at block-local cell (`col`, `row`) in a
/// `cols`-by-`text_rows` block with `count` floats, or `None` when the cell is
/// not a selectable float chip (#110). Chips ride only the bottom text row
/// (`text_rows - 1`); a `+k` marker cell and every other cell resolve to `None`.
/// Mirrors [`chip_cells`] exactly, so draw and hit-test never disagree.
pub fn chip_index_at_cell(
    cols: usize,
    text_rows: usize,
    count: usize,
    col: usize,
    row: usize,
) -> Option<usize> {
    if text_rows == 0 || row != text_rows - 1 {
        return None;
    }
    chip_cells(cols, count)
        .into_iter()
        .find_map(|(c, chip)| match chip {
            Chip::Float(i) if c == col => Some(i),
            _ => None,
        })
}

/// The overflow count `k` if block-local cell (`col`, `row`) is the `+k`
/// **marker** cell in a `cols`-by-`text_rows` block with `count` floats, else
/// `None`. The marker counterpart of [`chip_index_at_cell`], which resolves
/// only the individually-shown float chips. A click on the marker itself
/// resolves to the FIRST folded float (#113: `count - k`, computed by the
/// router) rather than a no-op — this helper just reports `k` so the router
/// can derive that fold boundary. Mirrors [`chip_cells`].
pub fn chip_marker_k(
    cols: usize,
    text_rows: usize,
    count: usize,
    col: usize,
    row: usize,
) -> Option<usize> {
    if text_rows == 0 || row != text_rows - 1 {
        return None;
    }
    chip_cells(cols, count)
        .into_iter()
        .find_map(|(c, chip)| match chip {
            Chip::PlusK(k) if c == col => Some(k),
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_floating_modes() {
        assert_eq!("hybrid".parse(), Ok(FloatingMode::Hybrid));
        assert_eq!("off".parse(), Ok(FloatingMode::Off));
    }

    #[test]
    fn malformed_floating_mode_errors() {
        // Case-sensitive, exact-match only — the config parser turns the error
        // into the documented default.
        assert_eq!("Hybrid".parse::<FloatingMode>(), Err(()));
        assert_eq!("chips".parse::<FloatingMode>(), Err(()));
        assert_eq!("".parse::<FloatingMode>(), Err(()));
    }

    #[test]
    fn default_is_hybrid() {
        assert_eq!(FloatingMode::default(), FloatingMode::Hybrid);
    }

    #[test]
    fn chip_cells_pack_from_the_right_edge() {
        // 3 floats in a 12-wide block: chips occupy the 3 rightmost cells, one per
        // float, left-to-right in id order (columns 9,10,11 → float indices 0,1,2).
        let cells = chip_cells(12, 3);
        assert_eq!(
            cells,
            vec![
                (9, Chip::Float(0)),
                (10, Chip::Float(1)),
                (11, Chip::Float(2))
            ]
        );
    }

    #[test]
    fn chip_cells_collapse_overflow_to_a_plus_k_marker() {
        // Budget is at most `cols` cells but we cap chips so a `+k` marker fits:
        // with a 4-cell budget and 10 floats, show 3 float chips then a "+7"
        // marker (the marker is NOT individually selectable).
        let cells = chip_cells(4, 10);
        assert_eq!(
            cells
                .iter()
                .filter(|(_, c)| matches!(c, Chip::Float(_)))
                .count(),
            3
        );
        assert!(cells.iter().any(|(_, c)| *c == Chip::PlusK(7)));
        // Never exceeds the block width.
        assert!(cells.iter().all(|(col, _)| *col < 4));
    }

    #[test]
    fn chip_cells_empty_without_floats() {
        assert!(chip_cells(12, 0).is_empty());
        assert!(chip_cells(0, 3).is_empty());
    }

    #[test]
    fn chip_index_at_cell_resolves_only_the_last_row() {
        // A click on a float chip's cell in the LAST text row resolves to that
        // float's index; the same column on another row misses (chips ride only
        // the bottom row). bottom row = text_rows - 1 = 2; chips at cols 9,10,11.
        let (cols, text_rows, count) = (12, 3, 3);
        assert_eq!(chip_index_at_cell(cols, text_rows, count, 10, 2), Some(1));
        assert_eq!(
            chip_index_at_cell(cols, text_rows, count, 10, 1),
            None,
            "not the bottom row"
        );
        assert_eq!(
            chip_index_at_cell(cols, text_rows, count, 3, 2),
            None,
            "left of the chips"
        );
    }

    #[test]
    fn chip_index_at_cell_ignores_the_plus_k_marker() {
        // With overflow, the +k marker cells are not individually selectable.
        let (cols, text_rows, count) = (4, 3, 10);
        let hits: Vec<_> = (0..cols)
            .filter_map(|c| chip_index_at_cell(cols, text_rows, count, c, text_rows - 1))
            .collect();
        assert_eq!(
            hits,
            vec![0, 1, 2],
            "only the 3 shown float chips are selectable"
        );
    }

    #[test]
    fn mixed_spec_borrows_both_chip_ids_and_overlay_rects() {
        // A hidden layer with pinned floats (#119): the pinned ones overlay
        // while the rest chip — one spec carries both halves.
        let spec = FloatSpec::Mixed {
            chips: vec![9],
            overlay: vec![PaneRect::new(7, 30, 12, 60, 18, "f", false)],
        };
        match spec.layer() {
            FloatLayer::Mixed { chips, overlay } => {
                assert_eq!(chips, &[9]);
                assert_eq!(overlay.len(), 1);
                assert_eq!(overlay[0].id, 7);
            }
            _ => assert!(false, "Mixed spec must borrow as Mixed layer"),
        }
    }
}

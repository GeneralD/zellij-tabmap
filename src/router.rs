//! Pure click routing — maps a left-click `(row, column)` against the geometry
//! the last `render` recorded, with no zellij types and no host effects. Every
//! function here is a decision the renderer's own data can answer, so the whole
//! module is unit-tested off-wasm (rule #8); the matching host effects
//! (`focus_terminal_pane`, `switch_tab_to`, `new_tab`, `close_tab_with_index`)
//! stay in `lib.rs`, dispatched past these predicates.

use std::collections::BTreeMap;

use crate::{line, minimap};

/// One visible tab's drawn pane geometry, captured each render so a later click
/// can map (row, column) back to the pane the frame actually drew (#74). Holds
/// exactly what [`minimap::pane_at_cell`] needs that the column-only
/// [`line::TabHit`] does not: the block's start column, drawn width and height,
/// the perspective `vinset`, and the tab's projected panes.
pub(crate) struct TabPaneGeom {
    pub(crate) start: usize,
    pub(crate) width: usize,
    pub(crate) rows: usize,
    pub(crate) vinset: usize,
    pub(crate) panes: Vec<minimap::PaneRect>,
    /// Hidden floating pane ids drawn as corner chips this frame (#110), in the
    /// same order [`crate::floating::chip_cells`] lays them out. Empty when the
    /// tab has no hidden floats (its layer is visible / off / it has none).
    pub(crate) hidden_floats: Vec<usize>,
    /// Visible floating panes overlaid this frame (#110), for the visible-layer
    /// hit-test. Empty when the tab's layer is hidden / off / has no floats.
    pub(crate) visible_floats: Vec<minimap::PaneRect>,
}

/// The stable id of the minimap pane drawn at click (`row`, `column`), or
/// `None` when the click missed a pane — outside every tab, on a tab too
/// narrow to draw a minimap (an L3/L4 rung carries no `tab_panes` entry), or
/// on a block's background/inset cell. `row` is zellij's click line (`isize`,
/// negative when the pointer is above the pane); a negative or out-of-range
/// row resolves to `None`, so the caller falls back to a plain tab-switch.
/// Hit-tests against the exact geometry the last `render` recorded, so it can
/// never focus a pane other than the one drawn under the cursor.
pub(crate) fn pane_at(
    tab_layout: &[line::TabHit],
    tab_panes: &BTreeMap<usize, TabPaneGeom>,
    row: isize,
    column: usize,
) -> Option<usize> {
    let row = usize::try_from(row).ok()?;
    let position = line::position_at_column(tab_layout, column)?;
    let geom = tab_panes.get(&position)?;
    let col = column.checked_sub(geom.start)?;
    minimap::pane_at_cell(&geom.panes, geom.width, geom.rows, geom.vinset, col, row)
}

/// The hidden floating-pane id whose corner chip is at click (`row`, `column`),
/// or `None` when the click missed every chip (#110). Resolves against the exact
/// chip layout [`crate::floating::chip_index_at_cell`] computed for the tab under
/// the cursor, so draw and hit-test never disagree. `row` is zellij's click line
/// (`isize`, negative above the pane); a negative or off-chip cell is a miss.
/// The caller tries this before the tiled [`pane_at`] (float priority, spec §7.1).
pub(crate) fn float_chip_at(
    tab_layout: &[line::TabHit],
    tab_panes: &BTreeMap<usize, TabPaneGeom>,
    row: isize,
    column: usize,
) -> Option<usize> {
    let row = usize::try_from(row).ok()?;
    let position = line::position_at_column(tab_layout, column)?;
    let geom = tab_panes.get(&position)?;
    if geom.hidden_floats.is_empty() {
        return None;
    }
    let col = column.checked_sub(geom.start)?;
    let index = crate::floating::chip_index_at_cell(
        geom.width,
        geom.rows,
        geom.hidden_floats.len(),
        col,
        row,
    )?;
    geom.hidden_floats.get(index).copied()
}

/// The hidden floating-pane id the `+k` overflow marker at click (`row`,
/// `column`) resolves to — the FIRST float folded into the marker (#113), i.e.
/// the one right after the individually-shown chips. `None` when the click is
/// off the marker cell. Focusing it (`should_float_if_hidden`) un-hides the
/// whole layer, so the remaining folded floats become individually clickable
/// overlay boxes — every overflow float is reachable in ≤ 2 clicks. The fold
/// boundary derives from the marker's own `k` (`len - k`), mirroring
/// [`crate::floating::chip_cells`] exactly, so draw and hit-test never
/// disagree. Same per-tab geometry resolution as [`float_chip_at`].
pub(crate) fn chip_marker_target_at(
    tab_layout: &[line::TabHit],
    tab_panes: &BTreeMap<usize, TabPaneGeom>,
    row: isize,
    column: usize,
) -> Option<usize> {
    let row = usize::try_from(row).ok()?;
    let position = line::position_at_column(tab_layout, column)?;
    let geom = tab_panes.get(&position)?;
    if geom.hidden_floats.is_empty() {
        return None;
    }
    let col = column.checked_sub(geom.start)?;
    let k =
        crate::floating::chip_marker_k(geom.width, geom.rows, geom.hidden_floats.len(), col, row)?;
    let index = geom.hidden_floats.len().checked_sub(k)?;
    geom.hidden_floats.get(index).copied()
}

/// The visible floating pane id drawn at click (`row`, `column`) — the overlay
/// counterpart of [`float_chip_at`] (#110). `None` when the tab has no visible
/// floats or the cell is off every float box. Resolves against the same
/// [`minimap::float_pane_at_cell`] mapping `render` painted, so draw and
/// hit-test never disagree. Tried alongside [`float_chip_at`] before the tiled
/// [`pane_at`] (float priority, spec §7.1).
pub(crate) fn float_overlay_at(
    tab_layout: &[line::TabHit],
    tab_panes: &BTreeMap<usize, TabPaneGeom>,
    row: isize,
    column: usize,
) -> Option<usize> {
    let row = usize::try_from(row).ok()?;
    let position = line::position_at_column(tab_layout, column)?;
    let geom = tab_panes.get(&position)?;
    if geom.visible_floats.is_empty() {
        return None;
    }
    let col = column.checked_sub(geom.start)?;
    minimap::float_pane_at_cell(
        &geom.panes,
        &geom.visible_floats,
        geom.width,
        geom.rows,
        geom.vinset,
        col,
        row,
    )
}

/// Whether `column` falls in the "+" button's drawn span — the pure routing
/// decision behind a new-tab click (#76). Tests the last frame's recorded
/// button geometry: `None` (button disabled, didn't fit, or no frame yet) is
/// always a miss. Split from the `new_tab` host effect so the decision is
/// unit-tested without a zellij host.
pub(crate) fn clicked_new_tab_button(
    button_layout: Option<line::ButtonHit>,
    column: usize,
) -> bool {
    button_layout.is_some_and(|hit| hit.contains(column))
}

/// The position of the tab whose close "×" cell is at (`row`, `column`), or
/// `None` when the click missed every close cell (#86). `row` is zellij's
/// click line (`isize`, negative above the pane); a negative row matches no
/// cell. Tests the last frame's recorded `close_layout`, which is empty
/// whenever the close button is disabled or only one tab is open — so this is
/// always a miss then, and the last tab is never closeable. Split from the
/// `close_tab_with_index` host effect so the routing is unit-tested without a
/// zellij host, mirroring [`clicked_new_tab_button`].
pub(crate) fn clicked_close_button(
    close_layout: &[line::CloseHit],
    row: isize,
    column: usize,
) -> Option<usize> {
    let row = usize::try_from(row).ok()?;
    close_layout
        .iter()
        .find(|hit| hit.contains(row, column))
        .map(|hit| hit.position)
}

/// The single action a left click resolves to, against the geometry the last
/// `render` recorded. Carries everything the matching host effect needs: the
/// tab index to close, the pane id to focus, or the 1-based tab target to
/// switch to. [`route_click`] returns exactly one of these; `lib.rs` turns it
/// into the one host call, so this module never reaches a zellij host.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ClickIntent {
    /// Open a new tab (#76) — the "+" button was clicked.
    NewTab,
    /// Close the tab at this 0-based position (#86) — its "×" cell was clicked.
    CloseTab(usize),
    /// Focus this pane id (#74) — a click landed on its drawn minimap cell.
    FocusPane(usize),
    /// Focus this floating pane id (#110) — a click landed on its corner chip
    /// (hidden layer), its overlay (visible layer), or the `+k` overflow
    /// marker (the first folded float, #113). Dispatched with
    /// `should_float_if_hidden = true`, so a hidden float is revealed+focused.
    FocusFloatingPane(usize),
    /// Switch to this 1-based tab target (#8) — a click landed on a tab block
    /// that draws no minimap (or on its background, off any pane). Carries the
    /// `u32` [`line::switch_target_at_column`] yields, ready for `switch_tab_to`.
    SwitchTab(u32),
    /// The click matched no affordance (a gap, trailing padding) — the handler
    /// does nothing.
    NoOp,
}

/// Resolve a left click at (`row`, `column`) to the single [`ClickIntent`] it
/// triggers, trying the affordances in the priority the bar paints them: the
/// "+" button sits on top (#76), then each tab's close "×" cell (#86), then the
/// finer click-to-focus minimap pane (#74), then a plain click-to-switch on the
/// tab's block (#8); a click matching none is [`ClickIntent::NoOp`]. Pure — the
/// caller dispatches the one matching host effect — so the whole routing
/// decision is unit-tested without a zellij host.
pub(crate) fn route_click(
    button_layout: Option<line::ButtonHit>,
    close_layout: &[line::CloseHit],
    tab_layout: &[line::TabHit],
    tab_panes: &BTreeMap<usize, TabPaneGeom>,
    row: isize,
    column: usize,
) -> ClickIntent {
    if clicked_new_tab_button(button_layout, column) {
        return ClickIntent::NewTab;
    }
    if let Some(position) = clicked_close_button(close_layout, row, column) {
        return ClickIntent::CloseTab(position);
    }
    // Floating panes sit on top of the tiled minimap, so a float — a hidden-layer
    // corner chip or a visible-layer overlay — wins over the tiled pane in the
    // same cell (float priority, spec §7.1).
    if let Some(id) = float_chip_at(tab_layout, tab_panes, row, column)
        .or_else(|| float_overlay_at(tab_layout, tab_panes, row, column))
    {
        return ClickIntent::FocusFloatingPane(id);
    }
    // The `+k` overflow marker folds the hidden floats that don't fit the chip
    // run. A click on it reveals-and-focuses the FIRST folded float (#113):
    // the reveal un-hides the whole layer, so the rest become individually
    // clickable overlay boxes. Resolving here — before the tiled fallback —
    // also keeps the marker shielding the pane it sits over (#110).
    if let Some(id) = chip_marker_target_at(tab_layout, tab_panes, row, column) {
        return ClickIntent::FocusFloatingPane(id);
    }
    if let Some(id) = pane_at(tab_layout, tab_panes, row, column) {
        return ClickIntent::FocusPane(id);
    }
    if let Some(target) = line::switch_target_at_column(tab_layout, column) {
        return ClickIntent::SwitchTab(target);
    }
    ClickIntent::NoOp
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MIN_ROWS;

    /// A `TabPaneGeom` for a block at `start` of `width` columns, holding panes
    /// `(id, x, y, w, h)`, at the minimum bar height with no perspective inset —
    /// the shape `render` records for a grid-rung tab (#74).
    fn geom(start: usize, width: usize, panes: &[(usize, u32, u32, u32, u32)]) -> TabPaneGeom {
        TabPaneGeom {
            start,
            width,
            rows: MIN_ROWS,
            vinset: 0,
            panes: panes
                .iter()
                .map(|&(id, x, y, w, h)| minimap::PaneRect::new(id, x, y, w, h, "sh", false))
                .collect(),
            hidden_floats: Vec::new(),
            visible_floats: Vec::new(),
        }
    }

    /// A drawn span; `active` is irrelevant to hit-testing here, so it is `false`.
    fn hit(position: usize, start: usize, width: usize) -> line::TabHit {
        line::TabHit {
            position,
            start,
            width,
            active: false,
        }
    }

    /// An active [`line::TabHit`] at `position` spanning `start..start + width`.
    fn hit_active(position: usize, start: usize, width: usize) -> line::TabHit {
        line::TabHit {
            active: true,
            ..hit(position, start, width)
        }
    }

    #[test]
    fn pane_at_resolves_a_click_to_the_pane_under_the_cursor() {
        // A tab block drawn at columns 10..30 holding two side-by-side panes (id
        // 7 left, id 3 right). A click in the block's left half resolves to pane
        // 7, the right half to pane 3 — the finer hit-test the column-only switch
        // (#8) could not make.
        let tab_layout = vec![hit_active(0, 10, 20)];
        let tab_panes: BTreeMap<usize, TabPaneGeom> = [(
            0usize,
            geom(10, 20, &[(7, 0, 0, 40, 24), (3, 40, 0, 40, 24)]),
        )]
        .into_iter()
        .collect();

        assert_eq!(
            pane_at(&tab_layout, &tab_panes, 1, 12),
            Some(7),
            "left half → pane 7"
        );
        assert_eq!(
            pane_at(&tab_layout, &tab_panes, 1, 27),
            Some(3),
            "right half → pane 3"
        );
    }

    #[test]
    fn pane_at_is_none_off_the_block_and_above_the_bar() {
        // A column outside every recorded span, and a negative click line (the
        // pointer above the pane), both resolve to no pane — so the caller falls
        // back to a plain tab-switch / no-op rather than focusing a wrong pane.
        let tab_layout = vec![hit_active(0, 10, 20)];
        let tab_panes: BTreeMap<usize, TabPaneGeom> =
            [(0usize, geom(10, 20, &[(7, 0, 0, 80, 24)]))]
                .into_iter()
                .collect();

        assert_eq!(
            pane_at(&tab_layout, &tab_panes, 1, 5),
            None,
            "column left of the block"
        );
        assert_eq!(
            pane_at(&tab_layout, &tab_panes, -1, 12),
            None,
            "line above the bar"
        );
    }

    #[test]
    fn pane_at_resolves_inside_an_inactive_tabs_minimap() {
        // A non-active grid-rung tab still records its pane geometry, so a click
        // on its minimap resolves to a pane — the handler then focuses it, which
        // also switches to that tab (zellij's `focus_terminal_pane`): a click on
        // a non-active tab's pane both switches and focuses in one step (#74).
        let tab_layout = vec![hit(1, 0, 12)];
        let tab_panes: BTreeMap<usize, TabPaneGeom> = [(1usize, geom(0, 12, &[(4, 0, 0, 80, 24)]))]
            .into_iter()
            .collect();

        assert_eq!(pane_at(&tab_layout, &tab_panes, 1, 6), Some(4));
    }

    #[test]
    fn pane_at_falls_back_when_the_tab_draws_no_minimap() {
        // A narrow tab (an L3 glyph / L4 hint rung) records a column span but no
        // pane geometry — the grid-rung filter dropped it — so a click resolves
        // to no pane and the caller falls back to #8's tab-switch, never a
        // wrong-pane focus.
        let tab_layout = vec![hit(0, 10, 3)];
        // tab_panes deliberately left empty for this tab.
        let tab_panes: BTreeMap<usize, TabPaneGeom> = BTreeMap::new();

        assert_eq!(pane_at(&tab_layout, &tab_panes, 1, 11), None);
    }

    #[test]
    fn clicked_new_tab_button_hit_tests_the_recorded_button_span() {
        // The pure routing predicate behind a new-tab click: a column inside the
        // recorded "+" span is a hit, one outside misses, and no recorded button
        // (disabled or no frame yet) always misses. Keeping the decision pure is
        // what lets it be tested without a zellij host — the `new_tab` host
        // effect (which reads stdin) is reached only past a true hit.
        assert!(
            !clicked_new_tab_button(None, 10),
            "no recorded button → every click misses"
        );

        let button = Some(line::ButtonHit {
            start: 20,
            width: 3,
        });
        assert!(
            clicked_new_tab_button(button, 20),
            "left edge of the span hits"
        );
        assert!(
            clicked_new_tab_button(button, 22),
            "right edge of the span hits"
        );
        assert!(
            !clicked_new_tab_button(button, 19),
            "just before the span misses"
        );
        assert!(
            !clicked_new_tab_button(button, 23),
            "just past the span misses"
        );
    }

    #[test]
    fn clicked_close_button_hit_tests_the_recorded_close_cell() {
        // The pure routing predicate behind a close click: only the exact
        // (row, column) cell recorded for a tab resolves to its position. A
        // click one row down (still in the block, but a pane/switch target) or
        // one column off misses, and an empty `close_layout` (disabled or a lone
        // tab) always misses — so the `close_tab_with_index` host effect is
        // reached only past a true hit, and the last tab is never closeable.
        assert_eq!(
            clicked_close_button(&[], 0, 9),
            None,
            "no recorded close cell → every click misses"
        );

        let close_layout = vec![line::CloseHit {
            position: 2,
            row: 0,
            column: 9,
        }];
        assert_eq!(
            clicked_close_button(&close_layout, 0, 9),
            Some(2),
            "the exact close cell resolves to its tab position"
        );
        assert_eq!(
            clicked_close_button(&close_layout, 1, 9),
            None,
            "one row below the close cell misses (still a switch/focus target)"
        );
        assert_eq!(
            clicked_close_button(&close_layout, 0, 8),
            None,
            "one column left of the close cell misses"
        );
        assert_eq!(
            clicked_close_button(&close_layout, -1, 9),
            None,
            "a negative click row (above the bar) matches no cell"
        );
    }

    #[test]
    fn route_click_resolves_to_the_topmost_matching_affordance() {
        // One frame carrying every affordance: a "+" button at cols 20..23, a
        // close "×" at (row 0, col 15), and a grid-rung tab block at cols 10..20
        // holding pane 7. route_click resolves a click to exactly one intent, in
        // the bar's paint priority — button > close > pane > tab block > nothing.
        let button = Some(line::ButtonHit {
            start: 20,
            width: 3,
        });
        let close_layout = vec![line::CloseHit {
            position: 0,
            row: 0,
            column: 15,
        }];
        let tab_layout = vec![hit_active(0, 10, 20)];
        let tab_panes: BTreeMap<usize, TabPaneGeom> =
            [(0usize, geom(10, 20, &[(7, 0, 0, 80, 24)]))]
                .into_iter()
                .collect();

        // "+" button sits on top: a press in its span opens a tab even though it
        // overlaps the tab block's columns.
        assert_eq!(
            route_click(button, &close_layout, &tab_layout, &tab_panes, 0, 21),
            ClickIntent::NewTab,
        );
        // The close "×" cell beats the pane/switch fallbacks under it.
        assert_eq!(
            route_click(button, &close_layout, &tab_layout, &tab_panes, 0, 15),
            ClickIntent::CloseTab(0),
        );
        // Off the button and close cell, a click on the minimap focuses its pane.
        assert_eq!(
            route_click(button, &close_layout, &tab_layout, &tab_panes, 1, 12),
            ClickIntent::FocusPane(7),
        );
        // A tab block that draws no minimap (no `tab_panes` entry) falls back to
        // a plain tab-switch, resolved to the 1-based `switch_tab_to` target.
        let narrow = vec![hit(0, 10, 3)];
        assert_eq!(
            route_click(None, &[], &narrow, &BTreeMap::new(), 1, 11),
            ClickIntent::SwitchTab(1),
        );
        // A click off every affordance is a no-op.
        assert_eq!(
            route_click(button, &close_layout, &tab_layout, &tab_panes, 1, 5),
            ClickIntent::NoOp,
        );
    }

    #[test]
    fn float_chip_at_resolves_a_hidden_float_click() {
        // A grid-rung tab at cols 10..30, MIN_ROWS tall, with two hidden floats
        // (ids 7, 9). Chips ride the bottom row (MIN_ROWS - 1 = 2), block-local
        // cols 18,19 → chips 0,1 → absolute cols 28,29. A click on the rightmost
        // chip resolves to float 9, the next to 7; a click above the bottom misses.
        let tab_layout = vec![hit_active(0, 10, 20)];
        let mut g = geom(10, 20, &[]);
        g.hidden_floats = vec![7, 9];
        let tab_panes: BTreeMap<usize, TabPaneGeom> = [(0usize, g)].into_iter().collect();
        assert_eq!(
            float_chip_at(&tab_layout, &tab_panes, 2, 29),
            Some(9),
            "rightmost chip → float 9"
        );
        assert_eq!(float_chip_at(&tab_layout, &tab_panes, 2, 28), Some(7));
        assert_eq!(
            float_chip_at(&tab_layout, &tab_panes, 1, 29),
            None,
            "not the bottom row"
        );
    }

    #[test]
    fn route_click_prefers_a_float_chip_over_the_tiled_pane_under_it() {
        // The bottom-right corner holds both a tiled pane (whole block) and a
        // float chip. route_click resolves the chip first (float priority §7.1).
        let tab_layout = vec![hit_active(0, 10, 20)];
        let mut g = geom(10, 20, &[(5, 0, 0, 80, 24)]); // one tiled pane fills the block
        g.hidden_floats = vec![7];
        let tab_panes: BTreeMap<usize, TabPaneGeom> = [(0usize, g)].into_iter().collect();
        // Chip at the bottom-right cell (row 2, col 10+20-1 = 29).
        assert_eq!(
            route_click(None, &[], &tab_layout, &tab_panes, 2, 29),
            ClickIntent::FocusFloatingPane(7),
            "the chip beats the tiled pane beneath it",
        );
        // A click elsewhere in the block still focuses the tiled pane.
        assert_eq!(
            route_click(None, &[], &tab_layout, &tab_panes, 0, 12),
            ClickIntent::FocusPane(5),
        );
    }

    #[test]
    fn route_click_prefers_a_visible_float_overlay_over_the_tiled_pane() {
        // A tab whose visible float overlay covers part of the block. A click on
        // the float resolves to FocusFloatingPane; elsewhere falls to the tiled
        // pane.
        let tab_layout = vec![hit_active(0, 10, 20)];
        let mut g = geom(10, 20, &[(0, 0, 0, 100, 40)]); // tiled fills block
        g.visible_floats = vec![minimap::PaneRect::new(7, 30, 12, 40, 16, "f", false)];
        let tab_panes: BTreeMap<usize, TabPaneGeom> = [(0usize, g)].into_iter().collect();
        // A click at the float's center (block-local ~col 8, row 1 → absolute col 18).
        assert_eq!(
            route_click(None, &[], &tab_layout, &tab_panes, 1, 18),
            ClickIntent::FocusFloatingPane(7),
        );
    }

    #[test]
    fn route_click_reveals_the_first_folded_float_from_the_marker() {
        // A narrow block (3 cols) with 5 hidden floats overflows the chip run:
        // `chip_cells(3, 5)` = [ +3 marker @0, Float0 @1, Float1 @2 ], folding
        // ids 103,104,105 behind the marker. Clicking the marker resolves to the
        // FIRST folded float (#113): focusing it un-hides the whole layer, so
        // the rest become individually clickable overlay boxes. The marker keeps
        // shielding the tiled pane beneath it, and real chips still resolve.
        let tab_layout = vec![hit_active(0, 10, 3)];
        let mut g = geom(10, 3, &[(5, 0, 0, 80, 24)]); // tiled pane fills the block
        g.hidden_floats = vec![101, 102, 103, 104, 105];
        let tab_panes: BTreeMap<usize, TabPaneGeom> = [(0usize, g)].into_iter().collect();
        let chip_row = (MIN_ROWS - 1) as isize;
        // Marker at block-local col 0 → absolute col 10: first folded float
        // (index 5 - 3 = 2 → id 103), never FocusPane(5).
        assert_eq!(
            route_click(None, &[], &tab_layout, &tab_panes, chip_row, 10),
            ClickIntent::FocusFloatingPane(103),
            "the +k marker reveals the first folded float",
        );
        // A real float chip still resolves (block-local col 2 → absolute col 12).
        assert_eq!(
            route_click(None, &[], &tab_layout, &tab_panes, chip_row, 12),
            ClickIntent::FocusFloatingPane(102),
        );
        // The marker rides only the bottom text row: one row above its column,
        // the click falls through to the tiled pane as before.
        assert_eq!(
            route_click(None, &[], &tab_layout, &tab_panes, chip_row - 1, 10),
            ClickIntent::FocusPane(5),
            "one row above the marker still focuses the tiled pane",
        );
    }

    #[test]
    fn route_click_marker_resolves_when_every_float_folds() {
        // A 1-col block folds EVERY hidden float into the marker:
        // `chip_cells(1, 3)` = [ +3 @0 ], no Float chips. The first folded index
        // is then 0 (`len - k = 3 - 3`), so the marker resolves to the first
        // hidden float — not to the tiled pane beneath.
        let tab_layout = vec![hit_active(0, 10, 1)];
        let mut g = geom(10, 1, &[(5, 0, 0, 80, 24)]);
        g.hidden_floats = vec![201, 202, 203];
        let tab_panes: BTreeMap<usize, TabPaneGeom> = [(0usize, g)].into_iter().collect();
        let chip_row = (MIN_ROWS - 1) as isize;
        assert_eq!(
            route_click(None, &[], &tab_layout, &tab_panes, chip_row, 10),
            ClickIntent::FocusFloatingPane(201),
        );
    }
}

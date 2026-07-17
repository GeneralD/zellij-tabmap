//! Plugin-pane output framing — the opposite end of the pipeline from
//! [`crate::projection`].
//!
//! [`crate::line::pack`] decides where every tab sits; [`crate::tab_block::assemble`]
//! turns each into a raw, color-only [`TabBlock`] (no cursor positioning). This
//! module is the final stage: [`bar`] assembles every visible tab from a packed
//! [`LineLayout`] and lays the blocks — plus the `← +N` / `+N →` overflow
//! markers — into the multi-row plugin pane via [`compose`], which positions
//! each block at its column and clears every row so a wider previous frame
//! cannot bleed through. Both are pure string transforms with no zellij
//! dependency, so they unit-test natively.

use std::collections::BTreeMap;

use crate::color::Palette;
use crate::line::LineLayout;
use crate::minimap::{Close, GradientSpec, PaneRect};
use crate::tab_block::{self, TabBlock};

/// Render a packed [`LineLayout`] into the full multi-row bar string.
///
/// Output is driven from `layout.tabs` (already ordered left-to-right). For each
/// [`crate::line::TabHit`] the tab's already-projected panes are looked up by
/// `position` in `panes_by_position` (an empty slice when the tab has none yet)
/// and assembled into a block of exactly `hit.width` columns via
/// [`crate::tab_block::assemble`]. Two invariants matter here:
///
/// * **Key panes by `position`, never by iteration order** — the pane manifest
///   is a map, so iterating it for output would make the bytes depend on its
///   ordering; looking each tab up by its own position keeps the frame stable.
/// * **Feed the *budgeted* width** (`hit.width`) into the ladder, not the active
///   width — a narrow inactive tab must degrade to its own rung.
///
/// The `← +N` / `+N →` overflow markers are placed at their own columns on the
/// middle row.
///
/// `close` ([`Close`]) enables the top-right close affordance (#86); the caller
/// passes an on-variant (carrying the Nerd Font vs ASCII form, #94) only when the
/// feature is on and closing a tab is safe (more than one tab open). It lands per
/// tab on the active tab — and, when `perspective` is off, on every tab — but not
/// on inactive perspective tabs, whose receded corner would carry it unbalanced.
#[allow(clippy::too_many_arguments)]
pub fn bar(
    rows: usize,
    layout: &LineLayout,
    panes_by_position: &BTreeMap<usize, Vec<PaneRect>>,
    palette: &Palette,
    prefix: &str,
    gradient: GradientSpec,
    inactive_dim: bool,
    perspective: bool,
    close: Close,
    floats_by_position: &BTreeMap<usize, crate::floating::FloatSpec>,
    suppressed_covers_by_position: &BTreeMap<usize, Vec<usize>>,
    pinned_floats_by_position: &BTreeMap<usize, Vec<usize>>,
) -> String {
    // #59: inactive tabs render through the canvas-receded palette while the
    // active tab keeps full vibrancy, so the selected tab reads at a glance.
    // Derived once per frame, not per tab; `inactive_dim: false` opts out and
    // reproduces the historical equally-vivid strip.
    let dimmed = inactive_dim.then(|| palette.dimmed());
    let blocks: Vec<TabBlock> = layout
        .tabs
        .iter()
        .map(|hit| {
            let panes = panes_by_position
                .get(&hit.position)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let tab_palette = match &dimmed {
                Some(dim) if !hit.active => dim,
                _ => palette,
            };
            // Close lands on the active tab, plus every tab when perspective is
            // off (#86) — inactive perspective tabs recede, where a corner glyph
            // reads unbalanced, so they skip it (carrying `Close::Off`). Same
            // predicate as the click hit-test in `State::render`, so draw and
            // hit-test never disagree. Tabs that keep it carry the bar-wide form
            // (#94).
            let tab_close = if close.is_on() && (hit.active || !perspective) {
                close
            } else {
                Close::Off
            };
            // This tab's floating layer (#110): its chips (hidden) or overlay
            // (visible), borrowed from the per-frame spec `State::render` built.
            // Absent → `None`, so a tab with no floats renders exactly as before.
            let floats = floats_by_position
                .get(&hit.position)
                .map(crate::floating::FloatSpec::layer)
                .unwrap_or(crate::floating::FloatLayer::None);
            // This tab's suppressed-pane cover ids (#118), resolved the same way
            // as `floats` — absent → an empty slice, so a tab with no covers
            // stamps no marker.
            let suppressed_covers = suppressed_covers_by_position
                .get(&hit.position)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            // This tab's pinned float ids (#119), resolved the same way as
            // `suppressed_covers` — absent → an empty slice, so a tab with no
            // pinned floats stamps no pin marker.
            let pinned_floats = pinned_floats_by_position
                .get(&hit.position)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            tab_block::assemble(
                panes,
                tab_palette,
                hit.width,
                rows,
                hit.position,
                prefix,
                gradient,
                hit.active,
                perspective,
                tab_close,
                floats,
                suppressed_covers,
                pinned_floats,
            )
        })
        .collect();
    // The inline new-tab `+` button (#76), when the layout reserved one: a block
    // placed through the same `compose` path as the tabs, sized to read as an
    // inactive tab — it takes `perspective` so it recedes in lockstep with them
    // (#66) instead of floating at the active tab's full height. Owned here so
    // `placed` can borrow it; appended after the tabs so it draws on top (its
    // span never overlaps one, so order is cosmetic).
    let button = layout.button.map(|hit| {
        (
            hit.start,
            tab_block::button_block(hit.width, rows, perspective),
        )
    });
    let placed: Vec<(usize, &TabBlock)> = layout
        .tabs
        .iter()
        .zip(&blocks)
        .map(|(hit, block)| (hit.start, block))
        .chain(button.iter().map(|(start, block)| (*start, block)))
        .collect();
    let markers: Vec<(usize, &str)> = layout
        .left
        .iter()
        .chain(layout.right.iter())
        .map(|overflow| (overflow.start, overflow.text.as_str()))
        .collect();
    compose(rows, &placed, &markers)
}

/// Lay pre-rendered blocks and overflow markers into `rows` absolutely
/// positioned, end-of-line-cleared rows for a multi-row plugin pane.
///
/// Each `(start, block)` in `placed` has its rows drawn at 0-based column
/// `start` (emitted 1-based); each `(start, text)` in `markers` is a single-line
/// overflow marker drawn on the middle row only — [`crate::line::pack`]
/// guarantees a marker span never overlaps a tab span. Every row is first homed
/// (`\u{1b}[{n};1H`), has its SGR attributes reset (`\u{1b}[0m`), then is cleared
/// to end-of-line (`\u{1b}[0K`). The reset before the erase matters: `0K` clears
/// using the *current* background (background-color erase), so without it a
/// prior frame's lingering background would paint the cleared tail. Resetting
/// first makes the no-bleed guarantee self-contained — it holds even if a block
/// line ever stops terminating in a reset. No trailing newline is emitted (a 4th
/// line in a 3-row pane scrolls the block up). Output order follows `placed`
/// then `markers`, so callers pass blocks left-to-right for byte-stable output.
pub fn compose(rows: usize, placed: &[(usize, &TabBlock)], markers: &[(usize, &str)]) -> String {
    let middle = rows / 2;
    (0..rows)
        .map(|row| {
            let cleared = format!("\u{1b}[{n};1H\u{1b}[0m\u{1b}[0K", n = row + 1);
            let blocks: String = placed
                .iter()
                .filter_map(|(start, block)| {
                    block.lines.get(row).map(|line| {
                        format!(
                            "\u{1b}[{n};{col}H{text}",
                            n = row + 1,
                            col = start + 1,
                            text = line.as_str()
                        )
                    })
                })
                .collect();
            let marks: String = if row == middle {
                markers
                    .iter()
                    .map(|(start, text)| {
                        format!("\u{1b}[{n};{col}H{text}", n = row + 1, col = start + 1)
                    })
                    .collect()
            } else {
                String::new()
            };
            format!("{cleared}{blocks}{marks}")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::line::{ButtonHit, Overflow, TabHit};
    use crate::minimap;
    use crate::tab_block::StyledLine;

    /// A block with the given row texts, for `compose` placement tests.
    fn block(rows: &[&str], width: usize, position: usize) -> TabBlock {
        TabBlock {
            lines: rows
                .iter()
                .map(|text| StyledLine(text.to_string()))
                .collect(),
            width,
            position,
        }
    }

    // ---- compose ---------------------------------------------------------

    #[test]
    fn compose_positions_each_row_at_the_block_start_column() {
        // Block at start column 4 → 1-based column 5, one cursor move per row.
        let b = block(&["R0", "R1", "R2"], 2, 0);
        let out = compose(3, &[(4, &b)], &[]);
        assert!(out.contains("\u{1b}[1;5HR0"));
        assert!(out.contains("\u{1b}[2;5HR1"));
        assert!(out.contains("\u{1b}[3;5HR2"));
    }

    #[test]
    fn compose_clears_every_row_to_end_of_line() {
        let b = block(&["R0", "R1", "R2"], 2, 0);
        assert_eq!(compose(3, &[(0, &b)], &[]).matches("\u{1b}[0K").count(), 3);
    }

    #[test]
    fn compose_resets_sgr_before_clearing_each_row() {
        // `0K` erases with the current background (background-color erase), so a
        // reset must precede it or a prior frame's lingering background paints
        // the cleared tail. Pinning the reset here keeps the no-bleed guarantee
        // self-contained rather than relying on every block line resetting.
        let b = block(&["R0", "R1", "R2"], 2, 0);
        let out = compose(3, &[(0, &b)], &[]);
        for n in 1..=3 {
            assert!(
                out.contains(&format!("\u{1b}[{n};1H\u{1b}[0m\u{1b}[0K")),
                "row {n} resets SGR before erase-to-EOL"
            );
        }
    }

    #[test]
    fn compose_has_no_trailing_newline() {
        let b = block(&["R0", "R1", "R2"], 2, 0);
        assert!(!compose(3, &[(0, &b)], &[]).ends_with('\n'));
    }

    #[test]
    fn compose_caps_at_the_row_budget() {
        // Two-row pane: the block's third row must not be emitted.
        let b = block(&["R0", "R1", "R2"], 2, 0);
        let out = compose(2, &[(0, &b)], &[]);
        assert_eq!(out.matches("\u{1b}[0K").count(), 2);
        assert!(out.contains("R0") && out.contains("R1"));
        assert!(!out.contains("R2"));
    }

    #[test]
    fn compose_homes_and_clears_every_row_of_a_tall_bar() {
        // #66 step 2: the bar height is runtime, so compose must home, reset and
        // clear exactly `rows` rows — no longer a fixed three. A 5-row block in a
        // 5-row pane draws all five rows, each homed at column 1.
        let b = block(&["R0", "R1", "R2", "R3", "R4"], 2, 0);
        let out = compose(5, &[(0, &b)], &[]);
        assert_eq!(out.matches("\u{1b}[0K").count(), 5);
        for n in 1..=5 {
            assert!(
                out.contains(&format!("\u{1b}[{n};1HR{}", n - 1)),
                "row {n} must be homed and carry its block line"
            );
        }
    }

    #[test]
    fn compose_lays_blocks_left_to_right_in_order() -> Result<(), Box<dyn std::error::Error>> {
        let a = block(&["AA", "AA", "AA"], 2, 0);
        let b = block(&["BB", "BB", "BB"], 2, 1);
        let out = compose(3, &[(0, &a), (5, &b)], &[]);
        let first = out.find("\u{1b}[1;1HAA").ok_or("block A on row 1")?;
        let second = out
            .find("\u{1b}[1;6HBB")
            .ok_or("block B on row 1 at col 6")?;
        assert!(first < second, "blocks emitted in given order");
        Ok(())
    }

    #[test]
    fn compose_draws_markers_only_on_the_middle_row() {
        let b = block(&["R0", "R1", "R2"], 2, 0);
        let out = compose(3, &[(0, &b)], &[(8, "+9")]);
        // rows=3 → middle is row index 1 → 1-based row 2, start col 8+1=9.
        assert!(out.contains("\u{1b}[2;9H+9"));
        assert_eq!(
            out.matches("+9").count(),
            1,
            "marker drawn once, on row 2 only"
        );
    }

    // ---- bar -------------------------------------------------------------

    fn layout(tabs: Vec<TabHit>, left: Option<Overflow>, right: Option<Overflow>) -> LineLayout {
        LineLayout {
            tabs,
            left,
            right,
            button: None,
        }
    }

    fn hit(position: usize, start: usize, width: usize, active: bool) -> TabHit {
        TabHit {
            position,
            start,
            width,
            active,
        }
    }

    #[test]
    fn bar_places_each_visible_block_at_its_start_column() {
        // Two narrow (L4) tabs at columns 0 and 2; no panes needed for the hint
        // rung. Every (row, block) pair gets a cursor move at the block's column.
        let lo = layout(vec![hit(0, 0, 2, false), hit(1, 2, 2, true)], None, None);
        let out = bar(
            3,
            &lo,
            &BTreeMap::new(),
            &Palette::default(),
            "\u{2318}",
            GradientSpec::OFF,
            false,
            false,
            Close::Off,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        for row in 1..=3 {
            assert!(
                out.contains(&format!("\u{1b}[{row};1H")),
                "block 0 row {row}"
            );
            assert!(
                out.contains(&format!("\u{1b}[{row};3H")),
                "block 1 row {row}"
            );
        }
    }

    #[test]
    fn bar_keys_the_hint_by_hit_position_not_iteration_index() {
        // Positions 3 and 4 sit at loop indices 0 and 1. The L4 hint encodes the
        // 1-based position, so a position-keyed bar shows ⌘4 / ⌘5; an
        // index-keyed bug would show ⌘1 / ⌘2.
        let lo = layout(vec![hit(3, 0, 2, true), hit(4, 2, 2, false)], None, None);
        let out = bar(
            3,
            &lo,
            &BTreeMap::new(),
            &Palette::default(),
            "\u{2318}",
            GradientSpec::OFF,
            false,
            false,
            Close::Off,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert!(out.contains("\u{2318}4"), "position 3 → ⌘4");
        assert!(out.contains("\u{2318}5"), "position 4 → ⌘5");
        assert!(!out.contains("\u{2318}1"), "must not key by loop index");
        assert!(!out.contains("\u{2318}2"), "must not key by loop index");
    }

    #[test]
    fn bar_assembles_each_tab_at_its_own_budgeted_width() {
        // Each tab picks its ladder rung from its *own* budgeted width, never a
        // shared one. The active tab (width 16) lands on L0: a color grid that
        // stamps "⌘1" as an in-block badge. The minimap draws the badge
        // cell-by-cell, so its glyph and digit are split by SGR escapes and never
        // form the *contiguous* "⌘1" that a hint rung emits. The inactive tab
        // (width 2) lands on L4: the shortcut hint proper, a contiguous,
        // centered "⌘2".
        //
        // That contiguous-vs-split split is exactly what discriminates the two
        // budgeting bugs: feeding the active width to the inactive would replace
        // its contiguous "⌘2" hint with an L0 grid (no contiguous "⌘2"); feeding
        // the inactive width to the active would collapse its grid+badge into a
        // contiguous "⌘1" hint.
        let mut panes = BTreeMap::new();
        panes.insert(0, vec![PaneRect::new(0, 0, 0, 10, 10, "shell", true)]);
        let lo = layout(vec![hit(0, 0, 16, true), hit(1, 16, 2, false)], None, None);
        let out = bar(
            3,
            &lo,
            &panes,
            &Palette::default(),
            "\u{2318}",
            GradientSpec::OFF,
            false,
            false,
            Close::Off,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert!(
            out.contains("\u{2318}2"),
            "inactive tab (width 2) renders its own L4 hint as a contiguous ⌘2"
        );
        assert!(
            !out.contains("\u{2318}1"),
            "active tab (width 16) is an L0 grid — its ⌘1 is a split badge, not a contiguous hint"
        );
    }

    /// Truecolor foreground escape for `c`, as the emitter writes it.
    fn fg(c: crate::color::Rgb) -> String {
        format!("\x1b[38;2;{};{};{}m", c.0, c.1, c.2)
    }

    /// Truecolor background escape for `c`, as the emitter writes it.
    fn bg(c: crate::color::Rgb) -> String {
        format!("\x1b[48;2;{};{};{}m", c.0, c.1, c.2)
    }

    /// Two single-pane tabs — tab 0 active at an L0 width, tab 1 inactive at an
    /// L2 width — so both render color grids whose fills witness the palette
    /// each tab was assembled with (#59).
    fn two_tab_fixture() -> (Palette, BTreeMap<usize, Vec<PaneRect>>, LineLayout) {
        let palette = Palette::new(
            vec![(10, 20, 30), (40, 50, 60), (70, 80, 90)],
            (200, 100, 50),
        );
        let mut panes = BTreeMap::new();
        panes.insert(0, vec![PaneRect::new(0, 0, 0, 10, 10, "", false)]);
        panes.insert(1, vec![PaneRect::new(1, 0, 0, 10, 10, "", false)]);
        let lo = layout(vec![hit(0, 0, 16, true), hit(1, 18, 6, false)], None, None);
        (palette, panes, lo)
    }

    #[test]
    fn bar_dims_inactive_tab_fills_and_keeps_the_active_vivid() {
        // #59: with `inactive_dim` on, an inactive tab's pane fills recede to
        // the dimmed palette while the active tab keeps full vibrancy — and
        // the active block's badge text turns white, proving the active flag
        // reached `assemble`.
        let (palette, panes, lo) = two_tab_fixture();
        let out = bar(
            3,
            &lo,
            &panes,
            &palette,
            "\u{2318}",
            GradientSpec::OFF,
            true,
            false,
            Close::Off,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert!(
            out.contains(&fg(palette.color_for(0))),
            "the active tab keeps its vivid fill"
        );
        assert!(
            out.contains(&fg(palette.dimmed().color_for(1))),
            "an inactive tab's fill recedes to the dimmed palette"
        );
        assert!(
            !out.contains(&fg(palette.color_for(1))),
            "an inactive tab must not render its vivid fill"
        );
        assert!(
            out.contains(&fg(minimap::ACTIVE_FG)),
            "the active tab's badge text is white — stands out on vivid fill (#59)"
        );
        assert!(
            !out.contains(&bg(palette.accent())),
            "no accent chip remains anywhere on the bar"
        );
    }

    #[test]
    fn bar_inactive_dim_off_keeps_every_tab_vivid() {
        // The opt-out: `inactive_dim: false` keeps the historical
        // equally-vivid strip — no dimmed fill anywhere.
        let (palette, panes, lo) = two_tab_fixture();
        let out = bar(
            3,
            &lo,
            &panes,
            &palette,
            "\u{2318}",
            GradientSpec::OFF,
            false,
            false,
            Close::Off,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert!(
            out.contains(&fg(palette.color_for(1))),
            "an inactive tab keeps its vivid fill when dimming is off"
        );
        assert!(
            !out.contains(&fg(palette.dimmed().color_for(1))),
            "no dimmed fill may appear when dimming is off"
        );
    }

    #[test]
    fn bar_draws_overflow_markers_on_the_middle_row() {
        let lo = layout(
            vec![hit(2, 5, 2, true)],
            Some(Overflow {
                hidden: 2,
                start: 0,
                text: "\u{2190} +2 ".to_string(),
            }),
            Some(Overflow {
                hidden: 3,
                start: 7,
                text: " +3 \u{2192}".to_string(),
            }),
        );
        let out = bar(
            3,
            &lo,
            &BTreeMap::new(),
            &Palette::default(),
            "\u{2318}",
            GradientSpec::OFF,
            false,
            false,
            Close::Off,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        // Middle row (row 2): left marker at col 1, right marker at col 8.
        assert!(out.contains("\u{1b}[2;1H\u{2190} +2 "));
        assert!(out.contains("\u{1b}[2;8H +3 \u{2192}"));
        // Each marker appears exactly once — never duplicated onto rows 1 / 3.
        assert_eq!(out.matches("+2").count(), 1);
        assert_eq!(out.matches("+3").count(), 1);
    }

    #[test]
    fn bar_draws_the_new_tab_button_at_its_reserved_span() {
        // #76: when the layout reserves a "+" button, `bar` lays its full-height
        // block at the button's start column through the same `compose` path as
        // the tabs. The "+" rides the middle row (the row `compose` homes markers
        // on), painted in the muted glyph foreground — never a tab's pane fill.
        let mut lo = layout(vec![hit(0, 0, 16, true)], None, None);
        lo.button = Some(ButtonHit {
            start: 18,
            width: 3,
        });
        let out = bar(
            3,
            &lo,
            &BTreeMap::new(),
            &Palette::default(),
            "\u{2318}",
            GradientSpec::OFF,
            false,
            false,
            Close::Off,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        // rows=3 → middle row index 1 → 1-based row 2, button start col 18+1=19.
        assert!(
            out.contains("\u{1b}[2;19H"),
            "the button block is homed at its reserved column"
        );
        assert!(
            out.contains(&fg(crate::color::button_glyph())),
            "the + is painted in the muted glyph foreground"
        );
        assert!(out.contains('+'), "the new-tab glyph is drawn");
    }

    #[test]
    fn bar_omits_the_button_when_the_layout_reserves_none() {
        // No reserved button → no "+" anywhere: the button is purely opt-in,
        // gated upstream by the config toggle (#76).
        let lo = layout(vec![hit(0, 0, 16, true)], None, None);
        let out = bar(
            3,
            &lo,
            &BTreeMap::new(),
            &Palette::default(),
            "\u{2318}",
            GradientSpec::OFF,
            false,
            false,
            Close::Off,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert!(!out.contains('+'), "no button reserved → no + drawn");
    }
}

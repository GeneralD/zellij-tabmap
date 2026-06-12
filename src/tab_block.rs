//! Per-tab compositor — assemble one tab into a fixed three-row block of an
//! exact budgeted width.
//!
//! [`crate::line::pack`] hands each tab a column budget; this module turns that
//! budget plus the tab's panes into three rendered rows via the L0–L4
//! degradation ladder (design §4.3). The detail shown is a pure function of the
//! budgeted width:
//!
//! | rung | width        | content                                    |
//! |------|--------------|--------------------------------------------|
//! | L0   | `>= L0_MIN`  | color grid + every fitting pane label      |
//! | L1   | `>= L1_MIN`  | color grid + the focused pane's label only |
//! | L2   | `>= L2_MIN`  | color grid only (text would garble)        |
//! | L3   | `>= L3_MIN`  | one representative split/grid glyph         |
//! | L4   | else         | the `⌘N` shortcut hint only                |
//!
//! Pixel work for the grid rungs (L0–L2) is delegated to
//! [`crate::minimap::render`] with the matching [`LabelMode`]; this module only
//! selects the rung, draws the L3 glyph and L4 hint, and guarantees the
//! three-row / exact-width contract. Per-pane label degradation (a too-narrow
//! or too-short pane, or a deep vertical stack) is the minimap's concern and
//! happens underneath every grid rung.
//!
//! A [`StyledLine`] is the concrete form of the design's `StyledLine`: an
//! ANSI-styled string whose *display* width (escape sequences excluded) equals
//! the block's budgeted width. Lines are **raw** — they carry color but no
//! cursor positioning; framing into the plugin pane stays in
//! [`crate::paint::compose`] so width accounting and the layer boundary hold.

use crate::color::{self, Palette, Rgb};
use crate::minimap::{self, GradientMode, LabelMode, PaneRect};
use unicode_width::UnicodeWidthChar;

/// Text-row height of every tab block — the bar is pinned to three rows, a
/// six-pixel canvas (two half-block pixels per row).
const ROWS: usize = 3;

/// Minimum width for the richest rung (L0): a wide active tab showing the color
/// grid with a label on every pane that fits. Matches [`crate::line::ACTIVE_MIN`]
/// so a fully-budgeted active tab always lands here.
pub const L0_MIN: usize = 16;
/// Minimum width for L1 — the color grid plus the focused pane's label only.
/// Below the active range there is room for one centered label but not several.
pub const L1_MIN: usize = 10;
/// Minimum width for L2 — color grid only. The layout still reads as colored
/// regions, but a label would crowd out the few remaining cells.
pub const L2_MIN: usize = 5;
/// Minimum width for L3 — a single representative split/grid glyph. Below this a
/// multi-cell grid no longer reads, so one symbol stands in for the whole tab.
pub const L3_MIN: usize = 3;

/// A rung of the L0–L4 degradation ladder. Selected purely from the budgeted
/// width by [`level_for`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// Color grid + all fitting pane labels.
    L0,
    /// Color grid + the focused pane's label only.
    L1,
    /// Color grid only.
    L2,
    /// One representative split/grid glyph.
    L3,
    /// The `⌘N` shortcut hint only.
    L4,
}

#[cfg(test)]
impl Level {
    /// How much detail this rung shows, 4 (richest, L0) down to 0 (sparsest, L4).
    /// Used to assert the ladder is monotonic in width.
    fn richness(self) -> u8 {
        match self {
            Level::L0 => 4,
            Level::L1 => 3,
            Level::L2 => 2,
            Level::L3 => 1,
            Level::L4 => 0,
        }
    }
}

/// One rendered row: an ANSI-styled string whose display width (escapes
/// excluded) equals the owning block's budgeted width. "Styled" means color via
/// embedded SGR escapes — this is the concrete realization of the design doc's
/// `StyledLine`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StyledLine(pub String);

impl StyledLine {
    /// The styled bytes, ready to print once positioned by [`crate::paint`].
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Display width in terminal columns, ignoring SGR/CSI escape sequences.
    pub fn width(&self) -> usize {
        display_width_ignoring_ansi(&self.0)
    }
}

/// One tab rendered as a fixed three-row block of exactly `width` columns.
///
/// A tab is identified by its `position` (zellij's `TabInfo.position`); there is
/// no separate stable tab id the way panes have one, and click-to-switch maps a
/// column back to a position, so position is the whole identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabBlock {
    /// The three rendered rows, top to bottom.
    pub lines: [StyledLine; ROWS],
    /// The budgeted display width every row fills.
    pub width: usize,
    /// 0-based tab position.
    pub position: usize,
}

/// Select the ladder rung for a tab budgeted `width` columns. A step function of
/// width alone — richer rungs need more columns, so it is monotonic.
pub fn level_for(width: usize) -> Level {
    match width {
        w if w >= L0_MIN => Level::L0,
        w if w >= L1_MIN => Level::L1,
        w if w >= L2_MIN => Level::L2,
        w if w >= L3_MIN => Level::L3,
        _ => Level::L4,
    }
}

/// Assemble `panes` into a three-row block of exactly `width` columns at tab
/// `position`, choosing detail via the ladder. `prefix` is the configured
/// shortcut glyph, used only by the L4 hint rung. `gradient` is the configured
/// fill sweep, used only by the grid rungs (L0–L2). `active` marks the bar's
/// selected tab: its shortcut text renders white and its focus ring draws,
/// while inactive blocks suppress the focus highlight (#59) — dimming the
/// *inactive* blocks is the caller's concern, applied through the palette it
/// hands in.
pub fn assemble(
    panes: &[PaneRect],
    palette: &Palette,
    width: usize,
    position: usize,
    prefix: &str,
    gradient: GradientMode,
    active: bool,
) -> TabBlock {
    // #32: stamp the `⌘N` shortcut *inside* the color block as a top-left badge
    // (text over the block's own colors) rather than a separate gutter, so
    // the shortcut shows on comfortably-sized tabs. The minimap self-skips the
    // badge when the block is too narrow to host it, so the grid rungs degrade
    // cleanly; the narrowest rungs (L3 glyph, L4 hint) carry the number on their
    // own as before.
    let hint = hint_text(position, prefix);
    let badge = Some(hint.as_str());
    let lines = match level_for(width) {
        Level::L0 => grid_lines(
            panes,
            palette,
            width,
            LabelMode::All,
            badge,
            gradient,
            active,
        ),
        Level::L1 => grid_lines(
            panes,
            palette,
            width,
            LabelMode::Focused,
            badge,
            gradient,
            active,
        ),
        Level::L2 => grid_lines(
            panes,
            palette,
            width,
            LabelMode::None,
            badge,
            gradient,
            active,
        ),
        Level::L3 => glyph_lines(panes, width, active),
        Level::L4 => hint_lines(position, prefix, width, active),
    };
    TabBlock {
        lines,
        width,
        position,
    }
}

/// The `⌘N` shortcut hint for a tab at 0-based `position`.
///
/// Shown 1-based to match `GoToTab N`. Per design §4.5 a tab with no `Super N`
/// binding (the 10th tab onward, i.e. 1-based >= 10) drops the prefix and shows
/// the bare number. This is the single source of the rule, used by the L4 rung
/// of [`assemble`].
pub fn hint_text(position: usize, prefix: &str) -> String {
    match position + 1 {
        number if number >= 10 => number.to_string(),
        number => format!("{prefix}{number}"),
    }
}

// ---- rung renderers -----------------------------------------------------

/// L0–L2: delegate the color grid to the minimap with the rung's label policy.
/// `badge`, when present, is the shortcut hint stamped into the block's top-left
/// (the minimap drops it if the block is too narrow to host it), white when
/// the block is the active tab (#59).
#[allow(clippy::too_many_arguments)]
fn grid_lines(
    panes: &[PaneRect],
    palette: &Palette,
    width: usize,
    mode: LabelMode,
    badge: Option<&str>,
    gradient: GradientMode,
    active: bool,
) -> [StyledLine; ROWS] {
    let block = minimap::render(panes, palette, width, ROWS, mode, badge, gradient, active);
    three_rows(block.lines().map(str::to_string), width)
}

/// L3: a single representative split/grid glyph centered on the canvas.
fn glyph_lines(panes: &[PaneRect], width: usize, active: bool) -> [StyledLine; ROWS] {
    let glyph = representative_glyph(panes);
    let fg = rung_text_fg(active);
    [
        StyledLine(blank_row(width)),
        StyledLine(text_row(&glyph.to_string(), fg, width)),
        StyledLine(blank_row(width)),
    ]
}

/// L4: the shortcut hint only, fitted to the budget, centered on the canvas.
fn hint_lines(position: usize, prefix: &str, width: usize, active: bool) -> [StyledLine; ROWS] {
    let hint = fit_hint(position, prefix, width);
    let fg = rung_text_fg(active);
    [
        StyledLine(blank_row(width)),
        StyledLine(text_row(&hint, fg, width)),
        StyledLine(blank_row(width)),
    ]
}

/// Text color of the narrow rungs (L3 glyph, L4 hint) (#59): the active tab's
/// single text element is pure white — stands out on the vivid fill. Inactive
/// tabs mute white toward the canvas fill so the text recedes gracefully.
fn rung_text_fg(active: bool) -> Rgb {
    if active {
        minimap::ACTIVE_FG
    } else {
        color::mixed(
            minimap::ACTIVE_FG,
            minimap::BG,
            minimap::INACTIVE_LABEL_BLEND,
        )
    }
}

/// Fit the shortcut hint into `width` columns: when the full `prefix + number`
/// overflows, drop the prefix to the bare number; when even that overflows (a
/// pathologically narrow slot), truncate the number to the budget. Number digits
/// are one column each, so a character truncation is a column truncation.
fn fit_hint(position: usize, prefix: &str, width: usize) -> String {
    let full = hint_text(position, prefix);
    if display_width_ignoring_ansi(&full) <= width {
        return full;
    }
    let bare = (position + 1).to_string();
    if display_width_ignoring_ansi(&bare) <= width {
        return bare;
    }
    bare.chars().take(width).collect()
}

/// Choose a glyph that reflects the tab's split layout from the panes' origins:
/// one pane → a single block, distinct columns → vertical bars, distinct rows →
/// horizontal bars, both → a grid. Sorting (not hashing) keeps it deterministic.
fn representative_glyph(panes: &[PaneRect]) -> char {
    if panes.len() <= 1 {
        return '\u{25aa}'; // ▪ single pane
    }
    let distinct = |values: &mut Vec<u32>| {
        values.sort_unstable();
        values.dedup();
        values.len() > 1
    };
    let varies_x = distinct(&mut panes.iter().map(|p| p.x).collect());
    let varies_y = distinct(&mut panes.iter().map(|p| p.y).collect());
    match (varies_x, varies_y) {
        (true, false) => '\u{25a5}', // ▥ side-by-side columns
        (false, true) => '\u{25a4}', // ▤ stacked rows
        _ => '\u{25a6}',             // ▦ grid (or degenerate overlap)
    }
}

// ---- styled-row primitives ----------------------------------------------

/// Collect up to three rendered rows, backfilling any short of three with a
/// blank canvas row so the `[_; ROWS]` contract always holds.
fn three_rows(mut rows: impl Iterator<Item = String>, width: usize) -> [StyledLine; ROWS] {
    let blank = || StyledLine(blank_row(width));
    [
        rows.next().map(StyledLine).unwrap_or_else(blank),
        rows.next().map(StyledLine).unwrap_or_else(blank),
        rows.next().map(StyledLine).unwrap_or_else(blank),
    ]
}

/// A full-width row of background half-blocks — the empty canvas.
fn blank_row(width: usize) -> String {
    let mut out = String::new();
    background_run(&mut out, width);
    out.push_str(minimap::RESET);
    out
}

/// `text` (no embedded ANSI) centered on a background canvas `width` columns
/// wide, drawn in `fg`. The text is first clamped to the budget by display
/// width — never splitting a wide glyph — so the row is always exactly `width`
/// display columns even when a caller passes oversized text. The remaining
/// padding is split left/right to center it.
fn text_row(text: &str, fg: Rgb, width: usize) -> String {
    let clamped = clamped_to_width(text, width);
    let text_width = display_width_ignoring_ansi(&clamped);
    let left = (width - text_width) / 2;
    let right = width - text_width - left;
    let mut out = String::new();
    background_run(&mut out, left);
    if text_width > 0 {
        minimap::put_bg(&mut out, minimap::BG);
        minimap::put_fg(&mut out, fg);
        out.push_str(&clamped);
    }
    background_run(&mut out, right);
    out.push_str(minimap::RESET);
    out
}

/// Keep the leading run of `s` whose cumulative display width fits within
/// `width`, never splitting a wide glyph. `s` carries no embedded ANSI (callers
/// pass plain glyph/hint text), so a plain `UnicodeWidthChar` fold is the right
/// measure. This makes [`text_row`] exact regardless of the caller — a defensive
/// belt to the ladder's width budgeting, not a substitute for it.
fn clamped_to_width(s: &str, width: usize) -> String {
    s.chars()
        .scan(0usize, |used, c| {
            let next = *used + UnicodeWidthChar::width(c).unwrap_or(0);
            (next <= width).then(|| {
                *used = next;
                c
            })
        })
        .collect()
}

/// Append `count` background half-block cells to `out`.
fn background_run(out: &mut String, count: usize) {
    if count == 0 {
        return;
    }
    minimap::put_fg(out, minimap::BG);
    minimap::put_bg(out, minimap::BG);
    out.push_str(&"\u{2580}".repeat(count)); // ▀
}

/// Display width of `s` in terminal columns, skipping SGR/CSI escape sequences
/// (`ESC [ … final-byte`). A small three-state machine folded over the chars.
fn display_width_ignoring_ansi(s: &str) -> usize {
    #[derive(Clone, Copy)]
    enum Csi {
        Text,
        Esc,
        Inside,
    }
    s.chars()
        .fold((Csi::Text, 0usize), |(state, width), c| match state {
            Csi::Text if c == '\u{1b}' => (Csi::Esc, width),
            Csi::Text => (Csi::Text, width + UnicodeWidthChar::width(c).unwrap_or(0)),
            Csi::Esc if c == '[' => (Csi::Inside, width),
            Csi::Esc => (Csi::Text, width),
            Csi::Inside if ('\u{40}'..='\u{7e}').contains(&c) => (Csi::Text, width),
            Csi::Inside => (Csi::Inside, width),
        })
        .1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_palette() -> Palette {
        Palette::new(
            vec![(10, 20, 30), (40, 50, 60), (70, 80, 90)],
            (200, 100, 50),
        )
    }

    /// A single full-size pane — exercises every rung without layout noise.
    fn one_pane(title: &str) -> Vec<PaneRect> {
        vec![PaneRect::new(0, 0, 0, 100, 40, title, true)]
    }

    /// Independent display-width measure for assertions: strip CSI escapes, then
    /// sum char widths. Deliberately does NOT consult any width stored on the
    /// block, so it cross-checks the constructor's own column accounting.
    fn measured(line: &StyledLine) -> usize {
        line.as_str()
            .chars()
            .scan(0u8, |csi, c| {
                // 0 = text, 1 = saw ESC, 2 = inside CSI
                let column = match (*csi, c) {
                    (0, '\u{1b}') => {
                        *csi = 1;
                        0
                    }
                    (1, '[') => {
                        *csi = 2;
                        0
                    }
                    (1, _) => {
                        *csi = 0;
                        0
                    }
                    (2, c) if ('\u{40}'..='\u{7e}').contains(&c) => {
                        *csi = 0;
                        0
                    }
                    (2, _) => 0,
                    (_, c) => UnicodeWidthChar::width(c).unwrap_or(0),
                };
                Some(column)
            })
            .sum()
    }

    #[test]
    fn styled_line_width_skips_csi_sequences_and_lone_escapes() {
        // `width()` must count display columns only: a CSI color sequence
        // contributes nothing, and a lone ESC not followed by `[` drops back
        // to text without counting the aborting character. The same string
        // drives the test-local `measured` cross-check through its matching
        // ESC-without-bracket branch, keeping the two accountings in lockstep.
        let line = StyledLine("\u{1b}[38;2;1;2;3mab\u{1b}Zcd".to_string());
        assert_eq!(line.width(), 4);
        assert_eq!(measured(&line), 4);
    }

    #[test]
    fn selects_l0_for_a_wide_active_tab() {
        assert_eq!(level_for(24), Level::L0);
        assert_eq!(level_for(L0_MIN), Level::L0);
    }

    #[test]
    fn selects_l1_at_a_medium_width() {
        assert_eq!(level_for(12), Level::L1);
        assert_eq!(level_for(L1_MIN), Level::L1);
        assert_eq!(level_for(L0_MIN - 1), Level::L1);
    }

    #[test]
    fn selects_l2_for_grid_only_width() {
        assert_eq!(level_for(7), Level::L2);
        assert_eq!(level_for(L2_MIN), Level::L2);
        assert_eq!(level_for(L1_MIN - 1), Level::L2);
    }

    #[test]
    fn selects_l3_for_single_glyph_width() {
        assert_eq!(level_for(4), Level::L3);
        assert_eq!(level_for(L3_MIN), Level::L3);
        assert_eq!(level_for(L2_MIN - 1), Level::L3);
    }

    #[test]
    fn selects_l4_for_extremely_narrow_width() {
        assert_eq!(level_for(2), Level::L4);
        assert_eq!(level_for(L3_MIN - 1), Level::L4);
        assert_eq!(level_for(0), Level::L4);
    }

    #[test]
    fn ladder_is_monotonic_in_width() {
        // Richness never decreases as the budget grows: a wider tab is always at
        // least as detailed as a narrower one.
        let richness: Vec<u8> = (0..=40).map(|w| level_for(w).richness()).collect();
        assert!(
            richness.windows(2).all(|pair| pair[0] <= pair[1]),
            "richness must be non-decreasing in width: {richness:?}"
        );
    }

    #[test]
    fn block_is_always_three_rows() {
        let palette = test_palette();
        // The `[_; ROWS]` type already guarantees the *count*; this guards the
        // *content* — every rung must fill all three rows with rendered bytes
        // rather than leave one blank. Sweep one width per rung plus the
        // degenerate zero.
        for width in [0, 2, 4, 7, 12, 24] {
            let block = assemble(
                &one_pane("cargo"),
                &palette,
                width,
                0,
                "\u{2318}",
                GradientMode::Off,
                false,
            );
            for (row, line) in block.lines.iter().enumerate() {
                assert!(
                    !line.as_str().is_empty(),
                    "width {width}, row {row}: row must carry rendered bytes, not be blank"
                );
            }
        }
    }

    #[test]
    fn block_matches_budgeted_width() {
        let palette = test_palette();
        // Width accounting must hold independent of pane shape, so sweep several
        // layouts: a lone pane, side-by-side columns, a 2×2 grid, and a CJK title
        // (whose 2-column glyphs overlay via continuation cells at the grid
        // rungs, #57). Each must fill exactly the budget on every row.
        let layouts: [(&str, Vec<PaneRect>); 4] = [
            ("single ascii", one_pane("cargo build")),
            (
                "side-by-side columns",
                vec![
                    PaneRect::new(0, 0, 0, 50, 40, "nvim", true),
                    PaneRect::new(1, 50, 0, 50, 40, "cargo", false),
                ],
            ),
            (
                "2x2 grid",
                vec![
                    PaneRect::new(0, 0, 0, 50, 20, "a", false),
                    PaneRect::new(1, 50, 0, 50, 20, "b", false),
                    PaneRect::new(2, 0, 20, 50, 20, "c", true),
                    PaneRect::new(3, 50, 20, 50, 20, "d", false),
                ],
            ),
            ("cjk title", one_pane("\u{5b9f}\u{88c5}\u{4e2d}")), // 実装中
        ];
        // One width per rung, plus boundaries, plus the degenerate zero.
        for (name, panes) in &layouts {
            for width in [0, 1, 2, 3, 4, 5, 9, 10, 15, 16, 20, 24] {
                let block = assemble(
                    panes,
                    &palette,
                    width,
                    3,
                    "\u{2318}",
                    GradientMode::Off,
                    false,
                );
                for (row, line) in block.lines.iter().enumerate() {
                    assert_eq!(
                        measured(line),
                        width,
                        "layout {name:?}, width {width}, row {row}: display width must equal the budget"
                    );
                }
            }
        }
    }

    #[test]
    fn shortcut_hint_uses_position_plus_one() {
        // 0-based position renders 1-based to match GoToTab N.
        assert_eq!(hint_text(0, "\u{2318}"), "\u{2318}1");
        assert_eq!(hint_text(4, "\u{2318}"), "\u{2318}5");
    }

    #[test]
    fn shortcut_prefix_is_configurable() {
        assert_eq!(hint_text(2, "Cmd+"), "Cmd+3");
        assert_eq!(hint_text(0, "g"), "g1");
    }

    #[test]
    fn position_ten_and_beyond_drops_prefix() {
        // Tab 10 onward (1-based) has no `Super N` binding, so the prefix goes.
        assert_eq!(hint_text(9, "\u{2318}"), "10");
        assert_eq!(hint_text(10, "\u{2318}"), "11");
    }

    #[test]
    fn l4_hint_fits_budget_with_a_long_prefix() {
        // "Cmd+3" is 5 columns but the L4 slot is 2: drop the prefix to "3" and
        // keep the row exactly at budget rather than overflowing and wrapping.
        let palette = test_palette();
        let block = assemble(
            &one_pane("x"),
            &palette,
            2,
            2,
            "Cmd+",
            GradientMode::Off,
            false,
        );
        for line in &block.lines {
            assert_eq!(measured(line), 2);
        }
        // The bare number survives in the middle row.
        assert!(block.lines[1].as_str().contains('3'));
        assert!(!block.lines[1].as_str().contains('C'));
    }

    #[test]
    fn l4_truncates_number_when_even_bare_overflows() {
        // The pathological tail of the fit rule: tab 10 (position 9) has no
        // prefix, so the bare hint is "10" — two columns. In a one-column slot
        // even that overflows, so the number itself is truncated to "1" rather
        // than wrapping past the pane edge. Last resort, but width stays exact.
        let palette = test_palette();
        let block = assemble(
            &one_pane("x"),
            &palette,
            1,
            9,
            "Cmd+",
            GradientMode::Off,
            false,
        );
        for line in &block.lines {
            assert_eq!(measured(line), 1, "a 1-column slot must stay 1 column");
        }
        assert!(
            block.lines[1].as_str().contains('1'),
            "the truncated leading digit survives in the middle row"
        );
    }

    #[test]
    fn vertical_stack_degrades_to_color_only() {
        // Three stacked panes at an L0 width: the minimap drops every label
        // (each region is one text row tall), so no label char survives even at
        // the richest rung. Degradation is delegated to the minimap's gate.
        let panes = vec![
            PaneRect::new(0, 0, 0, 100, 10, "aaa", false),
            PaneRect::new(1, 0, 10, 100, 10, "bbb", false),
            PaneRect::new(2, 0, 20, 100, 10, "ccc", true),
        ];
        let block = assemble(
            &panes,
            &test_palette(),
            20,
            0,
            "\u{2318}",
            GradientMode::Off,
            false,
        );
        for line in &block.lines {
            for ch in ['a', 'b', 'c'] {
                assert!(
                    !line.as_str().contains(ch),
                    "deep stack must render color-only, found {ch:?}"
                );
            }
        }
    }

    #[test]
    fn focused_pane_label_preferred_at_l1() {
        // Two side-by-side panes at an L1 width: only the focused pane is
        // labeled. "alpha" (focused) shows; "bravo" (not) is dropped — proving
        // L1 maps to LabelMode::Focused, not All.
        let panes = vec![
            PaneRect::new(0, 0, 0, 50, 40, "alpha", true),
            PaneRect::new(1, 50, 0, 50, 40, "bravo", false),
        ];
        let block = assemble(
            &panes,
            &test_palette(),
            12,
            0,
            "\u{2318}",
            GradientMode::Off,
            false,
        );
        let joined: String = block.lines.iter().map(StyledLine::as_str).collect();
        assert!(joined.contains('a'), "focused pane's label should appear");
        assert!(
            !joined.contains('b'),
            "non-focused pane's label must be dropped at L1"
        );
    }

    #[test]
    fn grid_rungs_stamp_the_shortcut_badge() {
        // A grid rung (here L0) stamps the "⌘N" shortcut as an in-block badge so
        // the shortcut shows on comfortably-sized tabs, not only on the narrow
        // L4 hint. The ⌘ glyph is the witness: a color escape carries only
        // digits, so the badge digit can't be told from the grid, but ⌘ is
        // unique to the badge. The narrow rungs (L3 glyph, L4 hint) carry the
        // number themselves, so the badge wiring only needs locking on a grid
        // rung.
        let palette = test_palette();
        let block = assemble(
            &one_pane("shell"),
            &palette,
            16,
            0,
            "\u{2318}",
            GradientMode::Off,
            false,
        );
        let joined: String = block.lines.iter().map(StyledLine::as_str).collect();
        assert!(
            joined.contains('\u{2318}'),
            "an L0 grid must stamp the ⌘ badge inside the block"
        );
    }

    #[test]
    fn active_block_badge_is_white_inactive_is_muted() {
        // #59: the active tab's badge renders in pure white on its vivid fill —
        // maximum contrast. An inactive block mutes the badge text toward the
        // pane fill so it recedes. No accent chip anywhere.
        let palette = test_palette();
        let block_for = |active: bool| -> String {
            assemble(
                &vec![PaneRect::new(0, 0, 0, 100, 40, "shell", false)],
                &palette,
                16,
                0,
                "\u{2318}",
                GradientMode::Off,
                active,
            )
            .lines
            .iter()
            .map(StyledLine::as_str)
            .collect()
        };
        let white_fg = {
            let (r, g, b) = minimap::ACTIVE_FG;
            format!("\x1b[38;2;{r};{g};{b}m")
        };
        let accent_bg = {
            let (r, g, b) = palette.accent();
            format!("\x1b[48;2;{r};{g};{b}m")
        };
        // Inactive badge is blended toward the pane fill (no ring on unfocused pane).
        let muted_fg = {
            let fill = palette.color_for(0);
            let c = crate::color::mixed(minimap::ACTIVE_FG, fill, minimap::INACTIVE_LABEL_BLEND);
            format!("\x1b[38;2;{};{};{}m", c.0, c.1, c.2)
        };
        let active = block_for(true);
        let inactive = block_for(false);
        assert!(
            active.contains(&white_fg),
            "the active block's badge text must be white — stands out on vivid fill (#59)"
        );
        assert!(
            !active.contains(&accent_bg),
            "the active block must not paint an accent chip"
        );
        assert!(
            inactive.contains(&muted_fg),
            "an inactive badge must be muted toward the pane fill (#59)"
        );
        assert!(
            !inactive.contains(&white_fg),
            "an inactive badge must not be pure white — it should be subdued"
        );
    }

    #[test]
    fn narrow_rungs_are_white_when_active_muted_when_inactive() {
        // #59: the L3 glyph and L4 hint are the tab's only text at narrow
        // widths. Active: pure white text on vivid fill — stands out. Inactive:
        // text muted toward the canvas fill — recedes gracefully.
        let palette = test_palette();
        let white_fg = {
            let (r, g, b) = minimap::ACTIVE_FG;
            format!("\x1b[38;2;{r};{g};{b}m")
        };
        // Inactive rung text is blended toward the canvas (BG).
        let muted_fg = {
            let c = crate::color::mixed(
                minimap::ACTIVE_FG,
                minimap::BG,
                minimap::INACTIVE_LABEL_BLEND,
            );
            format!("\x1b[38;2;{};{};{}m", c.0, c.1, c.2)
        };
        for width in [4, 2] {
            let row_for = |active: bool| -> String {
                assemble(
                    &one_pane("x"),
                    &palette,
                    width,
                    0,
                    "\u{2318}",
                    GradientMode::Off,
                    active,
                )
                .lines[1]
                    .as_str()
                    .to_string()
            };
            assert!(
                row_for(true).contains(&white_fg),
                "width {width}: the active rung text must be white (#59)"
            );
            assert!(
                row_for(false).contains(&muted_fg),
                "width {width}: an inactive rung text must be muted toward the canvas (#59)"
            );
        }
    }

    #[test]
    fn render_is_deterministic() {
        let palette = test_palette();
        let panes = vec![
            PaneRect::new(0, 0, 0, 50, 40, "nvim", true),
            PaneRect::new(1, 50, 0, 50, 40, "cargo", false),
        ];
        for width in [2, 4, 7, 12, 24] {
            let first = assemble(
                &panes,
                &palette,
                width,
                1,
                "\u{2318}",
                GradientMode::Off,
                false,
            );
            let second = assemble(
                &panes,
                &palette,
                width,
                1,
                "\u{2318}",
                GradientMode::Off,
                false,
            );
            assert_eq!(first, second, "width {width} must render identically");
        }
    }

    #[test]
    fn render_is_order_independent_for_disjoint_panes() {
        // Determinism must not depend on the order zellij lists panes in. Fill
        // and label are keyed on each pane's stable id and its projected region,
        // not its slice index — so for two non-overlapping panes, reversing the
        // list must produce byte-identical output. (A shared boundary cell would
        // be the only order-sensitive case; these columns are disjoint by
        // construction.)
        let palette = test_palette();
        let left = PaneRect::new(7, 0, 0, 50, 40, "nvim", true);
        let right = PaneRect::new(13, 50, 0, 50, 40, "cargo", false);
        let forward = [left.clone(), right.clone()];
        let reversed = [right, left];
        for width in [4, 12, 24] {
            let a = assemble(
                &forward,
                &palette,
                width,
                1,
                "\u{2318}",
                GradientMode::Off,
                false,
            );
            let b = assemble(
                &reversed,
                &palette,
                width,
                1,
                "\u{2318}",
                GradientMode::Off,
                false,
            );
            assert_eq!(
                a, b,
                "width {width}: pane list order must not change the render"
            );
        }
    }

    #[test]
    fn representative_glyph_reflects_layout() {
        let single = vec![PaneRect::new(0, 0, 0, 100, 40, "", true)];
        let columns = vec![
            PaneRect::new(0, 0, 0, 50, 40, "", false),
            PaneRect::new(1, 50, 0, 50, 40, "", false),
        ];
        let rows = vec![
            PaneRect::new(0, 0, 0, 100, 20, "", false),
            PaneRect::new(1, 0, 20, 100, 20, "", false),
        ];
        let grid = vec![
            PaneRect::new(0, 0, 0, 50, 20, "", false),
            PaneRect::new(1, 50, 0, 50, 20, "", false),
            PaneRect::new(2, 0, 20, 50, 20, "", false),
            PaneRect::new(3, 50, 20, 50, 20, "", false),
        ];
        // Each layout maps to a distinct glyph.
        let glyphs = [
            representative_glyph(&single),
            representative_glyph(&columns),
            representative_glyph(&rows),
            representative_glyph(&grid),
        ];
        let unique: std::collections::BTreeSet<_> = glyphs.iter().collect();
        assert_eq!(
            unique.len(),
            4,
            "distinct layouts get distinct glyphs: {glyphs:?}"
        );
    }

    #[test]
    fn display_width_ignores_ansi_escapes() {
        // A truecolor-styled "ab" plus reset measures 2, not the escape bytes.
        let styled = "\x1b[38;2;1;2;3m\x1b[48;2;4;5;6mab\x1b[0m";
        assert_eq!(display_width_ignoring_ansi(styled), 2);
        // A wide CJK char counts as two columns.
        assert_eq!(display_width_ignoring_ansi("\u{5b9f}"), 2); // 実
    }
}

//! Per-tab compositor — assemble one tab into a block of an exact budgeted
//! width and a caller-chosen row height.
//!
//! [`crate::line::pack`] hands each tab a column budget; this module turns that
//! budget plus the tab's panes into `rows` rendered rows via the L0–L4
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
//! exact-height / exact-width contract. Per-pane label degradation (a too-narrow
//! or too-short pane, or a deep vertical stack) is the minimap's concern and
//! happens underneath every grid rung.
//!
//! A [`StyledLine`] is the concrete form of the design's `StyledLine`: an
//! ANSI-styled string whose *display* width (escape sequences excluded) equals
//! the block's budgeted width. Lines are **raw** — they carry color but no
//! cursor positioning; framing into the plugin pane stays in
//! [`crate::paint::compose`] so width accounting and the layer boundary hold.

use crate::color::{self, Palette, Rgb};
use crate::minimap::{self, Close, GradientSpec, LabelMode, PaneRect};
use unicode_width::UnicodeWidthChar;

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

/// One tab rendered as a block of exactly `width` columns and a caller-chosen
/// number of rows.
///
/// A tab is identified by its `position` (zellij's `TabInfo.position`); there is
/// no separate stable tab id the way panes have one, and click-to-switch maps a
/// column back to a position, so position is the whole identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabBlock {
    /// The rendered rows, top to bottom — one per requested row.
    pub lines: Vec<StyledLine>,
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

/// Pixel rows of background to inset top and bottom of a grid block — the
/// perspective depth cue (#66): one (a half text row) for an *inactive* block
/// when perspective is on and the bar is at least four rows tall, none otherwise
/// (the active tab and short bars fill their full height). Single source for both
/// [`assemble`] (which paints with it) and the click-to-focus geometry record
/// (#74, `src/lib.rs`), so a pane hit-test insets exactly as the paint did.
pub fn vinset_for(perspective: bool, rows: usize, active: bool) -> usize {
    usize::from(perspective && rows >= 4 && !active)
}

/// Assemble `panes` into a `rows`-tall block of exactly `width` columns at tab
/// `position`, choosing detail via the ladder. `prefix` is the configured
/// shortcut glyph, used only by the L4 hint rung. `gradient` is the configured
/// fill sweep, used only by the grid rungs (L0–L2). `active` marks the bar's
/// selected tab: its shortcut text renders white and its focus ring draws,
/// while inactive blocks suppress the focus highlight (#59) — dimming the
/// *inactive* blocks is the caller's concern, applied through the palette it
/// hands in.
///
/// `perspective` is the depth cue (#66): when on **and** the bar is at least
/// four rows tall, every *inactive* grid block recedes by one row — a half-row
/// of background inset top and bottom — so the full-height active tab floats
/// forward. Below four rows, or for the active tab, the block fills its full
/// height (the historical look). The narrow rungs (L3 glyph, L4 hint) already
/// ride a blank-framed middle row, so the cue applies only to the grid rungs.
///
/// `close` ([`Close`]) stamps the top-right close affordance (#86); the minimap
/// reserves its cell(s) from the badge and label. Only the grid rungs (L0–L2)
/// honor it — the narrow rungs have no room and drop it like the rest of their
/// detail. The caller passes an on-variant only when the close button is enabled
/// *and* more than one tab is open (so the last tab keeps no close target), and
/// records the matching click cell against the live frame.
#[allow(clippy::too_many_arguments)]
pub fn assemble(
    panes: &[PaneRect],
    palette: &Palette,
    width: usize,
    rows: usize,
    position: usize,
    prefix: &str,
    gradient: GradientSpec,
    active: bool,
    perspective: bool,
    close: Close,
    floats: crate::floating::FloatLayer<'_>,
    suppressed_covers: &[usize],
    pinned_floats: &[usize],
) -> TabBlock {
    // Pixel rows of background to inset top and bottom of the minimap canvas: one
    // (a half text row) for an inactive block in perspective mode at ≥4 rows,
    // none otherwise. The minimap centers the panes in the shorter band.
    let vinset = vinset_for(perspective, rows, active);
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
            rows,
            vinset,
            LabelMode::All,
            badge,
            close,
            gradient,
            active,
            floats,
            suppressed_covers,
            pinned_floats,
        ),
        Level::L1 => grid_lines(
            panes,
            palette,
            width,
            rows,
            vinset,
            LabelMode::Focused,
            badge,
            close,
            gradient,
            active,
            floats,
            suppressed_covers,
            pinned_floats,
        ),
        Level::L2 => grid_lines(
            panes,
            palette,
            width,
            rows,
            vinset,
            LabelMode::None,
            badge,
            close,
            gradient,
            active,
            floats,
            suppressed_covers,
            pinned_floats,
        ),
        // The narrow rungs (L3 glyph, L4 hint) have no room for a close "×" — it
        // degrades away with the label and grid, so `close` is unused here (#86).
        Level::L3 => glyph_lines(panes, width, rows, active),
        Level::L4 => hint_lines(position, prefix, width, rows, active),
    };
    TabBlock {
        lines,
        width,
        position,
    }
}

/// Assemble the inline new-tab `+` button (#76) as a block of exactly `width`
/// columns and `rows` rows, sized to read as one of the **inactive** tabs beside
/// it: a muted [`color::button_fill`] band with a `+` (in [`color::button_glyph`])
/// centered on the vertical-center row — the same middle row
/// [`crate::paint::compose`] homes the overflow markers and the narrow-rung text
/// on, so the glyph lines up with the tab labels.
///
/// `perspective` mirrors the inactive grid block's depth recede (#66): when on
/// **and** the bar is at least four rows tall, the fill insets a half-row top and
/// bottom, so the button stands exactly as tall as the receded inactive tabs
/// rather than floating at the full-height active tab's height. Below four rows,
/// or with perspective off, the band fills its full height — which the inactive
/// tabs also do in that regime, so the size match holds either way. The `+` rides
/// the middle row, which is never a recede row at ≥4 rows (`rows / 2` is neither
/// the first nor the last), so the glyph keeps a solid band beneath it.
///
/// The recede inset is *transparent* (the terminal default background), not a
/// painted canvas color: a `BG`-painted inset read as a hard top/bottom frame
/// against the flat fill (#84). With the inset transparent the flat fill recedes
/// cleanly into the bar backdrop, so the button stays flat — no gradient — which
/// also keeps it visually distinct from the gradient-swept tabs.
///
/// Returns a [`TabBlock`] so [`crate::paint::bar`] can place it through the same
/// `compose` path as the tabs. The button carries no tab identity, so its
/// `position` is an inert placeholder: `compose` positions a block by the column
/// it is handed, never by the block's own `position`.
pub fn button_block(width: usize, rows: usize, perspective: bool) -> TabBlock {
    // The half-row (one half-block pixel) reserved at each end for an inactive
    // block in perspective mode at ≥4 rows — exactly `assemble`'s gate and inset,
    // so the button recedes in lockstep with the inactive tabs.
    let vinset = vinset_for(perspective, rows, false);
    let lines = minimap::button(width, rows, vinset)
        .lines()
        .map(|line| StyledLine(line.to_string()))
        .collect();
    TabBlock {
        lines,
        width,
        position: 0,
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
    rows: usize,
    vinset: usize,
    mode: LabelMode,
    badge: Option<&str>,
    close: Close,
    gradient: GradientSpec,
    active: bool,
    floats: crate::floating::FloatLayer<'_>,
    suppressed_covers: &[usize],
    pinned_floats: &[usize],
) -> Vec<StyledLine> {
    let block = minimap::render(
        panes,
        palette,
        width,
        rows,
        vinset,
        mode,
        badge,
        close,
        gradient,
        active,
        floats,
        suppressed_covers,
        pinned_floats,
    );
    padded_rows(block.lines().map(str::to_string), width, rows)
}

/// L3: a single representative split/grid glyph centered on the canvas.
fn glyph_lines(panes: &[PaneRect], width: usize, rows: usize, active: bool) -> Vec<StyledLine> {
    let glyph = representative_glyph(panes);
    centered_text_block(&glyph.to_string(), rung_text_fg(active), width, rows)
}

/// L4: the shortcut hint only, fitted to the budget, centered on the canvas.
fn hint_lines(
    position: usize,
    prefix: &str,
    width: usize,
    rows: usize,
    active: bool,
) -> Vec<StyledLine> {
    let hint = fit_hint(position, prefix, width);
    centered_text_block(&hint, rung_text_fg(active), width, rows)
}

/// A `rows`-tall, `width`-wide background canvas with `text` (drawn in `fg`) on
/// the vertical-center row and every other row blank. Shared by the narrow rungs
/// (L3 glyph, L4 hint), whose single text element rides the middle row — the
/// same row [`crate::paint::compose`] homes the overflow markers on.
fn centered_text_block(text: &str, fg: Rgb, width: usize, rows: usize) -> Vec<StyledLine> {
    let middle = rows / 2;
    (0..rows)
        .map(|row| match row == middle {
            true => StyledLine(text_row(text, fg, width)),
            false => StyledLine(blank_row(width)),
        })
        .collect()
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

/// Collect exactly `count` rendered rows: take the rendered ones in order and
/// backfill any shortfall with blank canvas rows (and drop any excess) so the
/// block's height always matches the budget. The minimap yields one row per
/// requested text row, so the backfill is a defensive belt, not the normal path.
fn padded_rows(rows: impl Iterator<Item = String>, width: usize, count: usize) -> Vec<StyledLine> {
    rows.map(Some)
        .chain(std::iter::repeat(None))
        .take(count)
        .map(|row| {
            row.map(StyledLine)
                .unwrap_or_else(|| StyledLine(blank_row(width)))
        })
        .collect()
}

/// A full-width row of solid `fill` half-blocks, SGR-reset at the end. The
/// general form behind [`blank_row`] (canvas fill) and the new-tab button's
/// muted-fill rows (#76).
fn fill_row(fill: Rgb, width: usize) -> String {
    let mut out = String::new();
    solid_run(&mut out, fill, width);
    out.push_str(minimap::RESET);
    out
}

/// A full-width row of background half-blocks — the empty canvas (the
/// canvas-colored [`fill_row`]).
fn blank_row(width: usize) -> String {
    fill_row(minimap::BG, width)
}

/// `text` (no embedded ANSI) centered on the background canvas `width` columns
/// wide, drawn in `fg` — the canvas-colored [`text_row_on`], used by the narrow
/// rungs (L3 glyph, L4 hint).
fn text_row(text: &str, fg: Rgb, width: usize) -> String {
    text_row_on(text, fg, minimap::BG, width)
}

/// `text` (no embedded ANSI) centered on a solid `fill` canvas `width` columns
/// wide, drawn in `fg`. Generalizes [`text_row`] (whose canvas is [`minimap::BG`])
/// for the new-tab button's muted fill (#76). The text is first clamped to the
/// budget by display width — never splitting a wide glyph — so the row is always
/// exactly `width` display columns even when a caller passes oversized text. The
/// remaining padding is split left/right to center it.
fn text_row_on(text: &str, fg: Rgb, fill: Rgb, width: usize) -> String {
    let clamped = clamped_to_width(text, width);
    let text_width = display_width_ignoring_ansi(&clamped);
    let left = (width - text_width) / 2;
    let right = width - text_width - left;
    let mut out = String::new();
    solid_run(&mut out, fill, left);
    if text_width > 0 {
        minimap::put_bg(&mut out, fill);
        minimap::put_fg(&mut out, fg);
        out.push_str(&clamped);
    }
    solid_run(&mut out, fill, right);
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

/// Append `count` solid-fill cells of `fill` to `out`: a run of `fill`-on-`fill`
/// half-blocks, which read as a solid color band. The canvas-colored case
/// (`fill = minimap::BG`) is the empty bar background; the new-tab button fills
/// with its own muted color (#76).
fn solid_run(out: &mut String, fill: Rgb, count: usize) {
    if count == 0 {
        return;
    }
    minimap::put_fg(out, fill);
    minimap::put_bg(out, fill);
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
    fn grid_rung_draws_hidden_float_chips() {
        // An L0 grid rung with two hidden floats stamps the chip glyph; the width
        // contract still holds on every row (chips ride reserved corner cells and
        // never widen the block). The caller only hands chips to grid rungs, the
        // same way labels/badges degrade on narrow rungs.
        let palette = test_palette();
        let hidden = [7usize, 9usize];
        let block = assemble(
            &one_pane("shell"),
            &palette,
            16,
            3,
            0,
            "\u{2318}",
            GradientSpec::OFF,
            true,
            false,
            Close::Off,
            crate::floating::FloatLayer::Hidden(&hidden),
            &[],
            &[],
        );
        let joined: String = block.lines.iter().map(StyledLine::as_str).collect();
        assert!(
            joined.contains(crate::floating::CHIP_GLYPH),
            "L0 rung stamps chips"
        );
        for line in &block.lines {
            assert_eq!(measured(line), 16);
        }
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
    fn block_fills_every_requested_row() {
        let palette = test_palette();
        // The block must own exactly the rows it is asked for — `MIN_ROWS` (3)
        // and taller (#66 step 2 makes the height runtime, no longer a fixed 3) —
        // and every rung must fill each of them with rendered bytes rather than
        // leave one blank. Sweep one width per rung plus the degenerate zero,
        // across a range of row counts.
        for rows in [3, 4, 5, 6] {
            for width in [0, 2, 4, 7, 12, 24] {
                let block = assemble(
                    &one_pane("cargo"),
                    &palette,
                    width,
                    rows,
                    0,
                    "\u{2318}",
                    GradientSpec::OFF,
                    false,
                    false,
                    Close::Off,
                    crate::floating::FloatLayer::None,
                    &[],
                    &[],
                );
                assert_eq!(
                    block.lines.len(),
                    rows,
                    "rows {rows}, width {width}: the block must have exactly the requested height"
                );
                for (row, line) in block.lines.iter().enumerate() {
                    assert!(
                        !line.as_str().is_empty(),
                        "rows {rows}, width {width}, row {row}: row must carry rendered bytes, not be blank"
                    );
                }
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
        // One width per rung, plus boundaries, plus the degenerate zero — and
        // across a range of heights, since #66 makes the row count runtime and a
        // taller block renders more rows that must each still fill the budget.
        for rows in [3, 4, 5, 6] {
            for (name, panes) in &layouts {
                for width in [0, 1, 2, 3, 4, 5, 9, 10, 15, 16, 20, 24] {
                    let block = assemble(
                        panes,
                        &palette,
                        width,
                        rows,
                        3,
                        "\u{2318}",
                        GradientSpec::OFF,
                        false,
                        false,
                        Close::Off,
                        crate::floating::FloatLayer::None,
                        &[],
                        &[],
                    );
                    for (row, line) in block.lines.iter().enumerate() {
                        assert_eq!(
                            measured(line),
                            width,
                            "rows {rows}, layout {name:?}, width {width}, row {row}: display width must equal the budget"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn perspective_recedes_inactive_blocks_only_at_four_or_more_rows() {
        // #66 step 3: perspective recedes an *inactive* grid block by a half-row
        // top and bottom — but only once the bar is at least four rows tall.
        // Isolate the cue by toggling it on the same inactive block: it changes
        // a 4-row block (the recede) yet is inert at 3 rows (the gate).
        let palette = test_palette();
        let panes = one_pane("server");
        let inactive = |rows, perspective| {
            assemble(
                &panes,
                &palette,
                16,
                rows,
                0,
                "\u{2318}",
                GradientSpec::OFF,
                false,
                perspective,
                Close::Off,
                crate::floating::FloatLayer::None,
                &[],
                &[],
            )
            .lines
        };
        assert_ne!(
            inactive(4, true),
            inactive(4, false),
            "perspective must recede a 4-row inactive block"
        );
        assert_eq!(
            inactive(3, true),
            inactive(3, false),
            "perspective must be a no-op below four rows"
        );
    }

    #[test]
    fn perspective_leaves_the_active_block_full_height() {
        // The active tab keeps full height under perspective — that height
        // contrast against the receded inactive tabs *is* the depth cue — so the
        // cue produces an identical active block whether it is on or off.
        let palette = test_palette();
        let panes = one_pane("server");
        let active = |perspective| {
            assemble(
                &panes,
                &palette,
                16,
                4,
                0,
                "\u{2318}",
                GradientSpec::OFF,
                true,
                perspective,
                Close::Off,
                crate::floating::FloatLayer::None,
                &[],
                &[],
            )
            .lines
        };
        assert_eq!(
            active(true),
            active(false),
            "the active block fills full height regardless of perspective"
        );
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
            3,
            2,
            "Cmd+",
            GradientSpec::OFF,
            false,
            false,
            Close::Off,
            crate::floating::FloatLayer::None,
            &[],
            &[],
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
            3,
            9,
            "Cmd+",
            GradientSpec::OFF,
            false,
            false,
            Close::Off,
            crate::floating::FloatLayer::None,
            &[],
            &[],
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
            3,
            0,
            "\u{2318}",
            GradientSpec::OFF,
            false,
            false,
            Close::Off,
            crate::floating::FloatLayer::None,
            &[],
            &[],
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
            3,
            0,
            "\u{2318}",
            GradientSpec::OFF,
            false,
            false,
            Close::Off,
            crate::floating::FloatLayer::None,
            &[],
            &[],
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
            3,
            0,
            "\u{2318}",
            GradientSpec::OFF,
            false,
            false,
            Close::Off,
            crate::floating::FloatLayer::None,
            &[],
            &[],
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
                3,
                0,
                "\u{2318}",
                GradientSpec::OFF,
                active,
                false,
                Close::Off,
                crate::floating::FloatLayer::None,
                &[],
                &[],
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
                    3,
                    0,
                    "\u{2318}",
                    GradientSpec::OFF,
                    active,
                    false,
                    Close::Off,
                    crate::floating::FloatLayer::None,
                    &[],
                    &[],
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
                3,
                1,
                "\u{2318}",
                GradientSpec::OFF,
                false,
                false,
                Close::Off,
                crate::floating::FloatLayer::None,
                &[],
                &[],
            );
            let second = assemble(
                &panes,
                &palette,
                width,
                3,
                1,
                "\u{2318}",
                GradientSpec::OFF,
                false,
                false,
                Close::Off,
                crate::floating::FloatLayer::None,
                &[],
                &[],
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
                3,
                1,
                "\u{2318}",
                GradientSpec::OFF,
                false,
                false,
                Close::Off,
                crate::floating::FloatLayer::None,
                &[],
                &[],
            );
            let b = assemble(
                &reversed,
                &palette,
                width,
                3,
                1,
                "\u{2318}",
                GradientSpec::OFF,
                false,
                false,
                Close::Off,
                crate::floating::FloatLayer::None,
                &[],
                &[],
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

    // ---- button_block (inline new-tab "+" button, #76) -------------------

    #[test]
    fn button_block_fills_every_row_to_the_budget() {
        // The "+" button holds the same exact-height / exact-width contract as a
        // tab block — every one of `rows` rows fills the budget — so `compose`
        // lays it without a wider previous frame bleeding through. The recede
        // rows (perspective on) are still full display width: a half-block split
        // between fill and canvas occupies the cell exactly like a solid one.
        for perspective in [false, true] {
            for rows in [3, 4, 5, 6] {
                for width in [3, 5, 8] {
                    let block = button_block(width, rows, perspective);
                    assert_eq!(block.lines.len(), rows, "rows {rows}: exact height");
                    for (row, line) in block.lines.iter().enumerate() {
                        assert_eq!(
                            measured(line),
                            width,
                            "perspective {perspective}, rows {rows}, width {width}, row {row}: display width must equal the budget"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn button_block_centers_the_plus_on_the_middle_row() {
        // The glyph rides the vertical-center row (rows / 2) — the row `compose`
        // homes markers and rung text on — so the "+" lines up with the tab
        // labels. Every other row carries just the muted fill, no glyph.
        let rows = 4;
        let block = button_block(3, rows, false);
        let middle = rows / 2;
        assert!(
            block.lines[middle].as_str().contains('+'),
            "the + rides the middle row"
        );
        for (row, line) in block.lines.iter().enumerate() {
            if row != middle {
                assert!(!line.as_str().contains('+'), "row {row} carries no glyph");
            }
        }
    }

    #[test]
    fn button_block_paints_the_muted_fill_and_glyph_colors() {
        // The block is painted with the theme-derived muted button fill
        // (background) and the "+" in the button glyph foreground (#76) — the
        // quiet affordance, never a tab's vivid pane fill.
        let fill_bg = {
            let (r, g, b) = crate::color::button_fill();
            format!("\x1b[48;2;{r};{g};{b}m")
        };
        let glyph_fg = {
            let (r, g, b) = crate::color::button_glyph();
            format!("\x1b[38;2;{r};{g};{b}m")
        };
        let block = button_block(3, 4, false);
        let joined: String = block.lines.iter().map(StyledLine::as_str).collect();
        assert!(
            joined.contains(&fill_bg),
            "every row is painted with the muted button fill"
        );
        assert!(
            joined.contains(&glyph_fg),
            "the + is drawn in the button glyph foreground"
        );
    }

    #[test]
    fn button_block_recedes_like_an_inactive_block_under_perspective() {
        // #76 size match: the button must read as one of the *inactive* tabs, so
        // it takes the same perspective recede an inactive grid block does (#66)
        // — a half-row inset top and bottom at ≥4 rows. The inset is *transparent*
        // (SGR 49, terminal default), not a painted canvas band (#84): the witness
        // is the top and bottom rows resetting to the default background over the
        // button fill, while a full-height button carries the solid fill with no
        // reset. Like `assemble`'s cue, the recede is gated on a four-row bar —
        // inert at three rows.
        let transparent = "\x1b[49m";
        let fill_fg = {
            let (r, g, b) = crate::color::button_fill();
            format!("\x1b[38;2;{r};{g};{b}m")
        };
        let fill_bg = {
            let (r, g, b) = crate::color::button_fill();
            format!("\x1b[48;2;{r};{g};{b}m")
        };
        let recede = button_block(5, 4, true);
        let full = button_block(5, 4, false);
        let last = recede.lines.len() - 1;
        // Top row `▄`: transparent upper half, button fill in the lower.
        assert!(
            recede.lines[0]
                .as_str()
                .starts_with(&format!("{transparent}{fill_fg}\u{2584}")),
            "the receded top row insets a transparent half-row above the fill"
        );
        // Bottom row `▀`: button fill in the upper half, transparent lower.
        assert!(
            recede.lines[last]
                .as_str()
                .starts_with(&format!("{transparent}{fill_fg}\u{2580}")),
            "the receded bottom row insets a transparent half-row below the fill"
        );
        // Full-height: the top row is solid fill with no transparent inset.
        assert!(
            full.lines[0].as_str().contains(&fill_bg)
                && !full.lines[0].as_str().contains(transparent),
            "with perspective off the top row is solid fill, no transparent inset"
        );
        // The cue is gated on a four-row bar: a three-row button never recedes,
        // matching `assemble`'s inactive-block gate.
        assert_eq!(
            button_block(5, 3, true).lines,
            button_block(5, 3, false).lines,
            "below four rows perspective must be a no-op for the button too"
        );
    }
}

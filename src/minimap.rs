//! Dependency-free minimap renderer.
//!
//! Turns a tab's pane layout into a compact, color-coded half-block grid. Each
//! text row holds two vertical "pixels" via the upper-half-block glyph `▀`
//! (U+2580): the foreground color paints the top pixel and the background color
//! the bottom one, doubling vertical resolution (a 3-text-row block is a 6-pixel
//! grid). Pane titles are overlaid where width allows and degrade gracefully —
//! labels that cannot fit are dropped rather than truncated into noise.
//!
//! This module has no zellij dependency, so it is unit-tested natively. The
//! plugin layer ([`crate::State`]) maps zellij's `PaneInfo` into [`PaneRect`]
//! and prints the ANSI string this module returns. Per-pane colors come from
//! the theme-derived [`Palette`], keyed on each pane's stable id.

use unicode_width::UnicodeWidthChar;

use crate::color::Palette;
// Re-exported so the historical `minimap::Rgb` path keeps resolving (the
// canonical definition lives in `crate::color`).
pub use crate::color::Rgb;

/// Tokyonight background (`#1a1b26`) — painted on empty space. Shared with
/// [`crate::tab_block`], which paints its glyph/hint rungs on the same canvas.
/// Canonical value lives in [`crate::color::CANVAS`] (the dim target, #59).
pub(crate) const BG: Rgb = crate::color::CANVAS;
/// Dark text color for labels drawn over a (light) pane fill.
const LABEL_FG: Rgb = (16, 17, 26);

pub(crate) const RESET: &str = "\x1b[0m";

/// A pane's position and size in terminal cells, plus display metadata.
///
/// Coordinates are absolute terminal cells, but only *relative* positions
/// matter: [`render`] normalizes every pane against the group's bounding box,
/// so the coordinate origin is irrelevant. `id` is the pane's stable identity
/// (from zellij's `PaneInfo.id`); it is the color key, so a pane keeps its
/// color across repaints even as its position in the list changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneRect {
    pub id: usize,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub title: String,
    pub focused: bool,
}

impl PaneRect {
    pub fn new(
        id: usize,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        title: impl Into<String>,
        focused: bool,
    ) -> Self {
        Self {
            id,
            x,
            y,
            w,
            h,
            title: title.into(),
            focused,
        }
    }
}

// ---- ANSI emission ------------------------------------------------------

pub(crate) fn put_fg(out: &mut String, c: Rgb) {
    out.push_str(&format!("\x1b[38;2;{};{};{}m", c.0, c.1, c.2));
}

pub(crate) fn put_bg(out: &mut String, c: Rgb) {
    out.push_str(&format!("\x1b[48;2;{};{};{}m", c.0, c.1, c.2));
}

/// Which pane titles to overlay as labels — the ladder's per-rung label policy.
///
/// The L0–L4 degradation ladder (see [`crate::tab_block`]) selects one of these
/// per tab from its budgeted width: `All` for a wide active tab, `Focused` for a
/// medium one, `None` once the block is too narrow for readable text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabelMode {
    /// Overlay no labels — render the color grid only.
    None,
    /// Overlay only the focused pane's label.
    Focused,
    /// Overlay every pane's label where width and height allow.
    All,
}

/// Gradient fill applied to each pane block (config key `gradient`, #40).
///
/// The sweep is *per pane block*: each filled column `x` of a pane spanning
/// `w` pixel columns is `mix(fill, stop, smoothstep(x / (w - 1)))`, where the
/// stop is the pane fill's luminance-shifted shade and the mix runs in
/// linear-light space ([`crate::color::gradient_at`], #46). Focus
/// ring, label glyphs, and badge glyphs stay solid; label/badge background
/// cells follow the sweep so text doesn't punch flat-colored holes in it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GradientMode {
    /// Flat fills — the historical look, byte-for-byte.
    #[default]
    Off,
    /// Sweep each pane block left-to-right from its base fill toward its stop.
    Sheen,
    /// Like `Sheen`, but the two half-block pixel rows of each text row sweep
    /// in opposite directions (top L→R, bottom R→L) — a woven texture.
    Weave,
}

impl std::str::FromStr for GradientMode {
    type Err = ();

    /// `"off"` / `"sheen"` / `"weave"` (exact match); any other value is an
    /// error so the config parser falls back to the documented default rather
    /// than panicking.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "off" => Ok(Self::Off),
            "sheen" => Ok(Self::Sheen),
            "weave" => Ok(Self::Weave),
            _ => Err(()),
        }
    }
}

/// The `⌘N` shortcut hint stamped into a block's top-left (#32), plus how its
/// text reads: `accented` paints it in the palette accent — the active tab's
/// cue (#59) — while a plain badge keeps the historical dark label color.
#[derive(Clone, Copy, Debug)]
pub struct Badge<'a> {
    /// The hint text (no embedded ANSI).
    pub text: &'a str,
    /// Whether the text draws in the palette accent instead of dark.
    pub accented: bool,
}

/// One label-overlay cell, placed by display column.
///
/// A glyph claims its leading cell plus one [`Continuation`] cell per extra
/// column it spans (a CJK glyph spans two), so the overlay stays in lockstep
/// with the terminal's cursor advance: the emitter paints the leading cell and
/// skips continuations, which the wide glyph already covers on screen.
///
/// [`Continuation`]: OverlayCell::Continuation
#[derive(Clone, Copy)]
enum OverlayCell {
    /// The leading cell of a glyph, carrying it and its pane's slice index.
    Glyph(char, usize),
    /// A cell covered by the trailing column(s) of a preceding wide glyph.
    Continuation,
}

// ---- core renderer ------------------------------------------------------

/// Fill color of pane `i` at pixel `(px, py)` — the base fill swept by the
/// active [`GradientMode`] across the pane's own column span (`bounds[i]`).
/// A one-column span degenerates to the base fill (`t = 0`).
fn fill_at(
    panes: &[PaneRect],
    palette: &Palette,
    bounds: &[(usize, usize)],
    gradient: GradientMode,
    i: usize,
    px: usize,
    py: usize,
) -> Rgb {
    let fill = palette.style_for(panes[i].id, panes[i].focused).fill;
    if gradient == GradientMode::Off {
        return fill;
    }
    let (px0, px1) = bounds[i];
    let span = px1.saturating_sub(px0);
    if span <= 1 {
        return fill;
    }
    // Smoothstep-ease the column ratio (3r² − 2r³) so the sweep eases in and
    // out instead of ramping linearly — the first/last columns no longer show
    // the largest perceived jumps (#46). Endpoints are fixed points (0→0,
    // 1→1), so the base fill and the stop are reached exactly.
    let ratio = (px - px0) as f32 / (span - 1) as f32;
    let eased = ratio * ratio * (3.0 - 2.0 * ratio);
    let t = (eased * 100.0).round() as u8;
    match gradient {
        GradientMode::Sheen => crate::color::gradient_at(fill, t),
        GradientMode::Weave if py.is_multiple_of(2) => crate::color::gradient_at(fill, t),
        GradientMode::Weave => crate::color::gradient_at(fill, 100 - t),
        GradientMode::Off => unreachable!("handled above"),
    }
}

/// Color of the pixel at `(px, py)` in the pixel grid. `grid` stores the
/// pane's *slice index* (to reach `panes[i]`); the color itself is keyed on
/// that pane's stable `id`, never its position. Ring pixels are painted solid
/// on top of the gradient sweep, so the focus outline stays intact in every
/// [`GradientMode`].
#[allow(clippy::too_many_arguments)]
fn pixel_color(
    grid: &[Option<usize>],
    ring: &[bool],
    panes: &[PaneRect],
    palette: &Palette,
    bounds: &[(usize, usize)],
    gradient: GradientMode,
    pw: usize,
    px: usize,
    py: usize,
) -> Rgb {
    match grid[py * pw + px] {
        Some(i) if ring[py * pw + px] => palette.ring_for(panes[i].id),
        Some(i) => fill_at(panes, palette, bounds, gradient, i, px, py),
        None => BG,
    }
}

/// Render `panes` into a `cols` × `text_rows` block (pixel rows = `2*text_rows`).
///
/// Colors come from `palette`, keyed on each pane's stable id. Returns an ANSI
/// string of `text_rows` lines, each terminated by a reset and newline. `mode`
/// selects which summarized pane titles are overlaid where width allows (see
/// [`LabelMode`]). `badge`, when present, is the tab's shortcut hint stamped into
/// the block's top-left over the underlying cell color — the pane fill, or the
/// focus ring where it overlaps a focused pane's outline — so it reads as a
/// label *inside* the color block; it is dropped when the block is too narrow
/// to host its display width. Its text is dark, or the palette accent when the
/// badge is [`accented`](Badge::accented) (the active tab's cue, #59).
/// Empty input yields an all-background block, with the badge still stamped
/// over it when one is given and fits. `gradient` selects the per-pane fill
/// sweep (see [`GradientMode`]); `Off` reproduces the historical flat fills
/// byte-for-byte.
pub fn render(
    panes: &[PaneRect],
    palette: &Palette,
    cols: usize,
    text_rows: usize,
    mode: LabelMode,
    badge: Option<Badge>,
    gradient: GradientMode,
) -> String {
    let pw = cols;
    let ph = text_rows * 2;
    if pw == 0 || text_rows == 0 {
        return String::new();
    }

    let minx = panes.iter().map(|p| p.x).min().unwrap_or(0);
    let miny = panes.iter().map(|p| p.y).min().unwrap_or(0);
    let maxx = panes.iter().map(|p| p.x + p.w).max().unwrap_or(1);
    let maxy = panes.iter().map(|p| p.y + p.h).max().unwrap_or(1);
    let bw = (maxx - minx).max(1) as f64;
    let bh = (maxy - miny).max(1) as f64;
    let map = |v: u32, lo: u32, span: f64, out: usize| -> usize {
        (((v - lo) as f64) / span * out as f64).round() as usize
    };

    let mut grid = vec![None::<usize>; ph * pw]; // pane index per pixel
    let mut ring = vec![false; ph * pw]; // focus-ring pixels
    let mut overlay = vec![None::<OverlayCell>; text_rows * pw]; // label cells
    let mut bounds = Vec::with_capacity(panes.len()); // (px0, px1) per pane

    for (i, p) in panes.iter().enumerate() {
        let px0 = map(p.x, minx, bw, pw).min(pw);
        let mut px1 = map(p.x + p.w, minx, bw, pw).min(pw);
        if px1 <= px0 {
            px1 = (px0 + 1).min(pw);
        }
        let py0 = map(p.y, miny, bh, ph).min(ph);
        let mut py1 = map(p.y + p.h, miny, bh, ph).min(ph);
        if py1 <= py0 {
            py1 = (py0 + 1).min(ph);
        }
        bounds.push((px0, px1));

        for py in py0..py1 {
            for px in px0..px1 {
                grid[py * pw + px] = Some(i);
            }
        }

        let cw = px1 - px0;
        let chh = py1 - py0;

        // Focus emphasis: bright outline if the region is big enough to read an
        // outline (≥3×3 px), otherwise brighten the whole (tiny) region.
        if p.focused {
            let outline = cw >= 3 && chh >= 3;
            for py in py0..py1 {
                for px in px0..px1 {
                    let edge = px == px0 || px == px1 - 1 || py == py0 || py == py1 - 1;
                    if !outline || edge {
                        ring[py * pw + px] = true;
                    }
                }
            }
        }

        // Label degradation: require ≥4 cols and ≥2 text rows, reserving a 1-col
        // margin each side so adjacent panes' labels never run together. A label
        // occupies one text row, so a region only one text row tall would have
        // its entire colored span overwritten with text — losing the pane's
        // identity. Requiring two rows keeps at least one row of pure color.
        // This height gate is *per region*, not per tab: it drops the label on
        // any pane that projects to a single text row — every pane of a deep
        // (≥3) vertical stack, AND the shorter side of an asymmetric two-way
        // vertical split, while the taller side (≥2 text rows) keeps its label.
        // An empty summarized title yields no label; a single char (e.g. a
        // pane titled `~`) is overlaid like any longer label.
        let trow0 = py0 / 2;
        let cell_text_rows = py1.div_ceil(2).saturating_sub(trow0);
        let inner = cw.saturating_sub(2);
        let want_label = match mode {
            LabelMode::None => false,
            LabelMode::Focused => p.focused,
            LabelMode::All => true,
        };
        if want_label && cw >= 4 && cell_text_rows >= 2 && inner >= 2 {
            let label = crate::title::summarize(&p.title, inner, false);
            // Placement is by display column (#57): the label is centered by
            // its display width and each glyph claims one cell per column it
            // spans (see [`OverlayCell`]), so a CJK rename overlays like any
            // ASCII title. Zero-width chars are skipped — emitting a joiner
            // would let a sequence-collapsing terminal advance fewer columns
            // than the overlay claimed — so width is priced per char
            // ([`crate::title::charwise_width`]) to match what is emitted.
            // `summarize` budgets with the same pricing, so the label always
            // fits `inner` and the edge guard below only defends against a
            // wide glyph straddling the region's right border.
            let label_width = crate::title::charwise_width(&label);
            let row = trow0 + cell_text_rows / 2;
            if label_width >= 1 && row < text_rows {
                let start = px0 + 1 + (inner - label_width) / 2;
                label
                    .chars()
                    .filter_map(|ch| {
                        UnicodeWidthChar::width(ch)
                            .filter(|w| *w >= 1)
                            .map(|w| (ch, w))
                    })
                    .scan(start, |col, (ch, w)| {
                        let at = *col;
                        *col += w;
                        Some((at, ch, w))
                    })
                    .take_while(|(at, _, w)| at + w <= px1)
                    .for_each(|(at, ch, w)| {
                        overlay[row * pw + at] = Some(OverlayCell::Glyph(ch, i));
                        (at + 1..at + w).for_each(|cc| {
                            overlay[row * pw + cc] = Some(OverlayCell::Continuation);
                        });
                    });
            }
        }
    }

    // The shortcut badge occupies the top text row's left cells, after a
    // one-cell margin, drawn over the underlying cell color — the pane fill, or
    // the focus ring where it sits on a focused pane's outline — so it reads
    // inside the block, integrating with the ring rather than punching a
    // fill-colored hole in it. It is dropped wholesale when it would not fit
    // within the width.
    const BADGE_COL: usize = 1;
    // Stamping is by display column (#57): each badge glyph claims its leading
    // cell (`Some`) plus one `None` continuation cell per extra column it
    // spans, mirroring the label overlay above — `shortcut_prefix` is
    // user-configurable, so a fullwidth glyph must advance two cells to stay
    // in lockstep with the terminal's advance. Zero-width chars are skipped.
    // Fitting is judged on the resulting per-column length; a badge wider than
    // the block is dropped wholesale rather than split mid-glyph.
    let badge_cells: Vec<Option<char>> = badge
        .map(|b| {
            b.text
                .chars()
                .filter_map(|ch| {
                    UnicodeWidthChar::width(ch)
                        .filter(|w| *w >= 1)
                        .map(|w| (ch, w))
                })
                .flat_map(|(ch, w)| {
                    std::iter::once(Some(ch)).chain(std::iter::repeat_n(None, w - 1))
                })
                .collect()
        })
        .unwrap_or_default();
    let badge_fits = !badge_cells.is_empty() && BADGE_COL + badge_cells.len() <= pw;
    // The active tab's badge text carries the accent (#59); a plain badge
    // keeps the historical dark label color.
    let badge_fg = match badge {
        Some(Badge { accented: true, .. }) => palette.accent(),
        _ => LABEL_FG,
    };

    let mut out = String::with_capacity(text_rows * pw * 24);
    for tr in 0..text_rows {
        for c in 0..pw {
            if tr == 0 && badge_fits && (BADGE_COL..BADGE_COL + badge_cells.len()).contains(&c) {
                // A `None` cell sits under the trailing column of a wide badge
                // glyph, which already covers it on screen — emit nothing.
                let Some(ch) = badge_cells[c - BADGE_COL] else {
                    continue;
                };
                let fill = pixel_color(&grid, &ring, panes, palette, &bounds, gradient, pw, c, 0);
                put_bg(&mut out, fill);
                put_fg(&mut out, badge_fg);
                out.push_str("\x1b[1m");
                out.push(ch);
                out.push_str("\x1b[22m");
                continue;
            }
            // A continuation cell is already covered on screen by its wide
            // glyph's advance — emit nothing so cells stay in lockstep.
            if let Some(OverlayCell::Continuation) = overlay[tr * pw + c] {
                continue;
            }
            if let Some(OverlayCell::Glyph(ch, i)) = overlay[tr * pw + c] {
                let style = palette.style_for(panes[i].id, panes[i].focused);
                put_bg(
                    &mut out,
                    fill_at(panes, palette, &bounds, gradient, i, c, 2 * tr),
                );
                put_fg(&mut out, LABEL_FG);
                if style.emphasized {
                    out.push_str("\x1b[1m");
                    out.push(ch);
                    out.push_str("\x1b[22m");
                    continue;
                }
                out.push(ch);
                continue;
            }
            let top = pixel_color(
                &grid,
                &ring,
                panes,
                palette,
                &bounds,
                gradient,
                pw,
                c,
                2 * tr,
            );
            let bottom = pixel_color(
                &grid,
                &ring,
                panes,
                palette,
                &bounds,
                gradient,
                pw,
                c,
                2 * tr + 1,
            );
            put_fg(&mut out, top);
            put_bg(&mut out, bottom);
            out.push('▀');
        }
        out.push_str(RESET);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A palette with distinct, easily recognized colors so tests can assert
    /// on exact escape sequences. The accent is unlike every slot; rings are
    /// derived per pane from its own fill (`ring_for`).
    fn test_palette() -> Palette {
        Palette::new(
            vec![(10, 20, 30), (40, 50, 60), (70, 80, 90)],
            (200, 100, 50),
        )
    }

    fn fg(c: Rgb) -> String {
        format!("\x1b[38;2;{};{};{}m", c.0, c.1, c.2)
    }

    fn one_focused() -> Vec<PaneRect> {
        vec![PaneRect::new(0, 0, 0, 100, 40, "nvim", true)]
    }

    #[test]
    fn render_emits_requested_row_count() {
        let out = render(
            &one_focused(),
            &test_palette(),
            10,
            3,
            LabelMode::None,
            None,
            GradientMode::Off,
        );
        assert_eq!(out.lines().count(), 3);
    }

    #[test]
    fn render_widens_a_degenerate_pane_to_one_pixel() {
        // A zero-area pane inside a larger bounding box maps both of its edges
        // to the same pixel on both axes. The renderer must widen it to a
        // single pixel rather than drop it, so its fill still shows up.
        let panes = vec![
            PaneRect::new(0, 0, 0, 100, 40, "a", false),
            PaneRect::new(1, 50, 20, 0, 0, "b", false),
        ];
        let palette = test_palette();
        let out = render(
            &panes,
            &palette,
            10,
            3,
            LabelMode::None,
            None,
            GradientMode::Off,
        );
        let (r, g, b) = palette.color_for(1);
        assert!(
            out.contains(&format!("2;{r};{g};{b}m")),
            "the degenerate pane's slot color must be painted somewhere"
        );
    }

    #[test]
    fn render_uses_truecolor_and_halfblock() {
        let out = render(
            &one_focused(),
            &test_palette(),
            10,
            3,
            LabelMode::None,
            None,
            GradientMode::Off,
        );
        assert!(out.contains("\x1b[38;2;"), "expected a truecolor fg escape");
        assert!(out.contains("\x1b[48;2;"), "expected a truecolor bg escape");
        assert!(out.contains('▀'), "expected the upper-half-block glyph");
    }

    #[test]
    fn render_draws_focus_ring_for_large_pane() {
        let palette = test_palette();
        let out = render(
            &one_focused(),
            &palette,
            10,
            3,
            LabelMode::None,
            None,
            GradientMode::Off,
        );
        assert!(
            out.contains(&fg(palette.ring_for(0))),
            "focused pane should show its ring color"
        );
    }

    #[test]
    fn render_keys_fill_on_pane_id_not_position() {
        let palette = test_palette();
        // Two unfocused panes whose ids land on *distinct* slots (1 and 2 in
        // the 3-slot test palette). Each must paint its own color in both list
        // orderings — proving the fill follows identity, not slice index.
        let render_pair = |a: usize, b: usize| {
            render(
                &[
                    PaneRect::new(a, 0, 0, 50, 40, "a", false),
                    PaneRect::new(b, 50, 0, 50, 40, "b", false),
                ],
                &palette,
                12,
                3,
                LabelMode::None,
                None,
                GradientMode::Off,
            )
        };
        let id1 = fg(palette.color_for(1));
        let id2 = fg(palette.color_for(2));
        assert_ne!(
            id1, id2,
            "test palette must give ids 1 and 2 distinct slots"
        );
        for out in [render_pair(1, 2), render_pair(2, 1)] {
            assert!(
                out.contains(&id1),
                "id 1 keeps its color regardless of order"
            );
            assert!(
                out.contains(&id2),
                "id 2 keeps its color regardless of order"
            );
        }
    }

    #[test]
    fn render_zero_size_is_empty() {
        assert!(
            render(
                &one_focused(),
                &test_palette(),
                0,
                3,
                LabelMode::None,
                None,
                GradientMode::Off
            )
            .is_empty()
        );
        assert!(
            render(
                &one_focused(),
                &test_palette(),
                10,
                0,
                LabelMode::None,
                None,
                GradientMode::Off
            )
            .is_empty()
        );
    }

    #[test]
    fn render_empty_panes_is_all_background() {
        let out = render(
            &[],
            &test_palette(),
            4,
            2,
            LabelMode::None,
            None,
            GradientMode::Off,
        );
        assert_eq!(out.lines().count(), 2);
        let (r, g, b) = BG;
        let bg = format!("\x1b[48;2;{};{};{}m", r, g, b);
        assert!(
            out.contains(&bg),
            "empty block should be painted with the background"
        );
    }

    #[test]
    fn labels_appear_when_wide_and_drop_when_narrow() {
        let panes = vec![PaneRect::new(0, 0, 0, 100, 40, "cargo", false)];
        // Wide enough: the label's leading char should be overlaid (dark text fg).
        let wide = render(
            &panes,
            &test_palette(),
            12,
            3,
            LabelMode::All,
            None,
            GradientMode::Off,
        );
        assert!(wide.contains('c'), "expected label text in a wide block");
        // Too narrow (cw < 4 after normalization): no label, only block glyphs.
        let narrow = render(
            &panes,
            &test_palette(),
            3,
            3,
            LabelMode::All,
            None,
            GradientMode::Off,
        );
        assert!(!narrow.contains('c'), "narrow block should drop the label");
    }

    #[test]
    fn single_char_title_keeps_its_label() {
        // A shell pane in the home directory is titled `~` — one char. It must
        // be overlaid like any longer label, not dropped (#38).
        let panes = vec![PaneRect::new(0, 0, 0, 100, 40, "~", false)];
        let wide = render(
            &panes,
            &test_palette(),
            12,
            3,
            LabelMode::All,
            None,
            GradientMode::Off,
        );
        assert!(
            wide.contains('~'),
            "expected the 1-char label in a wide block"
        );
    }

    #[test]
    fn deep_vertical_stack_drops_labels() {
        // Three panes stacked in a 6px-tall canvas leave each region only one
        // text row. A label there would replace the pane's entire colored span
        // with text, erasing its identity — so labels degrade to color-only for
        // a stack this deep, even at a width that would otherwise label.
        let panes = vec![
            PaneRect::new(0, 0, 0, 100, 10, "aaa", false),
            PaneRect::new(1, 0, 10, 100, 10, "bbb", false),
            PaneRect::new(2, 0, 20, 100, 10, "ccc", false),
        ];
        let out = render(
            &panes,
            &test_palette(),
            16,
            3,
            LabelMode::All,
            None,
            GradientMode::Off,
        );
        for ch in ['a', 'b', 'c'] {
            assert!(
                !out.contains(ch),
                "deep vertical stack must render color-only, found label char {ch:?}"
            );
        }
    }

    #[test]
    fn asymmetric_vertical_split_labels_only_the_taller_pane() {
        // Two stacked panes split 24/16 of a 40-tall group. Projected into the
        // 6px canvas the top spans pixel rows 0..4 (2 text rows → labeled) while
        // the bottom spans 4..6 (1 text row → color-only). The height gate is
        // per region, so the taller pane keeps its label and the shorter loses
        // it — not an all-or-nothing per-tab decision.
        let panes = vec![
            PaneRect::new(0, 0, 0, 100, 24, "zsh", false),
            PaneRect::new(1, 0, 24, 100, 16, "vim", true),
        ];
        let out = render(
            &panes,
            &test_palette(),
            16,
            3,
            LabelMode::All,
            None,
            GradientMode::Off,
        );
        assert!(
            out.contains('z'),
            "the taller pane (2 text rows) keeps its label"
        );
        assert!(
            !out.contains('v'),
            "the shorter pane (1 text row) drops its label"
        );
    }

    /// The visible text of each output line with CSI escape sequences
    /// stripped — what the terminal would actually show, glyph by glyph.
    fn visible_lines(out: &str) -> Vec<String> {
        out.lines()
            .map(|line| {
                line.chars()
                    .scan(0u8, |csi, c| {
                        // 0 = text, 1 = saw ESC, 2 = inside CSI
                        let kept = match (*csi, c) {
                            (0, '\u{1b}') => {
                                *csi = 1;
                                None
                            }
                            (1, '[') => {
                                *csi = 2;
                                None
                            }
                            (1, _) => {
                                *csi = 0;
                                None
                            }
                            (2, c) if ('\u{40}'..='\u{7e}').contains(&c) => {
                                *csi = 0;
                                None
                            }
                            (2, _) => None,
                            (_, c) => Some(c),
                        };
                        Some(kept)
                    })
                    .flatten()
                    .collect()
            })
            .collect()
    }

    #[test]
    fn labels_with_wide_glyphs_are_placed_width_aware() {
        // A CJK rename summarizes to multi-column chars. Placement is by
        // display column: each wide glyph occupies two cells (the second is a
        // continuation the emitter skips), so the label is centered by width
        // and every row keeps exactly the block's display width (#57).
        let panes = vec![PaneRect::new(0, 0, 0, 100, 40, "実装中", false)];
        let out = render(
            &panes,
            &test_palette(),
            12,
            3,
            LabelMode::All,
            None,
            GradientMode::Off,
        );
        let lines = visible_lines(&out);
        // 6-column label centered in the 10-column inner span: 3 block cells
        // each side.
        assert!(
            lines.contains(&"▀▀▀実装中▀▀▀".to_string()),
            "expected the centered CJK label, got {lines:?}"
        );
        for (row, line) in lines.iter().enumerate() {
            assert_eq!(
                unicode_width::UnicodeWidthStr::width(line.as_str()),
                12,
                "row {row} must keep the block's display width, got {line:?}"
            );
        }
    }

    #[test]
    fn emoji_zwj_label_renders_decomposed_but_width_consistent() {
        // An emoji ZWJ sequence (👩\u{200D}💻) decomposes on the overlay: the
        // joiner is zero-width, so emitting it would let a sequence-collapsing
        // terminal advance 2 columns where the overlay claimed 4 — the exact
        // desync #57 fixes. Skipping zero-width chars keeps the claimed cells
        // equal to the terminal's advance for what is actually emitted, and
        // `charwise_width` prices the label the same way (👩=2 + 💻=2 = 4, not
        // `UnicodeWidthStr`'s sequence-aware 2), so centering matches: the
        // parts render side by side, off-spec visually but never corrupting
        // the row or the margins. This test pins that trade-off.
        let panes = vec![PaneRect::new(0, 0, 0, 100, 40, "👩\u{200D}💻", false)];
        let out = render(
            &panes,
            &test_palette(),
            12,
            3,
            LabelMode::All,
            None,
            GradientMode::Off,
        );
        let lines = visible_lines(&out);
        // Both halves are emitted, centered by the 4-column char-sum width …
        assert!(
            lines.contains(&"▀▀▀▀👩💻▀▀▀▀".to_string()),
            "expected the decomposed emoji label, got {lines:?}"
        );
        // … and the joiner itself never reaches the output.
        assert!(
            !out.contains('\u{200D}'),
            "a zero-width joiner must not be emitted into the overlay"
        );
        for (row, line) in lines.iter().enumerate() {
            assert_eq!(
                unicode_width::UnicodeWidthStr::width(line.as_str()),
                12,
                "row {row} must keep the block's display width, got {line:?}"
            );
        }
    }

    #[test]
    fn wide_glyph_labels_truncate_without_splitting_at_the_edge() {
        // At 7 columns the inner span is 5: `summarize` keeps "実装" (4 cols)
        // plus the ellipsis. The trailing 中 must vanish entirely — a wide
        // glyph is never split into a half-rendered cell at the edge (#57).
        let panes = vec![PaneRect::new(0, 0, 0, 100, 40, "実装中", false)];
        let out = render(
            &panes,
            &test_palette(),
            7,
            3,
            LabelMode::All,
            None,
            GradientMode::Off,
        );
        let lines = visible_lines(&out);
        assert!(
            lines.iter().any(|l| l.contains("実装…")),
            "expected the truncated CJK label, got {lines:?}"
        );
        assert!(!out.contains('中'), "the dropped glyph must not leak");
        for (row, line) in lines.iter().enumerate() {
            assert_eq!(
                unicode_width::UnicodeWidthStr::width(line.as_str()),
                7,
                "row {row} must keep the block's display width, got {line:?}"
            );
        }
    }

    #[test]
    fn badge_is_stamped_when_it_fits_and_dropped_when_too_narrow() {
        // The shortcut badge lands in the top text row's left cells, drawn dark
        // over the pane fill so it reads inside the block. The `⌘` glyph is unique
        // to the badge (color escapes carry only digits/semicolons, the grid only
        // `▀`), so its presence/absence is a clean witness for the badge. A wide
        // block hosts it; a block too narrow for the 1-col margin plus the badge
        // drops it wholesale rather than truncating into noise.
        let panes = one_focused();
        let wide = render(
            &panes,
            &test_palette(),
            10,
            3,
            LabelMode::None,
            Some(Badge {
                text: "⌘ 1",
                accented: false,
            }),
            GradientMode::Off,
        );
        assert!(wide.contains('⌘'), "wide block should host the badge");
        let narrow = render(
            &panes,
            &test_palette(),
            1,
            3,
            LabelMode::None,
            Some(Badge {
                text: "⌘ 1",
                accented: false,
            }),
            GradientMode::Off,
        );
        assert!(
            !narrow.contains('⌘'),
            "too-narrow block must drop the badge"
        );
    }

    #[test]
    fn accented_badge_paints_accent_text_plain_badge_stays_dark() {
        // #59: the active tab's badge switches its text from the dark label
        // color to the palette accent, so the selected tab carries a colored
        // cue inside its block; an unaccented badge keeps the historical dark
        // text. LabelMode::None keeps labels out, so the badge is the only
        // possible source of either text color.
        let panes = one_focused();
        let palette = test_palette();
        let render_badge = |accented: bool| {
            render(
                &panes,
                &palette,
                10,
                3,
                LabelMode::None,
                Some(Badge {
                    text: "⌘ 1",
                    accented,
                }),
                GradientMode::Off,
            )
        };
        let active = render_badge(true);
        let inactive = render_badge(false);
        assert!(
            active.contains(&fg(palette.accent())),
            "an accented badge must draw its text in the palette accent"
        );
        assert!(
            !active.contains(&fg(LABEL_FG)),
            "an accented badge must not also emit the dark label color"
        );
        assert!(
            inactive.contains(&fg(LABEL_FG)),
            "a plain badge keeps the historical dark text"
        );
        assert!(
            !inactive.contains(&fg(palette.accent())),
            "a plain badge must not leak the accent"
        );
    }

    #[test]
    fn badge_with_wide_glyphs_is_stamped_width_aware() {
        // `shortcut_prefix` is user-configurable, so a badge can carry a
        // fullwidth glyph (`符` advances two terminal columns). Stamping is by
        // display column — the wide glyph takes two cells and the row keeps
        // exactly the block's display width (#57).
        let panes = one_focused();
        let out = render(
            &panes,
            &test_palette(),
            10,
            3,
            LabelMode::None,
            Some(Badge {
                text: "符1",
                accented: false,
            }),
            GradientMode::Off,
        );
        let lines = visible_lines(&out);
        assert!(
            lines[0].contains("符1"),
            "expected the wide-glyph badge, got {lines:?}"
        );
        for (row, line) in lines.iter().enumerate() {
            assert_eq!(
                unicode_width::UnicodeWidthStr::width(line.as_str()),
                10,
                "row {row} must keep the block's display width, got {line:?}"
            );
        }
    }

    #[test]
    fn badge_with_wide_glyphs_is_dropped_when_it_does_not_fit() {
        // Fitting is judged in display columns, not chars: `符符` is 2 chars
        // but 4 columns, which the 1-col margin pushes past a 4-col block.
        // The badge drops wholesale rather than splitting a glyph (#57).
        let panes = one_focused();
        let out = render(
            &panes,
            &test_palette(),
            4,
            3,
            LabelMode::None,
            Some(Badge {
                text: "符符",
                accented: false,
            }),
            GradientMode::Off,
        );
        assert!(
            !out.contains('符'),
            "a badge wider than the block must be dropped, not split"
        );
    }

    /// A single unfocused full-width pane — the simplest sweep canvas (no
    /// ring, no label), so every emitted color is a gradient sample.
    fn one_plain() -> Vec<PaneRect> {
        vec![PaneRect::new(1, 0, 0, 100, 40, "", false)]
    }

    #[test]
    fn gradient_off_paints_only_the_base_fill() {
        let palette = test_palette();
        let out = render(
            &one_plain(),
            &palette,
            10,
            2,
            LabelMode::None,
            None,
            GradientMode::Off,
        );
        let stop = fg(crate::color::gradient_at(palette.color_for(1), 100));
        assert!(out.contains(&fg(palette.color_for(1))));
        assert!(!out.contains(&stop), "off must not paint any swept shade");
    }

    #[test]
    fn sheen_sweeps_from_base_fill_to_stop() {
        let palette = test_palette();
        let out = render(
            &one_plain(),
            &palette,
            10,
            1,
            LabelMode::None,
            None,
            GradientMode::Sheen,
        );
        let base = fg(palette.color_for(1));
        let stop = fg(crate::color::gradient_at(palette.color_for(1), 100));
        assert!(out.starts_with(&base), "leftmost column is the base fill");
        assert!(
            out.contains(&stop),
            "rightmost column reaches the sweep stop"
        );
    }

    #[test]
    fn weave_reverses_the_bottom_pixel_row() {
        // Pixel row parity drives the direction: at column 0 the top pixel is
        // t=0 (base fill) while the bottom pixel sweeps R→L and is t=100 (the
        // stop) — so the very first cell must pair base fg with stop bg.
        let palette = test_palette();
        let out = render(
            &one_plain(),
            &palette,
            10,
            1,
            LabelMode::None,
            None,
            GradientMode::Weave,
        );
        let fill = palette.color_for(1);
        let stop = crate::color::gradient_at(fill, 100);
        let first_cell = format!("{}\x1b[48;2;{};{};{}m", fg(fill), stop.0, stop.1, stop.2);
        assert!(
            out.starts_with(&first_cell),
            "weave column 0 must be base-over-stop"
        );
    }

    #[test]
    fn one_column_span_degenerates_to_the_base_fill() {
        // A 1-px-wide pane has no sweep direction; it must render the base
        // fill without dividing by zero.
        let palette = test_palette();
        let out = render(
            &one_plain(),
            &palette,
            1,
            1,
            LabelMode::None,
            None,
            GradientMode::Sheen,
        );
        assert!(out.starts_with(&fg(palette.color_for(1))));
    }
}

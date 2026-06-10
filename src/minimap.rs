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

use crate::color::Palette;
// Re-exported so the historical `minimap::Rgb` path keeps resolving (the
// canonical definition lives in `crate::color`).
pub use crate::color::Rgb;

/// Tokyonight background (`#1a1b26`) — painted on empty space. Shared with
/// [`crate::tab_block`], which paints its glyph/hint rungs on the same canvas.
pub(crate) const BG: Rgb = (26, 27, 38);
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
/// `w` pixel columns is `lerp(fill, stop, x / (w - 1))`, where the stop is the
/// pane fill's luminance-shifted shade ([`crate::color::gradient_at`]). Focus
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
    let (px0, px1) = bounds[i];
    let span = px1.saturating_sub(px0);
    if span <= 1 {
        return fill;
    }
    let t = ((px - px0) * 100 / (span - 1)) as u8;
    match gradient {
        GradientMode::Off => fill,
        GradientMode::Sheen => crate::color::gradient_at(fill, t),
        GradientMode::Weave if py.is_multiple_of(2) => crate::color::gradient_at(fill, t),
        GradientMode::Weave => crate::color::gradient_at(fill, 100 - t),
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
        Some(_) if ring[py * pw + px] => palette.ring(),
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
/// the block's top-left in dark text over the underlying cell color — the pane
/// fill, or the focus ring where it overlaps a focused pane's outline — so it
/// reads as a label *inside* the color block; it is dropped when the block is
/// too narrow to host it, or when it contains a glyph wider than one column.
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
    badge: Option<&str>,
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
    let mut overlay = vec![None::<(char, usize)>; text_rows * pw]; // label chars
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
            let label_len = label.chars().count();
            // Placement below is char-indexed (one cell per char), correct only
            // when every char occupies one display column. `summarize` is
            // width-aware and can return wider glyphs (CJK renames now, icons in
            // #7) — drop those rather than corrupt the row; width-aware placement
            // lands in #7.
            if label_len >= 1 && crate::title::is_single_column(&label) {
                let row = trow0 + cell_text_rows / 2;
                let start = px0 + 1 + (inner - label_len) / 2;
                for (k, ch) in label.chars().enumerate() {
                    let col = start + k;
                    if col < px1 && row < text_rows {
                        overlay[row * pw + col] = Some((ch, i));
                    }
                }
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
    // Char-indexed stamping (one cell per char below) is correct only when every
    // badge glyph is one display column. `shortcut_prefix` is user-configurable,
    // so a wide glyph would desync the cells from the terminal's advance and
    // corrupt the row — drop the badge wholesale in that case, mirroring the
    // wide-glyph label guard above (width-aware placement lands in #7). The
    // default `⌘ ` + digit is all single-column, so the common case is untouched.
    let badge_chars: Vec<char> = badge.map(|b| b.chars().collect()).unwrap_or_default();
    let badge_fits = badge.is_some_and(crate::title::is_single_column)
        && !badge_chars.is_empty()
        && BADGE_COL + badge_chars.len() <= pw;

    let mut out = String::with_capacity(text_rows * pw * 24);
    for tr in 0..text_rows {
        for c in 0..pw {
            if tr == 0 && badge_fits && (BADGE_COL..BADGE_COL + badge_chars.len()).contains(&c) {
                let fill = pixel_color(&grid, &ring, panes, palette, &bounds, gradient, pw, c, 0);
                put_bg(&mut out, fill);
                put_fg(&mut out, LABEL_FG);
                out.push_str("\x1b[1m");
                out.push(badge_chars[c - BADGE_COL]);
                out.push_str("\x1b[22m");
                continue;
            }
            if let Some((ch, i)) = overlay[tr * pw + c] {
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
    /// on exact escape sequences. Accent and ring are unlike every slot — the
    /// ring is pinned explicitly so the asserted escapes stay value-stable.
    fn test_palette() -> Palette {
        Palette::new(
            vec![(10, 20, 30), (40, 50, 60), (70, 80, 90)],
            (200, 100, 50),
            Some((250, 250, 250)),
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
            out.contains(&fg(palette.ring())),
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

    #[test]
    fn labels_with_wide_glyphs_are_dropped_not_corrupted() {
        // A CJK rename summarizes to multi-column chars. The char-indexed
        // overlay would advance the cursor one cell per char while the terminal
        // advances two, mis-centering the label and corrupting the next cell.
        // Such labels must be dropped wholesale until width-aware placement (#7).
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
        assert!(
            !out.contains('実'),
            "wide-glyph label must be dropped, not placed"
        );
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
            Some("⌘ 1"),
            GradientMode::Off,
        );
        assert!(wide.contains('⌘'), "wide block should host the badge");
        let narrow = render(
            &panes,
            &test_palette(),
            1,
            3,
            LabelMode::None,
            Some("⌘ 1"),
            GradientMode::Off,
        );
        assert!(
            !narrow.contains('⌘'),
            "too-narrow block must drop the badge"
        );
    }

    #[test]
    fn badge_with_wide_glyphs_is_dropped_not_corrupted() {
        // `shortcut_prefix` is user-configurable, so a badge can carry a
        // fullwidth glyph (`符` advances two terminal columns). The stamping
        // loop writes one char per cell, so a wide glyph would desync the cells
        // from the cursor and corrupt the row. The block here is wide enough to
        // host a single-column badge, so width is not the reason for the drop —
        // the wide glyph is. The badge must be dropped wholesale, mirroring the
        // wide-glyph label guard.
        let panes = one_focused();
        let out = render(
            &panes,
            &test_palette(),
            10,
            3,
            LabelMode::None,
            Some("符1"),
            GradientMode::Off,
        );
        assert!(
            !out.contains('符'),
            "wide-glyph badge must be dropped, not placed"
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

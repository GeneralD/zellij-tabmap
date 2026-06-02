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

/// Tokyonight background (`#1a1b26`) — painted on empty space.
const BG: Rgb = (26, 27, 38);
/// Dark text color for labels drawn over a (light) pane fill.
const LABEL_FG: Rgb = (16, 17, 26);

const RESET: &str = "\x1b[0m";

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

fn put_fg(out: &mut String, c: Rgb) {
    out.push_str(&format!("\x1b[38;2;{};{};{}m", c.0, c.1, c.2));
}

fn put_bg(out: &mut String, c: Rgb) {
    out.push_str(&format!("\x1b[48;2;{};{};{}m", c.0, c.1, c.2));
}

// ---- core renderer ------------------------------------------------------

/// Color of the pixel at `(px, py)` in the pixel grid. `grid` stores the
/// pane's *slice index* (to reach `panes[i]`); the color itself is keyed on
/// that pane's stable `id`, never its position.
fn pixel_color(
    grid: &[Option<usize>],
    ring: &[bool],
    panes: &[PaneRect],
    palette: &Palette,
    pw: usize,
    px: usize,
    py: usize,
) -> Rgb {
    match grid[py * pw + px] {
        Some(_) if ring[py * pw + px] => palette.ring(),
        Some(i) => palette.style_for(panes[i].id, panes[i].focused).fill,
        None => BG,
    }
}

/// Render `panes` into a `cols` × `text_rows` block (pixel rows = `2*text_rows`).
///
/// Colors come from `palette`, keyed on each pane's stable id. Returns an ANSI
/// string of `text_rows` lines, each terminated by a reset and newline. When
/// `labels` is true, summarized pane titles are overlaid where width allows.
/// Empty input yields an all-background block.
pub fn render(
    panes: &[PaneRect],
    palette: &Palette,
    cols: usize,
    text_rows: usize,
    labels: bool,
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

        // Label degradation: require ≥4 cols and ≥1 text row, reserving a 1-col
        // margin each side so adjacent panes' labels never run together. If the
        // summarized title is shorter than 2 chars, drop it.
        let trow0 = py0 / 2;
        let cell_text_rows = py1.div_ceil(2).saturating_sub(trow0);
        let inner = cw.saturating_sub(2);
        if labels && cw >= 4 && cell_text_rows >= 1 && inner >= 2 {
            let label = crate::title::summarize(&p.title, inner, false);
            let label_len = label.chars().count();
            // Placement below is char-indexed (one cell per char), correct only
            // when every char occupies one display column. `summarize` is
            // width-aware and can return wider glyphs (CJK renames now, icons in
            // #7) — drop those rather than corrupt the row; width-aware placement
            // lands in #7.
            if label_len >= 2 && crate::title::is_single_column(&label) {
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

    let mut out = String::with_capacity(text_rows * pw * 24);
    for tr in 0..text_rows {
        for c in 0..pw {
            if let Some((ch, i)) = overlay[tr * pw + c] {
                let style = palette.style_for(panes[i].id, panes[i].focused);
                put_bg(&mut out, style.fill);
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
            let top = pixel_color(&grid, &ring, panes, palette, pw, c, 2 * tr);
            let bottom = pixel_color(&grid, &ring, panes, palette, pw, c, 2 * tr + 1);
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
    /// on exact escape sequences. Accent and ring are unlike every slot.
    fn test_palette() -> Palette {
        Palette::new(
            vec![(10, 20, 30), (40, 50, 60), (70, 80, 90)],
            (200, 100, 50),
            (250, 250, 250),
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
        let out = render(&one_focused(), &test_palette(), 10, 3, false);
        assert_eq!(out.lines().count(), 3);
    }

    #[test]
    fn render_uses_truecolor_and_halfblock() {
        let out = render(&one_focused(), &test_palette(), 10, 3, false);
        assert!(out.contains("\x1b[38;2;"), "expected a truecolor fg escape");
        assert!(out.contains("\x1b[48;2;"), "expected a truecolor bg escape");
        assert!(out.contains('▀'), "expected the upper-half-block glyph");
    }

    #[test]
    fn render_draws_focus_ring_for_large_pane() {
        let palette = test_palette();
        let out = render(&one_focused(), &palette, 10, 3, false);
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
                false,
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
        assert!(render(&one_focused(), &test_palette(), 0, 3, false).is_empty());
        assert!(render(&one_focused(), &test_palette(), 10, 0, false).is_empty());
    }

    #[test]
    fn render_empty_panes_is_all_background() {
        let out = render(&[], &test_palette(), 4, 2, false);
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
        let wide = render(&panes, &test_palette(), 12, 3, true);
        assert!(wide.contains('c'), "expected label text in a wide block");
        // Too narrow (cw < 4 after normalization): no label, only block glyphs.
        let narrow = render(&panes, &test_palette(), 3, 3, true);
        assert!(!narrow.contains('c'), "narrow block should drop the label");
    }

    #[test]
    fn labels_with_wide_glyphs_are_dropped_not_corrupted() {
        // A CJK rename summarizes to multi-column chars. The char-indexed
        // overlay would advance the cursor one cell per char while the terminal
        // advances two, mis-centering the label and corrupting the next cell.
        // Such labels must be dropped wholesale until width-aware placement (#7).
        let panes = vec![PaneRect::new(0, 0, 0, 100, 40, "実装中", false)];
        let out = render(&panes, &test_palette(), 12, 3, true);
        assert!(
            !out.contains('実'),
            "wide-glyph label must be dropped, not placed"
        );
    }
}

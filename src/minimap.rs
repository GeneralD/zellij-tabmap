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
//! and prints the ANSI string this module returns.

/// 24-bit color.
pub type Rgb = (u8, u8, u8);

/// Tokyonight background (`#1a1b26`) — painted on empty space.
const BG: Rgb = (26, 27, 38);
/// Dark text color for labels drawn over a (light) pane fill.
const LABEL_FG: Rgb = (16, 17, 26);

const RESET: &str = "\x1b[0m";

/// A pane's position and size in terminal cells, plus display metadata.
///
/// Coordinates are absolute terminal cells, but only *relative* positions
/// matter: [`render`] normalizes every pane against the group's bounding box,
/// so the coordinate origin is irrelevant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub title: String,
    pub focused: bool,
}

impl PaneRect {
    pub fn new(x: u32, y: u32, w: u32, h: u32, title: impl Into<String>, focused: bool) -> Self {
        Self {
            x,
            y,
            w,
            h,
            title: title.into(),
            focused,
        }
    }
}

// ---- palette ------------------------------------------------------------

/// Standard HSL → RGB. `h` in degrees, `s`/`l` in `0.0..=1.0`.
fn hsl(h: f64, s: f64, l: f64) -> Rgb {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - (hp.rem_euclid(2.0) - 1.0).abs());
    let (r1, g1, b1) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let q = |v: f64| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    (q(r1), q(g1), q(b1))
}

/// Distinct hue per pane index, spread by the golden angle (blue-first).
fn pane_hue(i: usize) -> f64 {
    210.0 + (i as f64) * 137.5
}

/// Fill color for a pane: vivid when focused, muted (same hue) otherwise.
fn pane_color(i: usize, focused: bool) -> Rgb {
    if focused {
        hsl(pane_hue(i), 0.72, 0.66)
    } else {
        hsl(pane_hue(i), 0.26, 0.50)
    }
}

/// Bright outline color drawn on the focused pane's border.
fn ring_color(i: usize) -> Rgb {
    hsl(pane_hue(i), 0.85, 0.82)
}

// ---- ANSI emission ------------------------------------------------------

fn put_fg(out: &mut String, c: Rgb) {
    out.push_str(&format!("\x1b[38;2;{};{};{}m", c.0, c.1, c.2));
}

fn put_bg(out: &mut String, c: Rgb) {
    out.push_str(&format!("\x1b[48;2;{};{};{}m", c.0, c.1, c.2));
}

// ---- label summarization ------------------------------------------------

/// Summarize a pane title to at most `max` characters: take the leading
/// command token, strip any path prefix, and cap with an ellipsis.
fn summarize(title: &str, max: usize) -> String {
    let token = title.split_whitespace().next().unwrap_or("");
    let base = token
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(title);
    let len = base.chars().count();
    if len <= max {
        base.to_string()
    } else if max == 0 {
        String::new()
    } else {
        let mut s: String = base.chars().take(max - 1).collect();
        s.push('…');
        s
    }
}

// ---- core renderer ------------------------------------------------------

/// Color of the pixel at `(px, py)` in the pixel grid.
fn pixel_color(
    grid: &[Option<usize>],
    ring: &[bool],
    panes: &[PaneRect],
    pw: usize,
    px: usize,
    py: usize,
) -> Rgb {
    match grid[py * pw + px] {
        Some(i) if ring[py * pw + px] => ring_color(i),
        Some(i) => pane_color(i, panes[i].focused),
        None => BG,
    }
}

/// Render `panes` into a `cols` × `text_rows` block (pixel rows = `2*text_rows`).
///
/// Returns an ANSI string of `text_rows` lines, each terminated by a reset and
/// newline. When `labels` is true, summarized pane titles are overlaid where
/// width allows. Empty input yields an all-background block.
pub fn render(panes: &[PaneRect], cols: usize, text_rows: usize, labels: bool) -> String {
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
            let label = summarize(&p.title, inner);
            let label_len = label.chars().count();
            if label_len >= 2 {
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
                put_bg(&mut out, pane_color(i, panes[i].focused));
                put_fg(&mut out, LABEL_FG);
                out.push(ch);
                continue;
            }
            let top = pixel_color(&grid, &ring, panes, pw, c, 2 * tr);
            let bottom = pixel_color(&grid, &ring, panes, pw, c, 2 * tr + 1);
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

    #[test]
    fn hsl_primaries() {
        assert_eq!(hsl(0.0, 1.0, 0.5), (255, 0, 0));
        assert_eq!(hsl(120.0, 1.0, 0.5), (0, 255, 0));
        assert_eq!(hsl(240.0, 1.0, 0.5), (0, 0, 255));
        assert_eq!(hsl(0.0, 0.0, 0.0), (0, 0, 0));
        assert_eq!(hsl(0.0, 0.0, 1.0), (255, 255, 255));
    }

    #[test]
    fn hue_is_distinct_per_index() {
        assert_ne!(pane_color(0, false), pane_color(1, false));
        // Focused is more saturated/lighter than the muted variant.
        assert_ne!(pane_color(0, true), pane_color(0, false));
    }

    #[test]
    fn summarize_keeps_short_titles() {
        assert_eq!(summarize("nvim", 10), "nvim");
        assert_eq!(summarize("nvim main.rs", 10), "nvim");
    }

    #[test]
    fn summarize_strips_path_prefix() {
        assert_eq!(summarize("/usr/bin/cargo watch", 10), "cargo");
    }

    #[test]
    fn summarize_caps_with_ellipsis() {
        assert_eq!(summarize("verylongcommand", 5), "very…");
        assert_eq!(summarize("verylongcommand", 0), "");
    }

    #[test]
    fn summarize_handles_empty() {
        assert_eq!(summarize("", 5), "");
    }

    fn one_focused() -> Vec<PaneRect> {
        vec![PaneRect::new(0, 0, 100, 40, "nvim", true)]
    }

    #[test]
    fn render_emits_requested_row_count() {
        let out = render(&one_focused(), 10, 3, false);
        assert_eq!(out.lines().count(), 3);
    }

    #[test]
    fn render_uses_truecolor_and_halfblock() {
        let out = render(&one_focused(), 10, 3, false);
        assert!(out.contains("\x1b[38;2;"), "expected a truecolor fg escape");
        assert!(out.contains("\x1b[48;2;"), "expected a truecolor bg escape");
        assert!(out.contains('▀'), "expected the upper-half-block glyph");
    }

    #[test]
    fn render_draws_focus_ring_for_large_pane() {
        let out = render(&one_focused(), 10, 3, false);
        let (r, g, b) = ring_color(0);
        let needle = format!("\x1b[38;2;{};{};{}m", r, g, b);
        assert!(
            out.contains(&needle),
            "focused pane should show its ring color"
        );
    }

    #[test]
    fn render_zero_size_is_empty() {
        assert!(render(&one_focused(), 0, 3, false).is_empty());
        assert!(render(&one_focused(), 10, 0, false).is_empty());
    }

    #[test]
    fn render_empty_panes_is_all_background() {
        let out = render(&[], 4, 2, false);
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
        let panes = vec![PaneRect::new(0, 0, 100, 40, "cargo", false)];
        // Wide enough: the label's leading char should be overlaid (dark text fg).
        let wide = render(&panes, 12, 3, true);
        assert!(wide.contains('c'), "expected label text in a wide block");
        // Too narrow (cw < 4 after normalization): no label, only block glyphs.
        let narrow = render(&panes, 3, 3, true);
        assert!(!narrow.contains('c'), "narrow block should drop the label");
    }
}

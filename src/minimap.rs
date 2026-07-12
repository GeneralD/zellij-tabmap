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
/// White text for all active-tab labels and badges (#59): on a vivid fill
/// white is the most legible and prominent choice. Shared with
/// [`crate::tab_block`], whose narrow rungs (L3/L4) use it for the active tab.
pub(crate) const ACTIVE_FG: Rgb = (255, 255, 255);
/// Blend fraction that subdues inactive labels toward the pane fill (#59):
/// enough to visually recede while remaining readable. A visual parameter —
/// retune freely; not a correctness constant.
pub(crate) const INACTIVE_LABEL_BLEND: u8 = 30;
/// The Nerd Font glyph stamped as a tab block's close affordance (#86). The
/// Material Design `md-close_circle` (U+F0159) reads as a close control. It is
/// drawn in alert red, one cell in from the block's right edge (#94) so a fill
/// cell of breathing room sits between it and the corner. Terminals without a
/// Nerd Font — zellij's simplified UI, surfaced to the plugin as
/// `capabilities.arrow_fonts` — use the ASCII [`CLOSE_GLYPH_ASCII`] instead; which
/// one a tab draws is carried by [`Close`], resolved at the render site.
pub(crate) const CLOSE_GLYPH: char = '\u{F0159}';

/// ASCII close glyph for a terminal without a Nerd Font (zellij's simplified UI):
/// `×` (U+00D7) still reads as a close control in any font. Drawn in black
/// ([`CLOSE_FG_ASCII`]) at the same `pw - 2` cell as the Nerd Font glyph (#94) —
/// one cell in from the block's right edge.
pub(crate) const CLOSE_GLYPH_ASCII: char = '×';

/// Foreground for the ASCII close `×` (#94): black. The Nerd Font glyph uses the
/// theme's alert red ([`Palette::alert`]); the plain `×` reads better in black.
pub(crate) const CLOSE_FG_ASCII: Rgb = (0, 0, 0);

/// Which close affordance a tab block stamps near its top-right corner, in the form
/// the terminal can draw (#86, #94). `Off` draws none. Both on-variants sit one
/// cell in from the right edge and differ only in glyph; each carries its already
/// resolved foreground [`Rgb`]:
/// - [`NerdFont`](Close::NerdFont): [`CLOSE_GLYPH`] in the carried color.
/// - [`Ascii`](Close::Ascii): [`CLOSE_GLYPH_ASCII`] in the carried color, for a
///   terminal without a Nerd Font.
///
/// The color is resolved in [`crate::State`] by applying the `close_button_color`
/// config ([`crate::config::CloseColor`]) against the per-glyph default — the
/// theme's alert red for the Nerd Font glyph, black for the ASCII `×` — so the
/// renderer stamps it directly with no theme lookup of its own (#94 follow-up).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Close {
    /// No close affordance.
    #[default]
    Off,
    /// The Nerd Font glyph in the carried foreground — the default when a Nerd
    /// Font is available.
    NerdFont(Rgb),
    /// The ASCII `×` fallback in the carried foreground — a simplified-UI terminal.
    Ascii(Rgb),
}

impl Close {
    /// Whether any close glyph is drawn.
    pub fn is_on(self) -> bool {
        !matches!(self, Close::Off)
    }

    /// How many columns in from the block's right edge the glyph sits — and so how
    /// many right-edge columns it reserves from the badge and labels (#94). Both
    /// on-variants sit one cell in (2): the glyph takes the `pw - 2` cell and the
    /// last column stays fill, leaving one cell of breathing room at the right
    /// edge so the mark reads inset rather than crowding the corner. `Off`
    /// reserves none. This is the single source of truth shared by the renderer
    /// (placement) and the click hit-test in [`crate::State`] (the cell a
    /// `LeftClick` closes from).
    pub fn right_offset(self) -> usize {
        match self {
            Close::Off => 0,
            Close::NerdFont(_) | Close::Ascii(_) => 2,
        }
    }
}

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

/// A projected pane's block-local pixel bounding box: columns `px0..px1`
/// (`0..cols`) and pixel rows `py0..py1` (`0..2*text_rows`, two pixels per text
/// row). The exact rectangle [`render`] paints for the pane, surfaced so click
/// hit-testing ([`pane_at_cell`]) can map a clicked cell back to the pane the
/// same frame drew (#74). Half-open on every side, like the loops that consume
/// it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneBox {
    pub px0: usize,
    pub px1: usize,
    pub py0: usize,
    pub py1: usize,
}

// ---- ANSI emission ------------------------------------------------------

pub(crate) fn put_fg(out: &mut String, c: Rgb) {
    out.push_str(&format!("\x1b[38;2;{};{};{}m", c.0, c.1, c.2));
}

pub(crate) fn put_bg(out: &mut String, c: Rgb) {
    out.push_str(&format!("\x1b[48;2;{};{};{}m", c.0, c.1, c.2));
}

/// Reset the background to the terminal's default (SGR 49) — a *transparent*
/// pixel that lets the bar's own backdrop show through, instead of a painted
/// canvas color. The perspective recede inset (#66/#84) uses this so a receded
/// tab recedes into the bar background rather than behind a dark band.
pub(crate) fn put_default_bg(out: &mut String) {
    out.push_str("\x1b[49m");
}

/// Emit one half-block cell from its top and bottom pixel colors. A `None`
/// pixel is transparent — rendered on the terminal's default background
/// ([`put_default_bg`]) so the bar's backdrop shows through. The glyph is chosen
/// so the solid half always carries a real color: `▀` (upper half) when only the
/// bottom is transparent, `▄` (lower half) when only the top is. A fully
/// transparent cell is a plain space.
pub(crate) fn put_halfblock(out: &mut String, top: Option<Rgb>, bottom: Option<Rgb>) {
    match (top, bottom) {
        (Some(t), Some(b)) => {
            put_fg(out, t);
            put_bg(out, b);
            out.push('\u{2580}'); // ▀
        }
        (Some(t), None) => {
            put_default_bg(out);
            put_fg(out, t);
            out.push('\u{2580}'); // ▀ — top color over a transparent bottom
        }
        (None, Some(b)) => {
            put_default_bg(out);
            put_fg(out, b);
            out.push('\u{2584}'); // ▄ — bottom color under a transparent top
        }
        (None, None) => {
            put_default_bg(out);
            out.push(' ');
        }
    }
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

/// Geometry of the gradient sweep (config key `gradient_shape`, #71).
///
/// `Linear` sweeps along a straight direction set by [`GradientSpec::angle`];
/// `Radial` sweeps along the distance from each pane block's center.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GradientShape {
    /// A straight sweep at [`GradientSpec::angle`] degrees.
    #[default]
    Linear,
    /// A circular sweep from the block's center outward (or inward).
    Radial,
}

impl std::str::FromStr for GradientShape {
    type Err = ();

    /// `"linear"` / `"radial"` (exact match); any other value errors so the
    /// config parser falls back to the documented default.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "linear" => Ok(Self::Linear),
            "radial" => Ok(Self::Radial),
            _ => Err(()),
        }
    }
}

/// Direction of a radial sweep (config key `gradient_radial`, #71).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RadialDirection {
    /// Base fill at the center, easing toward the stop at the block's edge.
    #[default]
    Outward,
    /// Stop at the center, easing toward the base fill at the block's edge.
    Inward,
}

impl std::str::FromStr for RadialDirection {
    type Err = ();

    /// `"outward"` / `"inward"` (exact match); any other value errors so the
    /// config parser falls back to the documented default.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "outward" => Ok(Self::Outward),
            "inward" => Ok(Self::Inward),
            _ => Err(()),
        }
    }
}

/// The full gradient configuration threaded into the renderer: the sweep
/// [`GradientMode`] plus its direction (#71).
///
/// `shape`/`angle`/`radial` shape the *position parameter* `t` (0 at the base
/// fill, 100 at the stop) that the mode then consumes:
///
/// * `Linear` projects each pixel onto the unit vector at `angle` degrees over
///   the pane block's pixel bounding box. The angle is the **perceived
///   on-screen** direction the sweep advances, measured clockwise from the
///   positive x-axis: `0` = left→right (the classic [`GradientMode::Sheen`]
///   look), `90` = top→bottom, `180` = right→left, `270` = bottom→top. The
///   half-block split already makes each pixel ≈ square, so no extra cell-aspect
///   correction is applied — the projection runs in raw pixel space.
/// * `Radial` uses the normalized distance from the block's center; `Outward`
///   puts the base fill at the center, `Inward` flips it.
///
/// The mode is orthogonal: `Off` ignores all of this, `Sheen` paints `t`
/// straight, and `Weave` flips `t` on odd pixel rows for either shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GradientSpec {
    pub mode: GradientMode,
    pub shape: GradientShape,
    /// Linear sweep angle in degrees, `[0, 360)`. Ignored for `Radial`.
    pub angle: u16,
    pub radial: RadialDirection,
}

impl GradientSpec {
    /// Build a spec from a mode with the default direction (linear, angle 0,
    /// outward) — the classic per-mode look before #71's direction controls.
    pub const fn from_mode(mode: GradientMode) -> Self {
        Self {
            mode,
            shape: GradientShape::Linear,
            angle: 0,
            radial: RadialDirection::Outward,
        }
    }

    /// No sweep — flat fills, the historical look. Test/fallback convenience.
    pub const OFF: Self = Self::from_mode(GradientMode::Off);
    /// A left-to-right linear sheen — the default-direction polished look.
    #[cfg(test)]
    pub const SHEEN: Self = Self::from_mode(GradientMode::Sheen);
    /// A left-to-right linear weave.
    #[cfg(test)]
    pub const WEAVE: Self = Self::from_mode(GradientMode::Weave);
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

/// Maximum change in the sweep position parameter `t` (`0..=100`) between two
/// adjacent pixels along the gradient axis. The sweep divides the base→stop
/// range evenly over the axis's pixels, but never lets one pixel jump more than
/// this — equivalently, the full stop needs at least `100 / MAX_GRADIENT_STEP`
/// pixels of span to be reached. A short axis (a near-square pane under a steep
/// angle, where only a handful of pixels span the sweep) therefore eases gently
/// part-way to the stop instead of banding hard in a few coarse jumps. Long axes
/// are unaffected: their even step already sits below the cap, so they still
/// reach the full stop exactly.
const MAX_GRADIENT_STEP: f32 = 15.0;

/// Per-pane gradient invariants, derived once from a pane block's pixel bounds
/// and the active [`GradientSpec`] so the per-pixel sample ([`sweep_t`]) never
/// redoes the trig, projection extents, or radius. Built by [`pane_sweep`] once
/// per pane (panes ≪ pixels), then consulted for every pixel of that pane.
#[derive(Clone, Copy)]
struct PaneSweep {
    kind: SweepKind,
    /// Per-pixel `t` ceiling — `(MAX_GRADIENT_STEP * steps).min(100)`, where
    /// `steps` is the axis span (linear) or max radius (radial). Caps how far a
    /// short axis can advance toward the stop (see [`MAX_GRADIENT_STEP`]).
    max_t: f32,
    /// `Weave` mode flips the sweep on odd pixel rows; precomputed so `sweep_t`
    /// only checks row parity, not the whole [`GradientMode`].
    weave: bool,
}

/// Shape-specific precomputed terms of a [`PaneSweep`]: the projection unit
/// vector + normalization range for a linear sweep, or the center + max radius
/// for a radial one.
#[derive(Clone, Copy)]
enum SweepKind {
    /// `proj(x, y) = x*dx + y*dy`, normalized as `(proj − lo) / span`.
    Linear {
        dx: f32,
        dy: f32,
        lo: f32,
        span: f32,
    },
    /// `dist = hypot(px − cx, py − cy) / max`, then directed by `radial`.
    Radial {
        cx: f32,
        cy: f32,
        max: f32,
        radial: RadialDirection,
    },
}

/// Precompute the per-pane gradient invariants for `spec` over pane block
/// `bounds` (`(px0, px1, py0, py1)`, the half-open pixel extents), or `None`
/// when the block degenerates to a single point along the sweep axis (no
/// direction → flat base fill). Computed once per pane so [`sweep_t`] need not
/// redo the trig / projection extents / radius for every pixel.
fn pane_sweep(spec: GradientSpec, bounds: (usize, usize, usize, usize)) -> Option<PaneSweep> {
    let (px0, px1, py0, py1) = bounds;
    // Inclusive pixel extents — the projection/distance must span the painted
    // pixels (px1/py1 are exclusive), so a one-pixel axis collapses to lo == hi.
    let (xlo, xhi) = (px0 as f32, px1.saturating_sub(1).max(px0) as f32);
    let (ylo, yhi) = (py0 as f32, py1.saturating_sub(1).max(py0) as f32);
    // `steps` is the axis length in pixels — the denominator of the even sweep
    // and the lever the per-pixel cap multiplies against.
    let (kind, steps) = match spec.shape {
        GradientShape::Linear => {
            // Project onto the unit vector at `angle` degrees (clockwise, y down),
            // normalized against the span of the bbox corners along that axis. At
            // angle 0 this reduces to (px − px0)/(px1 − px0 − 1) — the classic
            // left-to-right column ratio.
            let theta = (spec.angle as f32).to_radians();
            let (dx, dy) = (theta.cos(), theta.sin());
            let proj = |x: f32, y: f32| x * dx + y * dy;
            let corners = [
                proj(xlo, ylo),
                proj(xhi, ylo),
                proj(xlo, yhi),
                proj(xhi, yhi),
            ];
            let lo = corners.iter().copied().fold(f32::INFINITY, f32::min);
            let hi = corners.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let span = hi - lo;
            if span <= f32::EPSILON {
                return None;
            }
            (SweepKind::Linear { dx, dy, lo, span }, span)
        }
        GradientShape::Radial => {
            let (cx, cy) = ((xlo + xhi) / 2.0, (ylo + yhi) / 2.0);
            // Center is the midpoint, so the farthest corner sets the max radius.
            let max = ((xhi - cx).powi(2) + (yhi - cy).powi(2)).sqrt();
            if max <= f32::EPSILON {
                return None;
            }
            (
                SweepKind::Radial {
                    cx,
                    cy,
                    max,
                    radial: spec.radial,
                },
                max,
            )
        }
    };
    Some(PaneSweep {
        kind,
        // Cap the per-pixel change: the full base→stop range needs `steps`
        // pixels, but each pixel advances at most MAX_GRADIENT_STEP, so a short
        // axis only reaches `MAX_GRADIENT_STEP * steps` instead of banding.
        max_t: (MAX_GRADIENT_STEP * steps).min(100.0),
        weave: spec.mode == GradientMode::Weave,
    })
}

/// Position parameter `t ∈ [0, 100]` of the sample point `(px, py)` under the
/// precomputed `sweep`.
///
/// The base→stop range is spread evenly across the axis but rate-limited to
/// [`MAX_GRADIENT_STEP`] per pixel, so short axes stop short of the full stop
/// (see that constant). Weave's odd-row flip is applied to the *ratio* here, so
/// both rows mirror within the same capped range.
///
/// `px`/`py` are continuous pixel coordinates so callers can sample off the
/// integer grid — a label cell asks for its text-row vertical center
/// (`2*tr + 0.5`) rather than the top pixel, keeping its single background
/// sample faithful for vertical, diagonal, and radial sweeps.
fn sweep_t(sweep: &PaneSweep, px: f32, py: f32) -> u8 {
    let ratio = match sweep.kind {
        SweepKind::Linear { dx, dy, lo, span } => (px * dx + py * dy - lo) / span,
        SweepKind::Radial {
            cx,
            cy,
            max,
            radial,
        } => {
            let dist = (((px - cx).powi(2) + (py - cy).powi(2)).sqrt() / max).clamp(0.0, 1.0);
            match radial {
                RadialDirection::Outward => dist,
                RadialDirection::Inward => 1.0 - dist,
            }
        }
    };
    // Weave flips the sweep on odd pixel rows. Flipping the *ratio* (not the
    // final t) keeps both rows mirrored within the capped range below; the parity
    // is taken on the integer row the sample falls in (a label centered at
    // `2*tr + 0.5` truncates to the even top row, matching its top-pixel sample).
    let ratio = match sweep.weave {
        true if !(py as usize).is_multiple_of(2) => 1.0 - ratio,
        _ => ratio,
    };
    // Smoothstep-ease the ratio (3r² − 2r³) so the sweep eases in and out instead
    // of ramping linearly (#46). It is symmetric (s(1−r) = 1−s(r)), so the weave
    // ratio-flip above stays a clean mirror. Endpoints are fixed points, so the
    // base fill is reached exactly and the stop is reached exactly whenever the
    // cap leaves `max_t` at 100.
    let ratio = ratio.clamp(0.0, 1.0);
    let eased = ratio * ratio * (3.0 - 2.0 * ratio);
    (eased * sweep.max_t).round() as u8
}

/// Test-only convenience composing [`pane_sweep`] + [`sweep_t`] into the old
/// single-call `t` lookup, so the unit tests read against one entry point.
/// Production paints through [`pane_sweep`] once per pane and [`sweep_t`] per
/// pixel (the per-render precompute, so the trig / extents aren't redone each
/// pixel).
#[cfg(test)]
fn gradient_t(
    spec: GradientSpec,
    bounds: (usize, usize, usize, usize),
    px: f32,
    py: f32,
) -> Option<u8> {
    Some(sweep_t(&pane_sweep(spec, bounds)?, px, py))
}

/// Fill color of pane `i` at the continuous sample point `(px, py)` — the base
/// fill swept by the pane's precomputed [`PaneSweep`] (`sweeps[i]`). A block with
/// no sweep direction, or an `Off` gradient (then `sweeps[i]` is `None`),
/// degenerates to the base fill.
fn fill_at(
    panes: &[PaneRect],
    palette: &Palette,
    sweeps: &[Option<PaneSweep>],
    i: usize,
    px: f32,
    py: f32,
) -> Rgb {
    let fill = palette.color_for(panes[i].id);
    let Some(sweep) = sweeps[i].as_ref() else {
        return fill;
    };
    // `sweep_t` already baked in Weave's odd-row flip and the per-pixel cap,
    // so every active mode just shades the base fill by the position `t`.
    crate::color::gradient_at(fill, sweep_t(sweep, px, py))
}

/// Color of the pixel at `(px, py)` in the pixel grid, or `None` for a
/// background pixel — one no pane owns, including the perspective recede inset
/// (#66/#84) — which renders transparent (the terminal's default background)
/// rather than a painted canvas color. `grid` stores the pane's *slice index*
/// (to reach `panes[i]`); the color itself is keyed on that pane's stable `id`,
/// never its position. Ring pixels are painted solid on top of the gradient
/// sweep, so the focus outline stays intact in every [`GradientMode`].
#[allow(clippy::too_many_arguments)]
fn pixel_color(
    grid: &[Option<usize>],
    ring: &[bool],
    panes: &[PaneRect],
    palette: &Palette,
    sweeps: &[Option<PaneSweep>],
    pw: usize,
    px: usize,
    py: usize,
) -> Option<Rgb> {
    match grid[py * pw + px] {
        Some(i) if ring[py * pw + px] => Some(palette.ring_for(panes[i].id)),
        Some(i) => Some(fill_at(panes, palette, sweeps, i, px as f32, py as f32)),
        None => None,
    }
}

/// The tiled group's bounding box as `(minx, miny, bw, bh)` — origins and
/// clamped span. The single source both the tiled projection ([`project_panes`])
/// and the floating overlay ([`project_floats_into`]) map through, so a float is
/// placed relative to exactly the same box the tiles were, never expanding it
/// (#110).
pub(crate) fn bbox_of(panes: &[PaneRect]) -> (u32, u32, f64, f64) {
    let minx = panes.iter().map(|p| p.x).min().unwrap_or(0);
    let miny = panes.iter().map(|p| p.y).min().unwrap_or(0);
    let maxx = panes.iter().map(|p| p.x + p.w).max().unwrap_or(1);
    let maxy = panes.iter().map(|p| p.y + p.h).max().unwrap_or(1);
    (
        minx,
        miny,
        (maxx - minx).max(1) as f64,
        (maxy - miny).max(1) as f64,
    )
}

/// Project `panes` into their block-local pixel boxes and the pixel-ownership
/// grid for a `pw`-column, `ph`-pixel-row canvas (`ph == 2 * text_rows`), with
/// `vinset` background pixel rows reserved top and bottom (#66).
///
/// Returns `(grid, boxes)`: `grid[py * pw + px]` is the *slice index* of the pane
/// owning that pixel (`None` for background), filled pane-by-pane in slice order
/// so a later pane overwrites an earlier one on any overlap; `boxes[i]` is
/// `panes[i]`'s [`PaneBox`]. Pure geometry shared by [`render`] (to paint) and
/// [`pane_at_cell`] (to hit-test), so the two can never disagree about where a
/// pane sits. The bounding-box normalization means only relative positions
/// matter, so an absolute coordinate origin is irrelevant.
fn project_panes(
    panes: &[PaneRect],
    pw: usize,
    ph: usize,
    vinset: usize,
) -> (Vec<Option<usize>>, Vec<PaneBox>) {
    let mut grid = vec![None::<usize>; ph * pw];
    if pw == 0 || ph == 0 {
        return (grid, Vec::new());
    }
    // The panes occupy the canvas minus `vinset` background pixel rows top and
    // bottom; clamped so a degenerate over-inset can't underflow the height.
    let content_ph = ph.saturating_sub(2 * vinset).max(1);
    let vinset = (ph - content_ph) / 2;

    let (minx, miny, bw, bh) = bbox_of(panes);
    let map = |v: u32, lo: u32, span: f64, out: usize| -> usize {
        (((v - lo) as f64) / span * out as f64).round() as usize
    };

    let boxes: Vec<PaneBox> = panes
        .iter()
        .map(|p| {
            let px0 = map(p.x, minx, bw, pw).min(pw);
            let px1 = match map(p.x + p.w, minx, bw, pw).min(pw) {
                hi if hi <= px0 => (px0 + 1).min(pw),
                hi => hi,
            };
            // Map into the content band (`content_ph` high) and shift down by the
            // top inset, so the reserved background rows stay unpainted (#66). The
            // start clamps one row short of the band (`content_ph - 1`, never
            // negative since `content_ph >= 1`): `round()` can map a pane's top to
            // exactly `content_ph`, which would shift `py0` into the bottom inset
            // row (`vinset + content_ph`) and paint over the reserved background —
            // a thin pane on the bottom edge is the worst case. `py1` keeps the
            // full `content_ph` (exclusive) so a pane still reaches the band's
            // bottom.
            let py0 = (vinset + map(p.y, miny, bh, content_ph).min(content_ph - 1)).min(ph);
            let py1 = match (vinset + map(p.y + p.h, miny, bh, content_ph).min(content_ph)).min(ph)
            {
                hi if hi <= py0 => (py0 + 1).min(ph),
                hi => hi,
            };
            PaneBox { px0, px1, py0, py1 }
        })
        .collect();

    for (i, b) in boxes.iter().enumerate() {
        for py in b.py0..b.py1 {
            for px in b.px0..b.px1 {
                grid[py * pw + px] = Some(i);
            }
        }
    }

    (grid, boxes)
}

/// Map floating panes into their block-local pixel boxes and ownership grid
/// through a **given** bounding box (the tiled group's), never recomputing it —
/// so floats overlay the tiled minimap without shifting it (#110). Same rounding
/// and edge-clamp as [`project_panes`]; a float outside the tiled bbox clamps to
/// the block edge and never changes `pw`/`ph`. `grid[i]` is the float slice
/// index (not id), mirroring [`project_panes`].
pub(crate) fn project_floats_into(
    floats: &[PaneRect],
    bbox: (u32, u32, f64, f64),
    pw: usize,
    ph: usize,
    vinset: usize,
) -> (Vec<Option<usize>>, Vec<PaneBox>) {
    let mut grid = vec![None::<usize>; ph * pw];
    if pw == 0 || ph == 0 || floats.is_empty() {
        return (grid, Vec::new());
    }
    let content_ph = ph.saturating_sub(2 * vinset).max(1);
    let vinset = (ph - content_ph) / 2;
    let (minx, miny, bw, bh) = bbox;
    // `saturating_sub`: a float can sit outside the tiled bbox's top-left, where
    // `v < lo` — unlike a tiled pane, which always satisfies `v >= lo` — so guard
    // the subtraction instead of underflowing.
    let map = |v: u32, lo: u32, span: f64, out: usize| -> usize {
        (((v.saturating_sub(lo)) as f64) / span * out as f64).round() as usize
    };
    let boxes: Vec<PaneBox> = floats
        .iter()
        .map(|p| {
            let px0 = map(p.x, minx, bw, pw).min(pw);
            let px1 = match map(p.x + p.w, minx, bw, pw).min(pw) {
                hi if hi <= px0 => (px0 + 1).min(pw),
                hi => hi,
            };
            let py0 = (vinset + map(p.y, miny, bh, content_ph).min(content_ph - 1)).min(ph);
            let py1 = match (vinset + map(p.y + p.h, miny, bh, content_ph).min(content_ph)).min(ph)
            {
                hi if hi <= py0 => (py0 + 1).min(ph),
                hi => hi,
            };
            PaneBox { px0, px1, py0, py1 }
        })
        .collect();
    for (i, b) in boxes.iter().enumerate() {
        for py in b.py0..b.py1 {
            for px in b.px0..b.px1 {
                grid[py * pw + px] = Some(i);
            }
        }
    }
    (grid, boxes)
}

/// The stable id of the pane drawn at block-local cell (`col`, `row`) in a
/// `cols`-by-`text_rows` minimap of `panes` (same `vinset` as the [`render`] that
/// drew it), or `None` when the cell is background, inset, or out of range (#74).
///
/// A text row packs two pane pixels via the half-block `▀` (top pixel = fg,
/// bottom pixel = bg), but a terminal reports a click at *cell* resolution — it
/// cannot say which half was clicked. When the two pixels belong to different
/// panes this **biases to the top pixel** (`2*row`), the upper half-block the
/// cell visually leads with, and falls back to the bottom pixel (`2*row+1`) only
/// when the top is background — so a click on a split-boundary cell resolves to a
/// real, drawn pane, never a panic or a phantom. Reuses the exact ownership grid
/// [`render`] paints ([`project_panes`]), so the resolved pane is always the one
/// under the cursor on screen.
pub fn pane_at_cell(
    panes: &[PaneRect],
    cols: usize,
    text_rows: usize,
    vinset: usize,
    col: usize,
    row: usize,
) -> Option<usize> {
    let pw = cols;
    let ph = text_rows * 2;
    if pw == 0 || text_rows == 0 || col >= pw || row >= text_rows {
        return None;
    }
    let (grid, _) = project_panes(panes, pw, ph, vinset);
    let at = |py: usize| grid[py * pw + col];
    at(2 * row).or_else(|| at(2 * row + 1)).map(|i| panes[i].id)
}

/// The visible floating pane id drawn at block-local cell (`col`, `row`) over a
/// tiled minimap of `tiled` with `floats` overlaid, or `None` when the cell is
/// not on any float (#110). Uses the same [`project_floats_into`] mapping
/// [`render`] paints with, so draw and hit-test never disagree. The caller tries
/// this before the tiled [`pane_at_cell`] (float priority, spec §7.1).
pub fn float_pane_at_cell(
    tiled: &[PaneRect],
    floats: &[PaneRect],
    cols: usize,
    text_rows: usize,
    vinset: usize,
    col: usize,
    row: usize,
) -> Option<usize> {
    let pw = cols;
    let ph = text_rows * 2;
    if pw == 0 || text_rows == 0 || col >= pw || row >= text_rows || floats.is_empty() {
        return None;
    }
    let (grid, _) = project_floats_into(floats, bbox_of(tiled), pw, ph, vinset);
    let at = |py: usize| grid[py * pw + col];
    at(2 * row)
        .or_else(|| at(2 * row + 1))
        .map(|i| floats[i].id)
}

/// Render `panes` into a `cols` × `text_rows` block (pixel rows = `2*text_rows`).
///
/// `vinset` reserves that many background pixel rows at **both** the top and the
/// bottom of the canvas, mapping the panes into the shorter middle band so the
/// block appears to recede — the perspective depth cue for inactive tabs (#66).
/// `0` fills the whole canvas (the default for the active tab and for bars below
/// the perspective threshold); inactive perspective blocks pass `1`, leaving a
/// half-row of background top and bottom.
///
/// Colors come from `palette`, keyed on each pane's stable id. Returns an ANSI
/// string of `text_rows` lines, each terminated by a reset and newline. `mode`
/// selects which summarized pane titles are overlaid where width allows (see
/// [`LabelMode`]). `badge`, when present, is the tab's shortcut hint stamped into
/// the block's top-left over the underlying cell color — the pane fill, or the
/// focus ring where it overlaps a focused pane's outline — so it reads as a
/// label *inside* the color block; it is dropped when the block is too narrow
/// to host its display width.
/// Empty input yields an all-background block, with the badge still stamped
/// over it when one is given and fits. `gradient` selects the per-pane fill
/// sweep — mode, shape, and direction (see [`GradientSpec`]); `Off` reproduces
/// the historical flat fills byte-for-byte, as does the default `Sheen` at
/// angle 0. `active` marks the bar's selected tab (#59): its badge and
/// every pane label render pure white ([`ACTIVE_FG`]) — the focused one also
/// bold — and its focus ring is drawn. Inactive blocks suppress the highlight
/// entirely — no ring, no bold — and subdue text toward the pane fill by
/// [`INACTIVE_LABEL_BLEND`], so the active tab reads at a glance.
///
/// `close` ([`Close`]) stamps a close affordance near the block's top-right
/// corner (#86), the mirror of the top-left badge. `Off` draws none; the Nerd
/// Font glyph draws in alert red and the ASCII `×` in black, both one cell in
/// from the right edge (#94) — full strength on the active tab, toned
/// toward the fill on inactive ones. The badge and any top-row label both yield
/// the reserved right column(s) so the glyph never overprints them. The caller
/// only enables it on grid rungs wide enough to host it (see
/// [`crate::tab_block::assemble`]) and records the matching click cell — at the
/// same per-mode [`Close::right_offset`] — so a `LeftClick` there closes the tab.
#[allow(clippy::too_many_arguments)]
pub fn render(
    panes: &[PaneRect],
    palette: &Palette,
    cols: usize,
    text_rows: usize,
    vinset: usize,
    mode: LabelMode,
    badge: Option<&str>,
    close: Close,
    gradient: GradientSpec,
    active: bool,
    floats: crate::floating::FloatLayer<'_>,
) -> String {
    let pw = cols;
    let ph = text_rows * 2;
    if pw == 0 || text_rows == 0 {
        return String::new();
    }
    // Project every pane to its block-local pixel box and the pixel-ownership
    // grid in one shared step (#74): `render` paints from them here, and
    // [`pane_at_cell`] hit-tests against the same `project_panes` output, so a
    // click can never resolve to a pane other than the one drawn.
    let (grid, bounds) = project_panes(panes, pw, ph, vinset);
    // The visible floating layer overlays the tiled grid, mapped through the same
    // bbox so it sits in place without shifting the tiles (#110). Kept in its own
    // grid/boxes so the tiled `grid[i]`/`panes[i]` index space is never mixed.
    let float_rects: &[PaneRect] = match floats {
        crate::floating::FloatLayer::Visible(f) => f,
        _ => &[],
    };
    let (float_grid, float_bounds) = if float_rects.is_empty() {
        (Vec::new(), Vec::new())
    } else {
        project_floats_into(float_rects, bbox_of(panes), pw, ph, vinset)
    };
    // Float border pixels: the outline of each float box, drawn in the float's
    // `ring_for` shade so it reads as a distinct pane floating above the tiles.
    let mut float_ring = vec![false; float_grid.len()];
    for b in &float_bounds {
        for py in b.py0..b.py1 {
            for px in b.px0..b.px1 {
                let edge = px == b.px0 || px == b.px1 - 1 || py == b.py0 || py == b.py1 - 1;
                if edge {
                    float_ring[py * pw + px] = true;
                }
            }
        }
    }
    let mut ring = vec![false; ph * pw]; // focus-ring pixels
    let mut overlay = vec![None::<OverlayCell>; text_rows * pw]; // label cells

    // The shortcut badge occupies the top text row's left cells, after a
    // one-cell margin, drawn over the underlying cell color — the pane fill, or
    // the focus ring where it sits on a focused pane's outline — so it reads
    // inside the block, integrating with the ring rather than punching a
    // fill-colored hole in it. It is dropped wholesale when it would not fit
    // within the width. Computed before the pane loop so a label biased onto the
    // badge row (#65) can clear the badge's columns.
    const BADGE_COL: usize = 1;
    // Stamping is by display column (#57): each badge glyph claims its leading
    // cell (`Some`) plus one `None` continuation cell per extra column it
    // spans, mirroring the label overlay in the pane loop below — `shortcut_prefix`
    // is user-configurable, so a fullwidth glyph must advance two cells to stay
    // in lockstep with the terminal's advance. Zero-width chars are skipped.
    // Fitting is judged on the resulting per-column length; a badge wider than
    // the block is dropped wholesale rather than split mid-glyph.
    let badge_cells: Vec<Option<char>> = badge
        .map(|text| {
            text.chars()
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
    // The close glyph (#86) sits near the block's top-right corner, balancing the
    // `⌘N` badge in the opposite corner. Both modes sit one cell in from the right
    // edge, leaving a fill cell of breathing room at the corner (#94) —
    // `Close::right_offset` is that inset, the single source of truth
    // shared with the click hit-test in `State::render`. `close` is only ever set
    // on grid rungs (pw >= L2_MIN = 5), so `pw - offset` (offset ≤ 2) is always a
    // valid column; `.max(1)` keeps the dead `Off` case (close_col unused) in range.
    let close_on = close.is_on();
    let close_reserve = close.right_offset();
    let close_col = pw.saturating_sub(close_reserve.max(1));
    // The close glyph only appears on tabs that don't recede — the active tab
    // (never receded) and, when perspective is off, every tab (#86) — so it
    // always rides the top text row, beside the badge. (Inactive perspective
    // tabs inset their top row (#66/#84) and simply carry no close glyph.)
    let close_text_row = 0;
    // Pair each glyph with the foreground `close` already carries (#94 follow-up):
    // the Nerd Font glyph or the ASCII `×`, in the color resolved up in
    // `State::render`; `None` when no close is drawn. The inactive tone is mixed
    // per cell at the paint site from this base.
    let close_render: Option<(char, Rgb)> = match close {
        Close::Off => None,
        Close::NerdFont(fg) => Some((CLOSE_GLYPH, fg)),
        Close::Ascii(fg) => Some((CLOSE_GLYPH_ASCII, fg)),
    };
    // The close cell(s) are off-limits to the badge when close is on, so the two
    // corner overlays never collide on a narrow rung.
    let badge_fits = !badge_cells.is_empty()
        && BADGE_COL + badge_cells.len() <= pw.saturating_sub(close_reserve);

    for (i, p) in panes.iter().enumerate() {
        // `project_panes` already computed this pane's box and filled the grid;
        // read the box back to drive the focus ring and label overlays below.
        let PaneBox { px0, px1, py0, py1 } = bounds[i];

        let cw = px1 - px0;
        let chh = py1 - py0;

        // Focus emphasis: bright outline if the region is big enough to read an
        // outline (≥3×3 px), otherwise brighten the whole (tiny) region. Drawn
        // only on the bar's selected tab (#59) — an inactive tab's focused
        // pane carries no highlight at all.
        if active && p.focused {
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
            let label_width = crate::title::charwise_width(&label);
            // Row selection. A text-row label paints both half-block pixels of
            // its cell with this pane's fill, so the centered choice (`mid`) can
            // bleed: for a pane spanning exactly two text rows it is the lower
            // row, whose bottom pixel (`2*mid+1`) belongs to the pane below when
            // the region ends on an odd pixel boundary. Bias up to the pane's
            // first text row (`trow0`) in that case — it is wholly owned (its top
            // pixel is inside the pane, guaranteed by `2*trow0 >= py0`), so the
            // label sits clear of the boundary. For a top-of-split pane this is
            // row 0 (#65). The taller side of an asymmetric split (lower row
            // fully owned) and any pane spanning ≥3 text rows are unaffected.
            let mid = trow0 + cell_text_rows / 2;
            let bias_up = cell_text_rows == 2 && 2 * mid + 1 >= py1 && 2 * trow0 >= py0;
            let hi = px1.saturating_sub(1);
            // When the bias lands on text row 0 (`mid >= 1`, so only the bias
            // reaches it) the shortcut badge holds that row's left cells, so the
            // label must start no earlier than one gap past it.
            let badge_clear = BADGE_COL + badge_cells.len() + 1;
            let dodge = bias_up && trow0 == 0 && badge_fits;
            // Bias up only when the label then still fits to the right of the
            // badge; otherwise keep the centered lower row, accepting the
            // documented downward bleed rather than fragmenting the label
            // against the badge.
            let row = if bias_up && (!dodge || badge_clear + label_width <= hi) {
                trow0
            } else {
                mid
            };
            // Placement is by display column (#57): the label is centered by
            // its display width and each glyph claims one cell per column it
            // spans (see [`OverlayCell`]), so a CJK rename overlays like any
            // ASCII title. Zero-width chars are skipped — emitting a joiner
            // would let a sequence-collapsing terminal advance fewer columns
            // than the overlay claimed — so width is priced per char
            // ([`crate::title::charwise_width`]) to match what is emitted.
            // `summarize` budgets with the same pricing, so the label always
            // fits `inner` and the edge guard below only defends against a wide
            // glyph straddling the region's right border.
            if label_width >= 1 && row < text_rows {
                // Center on the pane's full inner width, then nudge right only
                // as far as the badge forces — a wide pane reads centered and
                // only a tight one shifts off-center (#65 follow-up).
                let centered = px0 + 1 + inner.saturating_sub(label_width) / 2;
                let start = if row == 0 && badge_fits {
                    centered.max(badge_clear)
                } else {
                    centered
                };
                // The close glyph sits near the top-right corner, so a label on the
                // right-edge pane sharing its row must stop short of it — the
                // mirror of the badge's left-edge `start` nudge above (#86).
                // `close` is only set on non-receding tabs, so the glyph's row is
                // the top one (`close_text_row`); `close_col` already carries the
                // per-mode inset (#94).
                let right_bound = if close_on && row == close_text_row {
                    px1.min(close_col)
                } else {
                    px1
                };
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
                    .take_while(|(at, _, w)| at + w <= right_bound)
                    .for_each(|(at, ch, w)| {
                        overlay[row * pw + at] = Some(OverlayCell::Glyph(ch, i));
                        (at + 1..at + w).for_each(|cc| {
                            overlay[row * pw + cc] = Some(OverlayCell::Continuation);
                        });
                    });
            }
        }
    }

    // Precompute each pane's sweep invariants once (trig / projection extents /
    // radius), so the per-pixel `sweep_t` below is a few mul-adds. `Off` skips
    // the sweep entirely — every pane reads its flat base fill.
    let sweeps: Vec<Option<PaneSweep>> = match gradient.mode {
        GradientMode::Off => vec![None; bounds.len()],
        _ => bounds
            .iter()
            .map(|b| pane_sweep(gradient, (b.px0, b.px1, b.py0, b.py1)))
            .collect(),
    };

    // Hidden floating panes are chipped into the bottom text row's right corner
    // (#110): one selectable glyph per float, laid out by `chip_cells`. Only the
    // `Hidden` layer draws chips; `None`/`Visible` leave `chip_ids` empty, so the
    // chip layout is empty and this is a no-op (the visible overlay is a separate
    // arm added in P3). `chip_row` is the bottom text row (`text_rows >= 1` here).
    let chip_ids: &[usize] = match floats {
        crate::floating::FloatLayer::Hidden(ids) => ids,
        _ => &[],
    };
    let chip_layout: Vec<(usize, crate::floating::Chip)> =
        crate::floating::chip_cells(pw, chip_ids.len());
    let chip_row = text_rows - 1;

    let mut out = String::with_capacity(text_rows * pw * 24);
    for tr in 0..text_rows {
        for c in 0..pw {
            if tr == 0 && badge_fits && (BADGE_COL..BADGE_COL + badge_cells.len()).contains(&c) {
                // A `None` cell sits under the trailing column of a wide badge
                // glyph, which already covers it on screen — emit nothing.
                let Some(ch) = badge_cells[c - BADGE_COL] else {
                    continue;
                };
                // The badge rides the top text row, whose upper pixel is the
                // recede inset on a receded tab — transparent there, so the
                // glyph reads on the bar backdrop instead of a painted band.
                let fill = pixel_color(&grid, &ring, panes, palette, &sweeps, pw, c, 0);
                match fill {
                    Some(f) => put_bg(&mut out, f),
                    None => put_default_bg(&mut out),
                }
                // Active tab: white badge — vivid fill + white = maximum contrast.
                // Inactive tab: muted toward the fill — recedes gracefully (#59).
                let badge_fg = if active {
                    ACTIVE_FG
                } else {
                    crate::color::mixed(
                        ACTIVE_FG,
                        fill.unwrap_or(crate::color::CANVAS),
                        INACTIVE_LABEL_BLEND,
                    )
                };
                put_fg(&mut out, badge_fg);
                out.push_str("\x1b[1m");
                out.push(ch);
                out.push_str("\x1b[22m");
                continue;
            }
            if tr == close_text_row && c == close_col {
                if let Some((glyph, base)) = close_render {
                    // The close glyph mirrors the badge in the opposite corner
                    // (#86): the Nerd Font glyph in alert red ([`Palette::alert`],
                    // from the theme's `exit_code_error.base`), the ASCII `×` in
                    // black (#94) — full strength on the active tab, toned toward
                    // the fill where perspective is off and inactive tabs still
                    // carry it. It rides the top text row (`close_text_row`) — the
                    // tabs that show it never recede — and is sampled over that
                    // row's upper pixel (`2 * close_text_row`). Its cell is reserved
                    // from the badge and any same-row label, so it never overprints
                    // them.
                    let fill = pixel_color(
                        &grid,
                        &ring,
                        panes,
                        palette,
                        &sweeps,
                        pw,
                        c,
                        2 * close_text_row,
                    );
                    match fill {
                        Some(f) => put_bg(&mut out, f),
                        None => put_default_bg(&mut out),
                    }
                    let close_fg = if active {
                        base
                    } else {
                        crate::color::mixed(
                            base,
                            fill.unwrap_or(crate::color::CANVAS),
                            INACTIVE_LABEL_BLEND,
                        )
                    };
                    put_fg(&mut out, close_fg);
                    out.push_str("\x1b[1m");
                    out.push(glyph);
                    out.push_str("\x1b[22m");
                    continue;
                }
            }
            // Hidden-float chips ride the bottom text row's right corner (#110),
            // mirroring the badge/close reservation: one glyph per cell over the
            // underlying fill, sampled over the row's upper pixel. Takes priority
            // over a same-cell label so the affordance is never overprinted — the
            // chip is the sole click target for a hidden float (issue #2).
            if tr == chip_row {
                if let Some((_, chip)) = chip_layout.iter().find(|(cc, _)| *cc == c) {
                    let fill =
                        pixel_color(&grid, &ring, panes, palette, &sweeps, pw, c, 2 * chip_row);
                    match fill {
                        Some(f) => put_bg(&mut out, f),
                        None => put_default_bg(&mut out),
                    }
                    // A float chip takes its own float's color; the `+k` overflow
                    // marker takes the accent so it reads apart from the chips.
                    let base = match chip {
                        crate::floating::Chip::Float(idx) => palette.color_for(chip_ids[*idx]),
                        crate::floating::Chip::PlusK(_) => palette.accent(),
                    };
                    let chip_fg = if active {
                        base
                    } else {
                        crate::color::mixed(
                            base,
                            fill.unwrap_or(crate::color::CANVAS),
                            INACTIVE_LABEL_BLEND,
                        )
                    };
                    put_fg(&mut out, chip_fg);
                    out.push(match chip {
                        crate::floating::Chip::Float(_) => crate::floating::CHIP_GLYPH,
                        crate::floating::Chip::PlusK(_) => crate::floating::CHIP_MORE_GLYPH,
                    });
                    continue;
                }
            }
            // A continuation cell is already covered on screen by its wide
            // glyph's advance — emit nothing so cells stay in lockstep.
            if let Some(OverlayCell::Continuation) = overlay[tr * pw + c] {
                continue;
            }
            if let Some(OverlayCell::Glyph(ch, i)) = overlay[tr * pw + c] {
                // Active tab: white label on vivid fill — prominent. Focused pane
                // also bold for maximum emphasis. Inactive tab: text muted toward
                // the pane fill so it recedes without becoming unreadable (#59).
                let highlighted = active && panes[i].focused;
                // A label paints one background sample for its whole cell; take it
                // at the text row's vertical center (`2*tr + 0.5`) so a vertical,
                // diagonal, or radial sweep reads correctly through the text
                // (at angle 0 the row is irrelevant — byte-identical to the
                // pre-#71 top-pixel sample).
                let label_fill =
                    fill_at(panes, palette, &sweeps, i, c as f32, 2.0 * tr as f32 + 0.5);
                put_bg(&mut out, label_fill);
                let label_fg = if active {
                    ACTIVE_FG
                } else {
                    crate::color::mixed(ACTIVE_FG, label_fill, INACTIVE_LABEL_BLEND)
                };
                put_fg(&mut out, label_fg);
                if highlighted {
                    out.push_str("\x1b[1m");
                    out.push(ch);
                    out.push_str("\x1b[22m");
                    continue;
                }
                out.push(ch);
                continue;
            }
            // A visible float, when present at this pixel, paints on top of the
            // tiled fill (float priority, spec §7.1): its border pixels take the
            // float's `ring_for` shade, its interior the float's `color_for`.
            // `float_grid` is length-0 when there is no visible layer, so `.get`
            // returns `None` and the tiled `pixel_color` shows through unchanged —
            // the no-float path stays byte-identical (#110).
            let float_px = |py: usize| -> Option<Rgb> {
                let i = (*float_grid.get(py * pw + c)?)?;
                Some(if float_ring[py * pw + c] {
                    palette.ring_for(float_rects[i].id)
                } else {
                    palette.color_for(float_rects[i].id)
                })
            };
            let top = float_px(2 * tr)
                .or_else(|| pixel_color(&grid, &ring, panes, palette, &sweeps, pw, c, 2 * tr));
            let bottom = float_px(2 * tr + 1)
                .or_else(|| pixel_color(&grid, &ring, panes, palette, &sweeps, pw, c, 2 * tr + 1));
            put_halfblock(&mut out, top, bottom);
        }
        out.push_str(RESET);
        out.push('\n');
    }
    out
}

/// Render the inline new-tab `+` button (#76) as a `width`-by-`rows` block that
/// reads as a single-pane **inactive** tab: a flat [`crate::color::button_fill`]
/// band, receding `vinset` half-rows of *transparent* inset at the top and
/// bottom, with a [`crate::color::button_glyph`]-colored `+` centered on the
/// middle row.
///
/// The recede inset is transparent (the terminal's default background via
/// [`put_halfblock`]), not a painted canvas color: a `BG`-painted inset read as
/// a hard top/bottom frame against the flat fill (#84), and once the *inactive
/// tabs'* own recede insets also went transparent the band-vs-fill seam was the
/// whole bug. With the inset transparent the flat fill recedes cleanly into the
/// bar backdrop, so the button needs no gradient — staying flat is what keeps it
/// visually distinct from the gradient-swept tabs. The recede still delivers the
/// height match an inactive tab gets (#76).
///
/// Returns an ANSI string of `rows` lines, each terminated by a reset and no
/// trailing newline, so the caller frames it through the same
/// [`crate::paint::compose`] path as a tab block (which consumes the lines via
/// `.lines()`, ignoring any trailing terminator).
pub(crate) fn button(width: usize, rows: usize, vinset: usize) -> String {
    let pw = width;
    let ph = rows * 2;
    if pw == 0 || rows == 0 {
        return String::new();
    }
    let fill = crate::color::button_fill();
    let glyph_fg = crate::color::button_glyph();
    // Mirror `render`'s vinset clamp: reserve `vinset` transparent pixel rows at
    // the top and bottom, the button occupying the middle band (always ≥ one
    // pixel). A pixel inside the band is the flat fill; outside it is transparent.
    let content_ph = ph.saturating_sub(2 * vinset).max(1);
    let vinset = (ph - content_ph) / 2;
    let pixel = |py: usize| (py >= vinset && py < vinset + content_ph).then_some(fill);
    let middle = rows / 2;
    // Center the `+` by display width, matching the flat renderer it replaces.
    let glyph_col = (pw - 1) / 2;
    (0..rows)
        .map(|tr| {
            let mut out = String::new();
            (0..pw).for_each(|c| match (tr == middle && c == glyph_col).then_some(()) {
                // The glyph rides the middle row, never a recede row, so the cell
                // beneath it is solid fill.
                Some(()) => {
                    put_bg(&mut out, fill);
                    put_fg(&mut out, glyph_fg);
                    out.push('+');
                }
                None => put_halfblock(&mut out, pixel(2 * tr), pixel(2 * tr + 1)),
            });
            out.push_str(RESET);
            out
        })
        .collect::<Vec<_>>()
        .join("\n")
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

    /// A distinctive resolved close foreground for tests that only care about the
    /// glyph's placement, not its color — `Close` now carries an already-resolved
    /// color (#94 follow-up), so geometry tests pass this stand-in.
    const TEST_CLOSE_FG: Rgb = (200, 30, 40);

    fn bg(c: Rgb) -> String {
        format!("\x1b[48;2;{};{};{}m", c.0, c.1, c.2)
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
            0,
            LabelMode::None,
            None,
            Close::Off,
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::None,
        );
        assert_eq!(out.lines().count(), 3);
    }

    #[test]
    fn vinset_recedes_the_top_and_bottom_rows() -> Result<(), Box<dyn std::error::Error>> {
        // #66 perspective: a one-pixel `vinset` reserves a half text row of
        // background at the top and bottom of the canvas, mapping the panes into
        // the shorter middle band. For a single full-tab pane in a 4-row block
        // that means the top text row's upper pixel and the bottom text row's
        // lower pixel become background (the recede that lifts the active tab),
        // while the middle rows stay fully filled. The recede pixel renders
        // *transparent* (SGR 49, terminal default), so it shows the bar backdrop
        // rather than a painted canvas band (#84).
        let transparent = "\x1b[49m";
        let panes = vec![PaneRect::new(0, 0, 0, 100, 40, "x", false)];
        let palette = test_palette();
        let fill = palette.color_for(0);
        let out = render(
            &panes,
            &palette,
            4,
            4,
            1,
            LabelMode::None,
            None,
            Close::Off,
            GradientSpec::OFF,
            false,
            crate::floating::FloatLayer::None,
        );
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 4);
        // Top row `▄`: upper pixel transparent, lower pixel the pane fill.
        assert!(
            lines[0].starts_with(&format!("{transparent}{}\u{2584}", fg(fill))),
            "top row must recede to transparent over fill: {:?}",
            lines[0]
        );
        // Bottom row `▀`: upper pixel the pane fill, lower pixel transparent.
        assert!(
            lines[3].starts_with(&format!("{transparent}{}\u{2580}", fg(fill))),
            "bottom row must recede to fill over transparent: {:?}",
            lines[3]
        );
        // A middle row stays fully the pane fill — no recede, no transparency.
        assert!(
            lines[1].starts_with(&format!("{}{}\u{2580}", fg(fill), bg(fill))),
            "middle row must stay fully filled: {:?}",
            lines[1]
        );
        // Contrast: with no inset the same top row is fully filled, proving the
        // recede above is the inset and not some other effect.
        let full = render(
            &panes,
            &palette,
            4,
            4,
            0,
            LabelMode::None,
            None,
            Close::Off,
            GradientSpec::OFF,
            false,
            crate::floating::FloatLayer::None,
        );
        assert!(
            full.lines()
                .next()
                .ok_or("a rendered row")?
                .starts_with(&format!("{}{}\u{2580}", fg(fill), bg(fill))),
            "with vinset 0 the top row fills completely"
        );
        Ok(())
    }

    #[test]
    fn vinset_keeps_a_thin_bottom_pane_out_of_the_reserved_row()
    -> Result<(), Box<dyn std::error::Error>> {
        // #66 regression: `round()` can map a pane's top to exactly `content_ph`,
        // which — without clamping the start to `content_ph - 1` — shifts `py0`
        // into the reserved bottom inset row and paints over it, spoiling the
        // recede. A thin pane hugging the bottom edge of a tall layout is the
        // trigger: its top (`y=39` of `40`) rounds up to the band height. The
        // bottom text row's lower pixel must stay background regardless.
        let panes = vec![
            PaneRect::new(0, 0, 0, 100, 39, "top", false),
            PaneRect::new(1, 0, 39, 100, 1, "edge", false),
        ];
        let palette = test_palette();
        // The thin edge pane (index 1) overwrites pane 0 at the band's last pixel,
        // so it owns the bottom text row's upper pixel.
        let edge_fill = palette.color_for(1);
        let out = render(
            &panes,
            &palette,
            4,
            4,
            1,
            LabelMode::None,
            None,
            Close::Off,
            GradientSpec::OFF,
            false,
            crate::floating::FloatLayer::None,
        );
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 4);
        // Bottom row must stay `▀` (fill over transparent inset); the buggy clamp
        // painted the thin pane into the lower pixel too, turning the cell into a
        // full █ block (bg = fill instead of the reserved transparent inset).
        assert!(
            lines[3].starts_with(&format!("\x1b[49m{}\u{2580}", fg(edge_fill))),
            "the reserved bottom inset must survive a thin bottom-edge pane: {:?}",
            lines[3]
        );
        Ok(())
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
            0,
            LabelMode::None,
            None,
            Close::Off,
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::None,
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
            0,
            LabelMode::None,
            None,
            Close::Off,
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::None,
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
            0,
            LabelMode::None,
            None,
            Close::Off,
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::None,
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
                0,
                LabelMode::None,
                None,
                Close::Off,
                GradientSpec::OFF,
                true,
                crate::floating::FloatLayer::None,
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
                0,
                LabelMode::None,
                None,
                Close::Off,
                GradientSpec::OFF,
                true,
                crate::floating::FloatLayer::None,
            )
            .is_empty()
        );
        assert!(
            render(
                &one_focused(),
                &test_palette(),
                10,
                0,
                0,
                LabelMode::None,
                None,
                Close::Off,
                GradientSpec::OFF,
                true,
                crate::floating::FloatLayer::None,
            )
            .is_empty()
        );
    }

    #[test]
    fn render_stamps_a_chip_for_each_hidden_float() {
        // Two hidden floats (ids 7, 9) over a lone tiled pane in a 12-wide, 3-row
        // block: the bottom row's two rightmost cells carry the chip glyph. Width
        // per row stays exactly 12 (chips never widen it).
        let palette = test_palette();
        let panes = one_focused();
        let hidden = [7usize, 9usize];
        let out = render(
            &panes,
            &palette,
            12,
            3,
            0,
            LabelMode::None,
            None,
            Close::Off,
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::Hidden(&hidden),
        );
        assert!(out.contains(crate::floating::CHIP_GLYPH), "chips are drawn");
        // Two chips → the glyph appears twice.
        assert_eq!(out.matches(crate::floating::CHIP_GLYPH).count(), 2);
        // Each row still measures exactly 12 display columns. Use the established
        // minimap test pattern: `visible_lines` strips ANSI, then measure width.
        for line in visible_lines(&out) {
            assert_eq!(unicode_width::UnicodeWidthStr::width(line.as_str()), 12);
        }
    }

    #[test]
    fn render_without_floats_is_byte_identical_to_none() {
        // `FloatLayer::None` must reproduce the pre-#110 output exactly, and an
        // empty hidden layer draws no chips (byte-identical to None).
        let palette = test_palette();
        let panes = one_focused();
        let base = |floats| {
            render(
                &panes,
                &palette,
                12,
                3,
                0,
                LabelMode::None,
                None,
                Close::Off,
                GradientSpec::OFF,
                true,
                floats,
            )
        };
        let empty: [usize; 0] = [];
        assert_eq!(
            base(crate::floating::FloatLayer::None),
            base(crate::floating::FloatLayer::Hidden(&empty)),
            "no floats draws no chips"
        );
    }

    #[test]
    fn render_overlays_a_visible_float_on_top_of_the_tiled_grid() {
        // A tiled pane (id 0) fills the block; a visible float (id 7) sits in the
        // middle. The float's color (keyed on id 7) must appear in the output, on
        // top of the tiled fill, and the row width stays exact.
        let palette = test_palette();
        let tiled = [PaneRect::new(0, 0, 0, 100, 40, "t", false)];
        let floats = [PaneRect::new(7, 30, 12, 40, 16, "f", false)];
        let out = render(
            &tiled,
            &palette,
            16,
            4,
            0,
            LabelMode::None,
            None,
            Close::Off,
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::Visible(&floats),
        );
        let float_fg = format!(
            "\x1b[38;2;{};{};{}m",
            palette.color_for(7).0,
            palette.color_for(7).1,
            palette.color_for(7).2
        );
        assert!(
            out.contains(&float_fg),
            "the visible float paints its own color on top"
        );
        // Width contract, measured the same way as Task 5 (ANSI stripped via
        // `visible_lines`, then `UnicodeWidthStr::width`).
        for line in visible_lines(&out) {
            assert_eq!(unicode_width::UnicodeWidthStr::width(line.as_str()), 16);
        }
    }

    #[test]
    fn float_pane_at_cell_resolves_a_visible_float_over_the_tiled_pane() {
        // Same geometry: a click in the float's box resolves to float id 7 (float
        // priority); a click outside it falls through to None (caller then tries
        // the tiled hit-test).
        let tiled = [PaneRect::new(0, 0, 0, 100, 40, "t", false)];
        let floats = [PaneRect::new(7, 30, 12, 40, 16, "f", false)];
        // Center of the float's box in a 16x4 block.
        assert_eq!(float_pane_at_cell(&tiled, &floats, 16, 4, 0, 8, 1), Some(7));
        // Top-left corner is tiled-only → the float hit-test misses.
        assert_eq!(float_pane_at_cell(&tiled, &floats, 16, 4, 0, 0, 0), None);
    }

    #[test]
    fn render_empty_panes_is_all_transparent() {
        // No panes → every pixel is background, which now renders transparent
        // (SGR 49) so the empty block shows the bar backdrop rather than a
        // painted canvas color (#84).
        let out = render(
            &[],
            &test_palette(),
            4,
            2,
            0,
            LabelMode::None,
            None,
            Close::Off,
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::None,
        );
        assert_eq!(out.lines().count(), 2);
        assert!(
            out.contains("\x1b[49m"),
            "empty block must reset to the default (transparent) background"
        );
        assert!(
            !out.contains("\x1b[48;2;"),
            "empty block must paint no truecolor background"
        );
    }

    // ---- pane_at_cell (#74) ----------------------------------------------

    #[test]
    fn pane_at_cell_resolves_a_horizontal_split() {
        // Two side-by-side panes over an 8-column block (id 7 left, id 3 right).
        // The left columns resolve to pane 7, the right columns to pane 3 — the
        // finer hit-test the column-only tab switch (#8) could not make.
        let panes = vec![
            PaneRect::new(7, 0, 0, 40, 24, "sh", false),
            PaneRect::new(3, 40, 0, 40, 24, "sh", false),
        ];
        assert_eq!(pane_at_cell(&panes, 8, 3, 0, 2, 1), Some(7), "left half");
        assert_eq!(pane_at_cell(&panes, 8, 3, 0, 6, 1), Some(3), "right half");
    }

    #[test]
    fn pane_at_cell_biases_a_split_boundary_cell_to_the_top_pixel() {
        // Two stacked panes split mid-block: pane 7 owns pixel rows 0..3, pane 3
        // owns 3..6. Text row 1 packs pixels 2 (pane 7) and 3 (pane 3) into one
        // cell; the terminal can't say which half was clicked, so the hit-test
        // biases to the top pixel — pane 7. Rows wholly inside one pane resolve
        // unambiguously.
        let panes = vec![
            PaneRect::new(7, 0, 0, 80, 12, "sh", false),
            PaneRect::new(3, 0, 12, 80, 12, "sh", false),
        ];
        assert_eq!(pane_at_cell(&panes, 4, 3, 0, 1, 0), Some(7), "top row → 7");
        assert_eq!(
            pane_at_cell(&panes, 4, 3, 0, 1, 1),
            Some(7),
            "boundary cell biases to the top pixel (pane 7)"
        );
        assert_eq!(
            pane_at_cell(&panes, 4, 3, 0, 1, 2),
            Some(3),
            "bottom row → 3"
        );
    }

    #[test]
    fn pane_at_cell_covers_a_single_pane_everywhere() {
        // A lone pane fills the whole block, so every in-range cell resolves to
        // it; an out-of-range column or row, and a click on an empty block,
        // resolve to nothing.
        let panes = vec![PaneRect::new(5, 0, 0, 80, 24, "sh", true)];
        assert_eq!(pane_at_cell(&panes, 4, 3, 0, 0, 0), Some(5));
        assert_eq!(pane_at_cell(&panes, 4, 3, 0, 3, 2), Some(5));
        assert_eq!(
            pane_at_cell(&panes, 4, 3, 0, 4, 0),
            None,
            "column past width"
        );
        assert_eq!(pane_at_cell(&panes, 4, 3, 0, 0, 3), None, "row past height");
        assert_eq!(pane_at_cell(&[], 4, 3, 0, 0, 0), None, "empty block");
    }

    #[test]
    fn project_panes_yields_an_empty_grid_for_a_zero_dimension_canvas() {
        // Belt-and-suspenders: `pane_at_cell` already rejects a zero-width or
        // zero-row block before projecting, but `project_panes` is the shared
        // geometry source and must stay safe on its own — never index a
        // `ph * pw == 0` buffer — so a degenerate canvas yields an empty grid
        // and no pane boxes.
        let panes = vec![PaneRect::new(0, 0, 0, 80, 24, "sh", false)];

        let (grid, boxes) = project_panes(&panes, 0, 4, 0);
        assert!(grid.is_empty(), "zero width → empty grid");
        assert!(boxes.is_empty(), "zero width → no pane boxes");

        let (grid, boxes) = project_panes(&panes, 8, 0, 0);
        assert!(grid.is_empty(), "zero height → empty grid");
        assert!(boxes.is_empty(), "zero height → no pane boxes");
    }

    #[test]
    fn project_floats_into_maps_through_the_tiled_bbox_without_expanding_it() {
        // A tiled pane spans (0,0,100,40); a float sits at (50,20,20,10) inside it.
        // Mapped through the tiled bbox into an 8x8-pixel canvas, the float lands in
        // the lower-right quadrant — and the tiled bbox is unchanged by the float.
        let tiled = [PaneRect::new(0, 0, 0, 100, 40, "t", false)];
        let (_, tiled_boxes) = project_panes(&tiled, 8, 8, 0);
        let bbox = bbox_of(&tiled);
        let floats = [PaneRect::new(7, 50, 20, 20, 10, "f", false)];
        let (fgrid, fboxes) = project_floats_into(&floats, bbox, 8, 8, 0);
        // The tiled pane still fills the whole canvas (float did not expand its bbox).
        assert_eq!(
            tiled_boxes[0],
            PaneBox {
                px0: 0,
                px1: 8,
                py0: 0,
                py1: 8
            }
        );
        // The float occupies a sub-rectangle in the lower-right, not the whole canvas.
        let fb = fboxes[0];
        assert!(
            fb.px0 >= 4 && fb.py0 >= 4,
            "float maps to the lower-right quadrant: {fb:?}"
        );
        assert!(fb.px1 <= 8 && fb.py1 <= 8);
        // Its grid cells point back to float index 0.
        assert_eq!(fgrid[fb.py0 * 8 + fb.px0], Some(0));
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
            0,
            LabelMode::All,
            None,
            Close::Off,
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::None,
        );
        assert!(wide.contains('c'), "expected label text in a wide block");
        // Too narrow (cw < 4 after normalization): no label, only block glyphs.
        let narrow = render(
            &panes,
            &test_palette(),
            3,
            3,
            0,
            LabelMode::All,
            None,
            Close::Off,
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::None,
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
            0,
            LabelMode::All,
            None,
            Close::Off,
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::None,
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
            0,
            LabelMode::All,
            None,
            Close::Off,
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::None,
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
            0,
            LabelMode::All,
            None,
            Close::Off,
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::None,
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
    fn top_split_label_biases_to_the_first_row_and_clears_the_badge() {
        // A top/bottom split: the top pane spans pixel rows 0..3 (text row 0
        // wholly, plus the upper pixel of text row 1). The centered choice would
        // put its label on text row 1, whose lower pixel belongs to the bottom
        // pane — the downward bleed. It must instead land on text row 0, leaving
        // the shared middle row clean. With room to spare the label stays
        // centered on the pane's full width (it already clears the badge, so no
        // rightward nudge): `top` lands at cols 5..8 of a 14-wide block, not
        // pushed up against the badge. The bottom pane keeps its label on the
        // row it fully owns (#65).
        let panes = vec![
            PaneRect::new(0, 0, 0, 100, 20, "top", false),
            PaneRect::new(1, 0, 20, 100, 20, "bot", true),
        ];
        let out = render(
            &panes,
            &test_palette(),
            14,
            3,
            0,
            LabelMode::All,
            Some("F1"),
            Close::Off,
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::None,
        );
        let lines = visible_lines(&out);
        assert_eq!(
            lines[0], "▀F1▀▀top▀▀▀▀▀▀",
            "top label belongs on the first row, centered clear of the badge, got {lines:?}"
        );
        assert!(
            !lines[1].contains("top"),
            "the label must not bleed onto the shared middle row, got {lines:?}"
        );
        assert!(
            lines[2].contains("bot"),
            "the bottom pane keeps its label on its owned row, got {lines:?}"
        );
    }

    #[test]
    fn top_split_label_biases_to_the_first_row_without_a_badge() {
        // Same bias with no badge: the top label still moves to text row 0 (the
        // bias is about avoiding the bleed, not merely dodging the badge) and,
        // with the row free, centers across the full inner width.
        let panes = vec![
            PaneRect::new(0, 0, 0, 100, 20, "top", false),
            PaneRect::new(1, 0, 20, 100, 20, "bot", true),
        ];
        let out = render(
            &panes,
            &test_palette(),
            14,
            3,
            0,
            LabelMode::All,
            None,
            Close::Off,
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::None,
        );
        let lines = visible_lines(&out);
        assert_eq!(
            lines[0], "▀▀▀▀▀top▀▀▀▀▀▀",
            "top label centers on the first row when no badge contends, got {lines:?}"
        );
        assert!(
            !lines[1].contains("top"),
            "the label must not bleed onto the shared middle row, got {lines:?}"
        );
    }

    #[test]
    fn biased_top_label_falls_back_below_the_badge_when_too_narrow() {
        // When the block is too narrow to host both the badge and the label on
        // the first row, the label drops back to the centered lower row — the
        // documented bleed — rather than fragmenting against the badge. Here the
        // 5-col block leaves no room beside the 2-col badge, so the single-char
        // label lands on text row 1 instead of row 0.
        let panes = vec![
            PaneRect::new(0, 0, 0, 100, 20, "x", false),
            PaneRect::new(1, 0, 20, 100, 20, "bot", true),
        ];
        let out = render(
            &panes,
            &test_palette(),
            5,
            3,
            0,
            LabelMode::All,
            Some("F1"),
            Close::Off,
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::None,
        );
        let lines = visible_lines(&out);
        assert!(
            lines[0].contains('F') && !lines[0].contains('x'),
            "the badge holds the first row; the label cannot fit beside it, got {lines:?}"
        );
        assert!(
            lines[1].contains('x'),
            "the label falls back to the centered lower row, got {lines:?}"
        );
    }

    #[test]
    fn biased_top_label_nudges_right_only_far_enough_to_clear_the_badge() {
        // When the full-width centered position would overlap the badge, the
        // label shifts right by just enough to clear it — not all the way into a
        // re-centered remaining window. In a 10-wide block the 3-char label
        // centers at col 3, which collides with the 2-col badge (occupying cols
        // 1..3); it nudges one cell to col 4 (badge end + one gap), landing at
        // `▀F1▀abc▀▀▀` rather than the further-right `▀F1▀▀abc▀▀`.
        let panes = vec![
            PaneRect::new(0, 0, 0, 100, 20, "abc", false),
            PaneRect::new(1, 0, 20, 100, 20, "bot", true),
        ];
        let out = render(
            &panes,
            &test_palette(),
            10,
            3,
            0,
            LabelMode::All,
            Some("F1"),
            Close::Off,
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::None,
        );
        let lines = visible_lines(&out);
        assert_eq!(
            lines[0], "▀F1▀abc▀▀▀",
            "the label nudges right only to clear the badge, got {lines:?}"
        );
    }

    #[test]
    fn long_titles_truncate_and_never_overflow_the_block() -> Result<(), Box<dyn std::error::Error>>
    {
        // Long titles — ASCII or fullwidth CJK — are summarized to fit and must
        // never push a cell past the block. The guarantee: every rendered text
        // row keeps an exact total display width of `pw`, whatever the title
        // length or per-glyph width (a fullwidth glyph claims a glyph cell plus
        // an empty continuation cell, and a wide glyph that would straddle the
        // right edge is dropped wholesale). This is the bias scenario (a
        // top/bottom split with a badge), where the new centering arithmetic
        // runs.
        let width = 20;
        for title in [
            "a-really-long-server-process-name-that-overflows",
            "サーバープロセスのとても長い名前",
        ] {
            let panes = vec![
                PaneRect::new(0, 0, 0, 100, 20, title, false),
                PaneRect::new(1, 0, 20, 100, 20, "bot", true),
            ];
            let out = render(
                &panes,
                &test_palette(),
                width,
                3,
                0,
                LabelMode::All,
                Some("F1"),
                Close::Off,
                GradientSpec::OFF,
                true,
                crate::floating::FloatLayer::None,
            );
            for line in visible_lines(&out) {
                let w: usize = line.chars().filter_map(UnicodeWidthChar::width).sum();
                assert_eq!(
                    w, width,
                    "row display width must equal the block width for {title:?}, got {w} in {line:?}"
                );
            }
        }
        Ok(())
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
            0,
            LabelMode::All,
            None,
            Close::Off,
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::None,
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
            0,
            LabelMode::All,
            None,
            Close::Off,
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::None,
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
            0,
            LabelMode::All,
            None,
            Close::Off,
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::None,
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
            0,
            LabelMode::None,
            Some("⌘ 1"),
            Close::Off,
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::None,
        );
        assert!(wide.contains('⌘'), "wide block should host the badge");
        let narrow = render(
            &panes,
            &test_palette(),
            1,
            3,
            0,
            LabelMode::None,
            Some("⌘ 1"),
            Close::Off,
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::None,
        );
        assert!(
            !narrow.contains('⌘'),
            "too-narrow block must drop the badge"
        );
    }

    #[test]
    fn active_badge_text_is_white_and_inactive_is_muted() {
        // #59: active badge = pure white over the pane fill — vivid fill + white
        // gives maximum contrast. Inactive badge = white blended toward the fill
        // by INACTIVE_LABEL_BLEND so it recedes. No background chip anywhere.
        // LabelMode::None keeps labels out, so the badge is the only text source.
        let panes = one_focused();
        let palette = test_palette();
        let render_badge = |active: bool| {
            render(
                &panes,
                &palette,
                10,
                3,
                0,
                LabelMode::None,
                Some("⌘ 1"),
                Close::Off,
                GradientSpec::OFF,
                active,
                crate::floating::FloatLayer::None,
            )
        };
        let active = render_badge(true);
        let inactive = render_badge(false);
        assert!(
            active.contains(&fg(ACTIVE_FG)),
            "the active block's badge text must be white — stands out on vivid fill (#59)"
        );
        assert!(
            !active.contains(&bg(palette.accent())),
            "the badge must not paint an accent background chip"
        );
        // Inactive badge is blended toward the pane fill. Since active=false
        // suppresses the ring, the badge background is the plain pane fill.
        let fill = palette.color_for(0); // one_focused() → id=0
        let muted = crate::color::mixed(ACTIVE_FG, fill, INACTIVE_LABEL_BLEND);
        assert!(
            inactive.contains(&fg(muted)),
            "an inactive badge must be muted toward the pane fill (#59)"
        );
        assert!(
            !inactive.contains(&fg(ACTIVE_FG)),
            "an inactive badge must not be pure white — it should be subdued"
        );
    }

    #[test]
    fn focus_highlight_paints_only_the_active_tab() {
        // #59: the focused pane in the *active* tab renders white bold text —
        // the single most prominent label on the bar. Inactive tabs suppress
        // the highlight entirely (no ring, no bold) and mute text toward the
        // pane fill, so only the active tab's cursor location stands out.
        let panes = one_focused();
        let palette = test_palette();
        let render_tab = |active: bool| {
            render(
                &panes,
                &palette,
                12,
                3,
                0,
                LabelMode::Focused,
                None,
                Close::Off,
                GradientSpec::OFF,
                active,
                crate::floating::FloatLayer::None,
            )
        };
        let active = render_tab(true);
        let inactive = render_tab(false);
        assert!(
            active.contains(&fg(ACTIVE_FG)),
            "the active tab's focused label must be white — stands out on vivid fill (#59)"
        );
        assert!(
            active.contains("\x1b[1m"),
            "the active tab's focused label must be bold"
        );
        assert!(
            active.contains(&fg(palette.ring_for(0))),
            "the active tab keeps the focus ring"
        );
        assert!(
            inactive.contains('n'),
            "the inactive tab still labels its focused pane"
        );
        // Inactive label is blended toward the pane fill (ring suppressed).
        let fill = palette.color_for(0); // one_focused() → id=0
        let muted = crate::color::mixed(ACTIVE_FG, fill, INACTIVE_LABEL_BLEND);
        assert!(
            inactive.contains(&fg(muted)),
            "the inactive focused label must be muted toward the pane fill (#59)"
        );
        assert!(
            !inactive.contains(&fg(ACTIVE_FG)),
            "inactive label must not be pure white — it should be subdued"
        );
        assert!(!inactive.contains("\x1b[1m"), "no bold on an inactive tab");
        assert!(
            !inactive.contains(&fg(palette.ring_for(0))),
            "no focus ring on an inactive tab"
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
            0,
            LabelMode::None,
            Some("符1"),
            Close::Off,
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::None,
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
            0,
            LabelMode::None,
            Some("符符"),
            Close::Off,
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::None,
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
            0,
            LabelMode::None,
            None,
            Close::Off,
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::None,
        );
        let stop = fg(crate::color::gradient_at(palette.color_for(1), 100));
        assert!(out.contains(&fg(palette.color_for(1))));
        assert!(!out.contains(&stop), "off must not paint any swept shade");
    }

    #[test]
    fn sheen_sweeps_from_base_fill_to_stop() {
        let palette = test_palette();
        // 21 columns → a 20 px axis, long enough that the per-pixel cap doesn't
        // bind and the rightmost column reaches the full stop.
        let out = render(
            &one_plain(),
            &palette,
            21,
            1,
            0,
            LabelMode::None,
            None,
            Close::Off,
            GradientSpec::SHEEN,
            true,
            crate::floating::FloatLayer::None,
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
        // stop) — so the very first cell must pair base fg with stop bg. The 21
        // columns give a 20 px axis so the bottom row reaches the full stop.
        let palette = test_palette();
        let out = render(
            &one_plain(),
            &palette,
            21,
            1,
            0,
            LabelMode::None,
            None,
            Close::Off,
            GradientSpec::WEAVE,
            true,
            crate::floating::FloatLayer::None,
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
            0,
            LabelMode::None,
            None,
            Close::Off,
            GradientSpec::SHEEN,
            true,
            crate::floating::FloatLayer::None,
        );
        assert!(out.starts_with(&fg(palette.color_for(1))));
    }

    // ---- gradient direction (#71) ---------------------------------------

    /// A linear sheen at `angle` degrees, default radial direction.
    fn lin(angle: u16) -> GradientSpec {
        GradientSpec {
            mode: GradientMode::Sheen,
            shape: GradientShape::Linear,
            angle,
            radial: RadialDirection::Outward,
        }
    }

    /// A radial sheen in `radial` direction (angle is irrelevant to radial).
    fn radial(radial: RadialDirection) -> GradientSpec {
        GradientSpec {
            mode: GradientMode::Sheen,
            shape: GradientShape::Radial,
            angle: 0,
            radial,
        }
    }

    #[test]
    fn gradient_t_linear_angle_0_runs_left_to_right() {
        // bounds: (px0, px1, py0, py1) — a long 41 px axis so the per-pixel cap
        // doesn't bind and the sweep spans the full base→stop range.
        let bounds = (0, 41, 0, 4);
        assert_eq!(gradient_t(lin(0), bounds, 0.0, 0.0), Some(0));
        assert_eq!(gradient_t(lin(0), bounds, 40.0, 0.0), Some(100));
        // y does not affect a horizontal sweep.
        assert_eq!(
            gradient_t(lin(0), bounds, 0.0, 3.0),
            Some(0),
            "angle 0 ignores y"
        );
    }

    #[test]
    fn gradient_t_linear_angle_90_runs_top_to_bottom() {
        // A tall 41 px axis so the cap doesn't bind (see angle-0 above).
        let bounds = (0, 10, 0, 41);
        assert_eq!(gradient_t(lin(90), bounds, 0.0, 0.0), Some(0));
        assert_eq!(gradient_t(lin(90), bounds, 0.0, 40.0), Some(100));
        // x does not affect a vertical sweep.
        assert_eq!(
            gradient_t(lin(90), bounds, 9.0, 0.0),
            Some(0),
            "angle 90 ignores x"
        );
    }

    #[test]
    fn gradient_t_linear_angle_180_reverses_angle_0() {
        // 180° is the reverse of 0°: the left edge now holds the stop, the right
        // the base fill. (`round`-to-u8 absorbs the tiny f32 fuzz in cos/sin π.)
        let bounds = (0, 41, 0, 4);
        assert_eq!(gradient_t(lin(180), bounds, 0.0, 0.0), Some(100));
        assert_eq!(gradient_t(lin(180), bounds, 40.0, 0.0), Some(0));
    }

    #[test]
    fn gradient_t_caps_the_per_pixel_step_on_a_short_axis() {
        // A 3-px-tall pane spans only 2 pixels vertically, so an even 0→100 sweep
        // would jump 50 per pixel. The cap holds each step to MAX_GRADIENT_STEP,
        // so the far edge eases only part-way to the stop (steps × the cap).
        let bounds = (0, 10, 0, 3); // span_y = 2
        let edge = (MAX_GRADIENT_STEP * 2.0).round() as u8;
        assert!(edge < 100, "a short axis must stop short of the full stop");
        assert_eq!(gradient_t(lin(90), bounds, 0.0, 0.0), Some(0));
        assert_eq!(gradient_t(lin(90), bounds, 0.0, 2.0), Some(edge));
        // The midpoint pixel sits one capped step in, not a coarse half-jump.
        assert_eq!(
            gradient_t(lin(90), bounds, 0.0, 1.0),
            Some((MAX_GRADIENT_STEP).round() as u8)
        );
    }

    #[test]
    fn gradient_t_radial_runs_from_center_to_edge() {
        // 41×41 block → center pixel at (20, 20); the radius to a corner is long
        // enough that the cap doesn't bind, so the edge reaches the full stop.
        let bounds = (0, 41, 0, 41);
        let (out, inn) = (
            radial(RadialDirection::Outward),
            radial(RadialDirection::Inward),
        );
        // Center: outward = base fill (0), inward = stop (100).
        assert_eq!(gradient_t(out, bounds, 20.0, 20.0), Some(0));
        assert_eq!(gradient_t(inn, bounds, 20.0, 20.0), Some(100));
        // Corner: outward = stop (100), inward = base fill (0) — the mirror.
        assert_eq!(gradient_t(out, bounds, 0.0, 0.0), Some(100));
        assert_eq!(gradient_t(inn, bounds, 0.0, 0.0), Some(0));
    }

    #[test]
    fn gradient_t_degenerate_axis_returns_none() {
        // A 1-px-wide block has no horizontal span (angle 0 → None) but can still
        // sweep vertically (angle 90 → a real value) — the #71 generalization of
        // the old column-only degeneracy check.
        assert_eq!(gradient_t(lin(0), (3, 4, 0, 8), 3.0, 0.0), None);
        assert_eq!(gradient_t(lin(90), (3, 4, 0, 8), 3.0, 0.0), Some(0));
        // A single-pixel block has no radius either.
        assert_eq!(
            gradient_t(radial(RadialDirection::Outward), (3, 4, 0, 1), 3.0, 0.0),
            None
        );
    }

    #[test]
    fn label_samples_the_text_row_center_between_its_two_pixels()
    -> Result<(), Box<dyn std::error::Error>> {
        // A vertical sweep's label takes one background sample at the text row's
        // vertical center (2*tr + 0.5), strictly between the row's top (2*tr) and
        // bottom (2*tr + 1) pixels — so the label tracks the sweep through the
        // text rather than freezing at the top pixel.
        let bounds = (0, 4, 0, 8);
        let tr = 2usize;
        let top = gradient_t(lin(90), bounds, 0.0, (2 * tr) as f32).ok_or("top sample")?;
        let center =
            gradient_t(lin(90), bounds, 0.0, 2.0 * tr as f32 + 0.5).ok_or("center sample")?;
        let bottom =
            gradient_t(lin(90), bounds, 0.0, (2 * tr + 1) as f32).ok_or("bottom sample")?;
        assert!(
            top < center && center < bottom,
            "center sample lies between the row's pixels: {top} < {center} < {bottom}"
        );
        Ok(())
    }

    #[test]
    fn render_vertical_sweep_is_uniform_across_each_row() {
        // End-to-end wiring: angle 90 produces no horizontal variation — every
        // column of a row shares the same fg/bg pair — while the top row begins
        // at the base fill and the bottom row reaches the capped sweep edge (a
        // 4-px-tall block is too short to hit the full stop under the cap).
        let palette = test_palette();
        let out = render(
            &one_plain(),
            &palette,
            4,
            2,
            0,
            LabelMode::None,
            None,
            Close::Off,
            lin(90),
            true,
            crate::floating::FloatLayer::None,
        );
        let fill = palette.color_for(1);
        // size 2 → 4 px tall → an inclusive span of 3 pixels.
        let edge =
            crate::color::gradient_at(fill, (MAX_GRADIENT_STEP * 3.0).min(100.0).round() as u8);
        let lines: Vec<&str> = out.lines().collect();
        assert!(
            lines[0].starts_with(&fg(fill)),
            "top row starts at the base fill, got {:?}",
            lines[0]
        );
        assert!(
            lines[1].contains(&bg(edge)),
            "bottom row reaches the capped sweep edge, got {:?}",
            lines[1]
        );
        // Uniform across columns: the base fg appears once per column on row 0.
        assert_eq!(
            lines[0].matches(&fg(fill)).count(),
            4,
            "row 0 must be uniform across its 4 columns"
        );
    }

    #[test]
    fn render_radial_outward_paints_the_corner_with_the_stop() {
        // The top-left pixel is a farthest corner: radial-outward paints it at the
        // stop shade, distinguishing it from a linear angle-0 sweep (whose
        // top-left corner is the base fill). 16×16 px keeps the corner radius long
        // enough that the cap doesn't bind, so the corner reaches the full stop.
        let palette = test_palette();
        let out = render(
            &one_plain(),
            &palette,
            16,
            8,
            0,
            LabelMode::None,
            None,
            Close::Off,
            radial(RadialDirection::Outward),
            true,
            crate::floating::FloatLayer::None,
        );
        let stop = fg(crate::color::gradient_at(palette.color_for(1), 100));
        assert!(
            out.starts_with(&stop),
            "radial outward paints the farthest corner at the stop, got {:?}",
            out.lines().next()
        );
    }

    #[test]
    fn close_stamps_the_top_right_only_when_enabled() {
        // `close: true` puts an "×" on the top text row; `close: false` leaves the
        // block exactly as it was (the v0.x look) — the opt-in gate (#86).
        let with = render(
            &one_plain(),
            &test_palette(),
            12,
            3,
            0,
            LabelMode::None,
            None,
            Close::NerdFont(TEST_CLOSE_FG),
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::None,
        );
        let without = render(
            &one_plain(),
            &test_palette(),
            12,
            3,
            0,
            LabelMode::None,
            None,
            Close::Off,
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::None,
        );
        let with_top = with.lines().next().unwrap_or_default();
        assert!(
            with_top.contains(CLOSE_GLYPH),
            "close=true stamps × on the top row: {with_top:?}"
        );
        assert!(
            !without.contains(CLOSE_GLYPH),
            "close=false stamps no ×: {without:?}"
        );
    }

    #[test]
    fn close_appears_once_on_the_top_row() {
        // The glyph is a single corner cell: exactly one × in the whole block, and
        // it is on the first line, never a lower row.
        let out = render(
            &one_plain(),
            &test_palette(),
            12,
            3,
            0,
            LabelMode::None,
            None,
            Close::NerdFont(TEST_CLOSE_FG),
            GradientSpec::OFF,
            false,
            crate::floating::FloatLayer::None,
        );
        assert_eq!(
            out.matches(CLOSE_GLYPH).count(),
            1,
            "exactly one ×: {out:?}"
        );
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[0].contains(CLOSE_GLYPH), "× rides the top row");
        let lower = &lines[1..];
        assert!(
            lower.iter().all(|l| !l.contains(CLOSE_GLYPH)),
            "no × on any lower row: {lower:?}"
        );
    }

    #[test]
    fn close_glyph_renders_in_the_carried_foreground() {
        // `Close` carries an already-resolved foreground (#94 follow-up); the
        // renderer stamps it at full strength on the active tab and tones it
        // toward the fill (never the raw color, never the white badge shade) on
        // an inactive one. A distinctive value pins that the carried color is
        // what reaches the screen.
        let carried = (222, 11, 99);
        let palette = test_palette();
        let active = render(
            &one_plain(),
            &palette,
            12,
            3,
            0,
            LabelMode::None,
            None,
            Close::NerdFont(carried),
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::None,
        );
        let active_top = active.lines().next().unwrap_or_default();
        assert!(
            active_top.contains(CLOSE_GLYPH),
            "close glyph rides the top row: {active_top:?}"
        );
        assert!(
            active_top.contains(&fg(carried)),
            "active close glyph is painted in the full carried color: {active_top:?}"
        );
        assert!(
            !active_top.contains(&fg(ACTIVE_FG)),
            "active close glyph no longer borrows the white badge shade: {active_top:?}"
        );

        let inactive = render(
            &one_plain(),
            &palette,
            12,
            3,
            0,
            LabelMode::None,
            None,
            Close::NerdFont(carried),
            GradientSpec::OFF,
            false,
            crate::floating::FloatLayer::None,
        );
        let inactive_top = inactive.lines().next().unwrap_or_default();
        assert!(
            inactive_top.contains(CLOSE_GLYPH),
            "inactive close glyph still rides the top row: {inactive_top:?}"
        );
        assert!(
            !inactive_top.contains(&fg(carried)),
            "inactive close glyph is toned toward the fill, not the raw color: {inactive_top:?}"
        );
        assert!(
            !inactive_top.contains(&fg(ACTIVE_FG)),
            "inactive close glyph is not the white badge shade either: {inactive_top:?}"
        );
    }

    #[test]
    fn close_clips_a_top_row_label_off_its_own_cell() {
        // With close on, a label that lands on the top row is bounded one column
        // short of the "×" cell so the two never overprint — the row-0 mirror of
        // the badge's left-edge nudge (#86). A top/bottom split puts the top
        // pane's label on row 0 (it biases up off the shared middle row), so the
        // close glyph and that label share the first line; both survive.
        let panes = vec![
            PaneRect::new(0, 0, 0, 100, 20, "top", false),
            PaneRect::new(1, 0, 20, 100, 20, "bot", true),
        ];
        let out = render(
            &panes,
            &test_palette(),
            14,
            3,
            0,
            LabelMode::All,
            None,
            Close::NerdFont(TEST_CLOSE_FG),
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::None,
        );
        let lines = visible_lines(&out);
        assert!(
            lines[0].contains(CLOSE_GLYPH),
            "the close × rides the top row beside the label: {lines:?}"
        );
        assert!(
            lines[0].contains("top"),
            "the row-0 label still renders next to the close cell: {lines:?}"
        );
    }

    #[test]
    fn close_coexists_with_the_badge() {
        // The badge (top-left) and the close × (top-right) both fit on a wide
        // block — the close cell is reserved from the badge, so neither is dropped.
        let out = render(
            &one_plain(),
            &test_palette(),
            12,
            3,
            0,
            LabelMode::None,
            Some("⌘ 1"),
            Close::NerdFont(TEST_CLOSE_FG),
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::None,
        );
        let top = out.lines().next().unwrap_or_default();
        assert!(top.contains('⌘'), "badge survives alongside close: {top:?}");
        assert!(
            top.contains(CLOSE_GLYPH),
            "close survives alongside badge: {top:?}"
        );
    }

    #[test]
    fn nerd_font_close_sits_one_cell_in_from_the_right_edge() {
        // The Nerd Font glyph sits one cell in from the right edge (`pw - 2`,
        // #94), leaving a fill cell at the corner. The ASCII "×" shares that
        // column (see `ascii_close_sits_one_cell_in_from_the_right_edge`).
        let w = 12;
        let out = render(
            &one_plain(),
            &test_palette(),
            w,
            3,
            0,
            LabelMode::None,
            None,
            Close::NerdFont(TEST_CLOSE_FG),
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::None,
        );
        let top: Vec<char> = visible_lines(&out)[0].chars().collect();
        assert_eq!(top.len(), w, "one visible char per cell: {top:?}");
        assert_eq!(
            top[w - 2],
            CLOSE_GLYPH,
            "the Nerd Font close glyph sits one cell in from the right edge: {top:?}"
        );
    }

    #[test]
    fn ascii_close_sits_one_cell_in_from_the_right_edge() {
        // Under simplified UI (no Nerd Font), the close mark is a plain ASCII
        // "×" painted black (#94), seated one cell in from the right edge
        // (`pw - 2`) — the same column as the Nerd Font glyph.
        let w = 12;
        let out = render(
            &one_plain(),
            &test_palette(),
            w,
            3,
            0,
            LabelMode::None,
            None,
            Close::Ascii(CLOSE_FG_ASCII),
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::None,
        );
        let top_line = out.lines().next().unwrap_or_default();
        assert!(
            top_line.contains(CLOSE_GLYPH_ASCII),
            "ASCII close uses the plain × sign: {top_line:?}"
        );
        assert!(
            !top_line.contains(CLOSE_GLYPH),
            "ASCII close never emits the Nerd Font glyph: {top_line:?}"
        );
        assert!(
            top_line.contains(&fg(CLOSE_FG_ASCII)),
            "ASCII close is painted black: {top_line:?}"
        );
        let top: Vec<char> = visible_lines(&out)[0].chars().collect();
        assert_eq!(top.len(), w, "one visible char per cell: {top:?}");
        assert_eq!(
            top[w - 2],
            CLOSE_GLYPH_ASCII,
            "the ASCII close sits one cell in from the right edge: {top:?}"
        );
    }
}

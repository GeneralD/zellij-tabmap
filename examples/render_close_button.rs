//! Visual sample for #86 — the close button on each tab block (on by default
//! since #94).
//!
//! When `close_button` is enabled, a tab block stamps a small close glyph
//! **near its top-right corner** — balancing the top-left `⌘N` shortcut badge.
//! A left-click on exactly that glyph cell closes the tab. The glyph form and
//! color are
//! **per terminal** (#94), but both modes seat the mark one cell in from the
//! right edge (`pw - 2`), leaving a fill cell of breathing room at the corner;
//! they differ only in glyph and color:
//!
//! - **Nerd Font** (default): the `md-close_circle` glyph in zellij's alert red
//!   (the theme's `exit_code_error.base`).
//! - **ASCII** (zellij's simplified UI — no Nerd Font): a plain `×` painted black.
//!
//! In both modes the glyph is full strength on the active tab and toned toward
//! the fill where an inactive tab still carries it, so it reads as a quiet
//! "close here" affordance rather than competing with the minimap. It lands on
//! the **active tab** — and, when the perspective depth cue is off, on **every**
//! tab — but not on the perspective-receded inactive tabs, whose inset corner
//! would carry it unbalanced. This sample runs with perspective **on**, so only
//! the active (lifted) tab shows the glyph; the receded inactive tabs
//! deliberately show none. It never appears on the last remaining tab either.
//!
//! This drives the **real** render path — `paint::bar` forwards the `Close` mode
//! through `tab_block::assemble` into `minimap::render`, the same code the plugin
//! runs — so the preview can never drift from what ships.
//!
//! Not part of the plugin — run with e.g.
//! `cargo run --example render_close_button --target aarch64-apple-darwin`
//! (substitute your host triple to override the wasm32-wasip1 default).
//! Pass `ascii` as the first argument to preview the simplified-UI `×` fallback
//! instead of the Nerd Font glyph.

use std::collections::BTreeMap;

use zellij_tabmap::color::Palette;
use zellij_tabmap::line::{self, Alignment};
use zellij_tabmap::minimap::{Close, GradientMode, GradientSpec, PaneRect};
use zellij_tabmap::paint;

const ROWS: usize = 4;
const GAP: usize = 2;

fn main() {
    // Tokyonight-ish slots + the frame-highlight orange as accent (as in the
    // other render examples), so the sample matches the real palette.
    let palette = Palette::new(
        vec![
            (122, 162, 247), // blue
            (158, 206, 106), // green
            (255, 158, 100), // orange
            (187, 154, 247), // magenta
            (125, 207, 255), // cyan
            (247, 118, 142), // red
        ],
        (255, 158, 100),
    )
    // Tokyonight's red as the alert/close-glyph color, standing in for the
    // theme's `exit_code_error.base` the plugin reads at runtime (#86).
    .with_alert((247, 118, 142));
    let mut panes = BTreeMap::new();
    // ⌘1 — a two-column split.
    panes.insert(
        0,
        vec![
            PaneRect::new(0, 0, 0, 60, 40, "nvim", true),
            PaneRect::new(1, 60, 0, 60, 40, "zsh", false),
        ],
    );
    // ⌘2 (active) — a 2×2-ish grid.
    panes.insert(
        1,
        vec![
            PaneRect::new(2, 0, 0, 60, 20, "cargo", true),
            PaneRect::new(3, 60, 0, 60, 20, "test", false),
            PaneRect::new(4, 0, 20, 120, 20, "git", false),
        ],
    );
    // ⌘3 — a single editor pane.
    panes.insert(2, vec![PaneRect::new(5, 0, 0, 120, 40, "docs", true)]);
    // ⌘4 — a two-row split.
    panes.insert(
        3,
        vec![
            PaneRect::new(6, 0, 0, 120, 20, "server", true),
            PaneRect::new(7, 0, 20, 120, 20, "logs", false),
        ],
    );

    // `ascii` previews the simplified-UI `×` fallback; the default is the Nerd
    // Font glyph the plugin uses on a fancy terminal. Both variants carry the
    // per-terminal glyph color the plugin resolves at runtime (#94): the ASCII
    // `×` is painted black, the Nerd Font glyph the theme's alert red (here the
    // palette's `with_alert` red).
    let close = match std::env::args().nth(1).as_deref() {
        Some("ascii") => Close::Ascii((0, 0, 0)),
        _ => Close::NerdFont(palette.alert()),
    };

    // Four tabs, none of them the lone survivor. Perspective is on (below), so
    // only the active tab (position 1) draws its close glyph; the receded
    // inactive tabs show none. A plain `pack` (no "+" button) keeps the focus on
    // the affordance.
    let layout = line::pack(160, 0, line::ACTIVE_MAX, 4, 1, Alignment::Left, GAP);
    let bar = paint::bar(
        ROWS,
        &layout,
        &panes,
        &palette,
        "\u{2318} ",
        GradientSpec::from_mode(GradientMode::Sheen),
        true,  // inactive_dim
        true,  // perspective on — so only the active tab shows the close glyph
        close, // Nerd Font glyph (default) or ASCII `×` (arg = "ascii")
        &BTreeMap::new(),
        &BTreeMap::new(), // suppressed-pane covers — none in this sample
    );

    // Hide the cursor so a held screenshot doesn't catch a stray cursor block
    // past the strip. Move the cursor below the rendered bar and restore it so the
    // demo doesn't leave the terminal in a hidden-cursor state.
    print!("{bar}\u{1b}[?25l\u{1b}[{n};1H\u{1b}[?25h", n = ROWS + 2);
}

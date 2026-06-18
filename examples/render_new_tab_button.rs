//! Visual sample for #76 — the inline "+" new-tab button.
//!
//! The button renders like a normal tab: a muted, theme-aware *fill* (not
//! line-art) with a centered "+", and it sits at the end of the tab strip,
//! right after the last visible tab — exactly where the next tab would go.
//! The fill is `CANVAS` lifted a little toward its luma-opposite (lighter on a
//! dark bar, darker on a light one), so it reads as a quiet affordance rather
//! than a competing tab.
//!
//! This drives the **real** render path — `line::pack_with_button` reserves and
//! places the span, and `paint::bar` draws the button from the same
//! `color::button_fill`/`button_glyph` the plugin uses — so the preview can
//! never drift from what ships.
//!
//! Not part of the plugin — run with e.g.
//! `cargo run --example render_new_tab_button --target aarch64-apple-darwin`
//! (substitute your host triple to override the wasm32-wasip1 default).

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
    );
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

    // The real reserve-and-place path: `pack_with_button` keeps the strip clear
    // of the button's span and records where the "+" goes; `paint::bar` then
    // paints it one gap past the last visible tab. A wider-than-needed bar
    // (160 cols, left-aligned) leaves the "+" sitting right after the last tab
    // with empty canvas beyond — the "just another slot at the end" intent.
    let layout = line::pack_with_button(160, 0, line::ACTIVE_MAX, 4, 1, Alignment::Left, GAP, true);
    let bar = paint::bar(
        ROWS,
        &layout,
        &panes,
        &palette,
        "\u{2318} ",
        GradientSpec::from_mode(GradientMode::Sheen),
        true,
        true,
        Close::Off,
    );

    // Hide the cursor so a held screenshot doesn't catch a stray cursor block
    // parked past the button. Move the cursor below the rendered bar and restore it
    // so the demo doesn't leave the terminal in a hidden-cursor state.
    print!("{bar}\u{1b}[?25l\u{1b}[{n};1H\u{1b}[?25h", n = ROWS + 2);
}

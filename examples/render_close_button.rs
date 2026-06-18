//! Visual sample for #86 — the opt-in close button on each tab block.
//!
//! When `close_button` is enabled, a tab block stamps a small close glyph (the
//! Nerd Font `md-close_circle`) into its **top-right corner** — balancing the
//! top-left `⌘N` shortcut badge. A left-click on exactly that cell closes the
//! tab; the glyph is zellij's alert red (the theme's `exit_code_error.base`),
//! full red on the active tab and toned toward the fill where an inactive tab
//! still carries it, so it reads as a quiet "close here" affordance rather than
//! competing with the minimap. It lands on the **active tab** — and, when the
//! perspective depth cue is off, on **every** tab — but not on the
//! perspective-receded inactive tabs, whose inset corner would carry it
//! unbalanced. This sample runs with perspective **on**, so only the active
//! (lifted) tab shows the glyph; the receded inactive tabs deliberately show
//! none. It never appears on the last remaining tab either.
//!
//! This sample drives `paint::bar` directly, so it shows the Nerd Font glyph;
//! the plugin downgrades it to a plain `×` on terminals running zellij's
//! simplified UI (see `State::render`).
//!
//! This drives the **real** render path — `paint::bar` forwards `close: true`
//! through `tab_block::assemble` into `minimap::render`, the same code the plugin
//! runs — so the preview can never drift from what ships.
//!
//! Not part of the plugin — run with e.g.
//! `cargo run --example render_close_button --target aarch64-apple-darwin`
//! (substitute your host triple to override the wasm32-wasip1 default).

use std::collections::BTreeMap;

use zellij_tabmap::color::Palette;
use zellij_tabmap::line::{self, Alignment};
use zellij_tabmap::minimap::{GradientMode, GradientSpec, PaneRect};
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
        true, // inactive_dim
        true, // perspective on — so only the active tab shows the close glyph
        true, // close_button enabled
    );

    // Hide the cursor so a held screenshot doesn't catch a stray cursor block
    // past the strip. Move the cursor below the rendered bar and restore it so the
    // demo doesn't leave the terminal in a hidden-cursor state.
    print!("{bar}\u{1b}[?25l\u{1b}[{n};1H\u{1b}[?25h", n = ROWS + 2);
}

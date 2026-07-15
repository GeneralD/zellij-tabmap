//! Visual sample for #110 — floating panes on the tab bar.
//!
//! With `floating "hybrid"` (the default), a tab depicts its floating panes
//! according to that tab's own floating layer:
//!
//! - **Visible layer** → each float is overlaid **graphically** on top of the
//!   tiled minimap, mapped through the *same* bounding box as the tiles so it
//!   sits in place without shifting them. A thin border in the float's shade
//!   sets it apart from the tiles beneath.
//! - **Hidden layer** → each float shrinks to a small selectable **chip** (`◲`)
//!   docked in the block's **bottom-right corner**, colored by that float's id;
//!   a `⋯` marker stands in for any that overflow the width. A left-click on a
//!   chip reveals and focuses that hidden float in one step.
//!
//! This sample lays out three tabs so the states sit side by side: a plain tab
//! (no floats), the **active** tab carrying a visible-float overlay, and a tab
//! whose hidden layer shows two corner chips.
//!
//! This drives the **real** render path — `paint::bar` forwards the per-tab
//! `FloatSpec` through `tab_block::assemble` into `minimap::render`, the same
//! code the plugin runs — so the preview can never drift from what ships.
//!
//! Not part of the plugin — run with e.g.
//! `cargo run --example render_floating --target aarch64-apple-darwin`
//! (substitute your host triple to override the wasm32-wasip1 default).

use std::collections::BTreeMap;

use zellij_tabmap::color::Palette;
use zellij_tabmap::floating::FloatSpec;
use zellij_tabmap::line::{self, Alignment};
use zellij_tabmap::minimap::{Close, GradientMode, GradientSpec, PaneRect};
use zellij_tabmap::paint;

const ROWS: usize = 4;
const GAP: usize = 2;

fn main() {
    // Tokyonight-ish slots + the frame-highlight orange as accent, matching the
    // other render examples so the sample tracks the real palette.
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

    // Each tab's *tiled* panes, keyed by tab position — the base minimap the
    // floating layer draws over.
    let mut panes = BTreeMap::new();
    // ⌘1 — a plain two-column split, no floats (the `floating "off"` look).
    panes.insert(
        0,
        vec![
            PaneRect::new(0, 0, 0, 60, 40, "nvim", true),
            PaneRect::new(1, 60, 0, 60, 40, "zsh", false),
        ],
    );
    // ⌘2 (active) — a 2×2-ish grid, with two visible floats overlaid (below), one
    // focused. No tiled pane is focused here: focus is on a float, and zellij
    // focuses one pane at a time, so the tiled panes carry no focus ring — only
    // the focused float carries the strong ring.
    panes.insert(
        1,
        vec![
            PaneRect::new(2, 0, 0, 60, 20, "cargo", false),
            PaneRect::new(3, 60, 0, 60, 20, "test", false),
            PaneRect::new(4, 0, 20, 120, 20, "git", false),
        ],
    );
    // ⌘3 — a single editor pane, with a hidden float layer (two chips, below).
    panes.insert(2, vec![PaneRect::new(5, 0, 0, 120, 40, "docs", true)]);

    // The per-tab floating layer, keyed the same way. Tab 1's layer is VISIBLE
    // with TWO floats overlaid through the tiles' own bbox: `htop` is focused, so
    // it keeps the full boundary ring, while `logs` is unfocused, so its ring
    // recedes toward its fill — the focused float stands out among its siblings
    // (#116). Tab 2's layer is HIDDEN, so its two floats become corner chips (ids
    // 101, 102 key their color). Tab 0 has none.
    let mut floats = BTreeMap::new();
    floats.insert(
        1,
        FloatSpec::Visible(vec![
            PaneRect::new(100, 8, 8, 44, 22, "htop", true), // focused → full ring
            PaneRect::new(103, 68, 8, 44, 22, "logs", false), // unfocused → weakened ring
        ]),
    );
    floats.insert(2, FloatSpec::Hidden(vec![101, 102]));

    // The active tab (⌘2) also hides a suppressed pane behind its full-width
    // `git` pane (id 4) — e.g. an edit-scrollback editor took its slot. Only the
    // active tab shows the awareness marker (#118), so this map carries the single
    // active-position entry; a `◳` lands in that cover pane's bottom-right corner.
    let mut suppressed_covers = BTreeMap::new();
    suppressed_covers.insert(1usize, vec![4usize]);

    // Three tabs, active in the middle. Perspective is off so every tab renders
    // at full height and the overlay / chips read clearly on each block; a plain
    // `pack` (no "+" button) keeps the focus on the floating affordances.
    let layout = line::pack(160, 0, line::ACTIVE_MAX, 3, 1, Alignment::Left, GAP);
    let bar = paint::bar(
        ROWS,
        &layout,
        &panes,
        &palette,
        "\u{2318} ",
        GradientSpec::from_mode(GradientMode::Sheen),
        true,       // inactive_dim
        false,      // perspective off — full height, so overlays/chips are legible
        Close::Off, // keep the focus on the floating panes
        &floats,
        &suppressed_covers,
    );

    // Hide the cursor so a held screenshot doesn't catch a stray cursor block
    // past the strip, move it below the rendered bar, then restore it so the demo
    // doesn't leave the terminal in a hidden-cursor state.
    print!("{bar}\u{1b}[?25l\u{1b}[{n};1H\u{1b}[?25h", n = ROWS + 2);
}

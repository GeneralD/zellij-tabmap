//! Visual sample for #119 — pinned floating panes on the tab bar.
//!
//! A float pinned with zellij's `toggle-pane-pinned` stays on the real screen
//! even while the floating layer is hidden, so the bar depicts it accordingly:
//!
//! - **Pin marker** → a pinned float's overlay carries a `⌖` in its top-right
//!   corner cell, in the float's full-strength ring color regardless of focus
//!   (the corner-marker vocabulary shared with #118's `◳`), so pinned and
//!   unpinned floats read apart at a glance.
//! - **Hidden layer, mixed** → hiding the layer folds only the *unpinned*
//!   floats into corner chips (`◲`); a pinned float keeps its overlay box,
//!   matching what zellij actually leaves on screen.
//!
//! This sample lays out three tabs so the states sit side by side: a plain tab
//! (no floats), the **active** tab whose visible layer holds a pinned `htop`
//! (marked) next to an unpinned `logs` (unmarked), and a tab whose hidden
//! layer keeps its pinned `note` overlaid while two unpinned floats chip.
//!
//! This drives the **real** render path — `paint::bar` forwards the per-tab
//! `FloatSpec` and pinned ids through `tab_block::assemble` into
//! `minimap::render`, the same code the plugin runs — so the preview can never
//! drift from what ships.
//!
//! Not part of the plugin — run with e.g.
//! `cargo run --example render_pinned --target aarch64-apple-darwin`
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
    // ⌘1 — a plain two-column split, no floats, for contrast.
    panes.insert(
        0,
        vec![
            PaneRect::new(0, 0, 0, 60, 40, "nvim", true),
            PaneRect::new(1, 60, 0, 60, 40, "zsh", false),
        ],
    );
    // ⌘2 (active) — a 2×2-ish grid under a visible float layer (below). Focus
    // is on the pinned float, so no tiled pane carries the strong ring.
    panes.insert(
        1,
        vec![
            PaneRect::new(2, 0, 0, 60, 20, "cargo", false),
            PaneRect::new(3, 60, 0, 60, 20, "test", false),
            PaneRect::new(4, 0, 20, 120, 20, "git", false),
        ],
    );
    // ⌘3 — a single editor pane whose hidden float layer is mixed (below).
    panes.insert(2, vec![PaneRect::new(5, 0, 0, 120, 40, "docs", true)]);

    // The per-tab floating layer. Tab 1's layer is VISIBLE with two floats:
    // `htop` is pinned, so a `⌖` lands in its top-right corner; `logs` is not,
    // so it renders exactly as in #110 — the side-by-side pinned/unpinned
    // contrast. `htop` also clears the label size gate (#120), showing the
    // marker and the centered title coexisting on one overlay.
    //
    // Tab 2's layer is HIDDEN and MIXED: its pinned `note` (id 104) is still on
    // the real screen, so it keeps its overlay box (marked, and dimmed with the
    // rest of the inactive tab), while the two unpinned floats (ids 101, 102)
    // fold into corner chips as before.
    let mut floats = BTreeMap::new();
    floats.insert(
        1,
        FloatSpec::Visible(vec![
            PaneRect::new(100, 8, 4, 44, 30, "htop", true), // pinned → ⌖ top-right
            PaneRect::new(103, 68, 8, 44, 22, "logs", false), // unpinned → no marker
        ]),
    );
    floats.insert(
        2,
        FloatSpec::Mixed {
            chips: vec![101, 102],
            overlay: vec![PaneRect::new(104, 10, 6, 72, 26, "note", false)],
        },
    );

    // Which float ids are pinned, keyed by tab position — the #119 input that
    // stamps the marker (and, for tab 2, keeps `note` out of the chip row).
    let mut pinned = BTreeMap::new();
    pinned.insert(1usize, vec![100usize]);
    pinned.insert(2usize, vec![104usize]);

    // Three tabs, active in the middle. Perspective is off so every tab renders
    // at full height and the markers / chips read clearly on each block; a plain
    // `pack` (no "+" button) keeps the focus on the pinned affordances.
    let layout = line::pack(160, 0, line::ACTIVE_MAX, 3, 1, Alignment::Left, GAP);
    let bar = paint::bar(
        ROWS,
        &layout,
        &panes,
        &palette,
        "\u{2318} ",
        GradientSpec::from_mode(GradientMode::Sheen),
        true,       // inactive_dim
        false,      // perspective off — full height, so overlays/markers are legible
        Close::Off, // keep the focus on the pinned floats
        &floats,
        &BTreeMap::new(), // no suppressed panes in this sample (#118 has its own)
        &pinned,
    );

    // Hide the cursor so a held screenshot doesn't catch a stray cursor block
    // past the strip, move it below the rendered bar, then restore it so the demo
    // doesn't leave the terminal in a hidden-cursor state.
    print!("{bar}\u{1b}[?25l\u{1b}[{n};1H\u{1b}[?25h", n = ROWS + 2);
}

//! README "Preview" generator — renders a representative multi-tab bar that
//! showcases the current renderer in one shot: color-coded pane minimaps,
//! gradient-sheen fills, active-tab emphasis (#59), the focus ring, per-tab
//! `⌘N` badges and degrading labels, and the perspective depth cue (#66) that
//! lifts the active tab. Five tabs of varied layouts approximate a real zellij
//! session.
//! Not part of the plugin — run with e.g.
//! `cargo run --example render_demo --target aarch64-apple-darwin`
//! (substitute your host triple to override the wasm32-wasip1 default).

use std::collections::BTreeMap;

use zellij_tabmap::color::Palette;
use zellij_tabmap::line::{self, Alignment};
use zellij_tabmap::minimap::{GradientMode, GradientSpec, PaneRect};
use zellij_tabmap::paint;

fn main() {
    // Tokyonight-ish slots + the frame-highlight orange as accent.
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
    // Five varied layouts, as in a live session. Each tab carries a focused
    // pane, but only the active tab's focus ring is drawn (#59).
    let mut panes = BTreeMap::new();
    // ⌘1 — a single editor pane.
    panes.insert(0, vec![PaneRect::new(0, 0, 0, 120, 40, "nvim", true)]);
    // ⌘2 — a two-column split.
    panes.insert(
        1,
        vec![
            PaneRect::new(1, 0, 0, 60, 40, "nvim", true),
            PaneRect::new(2, 60, 0, 60, 40, "zsh", false),
        ],
    );
    // ⌘3 (active) — a 2×2 grid; the focused pane gets the ring.
    panes.insert(
        2,
        vec![
            PaneRect::new(3, 0, 0, 60, 20, "cargo", true),
            PaneRect::new(4, 60, 0, 60, 20, "実装中", false),
            PaneRect::new(5, 0, 20, 60, 20, "git", false),
            PaneRect::new(6, 60, 20, 60, 20, "test", false),
        ],
    );
    // ⌘4 — a main pane with a stacked sidebar.
    panes.insert(
        3,
        vec![
            PaneRect::new(7, 0, 0, 72, 40, "server", true),
            PaneRect::new(8, 72, 0, 48, 20, "logs", false),
            PaneRect::new(9, 72, 20, 48, 20, "redis", false),
        ],
    );
    // ⌘5 — a two-row split.
    panes.insert(
        4,
        vec![
            PaneRect::new(10, 0, 0, 120, 20, "docs", true),
            PaneRect::new(11, 0, 20, 120, 20, "shell", false),
        ],
    );

    // Center-aligned so the strip follows focus; the active tab (position 2)
    // sits in the middle and the perspective cue lifts it forward.
    let layout = line::pack(170, 0, line::ACTIVE_MAX, 5, 2, Alignment::Center, 2);
    print!(
        "{}",
        paint::bar(
            4,
            &layout,
            &panes,
            &palette,
            "\u{2318} ",
            GradientSpec::from_mode(GradientMode::Sheen),
            true,
            true,
            false,
        )
    );
}

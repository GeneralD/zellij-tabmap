//! Standalone visual harness for #59: renders a full three-tab bar so the
//! active-tab cues (inactive dimming + white badge/label text + suppressed
//! inactive focus highlight) can be eyeballed.
//! Not part of the plugin — run with e.g.
//! `cargo run --example render_active_cue --target aarch64-apple-darwin`
//! (substitute your host triple to override the wasm32-wasip1 default).
//! Pass `no-dim` as the first argument to preview the `inactive_dim: false`
//! opt-out.

use std::collections::BTreeMap;

use zellij_tabmap::color::Palette;
use zellij_tabmap::line::{self, Alignment};
use zellij_tabmap::minimap::{GradientMode, PaneRect};
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
    let inactive_dim = std::env::args().nth(1).as_deref() != Some("no-dim");
    let layout = line::pack(100, 0, 24, 3, 1, Alignment::Center, 2);
    let mut panes = BTreeMap::new();
    // Every tab carries a focused pane, as in a live zellij session — the
    // inactive tabs' focused panes must show NO highlight (#59).
    panes.insert(
        0,
        vec![
            PaneRect::new(0, 0, 0, 60, 40, "nvim", true),
            PaneRect::new(1, 60, 0, 60, 40, "zsh", false),
        ],
    );
    panes.insert(
        1,
        vec![
            PaneRect::new(2, 0, 0, 60, 20, "cargo", true),
            PaneRect::new(3, 60, 0, 60, 40, "実装中", false),
            PaneRect::new(4, 0, 20, 60, 20, "git", false),
        ],
    );
    panes.insert(2, vec![PaneRect::new(5, 0, 0, 120, 40, "docs", true)]);
    print!(
        "{}",
        paint::bar(
            3,
            &layout,
            &panes,
            &palette,
            "\u{2318} ",
            GradientMode::Sheen,
            inactive_dim,
            false,
        )
    );
}

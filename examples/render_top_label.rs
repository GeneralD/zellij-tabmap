//! Standalone visual harness for #65: renders a bar whose active tab holds a
//! top/bottom split. The top pane spans exactly two text rows, so the centered
//! label choice would land on the shared middle row and bleed its lower
//! half-block pixel into the pane below. The fix biases that label up to the
//! pane's wholly-owned first row, dodging the shortcut badge in the top-left.
//! Not part of the plugin — run with e.g.
//! `cargo run --example render_top_label --target aarch64-apple-darwin`
//! (substitute your host triple to override the wasm32-wasip1 default).

use std::collections::BTreeMap;

use zellij_tabmap::color::Palette;
use zellij_tabmap::line::{self, Alignment};
use zellij_tabmap::minimap::{Close, GradientMode, GradientSpec, PaneRect};
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
    let layout = line::pack(64, 0, 40, 2, 0, Alignment::Center, 2);
    let mut panes = BTreeMap::new();
    // Active tab (0): a top/bottom split. The top pane occupies the upper half
    // (pixels 0..3 of the 6-pixel canvas) — exactly two text rows — so its
    // label must ride the first row, clear of the bottom pane.
    panes.insert(
        0,
        vec![
            PaneRect::new(0, 0, 0, 120, 20, "server", false),
            PaneRect::new(1, 0, 20, 120, 20, "logs", true),
        ],
    );
    // One inactive tab for context.
    panes.insert(1, vec![PaneRect::new(2, 0, 0, 120, 40, "nvim", true)]);
    print!(
        "{}",
        paint::bar(
            3,
            &layout,
            &panes,
            &palette,
            "\u{2318} ",
            GradientSpec::from_mode(GradientMode::Sheen),
            true,
            false,
            Close::Off,
            &BTreeMap::new(),
            &BTreeMap::new(), // suppressed-pane covers — none in this sample
            &BTreeMap::new(), // pinned-float ids — none in this sample
        )
    );
}

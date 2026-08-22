//! Standalone visual harness for #111: renders the same 2×2 tab twice so the
//! `color_strategy` opt-in can be eyeballed. The tab's pane ids collide
//! pairwise on the 6-slot palette (0/6 both hit slot 0, 1/7 slot 1): under the
//! default `stable` keying each colliding pair blends into one block, while
//! `distinct` nudges the colliding neighbor to the next free slot so every
//! seam reads. Blocks are assembled directly (not through `paint::bar`, whose
//! composed frame homes the cursor absolutely and would overprint the first
//! sample with the second).
//! Not part of the plugin — run with e.g.
//! `cargo run --example render_spread --target aarch64-apple-darwin`
//! (substitute your host triple to override the wasm32-wasip1 default).

use zellij_tabmap::color::Palette;
use zellij_tabmap::minimap::{Close, GradientMode, GradientSpec, PaneRect};
use zellij_tabmap::spread::ColorStrategy;
use zellij_tabmap::tab_block::{self, StyledLine};

fn main() {
    // Tokyonight-ish slots + the frame-highlight orange as accent — the same
    // palette as the sibling examples.
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
    // A 2×2 grid whose ids collide pairwise on the 6-slot palette: 0/6 sit
    // side by side on slot 0 (blue) and 1/7 on slot 1 (green) — the exact
    // adjacency collision #111 exists to kill.
    let panes = vec![
        PaneRect::new(0, 0, 0, 60, 20, "nvim", true),
        PaneRect::new(6, 60, 0, 60, 20, "zsh", false),
        PaneRect::new(1, 0, 20, 60, 20, "cargo", false),
        PaneRect::new(7, 60, 20, 60, 20, "git", false),
    ];
    for (label, strategy) in [
        (
            "color_strategy \"stable\" (default) — colliding neighbors blend into one block:",
            ColorStrategy::Stable,
        ),
        (
            "color_strategy \"distinct\" — every seam reads, non-colliding panes keep their hue:",
            ColorStrategy::Distinct,
        ),
    ] {
        println!("{label}");
        let block = tab_block::assemble(
            &panes,
            &palette,
            40,
            4,
            0,
            "\u{2318} ",
            GradientSpec::from_mode(GradientMode::Sheen),
            true,
            true,
            Close::Off,
            zellij_tabmap::floating::FloatLayer::None,
            &[],
            &[],
            strategy,
        );
        block
            .lines
            .iter()
            .map(StyledLine::as_str)
            .for_each(|line| println!("{line}\u{1b}[0m"));
        println!();
    }
}

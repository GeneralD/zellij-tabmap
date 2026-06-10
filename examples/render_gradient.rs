//! Standalone visual harness for #40: renders the same 2x2 layout in all
//! three gradient modes side by side. Not part of the plugin — run with
//! `cargo run --example render_gradient --target aarch64-apple-darwin`.

use zellij_tabmap::color::Palette;
use zellij_tabmap::minimap::{GradientMode, LabelMode, PaneRect, render};

fn main() {
    let panes = vec![
        PaneRect::new(0, 0, 0, 50, 20, "nvim", true),
        PaneRect::new(1, 50, 0, 50, 20, "cargo", false),
        PaneRect::new(2, 0, 20, 50, 20, "zsh", false),
        PaneRect::new(3, 50, 20, 50, 20, "git", false),
    ];
    let palette = Palette::default();
    let width: usize = std::env::args()
        .nth(1)
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(28);
    for (name, mode) in [
        ("off", GradientMode::Off),
        ("sheen", GradientMode::Sheen),
        ("weave", GradientMode::Weave),
    ] {
        println!("-- gradient \"{name}\" --");
        print!(
            "{}",
            render(
                &panes,
                &palette,
                width,
                3,
                LabelMode::All,
                Some("\u{2318} 1"),
                mode,
            )
        );
    }
}

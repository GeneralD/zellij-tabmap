//! Standalone visual harness for #40 / #71: renders the same 2x2 layout across
//! gradient modes *and* directions (linear angles + radial) side by side. Not
//! part of the plugin — run with e.g.
//! `cargo run --example render_gradient --target x86_64-unknown-linux-gnu`
//! (or substitute your host target to override the wasm32-wasip1 default).

use zellij_tabmap::color::Palette;
use zellij_tabmap::minimap::{
    Close, GradientMode, GradientShape, GradientSpec, LabelMode, PaneRect, RadialDirection, render,
};

/// A linear sheen at `angle` degrees.
fn linear(angle: u16) -> GradientSpec {
    GradientSpec {
        mode: GradientMode::Sheen,
        shape: GradientShape::Linear,
        angle,
        radial: RadialDirection::Outward,
    }
}

/// A radial sheen in `radial` direction.
fn radial(radial: RadialDirection) -> GradientSpec {
    GradientSpec {
        mode: GradientMode::Sheen,
        shape: GradientShape::Radial,
        angle: 0,
        radial,
    }
}

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
    for (name, spec) in [
        ("off", GradientSpec::from_mode(GradientMode::Off)),
        ("sheen (0° L→R)", linear(0)),
        ("sheen (90° top→bottom)", linear(90)),
        ("sheen (45° diagonal)", linear(45)),
        ("sheen (180° reverse)", linear(180)),
        ("weave", GradientSpec::from_mode(GradientMode::Weave)),
        ("radial outward", radial(RadialDirection::Outward)),
        ("radial inward", radial(RadialDirection::Inward)),
    ] {
        println!("-- gradient \"{name}\" --");
        print!(
            "{}",
            render(
                &panes,
                &palette,
                width,
                3,
                0,
                LabelMode::All,
                Some("\u{2318} 1"),
                Close::Off,
                spec,
                true,
                zellij_tabmap::floating::FloatLayer::None,
                &[], // suppressed-pane covers — none in this sample
            )
        );
    }
}

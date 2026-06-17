//! Standalone native render of one wide active tab, comparing the pre-#32 palette
//! (emphasis-color scrape) against the #32 follow palette (multiplayer colors),
//! both derived from zellij's real `tokyo-night` theme. No zellij/host needed.
//!
//! Usage: render_palette <old|new> [cols]
//! Emits the raw bar ANSI (absolute cursor positioning) to stdout.
//!
//! Both palettes are built through the same `Palette::new` the plugin uses, so
//! the black-sentinel drop and modulo cycling match production exactly; only the
//! *slot source* differs, which is precisely what #32 changes.

use std::collections::BTreeMap;

use zellij_tabmap::color::{Palette, Rgb};
use zellij_tabmap::line::{Alignment, pack};
use zellij_tabmap::minimap::{GradientMode, GradientSpec, PaneRect};
use zellij_tabmap::paint::bar;

// frame_highlight of tokyo-night: base is the accent that seeds the hint text
// shade. Focus rings are derived per pane from its own fill (issue #47).
const ACCENT: Rgb = (255, 158, 100); // orange

/// Pre-#32 palette: `emphasis_0..3` of `text_unselected`, `ribbon_unselected`,
/// and `frame_selected`, deduped in order — verbatim tokyo-night values run
/// through the very dedup the old `palette_from_style` used. Note the glaring
/// near-white `(192,202,245)` (a text *foreground* color) cycling in as a pane
/// fill, and that the orange accent collides with slot 0.
fn old_tokyonight() -> Palette {
    let emphasis: [[Rgb; 4]; 3] = [
        // text_unselected
        [
            (255, 158, 100),
            (42, 195, 222),
            (158, 206, 106),
            (187, 154, 247),
        ],
        // ribbon_unselected
        [
            (249, 51, 87),
            (192, 202, 245),
            (122, 162, 247),
            (187, 154, 247),
        ],
        // frame_selected (emphasis_3 is unset → the (0,0,0) sentinel)
        [(255, 158, 100), (42, 195, 222), (187, 154, 247), (0, 0, 0)],
    ];
    let slots = emphasis
        .into_iter()
        .flatten()
        .fold(Vec::new(), |mut acc, v| {
            if !acc.contains(&v) {
                acc.push(v);
            }
            acc
        });
    Palette::new(slots, ACCENT)
}

/// #32 follow palette: `multiplayer_user_colors`, verbatim tokyo-night values.
/// The unset players are the `(0,0,0)` sentinel that `Palette::new` drops, so
/// five clean entity hues survive and the orange accent stays reserved.
fn new_tokyonight() -> Palette {
    let players: [Rgb; 10] = [
        (187, 154, 247), // player_1  purple
        (122, 162, 247), // player_2  blue
        (0, 0, 0),       // player_3  unset
        (224, 175, 104), // player_4  yellow
        (42, 195, 222),  // player_5  cyan
        (0, 0, 0),       // player_6  unset
        (249, 51, 87),   // player_7  red
        (0, 0, 0),       // player_8  unset
        (0, 0, 0),       // player_9  unset
        (0, 0, 0),       // player_10 unset
    ];
    Palette::new(players.to_vec(), ACCENT)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let palette = match args.get(1).map(String::as_str) {
        Some("old") => old_tokyonight(),
        _ => new_tokyonight(),
    };
    let cols: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(30);

    // One wide active tab carrying a 2x3 pane grid, so several distinct slot
    // fills plus the focused pane's accent + ring are all on screen at once.
    let layout = pack(cols, 0, 28, 1, 0, Alignment::Center, 2);
    let mut panes: BTreeMap<usize, Vec<PaneRect>> = BTreeMap::new();
    panes.insert(
        0,
        vec![
            PaneRect::new(0, 0, 0, 33, 20, "nvim", false),
            PaneRect::new(1, 33, 0, 34, 20, "cargo", false),
            PaneRect::new(2, 67, 0, 33, 20, "git", false),
            PaneRect::new(3, 0, 20, 33, 20, "node", false),
            PaneRect::new(4, 33, 20, 34, 20, "zsh", true),
            PaneRect::new(5, 67, 20, 33, 20, "docker", false),
        ],
    );

    print!(
        "{}",
        bar(
            3,
            &layout,
            &panes,
            &palette,
            "\u{2318}",
            GradientSpec::from_mode(GradientMode::Sheen),
            true,
            false,
            false,
        )
    );
}

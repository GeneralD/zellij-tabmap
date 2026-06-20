//! The adapter that turns a live zellij theme [`Style`] into the renderer's
//! own [`color::Palette`]. This is the one boundary that touches both zellij's
//! color types and the dependency-free [`color`] module, so it is kept apart
//! from the pure renderer (rule #8): nothing here leaks a zellij type past
//! `palette_from_style`'s return value.

use zellij_tile::prelude::*;

use crate::color;

/// Convert a zellij theme color to the renderer's [`color::Rgb`].
fn rgb(c: PaletteColor) -> color::Rgb {
    match c {
        PaletteColor::Rgb(v) => v,
        PaletteColor::EightBit(n) => color::from_eightbit(n),
    }
}

/// Build the pane palette from the active theme style.
///
/// Slots come from the theme's `multiplayer_user_colors` — the set a theme
/// author designs to tell *different session users apart*, which is exactly
/// this bar's job: telling *different panes apart*. Being categorical
/// distinguishing colors, they read as coherent adjacent fills on the bar
/// background by construction — unlike the `emphasis` foreground-accent colors
/// an earlier version scraped, which are tuned to sit *on top of* a fill, not
/// beside one, and so never cohered as a minimap ramp. A theme defines only as
/// many player slots as it cares to; the rest stay unset and collapse to the
/// black sentinel that [`color::Palette::new`] drops — so a theme defining five
/// players yields five hues. The focused pane keeps its slot fill, and its
/// focus ring is derived from that fill as a luminance-shifted shade — the
/// outline stays in the pane's own hue family (issue #47).
/// `frame_highlight.base` is the accent that seeds the degraded-rung hint
/// text shade ([`color::Palette::hint`], issue #32). `exit_code_error.base` —
/// zellij's own semantic red — colors the close glyph ([`color::Palette::alert`],
/// issue #86).
pub(crate) fn palette_from_style(style: &Style) -> color::Palette {
    let colors = &style.colors;
    let players = colors.multiplayer_user_colors;
    let slots = [
        players.player_1,
        players.player_2,
        players.player_3,
        players.player_4,
        players.player_5,
        players.player_6,
        players.player_7,
        players.player_8,
        players.player_9,
        players.player_10,
    ]
    .into_iter()
    .map(rgb)
    .collect();
    color::Palette::new(slots, rgb(colors.frame_highlight.base))
        .with_alert(rgb(colors.exit_code_error.base))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_slots_come_from_multiplayer_user_colors() {
        // The follow palette draws pane fills from the theme's categorical
        // "distinguish session users" colors. A theme that defines three
        // players and leaves the rest unset must yield exactly those three
        // hues, in declaration order, with the unset (black-sentinel) slots
        // dropped — so pane identity cycles over a coherent theme-authored ramp
        // rather than the foreground emphasis colors an earlier version scraped.
        let mut style = Style::default();
        style.colors.multiplayer_user_colors = MultiplayerColors {
            player_1: PaletteColor::Rgb((10, 20, 30)),
            player_2: PaletteColor::Rgb((40, 50, 60)),
            player_4: PaletteColor::Rgb((70, 80, 90)),
            // player_3 and player_5..=player_10 stay EightBit(0) → dropped.
            ..Default::default()
        };
        style.colors.frame_highlight.base = PaletteColor::Rgb((200, 100, 50));

        let p = palette_from_style(&style);

        // Exactly the three defined hues, cycled by identity in declaration
        // order (player_3 dropped between player_2 and player_4).
        assert_eq!(p.color_for(0), (10, 20, 30));
        assert_eq!(p.color_for(1), (40, 50, 60));
        assert_eq!(p.color_for(2), (70, 80, 90));
        assert_eq!(p.color_for(3), (10, 20, 30));

        // The ring is derived from the pane's own fill as a luminance-shifted
        // shade (issue #47), so the outline stays in the pane's hue family
        // rather than tracking the theme accent.
        assert_ne!(p.ring_for(0), p.color_for(0));
    }
}

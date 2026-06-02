//! zellij-tabmap — a multi-row zellij tab bar that renders each tab as a
//! color-coded minimap of its pane layout.
//!
//! The plugin holds the latest tab and pane snapshots zellij hands it and, on
//! every relevant event, repaints. The actual pixel rendering lives in the
//! dependency-free [`minimap`] module so it can be unit-tested off-wasm.

pub mod color;
pub mod config;
pub mod line;
pub mod minimap;
pub mod paint;
pub mod projection;
pub mod tab_block;
pub mod title;

use std::collections::BTreeMap;
use zellij_tile::prelude::*;

use config::Config;

/// Text-row height of the bar. The layout pins the plugin pane to `size=3`, and
/// the minimap renders 2 vertical pixels per text row → a 6px-tall canvas.
const ROWS: usize = 3;

/// Plugin state: parsed configuration plus the most recent tab and pane
/// snapshots from zellij, and the theme-derived color palette.
#[derive(Default)]
pub struct State {
    config: Config,
    permitted: bool,
    tabs: Vec<TabInfo>,
    panes: PaneManifest,
    /// Pane colors derived from the live theme. Starts at the default-theme
    /// fallback (see [`color::Palette::default`]) and is refreshed on every
    /// `ModeUpdate`, which is how zellij delivers the active style.
    palette: color::Palette,
    /// The most recent render's per-tab column spans — the source of truth for
    /// click hit-testing. Re-recorded on every `render()` (and renders fire on
    /// each Tab/Pane update), so a click always tests against what is currently
    /// drawn, never a stale frame. Empty until the first render.
    tab_layout: Vec<line::TabHit>,
}

/// Convert a zellij theme color to the renderer's [`color::Rgb`].
fn rgb(c: PaletteColor) -> color::Rgb {
    match c {
        PaletteColor::Rgb(v) => v,
        PaletteColor::EightBit(n) => color::from_eightbit(n),
    }
}

/// Build the pane palette from the active theme style.
///
/// Slots are the four `emphasis` colors of three representative style
/// declarations (unselected text, unselected ribbon, selected frame),
/// deduped in order — the default theme yields roughly eight distinct hues.
/// The focused pane uses `frame_highlight`: its `base` as the accent fill and
/// `emphasis_0` as the ring.
fn palette_from_style(style: &Style) -> color::Palette {
    let colors = &style.colors;
    let slots = [
        colors.text_unselected,
        colors.ribbon_unselected,
        colors.frame_selected,
    ]
    .into_iter()
    .flat_map(|d| [d.emphasis_0, d.emphasis_1, d.emphasis_2, d.emphasis_3])
    .map(rgb)
    .fold(Vec::new(), |mut acc, v| {
        if !acc.contains(&v) {
            acc.push(v);
        }
        acc
    });
    color::Palette::new(
        slots,
        rgb(colors.frame_highlight.base),
        rgb(colors.frame_highlight.emphasis_0),
    )
}

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        // A fixed-size (`size=3`) default_tab_template pane is only stable when
        // the plugin marks itself non-selectable. Assert it first, then again
        // on PermissionResult (see `update`), since the post-permission
        // re-render is when a stale selectable state would surface.
        set_selectable(false);
        self.config = Config::from_configuration(&configuration);
        // ReadApplicationState delivers the Tab/Pane/Mode updates we render
        // from; ChangeApplicationState authorizes `switch_tab_to` for
        // click-to-switch (#8). A plugin started from `default_tab_template`
        // cannot show the interactive permission dialog (zellij#4982), so users
        // pre-grant both in the plugin permission cache and reload.
        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
        ]);
        subscribe(&[
            EventType::PermissionRequestResult,
            EventType::TabUpdate,
            EventType::PaneUpdate,
            EventType::ModeUpdate,
            EventType::Mouse,
        ]);
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::PermissionRequestResult(status) => {
                self.permitted = status == PermissionStatus::Granted;
                // Re-assert non-selectable: the post-permission re-render is the
                // moment a stale selectable state would destabilize the bar.
                set_selectable(false);
                true
            }
            Event::TabUpdate(tabs) => {
                self.tabs = tabs;
                true
            }
            Event::PaneUpdate(panes) => {
                self.panes = panes;
                true
            }
            Event::ModeUpdate(mode_info) => {
                // zellij delivers the active theme via the mode style. Refresh
                // the palette and repaint so pane colors track theme changes.
                self.palette = palette_from_style(&mode_info.style);
                true
            }
            Event::Mouse(Mouse::LeftClick(_row, column)) => {
                // A left click anywhere in a tab's column span focuses that tab;
                // the clicked row is irrelevant. The switch arrives back as a
                // TabUpdate, which drives the repaint — so this arm requests none.
                self.switch_to_tab_at(column);
                false
            }
            // Other events — including the non-click Mouse variants reserved for
            // v2 drag-to-reorder — need no repaint.
            _ => false,
        }
    }

    fn render(&mut self, _rows: usize, cols: usize) {
        // Reset the click geometry up front. If this frame bails out before
        // drawing — no permission yet, or no active tab mid-transition — a click
        // must find no spans to resolve against rather than the previous frame's
        // stale ones. The success path repopulates it at the end.
        self.tab_layout.clear();
        if !self.permitted {
            return;
        }
        let Some(active_position) = projection::active_tab(&self.tabs).map(|tab| tab.position)
        else {
            return;
        };

        // Pack the whole tab strip into column spans — active centered, the
        // tabs that don't fit collapsed into `← +N` / `+N →` end markers — then
        // render each visible tab into its budgeted block. `pack` clamps the
        // active width into the legible `16..=28` range, so the parser keeps the
        // raw value (see `config.rs`); §4.3–4.4 of the design.
        let layout = line::pack(
            cols,
            0,
            self.config.active_width,
            self.tabs.len(),
            active_position,
        );

        // Project only the visible tabs' tiled panes (the collapsed ones need no
        // block). Projection is the one step that touches zellij types, so it
        // happens here at the render site and `paint::bar` stays pure. Panes are
        // keyed by `position` so the output never depends on the manifest map's
        // iteration order.
        let panes_by_position: BTreeMap<usize, Vec<minimap::PaneRect>> = layout
            .tabs
            .iter()
            .map(|hit| {
                let panes = self
                    .panes
                    .panes
                    .get(&hit.position)
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                (hit.position, projection::project(panes))
            })
            .collect();

        print!(
            "{}",
            paint::bar(
                ROWS,
                &layout,
                &panes_by_position,
                &self.palette,
                &self.config.shortcut_prefix,
            )
        );

        // Record the spans this frame drew so a later click hit-tests against
        // the current layout. `pack` re-runs every render, so this is always
        // the live geometry — never a cached copy.
        self.tab_layout = layout.tabs;
    }
}

impl State {
    /// Focus the tab whose drawn block contains `column`; a click that landed on
    /// no block (overflow marker, gap, trailing padding) is a no-op. `column` is
    /// the 0-based click column zellij delivers, and
    /// [`line::switch_target_at_column`] resolves it to the 1-based index
    /// `switch_tab_to` expects.
    fn switch_to_tab_at(&self, column: usize) {
        let Some(target) = line::switch_target_at_column(&self.tab_layout, column) else {
            return;
        };
        switch_tab_to(target);
    }
}

// Native test builds link the whole lib, which references zellij-tile's host
// imports (all routed through `host_run_plugin_command`). Provide the symbol
// so `cargo test --lib --target <host>` links off-wasm. On wasm the real host
// supplies it, so this stub is compiled only under `cfg(test)`.
#[cfg(test)]
#[no_mangle]
extern "C" fn host_run_plugin_command() {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::line::TabHit;

    #[test]
    fn render_clears_tab_layout_when_it_cannot_draw() {
        // The bar only draws once permitted, and only repopulates `tab_layout`
        // on that success path. A frame that bails out earlier must still wipe
        // the previous frame's spans — otherwise a click would resolve against
        // geometry no longer on screen. (`permitted` defaults to false, so this
        // exercises the pre-draw early return.)
        let mut state = State::default();
        state.tab_layout = vec![TabHit {
            position: 3,
            start: 0,
            width: 8,
            active: true,
        }];

        state.render(ROWS, 80);

        assert!(
            state.tab_layout.is_empty(),
            "a frame that cannot draw leaves no stale click geometry"
        );
    }
}

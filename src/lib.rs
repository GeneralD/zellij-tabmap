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
        // v1 only reads state (to receive Tab/Pane updates). The permission
        // for tab switching / reordering is requested in the milestone that
        // actually performs the action, not eagerly here.
        request_permission(&[PermissionType::ReadApplicationState]);
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
            // Mouse is subscribed in `load()` to establish the event plumbing,
            // but intentionally not acted on yet: the current render does not
            // depend on it, so skipping the repaint is correct. Click-to-switch
            // lands in a later interaction milestone.
            _ => false,
        }
    }

    fn render(&mut self, _rows: usize, cols: usize) {
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
    }
}

// Native test builds link the whole lib, which references zellij-tile's host
// imports (all routed through `host_run_plugin_command`). Provide the symbol
// so `cargo test --lib --target <host>` links off-wasm. On wasm the real host
// supplies it, so this stub is compiled only under `cfg(test)`.
#[cfg(test)]
#[no_mangle]
extern "C" fn host_run_plugin_command() {}

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

        // Project the active tab's tiled panes into the renderer's rectangles,
        // then paint its 3-row minimap. The width budget is clamped to a legible
        // range here, at the render site, while the parser keeps the raw value
        // (see `config.rs`); §4.4 of the design.
        let panes = self
            .panes
            .panes
            .get(&active_position)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let width = self.config.active_width.clamp(16, 28).min(cols);
        let block = minimap::render(
            &projection::project(panes),
            &self.palette,
            width,
            ROWS,
            true,
        );

        // Non-active tabs get a minimal `⌘N` placeholder for now; full
        // width-budgeted multi-tab packing lands in the layout issue (#4).
        let hints = paint::inactive_hints(
            self.tabs
                .iter()
                .filter(|tab| !tab.active)
                .map(|tab| tab.position),
            &self.config.shortcut_prefix,
        );

        print!(
            "{}{}",
            paint::framed(&block, ROWS),
            paint::positioned_hints(&hints, width, cols)
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

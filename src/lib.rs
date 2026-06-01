//! zellij-tabmap — a multi-row zellij tab bar that renders each tab as a
//! color-coded minimap of its pane layout.
//!
//! The plugin holds the latest tab and pane snapshots zellij hands it and, on
//! every relevant event, repaints. The actual pixel rendering lives in the
//! dependency-free [`minimap`] module so it can be unit-tested off-wasm.

pub mod minimap;

use std::collections::BTreeMap;
use zellij_tile::prelude::*;

/// Plugin state: the most recent tab and pane snapshots from zellij.
#[derive(Default)]
pub struct State {
    permitted: bool,
    tabs: Vec<TabInfo>,
    panes: PaneManifest,
}

impl ZellijPlugin for State {
    fn load(&mut self, _configuration: BTreeMap<String, String>) {
        // v1 only reads state (to receive Tab/Pane updates). The permission
        // for tab switching / reordering is requested in the milestone that
        // actually performs the action, not eagerly here.
        request_permission(&[PermissionType::ReadApplicationState]);
        subscribe(&[
            EventType::PermissionRequestResult,
            EventType::TabUpdate,
            EventType::PaneUpdate,
            EventType::ModeUpdate,
        ]);
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::PermissionRequestResult(status) => {
                self.permitted = status == PermissionStatus::Granted;
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
            _ => false,
        }
    }

    fn render(&mut self, _rows: usize, _cols: usize) {
        if !self.permitted {
            return;
        }
        // Placeholder until #1 wires the minimap: project each tab's
        // PaneManifest entry into `minimap::PaneRect`s and print
        // `minimap::render(...)`, packing the blocks across the columns.
        // The renderer it will call is already complete and unit-tested.
        print!(
            "zellij-tabmap: {} tab(s), {} pane group(s) — minimap pending (#1)",
            self.tabs.len(),
            self.panes.panes.len()
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

//! Adapter from zellij's live application state to the renderer's input type.
//!
//! This is the one module that knows both vocabularies: it reads `TabInfo` /
//! `PaneInfo` (zellij-tile) and produces [`PaneRect`] (the dependency-free
//! [`crate::minimap`] renderer's input). Keeping the translation here lets the
//! renderer stay free of any zellij type, so it remains unit-testable off-wasm.
//!
//! Both functions are pure and total: no panics, no host calls. `PaneInfo` and
//! `TabInfo` derive `Default`, so the tests build fixtures with struct-update
//! syntax and run natively (`cargo test --lib` on the host triple).

use zellij_tile::prelude::{PaneInfo, TabInfo};

use crate::minimap::PaneRect;

/// The active tab, if any — the one zellij marks `active == true`.
///
/// zellij always has exactly one active tab in practice; the `Option` keeps the
/// projection total for the degenerate empty / no-active snapshot.
pub fn active_tab(tabs: &[TabInfo]) -> Option<&TabInfo> {
    tabs.iter().find(|tab| tab.active)
}

/// Whether a pane belongs to the user's tiled terminal layout — the set the
/// minimap depicts.
///
/// Excludes plugin panes (the tab-bar / status-bar / attention overlays),
/// floating panes, and suppressed (background) panes. The single source of this
/// filter, so every consumer of the tiled set stays in sync.
pub fn is_tiled_terminal(pane: &PaneInfo) -> bool {
    !(pane.is_plugin || pane.is_floating || pane.is_suppressed)
}

/// Whether a pane is a floating **terminal** pane — the set the bar overlays or
/// chips (#110). The floating sibling of [`is_tiled_terminal`]: it keeps
/// `is_floating` panes but still drops plugin and suppressed ones, so the
/// floating layer never picks up chrome or background panes. `is_suppressed` is
/// excluded on purpose — suppressed panes stay out of scope for #110.
pub fn is_floating_terminal(pane: &PaneInfo) -> bool {
    pane.is_floating && !(pane.is_plugin || pane.is_suppressed)
}

/// The ids of a tab's tiled terminal panes in **reading order** — top to bottom,
/// then left to right — the per-tab order the wheel walks in `pane` mode (#80,
/// restored #108). Chrome / floating / suppressed panes are dropped via the same
/// [`is_tiled_terminal`] filter the minimap uses, so the wheel and the minimap
/// can never disagree on which panes exist.
pub fn pane_ids_in_reading_order(panes: &[PaneInfo]) -> Vec<u32> {
    let mut tiled: Vec<&PaneInfo> = panes
        .iter()
        .filter(|pane| is_tiled_terminal(pane))
        .collect();
    tiled.sort_by_key(|pane| (pane.pane_y, pane.pane_x));
    tiled.into_iter().map(|pane| pane.id).collect()
}

/// Project a tab's panes into renderer rectangles, dropping everything that is
/// not part of the user's tiled layout.
///
/// Filtered out: plugin panes (the tab-bar / status-bar / attention overlays),
/// floating panes, and suppressed panes. What survives is the tiled content
/// whose `pane_x/y/columns/rows` describe the layout the minimap depicts. The
/// renderer normalizes by bounding box, so an absolute-vs-relative origin offset
/// in the coordinates does not affect the projected topology.
pub fn project(panes: &[PaneInfo]) -> Vec<PaneRect> {
    panes
        .iter()
        .filter(|pane| is_tiled_terminal(pane))
        .map(|pane| {
            PaneRect::new(
                pane.id as usize,
                pane.pane_x as u32,
                pane.pane_y as u32,
                pane.pane_columns as u32,
                pane.pane_rows as u32,
                pane.title.clone(),
                pane.is_focused,
            )
        })
        .collect()
}

/// Project a tab's **floating** panes into renderer rectangles — the parallel of
/// [`project`] for the floating layer (#110). Carries id, geometry, title, and
/// focus so a visible-layer overlay can place each float; a hidden layer uses
/// only the ids (for corner chips). Order follows the manifest, which is stable
/// per frame.
pub fn project_floating(panes: &[PaneInfo]) -> Vec<PaneRect> {
    panes
        .iter()
        .filter(|pane| is_floating_terminal(pane))
        .map(|pane| {
            PaneRect::new(
                pane.id as usize,
                pane.pane_x as u32,
                pane.pane_y as u32,
                pane.pane_columns as u32,
                pane.pane_rows as u32,
                pane.title.clone(),
                pane.is_focused,
            )
        })
        .collect()
}

/// Whether a pane is a **suppressed** terminal pane — one hidden behind the pane
/// that replaced its slot (#118). The suppressed sibling of [`is_tiled_terminal`]
/// / [`is_floating_terminal`]: keeps `is_suppressed` panes but drops plugin ones
/// (a plugin-driven suppress is chrome, not the user's content).
pub fn is_suppressed_terminal(pane: &PaneInfo) -> bool {
    pane.is_suppressed && !pane.is_plugin
}

/// Project a tab's **suppressed** panes into renderer rectangles — the parallel
/// of [`project`] / [`project_floating`] for the suppressed layer (#118). Carries
/// id + geometry so cover-matching ([`crate::suppressed::cover_ids`]) can find
/// which visible tiled pane hides each. Title/focus ride along for parity with
/// the other projectors; only id + geometry are used downstream.
pub fn project_suppressed(panes: &[PaneInfo]) -> Vec<PaneRect> {
    panes
        .iter()
        .filter(|pane| is_suppressed_terminal(pane))
        .map(|pane| {
            PaneRect::new(
                pane.id as usize,
                pane.pane_x as u32,
                pane.pane_y as u32,
                pane.pane_columns as u32,
                pane.pane_rows as u32,
                pane.title.clone(),
                pane.is_focused,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(position: usize, active: bool) -> TabInfo {
        TabInfo {
            position,
            active,
            ..Default::default()
        }
    }

    fn content_pane(x: usize, y: usize, w: usize, h: usize, focused: bool) -> PaneInfo {
        PaneInfo {
            pane_x: x,
            pane_y: y,
            pane_columns: w,
            pane_rows: h,
            is_focused: focused,
            title: "sh".to_string(),
            ..Default::default()
        }
    }

    fn content_pane_with_id(id: u32, x: usize, focused: bool) -> PaneInfo {
        PaneInfo {
            id,
            pane_x: x,
            pane_y: 1,
            pane_columns: 40,
            pane_rows: 24,
            is_focused: focused,
            title: "sh".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn active_tab_picks_the_marked_one() {
        let tabs = [tab(0, false), tab(1, true), tab(2, false)];
        assert_eq!(active_tab(&tabs).map(|t| t.position), Some(1));
    }

    #[test]
    fn active_tab_is_none_when_unmarked() {
        let tabs = [tab(0, false), tab(1, false)];
        assert!(active_tab(&tabs).is_none());
        assert!(active_tab(&[]).is_none());
    }

    #[test]
    fn project_drops_chrome_and_overlays() {
        let panes = [
            PaneInfo {
                is_plugin: true,
                ..Default::default()
            },
            PaneInfo {
                is_floating: true,
                ..Default::default()
            },
            PaneInfo {
                is_suppressed: true,
                ..Default::default()
            },
            content_pane(0, 1, 80, 24, true),
        ];
        let rects = project(&panes);
        assert_eq!(rects.len(), 1);
        assert!(rects[0].focused);
    }

    #[test]
    fn project_maps_geometry_fields() {
        let rects = project(&[content_pane(10, 1, 80, 24, false)]);
        let rect = &rects[0];
        assert_eq!((rect.x, rect.y, rect.w, rect.h), (10, 1, 80, 24));
        assert_eq!(rect.title, "sh");
        assert!(!rect.focused);
    }

    #[test]
    fn project_preserves_two_column_split_offsets() {
        // Two panes side by side: the projection must keep the horizontal
        // offset so the renderer paints two bands left-to-right.
        let rects = project(&[
            content_pane(0, 1, 40, 24, true),
            content_pane(40, 1, 40, 24, false),
        ]);
        assert_eq!(rects.len(), 2);
        assert_eq!((rects[0].x, rects[1].x), (0, 40));
        assert_eq!(rects[0].y, rects[1].y);
    }

    #[test]
    fn project_preserves_two_row_split_offsets() {
        // Two stacked panes: the vertical offset must survive so the renderer
        // paints two bands top-to-bottom.
        let rects = project(&[
            content_pane(0, 1, 80, 12, true),
            content_pane(0, 13, 80, 12, false),
        ]);
        assert_eq!(rects.len(), 2);
        assert_eq!((rects[0].y, rects[1].y), (1, 13));
        assert_eq!(rects[0].x, rects[1].x);
    }

    #[test]
    fn project_carries_pane_id_as_color_key() {
        // The id must survive projection so the renderer can key colors on
        // stable identity. Position in the list must not become the id.
        let rects = project(&[
            content_pane_with_id(7, 0, true),
            content_pane_with_id(3, 40, false),
        ]);
        assert_eq!((rects[0].id, rects[1].id), (7, 3));
    }

    #[test]
    fn project_is_empty_for_no_content_panes() {
        let panes = [PaneInfo {
            is_plugin: true,
            ..Default::default()
        }];
        assert!(project(&panes).is_empty());
        assert!(project(&[]).is_empty());
    }

    #[test]
    fn is_floating_terminal_selects_only_floats() {
        // A floating terminal pane passes; tiled, plugin, and suppressed panes do not.
        assert!(is_floating_terminal(&PaneInfo {
            is_floating: true,
            ..Default::default()
        }));
        assert!(!is_floating_terminal(&PaneInfo::default())); // tiled
        assert!(!is_floating_terminal(&PaneInfo {
            is_floating: true,
            is_plugin: true,
            ..Default::default()
        }));
        assert!(!is_floating_terminal(&PaneInfo {
            is_floating: true,
            is_suppressed: true,
            ..Default::default()
        }));
    }

    #[test]
    fn project_floating_keeps_only_floats_with_geometry() {
        // Two floats and one tiled pane: only the floats survive, carrying id and
        // geometry (so a visible-layer overlay can place them). The tiled pane is
        // dropped — `project` (not this) handles the tiled layer.
        let panes = [
            content_pane(0, 1, 80, 24, true), // tiled
            PaneInfo {
                id: 7,
                is_floating: true,
                pane_x: 10,
                pane_y: 5,
                pane_columns: 30,
                pane_rows: 10,
                is_focused: true,
                title: "top".to_string(),
                ..Default::default()
            },
            PaneInfo {
                id: 9,
                is_floating: true,
                pane_x: 40,
                pane_y: 8,
                pane_columns: 20,
                pane_rows: 6,
                title: "bot".to_string(),
                ..Default::default()
            },
        ];
        let floats = project_floating(&panes);
        assert_eq!(floats.len(), 2);
        assert_eq!((floats[0].id, floats[1].id), (7, 9));
        assert_eq!(
            (floats[0].x, floats[0].y, floats[0].w, floats[0].h),
            (10, 5, 30, 10)
        );
        assert!(floats[0].focused);
    }

    #[test]
    fn project_floating_is_empty_without_floats() {
        assert!(project_floating(&[content_pane(0, 1, 80, 24, true)]).is_empty());
        assert!(project_floating(&[]).is_empty());
    }

    #[test]
    fn project_suppressed_keeps_only_suppressed_terminals() {
        let panes = [
            PaneInfo {
                is_plugin: true,
                is_suppressed: true,
                ..Default::default()
            }, // plugin suppress → dropped
            PaneInfo {
                is_floating: true,
                ..Default::default()
            }, // float → dropped
            content_pane(0, 1, 80, 24, true), // tiled → dropped
            PaneInfo {
                id: 7,
                is_suppressed: true,
                pane_x: 40,
                pane_y: 1,
                pane_columns: 40,
                pane_rows: 24,
                title: "sh".to_string(),
                ..Default::default()
            },
        ];
        let rects = project_suppressed(&panes);
        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0].id, 7);
        assert_eq!(
            (rects[0].x, rects[0].y, rects[0].w, rects[0].h),
            (40, 1, 40, 24)
        );
    }

    #[test]
    fn pane_ids_in_reading_order_sorts_top_to_bottom_then_left_to_right() {
        let pane = |id: u32, x: usize, y: usize| PaneInfo {
            id,
            pane_x: x,
            pane_y: y,
            ..Default::default()
        };
        // Declared out of order; reading order is (pane_y, then pane_x): the
        // top row left→right (10 at x=0, 20 at x=40) then the lower pane (30).
        let panes = [pane(20, 40, 0), pane(30, 0, 12), pane(10, 0, 0)];
        assert_eq!(pane_ids_in_reading_order(&panes), vec![10, 20, 30]);
    }

    #[test]
    fn pane_ids_in_reading_order_drops_chrome_and_overlays() {
        // The same tiled filter as the minimap: plugin / floating / suppressed
        // panes never enter the wheel traversal.
        let panes = [
            PaneInfo {
                id: 1,
                is_plugin: true,
                ..Default::default()
            },
            PaneInfo {
                id: 2,
                is_floating: true,
                ..Default::default()
            },
            PaneInfo {
                id: 3,
                is_suppressed: true,
                ..Default::default()
            },
            content_pane_with_id(7, 0, true),
        ];
        assert_eq!(pane_ids_in_reading_order(&panes), vec![7]);
        assert!(pane_ids_in_reading_order(&[]).is_empty());
    }
}

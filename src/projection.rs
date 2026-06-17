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
/// minimap depicts and the wheel navigates (#80).
///
/// Excludes plugin panes (the tab-bar / status-bar / attention overlays),
/// floating panes, and suppressed (background) panes. The single source of this
/// filter, so [`project`] and the scroll traversal can never drift apart.
pub fn is_tiled_terminal(pane: &PaneInfo) -> bool {
    !(pane.is_plugin || pane.is_floating || pane.is_suppressed)
}

/// The ids of a tab's tiled terminal panes in **reading order** — top to bottom,
/// then left to right — the per-tab order the wheel walks in `pane` mode (#80).
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

    fn pane_at(id: u32, x: usize, y: usize) -> PaneInfo {
        PaneInfo {
            id,
            pane_x: x,
            pane_y: y,
            pane_columns: 40,
            pane_rows: 12,
            title: "sh".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn reading_order_sorts_top_to_bottom_then_left_to_right() {
        // A 2x2 grid given out of order: reading order is top-left, top-right,
        // bottom-left, bottom-right (y first, then x) — the wheel's per-tab walk.
        let panes = [
            pane_at(30, 40, 13), // bottom-right
            pane_at(10, 0, 1),   // top-left
            pane_at(20, 40, 1),  // top-right
            pane_at(25, 0, 13),  // bottom-left
        ];
        assert_eq!(pane_ids_in_reading_order(&panes), vec![10, 20, 25, 30]);
    }

    #[test]
    fn reading_order_drops_chrome_and_overlays() {
        // Only tiled terminal panes are walked — the bars, floats, and suppressed
        // panes never appear in the traversal.
        let panes = [
            PaneInfo {
                is_plugin: true,
                ..pane_at(99, 0, 1)
            },
            pane_at(10, 0, 1),
            PaneInfo {
                is_floating: true,
                ..pane_at(98, 0, 13)
            },
            PaneInfo {
                is_suppressed: true,
                ..pane_at(97, 40, 1)
            },
        ];
        assert_eq!(pane_ids_in_reading_order(&panes), vec![10]);
        assert!(pane_ids_in_reading_order(&[]).is_empty());
    }
}

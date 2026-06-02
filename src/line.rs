//! Pure tab-bar line packing: turn a tab list plus a column budget into per-tab
//! column spans, keeping the active tab centered and collapsing the tabs that do
//! not fit into `← +N` / `+N →` overflow markers at the ends.
//!
//! This is layout math only — no `zellij_tile` calls and no rendering — so it
//! runs and is unit-tested on the native host like the rest of the renderer
//! (`minimap` / `paint` / `projection`). The [`TabHit`] spans it produces are
//! the input for click-to-switch (#8) and, later, drag-and-drop reordering
//! (#10), so each span reflects exactly where a block is drawn, measured in
//! display columns (see [`display_width`]) rather than `char` count.

use unicode_width::UnicodeWidthStr;

/// Active-block clamp range (design §4.4): the active tab carries a precise,
/// title-bearing minimap — kept legible, but never hogging the whole bar.
pub const ACTIVE_MIN: usize = 16;
pub const ACTIVE_MAX: usize = 28;
/// Every shown inactive block is at least this wide, so a packed bar never
/// degrades a tab into a 0/1-column sliver.
pub const INACTIVE_MIN: usize = 2;

/// One visible tab's drawn column span, for hit-testing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabHit {
    /// 0-based tab position (zellij `TabInfo.position`).
    pub position: usize,
    /// 0-based start column in the bar.
    pub start: usize,
    /// Drawn width in display columns.
    pub width: usize,
    pub active: bool,
}

/// A run of collapsed tabs at one end of the bar, drawn as `← +N` / `+N →`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Overflow {
    /// How many tabs this marker stands in for.
    pub hidden: usize,
    /// 0-based start column of the marker text.
    pub start: usize,
    /// The rendered marker (`← +N ` on the left, ` +N →` on the right).
    pub text: String,
}

/// The packed bar: visible tabs left-to-right plus optional end markers.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct LineLayout {
    /// Visible tabs, ordered left-to-right, contiguous around the active tab.
    pub tabs: Vec<TabHit>,
    pub left: Option<Overflow>,
    pub right: Option<Overflow>,
}

/// Display width of a string in terminal cells — icons and CJK count as their
/// real width, not their `char` count — via the Unicode width tables.
pub fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

fn left_marker(hidden: usize) -> String {
    format!("← +{hidden} ")
}

fn right_marker(hidden: usize) -> String {
    format!(" +{hidden} →")
}

/// Pack `tab_count` tabs into `cols` columns with the active tab centered.
///
/// `prefix_width` reserves leading columns (e.g. a logo); `active_desired` is
/// the active block's requested width before the `16..=28` clamp. Inactive tabs
/// share the remainder evenly down to a 2-column floor, capped at the active
/// width so the active stays the prominent block and genuine slack remains to
/// center it. When even the floors do not all fit, the tabs farthest from the
/// active collapse into end markers.
pub fn pack(
    cols: usize,
    prefix_width: usize,
    active_desired: usize,
    tab_count: usize,
    active: usize,
) -> LineLayout {
    let total_w = cols.saturating_sub(prefix_width);
    if tab_count == 0 || total_w == 0 {
        return LineLayout::default();
    }
    let active = active.min(tab_count - 1);
    let active_w = active_desired.clamp(ACTIVE_MIN, ACTIVE_MAX).min(total_w);

    if tab_count == 1 {
        let start = prefix_width + (total_w - active_w) / 2;
        return LineLayout {
            tabs: vec![TabHit {
                position: active,
                start,
                width: active_w,
                active: true,
            }],
            left: None,
            right: None,
        };
    }

    let inactives = tab_count - 1;
    // Cap at the active width (raised to the floor so the clamp bounds never
    // invert on a sub-2-column bar), then floor at 2 columns.
    let inactive_cap = active_w.max(INACTIVE_MIN);
    let inactive_w =
        (total_w.saturating_sub(active_w) / inactives).clamp(INACTIVE_MIN, inactive_cap);

    if active_w + inactives * inactive_w <= total_w {
        packed_centered(
            prefix_width,
            total_w,
            active_w,
            inactive_w,
            tab_count,
            active,
        )
    } else {
        packed_with_overflow(prefix_width, total_w, active_w, tab_count, active)
    }
}

/// Every tab fits: lay them out in order and slide the row so the active block
/// is centered, clamped into the leftover slack so nothing spills off an edge.
fn packed_centered(
    prefix_width: usize,
    total_w: usize,
    active_w: usize,
    inactive_w: usize,
    tab_count: usize,
    active: usize,
) -> LineLayout {
    let content = active_w + (tab_count - 1) * inactive_w;
    let slack = total_w - content;
    let active_centered = (total_w - active_w) / 2;
    // Shift so the blocks before the active one end at the bar's center; clamp
    // into `0..=slack` so a far-left / far-right active just butts against its
    // edge instead of dragging tabs out of view.
    let row_start = active_centered
        .saturating_sub(active * inactive_w)
        .min(slack);

    let tabs = (0..tab_count)
        .scan(prefix_width + row_start, |col, position| {
            let width = if position == active {
                active_w
            } else {
                inactive_w
            };
            let hit = TabHit {
                position,
                start: *col,
                width,
                active: position == active,
            };
            *col += width;
            Some(hit)
        })
        .collect();

    LineLayout {
        tabs,
        left: None,
        right: None,
    }
}

/// Too many tabs to fit even at the floor width: grow a contiguous window
/// outward from the active tab, balancing the two sides, and collapse the rest
/// into end markers.
fn packed_with_overflow(
    prefix_width: usize,
    total_w: usize,
    active_w: usize,
    tab_count: usize,
    active: usize,
) -> LineLayout {
    let before = active;
    let after = tab_count - 1 - active;
    let inactive_w = INACTIVE_MIN;

    let (visible_left, visible_right) = grow(before, after, inactive_w, active_w, total_w, 0, 0);
    let left_hidden = before - visible_left;
    let right_hidden = after - visible_right;

    debug_assert_eq!(
        (visible_left + 1 + visible_right) + left_hidden + right_hidden,
        tab_count,
        "every tab is either visible or collapsed into a marker"
    );

    // Decide which end markers to draw. The active block always fits
    // (`active_w <= total_w`); the markers share whatever columns remain. The
    // two ends are treated symmetrically (see `marker_fit`) so a narrow bar
    // never suppresses one end while keeping the other — an asymmetric drop
    // would understate the hidden count and mislead hit-test consumers.
    let content_w = (visible_left + visible_right) * inactive_w + active_w;
    let slack = total_w.saturating_sub(content_w);
    let left_w = if left_hidden > 0 {
        display_width(&left_marker(left_hidden))
    } else {
        0
    };
    let right_w = if right_hidden > 0 {
        display_width(&right_marker(right_hidden))
    } else {
        0
    };
    let (show_left, show_right) = marker_fit(left_hidden, right_hidden, left_w, right_w, slack);

    let left = show_left.then(|| Overflow {
        hidden: left_hidden,
        start: prefix_width,
        text: left_marker(left_hidden),
    });

    let tabs_start = prefix_width + if show_left { left_w } else { 0 };

    let tabs: Vec<TabHit> = (active - visible_left..=active + visible_right)
        .scan(tabs_start, |col, position| {
            let width = if position == active {
                active_w
            } else {
                inactive_w
            };
            let hit = TabHit {
                position,
                start: *col,
                width,
                active: position == active,
            };
            *col += width;
            Some(hit)
        })
        .collect();

    let right_start = tabs.last().map_or(tabs_start, |tab| tab.start + tab.width);
    let right = show_right.then(|| Overflow {
        hidden: right_hidden,
        start: right_start,
        text: right_marker(right_hidden),
    });

    LineLayout { tabs, left, right }
}

/// Choose which overflow markers fit in `slack` columns, treating both ends
/// symmetrically. Prefer showing both; when only one fits, surface the side
/// hiding more tabs (ties → left, reading order) so the larger hidden count is
/// never the one that gets dropped.
fn marker_fit(
    left_hidden: usize,
    right_hidden: usize,
    left_w: usize,
    right_w: usize,
    slack: usize,
) -> (bool, bool) {
    let want_left = left_hidden > 0;
    let want_right = right_hidden > 0;
    if want_left && want_right && left_w + right_w <= slack {
        return (true, true);
    }
    let left_fits = want_left && left_w <= slack;
    let right_fits = want_right && right_w <= slack;
    if left_fits && right_fits {
        // Each fits alone but not together: keep the marker standing in for more.
        return (left_hidden >= right_hidden, left_hidden < right_hidden);
    }
    (left_fits, right_fits)
}

/// Greedily grow the visible window outward from the active tab, always adding
/// to the side with fewer shown tabs (so the active stays centered), as long as
/// the next tab plus the markers for whatever stays hidden still fit.
fn grow(
    before: usize,
    after: usize,
    inactive_w: usize,
    active_w: usize,
    total_w: usize,
    visible_left: usize,
    visible_right: usize,
) -> (usize, usize) {
    let fits = |left: usize, right: usize| {
        let markers = (if before - left > 0 {
            display_width(&left_marker(before - left))
        } else {
            0
        }) + (if after - right > 0 {
            display_width(&right_marker(after - right))
        } else {
            0
        });
        (left + right) * inactive_w + active_w + markers <= total_w
    };
    let can_left = visible_left < before && fits(visible_left + 1, visible_right);
    let can_right = visible_right < after && fits(visible_left, visible_right + 1);
    if !can_left && !can_right {
        return (visible_left, visible_right);
    }
    if can_left && (!can_right || visible_left <= visible_right) {
        return grow(
            before,
            after,
            inactive_w,
            active_w,
            total_w,
            visible_left + 1,
            visible_right,
        );
    }
    grow(
        before,
        after,
        inactive_w,
        active_w,
        total_w,
        visible_left,
        visible_right + 1,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn within_bounds(layout: &LineLayout, cols: usize) -> bool {
        let marker_in_bounds = |marker: &Option<Overflow>| {
            marker
                .as_ref()
                .is_none_or(|o| o.start + display_width(&o.text) <= cols)
        };
        layout.tabs.iter().all(|tab| tab.start + tab.width <= cols)
            && marker_in_bounds(&layout.left)
            && marker_in_bounds(&layout.right)
    }

    fn ordered_non_overlapping(layout: &LineLayout) -> bool {
        // tabs do not overlap each other; and, when present, the left marker
        // sits before the first tab and the right marker after the last.
        let tabs_ok = layout
            .tabs
            .windows(2)
            .all(|w| w[0].start + w[0].width <= w[1].start);
        let left_ok = match (&layout.left, layout.tabs.first()) {
            (Some(marker), Some(first)) => {
                marker.start + display_width(&marker.text) <= first.start
            }
            _ => true,
        };
        let right_ok = match (&layout.right, layout.tabs.last()) {
            (Some(marker), Some(last)) => last.start + last.width <= marker.start,
            _ => true,
        };
        tabs_ok && left_ok && right_ok
    }

    fn hidden(side: &Option<Overflow>) -> usize {
        side.as_ref().map_or(0, |o| o.hidden)
    }

    fn active_margins(layout: &LineLayout, cols: usize, prefix: usize) -> Option<(usize, usize)> {
        layout
            .tabs
            .iter()
            .find(|t| t.active)
            .map(|a| (a.start - prefix, cols - (a.start + a.width)))
    }

    #[test]
    fn active_clamped_up_to_minimum() {
        // Requesting 8 (< 16) yields a 16-column active block.
        assert_eq!(
            pack(120, 0, 8, 1, 0).tabs.first().map(|t| t.width),
            Some(ACTIVE_MIN)
        );
    }

    #[test]
    fn active_clamped_down_to_maximum() {
        // Requesting 40 (> 28) yields a 28-column active block.
        assert_eq!(
            pack(120, 0, 40, 1, 0).tabs.first().map(|t| t.width),
            Some(ACTIVE_MAX)
        );
    }

    #[test]
    fn inactive_blocks_keep_a_two_column_floor() {
        // Many tabs in a narrow bar: every shown inactive block is >= 2 wide.
        let layout = pack(40, 0, 16, 12, 0);
        assert!(layout
            .tabs
            .iter()
            .filter(|t| !t.active)
            .all(|t| t.width >= INACTIVE_MIN));
    }

    #[test]
    fn packed_width_never_exceeds_cols() {
        // The remainder splits across inactives without the row exceeding cols.
        let layout = pack(100, 0, 20, 6, 2);
        assert!(within_bounds(&layout, 100));
        assert_eq!(layout.tabs.len(), 6);
    }

    #[test]
    fn active_block_is_centered_for_odd_tab_count() {
        let layout = pack(120, 0, 20, 3, 1);
        let margins = active_margins(&layout, 120, 0);
        assert!(
            matches!(margins, Some((l, r)) if l.abs_diff(r) <= 1),
            "margins: {margins:?}"
        );
    }

    #[test]
    fn active_block_is_centered_for_even_tab_count() {
        let layout = pack(120, 0, 20, 4, 1);
        let margins = active_margins(&layout, 120, 0);
        assert!(
            matches!(margins, Some((l, r)) if l.abs_diff(r) <= 1),
            "margins: {margins:?}"
        );
    }

    #[test]
    fn no_overflow_markers_when_every_tab_fits() {
        let layout = pack(120, 0, 20, 4, 1);
        assert!(layout.left.is_none() && layout.right.is_none());
        assert_eq!(layout.tabs.len(), 4);
    }

    #[test]
    fn right_overflow_marks_the_tail_when_active_is_first() {
        let layout = pack(40, 0, 16, 20, 0);
        assert!(
            layout.left.is_none(),
            "no left marker when active is the first tab"
        );
        assert!(
            hidden(&layout.right) >= 1,
            "tail tabs collapse on the right"
        );
        // visible tabs start at the active (position 0) and stay contiguous.
        assert_eq!(layout.tabs.first().map(|t| t.position), Some(0));
    }

    #[test]
    fn left_overflow_marks_the_head_when_active_is_last() {
        let layout = pack(40, 0, 16, 20, 19);
        assert!(
            layout.right.is_none(),
            "no right marker when active is the last tab"
        );
        assert!(hidden(&layout.left) >= 1, "head tabs collapse on the left");
        assert_eq!(layout.tabs.last().map(|t| t.position), Some(19));
    }

    #[test]
    fn both_ends_overflow_and_counts_sum_to_the_hidden_total() {
        let layout = pack(40, 0, 16, 20, 10);
        assert!(
            hidden(&layout.left) >= 1 && hidden(&layout.right) >= 1,
            "both ends collapse"
        );
        // conservation: every tab is visible or collapsed into exactly one marker.
        assert_eq!(
            layout.tabs.len() + hidden(&layout.left) + hidden(&layout.right),
            20
        );
    }

    #[test]
    fn overflow_surfaces_the_larger_hidden_side_when_only_one_marker_fits() {
        // 23 cols only hold the active block (16) plus one marker, not both, yet
        // both sides hide tabs (10 left, 9 right). The larger-count side wins —
        // never an arbitrary end — and nothing spills past the bar.
        let layout = pack(23, 0, 16, 20, 10);
        assert_eq!(layout.left.as_ref().map(|o| o.hidden), Some(10));
        assert!(layout.right.is_none(), "the smaller (right) side yields");
        assert!(within_bounds(&layout, 23));
    }

    #[test]
    fn tab_ranges_are_ordered_in_bounds_and_contiguous() {
        // prefix_width 4 exercises the leading offset.
        let layout = pack(80, 4, 20, 8, 3);
        assert!(within_bounds(&layout, 80));
        assert!(ordered_non_overlapping(&layout));
        assert!(
            layout.tabs.iter().all(|t| t.start >= 4),
            "every span starts after the prefix"
        );
        let positions: Vec<_> = layout.tabs.iter().map(|t| t.position).collect();
        assert!(
            positions.windows(2).all(|w| w[0] + 1 == w[1]),
            "contiguous positions: {positions:?}"
        );
    }

    #[test]
    fn overflow_marker_text_matches_its_display_width() {
        assert_eq!(display_width(&left_marker(3)), 5); // "← +3 "
        assert_eq!(display_width(&right_marker(12)), 6); // " +12 →"
    }

    #[test]
    fn invariants_hold_across_the_input_space() {
        // A deterministic sweep standing in for property testing: no panic
        // (every subtraction / clamp stays valid), spans stay ordered and in
        // bounds, and the active tab is always visible. The conservation law is
        // enforced by the debug_assert in `packed_with_overflow`, which runs in
        // these (debug) test builds.
        for cols in (0..=160).step_by(3) {
            for tab_count in 1..=40 {
                for active in 0..tab_count {
                    let layout = pack(cols, 0, 20, tab_count, active);
                    assert!(
                        within_bounds(&layout, cols),
                        "bounds: cols={cols} n={tab_count} a={active}"
                    );
                    assert!(
                        ordered_non_overlapping(&layout),
                        "order: cols={cols} n={tab_count} a={active}"
                    );
                    let has_active = layout.tabs.iter().any(|t| t.active && t.position == active);
                    assert!(
                        has_active || cols == 0,
                        "active visible unless the bar is empty: cols={cols} n={tab_count} a={active}"
                    );
                }
            }
        }
    }
}

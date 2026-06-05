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

/// Which way a dragged tab travels, in `zellij`-free terms. Drag-and-drop
/// reorder (#10) maps this to `Direction::Left` / `Direction::Right` at the
/// host-calling site, so this layout module stays free of `zellij_tile` types
/// (like the rest of `line` / `minimap` / `paint`, it is native-testable).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shift {
    Left,
    Right,
}

/// How the all-fit tab row is anchored horizontally (config key `align`).
///
/// Governs **only** the branch where every tab fits: `Center` re-centers the
/// active block on each focus change, so the whole strip slides; `Left` pins the
/// row's left edge at column 0, removing the whole-strip slide. `Left` does *not*
/// freeze every tab's column: the active block is still drawn wider than the
/// inactives, so the tabs after it shift right as focus crosses them — only the
/// leftmost tab is truly pinned. When tabs overflow, the layout always follows
/// the active tab regardless of this — see [`pack`] — because the active block
/// must stay on screen to be usable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alignment {
    /// Pin the row's left edge at column 0; the strip no longer slides as a
    /// whole on focus change (tabs after the wider active block still reflow).
    Left,
    /// Center the active block; the strip slides to keep it centered.
    Center,
}

impl std::str::FromStr for Alignment {
    type Err = ();

    /// `"left"` / `"center"` (exact match); any other value is an error so the
    /// config parser falls back to the documented default rather than panicking.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "left" => Ok(Self::Left),
            "center" => Ok(Self::Center),
            _ => Err(()),
        }
    }
}

/// Display width of a string in terminal cells — icons and CJK count as their
/// real width, not their `char` count — via the Unicode width tables.
pub fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// The 0-based `position` of the visible tab whose drawn span `[start, start +
/// width)` contains `column`, or `None` when the column misses every block (an
/// overflow marker, an inter-block gap, or trailing padding). This is the exact
/// hit-test that both *grabs* a tab for drag-and-drop (#10) and underlies
/// click-to-switch (#8); keeping it column-precise means a stray click or grab
/// is a no-op, never a wrong tab.
pub fn position_at_column(tabs: &[TabHit], column: usize) -> Option<usize> {
    tabs.iter()
        .find(|tab| (tab.start..tab.start + tab.width).contains(&column))
        .map(|tab| tab.position)
}

/// The 1-based tab index `switch_tab_to` expects for the visible tab drawn at
/// `column`, or `None` when the click missed every block (a no-op, never a
/// wrong-tab switch).
///
/// zellij's `switch_tab_to` is **1-indexed** while `TabInfo.position` is
/// 0-indexed, so the matched [`position_at_column`] is returned offset by one.
/// Keeping the `+ 1` conversion here — in one unit-tested pure function — pins
/// the off-by-one natively, rather than burying it at the host-calling click
/// site where no native test can reach it.
pub fn switch_target_at_column(tabs: &[TabHit], column: usize) -> Option<u32> {
    position_at_column(tabs, column).map(|position| position as u32 + 1)
}

/// Resolve a drag that grabbed the tab at 0-based `from` and released at
/// `release_col`: how many single-position steps, and which way, to move the
/// grabbed tab so it lands where it was dropped — or `None` when the gesture is
/// a no-op (released back on its own slot, or nothing is drawn to drop onto).
///
/// The drop column is clamped into the drawn strip by [`drop_position_at_column`]
/// (left of every block → first visible tab, right of every block → last,
/// otherwise the block under the column), so a drop never silently vanishes off
/// an edge. `zellij`'s `MoveTabByTabId` shifts a tab one neighbour per call, and
/// packed positions are contiguous, so the step count is exactly the absolute
/// distance between `from` and the drop position. Direction is `zellij`-free
/// (see [`Shift`]); the caller maps it to `Direction` and emits one move per
/// step.
///
/// `from` must be the position of a currently drawn tab. A `from` that is not
/// among `tabs` — a stale grab whose tab scrolled into the overflow, or the
/// layout repacked mid-drag — resolves to `None` rather than a delta against a
/// position no longer on screen, keeping the conservative "uncertain ⇒ no-op"
/// stance of the click hit-test (a stray gesture never moves the wrong tab).
pub fn drag_steps(tabs: &[TabHit], from: usize, release_col: usize) -> Option<(Shift, usize)> {
    tabs.iter().find(|tab| tab.position == from)?;
    let to = drop_position_at_column(tabs, release_col)?;
    let steps = to.abs_diff(from);
    let shift = if to > from { Shift::Right } else { Shift::Left };
    (steps > 0).then_some((shift, steps))
}

/// The 0-based `position` a release at `release_col` drops onto. Unlike
/// [`position_at_column`] (an exact grab hit that may miss), a drop is clamped
/// into the drawn strip: a column left of the first block targets the first
/// visible tab, one at or past the last block targets the last, and one inside
/// the contiguous run targets the block under it. `None` only when nothing is
/// drawn — so an in-strip drop always resolves to a sensible neighbour rather
/// than discarding the gesture.
fn drop_position_at_column(tabs: &[TabHit], release_col: usize) -> Option<usize> {
    let first = tabs.first()?;
    let last = tabs.last()?;
    if release_col < first.start {
        return Some(first.position);
    }
    if release_col >= last.start + last.width {
        return Some(last.position);
    }
    // Packed spans are contiguous between the ends, so the exact hit always
    // lands; the fallback only guards a non-contiguous layout defensively.
    position_at_column(tabs, release_col).or(Some(last.position))
}

fn left_marker(hidden: usize) -> String {
    format!("← +{hidden} ")
}

fn right_marker(hidden: usize) -> String {
    format!(" +{hidden} →")
}

/// Pack `tab_count` tabs into `cols` columns, anchoring the all-fit row per
/// `align` (centered active block, or left-pinned — see [`Alignment`]).
///
/// `prefix_width` reserves leading columns (e.g. a logo); `active_desired` is
/// the active block's requested width before the `16..=28` clamp. Inactive tabs
/// share the remainder evenly down to a 2-column floor, capped at the active
/// width so the active stays the prominent block and genuine slack remains to
/// position it. When even the floors do not all fit, the tabs farthest from the
/// active collapse into end markers and the window follows the active tab —
/// `align` does not apply, since the active must stay on screen.
pub fn pack(
    cols: usize,
    prefix_width: usize,
    active_desired: usize,
    tab_count: usize,
    active: usize,
    align: Alignment,
) -> LineLayout {
    let total_w = cols.saturating_sub(prefix_width);
    if tab_count == 0 || total_w == 0 {
        return LineLayout::default();
    }
    let active = active.min(tab_count - 1);
    let active_w = active_desired.clamp(ACTIVE_MIN, ACTIVE_MAX).min(total_w);

    if tab_count == 1 {
        let start = match align {
            Alignment::Left => prefix_width,
            Alignment::Center => prefix_width + (total_w - active_w) / 2,
        };
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
        packed_aligned(
            prefix_width,
            total_w,
            active_w,
            inactive_w,
            tab_count,
            active,
            align,
        )
    } else {
        packed_with_overflow(prefix_width, total_w, active_w, tab_count, active)
    }
}

/// Every tab fits: lay them out in order, anchoring the row per `align`. `Center`
/// slides the row so the active block is centered (clamped into the leftover
/// slack so nothing spills off an edge); `Left` pins the row's left edge at
/// column 0, so the strip no longer slides as a whole on a focus change (the
/// wider active block still pushes the tabs drawn after it to the right).
fn packed_aligned(
    prefix_width: usize,
    total_w: usize,
    active_w: usize,
    inactive_w: usize,
    tab_count: usize,
    active: usize,
    align: Alignment,
) -> LineLayout {
    let content = active_w + (tab_count - 1) * inactive_w;
    let slack = total_w - content;
    let row_start = match align {
        // Left-anchored: the row always starts at the prefix, so the left edge
        // stays put and the whole-strip slide is gone. (The wider active block
        // still reflows the tabs drawn after it; only the leftmost is pinned.)
        Alignment::Left => 0,
        // Active-centered: shift so the blocks before the active one end at the
        // bar's center; clamp into `0..=slack` so a far-left / far-right active
        // just butts against its edge instead of dragging tabs out of view.
        Alignment::Center => ((total_w - active_w) / 2)
            .saturating_sub(active * inactive_w)
            .min(slack),
    };

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
            pack(120, 0, 8, 1, 0, Alignment::Center)
                .tabs
                .first()
                .map(|t| t.width),
            Some(ACTIVE_MIN)
        );
    }

    #[test]
    fn active_clamped_down_to_maximum() {
        // Requesting 40 (> 28) yields a 28-column active block.
        assert_eq!(
            pack(120, 0, 40, 1, 0, Alignment::Center)
                .tabs
                .first()
                .map(|t| t.width),
            Some(ACTIVE_MAX)
        );
    }

    #[test]
    fn inactive_blocks_keep_a_two_column_floor() {
        // Many tabs in a narrow bar: every shown inactive block is >= 2 wide.
        let layout = pack(40, 0, 16, 12, 0, Alignment::Center);
        assert!(layout
            .tabs
            .iter()
            .filter(|t| !t.active)
            .all(|t| t.width >= INACTIVE_MIN));
    }

    #[test]
    fn packed_width_never_exceeds_cols() {
        // The remainder splits across inactives without the row exceeding cols.
        let layout = pack(100, 0, 20, 6, 2, Alignment::Center);
        assert!(within_bounds(&layout, 100));
        assert_eq!(layout.tabs.len(), 6);
    }

    #[test]
    fn active_block_is_centered_for_odd_tab_count() {
        let layout = pack(120, 0, 20, 3, 1, Alignment::Center);
        let margins = active_margins(&layout, 120, 0);
        assert!(
            matches!(margins, Some((l, r)) if l.abs_diff(r) <= 1),
            "margins: {margins:?}"
        );
    }

    #[test]
    fn active_block_is_centered_for_even_tab_count() {
        let layout = pack(120, 0, 20, 4, 1, Alignment::Center);
        let margins = active_margins(&layout, 120, 0);
        assert!(
            matches!(margins, Some((l, r)) if l.abs_diff(r) <= 1),
            "margins: {margins:?}"
        );
    }

    #[test]
    fn no_overflow_markers_when_every_tab_fits() {
        let layout = pack(120, 0, 20, 4, 1, Alignment::Center);
        assert!(layout.left.is_none() && layout.right.is_none());
        assert_eq!(layout.tabs.len(), 4);
    }

    fn span_of(layout: &LineLayout, position: usize) -> Option<(usize, usize)> {
        layout
            .tabs
            .iter()
            .find(|t| t.position == position)
            .map(|t| (t.start, t.width))
    }

    #[test]
    fn left_align_pins_the_left_edge_across_focus_changes() {
        // Non-degenerate widths so the test means something: active_w = 24,
        // inactive_w = (120 - 24) / 7 = 13, so the active block is genuinely
        // wider than an inactive one. Under `Left` the row's left edge is pinned
        // at column 0 for *every* focus position — the whole-strip slide that
        // `Center` does is gone. This is the regression this option exists to
        // prevent; the earlier `active_w == inactive_w` form passed trivially
        // because nothing could reflow.
        let left_edge = |active| pack(120, 0, 24, 8, active, Alignment::Left).tabs[0].start;
        assert!(
            (0..8).all(|active| left_edge(active) == 0),
            "left edge pinned at 0 for every focus: {:?}",
            (0..8).map(left_edge).collect::<Vec<_>>()
        );
    }

    #[test]
    fn left_align_still_reflows_tabs_after_the_active() {
        // Honesty test: `Left` pins the *left edge*, not every tab's column. The
        // active block is wider than the inactives, so a tab drawn after it shifts
        // when focus crosses it. Position 3 sits right of active 2 (so the wide
        // block precedes it) but left of active 5 (so it does not) — its start
        // differs between the two. Documents the limitation so a future change
        // can't quietly over-promise full rigidity.
        let active_left = pack(120, 0, 24, 8, 2, Alignment::Left);
        let active_right = pack(120, 0, 24, 8, 5, Alignment::Left);
        assert_ne!(
            span_of(&active_left, 3),
            span_of(&active_right, 3),
            "a tab after the active reflows as the wide block moves past it"
        );
    }

    #[test]
    fn center_align_slides_the_left_edge_when_the_active_changes() {
        // The contrast to `Left`, with the same non-degenerate widths: `Center`
        // re-centers the active block, so the row's left edge shifts with focus.
        // If this ever stopped differing, `Center` would have silently collapsed
        // into `Left`.
        let focus_low = pack(120, 0, 24, 8, 0, Alignment::Center);
        let focus_high = pack(120, 0, 24, 8, 7, Alignment::Center);
        assert_ne!(
            focus_low.tabs[0].start, focus_high.tabs[0].start,
            "the centered row's left edge slides when the active tab changes"
        );
    }

    #[test]
    fn left_align_single_tab_anchors_at_the_prefix() {
        // The lone-tab fast path honors `align` too: `Left` starts it at the
        // prefix instead of centering it.
        let layout = pack(120, 4, 20, 1, 0, Alignment::Left);
        assert_eq!(span_of(&layout, 0), Some((4, 20)));
    }

    #[test]
    fn right_overflow_marks_the_tail_when_active_is_first() {
        let layout = pack(40, 0, 16, 20, 0, Alignment::Center);
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
        let layout = pack(40, 0, 16, 20, 19, Alignment::Center);
        assert!(
            layout.right.is_none(),
            "no right marker when active is the last tab"
        );
        assert!(hidden(&layout.left) >= 1, "head tabs collapse on the left");
        assert_eq!(layout.tabs.last().map(|t| t.position), Some(19));
    }

    #[test]
    fn both_ends_overflow_and_counts_sum_to_the_hidden_total() {
        let layout = pack(40, 0, 16, 20, 10, Alignment::Center);
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
        let layout = pack(23, 0, 16, 20, 10, Alignment::Center);
        assert_eq!(layout.left.as_ref().map(|o| o.hidden), Some(10));
        assert!(layout.right.is_none(), "the smaller (right) side yields");
        assert!(within_bounds(&layout, 23));
    }

    #[test]
    fn tab_ranges_are_ordered_in_bounds_and_contiguous() {
        // prefix_width 4 exercises the leading offset.
        let layout = pack(80, 4, 20, 8, 3, Alignment::Center);
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
        // these (debug) test builds. Both alignments are swept — the row anchor
        // must never violate these invariants, in either the all-fit or the
        // overflow branch.
        for align in [Alignment::Left, Alignment::Center] {
            for cols in (0..=160).step_by(3) {
                for tab_count in 1..=40 {
                    for active in 0..tab_count {
                        let layout = pack(cols, 0, 20, tab_count, active, align);
                        assert!(
                            within_bounds(&layout, cols),
                            "bounds: align={align:?} cols={cols} n={tab_count} a={active}"
                        );
                        assert!(
                            ordered_non_overlapping(&layout),
                            "order: align={align:?} cols={cols} n={tab_count} a={active}"
                        );
                        let has_active =
                            layout.tabs.iter().any(|t| t.active && t.position == active);
                        assert!(
                            has_active || cols == 0,
                            "active visible unless empty: align={align:?} cols={cols} n={tab_count} a={active}"
                        );
                    }
                }
            }
        }
    }

    // ---- switch_target_at_column (click hit-test, #8) --------------------

    fn hit(position: usize, start: usize, width: usize, active: bool) -> TabHit {
        TabHit {
            position,
            start,
            width,
            active,
        }
    }

    #[test]
    fn click_inside_a_block_resolves_to_its_one_based_index() {
        // position 0 spans [0, 2); position 1 spans [2, 4). switch_tab_to is
        // 1-indexed, so position 0 → 1 and position 1 → 2. (A `+ 0` regression
        // would return 0 / 1 here and fail.)
        let tabs = vec![hit(0, 0, 2, false), hit(1, 2, 2, true)];
        assert_eq!(switch_target_at_column(&tabs, 0), Some(1));
        assert_eq!(switch_target_at_column(&tabs, 2), Some(2));
    }

    #[test]
    fn click_covers_first_and_last_column_of_a_block() {
        // position 2 spans columns 4, 5, 6 (start 4, width 3). Both edges are
        // inside; the column one past the end (7) belongs to no block.
        let tabs = vec![hit(2, 4, 3, true)];
        assert_eq!(switch_target_at_column(&tabs, 4), Some(3), "first column");
        assert_eq!(switch_target_at_column(&tabs, 6), Some(3), "last column");
        assert_eq!(switch_target_at_column(&tabs, 7), None, "one past the end");
    }

    #[test]
    fn click_left_of_the_first_block_is_a_no_op() {
        // A left overflow marker occupies columns 0..5; the first tab starts at
        // 5. Every column the marker covers resolves to nothing.
        let tabs = vec![hit(3, 5, 2, true), hit(4, 7, 2, false)];
        for column in 0..5 {
            assert_eq!(
                switch_target_at_column(&tabs, column),
                None,
                "col {column} is left of every block"
            );
        }
        assert_eq!(switch_target_at_column(&tabs, 5), Some(4));
    }

    #[test]
    fn click_in_a_gap_between_blocks_is_a_no_op() {
        // Non-contiguous blocks (a marker or padding sits between them): the gap
        // columns 2, 3, 4 resolve to neither tab.
        let tabs = vec![hit(0, 0, 2, true), hit(1, 5, 2, false)];
        assert_eq!(switch_target_at_column(&tabs, 1), Some(1));
        assert_eq!(switch_target_at_column(&tabs, 2), None, "gap");
        assert_eq!(switch_target_at_column(&tabs, 4), None, "gap");
        assert_eq!(switch_target_at_column(&tabs, 5), Some(2));
    }

    #[test]
    fn click_on_an_empty_layout_is_a_no_op() {
        assert_eq!(switch_target_at_column(&[], 0), None);
        assert_eq!(switch_target_at_column(&[], 7), None);
    }

    #[test]
    fn hit_test_covers_exactly_the_drawn_tab_columns() {
        // Sweep a real packed layout (12 tabs in 40 cols → overflow markers at
        // both ends): every column a tab is drawn on resolves to that tab's
        // 1-based index, and every column no tab covers (markers / gaps /
        // padding) resolves to None — so a stray click is never a wrong switch.
        let cols = 40;
        let layout = pack(cols, 0, 16, 12, 5, Alignment::Center);
        for tab in &layout.tabs {
            for column in tab.start..tab.start + tab.width {
                assert_eq!(
                    switch_target_at_column(&layout.tabs, column),
                    Some(tab.position as u32 + 1),
                    "column {column} is drawn on position {}",
                    tab.position
                );
            }
        }
        let covered = |c: usize| {
            layout
                .tabs
                .iter()
                .any(|t| (t.start..t.start + t.width).contains(&c))
        };
        for column in (0..cols).filter(|c| !covered(*c)) {
            assert_eq!(
                switch_target_at_column(&layout.tabs, column),
                None,
                "uncovered column {column}"
            );
        }
    }

    // ---- position_at_column (grab hit-test, #10) -------------------------

    #[test]
    fn position_at_column_returns_zero_based_position_not_the_switch_index() {
        // position 0 spans [0, 2); position 1 spans [2, 4). The grab hit-test
        // yields the raw 0-based position, whereas switch_target adds one for
        // the 1-indexed `switch_tab_to`.
        let tabs = vec![hit(0, 0, 2, false), hit(1, 2, 2, true)];
        assert_eq!(position_at_column(&tabs, 0), Some(0));
        assert_eq!(position_at_column(&tabs, 2), Some(1));
        assert_eq!(
            switch_target_at_column(&tabs, 0),
            Some(1),
            "switch = pos + 1"
        );
        assert_eq!(
            position_at_column(&tabs, 4),
            None,
            "one past the last block"
        );
        assert_eq!(position_at_column(&[], 0), None, "empty layout");
    }

    // ---- drag_steps (drop resolution + delta, #10) -----------------------

    #[test]
    fn drag_to_a_later_block_steps_right_by_the_position_delta() {
        // positions 0..5, each width 2, contiguous: p1 spans [2,4), p3 [6,8).
        let tabs: Vec<TabHit> = (0..5).map(|p| hit(p, p * 2, 2, p == 2)).collect();
        // grabbed position 1, released inside position 3's span → move right 2.
        assert_eq!(drag_steps(&tabs, 1, 6), Some((Shift::Right, 2)));
        assert_eq!(
            drag_steps(&tabs, 1, 7),
            Some((Shift::Right, 2)),
            "either column of p3"
        );
    }

    #[test]
    fn drag_to_an_earlier_block_steps_left_by_the_position_delta() {
        let tabs: Vec<TabHit> = (0..5).map(|p| hit(p, p * 2, 2, p == 2)).collect();
        // grabbed position 3, released inside position 1's span → move left 2.
        assert_eq!(drag_steps(&tabs, 3, 2), Some((Shift::Left, 2)));
    }

    #[test]
    fn drag_released_on_its_own_slot_is_a_no_op() {
        // A click-without-move (grab and release on the same block) must never
        // emit a move action — both columns of position 2's span resolve to None.
        let tabs: Vec<TabHit> = (0..5).map(|p| hit(p, p * 2, 2, p == 2)).collect();
        assert_eq!(
            drag_steps(&tabs, 2, 4),
            None,
            "first column of the grabbed slot"
        );
        assert_eq!(
            drag_steps(&tabs, 2, 5),
            None,
            "last column of the grabbed slot"
        );
    }

    #[test]
    fn drop_past_the_last_block_clamps_to_the_last_visible_tab() {
        // positions 0..5; last block (p4) spans [8,10). A release at or beyond
        // column 10 (slack / right overflow marker) lands the tab at the end.
        let tabs: Vec<TabHit> = (0..5).map(|p| hit(p, p * 2, 2, p == 2)).collect();
        assert_eq!(
            drag_steps(&tabs, 1, 10),
            Some((Shift::Right, 3)),
            "at the end boundary"
        );
        assert_eq!(
            drag_steps(&tabs, 1, 99),
            Some((Shift::Right, 3)),
            "far past the end"
        );
    }

    #[test]
    fn drop_before_the_first_block_clamps_to_the_first_visible_tab() {
        // First block does not start at column 0 (a left overflow marker / row
        // offset occupies [0,5)); a release in that region targets the first
        // visible tab, position 2.
        let tabs = vec![hit(2, 5, 2, true), hit(3, 7, 2, false)];
        assert_eq!(drag_steps(&tabs, 3, 0), Some((Shift::Left, 1)), "far left");
        assert_eq!(
            drag_steps(&tabs, 3, 4),
            Some((Shift::Left, 1)),
            "just left of the first block"
        );
    }

    #[test]
    fn drag_on_an_empty_or_single_tab_layout_never_moves() {
        assert_eq!(drag_steps(&[], 0, 5), None, "nothing drawn");
        let one = vec![hit(0, 4, 2, true)];
        // A lone tab has only one slot, so every drop resolves back onto it.
        assert_eq!(drag_steps(&one, 0, 4), None, "onto itself");
        assert_eq!(drag_steps(&one, 0, 0), None, "clamped left to itself");
        assert_eq!(drag_steps(&one, 0, 99), None, "clamped right to itself");
    }

    #[test]
    fn drag_with_a_grabbed_position_not_currently_drawn_is_a_no_op() {
        // Only positions 5,6,7 are visible (the rest collapsed into overflow). A
        // grab whose recorded position is no longer on screen — a stale drag, or
        // the grabbed tab scrolled into the overflow before release — resolves to
        // no move, never a delta against an off-screen position.
        let tabs = vec![hit(5, 0, 2, false), hit(6, 2, 2, true), hit(7, 4, 2, false)];
        assert_eq!(
            drag_steps(&tabs, 2, 4),
            None,
            "grabbed position 2 is hidden"
        );
        assert_eq!(
            drag_steps(&tabs, 9, 0),
            None,
            "grabbed position 9 is hidden"
        );
        // A grab on a position that IS drawn still resolves normally.
        assert_eq!(
            drag_steps(&tabs, 5, 4),
            Some((Shift::Right, 2)),
            "visible grab works"
        );
    }

    #[test]
    fn drag_steps_matches_the_clamped_drop_across_every_grab_and_release() {
        // A deterministic sweep over a hand-built contiguous strip: positions
        // 0..5, each width 3, drawn from column 0 (p spans [3p, 3p+3)), last
        // block ends at 18. The expected drop position is derived independently
        // of `drop_position_at_column` — `release_col / 3`, clamped to the last
        // position past the end — so the assertion genuinely cross-checks the
        // implementation rather than restating it.
        let spans: Vec<TabHit> = (0..6).map(|p| hit(p, p * 3, 3, p == 2)).collect();
        for from in 0usize..6 {
            for release_col in 0usize..24 {
                let to = if release_col >= 18 {
                    5
                } else {
                    release_col / 3
                };
                let expected = (to != from).then(|| {
                    let shift = if to > from { Shift::Right } else { Shift::Left };
                    (shift, to.abs_diff(from))
                });
                assert_eq!(
                    drag_steps(&spans, from, release_col),
                    expected,
                    "from {from}, release_col {release_col} (drop position {to})"
                );
            }
        }
    }
}

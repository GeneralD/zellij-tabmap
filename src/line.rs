//! Pure tab-bar line packing: turn a tab list plus a column budget into per-tab
//! column spans, keeping the active tab centered and collapsing the tabs that do
//! not fit into `← +N` / `+N →` overflow markers at the ends.
//!
//! This is layout math only — no `zellij_tile` calls and no rendering — so it
//! runs and is unit-tested on the native host like the rest of the renderer
//! (`minimap` / `paint` / `projection`). The [`TabHit`] spans it produces are
//! the input for click-to-switch (#8), so each span reflects exactly where a
//! block is drawn, measured in display columns (see [`display_width`]) rather
//! than `char` count.

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

/// The inline new-tab `+` button's drawn column span (#76). Like [`TabHit`] but
/// carries no tab identity — a click here opens a *new* tab rather than
/// switching to an existing one — so it records only the span the same
/// column-range hit-test the tabs use can route against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ButtonHit {
    /// 0-based start column in the bar.
    pub start: usize,
    /// Drawn width in display columns.
    pub width: usize,
}

impl ButtonHit {
    /// Whether `column` lands on the button's drawn span. Column-range only:
    /// the button is a full-height fill, so it owns its columns on *every* row
    /// — there is no per-row trailing space to mis-trigger on (#76).
    pub fn contains(&self, column: usize) -> bool {
        let end = self.start.saturating_add(self.width);
        (self.start..end).contains(&column)
    }
}

/// One tab's close-button cell — the single top-right cell whose left-click
/// closes that tab (#86). Unlike [`TabHit`] / [`ButtonHit`], which own a whole
/// column range, the close target is **one exact cell**: it needs row precision
/// so a click lower in the same column still switches to (or focuses a pane of)
/// the tab rather than closing it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CloseHit {
    /// 0-based tab position to close (zellij `TabInfo.position`), passed straight
    /// to `close_tab_with_index`.
    pub position: usize,
    /// 0-based row of the close cell in the bar (the top text row, `0`).
    pub row: usize,
    /// 0-based column of the close cell in the bar (the block's right edge).
    pub column: usize,
}

impl CloseHit {
    /// Whether a click at (`row`, `column`) lands exactly on the close cell.
    pub fn contains(&self, row: usize, column: usize) -> bool {
        self.row == row && self.column == column
    }
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

/// The packed bar: visible tabs left-to-right plus optional end markers, and
/// the optional inline new-tab `+` button at the end of the strip (#76).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct LineLayout {
    /// Visible tabs, ordered left-to-right, contiguous around the active tab.
    pub tabs: Vec<TabHit>,
    pub left: Option<Overflow>,
    pub right: Option<Overflow>,
    /// The new-tab `+` button span, set only by [`pack_with_button`] (plain
    /// [`pack`] always leaves it `None`). When `Some`, it sits one `gap` past
    /// the last visible tab and any right marker is shifted beyond it (#76).
    pub button: Option<ButtonHit>,
}

/// How the all-fit tab row is anchored horizontally (config key `align`).
///
/// Governs **only** the branch where every tab fits: `Center` re-centers the
/// active block on each focus change, so the whole strip slides; `Left` pins the
/// row's left edge at the start of the tab area — `prefix_width` (column 0 when
/// no prefix is reserved) — removing the whole-strip slide. `Left` does *not*
/// freeze every tab's column: the active block is still drawn wider than the
/// inactives, so the tabs after it shift right as focus crosses them — only the
/// leftmost tab is truly pinned. When tabs overflow, the layout always follows
/// the active tab regardless of this — see [`pack`] — because the active block
/// must stay on screen to be usable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alignment {
    /// Pin the row's left edge at the start of the tab area (just after any
    /// reserved prefix); the strip no longer slides as a whole on focus change
    /// (tabs after the wider active block still reflow).
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
/// hit-test underlying click-to-switch (#8) and click-to-focus (#74); keeping it
/// column-precise means a stray click is a no-op, never a wrong tab.
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
/// position it. `gap` columns of empty space are kept between every adjacent
/// pair of blocks (config key `tab_gap`); that budget is reserved *before* the
/// inactives share the remainder, so separating the screens never pushes the row
/// past the bar. The gap is drawn as the cleared pane background — no separator
/// glyph — so [`crate::paint::compose`] needs no change. When even the floors
/// plus gaps do not all fit, the tabs farthest from the active collapse into end
/// markers and the window follows the active tab — `align` does not apply, since
/// the active must stay on screen. No gap is inserted between a tab and an
/// overflow marker: the markers carry their own padding spaces.
pub fn pack(
    cols: usize,
    prefix_width: usize,
    active_desired: usize,
    tab_count: usize,
    active: usize,
    align: Alignment,
    gap: usize,
) -> LineLayout {
    let total_w = cols.saturating_sub(prefix_width);
    if tab_count == 0 || total_w == 0 {
        return LineLayout::default();
    }
    let active = active.min(tab_count - 1);
    let active_w = active_desired.clamp(ACTIVE_MIN, ACTIVE_MAX).min(total_w);
    // `gap` is unbounded config input (`tab_gap`); clamp it to the render
    // budget at entry, mirroring `active_w` above, so every downstream
    // `inactives * gap` / `inactive_w + gap` term stays in-bounds and a
    // pathological value just collapses the blocks instead of overflowing.
    let gap = gap.min(total_w);

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
            button: None,
        };
    }

    let inactives = tab_count - 1;
    // One gap sits between each adjacent pair of blocks (`inactives` gaps for
    // `tab_count` tabs); reserve that budget before the inactives share the
    // remainder, so separating the screens never pushes the row past `total_w`.
    let gaps = inactives * gap;
    // Cap at the active width (raised to the floor so the clamp bounds never
    // invert on a sub-2-column bar), then floor at 2 columns.
    let inactive_cap = active_w.max(INACTIVE_MIN);
    let inactive_w = (total_w.saturating_sub(active_w).saturating_sub(gaps) / inactives)
        .clamp(INACTIVE_MIN, inactive_cap);

    if active_w + inactives * inactive_w + gaps <= total_w {
        packed_aligned(
            prefix_width,
            total_w,
            active_w,
            inactive_w,
            gap,
            tab_count,
            active,
            align,
        )
    } else {
        packed_with_overflow(prefix_width, total_w, active_w, gap, tab_count, active)
    }
}

/// The width an inline new-tab `+` button would copy from `layout` (#76): an
/// **inactive** tab's width, so the button reads as just another slot at the end
/// of the strip rather than a cramped icon.
///
/// When no inactive tab is in view the source depends on *why*: a lone tab with
/// no overflow markers is a genuine single-tab bar, so the sole tab's width is
/// used (the "+" is its sibling); but when overflow markers are present the
/// inactive tabs are merely *hidden* — only the wide active block shows — so the
/// floor [`INACTIVE_MIN`] an overflowed inactive renders at is used instead of
/// the active width, which would size the button far too wide. An empty layout
/// also falls back to the floor.
fn button_slot_width(layout: &LineLayout) -> usize {
    if let Some(inactive) = layout.tabs.iter().find(|tab| !tab.active) {
        return inactive.width;
    }
    match layout.left.is_none() && layout.right.is_none() {
        true => layout.tabs.first().map_or(INACTIVE_MIN, |tab| tab.width),
        false => INACTIVE_MIN,
    }
}

/// Like [`pack`], but also reserves and records the inline new-tab `+` button
/// at the end of the strip (#76).
///
/// The button is sized to match the bar's **inactive tabs** (see
/// [`button_slot_width`]) so it reads as just another slot, not a cramped icon.
/// Reserving the button's span shrinks the budget the tabs repack into, which can
/// narrow the inactive tabs below a full-budget trial — so the width that makes
/// the "+" a true sibling is a *fixed point*: the largest button width whose
/// reduced-budget repack still renders its inactive tabs at least that wide. The
/// search runs from the widest the button could be (the full-budget trial)
/// downward; the [`INACTIVE_MIN`] floor satisfies it once the reserve fits, so a
/// button is dropped only when even that floor overflows the bar. The reserved
/// width plus one `gap` is held back from `cols` *before* packing, so the tabs
/// never pack into the button's span. The button is
/// then placed one `gap` past the last visible
/// tab — "just another slot at the end of the strip" — rather than pinned to the
/// far-right edge (a fill reads as part of the strip). On overflow the right
/// `+N →` marker, which `pack` butts against the last visible tab, is shifted
/// just past the button so the button stays **directly after the last visible
/// tab** and the marker sits beyond it (closer to the edge) — the placement
/// chosen in #76.
///
/// Returns the same [`LineLayout`] as `pack` with `button` set, or with
/// `button: None` (a plain `pack`) when `with_button` is false, or when even an
/// inactive-tab-sized reserve does not fit `cols` — a bar too narrow to host the
/// button drops it rather than overlapping the strip.
#[allow(clippy::too_many_arguments)]
pub fn pack_with_button(
    cols: usize,
    prefix_width: usize,
    active_desired: usize,
    tab_count: usize,
    active: usize,
    align: Alignment,
    gap: usize,
    with_button: bool,
) -> LineLayout {
    let plain = || {
        pack(
            cols,
            prefix_width,
            active_desired,
            tab_count,
            active,
            align,
            gap,
        )
    };
    if !with_button {
        return plain();
    }

    // Size the button like an inactive tab so the "+" is a sibling slot rather
    // than a fixed-width icon. The full-budget trial is the *widest* the button
    // could be; search downward from it for the largest width whose reduced-budget
    // repack still renders its inactive tabs at least that wide (the fixed point —
    // a wider reserve would shrink the tabs below the button, making the "+" look
    // bigger than the slots beside it). Saturating throughout: `gap` is unbounded
    // config input (`tab_gap`), so a pathological value collapses to "no room"
    // (button dropped) instead of overflowing — mirroring `pack`'s entry clamp.
    let max_button_width = button_slot_width(&plain());
    let solved = (INACTIVE_MIN..=max_button_width)
        .rev()
        .find_map(|button_width| {
            let reserve = button_width.saturating_add(gap);
            if cols <= prefix_width.saturating_add(reserve) {
                return None;
            }
            let layout = pack(
                cols - reserve,
                prefix_width,
                active_desired,
                tab_count,
                active,
                align,
                gap,
            );
            (button_slot_width(&layout) >= button_width).then_some((layout, button_width, reserve))
        });
    // No width fits — not even the INACTIVE_MIN floor: the bar is too narrow to
    // host the button, so drop it rather than overlap the strip.
    let Some((mut layout, button_width, reserve)) = solved else {
        return plain();
    };

    // Anchor the button one `gap` past the last visible tab (or at the prefix
    // when nothing is drawn). The strip ends by `cols - reserve` and
    // `reserve = button_width + gap`, so `last_tab_end + gap + button_width
    // <= cols` — the button always fits.
    let start = layout.tabs.last().map_or(prefix_width, |tab| {
        tab.start.saturating_add(tab.width).saturating_add(gap)
    });

    // Slide the right marker past the button. `pack` placed it butting the last
    // visible tab; shifting it by the whole `reserve` (`= gap + button_width`)
    // lands it exactly at the button's end, beyond it, and keeps it in bounds:
    // its end was `<= cols - reserve`, so after `+ reserve` it is `<= cols`.
    if let Some(marker) = layout.right.as_mut() {
        marker.start += reserve;
    }

    layout.button = Some(ButtonHit {
        start,
        width: button_width,
    });
    layout
}

/// Every tab fits: lay them out in order, anchoring the row per `align`. `Center`
/// slides the row so the active block is centered (clamped into the leftover
/// slack so nothing spills off an edge); `Left` zeroes the in-row offset so the
/// row begins right at `prefix_width` (column 0 when no prefix), and the strip no
/// longer slides as a whole on a focus change (the wider active block still
/// pushes the tabs drawn after it to the right).
// Eight distinct geometric inputs (widths, gap, count, focus, anchor) — each a
// genuinely independent layout quantity, not a sign this pure helper does too
// much. Bundling them into a struct is a separate refactor, not this gap change.
#[allow(clippy::too_many_arguments)]
fn packed_aligned(
    prefix_width: usize,
    total_w: usize,
    active_w: usize,
    inactive_w: usize,
    gap: usize,
    tab_count: usize,
    active: usize,
    align: Alignment,
) -> LineLayout {
    // `content` spans the blocks plus the `tab_count - 1` inter-block gaps.
    let content = active_w + (tab_count - 1) * (inactive_w + gap);
    let slack = total_w - content;
    let row_start = match align {
        // Left-anchored: the row always starts at the prefix, so the left edge
        // stays put and the whole-strip slide is gone. (The wider active block
        // still reflows the tabs drawn after it; only the leftmost is pinned.)
        Alignment::Left => 0,
        // Active-centered: shift so the blocks before the active one end at the
        // bar's center; clamp into `0..=slack` so a far-left / far-right active
        // just butts against its edge instead of dragging tabs out of view. Each
        // block before the active spans `inactive_w + gap`.
        Alignment::Center => ((total_w - active_w) / 2)
            .saturating_sub(active * (inactive_w + gap))
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
            // Advance past this block and the gap that follows it; the trailing
            // gap after the last block is harmless (no further hit is emitted).
            *col += width + gap;
            Some(hit)
        })
        .collect();

    LineLayout {
        tabs,
        left: None,
        right: None,
        button: None,
    }
}

/// Too many tabs to fit even at the floor width: grow a contiguous window
/// outward from the active tab, balancing the two sides, and collapse the rest
/// into end markers.
fn packed_with_overflow(
    prefix_width: usize,
    total_w: usize,
    active_w: usize,
    gap: usize,
    tab_count: usize,
    active: usize,
) -> LineLayout {
    let before = active;
    let after = tab_count - 1 - active;
    let inactive_w = INACTIVE_MIN;

    let (visible_left, visible_right) =
        grow(before, after, inactive_w, gap, active_w, total_w, 0, 0);
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
    // The visible window of `visible_left + 1 + visible_right` blocks carries
    // `visible_left + visible_right` inter-block gaps (none between a tab and an
    // overflow marker — markers pad themselves).
    let content_w = (visible_left + visible_right) * (inactive_w + gap) + active_w;
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
            *col += width + gap;
            Some(hit)
        })
        .collect();

    // `tab.start + tab.width` (not the scan's running `col`) excludes the
    // trailing gap, so the right marker butts directly against the last block.
    let right_start = tabs.last().map_or(tabs_start, |tab| tab.start + tab.width);
    let right = show_right.then(|| Overflow {
        hidden: right_hidden,
        start: right_start,
        text: right_marker(right_hidden),
    });

    LineLayout {
        tabs,
        left,
        right,
        button: None,
    }
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
// Eight inputs: the two side budgets, the per-block width, the gap, the active
// width, the column budget, and the two accumulating window counts the recursion
// carries — each independent, not a sign this tail-recursive helper does too much.
#[allow(clippy::too_many_arguments)]
fn grow(
    before: usize,
    after: usize,
    inactive_w: usize,
    gap: usize,
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
        // A window of `left + 1 + right` blocks holds `left + right` inter-block
        // gaps, so each shown inactive costs `inactive_w + gap`.
        (left + right) * (inactive_w + gap) + active_w + markers <= total_w
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
            gap,
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
        gap,
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
            && layout.button.is_none_or(|b| b.start + b.width <= cols)
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
        // The button (when present) sits after the last tab and, when a right
        // marker is drawn, before it — the #76 [tabs][button][marker] order.
        let button_after_tabs = match (&layout.button, layout.tabs.last()) {
            (Some(button), Some(last)) => last.start + last.width <= button.start,
            _ => true,
        };
        let button_before_right = match (&layout.button, &layout.right) {
            (Some(button), Some(marker)) => button.start + button.width <= marker.start,
            _ => true,
        };
        tabs_ok && left_ok && right_ok && button_after_tabs && button_before_right
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
            pack(120, 0, 8, 1, 0, Alignment::Center, 0)
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
            pack(120, 0, 40, 1, 0, Alignment::Center, 0)
                .tabs
                .first()
                .map(|t| t.width),
            Some(ACTIVE_MAX)
        );
    }

    #[test]
    fn inactive_blocks_keep_a_two_column_floor() {
        // Many tabs in a narrow bar: every shown inactive block is >= 2 wide.
        let layout = pack(40, 0, 16, 12, 0, Alignment::Center, 0);
        assert!(
            layout
                .tabs
                .iter()
                .filter(|t| !t.active)
                .all(|t| t.width >= INACTIVE_MIN)
        );
    }

    #[test]
    fn packed_width_never_exceeds_cols() {
        // The remainder splits across inactives without the row exceeding cols.
        let layout = pack(100, 0, 20, 6, 2, Alignment::Center, 0);
        assert!(within_bounds(&layout, 100));
        assert_eq!(layout.tabs.len(), 6);
    }

    #[test]
    fn active_block_is_centered_for_odd_tab_count() {
        let layout = pack(120, 0, 20, 3, 1, Alignment::Center, 0);
        let margins = active_margins(&layout, 120, 0);
        assert!(
            matches!(margins, Some((l, r)) if l.abs_diff(r) <= 1),
            "margins: {margins:?}"
        );
    }

    #[test]
    fn active_block_is_centered_for_even_tab_count() {
        let layout = pack(120, 0, 20, 4, 1, Alignment::Center, 0);
        let margins = active_margins(&layout, 120, 0);
        assert!(
            matches!(margins, Some((l, r)) if l.abs_diff(r) <= 1),
            "margins: {margins:?}"
        );
    }

    #[test]
    fn no_overflow_markers_when_every_tab_fits() {
        let layout = pack(120, 0, 20, 4, 1, Alignment::Center, 0);
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
        // prefix = 0 here, so the pinned left edge is absolute column 0.
        let left_edge = |active| pack(120, 0, 24, 8, active, Alignment::Left, 0).tabs[0].start;
        let edges: Vec<_> = (0..8).map(left_edge).collect();
        assert!(
            edges.iter().all(|&edge| edge == 0),
            "left edge pinned at 0 for every focus: {edges:?}"
        );
    }

    #[test]
    fn left_align_anchors_the_row_at_the_prefix_not_absolute_column_zero() {
        // With a reserved prefix the pinned edge is `prefix_width`, not absolute
        // column 0: `Left` zeroes the *in-row* offset, and tabs are laid out from
        // `prefix_width + 0`. Guards the doc claim against the general
        // `pack(.., prefix_width, ..)` API, not just the live `prefix == 0` caller.
        let prefix = 4;
        let left_edge = |active| pack(120, prefix, 24, 8, active, Alignment::Left, 0).tabs[0].start;
        let edges: Vec<_> = (0..8).map(left_edge).collect();
        assert!(
            edges.iter().all(|&edge| edge == prefix),
            "left edge pinned at the prefix ({prefix}) for every focus: {edges:?}"
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
        let active_left = pack(120, 0, 24, 8, 2, Alignment::Left, 0);
        let active_right = pack(120, 0, 24, 8, 5, Alignment::Left, 0);
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
        let focus_low = pack(120, 0, 24, 8, 0, Alignment::Center, 0);
        let focus_high = pack(120, 0, 24, 8, 7, Alignment::Center, 0);
        assert_ne!(
            focus_low.tabs[0].start, focus_high.tabs[0].start,
            "the centered row's left edge slides when the active tab changes"
        );
    }

    #[test]
    fn left_align_single_tab_anchors_at_the_prefix() {
        // The lone-tab fast path honors `align` too: `Left` starts it at the
        // prefix instead of centering it.
        let layout = pack(120, 4, 20, 1, 0, Alignment::Left, 0);
        assert_eq!(span_of(&layout, 0), Some((4, 20)));
    }

    #[test]
    fn right_overflow_marks_the_tail_when_active_is_first() {
        let layout = pack(40, 0, 16, 20, 0, Alignment::Center, 0);
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
        let layout = pack(40, 0, 16, 20, 19, Alignment::Center, 0);
        assert!(
            layout.right.is_none(),
            "no right marker when active is the last tab"
        );
        assert!(hidden(&layout.left) >= 1, "head tabs collapse on the left");
        assert_eq!(layout.tabs.last().map(|t| t.position), Some(19));
    }

    #[test]
    fn both_ends_overflow_and_counts_sum_to_the_hidden_total() {
        let layout = pack(40, 0, 16, 20, 10, Alignment::Center, 0);
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
        let layout = pack(23, 0, 16, 20, 10, Alignment::Center, 0);
        assert_eq!(layout.left.as_ref().map(|o| o.hidden), Some(10));
        assert!(layout.right.is_none(), "the smaller (right) side yields");
        assert!(within_bounds(&layout, 23));
    }

    #[test]
    fn tab_ranges_are_ordered_in_bounds_and_contiguous() {
        // prefix_width 4 exercises the leading offset.
        let layout = pack(80, 4, 20, 8, 3, Alignment::Center, 0);
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

    // ---- tab_gap (inter-block separation) --------------------------------

    #[test]
    fn gap_separates_every_adjacent_pair_in_the_all_fit_row() {
        // Four tabs that all fit with a 2-column gap between each pair. Every
        // adjacent pair is separated by exactly `gap` cleared columns — the
        // separation this option exists to produce — and nothing spills off the
        // bar despite the reserved gap budget.
        let gap = 2;
        let layout = pack(120, 0, 20, 4, 1, Alignment::Center, gap);
        assert!(
            layout.left.is_none() && layout.right.is_none(),
            "every tab fits"
        );
        assert_eq!(layout.tabs.len(), 4);
        let spans: Vec<_> = layout.tabs.iter().map(|t| (t.start, t.width)).collect();
        assert!(
            spans.windows(2).all(|w| w[1].0 == w[0].0 + w[0].1 + gap),
            "each adjacent pair separated by exactly {gap} columns: {spans:?}"
        );
        assert!(within_bounds(&layout, 120));
    }

    #[test]
    fn zero_gap_packs_blocks_flush() {
        // Regression guard: gap = 0 reproduces the v0.1.0 flush look — adjacent
        // blocks touch, with no cleared column between them.
        let layout = pack(120, 0, 20, 4, 1, Alignment::Center, 0);
        assert!(
            layout
                .tabs
                .windows(2)
                .all(|w| w[1].start == w[0].start + w[0].width),
            "blocks touch when gap is 0"
        );
    }

    #[test]
    fn gap_separates_visible_blocks_in_the_overflow_branch() {
        // Many tabs in a narrow bar force the overflow branch. With a gap the
        // visible window's blocks are still separated by exactly `gap`, the row
        // stays in bounds, and (when drawn) the right marker butts directly
        // against the last block — no gap precedes an overflow marker, since the
        // marker carries its own padding spaces.
        let gap = 1;
        let layout = pack(40, 0, 16, 20, 10, Alignment::Center, gap);
        assert!(within_bounds(&layout, 40));
        assert!(ordered_non_overlapping(&layout));
        assert!(
            layout.tabs.len() >= 2,
            "the window shows several blocks so the gap check is meaningful"
        );
        let spans: Vec<_> = layout.tabs.iter().map(|t| (t.start, t.width)).collect();
        assert!(
            spans.windows(2).all(|w| w[1].0 == w[0].0 + w[0].1 + gap),
            "visible blocks separated by exactly {gap}: {spans:?}"
        );
        let right_start = layout.right.as_ref().map(|marker| marker.start);
        let last_end = layout.tabs.last().map(|tab| tab.start + tab.width);
        assert!(
            right_start.is_some(),
            "a 20-tab strip in 40 columns must draw a right overflow marker"
        );
        assert_eq!(
            right_start, last_end,
            "right marker butts against the last block, no gap before it"
        );
    }

    #[test]
    fn invariants_hold_across_the_input_space() {
        // A deterministic sweep standing in for property testing: no panic
        // (every subtraction / clamp stays valid), spans stay ordered and in
        // bounds, and the active tab is always visible. The conservation law is
        // enforced by the debug_assert in `packed_with_overflow`, which runs in
        // these (debug) test builds. Both alignments are swept — the row anchor
        // must never violate these invariants, in either the all-fit or the
        // overflow branch. The `gap` dimension covers the inter-block separation:
        // reserving gap budget must never push a span out of bounds or make two
        // spans overlap, in either branch. The last two values are pathological:
        // a gap wider than most `cols` here, and `usize::MAX` — the overflow
        // canary. `gap` is unbounded config (`tab_gap`), so without the entry
        // clamp `inactives * gap` would overflow and panic in these debug builds;
        // the invariants holding under `usize::MAX` proves the clamp degrades the
        // layout gracefully (blocks collapse) instead of breaking.
        for gap in [0, 1, 2, 3, 100, usize::MAX] {
            for align in [Alignment::Left, Alignment::Center] {
                for cols in (0..=160).step_by(3) {
                    for tab_count in 1..=40 {
                        for active in 0..tab_count {
                            let layout = pack(cols, 0, 20, tab_count, active, align, gap);
                            assert!(
                                within_bounds(&layout, cols),
                                "bounds: gap={gap} align={align:?} cols={cols} n={tab_count} a={active}"
                            );
                            assert!(
                                ordered_non_overlapping(&layout),
                                "order: gap={gap} align={align:?} cols={cols} n={tab_count} a={active}"
                            );
                            let has_active =
                                layout.tabs.iter().any(|t| t.active && t.position == active);
                            assert!(
                                has_active || cols == 0,
                                "active visible unless empty: gap={gap} align={align:?} cols={cols} n={tab_count} a={active}"
                            );
                        }
                    }
                }
            }
        }
    }

    // ---- pack_with_button (inline new-tab "+" button, #76) ---------------

    #[test]
    fn button_sits_one_gap_past_the_last_tab_when_every_tab_fits() {
        // Few tabs, plenty of room: the button is just another slot at the end,
        // one `gap` past the last visible tab — not pinned to the far-right edge
        // (there is trailing slack beyond it).
        let gap = 2;
        let layout = pack_with_button(120, 0, 20, 4, 1, Alignment::Left, gap, true);
        let last_end = layout.tabs.last().map(|t| t.start + t.width);
        let button = layout.button;
        assert_eq!(
            button.map(|b| b.start),
            last_end.map(|end| end + gap),
            "button starts one gap past the last tab"
        );
        // The button is sized like an inactive tab — a sibling slot, not a
        // fixed-width icon (#76 follow-up).
        let inactive_w = layout.tabs.iter().find(|t| !t.active).map(|t| t.width);
        assert!(
            inactive_w.is_some(),
            "the four-tab fixture has inactive tabs"
        );
        assert_eq!(
            button.map(|b| b.width),
            inactive_w,
            "button matches an inactive tab's width"
        );
        assert!(layout.right.is_none(), "no overflow with four tabs in 120");
        assert!(
            button.is_some_and(|b| b.start + b.width < 120),
            "trailing slack remains to the button's right (not pinned to the edge)"
        );
        assert!(within_bounds(&layout, 120) && ordered_non_overlapping(&layout));
    }

    #[test]
    fn button_width_tracks_the_reduced_budget_inactive_not_the_full_budget_trial()
    -> Result<(), Box<dyn std::error::Error>> {
        // Regression for the mid-sized bar where reserving the button shrinks the
        // inactive tabs below the full-budget trial. 4 tabs / 80 cols / gap 2:
        // the full-budget pack trials its inactives at 18, but once an 18+2
        // reserve is held back the repack shares the remaining 60 cols among the
        // inactives at 11. The "+" must equal the *rendered* 11, not the trial 18,
        // or it would sit visibly wider than the tabs it sits beside. The bar is
        // wide enough that the inactives are uncapped (11 < active_w 20), so the
        // two budgets genuinely disagree — the case the size-match has to cover.
        let full_budget_inactive = pack(80, 0, 20, 4, 1, Alignment::Left, 2)
            .tabs
            .iter()
            .find(|tab| !tab.active)
            .map(|tab| tab.width);
        let layout = pack_with_button(80, 0, 20, 4, 1, Alignment::Left, 2, true);
        let button = layout
            .button
            .ok_or("an 80-col 4-tab bar must record a button")?;
        let rendered_inactive = layout
            .tabs
            .iter()
            .find(|tab| !tab.active)
            .map(|tab| tab.width);
        assert_eq!(
            Some(button.width),
            rendered_inactive,
            "button matches the inactive width of the layout actually drawn"
        );
        assert!(
            full_budget_inactive.is_some_and(|trial| button.width < trial),
            "the fix narrows the button below the full-budget trial ({full_budget_inactive:?} → {})",
            button.width
        );
        assert!(
            layout.right.is_none(),
            "no overflow: the inactives still fit"
        );
        assert!(within_bounds(&layout, 80) && ordered_non_overlapping(&layout));
        Ok(())
    }

    #[test]
    fn button_stays_after_the_last_tab_with_the_right_marker_beyond_it_on_overflow() {
        // Many tabs in a narrow bar force the overflow branch. The button still
        // sits directly after the last *visible* tab, and the `+N →` right
        // marker is shifted to sit beyond the button (#76's [tabs][button]
        // [marker] order) — never overlapping, never past the bar.
        let cols = 40;
        let gap = 1;
        let layout = pack_with_button(cols, 0, 16, 20, 10, Alignment::Center, gap, true);
        let last_end = layout.tabs.last().map(|t| t.start + t.width);
        let button = layout.button;
        let marker_start = layout.right.as_ref().map(|m| m.start);
        assert!(button.is_some(), "the overflow case still records a button");
        assert!(
            marker_start.is_some(),
            "a 20-tab strip in 40 columns must draw a right overflow marker"
        );
        assert_eq!(
            button.map(|b| b.start),
            last_end.map(|end| end + gap),
            "button directly after the last visible tab"
        );
        assert_eq!(
            marker_start,
            button.map(|b| b.start + b.width),
            "right marker butts the button's end, beyond it (#76)"
        );
        assert!(within_bounds(&layout, cols), "nothing spills past the bar");
        assert!(ordered_non_overlapping(&layout));
    }

    #[test]
    fn button_is_dropped_when_the_bar_is_too_narrow_to_host_it() {
        // A bar with no room for an inactive-tab-sized reserve falls back to a
        // plain pack: no button rather than one overlapping the strip. With three
        // tabs in four columns the active block fills the bar, so the reserve
        // can't fit and the layout equals what `pack` alone would produce.
        let layout = pack_with_button(4, 0, 16, 3, 1, Alignment::Center, 1, true);
        assert!(layout.button.is_none(), "too narrow → no button");
        assert_eq!(
            layout,
            pack(4, 0, 16, 3, 1, Alignment::Center, 1),
            "the fallback is exactly a plain pack"
        );
    }

    #[test]
    fn disabled_button_records_no_button() {
        // `with_button: false` degrades to a plain pack, reclaiming the columns a
        // button would have reserved for the tab strip (#76).
        let layout = pack_with_button(120, 0, 20, 4, 1, Alignment::Left, 2, false);
        assert!(layout.button.is_none());
        assert_eq!(layout, pack(120, 0, 20, 4, 1, Alignment::Left, 2));
    }

    #[test]
    fn button_contains_covers_exactly_its_span() {
        // The hit-test is the half-open span `[start, start + width)`: the first
        // and last columns are inside, the column one past the end is not.
        let button = ButtonHit {
            start: 10,
            width: 3,
        };
        assert!(button.contains(10), "first column");
        assert!(button.contains(12), "last column");
        assert!(!button.contains(9), "one before the start");
        assert!(!button.contains(13), "one past the end");
    }

    #[test]
    fn close_contains_matches_exactly_one_cell() {
        // The close target is a single cell — both row and column must match.
        // The same column on another row, or the same row on another column, is a
        // miss, so a click below the × still switches/focuses the tab (#86).
        let close = CloseHit {
            position: 2,
            row: 0,
            column: 11,
        };
        assert!(close.contains(0, 11), "the exact close cell");
        assert!(!close.contains(1, 11), "same column, lower row");
        assert!(!close.contains(0, 10), "same row, neighbour column");
        assert!(!close.contains(0, 12), "same row, neighbour column");
    }

    #[test]
    fn button_hit_test_never_collides_with_a_tab_switch() {
        // Over a packed-with-button layout, the button span and the tab spans
        // are disjoint: no column resolves to both a tab switch and the button,
        // so click routing is unambiguous (#76).
        let cols = 60;
        let layout = pack_with_button(cols, 0, 20, 5, 2, Alignment::Center, 1, true);
        let button = layout.button;
        assert!(button.is_some(), "the button is recorded");
        let collides = (0..cols).any(|column| {
            switch_target_at_column(&layout.tabs, column).is_some()
                && button.is_some_and(|b| b.contains(column))
        });
        assert!(
            !collides,
            "no column routes to both a tab switch and the button"
        );
    }

    #[test]
    fn button_invariants_hold_across_the_input_space() {
        // The same deterministic sweep as the plain-pack invariants, now through
        // `pack_with_button`: across alignments, gaps (including the pathological
        // `usize::MAX` reserve-overflow canary), widths and focus, the button —
        // when recorded — stays in bounds and after every tab, and the shifted
        // right marker never overlaps it. Whenever a button IS dropped, the
        // result is exactly a plain pack.
        for gap in [0, 1, 2, 3, 100, usize::MAX] {
            for align in [Alignment::Left, Alignment::Center] {
                for cols in (0..=160).step_by(3) {
                    for tab_count in 1..=40 {
                        for active in 0..tab_count {
                            let layout =
                                pack_with_button(cols, 0, 20, tab_count, active, align, gap, true);
                            assert!(
                                within_bounds(&layout, cols),
                                "bounds: gap={gap} align={align:?} cols={cols} n={tab_count} a={active}"
                            );
                            assert!(
                                ordered_non_overlapping(&layout),
                                "order: gap={gap} align={align:?} cols={cols} n={tab_count} a={active}"
                            );
                            if layout.button.is_none() {
                                assert_eq!(
                                    layout,
                                    pack(cols, 0, 20, tab_count, active, align, gap),
                                    "a dropped button leaves a plain pack: gap={gap} align={align:?} cols={cols} n={tab_count} a={active}"
                                );
                            }
                            // Whenever a button IS recorded, it is never *wider*
                            // than an inactive tab rendered in *this* layout — the
                            // size match the user asked for, held across the whole
                            // space, not just the wide bars where the full-budget
                            // trial happens to agree. (At the fixed point it equals
                            // the inactive width; off it, it is at most one step
                            // narrower — never the bigger-than-the-tabs look.) (#76)
                            if let Some(button) = layout.button {
                                assert!(
                                    button.width <= button_slot_width(&layout),
                                    "button {} wider than the rendered inactive {}: gap={gap} align={align:?} cols={cols} n={tab_count} a={active}",
                                    button.width,
                                    button_slot_width(&layout),
                                );
                            }
                        }
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
        let layout = pack(cols, 0, 16, 12, 5, Alignment::Center, 0);
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
}

//! Opt-in one-row title bar. The existing router dispatches its spans and cells.
//! Fixed dark colors and white labels deliberately bypass minimap hue cycling.
use crate::line::{CloseHit, LineLayout, Overflow, TabHit, display_width};
use unicode_width::UnicodeWidthChar;

const MIN_WIDTH: usize = 12;
const MAX_WIDTH: usize = 28;
const FG: &str = "\x1b[38;2;255;255;255m";
const ACTIVE: &str = "\x1b[48;2;36;68;92m\x1b[1;4m";
const INACTIVE: &str = "\x1b[48;2;37;52;72m";
const RESET: &str = "\x1b[0m";

pub struct Frame {
    pub output: String,
    pub layout: LineLayout,
    pub close: Vec<CloseHit>,
}

fn marker(hidden: usize, left: bool) -> String {
    if left {
        format!("← +{hidden} ")
    } else {
        format!(" +{hidden} →")
    }
}

/// Keep a contiguous window around the active tab. Normal visible tabs retain
/// 12 columns; hidden tabs get count markers instead of unusable tiny slivers.
fn pack(cols: usize, count: usize, active: usize, desired: usize, gap: usize) -> LineLayout {
    if count == 0 || cols == 0 {
        return LineLayout::default();
    }
    let active = active.min(count - 1);
    let desired = desired.clamp(MIN_WIDTH, MAX_WIDTH);
    let gap = gap.min(cols);
    let (mut lo, mut hi) = (0, count);
    loop {
        let n = hi - lo;
        let marks = if lo > 0 {
            display_width(&marker(lo, true))
        } else {
            0
        } + if hi < count {
            display_width(&marker(count - hi, false))
        } else {
            0
        };
        let needed = n
            .saturating_mul(MIN_WIDTH)
            .saturating_add((n - 1).saturating_mul(gap))
            .saturating_add(marks);
        if needed <= cols || n == 1 {
            break;
        }
        if active - lo >= hi - active - 1 && lo < active {
            lo += 1;
        } else {
            hi -= 1;
        }
    }
    // On an exceptionally narrow terminal keep the active tab reachable; omit
    // markers when their reservation would consume the tab's minimum width.
    let mut left = (lo > 0).then(|| marker(lo, true));
    let mut right = (hi < count).then(|| marker(count - hi, false));
    let mut mark_width =
        left.as_deref().map_or(0, display_width) + right.as_deref().map_or(0, display_width);
    if hi - lo == 1 && cols < MIN_WIDTH + mark_width {
        left = None;
        right = None;
        mark_width = 0;
    }
    let n = hi - lo;
    let width = ((cols
        .saturating_sub(mark_width)
        .saturating_sub((n - 1).saturating_mul(gap)))
        / n)
        .min(desired);
    let mut cursor = left.as_deref().map_or(0, display_width);
    let left = left.map(|text| Overflow {
        hidden: lo,
        start: 0,
        text,
    });
    let mut tabs = Vec::with_capacity(n);
    for position in lo..hi {
        tabs.push(TabHit {
            position,
            start: cursor,
            width,
            active: position == active,
        });
        cursor += width;
        if position + 1 < hi {
            cursor += gap;
        }
    }
    let right = right.map(|text| Overflow {
        hidden: count - hi,
        start: cursor,
        text,
    });
    LineLayout {
        tabs,
        left,
        right,
        button: None,
    }
}

fn clipped(text: &str, width: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars().filter(|ch| !ch.is_control()) {
        let w = ch.width().unwrap_or(0);
        // Match the minimap's per-character cell accounting: emitting a ZWJ
        // could collapse a multi-character emoji and move the close glyph away
        // from its recorded cell. Skip zero-width characters consistently.
        if w == 0 {
            continue;
        }
        if used + w > width {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push_str(&" ".repeat(width - used));
    out
}

/// One row of titles; clear any extra allocated rows without inventing targets.
/// Close is exactly the white × cell; its surrounding padding switches tabs.
#[allow(clippy::too_many_arguments)]
pub fn render(
    rows: usize,
    cols: usize,
    names: &[(usize, &str)],
    active: usize,
    prefix: &str,
    desired: usize,
    gap: usize,
    close_enabled: bool,
) -> Frame {
    let active_ordinal = names
        .iter()
        .position(|(position, _)| *position == active)
        .unwrap_or(0);
    let mut layout = pack(cols, names.len(), active_ordinal, desired, gap);
    let mut output = String::new();
    for row in 0..rows {
        output.push_str(&format!("\x1b[{};1H{RESET}\x1b[0K", row + 1));
    }
    let mut close = Vec::new();
    for hit in &mut layout.tabs {
        let ordinal = hit.position;
        // The upstream packer uses ordinal slots; keep actual host positions in
        // recorded geometry, so close/switch route to the native tab identity.
        let (position, name) = names[ordinal];
        hit.position = position;
        hit.active = position == active;
        let can_close = close_enabled && names.len() > 1 && hit.width >= 4;
        let text_width = hit.width.saturating_sub(if can_close { 2 } else { 0 });
        let label = format!(
            "{}{prefix}{} {name}",
            if hit.active { ">" } else { " " },
            position + 1
        );
        output.push_str(&format!(
            "\x1b[1;{}H{RESET}{}{}{}",
            hit.start + 1,
            if hit.active { ACTIVE } else { INACTIVE },
            FG,
            clipped(&label, text_width)
        ));
        if can_close {
            // Anchor the close cell independently of terminal text shaping.
            output.push_str(&format!("\x1b[1;{}H× ", hit.start + hit.width - 1));
            close.push(CloseHit {
                position,
                row: 0,
                column: hit.start + hit.width - 2,
            });
        }
        output.push_str(RESET);
    }
    for mark in [&layout.left, &layout.right].into_iter().flatten() {
        output.push_str(&format!(
            "\x1b[1;{}H{RESET}{INACTIVE}{FG}{}{RESET}",
            mark.start + 1,
            mark.text
        ));
    }
    Frame {
        output,
        layout,
        close,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::{ClickIntent, route_click};
    use std::collections::BTreeMap;

    #[test]
    fn unicode_labels_keep_the_close_cell_separate_from_switch_padding() {
        let names = [
            (2, "a long ASCII title that must be clipped"),
            (5, "漢字漢字漢字漢字漢字漢字漢字漢字漢字漢字"),
            (9, "👩‍💻👩‍💻👩‍💻👩‍💻👩‍💻👩‍💻👩‍💻👩‍💻"),
            (14, "👍🏽👍🏽👍🏽👍🏽👍🏽👍🏽👍🏽👍🏽"),
            (20, "\n\r\tcontrol e\u{301} and a long title to clip"),
        ];
        let frame = render(1, 160, &names, 9, "Alt-", 24, 2, true);
        let tab_panes = BTreeMap::new();
        // Five 24-column tabs, separated by two columns: the close glyph owns
        // the penultimate cell, while padding on either side switches tabs.
        assert_eq!(frame.close.len(), 5);
        for ((position, column), close) in [(2, 22), (5, 48), (9, 74), (14, 100), (20, 126)]
            .into_iter()
            .zip(&frame.close)
        {
            assert_eq!(
                (close.position, close.row, close.column),
                (position, 0, column)
            );
            assert!(frame.output.contains(&format!("\x1b[1;{}H× ", column + 1)));
            assert_eq!(
                route_click(
                    None,
                    &frame.close,
                    &frame.layout.tabs,
                    &tab_panes,
                    0,
                    column
                ),
                ClickIntent::CloseTab(position)
            );
            for padding in [column - 1, column + 1] {
                assert_eq!(
                    route_click(
                        None,
                        &frame.close,
                        &frame.layout.tabs,
                        &tab_panes,
                        0,
                        padding
                    ),
                    ClickIntent::SwitchTab(position as u32 + 1)
                );
            }
        }
        for unsafe_label_char in ['\n', '\r', '\t', '\u{301}', '\u{200d}'] {
            assert!(!frame.output.contains(unsafe_label_char));
        }
    }

    #[test]
    fn narrow_and_overflowing_bars_keep_the_active_tab_and_targets_in_bounds() {
        let names = [
            (2, "first"),
            (5, "second"),
            (9, "middle"),
            (14, "fourth"),
            (20, "last"),
        ];
        for cols in [0, 1, 3, 4, 11, 12, 20, 40, 80, 160] {
            for desired in [0, 24, usize::MAX] {
                for gap in [0, 2, usize::MAX] {
                    for active in [2, 9, 20] {
                        let frame = render(3, cols, &names, active, "", desired, gap, true);
                        assert!(frame.layout.button.is_none());
                        if cols == 0 {
                            assert!(frame.layout.tabs.is_empty());
                            assert!(frame.close.is_empty());
                            continue;
                        }
                        assert!(
                            frame
                                .layout
                                .tabs
                                .iter()
                                .any(|tab| tab.active && tab.position == active)
                        );
                        for tab in &frame.layout.tabs {
                            assert!((cols.min(12)..=28).contains(&tab.width));
                            assert!(tab.start + tab.width <= cols);
                        }
                        for pair in frame.layout.tabs.windows(2) {
                            assert_eq!(pair[0].width, pair[1].width);
                            assert!(pair[0].start + pair[0].width <= pair[1].start);
                            let ordinal = names
                                .iter()
                                .position(|(position, _)| *position == pair[0].position)
                                .unwrap();
                            assert_eq!(pair[1].position, names[ordinal + 1].0);
                        }
                        let mut hidden = 0;
                        for marker in [&frame.layout.left, &frame.layout.right]
                            .into_iter()
                            .flatten()
                        {
                            hidden += marker.hidden;
                            let end = marker.start + display_width(&marker.text);
                            assert!(end <= cols);
                            for column in marker.start..end {
                                assert_eq!(
                                    crate::line::position_at_column(&frame.layout.tabs, column),
                                    None
                                );
                            }
                        }
                        if hidden > 0 || frame.layout.tabs.len() == names.len() {
                            assert_eq!(hidden + frame.layout.tabs.len(), names.len());
                        } else {
                            // When markers cannot fit, the active tab gets the
                            // available columns instead of disappearing.
                            assert_eq!(frame.layout.tabs.len(), 1);
                        }
                        for close in &frame.close {
                            assert_eq!(close.row, 0);
                            assert!(close.column < cols);
                            assert_eq!(
                                crate::line::position_at_column(&frame.layout.tabs, close.column),
                                Some(close.position)
                            );
                        }
                    }
                }
            }
        }
        let empty = render(1, 80, &[], 0, "", 24, 2, true);
        assert_eq!(empty.layout, LineLayout::default());
        assert!(empty.close.is_empty());
    }

    #[test]
    fn disabled_close_and_the_last_tab_have_no_close_glyph_or_target() {
        for (names, close_enabled) in [
            (&[(2, "first"), (9, "second")][..], false),
            (&[(9, "last")][..], true),
        ] {
            let frame = render(1, 80, names, 9, "", 24, 2, close_enabled);
            assert!(frame.close.is_empty());
            assert!(!frame.output.contains('×'));
            for tab in &frame.layout.tabs {
                assert_eq!(
                    route_click(
                        None,
                        &frame.close,
                        &frame.layout.tabs,
                        &BTreeMap::new(),
                        0,
                        tab.start + tab.width - 2
                    ),
                    ClickIntent::SwitchTab(tab.position as u32 + 1)
                );
            }
        }
    }
}

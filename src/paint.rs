//! Plugin-pane output framing — the opposite end of the pipeline from
//! [`crate::projection`].
//!
//! [`crate::minimap::render`] produces a clean `\n`-separated block, but writing
//! that block straight into a multi-row plugin pane corrupts it (see [`framed`]).
//! These helpers reshape the renderer's output for the pane and provide the
//! minimal non-active-tab placeholder for this milestone. Both are pure string
//! transforms with no zellij dependency, so they unit-test natively.

/// Reframe the minimap's `\n`-separated, fully-painted rows into output safe for
/// a multi-row plugin pane.
///
/// `render` returns each row as exactly `cols` painted cells terminated by a
/// reset and a newline. zellij homes the cursor to the pane's top-left before
/// each `render()`, but the official one-row tab bar leans on a bare `print!` —
/// reused unchanged across three rows it corrupts the block. So each row is
/// explicitly positioned (`\u{1b}[{n};1H`, 1-based and pane-relative) and cleared
/// to end-of-line (`\u{1b}[0K`) so a previous, wider frame cannot bleed through
/// when the layout shrinks. Emits no trailing newline: a 3rd newline in a 3-row
/// pane would scroll the block up.
pub fn framed(body: &str, rows: usize) -> String {
    body.lines()
        .take(rows)
        .enumerate()
        .map(|(index, line)| format!("\u{1b}[{row};1H{line}\u{1b}[0K", row = index + 1))
        .collect()
}

/// Minimal placeholder for non-active tabs in this milestone: space-joined
/// position hints such as `⌘2 ⌘3`.
///
/// `positions` are the 0-based `TabInfo.position`s of the non-active tabs;
/// `prefix` is the configured shortcut glyph. Hints are shown 1-based to match
/// `GoToTab N`. Per design §4.5 a tab with no `Super N` binding (the 10th tab
/// onward, i.e. 1-based ≥ 10) drops the prefix and shows the bare number. Full
/// width-budgeted multi-tab packing with `+N` overflow lands in the layout
/// issue (#4); this is deliberately just a non-panicking marker.
pub fn inactive_hints(positions: impl Iterator<Item = usize>, prefix: &str) -> String {
    positions
        .map(|position| match position + 1 {
            number if number >= 10 => number.to_string(),
            number => format!("{prefix}{number}"),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Place the non-active hint string just to the right of the active block, or
/// emit nothing when there are no other tabs. Row 2 keeps it vertically centered
/// in the 3-row bar; `block_width` is the active block's column budget, and the
/// `+ 2` leaves a one-column gap after it.
pub fn positioned_hints(hints: &str, block_width: usize) -> String {
    if hints.is_empty() {
        return String::new();
    }
    format!("\u{1b}[2;{column}H{hints}", column = block_width + 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framed_positions_each_row_top_to_bottom() -> Result<(), Box<dyn std::error::Error>> {
        let framed = framed("AA\nBB\nCC\n", 3);
        let one = framed.find("\u{1b}[1;1H").ok_or("row 1 positioned")?;
        let two = framed.find("\u{1b}[2;1H").ok_or("row 2 positioned")?;
        let three = framed.find("\u{1b}[3;1H").ok_or("row 3 positioned")?;
        assert!(one < two && two < three, "rows are in top-to-bottom order");
        assert!(framed.contains("\u{1b}[1;1HAA\u{1b}[0K"));
        assert!(framed.contains("\u{1b}[2;1HBB\u{1b}[0K"));
        assert!(framed.contains("\u{1b}[3;1HCC\u{1b}[0K"));
        Ok(())
    }

    #[test]
    fn framed_clears_every_row_to_end_of_line() {
        assert_eq!(framed("AA\nBB\nCC\n", 3).matches("\u{1b}[0K").count(), 3);
    }

    #[test]
    fn framed_has_no_trailing_newline() {
        assert!(!framed("AA\nBB\nCC\n", 3).ends_with('\n'));
    }

    #[test]
    fn framed_caps_at_the_row_budget() {
        // A body with more lines than the pane has rows must not overflow.
        let framed = framed("AA\nBB\nCC\nDD\nEE\n", 3);
        assert_eq!(framed.matches(";1H").count(), 3);
        assert!(!framed.contains("DD"));
    }

    #[test]
    fn framed_empty_body_is_empty() {
        assert_eq!(framed("", 3), "");
    }

    #[test]
    fn inactive_hints_prefixes_low_positions() {
        // Positions 1 and 2 (0-based) → tabs 2 and 3 (1-based).
        assert_eq!(inactive_hints([1, 2].into_iter(), "⌘"), "⌘2 ⌘3");
    }

    #[test]
    fn inactive_hints_drops_prefix_without_a_binding() {
        // Position 9 (0-based) → tab 10, which has no `Super 10` binding.
        assert_eq!(inactive_hints([9].into_iter(), "⌘"), "10");
    }

    #[test]
    fn inactive_hints_empty_for_no_other_tabs() {
        assert_eq!(inactive_hints(std::iter::empty(), "⌘"), "");
    }

    #[test]
    fn positioned_hints_places_after_the_block() {
        // Block width 24 → hints start at column 26 (one-column gap), row 2.
        assert_eq!(positioned_hints("⌘2", 24), "\u{1b}[2;26H⌘2");
    }

    #[test]
    fn positioned_hints_empty_when_no_hints() {
        assert_eq!(positioned_hints("", 24), "");
    }
}

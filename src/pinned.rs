//! Pinned-float detection from a per-tab session-layout dump (#119).
//!
//! `PaneInfo` carries no pin flag, but `dump_session_layout_for_tab`
//! serializes each float with its cell geometry and a `pinned true` child
//! node (zellij 0.44.3 `session_serialization.rs`). This module is the pure
//! half of that route: a string-level KDL scan ([`pinned_float_rects`]) plus
//! the geometry correlation back to manifest float ids ([`pinned_ids`]). No
//! zellij types and no host calls, so the whole module runs under
//! `cargo test --lib` (rule #8); the wasm-only dump call itself stays in
//! `lib.rs` (rule #17).

use crate::minimap::PaneRect;

/// A float's cell rectangle as serialized in the layout dump — the
/// correlation key. The dump's `x/y/width/height` are the same tab-relative
/// cell values `PaneInfo.pane_x/pane_y/pane_columns/pane_rows` report, so
/// exact equality is the match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellRect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

/// The cell rects of every floating pane marked `pinned true` in a session
/// layout dump, in document order.
///
/// String-level scan, no KDL crate: the dump is `kdl-rs` Display output —
/// one node per line, blocks opened by a trailing `{` and closed by a lone
/// `}` — so a line-based walk with a block-name stack suffices. Only
/// `floating_panes` blocks **directly under a `tab` node** count: the same
/// document carries `swap_floating_layout` / `new_tab_template` template
/// blocks whose `floating_panes` are layout templates, not live panes.
/// Anything unparseable degrades to "not pinned": a float with a percent
/// (`width "60%"`) or missing coordinate is skipped, and malformed input
/// yields an empty vec — the cue is additive, never load-bearing.
pub fn pinned_float_rects(kdl: &str) -> Vec<CellRect> {
    let mut stack: Vec<String> = Vec::new();
    let mut current: Option<FloatFields> = None;
    let mut rects = Vec::new();
    for raw in kdl.lines() {
        let line = raw.trim();
        if line == "}" {
            if in_float_pane(&stack) {
                if let Some(rect) = current.take().and_then(|fields| fields.rect()) {
                    rects.push(rect);
                }
            }
            stack.pop();
            continue;
        }
        if let Some(name) = block_open(line) {
            stack.push(name.to_string());
            if in_float_pane(&stack) {
                current = Some(FloatFields::default());
            }
            continue;
        }
        if in_float_pane(&stack) {
            if let Some(fields) = current.as_mut() {
                fields.absorb(line);
            }
        }
    }
    rects
}

/// The geometry fields plus the pin flag accumulated while scanning one float
/// `pane` block. All-or-nothing: a rect is emitted only when every coordinate
/// parsed as a bare cell count and `pinned true` was seen.
#[derive(Default)]
struct FloatFields {
    x: Option<usize>,
    y: Option<usize>,
    w: Option<usize>,
    h: Option<usize>,
    pinned: bool,
}

impl FloatFields {
    /// Absorb one child line of the float's block. Fixed cell counts are bare
    /// integers (`x 30`); a percent value serializes as a quoted string
    /// (`width "60%"`) and fails the parse, leaving the field `None` — the
    /// whole float is then dropped rather than misplaced.
    fn absorb(&mut self, line: &str) {
        let mut tokens = line.split_whitespace();
        let (Some(name), Some(value)) = (tokens.next(), tokens.next()) else {
            return;
        };
        match name {
            "x" => self.x = value.parse().ok(),
            "y" => self.y = value.parse().ok(),
            "width" => self.w = value.parse().ok(),
            "height" => self.h = value.parse().ok(),
            "pinned" => self.pinned = value == "true",
            _ => {}
        }
    }

    /// The completed rect, if this float was pinned and fully parsed.
    fn rect(&self) -> Option<CellRect> {
        if !self.pinned {
            return None;
        }
        Some(CellRect {
            x: self.x?,
            y: self.y?,
            w: self.w?,
            h: self.h?,
        })
    }
}

/// Whether `line` opens a KDL block, returning the node's name (its first
/// token). kdl-rs Display puts the `{` at the end of the node's own line.
fn block_open(line: &str) -> Option<&str> {
    if !line.ends_with('{') {
        return None;
    }
    line.split_whitespace().next().filter(|name| *name != "{")
}

/// Whether the open-block stack sits inside a live tab's float list:
/// `… > tab > floating_panes > pane`. Template lists (`swap_floating_layout`,
/// `new_tab_template`) never have `tab` as the grandparent, so they miss; a
/// nested block inside the float (`plugin { … }`) pushes past `pane`, so its
/// lines are ignored without losing the accumulated fields.
fn in_float_pane(stack: &[String]) -> bool {
    let n = stack.len();
    n >= 3 && stack[n - 1] == "pane" && stack[n - 2] == "floating_panes" && stack[n - 3] == "tab"
}

/// The ids of the manifest floats whose cell geometry exactly matches a
/// pinned rect from the dump — the dump carries no pane ids, so geometry is
/// the join key (both sides report the same tab-relative cell space). Every
/// float matching a pinned rect is returned: two floats sharing an exact
/// rect are indistinguishable, and an extra pin cue on the twin is the
/// harmless reading of that ambiguity.
pub fn pinned_ids(pinned: &[CellRect], floats: &[PaneRect]) -> Vec<usize> {
    floats
        .iter()
        .filter(|f| {
            pinned.iter().any(|r| {
                (r.x, r.y, r.w, r.h) == (f.x as usize, f.y as usize, f.w as usize, f.h as usize)
            })
        })
        .map(|f| f.id)
        .collect()
}

/// One tab's pinned-float ids, resolved end to end from its dump text (rule
/// #17: the wasm-only `dump_session_layout_for_tab` call itself stays in
/// `lib.rs`, but everything downstream of the KDL string is pure and belongs
/// here, where `cargo test --lib` can actually exercise it). `None` when the
/// dump is unavailable (`kdl`) or nothing in it is pinned, so a caller
/// building a sparse map can skip the entry entirely with `?`/`filter_map`.
pub(crate) fn pinned_ids_for_tab(kdl: Option<&str>, floats: &[PaneRect]) -> Option<Vec<usize>> {
    let ids = pinned_ids(&pinned_float_rects(kdl?), floats);
    (!ids.is_empty()).then_some(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dump shaped exactly like zellij 0.44.3's `serialize_session_layout`
    /// output: bare-integer geometry child nodes in `height/width/x/y` order,
    /// `pinned true` only on the pinned float, and trailing
    /// `new_tab_template` / `swap_floating_layout` template blocks whose
    /// `floating_panes` must NOT be ingested.
    const DUMP: &str = r#"layout {
    tab name="Tab #1" focus=true hide_floating_panes=true {
        pane command="zsh" {
            start_suspended true
        }
        floating_panes {
            pane command="htop" focus=true {
                start_suspended true
                height 18
                width 60
                x 30
                y 12
                pinned true
            }
            pane {
                height 10
                width 40
                x 5
                y 8
            }
        }
    }
    new_tab_template {
        pane
    }
    swap_floating_layout name="staggered" {
        floating_panes {
            pane {
                height 4
                width 3
                x 1
                y 2
                pinned true
            }
        }
    }
}"#;

    #[test]
    fn parses_only_the_live_pinned_float() {
        // One pinned float, one unpinned float, and a pinned-looking template
        // float inside swap_floating_layout: only the live pinned one — under
        // `tab > floating_panes` — comes back.
        assert_eq!(
            pinned_float_rects(DUMP),
            vec![CellRect {
                x: 30,
                y: 12,
                w: 60,
                h: 18
            }]
        );
    }

    #[test]
    fn a_percent_coordinate_drops_the_float() {
        // Live dumps normalize to fixed cells, but a percent value would be a
        // quoted string ("60%") — unparseable as a cell count, so the float is
        // skipped rather than misplaced.
        let kdl = r#"layout {
    tab name="t" {
        pane
        floating_panes {
            pane {
                height 18
                width "60%"
                x 30
                y 12
                pinned true
            }
        }
    }
}"#;
        assert!(pinned_float_rects(kdl).is_empty());
    }

    #[test]
    fn a_nested_block_inside_the_float_does_not_break_the_scan() {
        // A plugin float carries a nested `plugin { ... }` block; the scan
        // must skip its contents without losing the pane's own fields.
        let kdl = r#"layout {
    tab name="t" {
        pane
        floating_panes {
            pane {
                height 18
                width 60
                x 30
                y 12
                plugin location="zellij:session-manager" {
                    some_config true
                }
                pinned true
            }
        }
    }
}"#;
        assert_eq!(
            pinned_float_rects(kdl),
            vec![CellRect {
                x: 30,
                y: 12,
                w: 60,
                h: 18
            }]
        );
    }

    #[test]
    fn malformed_input_yields_nothing() {
        assert!(pinned_float_rects("").is_empty());
        assert!(pinned_float_rects("not kdl at all { { {").is_empty());
        // pinned but with a coordinate missing: dropped, not guessed.
        let missing = r#"layout {
    tab name="t" {
        floating_panes {
            pane {
                width 60
                x 30
                y 12
                pinned true
            }
        }
    }
}"#;
        assert!(pinned_float_rects(missing).is_empty());
    }

    #[test]
    fn two_pinned_floats_in_the_same_tab_are_both_captured() {
        // Guards the accumulator reset: `current.take()` on the first
        // float's closing `}` must hand back a fresh `FloatFields` for the
        // second, or its fields would bleed into (or be shadowed by) the
        // first's.
        let kdl = r#"layout {
    tab name="t" {
        pane
        floating_panes {
            pane command="htop" {
                height 18
                width 60
                x 30
                y 12
                pinned true
            }
            pane command="btm" {
                height 6
                width 20
                x 90
                y 30
                pinned true
            }
        }
    }
}"#;
        assert_eq!(
            pinned_float_rects(kdl),
            vec![
                CellRect {
                    x: 30,
                    y: 12,
                    w: 60,
                    h: 18
                },
                CellRect {
                    x: 90,
                    y: 30,
                    w: 20,
                    h: 6
                },
            ]
        );
    }

    #[test]
    fn an_explicit_pinned_false_line_is_treated_as_unpinned() {
        // A distinct path from a missing coordinate: geometry is complete,
        // but an explicit `pinned false` line still excludes the float.
        let kdl = r#"layout {
    tab name="t" {
        pane
        floating_panes {
            pane {
                height 18
                width 60
                x 30
                y 12
                pinned false
            }
        }
    }
}"#;
        assert!(pinned_float_rects(kdl).is_empty());
    }

    /// A manifest float as the projection produces it (id + cell geometry).
    fn float(id: usize, x: u32, y: u32, w: u32, h: u32) -> PaneRect {
        PaneRect::new(id, x, y, w, h, "f", false)
    }

    #[test]
    fn correlates_a_pinned_rect_to_the_matching_float_id() {
        let pinned = [CellRect {
            x: 30,
            y: 12,
            w: 60,
            h: 18,
        }];
        let floats = [float(7, 30, 12, 60, 18), float(9, 5, 8, 40, 10)];
        assert_eq!(pinned_ids(&pinned, &floats), vec![7]);
    }

    #[test]
    fn no_geometry_match_yields_no_ids() {
        let pinned = [CellRect {
            x: 30,
            y: 12,
            w: 60,
            h: 18,
        }];
        // Same size, shifted one cell: not the same float.
        let floats = [float(7, 31, 12, 60, 18)];
        assert!(pinned_ids(&pinned, &floats).is_empty());
        assert!(pinned_ids(&[], &floats).is_empty());
        assert!(pinned_ids(&pinned, &[]).is_empty());
    }

    #[test]
    fn twin_geometry_marks_both_floats() {
        // Two floats sharing an exact rect are indistinguishable in the dump
        // (it carries no ids); an extra pin cue on the twin is the harmless
        // reading of that ambiguity (design §3.3).
        let pinned = [CellRect {
            x: 30,
            y: 12,
            w: 60,
            h: 18,
        }];
        let floats = [float(7, 30, 12, 60, 18), float(9, 30, 12, 60, 18)];
        assert_eq!(pinned_ids(&pinned, &floats), vec![7, 9]);
    }

    #[test]
    fn pinned_ids_for_tab_is_none_without_a_dump() {
        // The wasm-only dump call yields no text off-wasm (rule #17) — the
        // resolver must treat a missing dump as "no pins", not panic or guess.
        let floats = [float(7, 30, 12, 60, 18)];
        assert_eq!(pinned_ids_for_tab(None, &floats), None);
    }

    #[test]
    fn pinned_ids_for_tab_is_none_when_the_dump_has_no_pins() {
        // A live dump with a float but no `pinned true` line resolves to no
        // entry, not an empty-vec entry — callers skip it with `?`.
        let kdl = r#"layout {
    tab name="t" {
        pane
        floating_panes {
            pane {
                height 10
                width 40
                x 5
                y 8
            }
        }
    }
}"#;
        let floats = [float(7, 5, 8, 40, 10)];
        assert_eq!(pinned_ids_for_tab(Some(kdl), &floats), None);
    }

    #[test]
    fn pinned_ids_for_tab_resolves_a_matching_pin_end_to_end() {
        let floats = [float(7, 30, 12, 60, 18), float(9, 5, 8, 40, 10)];
        assert_eq!(pinned_ids_for_tab(Some(DUMP), &floats), Some(vec![7]));
    }
}

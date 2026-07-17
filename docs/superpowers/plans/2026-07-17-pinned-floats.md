# Pinned-Float Distinction Implementation Plan (#119)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Mark pinned floating panes with a `⌖` corner marker on the minimap, and keep them drawn as overlay boxes (not `◲` chips) while their layer is hidden — matching what zellij actually keeps on screen.

**Architecture:** Pin state is invisible in `PaneInfo`, so `lib.rs` re-dumps each float-bearing tab's session layout (`dump_session_layout_for_tab`, `ReadApplicationState` only) on every `PaneUpdate`, parses the KDL string with a new pure module `src/pinned.rs`, and correlates rects back to float ids by exact cell geometry. A new `FloatSpec::Mixed { chips, overlay }` variant carries a hidden layer's split (pinned → overlay, rest → chips); the pin marker itself rides a new `pinned_floats: &[usize]` parameter threaded down the existing `paint::bar → tab_block → minimap::render` path exactly like #118's `suppressed_covers`.

**Tech Stack:** Rust → `wasm32-wasip1` (zellij-tile 0.44.3). Native tests: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib`.

**Spec:** `docs/superpowers/specs/2026-07-17-pinned-floats-design.md`. The spec's V1/V2 gates are **already verified YES** by static source reading of zellij v0.44.3 (see Task 8's spec note), so this plan implements the main line with no fallback:

- V1 YES — zellij renders pinned floats while the layer is hidden (`zellij-server/src/panes/floating_panes/mod.rs:438-447` filters to `is_pinned` when `!show_panes`; e2e snapshot `pin_floating_panes.snap` confirms).
- V2 YES — hidden floats (with `pinned`) are included in the dump (`screen.rs:5024-5064` iterates `get_floating_panes()` unfiltered; `hide_floating_panes` is only a tab attribute).

**Serialized KDL shape** (verified against `session_serialization.rs:404-476,652-690` and snapshot `can_serialize_tab_with_floating_panes.snap`): geometry as bare-integer **child nodes**, `pinned true` node only when pinned, floats under `tab > floating_panes > pane`. Percent values (`x "30%"`) cannot occur for x/y and effectively never occur for width/height in a live dump (live `PaneGeom`s normalize to Fixed on placement) — the parser still skips them defensively. The same document carries `swap_floating_layout` / `new_tab_template` template blocks with their own `floating_panes` — the parser MUST scope to blocks under a `tab` node.

**Conventions that apply to every task:** no `unwrap()`/`expect()` anywhere (prod or tests — use `Result`-returning test fns with `?`, or plain asserts when nothing is fallible); comments state constraints, not narration; all commits in English.

---

## File Structure

| File | Change |
|---|---|
| `src/pinned.rs` | **New**: `CellRect`, `pinned_float_rects` (KDL scan), `pinned_ids` (geometry correlation) — pure, no zellij types |
| `src/floating.rs` | Add `Mixed { chips, overlay }` to `FloatSpec` / `FloatLayer` |
| `src/minimap.rs` | New `pinned_floats: &[usize]` render param; `Mixed` arms; `PIN_MARKER_GLYPH` corner marker |
| `src/tab_block.rs` | Thread `pinned_floats` through `assemble` / `grid_lines` |
| `src/paint.rs` | Thread `pinned_by_position` through `bar` |
| `src/lib.rs` | `pub mod pinned;` · `pinned_by_tab` state · dump seam + `refresh_pinned` on `PaneUpdate` · `FloatSpec` partition · `TabPaneGeom` recording · wheel/anchor inclusion |
| `README.md`, `.claude/rules/zellij-plugin-development.md`, specs | Docs (Task 8) |

`src/router.rs` needs **no change**: `TabPaneGeom` already carries `hidden_floats` and `visible_floats` side by side, and `route_click` already tries chips, overlay, and marker independently.

---

### Task 1: `src/pinned.rs` — KDL parser (`pinned_float_rects`)

**Files:**

- Create: `src/pinned.rs`
- Modify: `src/lib.rs` (module declaration only)

- [ ] **Step 1: Register the module**

In `src/lib.rs`, add to the module list (alphabetical, after `pub mod paint;`):

```rust
pub mod pinned;
```

- [ ] **Step 2: Create `src/pinned.rs` with the types and failing tests**

Create the file with this content (implementation of `pinned_float_rects` deliberately stubbed to `Vec::new()` so the tests fail first):

```rust
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
pub fn pinned_float_rects(_kdl: &str) -> Vec<CellRect> {
    Vec::new()
}
```

Then the test module at the bottom of the same file:

```rust
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
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib pinned::`
Expected: FAIL — `parses_only_the_live_pinned_float`, `a_nested_block_inside_the_float_does_not_break_the_scan` (the stub returns empty; the other two pass vacuously — that is fine).

- [ ] **Step 4: Implement the scanner**

Replace the stub with:

```rust
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
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib pinned::`
Expected: PASS (4 tests).

- [ ] **Step 6: Commit**

```bash
git add src/pinned.rs src/lib.rs
git commit -m "feat(pinned): parse pinned float rects from a session-layout dump (#119)"
```

---

### Task 2: `src/pinned.rs` — geometry correlation (`pinned_ids`)

**Files:**

- Modify: `src/pinned.rs`

- [ ] **Step 1: Add the failing tests**

Append inside `mod tests`:

```rust
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
```

- [ ] **Step 2: Run to verify they fail to compile (function missing)**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib pinned::`
Expected: compile error — `pinned_ids` not found.

- [ ] **Step 3: Implement `pinned_ids`**

Add after `pinned_float_rects`:

```rust
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib pinned::`
Expected: PASS (7 tests).

- [ ] **Step 5: Commit**

```bash
git add src/pinned.rs
git commit -m "feat(pinned): correlate pinned rects to float ids by exact geometry (#119)"
```

---

### Task 3: `src/floating.rs` — `Mixed` float layer variant

**Files:**

- Modify: `src/floating.rs`

- [ ] **Step 1: Add the failing test**

Append inside `mod tests` in `src/floating.rs`:

```rust
    #[test]
    fn mixed_spec_borrows_both_chip_ids_and_overlay_rects() {
        // A hidden layer with pinned floats (#119): the pinned ones overlay
        // while the rest chip — one spec carries both halves.
        let spec = FloatSpec::Mixed {
            chips: vec![9],
            overlay: vec![PaneRect::new(7, 30, 12, 60, 18, "f", false)],
        };
        match spec.layer() {
            FloatLayer::Mixed { chips, overlay } => {
                assert_eq!(chips, &[9]);
                assert_eq!(overlay.len(), 1);
                assert_eq!(overlay[0].id, 7);
            }
            _ => assert!(false, "Mixed spec must borrow as Mixed layer"),
        }
    }
```

- [ ] **Step 2: Run to verify it fails to compile**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib floating::`
Expected: compile error — no `Mixed` variant.

- [ ] **Step 3: Add the variants**

In `FloatLayer` (after `Visible`):

```rust
    /// A hidden layer whose pinned floats stay on screen (#119): zellij keeps
    /// pinned floats rendered while the layer is hidden, so the bar overlays
    /// them like a visible layer and chips only the rest.
    Mixed {
        chips: &'a [usize],
        overlay: &'a [PaneRect],
    },
```

In `FloatSpec` (after `Visible`):

```rust
    /// See [`FloatLayer::Mixed`] — the owned per-frame form (#119).
    Mixed {
        chips: Vec<usize>,
        overlay: Vec<PaneRect>,
    },
```

In `FloatSpec::layer`, add the arm:

```rust
            FloatSpec::Mixed { chips, overlay } => FloatLayer::Mixed { chips, overlay },
```

Also update the `FloatLayer` doc comment's variant list (it enumerates `None` / `Hidden` / `Visible`) to mention `Mixed`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib`
Expected: PASS (all — the new variant breaks no exhaustive match because `minimap::render` matches with `_` arms; if the compiler flags any non-exhaustive match elsewhere, add a `Mixed`-ignoring arm there and note it for Task 5).

- [ ] **Step 5: Commit**

```bash
git add src/floating.rs
git commit -m "feat(floating): add the Mixed chips+overlay float layer (#119)"
```

---

### Task 4: Thread `pinned_floats` through the paint pipeline (mechanical, no behavior change)

**Files:**

- Modify: `src/minimap.rs` (`render` signature + every test call site)
- Modify: `src/tab_block.rs` (`assemble`, `grid_lines` signatures + pass-through + every test call site)
- Modify: `src/paint.rs` (`bar` signature + per-tab resolution + any test call sites)
- Modify: `src/lib.rs` (the one `paint::bar` call site)

This mirrors exactly how #118 threaded `suppressed_covers`. After this task the parameter is plumbed but always empty, so **every existing test must still pass unchanged in behavior**.

- [ ] **Step 1: `minimap::render` — add the parameter**

Add a final parameter after `suppressed_covers: &[usize]`:

```rust
    pinned_floats: &[usize],
```

Document it in the doc comment: "`pinned_floats` lists the ids of overlay floats that are pinned (#119); each stamps a [`PIN_MARKER_GLYPH`] in its top-right corner cell (Task 5)." (The parameter is unused until Task 5 — add `let _ = pinned_floats;` at the top of the body to keep clippy quiet in this intermediate commit, and remove it in Task 5.)

- [ ] **Step 2: `tab_block` — thread it**

`assemble` gains `pinned_floats: &[usize]` after `suppressed_covers`, passes it to every `grid_lines` call; `grid_lines` gains the same parameter and passes it to `minimap::render` as the new final argument.

- [ ] **Step 3: `paint::bar` — thread it**

`bar` gains `pinned_by_position: &BTreeMap<usize, Vec<usize>>` after `suppressed_covers_by_position`. Inside the per-tab closure, resolve like `suppressed_covers`:

```rust
            // This tab's pinned float ids (#119), resolved the same way as
            // `suppressed_covers` — absent → an empty slice, so a tab with no
            // pinned floats stamps no pin marker.
            let pinned_floats = pinned_by_position
                .get(&hit.position)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
```

and pass `pinned_floats` as `assemble`'s new argument.

- [ ] **Step 4: Update every call site**

- `src/lib.rs` `render()`: pass `&BTreeMap::new()` as `paint::bar`'s new argument (replaced with live state in Task 6).
- Every `minimap::render(...)` call in `src/minimap.rs` tests: append `&[]`.
- Every `assemble(...)` / `grid_lines(...)` call in `src/tab_block.rs` (tests and non-test): append `&[]`.
- Every `bar(...)` call in `src/paint.rs` tests: append `&BTreeMap::new()`.

- [ ] **Step 5: Verify no behavior change**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib`
Expected: PASS — same test count as before this task (325 + Tasks 1–3 additions), zero failures.

Run: `cargo clippy --target wasm32-wasip1 --all-features --lib`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add src/minimap.rs src/tab_block.rs src/paint.rs src/lib.rs
git commit -m "refactor(render): thread pinned-float ids through the paint pipeline (#119)"
```

---

### Task 5: `src/minimap.rs` — pin marker + `Mixed` rendering

**Files:**

- Modify: `src/minimap.rs`

- [ ] **Step 1: Add the failing tests**

Append to `mod tests` in `src/minimap.rs`, mirroring the `f_*` test conventions (`test_palette()`, `visible_lines`, 12-arg + new 13th-arg `render` call):

```rust
    #[test]
    fn f_a_pinned_float_carries_the_corner_pin_marker() {
        // A visible float on the block's right half, pinned: its top-right
        // corner cell (col 23, row 0 at pw=24) carries the pin glyph. The
        // same render without the pinned id must not.
        let palette = test_palette();
        let tiled = [PaneRect::new(2, 0, 0, 120, 40, "sh", false)];
        let floats = [PaneRect::new(9, 60, 0, 60, 40, "htop", false)];
        let out = render(
            &tiled,
            &palette,
            24,
            4,
            0,
            LabelMode::None,
            None,
            Close::Off,
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::Visible(&floats),
            &[],
            &[9],
        );
        let top: Vec<char> = visible_lines(&out)[0].chars().collect();
        assert_eq!(
            top[23], PIN_MARKER_GLYPH,
            "the pinned float's top-right corner carries the pin: {top:?}"
        );

        let unpinned = render(
            &tiled,
            &palette,
            24,
            4,
            0,
            LabelMode::None,
            None,
            Close::Off,
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::Visible(&floats),
            &[],
            &[],
        );
        let top: Vec<char> = visible_lines(&unpinned)[0].chars().collect();
        assert_ne!(top[23], PIN_MARKER_GLYPH, "no pin id → no marker: {top:?}");
    }

    #[test]
    fn f_a_one_column_float_draws_no_pin_marker() {
        // A float projecting to a single column would be all marker — the
        // size gate keeps it color + ring only.
        let palette = test_palette();
        let tiled = [PaneRect::new(2, 0, 0, 120, 40, "sh", false)];
        let floats = [PaneRect::new(9, 115, 0, 5, 40, "f", false)];
        let out = render(
            &tiled,
            &palette,
            24,
            4,
            0,
            LabelMode::None,
            None,
            Close::Off,
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::Visible(&floats),
            &[],
            &[9],
        );
        assert!(
            !out.contains(PIN_MARKER_GLYPH),
            "a 1-column float carries no pin marker"
        );
    }

    #[test]
    fn f_an_occluded_corner_draws_no_pin_marker() {
        // A later (topmost) float covers the pinned float's top-right corner:
        // the pinned float no longer owns that cell, so no marker — and the
        // unpinned cover float draws none of its own.
        let palette = test_palette();
        let tiled = [PaneRect::new(2, 0, 0, 120, 40, "sh", false)];
        let floats = [
            PaneRect::new(9, 60, 0, 60, 40, "under", false),
            PaneRect::new(5, 100, 0, 20, 40, "over", false),
        ];
        let out = render(
            &tiled,
            &palette,
            24,
            4,
            0,
            LabelMode::None,
            None,
            Close::Off,
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::Visible(&floats),
            &[],
            &[9],
        );
        assert!(
            !out.contains(PIN_MARKER_GLYPH),
            "an occluded corner stamps no marker"
        );
    }

    #[test]
    fn f_mixed_layer_draws_chips_and_the_pinned_overlay_together() {
        // A hidden layer with one pinned float (#119): the pinned float
        // overlays (with its pin marker) while the other float chips into the
        // bottom-right corner — and the chip keeps owning its cell even where
        // the overlay covers it.
        let palette = test_palette();
        let tiled = [PaneRect::new(2, 0, 0, 120, 40, "sh", false)];
        let overlay = [PaneRect::new(9, 60, 0, 60, 40, "htop", false)];
        let chips = [7usize];
        let out = render(
            &tiled,
            &palette,
            24,
            4,
            0,
            LabelMode::None,
            None,
            Close::Off,
            GradientSpec::OFF,
            true,
            crate::floating::FloatLayer::Mixed {
                chips: &chips,
                overlay: &overlay,
            },
            &[],
            &[9],
        );
        let lines = visible_lines(&out);
        let top: Vec<char> = lines[0].chars().collect();
        let bottom: Vec<char> = lines[3].chars().collect();
        assert_eq!(top[23], PIN_MARKER_GLYPH, "pinned overlay pin: {top:?}");
        assert_eq!(
            bottom[23],
            crate::floating::CHIP_GLYPH,
            "the unpinned float still chips, over the overlay: {bottom:?}"
        );
    }

    #[test]
    fn f_an_inactive_tab_still_pins_but_muted() {
        // The pin cue applies on every tab; inactive tabs mute the glyph's fg
        // toward the fill like every other glyph, but the marker stays.
        let palette = test_palette();
        let tiled = [PaneRect::new(2, 0, 0, 120, 40, "sh", false)];
        let floats = [PaneRect::new(9, 60, 0, 60, 40, "htop", false)];
        let out = render(
            &tiled,
            &palette,
            24,
            4,
            0,
            LabelMode::None,
            None,
            Close::Off,
            GradientSpec::OFF,
            false,
            crate::floating::FloatLayer::Visible(&floats),
            &[],
            &[9],
        );
        assert!(
            out.contains(PIN_MARKER_GLYPH),
            "inactive tabs keep the (muted) pin marker"
        );
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib f_`
Expected: compile error — `PIN_MARKER_GLYPH` missing (then, once defined, assertion failures until Step 3 completes).

- [ ] **Step 3: Implement**

(a) The glyph constant, near `CHIP_GLYPH`'s siblings in `minimap.rs` (top constants block):

```rust
/// The pin-marker glyph stamped in a pinned float's top-right corner cell
/// (#119): POSITION INDICATOR — standard Unicode, single display column, no
/// Nerd Font dependency (matching ◲ / ◳ / ⋯). A visual parameter — retune
/// freely after render checks; not a correctness constant.
pub(crate) const PIN_MARKER_GLYPH: char = '⌖';
```

(b) In `render`, extend the two extraction matches to cover `Mixed` (and remove Task 4's `let _ = pinned_floats;`):

```rust
    let float_rects: &[PaneRect] = match floats {
        crate::floating::FloatLayer::Visible(f) => f,
        crate::floating::FloatLayer::Mixed { overlay, .. } => overlay,
        _ => &[],
    };
```

```rust
    let chip_ids: &[usize] = match floats {
        crate::floating::FloatLayer::Hidden(ids) => ids,
        crate::floating::FloatLayer::Mixed { chips, .. } => chips,
        _ => &[],
    };
```

(c) Pin-mark placement, right after the float-label overlay block (after the `float_overlay` loop, before the badge computation):

```rust
    // Pin markers (#119): a pinned float stamps one glyph in its top-right
    // corner cell, the float-layer sibling of the suppressed marker's
    // corner-cell vocabulary (#118). Only where the float itself owns both of
    // the cell's pixels (an overlapping float on top keeps its own paint),
    // and only when the box is at least two columns wide — a one-column float
    // would be all marker. Each entry is (col, text_row, float index).
    let pin_marks: Vec<(usize, usize, usize)> = float_bounds
        .iter()
        .enumerate()
        .filter(|(i, b)| {
            pinned_floats.contains(&float_rects[*i].id) && b.px1 - b.px0 >= 2
        })
        .filter_map(|(i, b)| {
            let col = b.px1 - 1;
            let row = b.py0.div_ceil(2);
            let owned = row < text_rows
                && float_grid[2 * row * pw + col] == Some(i)
                && float_grid[(2 * row + 1) * pw + col] == Some(i);
            owned.then_some((col, row, i))
        })
        .collect();
```

(d) The paint branch, inside the `for tr / for c` loop, right after the suppressed-marker branch and before the `float_overlay` match:

```rust
            // A pinned float's corner pin (#119): bg the float's flat fill,
            // fg its full-strength ring shade so the pin reads on both the
            // ring and the fill — muted toward the fill on an inactive tab
            // like every other glyph. The chip branch above already consumed
            // its cells: a Mixed layer's chips own their corner over any
            // overlay beneath (mirroring the router, which tries chips first).
            if let Some(&(_, _, fi)) = pin_marks.iter().find(|(mc, mr, _)| *mc == c && *mr == tr) {
                let fill = palette.color_for(float_rects[fi].id);
                put_bg(&mut out, fill);
                let base = palette.float_ring_for(float_rects[fi].id, true);
                let pin_fg = if active {
                    base
                } else {
                    crate::color::mixed(base, fill, INACTIVE_LABEL_BLEND)
                };
                put_fg(&mut out, pin_fg);
                out.push(PIN_MARKER_GLYPH);
                continue;
            }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib`
Expected: PASS (all, including the 5 new `f_` tests).

- [ ] **Step 5: Commit**

```bash
git add src/minimap.rs
git commit -m "feat(minimap): stamp a corner pin marker on pinned floats and render Mixed layers (#119)"
```

---

### Task 6: `src/lib.rs` — dump-driven pinned map + hidden-layer partition

**Files:**

- Modify: `src/lib.rs`

- [ ] **Step 1: Add the failing tests**

Append to `mod tests` in `src/lib.rs` (reuse the existing `tab` / `content_pane` helpers):

```rust
    /// A floating terminal pane with real geometry, as the manifest reports it.
    fn floating_pane(id: u32, x: usize, y: usize, w: usize, h: usize) -> PaneInfo {
        PaneInfo {
            id,
            is_floating: true,
            pane_x: x,
            pane_y: y,
            pane_columns: w,
            pane_rows: h,
            title: "f".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn render_partitions_a_hidden_layer_by_pin_state() {
        // A hidden layer with floats 7 (pinned) and 9: the pinned one records
        // as an overlay rect (it is still on the real screen, #119) while only
        // the unpinned one chips.
        let mut state = State::default();
        state.permitted = true;
        state.tabs = vec![TabInfo {
            active: true,
            are_floating_panes_visible: false,
            ..tab(0, 1)
        }];
        state.panes.panes.insert(
            0,
            vec![
                content_pane(0, 1, 80, 24),
                floating_pane(7, 10, 5, 30, 10),
                floating_pane(9, 40, 8, 20, 6),
            ],
        );
        state.pinned_by_tab = BTreeMap::from([(0usize, vec![7usize])]);

        state.render(MIN_ROWS, 80);

        let geom = state.tab_panes.get(&0);
        assert_eq!(
            geom.map(|g| g.hidden_floats.clone()),
            Some(vec![9]),
            "only the unpinned float chips"
        );
        assert_eq!(
            geom.map(|g| g.visible_floats.iter().map(|f| f.id).collect::<Vec<_>>()),
            Some(vec![7]),
            "the pinned float stays an overlay while its layer is hidden"
        );
    }

    #[test]
    fn render_keeps_a_fully_pinned_hidden_layer_chipless() {
        // Every hidden float pinned → all overlay, no chips.
        let mut state = State::default();
        state.permitted = true;
        state.tabs = vec![TabInfo {
            active: true,
            are_floating_panes_visible: false,
            ..tab(0, 1)
        }];
        state
            .panes
            .panes
            .insert(0, vec![content_pane(0, 1, 80, 24), floating_pane(7, 10, 5, 30, 10)]);
        state.pinned_by_tab = BTreeMap::from([(0usize, vec![7usize])]);

        state.render(MIN_ROWS, 80);

        let geom = state.tab_panes.get(&0);
        assert_eq!(geom.map(|g| g.hidden_floats.len()), Some(0));
        assert_eq!(geom.map(|g| g.visible_floats.len()), Some(1));
    }

    #[test]
    fn a_visible_layer_ignores_pin_state() {
        // Pin only changes hidden-layer classification: a visible layer keeps
        // every float in the overlay exactly as before.
        let mut state = State::default();
        state.permitted = true;
        state.tabs = vec![TabInfo {
            active: true,
            are_floating_panes_visible: true,
            ..tab(0, 1)
        }];
        state.panes.panes.insert(
            0,
            vec![
                content_pane(0, 1, 80, 24),
                floating_pane(7, 10, 5, 30, 10),
                floating_pane(9, 40, 8, 20, 6),
            ],
        );
        state.pinned_by_tab = BTreeMap::from([(0usize, vec![7usize])]);

        state.render(MIN_ROWS, 80);

        let geom = state.tab_panes.get(&0);
        assert_eq!(geom.map(|g| g.hidden_floats.len()), Some(0));
        assert_eq!(geom.map(|g| g.visible_floats.len()), Some(2));
    }

    #[test]
    fn pane_update_refreshes_pinned_without_a_host_panic_off_wasm() {
        // The dump call is wasm-only (rule #17); the native seam returns None,
        // so a PaneUpdate with floats must neither panic nor invent pins —
        // and stale entries from a previous refresh are dropped.
        let mut state = State::default();
        state.pinned_by_tab = BTreeMap::from([(0usize, vec![7usize])]);
        let mut manifest = PaneManifest::default();
        manifest
            .panes
            .insert(0, vec![content_pane(0, 1, 80, 24), floating_pane(7, 10, 5, 30, 10)]);

        assert!(state.update(Event::PaneUpdate(manifest)));
        assert!(
            state.pinned_by_tab.is_empty(),
            "the native seam yields no dumps, so no pins survive a refresh"
        );
    }

    #[test]
    fn floating_off_clears_the_pinned_map() {
        // With `floating = off` the bar depicts no floats — the refresh skips
        // every dump and drops stale pin data.
        let mut state = State::default();
        state.config = Config {
            floating: crate::floating::FloatingMode::Off,
            ..Default::default()
        };
        state.pinned_by_tab = BTreeMap::from([(0usize, vec![7usize])]);
        let mut manifest = PaneManifest::default();
        manifest
            .panes
            .insert(0, vec![content_pane(0, 1, 80, 24), floating_pane(7, 10, 5, 30, 10)]);

        assert!(state.update(Event::PaneUpdate(manifest)));
        assert!(state.pinned_by_tab.is_empty());
    }
```

- [ ] **Step 2: Run to verify they fail to compile**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib`
Expected: compile error — `pinned_by_tab` field missing.

- [ ] **Step 3: Implement**

(a) State field (after `close_layout`):

```rust
    /// Per-tab pinned floating-pane ids (#119), keyed by tab position and
    /// rebuilt on every `PaneUpdate` from a per-tab session-layout dump
    /// ([`Self::refresh_pinned`]) — pin state is invisible in `PaneInfo`.
    /// Drives the hidden-layer partition (pinned floats overlay, the rest
    /// chip), the corner pin marker, and the wheel/anchor visibility of
    /// hidden-layer pinned floats. Ids are `PaneRect`-space (`usize`).
    pinned_by_tab: BTreeMap<usize, Vec<usize>>,
```

(b) The host seam, as free functions near the bottom of the non-test code (right before the `#[cfg(test)]` stub section):

```rust
/// The KDL text of one tab's live session layout, via the host (#119) — or
/// `None` when the dump fails (the cue is additive, so a failed dump just
/// means "no pins this frame"). `dump_session_layout_for_tab` reads its reply
/// back from the plugin's stdin (rule #17), which panics off-wasm, so the
/// native-test build stubs the whole call; tests inject `pinned_by_tab`
/// directly instead.
#[cfg(not(test))]
fn layout_dump_for_tab(tab_index: usize) -> Option<String> {
    dump_session_layout_for_tab(tab_index)
        .ok()
        .map(|(kdl, _metadata)| kdl)
}

#[cfg(test)]
fn layout_dump_for_tab(_tab_index: usize) -> Option<String> {
    None
}
```

(c) `refresh_pinned`, in `impl State` (near `scroll`):

```rust
    /// Rebuild [`Self::pinned_by_tab`] from a per-tab session-layout dump
    /// (#119). A pin toggle arrives as a `PaneUpdate` with no `PaneInfo`
    /// field changed, so manifest-keyed caching would miss it — every
    /// float-bearing tab is re-dumped on each `PaneUpdate` instead. Tabs
    /// without floats are skipped (no dump), and `floating = off` skips
    /// everything: the bar depicts no floats, so pin data would be dead
    /// weight.
    fn refresh_pinned(&mut self) {
        if self.config.floating == floating::FloatingMode::Off {
            self.pinned_by_tab = BTreeMap::new();
            return;
        }
        self.pinned_by_tab = self
            .panes
            .panes
            .iter()
            .filter(|(_, panes)| panes.iter().any(projection::is_floating_terminal))
            .filter_map(|(&position, panes)| {
                let kdl = layout_dump_for_tab(position)?;
                let rects = pinned::pinned_float_rects(&kdl);
                let ids = pinned::pinned_ids(&rects, &projection::project_floating(panes));
                (!ids.is_empty()).then_some((position, ids))
            })
            .collect();
    }
```

(Adjust the `filter` closure to `|p| projection::is_floating_terminal(p)` if the point-free form fails the borrow check.)

(d) The `PaneUpdate` arm:

```rust
            Event::PaneUpdate(panes) => {
                self.panes = panes;
                // Pin state rides only in the session-layout dump (#119) —
                // refresh it with the same cadence as the manifest it
                // correlates against.
                self.refresh_pinned();
                true
            }
```

(e) The `FloatSpec` partition in `render()` — replace the `let spec = if floats.is_empty() { ... }` chain with:

```rust
                    let spec = if floats.is_empty() {
                        floating::FloatSpec::None
                    } else if visible {
                        floating::FloatSpec::Visible(floats)
                    } else {
                        // A hidden layer's pinned floats stay on the real
                        // screen (#119) — zellij renders pinned floats while
                        // the layer is hidden — so they keep overlaying and
                        // only the rest fold into chips.
                        let pinned = self.pinned_by_tab.get(&hit.position);
                        let (overlay, chipped): (Vec<_>, Vec<_>) =
                            floats.into_iter().partition(|f| {
                                pinned.is_some_and(|ids| ids.contains(&f.id))
                            });
                        let chips = chipped.into_iter().map(|f| f.id).collect();
                        if overlay.is_empty() {
                            floating::FloatSpec::Hidden(chips)
                        } else {
                            floating::FloatSpec::Mixed { chips, overlay }
                        }
                    };
```

(f) `TabPaneGeom` recording — extend both matches:

```rust
                        hidden_floats: match floats_by_position.get(&hit.position) {
                            Some(floating::FloatSpec::Hidden(ids)) => ids.clone(),
                            Some(floating::FloatSpec::Mixed { chips, .. }) => chips.clone(),
                            _ => Vec::new(),
                        },
```

```rust
                        visible_floats: match floats_by_position.get(&hit.position) {
                            Some(floating::FloatSpec::Visible(rects)) => rects.clone(),
                            Some(floating::FloatSpec::Mixed { overlay, .. }) => overlay.clone(),
                            _ => Vec::new(),
                        },
```

(Update the two comments above them: chips also come from a `Mixed` layer's hidden half; overlay rects also come from a `Mixed` layer's pinned half.)

(g) The `paint::bar` call: replace Task 4's `&BTreeMap::new()` with `&self.pinned_by_tab`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib`
Expected: PASS (all, including the 5 new tests).

- [ ] **Step 5: Commit**

```bash
git add src/lib.rs
git commit -m "feat(lib): dump-driven pin detection and hidden-layer partition (#119)"
```

---

### Task 7: Wheel / focus anchor — hidden-layer pinned floats are visible

**Files:**

- Modify: `src/lib.rs`

- [ ] **Step 1: Add the failing tests**

Append to `mod tests` in `src/lib.rs`:

```rust
    #[test]
    fn pane_focus_order_includes_a_hidden_layers_pinned_float() {
        // Tab 1's layer is hidden but its float 25 is pinned — it is on the
        // real screen (#119), so the wheel walks it; the unpinned hidden
        // float case stays excluded (pinned_by_tab carries no entry for it).
        let mut state = scroll_state(scroll::ScrollMode::Pane, Some(10));
        if let Some(panes) = state.panes.panes.get_mut(&1) {
            panes.push(PaneInfo {
                id: 25,
                is_floating: true,
                pane_x: 5,
                pane_y: 5,
                ..Default::default()
            });
        }
        state.pinned_by_tab = BTreeMap::from([(1usize, vec![25usize])]);
        assert_eq!(state.pane_focus_order(), vec![10, 20, 30, 25]);
    }

    #[test]
    fn focused_pane_id_resolves_a_focused_hidden_pinned_float() {
        // Mirrors `focused_pane_id_ignores_a_focused_float_while_the_layer_is
        // _hidden`, but pinned: the float is on screen, so it anchors the
        // wheel even while its layer is hidden.
        let mut state = scroll_state(scroll::ScrollMode::Pane, None);
        if let Some(panes) = state.panes.panes.get_mut(&0) {
            panes.push(PaneInfo {
                id: 15,
                is_floating: true,
                is_focused: true,
                pane_x: 5,
                pane_y: 5,
                ..Default::default()
            });
        }
        state.pinned_by_tab = BTreeMap::from([(0usize, vec![15usize])]);
        assert_eq!(state.focused_pane_id(), Some(15));
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib pane_focus_order_includes focused_pane_id_resolves_a_focused_hidden`
Expected: FAIL (the hidden float is excluded today).

- [ ] **Step 3: Implement**

(a) The predicate, in `impl State`:

```rust
    /// Whether float `id` in the tab at `position` is pinned (#119). Pinned
    /// floats stay on the real screen while their layer is hidden, so the
    /// wheel and the focus anchor treat them as visible.
    fn is_pinned(&self, position: usize, id: u32) -> bool {
        self.pinned_by_tab
            .get(&position)
            .is_some_and(|ids| ids.contains(&(id as usize)))
    }
```

(b) `focused_pane_id` — extend the float arm of the filter:

```rust
            .filter(|pane| {
                projection::is_tiled_terminal(pane)
                    || (projection::is_floating_terminal(pane)
                        && (active.are_floating_panes_visible
                            || self.is_pinned(active.position, pane.id)))
            })
```

(c) `pane_focus_order` — replace the `if tab.are_floating_panes_visible { ... }` block with an unconditional filtered extend:

```rust
                if let Some(p) = panes {
                    order.extend(
                        projection::project_floating(p)
                            .into_iter()
                            .filter(|f| {
                                tab.are_floating_panes_visible
                                    || self.is_pinned(tab.position, f.id as u32)
                            })
                            .map(|f| f.id as u32),
                    );
                }
```

(d) Update the two doc comments that say hidden floats are *always* excluded from the wheel (`focused_pane_id`, `pane_focus_order`): a hidden float is reached only via its chip **unless it is pinned** — a pinned float stays on the real screen while its layer is hidden (#119), so it walks like a visible one.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib`
Expected: PASS (all).

- [ ] **Step 5: Commit**

```bash
git add src/lib.rs
git commit -m "feat(scroll): walk hidden-layer pinned floats with the wheel (#119)"
```

---

### Task 8: Docs — README, project rule #20, spec reconciliation

**Files:**

- Modify: `README.md` (the `floating` section)
- Modify: `.claude/rules/zellij-plugin-development.md` (append entry #20)
- Modify: `docs/superpowers/specs/2026-07-12-floating-panes-in-minimap-design.md` (reconcile the "pinned is blocked" line)
- Modify: `docs/superpowers/specs/2026-07-17-pinned-floats-design.md` (record V1/V2 verification results)

- [ ] **Step 1: README**

In the `floating` section (near the chip/overflow-marker paragraphs), add:

> A float pinned with zellij's `toggle-pane-pinned` carries a small `⌖` pin marker in its top-right corner. Since a pinned float stays on the real screen even when the floating layer is hidden, the bar keeps drawing it as an overlay box instead of folding it into a `◲` chip — only unpinned floats chip. No extra permission is needed: pin state is read from zellij's session-layout dump, which rides the existing `ReadApplicationState` grant.

- [ ] **Step 2: Append rule entry #20**

Append to `.claude/rules/zellij-plugin-development.md`:

```markdown
---

## 20. Pin state is readable only via `dump_session_layout_for_tab`'s KDL string (verified 0.44.3)

`PaneInfo` never carries a pane's pinned flag, and a pin toggle
(`toggle-pane-pinned`) arrives as a `PaneUpdate` with **no field changed** —
so pin state is invisible to the manifest and manifest-keyed caching misses
every toggle. The one read path (#119):

- `dump_session_layout_for_tab(tab_index)` needs only `ReadApplicationState`
  (zellij's `check_command_permission` puts `DumpSessionLayout` in that arm) —
  no new permission, so no #15 freeze.
- The returned `LayoutMetadata` does NOT carry pinned — parse the KDL
  **string**. Each float serializes under `tab > floating_panes > pane` with
  bare-integer `height/width/x/y` child nodes and a `pinned true` child node
  (absent when unpinned). Live `PaneGeom`s normalize to Fixed on placement
  (`pane_size.rs::apply_floating_pane_position`), so percent values
  effectively never occur in a live dump — defend anyway.
- The dump's cell values equal `PaneInfo.pane_x/pane_y/pane_columns/pane_rows`
  exactly (same cell space) — geometry is the join key back to pane ids (the
  KDL carries no ids; identical rects are ambiguous, degrade gracefully).
- The same document carries `swap_floating_layout` / `new_tab_template`
  template blocks with their own `floating_panes` — scope parsing to blocks
  under a `tab` node or you will ingest layout templates as live panes.
- Hidden floats ARE included: `screen.rs::get_layout_metadata` iterates
  `get_floating_panes()` unfiltered, and `hide_floating_panes` is just a tab
  attribute. And zellij keeps **pinned floats rendered while the layer is
  hidden** (`floating_panes/mod.rs::render` filters to `is_pinned` when
  `!show_panes`; e2e snapshot `pin_floating_panes.snap`), so a bar can
  truthfully keep overlaying them.
- The shim reads its reply back from stdin — rule #17 applies: the call
  panics off-wasm. Keep it behind a `#[cfg(test)]`-stubbed seam and inject
  the parsed state in native tests.
```

- [ ] **Step 3: Reconcile the #110 spec**

In `docs/superpowers/specs/2026-07-12-floating-panes-in-minimap-design.md`, find the non-goal/deferred line about pinned floats being blocked (`grep -n "pinned" docs/superpowers/specs/2026-07-12-floating-panes-in-minimap-design.md`) and update it to state that the distinction shipped separately via the session-layout-dump route — reference #119 and `2026-07-17-pinned-floats-design.md`. Keep it one sentence; do not rewrite the section.

- [ ] **Step 4: Record the verification results in the #119 spec**

In `docs/superpowers/specs/2026-07-17-pinned-floats-design.md` §3.5, replace the two-item "実装冒頭の検証ゲート" list's framing with the verified results (keep it brief): V1 = YES (`floating_panes/mod.rs:438-447` renders only `is_pinned` panes when `!show_panes`; e2e snapshot `pin_floating_panes.snap`), V2 = YES (`screen.rs` collects `get_floating_panes()` unfiltered; `hide_floating_panes` is a tab attribute only) — both verified 2026-07-17 by static reading of zellij v0.44.3, so the fallbacks were not needed.

- [ ] **Step 5: Commit**

```bash
git add README.md .claude/rules/zellij-plugin-development.md docs/superpowers/specs/
git commit -m "docs: pinned-float cue, dump-route trap entry, spec reconciliation (#119)"
```

---

### Task 9: Full gates

- [ ] **Step 1: Run every gate**

```bash
cargo fmt --check
CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib
cargo clippy --target wasm32-wasip1 --all-features --lib
cargo build --target wasm32-wasip1
cargo build --examples --target aarch64-apple-darwin
```

Expected: all clean/green. Fix and amend into the responsible commit (or a follow-up `fix:` commit) if anything fails.

No commit for this task unless fixes were needed.

---

## Self-Review Notes

- **Spec coverage:** §3.1/§3.2 → Tasks 1+6; §3.3 → Task 2; §3.4 → Tasks 4+5; §3.5 → Tasks 3+5+6+7 (V1/V2 pre-verified, fallbacks unused); §3.6 → degradation is embedded in Tasks 1/2/6 (empty-on-failure paths + tests); §4 → per-task tests; §5 non-goals respected (no `set_floating_pane_pinned`, no chip-level pin cue, no dump throttling); §6 file map matches.
- **Type consistency:** `pinned_by_tab: BTreeMap<usize, Vec<usize>>` everywhere; `pinned_ids(&[CellRect], &[PaneRect]) -> Vec<usize>`; render param `pinned_floats: &[usize]`; bar param `pinned_by_position: &BTreeMap<usize, Vec<usize>>`; ids cross to `u32` only at the wheel boundary (`is_pinned(position, id: u32)`).
- **Router untouched** — verified `TabPaneGeom` already models chips+overlay coexistence and `route_click` tries both.
- **Ship phase (outside this plan):** VHS screenshot of a pinned float (marker + hidden-layer overlay) for the PR body; visual glyph check may retune `PIN_MARKER_GLYPH` (`⌖` → `⚲`/`◉`) as a one-constant change.

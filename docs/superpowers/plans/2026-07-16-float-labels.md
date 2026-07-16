# Float Labels Implementation Plan (#120)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Draw a visible float's summarized title inside its overlay when the box is large enough, mirroring the tiled-pane label treatment.

**Architecture:** Reuse the tiled label vocabulary (`title::summarize` + display-width scan + `OverlayCell`) in a dedicated `float_overlay`, gated by the existing `LabelMode` (the L0–L4 ladder) and a conservative size gate. A new draw branch in `render()`'s paint loop paints the label over the float's own interior fill. Pure rendering — no new zellij permission, no config key.

**Tech Stack:** Rust → `wasm32-wasip1`; native unit tests via `CARGO_BUILD_TARGET=<host-triple> cargo test --lib`.

**Spec:** `docs/superpowers/specs/2026-07-16-float-labels-design.md`

**Conventions:**

- No `unwrap()`/`expect()` in production or tests; test fns return `Result` and use `?` where a fallible op appears (plain `assert!` fns are fine when nothing is fallible, matching the existing `render_*` tests).
- The renderer stays free of any zellij type; verify via `render()` string output (rule #7 paint-side testing).
- English code/comments. Iterator chains over loops; guard clauses.
- Build the plugin: `cargo build --target wasm32-wasip1`. CI-exact clippy: `cargo clippy --target wasm32-wasip1 --all-features --lib`.

---

## Context for the implementer

All work is in `src/minimap.rs`, inside `pub fn render(...)`. Key facts already in the file:

- `render(panes, palette, cols, text_rows, vinset, mode: LabelMode, badge, close, gradient, active, floats, suppressed_covers) -> String`. `pw = cols`. `mode` is the `LabelMode`.
- After `project_panes`, the file computes (≈ lines 979–1005):
  - `float_rects: &[PaneRect]` — the visible floats (`id`, `title`, `focused`); empty unless `floats` is `FloatLayer::Visible`.
  - `(float_grid, float_bounds)` from `project_floats_into(...)`. `float_grid: Vec<Option<usize>>` length `ph*pw` (pixel → float index). `float_bounds: Vec<PaneBox>` (index-aligned with `float_rects`); `PaneBox { px0, px1, py0, py1 }` are block pixels, half-open.
  - `float_ring: Vec<bool>` marks each float's border pixels **only where that float owns the pixel** (`float_grid[..] == Some(i)`).
- `let mut overlay = vec![None::<OverlayCell>; text_rows * pw];` (≈ line 1007) is the **tiled** label overlay. Add the float overlay right after it.
- `OverlayCell::Glyph(char, usize)` / `OverlayCell::Continuation` (enum ≈ line 369). The `usize` is a slice index — reuse it for the float index in `float_overlay` (kept separate from tiled `overlay`, so no index-space mixing).
- Constants near the top: `ACTIVE_FG: Rgb = (255,255,255)` (line 29), `FLOAT_SHADOW_BLEND` (line 39). Add the new constants beside these.
- `title::summarize(title: &str, available: usize, icons: bool) -> String`, `title::charwise_width(s: &str) -> usize`. `use unicode_width::UnicodeWidthChar;` already imported.
- `palette.color_for(id) -> Rgb` is a float's interior fill color.
- Emission helpers: `put_bg(&mut out, Rgb)`, `put_fg(&mut out, Rgb)`. Bold is `out.push_str("\x1b[1m"); out.push(ch); out.push_str("\x1b[22m");`.
- The paint loop is `for tr in 0..text_rows { for c in 0..pw { ... } }`. Existing corner-overlay branches (badge, close, chip, suppressed marker) each `find`/match and `continue`. The **suppressed-marker branch** ends just before `// Tiled-pane label vs. the visible float (#110).` / `match label_plan[tr * pw + c] { ... }`. Insert the float-label draw **between the suppressed-marker branch and that `match`** so a float label wins its cell as the top layer.
- Existing float-label-adjacent tests to copy style from: `render_overlays_a_visible_float_on_top_of_the_tiled_grid`, `render_marks_a_cover_pane_and_leaves_others_clean`. Helpers in the test module: `test_palette()`, `triple(c) -> String` (`"2;r;g;bm"`), `PaneRect::new(id, x, y, w, h, title, focused)`, `crate::floating::FloatLayer::{Visible, None}`, `GradientSpec::OFF`.

Float overlay coordinate note: floats use the **same virtual coordinate space** as the tiled `PaneRect`s (e.g. tiled `0,0,120,40`; a float `60,20,60,20`). At `cols=24, text_rows=4` (ph=8), a float `PaneRect::new(9, 0, 0, 120, 40, "cargo", false)` projects to the whole block (`px0=0,px1=24,py0=0,py1=8`) — big enough for a label. A float `PaneRect::new(9, 0, 0, 20, 8, "cargo", false)` projects to a small box (`≈ px 0..4, py 0..2`) — below the gate.

---

## Task 1: Size-gated float label — placement + draw

**Files:**

- Modify: `src/minimap.rs` (constants near line 39; placement after the tiled `overlay` decl ≈ line 1007; draw branch in the paint loop before the tiled `match label_plan`)
- Test: `src/minimap.rs` `#[cfg(test)] mod tests`

- [ ] **Step 1: Write the failing tests**

Add to the tests module:

```rust
    #[test]
    fn f_a_large_float_shows_its_label() {
        // A visible float spanning the whole block is large enough to carry its
        // summarized title; LabelMode::All labels every fitting float.
        let palette = test_palette();
        let tiled = [PaneRect::new(2, 0, 0, 120, 40, "sh", false)];
        let floats = [PaneRect::new(9, 0, 0, 120, 40, "cargo", false)];
        let out = render(
            &tiled, &palette, 24, 4, 0, LabelMode::All, None, Close::Off,
            GradientSpec::OFF, true, crate::floating::FloatLayer::Visible(&floats), &[],
        );
        assert!(out.contains('c') && out.contains('a') && out.contains('r'),
            "a large float shows its title glyphs");
        // The label sits on the float's interior fill, in white.
        assert!(out.contains(&triple(ACTIVE_FG)), "float label is white");
    }

    #[test]
    fn f_a_small_float_has_no_label() {
        // A float below the size gate stays color + ring only.
        let palette = test_palette();
        let tiled = [PaneRect::new(2, 0, 0, 120, 40, "sh", false)];
        let floats = [PaneRect::new(9, 0, 0, 20, 8, "cargo", false)];
        let out = render(
            &tiled, &palette, 24, 4, 0, LabelMode::All, None, Close::Off,
            GradientSpec::OFF, true, crate::floating::FloatLayer::Visible(&floats), &[],
        );
        // "cargo" summarizes to a command basename; none of its glyphs should
        // appear as a label on a box too small to hold one.
        assert!(!out.contains('c') && !out.contains('g'),
            "a small float shows no label");
    }

    #[test]
    fn f_a_focused_float_label_is_bold() {
        let palette = test_palette();
        let tiled = [PaneRect::new(2, 0, 0, 120, 40, "sh", false)];
        let floats = [PaneRect::new(9, 0, 0, 120, 40, "cargo", true)];
        let out = render(
            &tiled, &palette, 24, 4, 0, LabelMode::All, None, Close::Off,
            GradientSpec::OFF, true, crate::floating::FloatLayer::Visible(&floats), &[],
        );
        assert!(out.contains("\x1b[1m"), "a focused float's label is bold");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib "minimap::tests::f_a_"`
Expected: FAIL (no float-label output yet — glyphs absent / small-float assertion may pass trivially, large-float and bold fail).

- [ ] **Step 3: Add the size-gate constants**

Beside `FLOAT_SHADOW_BLEND` (≈ line 39):

```rust
/// Minimum interior width (columns inside the 1px ring, `fw - 2`) a visible
/// float overlay must have before it carries a label (#120) — below this a
/// truncated title is illegible, so the float stays color + ring only.
const FLOAT_LABEL_MIN_INNER_WIDTH: usize = 4;
/// Minimum pixel height a float overlay must span before it carries a label
/// (#120). A label is one text row (2px); the ring owns the top and bottom
/// pixel, so 6px (3 text rows) is the smallest box with a fully-interior row.
const FLOAT_LABEL_MIN_HEIGHT_PX: usize = 6;
```

- [ ] **Step 4: Build the float-label overlay**

Immediately after `let mut overlay = vec![None::<OverlayCell>; text_rows * pw];` (≈ line 1007), add:

```rust
    // Float labels (#120): a large-enough visible float carries its summarized
    // title, centered on an interior text row, painted over its own fill in the
    // paint loop below. Reuses the tiled label vocabulary (`summarize` +
    // display-width scan + `OverlayCell`) but in its own overlay so the tiled
    // `grid[i]` index space is never mixed. Gated by the same `LabelMode` the
    // tiled labels use (the L0–L4 ladder), plus a conservative size gate.
    let mut float_overlay = vec![None::<OverlayCell>; text_rows * pw];
    for (i, b) in float_bounds.iter().enumerate() {
        let want = match mode {
            LabelMode::None => false,
            LabelMode::Focused => float_rects[i].focused,
            LabelMode::All => true,
        };
        let inner = (b.px1 - b.px0).saturating_sub(2);
        if !want
            || inner < FLOAT_LABEL_MIN_INNER_WIDTH
            || b.py1 - b.py0 < FLOAT_LABEL_MIN_HEIGHT_PX
        {
            continue;
        }
        // Centered interior text row: the row nearest the box's vertical center
        // whose two pixels both sit inside the ring band [py0+1, py1-2]. The
        // height gate (≥ 6px) guarantees one exists.
        let first_interior = (b.py0 + 1).div_ceil(2);
        let last_interior = (b.py1 - 3) / 2;
        let row = ((b.py0 + b.py1) / 2 / 2).clamp(first_interior, last_interior);
        let label = crate::title::summarize(&float_rects[i].title, inner, false);
        let label_width = crate::title::charwise_width(&label);
        if label_width == 0 || row >= text_rows {
            continue;
        }
        // Center by display width; stop short of the right ring column.
        let start = b.px0 + 1 + inner.saturating_sub(label_width) / 2;
        let right_bound = b.px1 - 1;
        let cells: Vec<(usize, char, usize)> = label
            .chars()
            .filter_map(|ch| {
                UnicodeWidthChar::width(ch)
                    .filter(|w| *w >= 1)
                    .map(|w| (ch, w))
            })
            .scan(start, |col, (ch, w)| {
                let at = *col;
                *col += w;
                Some((at, ch, w))
            })
            .take_while(|(at, _, w)| at + w <= right_bound)
            .collect();
        // Only label where THIS float owns every covered cell (both pixels) —
        // an overlapping float on top keeps its own label/fill, no bleed-through.
        // Checked once here so the paint branch below needs no ownership test.
        let owned = cells.iter().all(|(at, _, w)| {
            (*at..*at + *w).all(|cc| {
                float_grid[2 * row * pw + cc] == Some(i)
                    && float_grid[(2 * row + 1) * pw + cc] == Some(i)
            })
        });
        if !owned {
            continue;
        }
        cells.iter().for_each(|&(at, ch, w)| {
            float_overlay[row * pw + at] = Some(OverlayCell::Glyph(ch, i));
            (at + 1..at + w).for_each(|cc| {
                float_overlay[row * pw + cc] = Some(OverlayCell::Continuation);
            });
        });
    }
```

- [ ] **Step 5: Add the draw branch in the paint loop**

Find the suppressed-marker branch (ends with `out.push(crate::suppressed::SUPPRESSED_MARKER_GLYPH); ... continue; } }`) and the following `// Tiled-pane label vs. the visible float (#110).` comment + `match label_plan[tr * pw + c] {`. **Between them**, insert:

```rust
            // A float label (#120) is the top layer: it paints over its own
            // float's interior on a cell that float fully owns (verified at
            // placement), white and bold when focused. A continuation cell emits
            // nothing — the leading wide glyph already advanced through it,
            // preserving the row width (#118). The cell is float-interior, and
            // floats take no drop-shadow, so the background is the flat fill.
            match float_overlay[tr * pw + c] {
                Some(OverlayCell::Glyph(ch, fi)) => {
                    put_bg(&mut out, palette.color_for(float_rects[fi].id));
                    put_fg(&mut out, ACTIVE_FG);
                    if float_rects[fi].focused {
                        out.push_str("\x1b[1m");
                        out.push(ch);
                        out.push_str("\x1b[22m");
                    } else {
                        out.push(ch);
                    }
                    continue;
                }
                Some(OverlayCell::Continuation) => continue,
                None => {}
            }
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib "minimap::tests::f_a_"`
Expected: PASS (3 tests).

- [ ] **Step 7: Full verification**

Run:

- `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib` → all pass (no regression in the existing float/label tests).
- `cargo clippy --target wasm32-wasip1 --all-features --lib` → clean.
- `cargo build --target wasm32-wasip1` → builds.

- [ ] **Step 8: Commit**

```bash
git add src/minimap.rs
git commit -m "feat(minimap): label a large visible float inside its overlay (#120)"
```

---

## Task 2: Regression pins — mode gating, wide glyphs, overlap

These behaviors are implemented in Task 1 (the `LabelMode` match, the `Continuation` handling, the ownership check). This task adds explicit regression tests so each is pinned — matching the project habit of fixing every young branch with a test.

**Files:**

- Test: `src/minimap.rs` `#[cfg(test)] mod tests`

- [ ] **Step 1: Write the tests**

```rust
    #[test]
    fn f_focused_mode_labels_only_the_focused_float() {
        let palette = test_palette();
        let tiled = [PaneRect::new(2, 0, 0, 120, 40, "sh", false)];
        // Two large side-by-side floats; only the right one is focused.
        let floats = [
            PaneRect::new(9, 0, 0, 60, 40, "vim", false),
            PaneRect::new(8, 60, 0, 60, 40, "cargo", true),
        ];
        let out = render(
            &tiled, &palette, 40, 4, 0, LabelMode::Focused, None, Close::Off,
            GradientSpec::OFF, true, crate::floating::FloatLayer::Visible(&floats), &[],
        );
        assert!(out.contains('c') && out.contains('g'),
            "the focused float (cargo) is labeled");
        assert!(!out.contains('v'),
            "the unfocused float (vim) is not labeled under Focused");
    }

    #[test]
    fn f_none_mode_labels_no_floats() {
        let palette = test_palette();
        let tiled = [PaneRect::new(2, 0, 0, 120, 40, "sh", false)];
        let floats = [PaneRect::new(9, 0, 0, 120, 40, "cargo", true)];
        let out = render(
            &tiled, &palette, 24, 4, 0, LabelMode::None, None, Close::Off,
            GradientSpec::OFF, true, crate::floating::FloatLayer::Visible(&floats), &[],
        );
        assert!(!out.contains('c') && !out.contains('g'),
            "LabelMode::None draws no float label");
    }

    #[test]
    fn f_wide_glyph_label_preserves_row_width() {
        // A CJK float title must not break the one-visible-cell-per-column
        // invariant: every rendered row carries exactly `pw` display columns.
        let palette = test_palette();
        let tiled = [PaneRect::new(2, 0, 0, 120, 40, "sh", false)];
        let floats = [PaneRect::new(9, 0, 0, 120, 40, "実行中", true)];
        let out = render(
            &tiled, &palette, 24, 4, 0, LabelMode::All, None, Close::Off,
            GradientSpec::OFF, true, crate::floating::FloatLayer::Visible(&floats), &[],
        );
        // Each line (minus the trailing reset) must be `pw` display columns wide.
        for line in out.lines() {
            let visible = strip_ansi(line);
            let width: usize = visible
                .chars()
                .map(|ch| UnicodeWidthChar::width(ch).unwrap_or(0))
                .sum();
            assert_eq!(width, 24, "row stays 24 display columns: {visible:?}");
        }
    }

    #[test]
    fn f_overlapping_floats_only_the_top_is_labeled() {
        let palette = test_palette();
        let tiled = [PaneRect::new(2, 0, 0, 120, 40, "sh", false)];
        // `top` (declared last → topmost) fully covers `bottom`'s label area.
        let floats = [
            PaneRect::new(8, 0, 0, 120, 40, "vim", false), // bottom
            PaneRect::new(9, 0, 0, 120, 40, "cargo", false), // top
        ];
        let out = render(
            &tiled, &palette, 24, 4, 0, LabelMode::All, None, Close::Off,
            GradientSpec::OFF, true, crate::floating::FloatLayer::Visible(&floats), &[],
        );
        assert!(out.contains('c') && out.contains('g'),
            "the topmost float (cargo) is labeled");
        assert!(!out.contains('v'),
            "the occluded float (vim) does not bleed its label through");
    }
```

- [ ] **Step 2: Add the `strip_ansi` test helper (if absent)**

Check whether a `strip_ansi` helper already exists in the tests module (`grep -n "fn strip_ansi" src/minimap.rs`). If not, add:

```rust
    /// Drop SGR escape sequences (`\x1b[...m`), leaving only visible glyphs —
    /// test-only, for measuring a rendered row's display width.
    fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(ch) = chars.next() {
            if ch == '\x1b' {
                for c in chars.by_ref() {
                    if c == 'm' {
                        break;
                    }
                }
            } else {
                out.push(ch);
            }
        }
        out
    }
```

- [ ] **Step 3: Run the tests to verify they pass**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib "minimap::tests::f_"`
Expected: PASS (all `f_*` tests). If `f_wide_glyph_label_preserves_row_width` fails, the continuation handling regressed — revisit Task 1 Step 5.

- [ ] **Step 4: Commit**

```bash
git add src/minimap.rs
git commit -m "test(minimap): pin float-label mode gating, wide glyphs, and overlap (#120)"
```

---

## Task 3: Visual demo

**Files:**

- Modify: `examples/render_floating.rs`

- [ ] **Step 1: Make the visible float large enough to carry a label**

In `examples/render_floating.rs`, the active tab (⌘2) already overlays two visible floats (`htop` focused, `logs` unfocused). Confirm at least one float box is large enough to show its label at the example's dimensions (the example renders full-height; `htop` at `8,8,44,22` over a 120-wide tab should clear the gate). If needed, widen/enlarge one float so its label renders, and update the module doc comment to mention that a large float now shows its title (#120).

- [ ] **Step 2: Render and eyeball the label**

Run: `cargo run --example render_floating --target aarch64-apple-darwin`
Expected: the focused float shows its title (e.g. `htop`) in white/bold inside the overlay; small floats stay unlabeled.

- [ ] **Step 3: Confirm the example still builds and commit**

Run: `cargo build --examples --target aarch64-apple-darwin` → builds.

```bash
git add examples/render_floating.rs
git commit -m "docs(examples): show a labeled float in the overlay (#120)"
```

(A VHS screenshot for the PR is captured by the controller after the tasks, not in a task step.)

---

## Definition of done

- All `minimap::tests::f_*` tests pass; no regression in the full `cargo test --lib` suite.
- `cargo clippy --target wasm32-wasip1 --all-features --lib` clean; `cargo build --target wasm32-wasip1` and `cargo build --examples` succeed.
- A large visible float shows its centered, size-gated title; small floats stay color + ring only; the label follows `LabelMode`, bolds when focused, never breaks row width, and never bleeds through an overlapping float.
- No new zellij permission, no new config key.

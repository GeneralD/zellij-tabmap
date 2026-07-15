# Float Label Occlusion + Ellipsis Truncation Cue Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A visible floating pane occludes the tiled labels it overlaps, and a label cut by the float shows a `…` cue over a subtle shadow, so the float reads as floating on top.

**Architecture:** Resolve every label cell's drawing decision in one pure, left-to-right pre-pass (`resolve_label_plan`) that treats each glyph as an atomic unit — so a wide (CJK) glyph at a float boundary never splits and the row width contract holds. `render` computes the float's per-cell coverage, runs the pre-pass, and the draw loop just consumes the plan. All new code is dependency-free in `minimap.rs`, unit-tested off-wasm.

**Tech Stack:** Rust, `wasm32-wasip1` target; native tests on the host triple.

---

## Design recap (from the committed spec)

- Occlusion: a tiled label cell the float covers is not drawn; the float/tiled fill paints it.
- Cue: a **single-column** label glyph the float cuts (a neighbour cell continues the label under the float) is drawn as `…` with its background darkened `LABEL_SHADOW_BLEND` (25%) toward black.
- Wide glyphs are never turned into `…` (a `…` is one column and would shorten the row); a wide glyph any of whose columns the float covers is dropped to fill on every column.
- `floating = hybrid` only; no float self-title; active + inactive tabs.

---

### Task 1: Pure `resolve_label_plan` pre-pass + `LabelDraw` + shadow const

**Files:**

- Modify: `src/minimap.rs` (add near the other private render helpers, e.g. just above `fn render`)
- Test: `src/minimap.rs` `mod tests`

- [ ] **Step 1: Write the failing tests**

Add to `mod tests`:

```rust
#[test]
fn label_plan_draws_clear_labels_and_skips_wide_continuations() {
    // Row of 6 cells: "ab" (two 1-col glyphs) at 0,1; a 2-col glyph at 2,3
    // (Glyph + Continuation); 4,5 empty. No float cover → every glyph draws.
    let o = |c| Some(OverlayCell::Glyph(c, 0));
    let overlay = vec![o('a'), o('b'), o('実'), Some(OverlayCell::Continuation), None, None];
    let cover = vec![false; 6];
    let plan = resolve_label_plan(&overlay, &cover, 6, 1);
    assert_eq!(
        plan,
        vec![
            LabelDraw::Char,  // a
            LabelDraw::Char,  // b
            LabelDraw::Char,  // 実 (leading)
            LabelDraw::Skip,  // 実 continuation
            LabelDraw::Fill,  // empty
            LabelDraw::Fill,
        ]
    );
}

#[test]
fn label_plan_occludes_a_covered_glyph() {
    // "ab": the float covers cell 1 ("b") → that cell falls to Fill; "a" is a
    // right boundary (its neighbour is covered AND carries a label) → Ellipsis.
    let o = |c| Some(OverlayCell::Glyph(c, 0));
    let overlay = vec![o('a'), o('b'), None, None];
    let cover = vec![false, true, false, false];
    let plan = resolve_label_plan(&overlay, &cover, 4, 1);
    assert_eq!(plan[0], LabelDraw::Ellipsis, "a is cut by the float → …");
    assert_eq!(plan[1], LabelDraw::Fill, "b is under the float");
}

#[test]
fn label_plan_left_boundary_is_an_ellipsis() {
    // The float covers cell 0 (carrying a label glyph); cell 1 ("b") is the
    // left boundary → Ellipsis.
    let o = |c| Some(OverlayCell::Glyph(c, 0));
    let overlay = vec![o('a'), o('b'), None, None];
    let cover = vec![true, false, false, false];
    let plan = resolve_label_plan(&overlay, &cover, 4, 1);
    assert_eq!(plan[0], LabelDraw::Fill);
    assert_eq!(plan[1], LabelDraw::Ellipsis);
}

#[test]
fn label_plan_does_not_cue_a_label_that_merely_abuts_the_float() {
    // "a" at 0; the float covers cell 1 but there is NO label there → "a" ends
    // naturally at the float, so no cue.
    let overlay = vec![Some(OverlayCell::Glyph('a', 0)), None, None, None];
    let cover = vec![false, true, false, false];
    let plan = resolve_label_plan(&overlay, &cover, 4, 1);
    assert_eq!(plan[0], LabelDraw::Char, "no label continues under the float");
}

#[test]
fn label_plan_never_ellipsizes_a_wide_glyph_and_keeps_width() {
    // A 2-col glyph at 0,1 whose trailing column the float covers → the whole
    // glyph drops to Fill on BOTH columns (never a 1-col `…`, so width holds).
    let overlay = vec![
        Some(OverlayCell::Glyph('実', 0)),
        Some(OverlayCell::Continuation),
        None,
        None,
    ];
    let cover = vec![false, true, false, false];
    let plan = resolve_label_plan(&overlay, &cover, 4, 1);
    assert_eq!(plan[0], LabelDraw::Fill);
    assert_eq!(plan[1], LabelDraw::Fill);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib minimap::tests::label_plan`
Expected: FAIL — `resolve_label_plan` / `LabelDraw` / `LABEL_SHADOW_BLEND` undefined.

- [ ] **Step 3: Implement the const, enum, and pre-pass**

Add near `ACTIVE_FG` / `INACTIVE_LABEL_BLEND`:

```rust
/// A `…` truncation cue's cell background is darkened this many percent toward
/// black (#110) — a subtle drop-shadow that reads as the float sitting on top of
/// the cut label. Chosen deliberately low so the cue stays understated.
const LABEL_SHADOW_BLEND: u8 = 25;
```

Add above `fn render` (with the other private render helpers):

```rust
/// Per-cell drawing decision for the tiled-pane labels once a visible float may
/// overlap them (#110). Resolved as a unit per glyph so a wide (CJK) glyph is
/// never split — the row's display width is preserved.
#[derive(Clone, Copy, PartialEq, Debug)]
enum LabelDraw {
    /// Draw the overlay glyph normally (the leading cell of a clear glyph).
    Char,
    /// Draw `…` over a shadow-darkened fill — a single-column boundary glyph the
    /// float cuts off.
    Ellipsis,
    /// Emit nothing — a continuation column of a drawn wide glyph.
    Skip,
    /// Fall through to the float overlay / tiled fill — a cell the float covers,
    /// or a covered wide glyph's column.
    Fill,
}

/// Resolve every cell's [`LabelDraw`] from the placed label `overlay` and the
/// float's per-cell coverage `cover` (both `text_rows * pw` long). Pure, so it is
/// unit-tested off-wasm. Left-to-right, a glyph is classified as a unit: a clear
/// glyph draws (leading `Char`, trailing `Skip`); a glyph any of whose columns
/// the float covers drops to `Fill` on every column; a clear **single-column**
/// glyph whose left/right neighbour still carries the label under the float
/// becomes `Ellipsis`. A wide glyph never becomes `Ellipsis` (a `…` is one column
/// and would shorten the row), so its cue degrades to plain occlusion.
fn resolve_label_plan(
    overlay: &[Option<OverlayCell>],
    cover: &[bool],
    pw: usize,
    text_rows: usize,
) -> Vec<LabelDraw> {
    let mut plan = vec![LabelDraw::Fill; text_rows * pw];
    for tr in 0..text_rows {
        let row = tr * pw;
        let mut c = 0;
        while c < pw {
            let width = match overlay[row + c] {
                Some(OverlayCell::Glyph(ch, _)) => {
                    UnicodeWidthChar::width(ch).unwrap_or(1).max(1)
                }
                // A None cell, or an orphan Continuation, stays Fill / advance one.
                _ => {
                    c += 1;
                    continue;
                }
            };
            let covered = |x: usize| x < pw && cover[row + x];
            let has_label = |x: usize| {
                x < pw
                    && matches!(
                        overlay[row + x],
                        Some(OverlayCell::Glyph(..)) | Some(OverlayCell::Continuation)
                    )
            };
            let covered_any = (0..width).any(|k| covered(c + k));
            if covered_any {
                for k in 0..width.min(pw - c) {
                    plan[row + c + k] = LabelDraw::Fill;
                }
            } else if width == 1 {
                let boundary = (has_label(c + 1) && covered(c + 1))
                    || (c > 0 && has_label(c - 1) && covered(c - 1));
                plan[row + c] = if boundary {
                    LabelDraw::Ellipsis
                } else {
                    LabelDraw::Char
                };
            } else {
                plan[row + c] = LabelDraw::Char;
                for k in 1..width.min(pw - c) {
                    plan[row + c + k] = LabelDraw::Skip;
                }
            }
            c += width.max(1);
        }
    }
    plan
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib minimap::tests::label_plan`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add src/minimap.rs
git commit -m "feat(minimap): resolve tiled-label draw plan under a visible float (#110)"
```

---

### Task 2: Wire the plan into `render` (occlude + `…`+shadow)

**Files:**

- Modify: `src/minimap.rs` (`render`)
- Test: `src/minimap.rs` `mod tests`

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` (string assertions on the real `render`, mirroring the existing float/label tests):

```rust
#[test]
fn render_occludes_a_tiled_label_under_a_visible_float() {
    // A tiled pane titled "cargo" fills the block; a visible float sits over its
    // middle. The float's color still paints, and the run "cargo" no longer
    // appears verbatim (its middle is occluded / cut to `…`).
    let palette = test_palette();
    let tiled = [PaneRect::new(0, 0, 0, 100, 40, "cargo", false)];
    let floats = [PaneRect::new(7, 40, 12, 20, 16, "f", false)];
    let out = render(
        &tiled, &palette, 16, 4, 0, LabelMode::All, None, Close::Off,
        GradientSpec::OFF, true, crate::floating::FloatLayer::Visible(&floats),
    );
    let stripped: String = visible_lines(&out).join("\n");
    assert!(!stripped.contains("cargo"), "the label is cut by the float, not shown whole");
    assert!(stripped.contains('…'), "the cut is marked with an ellipsis");
    // Width contract holds despite the occlusion / ellipsis.
    for line in visible_lines(&out) {
        assert_eq!(unicode_width::UnicodeWidthStr::width(line.as_str()), 16);
    }
}

#[test]
fn render_ellipsis_cue_darkens_its_cell() {
    // The `…` cell's background is the shadow blend of the pane fill, not the
    // plain fill — proof the drop-shadow is applied. Use a single tiled pane so
    // the fill is a known color.
    let palette = test_palette();
    let tiled = [PaneRect::new(0, 0, 0, 100, 40, "cargo", false)];
    let floats = [PaneRect::new(7, 40, 12, 20, 16, "f", false)];
    let out = render(
        &tiled, &palette, 16, 4, 0, LabelMode::All, None, Close::Off,
        GradientSpec::OFF, true, crate::floating::FloatLayer::Visible(&floats),
    );
    // The shadow bg for pane 0's fill.
    let fill = palette.color_for(0);
    let shadow = crate::color::mixed(fill, (0, 0, 0), LABEL_SHADOW_BLEND);
    let shadow_bg = format!("\x1b[48;2;{};{};{}m", shadow.0, shadow.1, shadow.2);
    assert!(out.contains(&shadow_bg), "the ellipsis cell carries the shadow background");
}
```

> **Note:** if the exact float geometry does not overlap the centered "cargo"
> label in the 16×4 block, adjust the float rect in BOTH tests so it covers the
> label's middle (verify by eye with `cargo run --example render_floating`), then
> keep the asserts. The contract (occlusion + `…` + width) is what matters.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib minimap::tests::render_occludes minimap::tests::render_ellipsis`
Expected: FAIL — labels currently draw over the float; no `…`, no shadow bg.

- [ ] **Step 3: Compute coverage + plan, and rewrite the label branch**

In `render`, right after `float_ring` is built (before the `ring`/`overlay` are declared is fine, but `overlay` must be fully populated first — so place this **after** the pane-projection loop that fills `overlay`, next to the `chip_layout` / `chip_row` setup):

```rust
    // Per text-cell float coverage (#110): a cell is "under the float" if either
    // of its two half-block pixels is covered. Empty `float_grid` → all false, so
    // the no-float path is unaffected.
    let cell_covered: Vec<bool> = (0..text_rows * pw)
        .map(|idx| {
            let (tr, c) = (idx / pw, idx % pw);
            float_grid.get(2 * tr * pw + c).copied().flatten().is_some()
                || float_grid
                    .get((2 * tr + 1) * pw + c)
                    .copied()
                    .flatten()
                    .is_some()
        })
        .collect();
    // Resolve, per cell, how each tiled label draws against the float overlay.
    let label_plan = resolve_label_plan(&overlay, &cell_covered, pw, text_rows);
```

Replace the existing continuation + glyph branches in the draw loop:

```rust
            // A continuation cell is already covered on screen by its wide
            // glyph's advance — emit nothing so cells stay in lockstep.
            if let Some(OverlayCell::Continuation) = overlay[tr * pw + c] {
                continue;
            }
            if let Some(OverlayCell::Glyph(ch, i)) = overlay[tr * pw + c] {
                // ... existing body ...
                out.push(ch);
                continue;
            }
```

with a match on the pre-resolved plan:

```rust
            // Tiled-pane label vs. the visible float (#110). The pre-resolved plan
            // says whether this cell draws its glyph, a `…` truncation cue over a
            // shadow, nothing (a wide glyph's continuation), or falls through to
            // the float / tiled fill.
            match label_plan[tr * pw + c] {
                LabelDraw::Skip => continue,
                LabelDraw::Char => {
                    if let Some(OverlayCell::Glyph(ch, i)) = overlay[tr * pw + c] {
                        // Active tab: white label on vivid fill. Focused pane bold.
                        // Inactive: text muted toward the fill so it recedes (#59).
                        let highlighted = active && panes[i].focused;
                        let label_fill =
                            fill_at(panes, palette, &sweeps, i, c as f32, 2.0 * tr as f32 + 0.5);
                        put_bg(&mut out, label_fill);
                        let label_fg = if active {
                            ACTIVE_FG
                        } else {
                            crate::color::mixed(ACTIVE_FG, label_fill, INACTIVE_LABEL_BLEND)
                        };
                        put_fg(&mut out, label_fg);
                        if highlighted {
                            out.push_str("\x1b[1m");
                            out.push(ch);
                            out.push_str("\x1b[22m");
                        } else {
                            out.push(ch);
                        }
                        continue;
                    }
                }
                LabelDraw::Ellipsis => {
                    if let Some(OverlayCell::Glyph(_, i)) = overlay[tr * pw + c] {
                        // `…` over the fill darkened toward black — the float's cut
                        // edge reads as a shadow the label slides under (#110). No
                        // bold: `…` is a marker, not the pane's own glyph.
                        let label_fill =
                            fill_at(panes, palette, &sweeps, i, c as f32, 2.0 * tr as f32 + 0.5);
                        let shadow = crate::color::mixed(label_fill, (0, 0, 0), LABEL_SHADOW_BLEND);
                        put_bg(&mut out, shadow);
                        let label_fg = if active {
                            ACTIVE_FG
                        } else {
                            crate::color::mixed(ACTIVE_FG, label_fill, INACTIVE_LABEL_BLEND)
                        };
                        put_fg(&mut out, label_fg);
                        out.push('…');
                        continue;
                    }
                }
                LabelDraw::Fill => {}
            }
```

> **Byte-identical guarantee:** with no visible float, `cell_covered` is all
> `false`, so `resolve_label_plan` returns `Char` for every glyph leading, `Skip`
> for every wide continuation, and `Fill` for the rest — the `Char`/`Skip` arms
> reproduce the pre-change bytes exactly, so
> `render_without_floats_is_byte_identical_to_none` still passes.

- [ ] **Step 4: Run the full lib tests**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib`
Expected: PASS (all prior tests green, including the byte-identical and width-contract ones, plus the 2 new render tests).

- [ ] **Step 5: Build wasm and examples**

Run: `cargo build --target wasm32-wasip1 && CARGO_BUILD_TARGET=aarch64-apple-darwin cargo build --examples`
Expected: both succeed.

- [ ] **Step 6: Commit**

```bash
git add src/minimap.rs
git commit -m "feat(minimap): a visible float occludes tiled labels, cutting them with a shadowed … (#110)"
```

---

### Task 3: Refresh the demo image and PR

**Files:**

- Modify: `assets/floating-demo.png` (regenerate)
- Modify: PR #112 body (mention occlusion + cue)

- [ ] **Step 1: Regenerate the demo screenshot**

The committed `assets/floating-demo.png` still shows the old label-over-float look. Re-render `render_floating` (which overlays a float on the "cargo/test/git" grid) and recapture, using the same VHS+ffmpeg recipe used to create it (FontSize 26, Width 1500, Height 200), then overwrite `assets/floating-demo.png`. Verify the new image shows `car…` / `…st` (occluded + ellipsis).

- [ ] **Step 2: Commit the refreshed asset**

```bash
git add assets/floating-demo.png
git commit -m "docs(assets): refresh floating demo with label occlusion + … cue (#110)"
```

- [ ] **Step 3: Update the PR body**

Add a sentence to the "What it does" section of PR #112: the visible-layer overlay now occludes the tiled labels it covers, marking a cut label with a shadowed `…` so the float reads as floating on top. Re-embed the refreshed image (same SHA-pinned raw URL scheme, new commit SHA).

- [ ] **Step 4: Push**

```bash
git push
```

---

## Self-Review

**1. Spec coverage:** occlusion (Task 1 `Fill` + Task 2 fall-through), `…`+shadow cue (Task 1 `Ellipsis` + Task 2 draw), no-false-cue (Task 1 test), wide-glyph width safety (Task 1 test + `resolve_label_plan` unit rule), byte-identical no-float (Task 2 note + existing test), scope hybrid-only (unchanged — floats only reach `render` under hybrid). All covered.

**2. Placeholder scan:** none — every step has concrete code or a concrete command. The Task 2 float-geometry note is a verification instruction, not a placeholder.

**3. Type consistency:** `LabelDraw{Char,Ellipsis,Skip,Fill}`, `resolve_label_plan(&[Option<OverlayCell>], &[bool], usize, usize) -> Vec<LabelDraw>`, `LABEL_SHADOW_BLEND: u8`, `crate::color::mixed(from, to, percent: u8)` — consistent across tasks and with the existing `OverlayCell` / `mixed` signatures.

**4. Ambiguity:** the `…` fires only for single-column glyphs; wide glyphs degrade to plain occlusion (explicit in the enum doc + Task 1 test). Shadow = `mixed(fill, black, 25)`. Boundary = neighbour cell is float-covered AND still carries the label.

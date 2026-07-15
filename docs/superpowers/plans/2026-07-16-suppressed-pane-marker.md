# Suppressed-Pane Awareness Marker Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Mark, in the active tab's minimap, each tiled pane that hides a suppressed pane in its slot — a single corner glyph signalling "something is suppressed behind this" (#118).

**Architecture:** Follows the #110 floating split exactly. A dependency-free pure layer (a new `projection::project_suppressed` + a new `src/suppressed.rs` cover-matcher) decides *which* panes get marked; `minimap::render` stamps one glyph in the cover pane's block-local bottom-right cell. Data/decision is unit-tested off-wasm (rule #8); paint is unit-tested on the dependency-free renderer (rule #7). No zellij type leaks into the pure layer; no new permission.

**Tech Stack:** Rust → `wasm32-wasip1`; native unit tests on the host triple (`CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib`).

---

## Design reference

`docs/superpowers/specs/2026-07-16-suppressed-pane-marker-design.md`. Read §3 (decisions), §4 (design), §7 (spike gate) before starting.

**Approach A (this plan): per-pane cover marker.** Task 1 (the P0 spike) gates it: if the spike shows suppressed geometry is unreadable or does not overlap its cover, STOP and revise this plan for Approach B (a tab-block corner count marker) per design §7 — Tasks 2–3 (projection + cover match) are then replaced by a simple per-tab count, and Task 4 draws in the block corner like a hidden-float chip instead of a pane corner.

## File Structure

- `src/projection.rs` — add `is_suppressed_terminal` predicate + `project_suppressed` (mirrors `is_floating_terminal` / `project_floating`). Responsibility: turn `PaneInfo` → `PaneRect` for the suppressed set.
- `src/suppressed.rs` — **new**, dependency-free (mirrors `src/floating.rs`). Responsibility: the pure cover-matcher `cover_ids` + the marker glyph constant. No zellij types → unit-tested off-wasm.
- `src/lib.rs` — register `pub mod suppressed;`; build the active tab's cover-id list; pass it to `paint::bar`.
- `src/minimap.rs` — `render` gains a `suppressed_covers: &[usize]` param and stamps the marker; renderer stays zellij-type-free.
- `src/tab_block.rs` — `assemble` threads the per-tab cover list to `render`.
- `src/paint.rs` — `bar` gains `suppressed_covers_by_position` and forwards the active tab's list.
- `examples/render_floating.rs` — add a suppressed marker to the sample so the effect is demonstrable.

---

## Phase 0 — Spike (empirical, no code)

### Task 1: Confirm suppressed geometry with the eprintln oracle + expect PTY

**Files:** temporary `src/lib.rs` edit (eprintln oracle, **never committed**)

- [ ] **Step 1: Add the temporary oracle**

In `update()`, before any `permitted`/state gate, log every pane's suppressed flag and geometry (adapt the #110 `DBG110` oracle):

```rust
for (pos, panes) in &self.panes_by_tab {
    for p in panes {
        eprintln!(
            "DBG118 tab={pos} id={} suppressed={} floating={} plugin={} focused={} x={} y={} w={} h={} title={:?}",
            p.id, p.is_suppressed, p.is_floating, p.is_plugin, p.is_focused,
            p.pane_x, p.pane_y, p.pane_columns, p.pane_rows, p.title
        );
    }
}
```

(Match the field/accessor names to the actual per-tab pane store in `lib.rs`; the #110 oracle at `docs/superpowers/plans/2026-07-12-floating-panes-in-minimap.md:62` is the template.)

- [ ] **Step 2: Drive a suppressed pane**

Pre-seed `permissions.kdl` (rule #1), start the isolated session with the expect PTY (rule #4), then from an outside shell create a suppressed pane by opening the scrollback editor in a tiled pane:

```bash
zellij --session tabmap-verify-118 action edit-scrollback   # suppresses the focused tiled pane behind $EDITOR
```

- [ ] **Step 3: Read the oracle**

Tail `zellij.log` (`$(getconf DARWIN_USER_TEMP_DIR)/zellij-<uid>/zellij-log/zellij.log`, rule #5) for `DBG118` lines.

- [ ] **Step 4: Decide the branch and record the finding**

Judge and write the conclusion into this plan (a `### Spike result` block) and the design doc §7:

1. Does the suppressed pane appear with `is_suppressed=true` and **valid** `pane_x/y/columns/rows`?
2. Is its rect **contained by** (or equal to) exactly one visible tiled pane's rect (its cover)?

- **Both yes → Approach A.** Proceed to Task 2.
- **Either no → Approach B.** Stop; revise this plan (see "Design reference" above) before writing code.

- [ ] **Step 5: Revert the oracle**

`git checkout src/lib.rs`. **Never commit the oracle.** Carry only the recorded conclusion forward.

---

## Phase 1 — Pure layer (Approach A; TDD, gated on Task 1)

### Task 2: `project_suppressed` + `is_suppressed_terminal`

**Files:**

- Modify: `src/projection.rs`
- Test: `src/projection.rs` (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test**

Add to `mod tests` in `src/projection.rs` (uses the existing `content_pane` helper):

```rust
#[test]
fn project_suppressed_keeps_only_suppressed_terminals() {
    let panes = [
        PaneInfo { is_plugin: true, is_suppressed: true, ..Default::default() }, // plugin suppress → dropped
        PaneInfo { is_floating: true, ..Default::default() },                    // float → dropped
        content_pane(0, 1, 80, 24, true),                                        // tiled → dropped
        PaneInfo {
            id: 7,
            is_suppressed: true,
            pane_x: 40, pane_y: 1, pane_columns: 40, pane_rows: 24,
            title: "sh".to_string(),
            ..Default::default()
        },
    ];
    let rects = project_suppressed(&panes);
    assert_eq!(rects.len(), 1);
    assert_eq!(rects[0].id, 7);
    assert_eq!((rects[0].x, rects[0].y, rects[0].w, rects[0].h), (40, 1, 40, 24));
}
```

- [ ] **Step 2: Run it, verify it fails**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib projection::tests::project_suppressed_keeps_only_suppressed_terminals`
Expected: FAIL — `project_suppressed` / `is_suppressed_terminal` not found.

- [ ] **Step 3: Implement**

Add after `is_floating_terminal` / `project_floating` in `src/projection.rs`:

```rust
/// Whether a pane is a **suppressed** terminal pane — one hidden behind the pane
/// that replaced its slot (#118). The suppressed sibling of [`is_tiled_terminal`]
/// / [`is_floating_terminal`]: keeps `is_suppressed` panes but drops plugin ones
/// (a plugin-driven suppress is chrome, not the user's content).
pub fn is_suppressed_terminal(pane: &PaneInfo) -> bool {
    pane.is_suppressed && !pane.is_plugin
}

/// Project a tab's **suppressed** panes into renderer rectangles — the parallel
/// of [`project`] / [`project_floating`] for the suppressed layer (#118). Carries
/// id + geometry so cover-matching ([`crate::suppressed::cover_ids`]) can find
/// which visible tiled pane hides each. Title/focus ride along for parity with
/// the other projectors; only id + geometry are used downstream.
pub fn project_suppressed(panes: &[PaneInfo]) -> Vec<PaneRect> {
    panes
        .iter()
        .filter(|pane| is_suppressed_terminal(pane))
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
```

- [ ] **Step 4: Run it, verify it passes**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib projection::tests::project_suppressed_keeps_only_suppressed_terminals`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/projection.rs
git commit -m "feat(projection): project a tab's suppressed panes (#118)"
```

### Task 3: `src/suppressed.rs` — the cover-matcher

**Files:**

- Create: `src/suppressed.rs`
- Modify: `src/lib.rs` (add `pub mod suppressed;`)
- Test: `src/suppressed.rs` (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Create the module with a failing test**

Create `src/suppressed.rs`:

```rust
//! Dependency-free suppressed-pane awareness layer (#118): match each suppressed
//! pane to the visible tiled pane that hides it, so the bar can mark that cover
//! pane's corner. No zellij types, so the whole module is unit-tested off-wasm
//! (rule #8), exactly like `floating`/`minimap`/`scroll`.

use crate::minimap::PaneRect;

/// The marker glyph — a small quadrant marker distinct from the hidden-float
/// chip [`crate::floating::CHIP_GLYPH`] (`◲`): a different rotation, and it rides
/// an individual pane's corner rather than the tab block's, so the two
/// "hidden-thing" markers never conflate. Single display column.
pub const SUPPRESSED_MARKER_GLYPH: char = '◳';

/// The tiled pane ids that hide at least one suppressed pane in their slot (#118).
/// A tiled pane `t` **covers** suppressed `s` when `t`'s rect contains `s`'s rect
/// (same slot, or `s` nested inside `t`) — the geometry the P0 spike confirmed for
/// edit-scrollback. Returned in `tiled` order and naturally deduped (each cover is
/// yielded at most once), so the renderer marks a pane once no matter how many
/// panes hide behind it — awareness is presence, not count.
pub fn cover_ids(suppressed: &[PaneRect], tiled: &[PaneRect]) -> Vec<usize> {
    tiled
        .iter()
        .filter(|t| suppressed.iter().any(|s| covers(t, s)))
        .map(|t| t.id)
        .collect()
}

/// Whether tiled rect `t` fully contains suppressed rect `s` (half-open extents).
fn covers(t: &PaneRect, s: &PaneRect) -> bool {
    t.x <= s.x && t.y <= s.y && s.x + s.w <= t.x + t.w && s.y + s.h <= t.y + t.h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(id: usize, x: u32, y: u32, w: u32, h: u32) -> PaneRect {
        PaneRect::new(id, x, y, w, h, "p", false)
    }

    #[test]
    fn marks_the_tiled_pane_that_covers_a_suppressed_one() {
        // A suppressed pane sits in the exact slot of tiled pane 2 (same rect);
        // tiled pane 1 is elsewhere. Only pane 2 is a cover.
        let tiled = [rect(1, 0, 0, 60, 40), rect(2, 60, 0, 60, 40)];
        let suppressed = [rect(9, 60, 0, 60, 40)];
        assert_eq!(cover_ids(&suppressed, &tiled), vec![2]);
    }

    #[test]
    fn marks_each_cover_once_regardless_of_count() {
        // Two suppressed panes both nested inside tiled pane 2 → pane 2 marked once.
        let tiled = [rect(1, 0, 0, 60, 40), rect(2, 60, 0, 60, 40)];
        let suppressed = [rect(9, 60, 0, 60, 40), rect(10, 70, 5, 10, 10)];
        assert_eq!(cover_ids(&suppressed, &tiled), vec![2]);
    }

    #[test]
    fn no_cover_when_geometry_does_not_overlap() {
        let tiled = [rect(1, 0, 0, 60, 40)];
        let suppressed = [rect(9, 200, 200, 10, 10)];
        assert!(cover_ids(&suppressed, &tiled).is_empty());
    }
}
```

Register the module — add near the other `pub mod` lines in `src/lib.rs`:

```rust
pub mod suppressed;
```

- [ ] **Step 2: Run it, verify it passes** (implementation is written inline above)

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib suppressed::`
Expected: PASS (3 tests).

- [ ] **Step 3: Commit**

```bash
git add src/suppressed.rs src/lib.rs
git commit -m "feat(suppressed): match suppressed panes to their cover pane (#118)"
```

---

## Phase 2 — Renderer (Approach A; TDD)

### Task 4: `render` stamps the marker

**Files:**

- Modify: `src/minimap.rs` (`render` signature + one paint branch; update in-file test call sites)
- Test: `src/minimap.rs` (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Add the param and update every existing `render(...)` call to pass `&[]`**

Append a parameter to `render` (after `floats`):

```rust
pub fn render(
    panes: &[PaneRect],
    palette: &Palette,
    cols: usize,
    text_rows: usize,
    vinset: usize,
    mode: LabelMode,
    badge: Option<&str>,
    close: Close,
    gradient: GradientSpec,
    active: bool,
    floats: crate::floating::FloatLayer<'_>,
    suppressed_covers: &[usize],
) -> String {
```

Every existing caller in `src/minimap.rs` tests (search `render(` — the calls ending in `FloatLayer::…,`) gains a trailing `&[],`. Do this first so the file compiles before the new behavior lands.

- [ ] **Step 2: Write the failing test**

Add to `mod tests` in `src/minimap.rs` (mirrors the #116 render test — match the RGB triple without the fg/bg prefix):

```rust
#[test]
fn render_marks_a_cover_pane_and_leaves_others_clean() {
    // Two side-by-side tiled panes; pane 3 covers a suppressed pane. Pane 3's
    // corner shows the marker glyph in pane 3's ring shade; pane 2 does not.
    let palette = test_palette();
    let panes = [
        PaneRect::new(2, 0, 0, 60, 40, "a", false),
        PaneRect::new(3, 60, 0, 60, 40, "b", false),
    ];
    let render_with = |covers: &[usize]| {
        render(
            &panes, &palette, 24, 4, 0, LabelMode::None, None, Close::Off,
            GradientSpec::OFF, true, crate::floating::FloatLayer::None, covers,
        )
    };
    let triple = |c: (u8, u8, u8)| format!("2;{};{};{}m", c.0, c.1, c.2);
    let marker_fg = triple(palette.ring_for(3));

    let marked = render_with(&[3]);
    assert!(
        marked.contains(crate::suppressed::SUPPRESSED_MARKER_GLYPH.encode_utf8(&mut [0u8; 4]).as_ref()),
        "a cover pane shows the suppressed marker glyph"
    );
    assert!(marked.contains(&marker_fg), "marker uses the cover pane's ring shade");

    let clean = render_with(&[]);
    assert!(
        !clean.contains(crate::suppressed::SUPPRESSED_MARKER_GLYPH.encode_utf8(&mut [0u8; 4]).as_ref()),
        "no marker without a cover"
    );
}
```

- [ ] **Step 3: Run it, verify it fails**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib minimap::tests::render_marks_a_cover_pane_and_leaves_others_clean`
Expected: FAIL — glyph absent (param is accepted but unused).

- [ ] **Step 4: Implement the marker**

After `chip_layout` / `chip_row` are computed (around line 1079), add the marker-cell list. `bounds` (from `project_panes`) is index-aligned with `panes`:

```rust
// Suppressed-pane awareness markers (#118): a cover pane — one hiding a
// suppressed pane in its slot — gets ONE glyph in its block-local bottom-right
// cell, signalling "something is suppressed behind this". `bounds[i]` is the
// same PaneBox `pane_at_cell` uses, so the marker lands on the exact rectangle
// drawn. Presence only (one per cover). A cell already reserved by a hidden-float
// chip is skipped — the chip owns its corner (#110).
let suppressed_marks: Vec<(usize, usize, usize)> = panes
    .iter()
    .zip(bounds.iter())
    .filter(|(p, _)| suppressed_covers.contains(&p.id))
    .filter(|(_, b)| b.px1 > b.px0 && b.py1 > b.py0)
    .map(|(p, b)| (b.px1 - 1, (b.py1 - 1) / 2, p.id))
    .filter(|(col, tr, _)| !(*tr == chip_row && chip_layout.iter().any(|(cc, _)| cc == col)))
    .collect();
```

Then, in the cell loop, **after** the hidden-float chip branch (`if tr == chip_row { … }`, ends ~line 1362) and **before** the `match label_plan[…]` label branch (~line 1369), add:

```rust
// A suppressed-pane marker overrides a same-cell label so the awareness cue
// is never overprinted (like the chip above). Drawn in the cover pane's ring
// shade so it contrasts with its own fill (reusing the #47/#116 ring
// vocabulary). Markers are passed for the active tab only, so no inactive blend.
if let Some((_, _, cover_id)) = suppressed_marks.iter().find(|(mc, mr, _)| *mc == c && *mr == tr) {
    let fill = pixel_color(&grid, &ring, panes, palette, &sweeps, pw, c, 2 * tr);
    match fill {
        Some(f) => put_bg(&mut out, f),
        None => put_default_bg(&mut out),
    }
    put_fg(&mut out, palette.ring_for(*cover_id));
    out.push(crate::suppressed::SUPPRESSED_MARKER_GLYPH);
    continue;
}
```

- [ ] **Step 5: Run it, verify it passes; run the whole lib suite**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib`
Expected: PASS (new test green, all prior tests still green — the `&[]` call-site updates keep the no-marker path byte-identical).

- [ ] **Step 6: Commit**

```bash
git add src/minimap.rs
git commit -m "feat(minimap): mark a cover pane that hides a suppressed pane (#118)"
```

---

## Phase 3 — Wiring (TDD where a seam exists; else compile + suite)

### Task 5: Thread the cover list through `tab_block::assemble` and `paint::bar`

**Files:**

- Modify: `src/tab_block.rs` (`assemble` gains `suppressed_covers: &[usize]`, forwards to `render`)
- Modify: `src/paint.rs` (`bar` gains `suppressed_covers_by_position: &BTreeMap<usize, Vec<usize>>`, forwards the per-position slice)

- [ ] **Step 1: `tab_block::assemble`**

Add `suppressed_covers: &[usize]` to `assemble`'s parameter list and pass it as the new final argument to `minimap::render(...)`. Update `assemble`'s in-file test/callers to pass `&[]`.

- [ ] **Step 2: `paint::bar`**

Add `suppressed_covers_by_position: &BTreeMap<usize, Vec<usize>>` to `bar`'s signature (next to `floats_by_position`). Inside the tab loop, resolve this tab's slice the same way `floats` is resolved (`src/paint.rs:86`):

```rust
let suppressed_covers = suppressed_covers_by_position
    .get(&hit.position)
    .map(Vec::as_slice)
    .unwrap_or(&[]);
```

Pass `suppressed_covers` into the `tab_block::assemble(...)` call as its new argument. Update `bar`'s in-file tests to pass `&BTreeMap::new()`.

- [ ] **Step 3: Compile + run the suite**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib`
Expected: PASS (pure threading; empty maps keep every tab byte-identical).

- [ ] **Step 4: Commit**

```bash
git add src/tab_block.rs src/paint.rs
git commit -m "feat(paint): thread suppressed cover ids to the renderer (#118)"
```

### Task 6: Build the active-tab cover list in `lib.rs`

**Files:**

- Modify: `src/lib.rs` (build `suppressed_covers_by_position`; pass to `paint::bar`)

- [ ] **Step 1: Build the map for the active tab only**

Where `floats_by_position` is built (`src/lib.rs:287`) and the active tab's panes are in scope, add a sibling map. For the **active** tab position only: project its suppressed panes, project its tiled panes, match:

```rust
// Suppressed-pane awareness markers, active tab only (#118, design §3). Empty
// for every other tab, so only the tab you're on shows the cue.
let suppressed_covers_by_position: BTreeMap<usize, Vec<usize>> = active_tab(&self.tabs)
    .and_then(|t| self.panes_by_tab.get(&t.position).map(|panes| (t.position, panes)))
    .map(|(pos, panes)| {
        let covers = suppressed::cover_ids(
            &projection::project_suppressed(panes),
            &projection::project(panes),
        );
        BTreeMap::from([(pos, covers)])
    })
    .unwrap_or_default();
```

(Match `active_tab`, `self.tabs`, and the per-tab pane store to `lib.rs`'s actual names — the same sources `floats_by_position` reads. `active_tab` and `project` already exist in `projection`.)

- [ ] **Step 2: Pass it to `paint::bar`**

Add `&suppressed_covers_by_position` to the `paint::bar(...)` call (`src/lib.rs:349`), positioned to match the new `bar` signature.

- [ ] **Step 3: Build wasm + run the suite**

Run: `cargo build --target wasm32-wasip1` then `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib`
Expected: both succeed.

- [ ] **Step 4: CI-exact clippy**

Run: `cargo clippy --target wasm32-wasip1 --all-features --lib`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add src/lib.rs
git commit -m "feat: surface suppressed-pane markers for the active tab (#118)"
```

---

## Phase 4 — Visual demo

### Task 7: Show the marker in the render example

**Files:**

- Modify: `examples/render_floating.rs` (add a suppressed cover to a tab) — or add a focused `examples/render_suppressed.rs` if the floating example is already crowded.

- [ ] **Step 1: Add a suppressed marker to the sample**

Extend the example so one tiled pane in a tab is a cover (pass its id through the new `suppressed_covers_by_position` argument to `paint::bar`). Keep the existing floating demo intact.

- [ ] **Step 2: Render + eyeball**

Run: `cargo run --example render_floating --target aarch64-apple-darwin`
Expected: the cover pane shows a `◳` in its bottom-right corner; other panes are unchanged.

- [ ] **Step 3: Commit**

```bash
git add examples/render_floating.rs
git commit -m "docs(examples): demo the suppressed-pane marker (#118)"
```

---

## Self-Review

- **Spec coverage:** design §4.1 → Task 2; §4.2 → Task 3; §4.3 (glyph, precedence vs chip, skip-when-no-cell) → Task 4; §3 active-tab-only + no config/permission → Task 6 (map built for the active tab only; no new param on any host call); §7 spike → Task 1. Covered.
- **Placeholder scan:** none — every code step shows real code; wiring steps name exact files, insertion points, and existing anchors to match.
- **Type consistency:** `project_suppressed` (Task 2) → `suppressed::cover_ids` (Task 3) → `render(..., suppressed_covers: &[usize])` (Task 4) → `assemble`/`bar` threading (Task 5) → `suppressed_covers_by_position: BTreeMap<usize, Vec<usize>>` (Task 6). `SUPPRESSED_MARKER_GLYPH`, `ring_for`, `PaneBox { px0, px1, py0, py1 }`, `chip_row`, `chip_layout` all referenced as defined.
- **Gate:** Tasks 2–7 assume the Task 1 spike confirmed Approach A. If it did not, revise per "Design reference" before proceeding.

# Overflow-Chip Reveal Implementation Plan (#113)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a click on the `+k` overflow chip marker reveal-and-focus the first folded hidden float, so every overflow float becomes reachable from the bar (≤ 2 clicks).

**Architecture:** Repurpose `router::chip_marker_at` into `chip_marker_target_at`: it resolves the marker cell to the FIRST folded float's id (`hidden_floats[len - k]`, deriving the fold boundary from the marker's own `k`). `route_click`'s marker branch returns the existing `ClickIntent::FocusFloatingPane(id)` instead of `NoOp`; the `lib.rs` dispatch (`focus_terminal_pane(id, true, false)`) already un-hides the whole layer, making the remaining folded floats individually clickable overlay boxes. No new intent, no plugin state, no permission, no config, no visual change.

**Tech Stack:** Rust → `wasm32-wasip1`; native unit tests via `CARGO_BUILD_TARGET=<host-triple> cargo test --lib`.

**Spec:** `docs/superpowers/specs/2026-07-16-overflow-chip-reveal-design.md`

**Conventions:**

- No `unwrap()`/`expect()` in production or tests; plain `assert!`/`assert_eq!` test fns are fine when nothing is fallible (matching the existing `route_click_*` tests).
- English code/comments. Iterator chains and guard clauses over imperative branching.
- Build the plugin: `cargo build --target wasm32-wasip1`. CI-exact clippy: `cargo clippy --target wasm32-wasip1 --all-features --lib`.

---

## Context for the implementer

All code work is in `src/router.rs` (plus doc comments in `src/floating.rs`). Key facts already in the files:

- `chip_cells(cols, count)` (`src/floating.rs:98`): when `count > cols`, lays out `PlusK(k)` at block-local col 0 with `k = count - shown`, `shown = cols - 1`, and `Float(0..shown)` chips at cols `1..=shown`. So the floats folded into the marker are indices `shown..count`, and the **first folded index = `count - k`**. With `cols == 1` everything folds (`shown = 0`, `k = count`, first folded index 0).
- `chip_marker_k(cols, text_rows, count, col, row)` (`src/floating.rs`, just below `chip_index_at_cell`): returns `Some(k)` when `(col, row)` is the marker cell (bottom text row only). Its doc comment currently says the marker "selects nothing" and describes the no-op consumption — that text must be updated by this task.
- `router::TabPaneGeom.hidden_floats: Vec<usize>` holds the hidden float ids **in the same order `chip_cells` lays them out** — this is the index→id table (`float_chip_at` uses it the same way).
- `router::chip_marker_at` (`src/router.rs:87`) resolves the per-tab geometry and returns `chip_marker_k(...)`. `route_click` (`src/router.rs:~220`) consumes `is_some()` as `ClickIntent::NoOp`, with a comment block explicitly deferring reachability to #113.
- `ClickIntent::FocusFloatingPane(usize)` already exists and is dispatched in `lib.rs` as `focus_terminal_pane(id as u32, true, false)` — `should_float_if_hidden = true` un-hides the layer (`.claude/rules/zellij-plugin-development.md` #18: hidden float ids stay valid).
- Existing tests to model on: `route_click_treats_the_overflow_marker_as_a_no_op` (`src/router.rs:~538`) builds exactly the overflow fixture this feature changes — `hit_active(0, 10, 3)`, `geom(10, 3, &[(5, 0, 0, 80, 24)])`, `hidden_floats = vec![101..105]`, marker at absolute col 10 on `chip_row = (MIN_ROWS - 1) as isize`. That test will be REPLACED (renamed + new expectation), not kept alongside.

---

## Task 1: Marker click → reveal-and-focus the first folded float

**Files:**

- Modify: `src/router.rs` (rename/repurpose `chip_marker_at` ≈ line 87; `route_click` marker branch ≈ line 220; module-header mention of the marker ≈ line 83 if present)
- Modify: `src/floating.rs` (doc comments only: `chip_marker_k`, and any `Chip::PlusK` doc text claiming the marker is decorative/unselectable)
- Test: `src/router.rs` `#[cfg(test)] mod tests`

- [ ] **Step 1: Rewrite the marker test to the new expectation (failing) and add the all-folded edge test**

Replace the whole `route_click_treats_the_overflow_marker_as_a_no_op` test with:

```rust
    #[test]
    fn route_click_reveals_the_first_folded_float_from_the_marker() {
        // A narrow block (3 cols) with 5 hidden floats overflows the chip run:
        // `chip_cells(3, 5)` = [ +3 marker @0, Float0 @1, Float1 @2 ], folding
        // ids 103,104,105 behind the marker. Clicking the marker resolves to the
        // FIRST folded float (#113): focusing it un-hides the whole layer, so
        // the rest become individually clickable overlay boxes. The marker keeps
        // shielding the tiled pane beneath it, and real chips still resolve.
        let tab_layout = vec![hit_active(0, 10, 3)];
        let mut g = geom(10, 3, &[(5, 0, 0, 80, 24)]); // tiled pane fills the block
        g.hidden_floats = vec![101, 102, 103, 104, 105];
        let tab_panes: BTreeMap<usize, TabPaneGeom> = [(0usize, g)].into_iter().collect();
        let chip_row = (MIN_ROWS - 1) as isize;
        // Marker at block-local col 0 → absolute col 10: first folded float
        // (index 5 - 3 = 2 → id 103), never FocusPane(5).
        assert_eq!(
            route_click(None, &[], &tab_layout, &tab_panes, chip_row, 10),
            ClickIntent::FocusFloatingPane(103),
            "the +k marker reveals the first folded float",
        );
        // A real float chip still resolves (block-local col 2 → absolute col 12).
        assert_eq!(
            route_click(None, &[], &tab_layout, &tab_panes, chip_row, 12),
            ClickIntent::FocusFloatingPane(102),
        );
    }

    #[test]
    fn route_click_marker_resolves_when_every_float_folds() {
        // A 1-col block folds EVERY hidden float into the marker:
        // `chip_cells(1, 3)` = [ +3 @0 ], no Float chips. The first folded index
        // is then 0 (`len - k = 3 - 3`), so the marker resolves to the first
        // hidden float — not to the tiled pane beneath.
        let tab_layout = vec![hit_active(0, 10, 1)];
        let mut g = geom(10, 1, &[(5, 0, 0, 80, 24)]);
        g.hidden_floats = vec![201, 202, 203];
        let tab_panes: BTreeMap<usize, TabPaneGeom> = [(0usize, g)].into_iter().collect();
        let chip_row = (MIN_ROWS - 1) as isize;
        assert_eq!(
            route_click(None, &[], &tab_layout, &tab_panes, chip_row, 10),
            ClickIntent::FocusFloatingPane(201),
        );
    }
```

Also grep for other references to the old test name / no-op marker expectation in test comments (`grep -n "no_op\|no-op" src/router.rs`) and update any that describe the marker as a no-op.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib "router::tests::route_click_"`
Expected: FAIL — `route_click_reveals_the_first_folded_float_from_the_marker` gets `NoOp` where `FocusFloatingPane(103)` is expected; the all-folded test gets `NoOp` too (falls into the marker-consume branch).

- [ ] **Step 3: Repurpose `chip_marker_at` into `chip_marker_target_at`**

Replace the whole `chip_marker_at` function (signature, doc, body) with:

```rust
/// The hidden floating-pane id the `+k` overflow marker at click (`row`,
/// `column`) resolves to — the FIRST float folded into the marker (#113), i.e.
/// the one right after the individually-shown chips. `None` when the click is
/// off the marker cell. Focusing it (`should_float_if_hidden`) un-hides the
/// whole layer, so the remaining folded floats become individually clickable
/// overlay boxes — every overflow float is reachable in ≤ 2 clicks. The fold
/// boundary derives from the marker's own `k` (`len - k`), mirroring
/// [`crate::floating::chip_cells`] exactly, so draw and hit-test never
/// disagree. Same per-tab geometry resolution as [`float_chip_at`].
pub(crate) fn chip_marker_target_at(
    tab_layout: &[line::TabHit],
    tab_panes: &BTreeMap<usize, TabPaneGeom>,
    row: isize,
    column: usize,
) -> Option<usize> {
    let row = usize::try_from(row).ok()?;
    let position = line::position_at_column(tab_layout, column)?;
    let geom = tab_panes.get(&position)?;
    if geom.hidden_floats.is_empty() {
        return None;
    }
    let col = column.checked_sub(geom.start)?;
    let k =
        crate::floating::chip_marker_k(geom.width, geom.rows, geom.hidden_floats.len(), col, row)?;
    geom.hidden_floats
        .get(geom.hidden_floats.len().saturating_sub(k))
        .copied()
}
```

Note: `chip_marker_k` only returns `Some` when the run actually overflowed, so `k <= len` always holds; the `saturating_sub` + `get` keep the function total without an `unwrap`.

- [ ] **Step 4: Update the `route_click` marker branch**

Replace the marker-consume block (the comment + `if chip_marker_at(...).is_some() { return ClickIntent::NoOp; }`) with:

```rust
    // The `+k` overflow marker folds the hidden floats that don't fit the chip
    // run. A click on it reveals-and-focuses the FIRST folded float (#113):
    // the reveal un-hides the whole layer, so the rest become individually
    // clickable overlay boxes. Resolving here — before the tiled fallback —
    // also keeps the marker shielding the pane it sits over (#110).
    if let Some(id) = chip_marker_target_at(tab_layout, tab_panes, row, column) {
        return ClickIntent::FocusFloatingPane(id);
    }
```

Also update the module-header sentence around `src/router.rs:83` if it still calls the marker "decorative … it selects nothing".

- [ ] **Step 5: Update the `src/floating.rs` doc comments**

- `chip_marker_k`'s doc: replace the "the marker itself selects nothing, but the hit-test lets the router consume a click on it (a no-op)…" sentence with one stating the marker resolves to the first folded float (#113) and this helper reports `k` so the router derives the fold boundary (`count - k`).
- Grep `src/floating.rs` for other "decorative"/"unselectable"/"selects nothing" claims about `PlusK` (`grep -n "decorative\|selects nothing\|個別選択" src/floating.rs`) and align them. Do NOT change any code in `floating.rs` — doc comments only.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib "router::tests::"`
Expected: PASS (all router tests, including the two new/rewritten ones).

- [ ] **Step 7: Full verification**

- `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib` → all pass.
- `cargo clippy --target wasm32-wasip1 --all-features --lib` → clean.
- `cargo build --target wasm32-wasip1` → builds.
- `cargo build --examples --target aarch64-apple-darwin` → builds (no example touches the router, but the gate is cheap).

- [ ] **Step 8: Commit**

```bash
git add src/router.rs src/floating.rs
git commit -m "feat(router): reveal the first folded float from the +k marker (#113)"
```

---

## Task 2: Reconcile the #110 floating spec with the new marker behavior

**Files:**

- Modify: `docs/superpowers/specs/2026-07-12-floating-panes-in-minimap-design.md` (lines ≈ 102 and ≈ 177)

- [ ] **Step 1: Update the two stale statements**

Both currently describe the marker as unselectable. Rewrite (keeping the document's Japanese, its list/table formatting, and line-wrap style):

- Line ≈ 102 — `- 入りきらない時は末尾を`+k`に畳む（`+k`は個別選択不可 → その旨をログ、silent-truncation にしない）。` → state that since #113 a click on `+k` reveals-and-focuses the first folded float, and the reveal makes the rest individually clickable in the overlay.
- Line ≈ 177 (risk table row 「狭い block にチップが入りきらない」) — replace 「個別選択はチップ表示分のみ」 with the #113 reachability (marker → reveal → individual overlay clicks, ≤ 2 clicks).

Verify no other stale claims: `grep -n "個別選択\|選択不可" docs/superpowers/specs/2026-07-12-floating-panes-in-minimap-design.md` must return no line still claiming the folded floats are unreachable.

- [ ] **Step 2: Commit**

```bash
git add docs/superpowers/specs/2026-07-12-floating-panes-in-minimap-design.md
git commit -m "docs(specs): reconcile the floating spec with the reachable +k marker (#113)"
```

---

## Definition of done

- Marker click resolves to `FocusFloatingPane(first folded id)` — pinned by `route_click_reveals_the_first_folded_float_from_the_marker` and the all-folded (`cols = 1`) edge test; real chips and the `count <= cols` no-marker case stay pinned by the existing tests.
- The marker still shields the tiled pane beneath it (same assertions).
- No `unwrap()`/`expect()`; no new intent, plugin state, permission, or config; no visual change.
- Full `cargo test --lib` green; CI-exact clippy clean; wasm + examples build.
- All "marker is decorative / selects nothing" prose (code docs + #110 spec) reconciled.

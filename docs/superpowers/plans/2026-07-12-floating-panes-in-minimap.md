# Floating Panes in the Minimap — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** フローティングペインを tabmap に描画し（表示中はグラフィカルにオーバーレイ、非表示は右下隅の個別チップ）、非表示のフロートもクリック（と #80 のホイール）で選択＝reveal+focus できるようにする（issue #110）。

**Architecture:** 「バーは zellij 状態の純投影」原則を守る — フロートを自前で隠さず、毎フレーム `TabInfo.are_floating_panes_visible` とペイン集合から描き直す。タイル層の bounding-box 正規化にフロートを混ぜず、フロートは別レイヤーとして合成する。描画ロジックは新規 dependency-free モジュール `floating.rs` と既存 `minimap.rs` に閉じ、クリック解決は既存の pure な `router.rs` に合流させる。

**Tech Stack:** Rust → `wasm32-wasip1`（zellij プラグイン）、zellij-tile 0.44.3。renderer は zellij 型非依存で `cargo test --lib`（host triple）でネイティブテスト。

**設計仕様:** `docs/superpowers/specs/2026-07-12-floating-panes-in-minimap-design.md`

---

## 前提コマンド（各タスク共通）

- **ネイティブ単体テスト:** `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib`
- **単一テスト:** `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib <test_name> -- --exact`
- **wasm ビルド:** `cargo build --target wasm32-wasip1`
- テストは本プロジェクトの既存流儀に合わせる: **`Result` を返さない素の `#[test] fn`**、`unwrap`/`expect` を使わず `assert!`/`assert_eq!`、fixture 関数（`content_pane` / `focusable_pane` / `tab`）で構築。（ユーザーのグローバル Rust 規約より **プロジェクト既存スタイルを優先** — CLAUDE.md「Project-Specific Rules Take Precedence」）
- コミットは英語。1論理変更＝1コミット。

## File Structure（作成/変更するファイルと責務）

- **`src/floating.rs`（新規）** — dependency-free。`FloatingMode`（config enum＋`FromStr`）、非表示チップのレイアウト＆ヒットテスト純関数、（P3で）表示中フロートのオーバーレイ写像純関数、`FloatLayer` 描画入力型。zellij 型を持ち込まない（`cargo test --lib` 対象）。
- **`src/projection.rs`（変更）** — `is_tiled_terminal` はそのまま。姉妹関数 `is_floating_terminal` と `project_floating`（フロート → `PaneRect`）を追加。
- **`src/config.rs`（変更）** — `Config.floating: floating::FloatingMode` フィールド＋パース＋デフォルト。
- **`src/minimap.rs`（変更）** — `render` に `FloatLayer` 引数を追加。非表示チップの stamp（P2）と表示中フロートのオーバーレイ合成（P3）。`bbox_of` 抽出と `project_floats_into` 追加（P3）。
- **`src/tab_block.rs`（変更）** — `assemble` に `FloatLayer` を通し、grid ラング（L0–L2）でのみ `render` へ渡す。
- **`src/paint.rs`（変更）** — `bar` にタブごとの `FloatLayer` マップ引数を追加し `assemble` へ転送（`compose` は無変更）。
- **`src/router.rs`（変更）** — `TabPaneGeom` にフロートのヒット情報を追加、`float_chip_at` / `float_pane_at` ヒットテスト、`route_click` に close の後・tiled pane の前でフロート判定を挿入、`ClickIntent::FocusFloatingPane(usize)` を追加。
- **`src/lib.rs`（変更）** — 毎フレーム `panes_by_position` と並列にフロートデータを構築、`paint::bar` へ渡し、フロートのヒット幾何を記録、`FocusFloatingPane(id) => focus_terminal_pane(id, true, false)` を dispatch。（P4）`pane_focus_order` にフロートを合流。
- **`README.md`（変更）** — `floating` config キーを文書化。

## 実装フェーズ概観

- **Phase 0 — スパイク（実機検証、コードなし）:** 3つの仮定を裏取りし P2/P4 の分岐を確定。
- **Phase 1 — データ＋config（純粋、TDD）:** `FloatingMode`、`Config.floating`、`projection::project_floating`。描画はまだ。
- **Phase 2 — チップ＋非表示選択（TDD）:** チップ描画・ヒットテスト・クリックで reveal+focus。**issue #2 達成**。
- **Phase 3 — 表示中オーバーレイ（TDD）:** グラフィカル合成・ボーダー・クリック。**issue #1 達成**。
- **Phase 4 — ホイール walk（TDD、P0-3 でゲート）:** `pane_focus_order` にフロート合流。

---

## Phase 0 — スパイク（実機検証）

### Task 0: 3つの仮定を eprintln オラクル＋expect PTY で裏取りする

**Files:** 一時的に `src/lib.rs`（eprintln オラクル、**コミットしない**）

このタスクはコード成果物ではなく **検証**。手順は `.claude/rules/zellij-plugin-development.md` §1–5 に準拠。この環境には `zellij 0.44.3`（`which zellij` 確認済み）、`expect`（`/usr/bin/expect`）、`wasm32-wasip1` target が揃っている。既存の稼働セッション（`excellent-zebra` 等）には干渉しないよう別名 `tabmap-verify-110` を使う。

- [ ] **Step 1: eprintln オラクルを一時追加**

`src/lib.rs` の `update()` 内、`Event::PaneUpdate` / `Event::TabUpdate` アームの先頭（`permitted` ゲートより前）に、フロート観測用のダンプを一時挿入する:

```rust
// TEMPORARY spike oracle — DO NOT COMMIT. Revert with `git checkout src/lib.rs`.
Event::PaneUpdate(panes) => {
    for (pos, list) in &panes.panes {
        for p in list {
            eprintln!(
                "DBG110 pane tab={pos} id={} floating={} suppressed={} focused={} x={} y={} w={} h={} title={:?}",
                p.id, p.is_floating, p.is_suppressed, p.is_focused,
                p.pane_x, p.pane_y, p.pane_columns, p.pane_rows, p.title
            );
        }
    }
    self.panes = panes;
    true
}
Event::TabUpdate(tabs) => {
    for t in &tabs {
        eprintln!(
            "DBG110 tab pos={} active={} floats_visible={} float_count={}",
            t.position, t.active, t.are_floating_panes_visible, t.selectable_floating_panes_count
        );
    }
    self.tabs = tabs;
    true
}
```

- [ ] **Step 2: wasm をビルドし permissions.kdl を事前シード**

```bash
cargo build --target wasm32-wasip1
```

macOS のキャッシュに、ビルドした wasm の絶対パスで両形式（裸パス＋`file:`プレフィックス）の `ReadApplicationState` / `ChangeApplicationState` を追記する（`.claude/rules/zellij-plugin-development.md` §1）。

- [ ] **Step 3: expect PTY で新セッションを起動し、フロートを操作して zellij.log を観測**

`create-hold.exp`（`.claude/rules/zellij-plugin-development.md` §4 の雛形）で `tabmap-verify-110` を起動・アタッチしたまま、別シェルから:

```bash
zellij --session tabmap-verify-110 action new-pane -f            # フロート生成
zellij --session tabmap-verify-110 action are-floating-panes-visible
zellij --session tabmap-verify-110 action toggle-floating-panes  # フロート層を非表示化
# 非表示化した状態で PaneUpdate をもう一度飛ばすため、タイル側を1つ操作
zellij --session tabmap-verify-110 action new-pane -d right
```

zellij.log（`$(getconf DARWIN_USER_TEMP_DIR)/zellij-<uid>/zellij-log/zellij.log`）の `DBG110` 行を読む。

- [ ] **Step 4: 3つの検証結果を記録する**

以下を zellij.log の `DBG110` 行から判定し、このタスクの結論として本プランのコメント欄（またはコミットメッセージ）に残す:

1. **（P2 をゲート）非表示フロートは manifest に残るか。** `toggle-floating-panes` で非表示化した後の `PaneUpdate` に、そのフロートの行が `floating=true` かつ **id 付き**で出続けるか。
   - **YES →** 個別チップは実装可能（P2 を計画どおり進める）。
   - **NO（消える/id無し） →** P2 のチップをカウントバッジ（`◰N`）へ縮退し、`FloatLayer::Hidden` は ids ではなく count を持つ形に変更。issue #2 は「層トグル」までに縮退（要ユーザー再判断）。
2. **表示中フロートの座標。** `are_floating_panes_visible=true` の間、フロート行の `x/y/w/h` がタイルと同じ座標系（`pane_y` はバー直下 = タイルと整合）で妥当か。→ P3 オーバーレイの前提。
3. **（P4 をゲート）フォーカスロスで層が自動 hide されるか。** フロートにフォーカス中 → `move-focus` でタイルへ移した後の `TabUpdate` で `are_floating_panes_visible` が `false` に自動反転するか。
   - **YES →** P4 は walk にフロートを含める（reveal は一時表示になり後腐れなし）。
   - **NO →** P4 は walk をタイル＋表示中フロートのみに絞る。

- [ ] **Step 5: オラクルを撤去**

```bash
git checkout src/lib.rs
CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib   # 既存テストが緑に戻ることを確認
```

**注意:** eprintln オラクルは絶対にコミットしない。P0 の結論だけを次フェーズに持ち込む。

---

## Phase 1 — データ＋config（純粋、TDD）

### Task 1: `floating.rs` に `FloatingMode` enum を作る

**Files:**

- Create: `src/floating.rs`
- Modify: `src/lib.rs`（`pub mod floating;` を追加）

- [ ] **Step 1: モジュール宣言を追加**

`src/lib.rs` のモジュール宣言群（8–18行目付近）に、アルファベット順の位置へ追加:

```rust
pub mod floating;
```

（`pub mod config;` と `pub mod line;` の間。）

- [ ] **Step 2: 失敗するテストを書く**

`src/floating.rs` を新規作成し、まずテストだけ:

```rust
//! Dependency-free floating-pane layer: config mode, hidden-float chips, and
//! (later) the visible-float overlay mapping. No zellij types, so the whole
//! module is unit-tested off-wasm (rule #8), exactly like `minimap`/`scroll`.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_floating_modes() {
        assert_eq!("hybrid".parse(), Ok(FloatingMode::Hybrid));
        assert_eq!("off".parse(), Ok(FloatingMode::Off));
    }

    #[test]
    fn malformed_floating_mode_errors() {
        // Case-sensitive, exact-match only — the config parser turns the error
        // into the documented default.
        assert_eq!("Hybrid".parse::<FloatingMode>(), Err(()));
        assert_eq!("chips".parse::<FloatingMode>(), Err(()));
        assert_eq!("".parse::<FloatingMode>(), Err(()));
    }

    #[test]
    fn default_is_hybrid() {
        assert_eq!(FloatingMode::default(), FloatingMode::Hybrid);
    }
}
```

- [ ] **Step 3: テストを実行して失敗を確認**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib floating`
Expected: FAIL（`FloatingMode` 未定義でコンパイルエラー）

- [ ] **Step 4: 最小実装を書く**

`src/floating.rs` の先頭（テスト mod の前）に:

```rust
/// How the bar depicts a tab's floating-pane layer (config key `floating`, #110).
///
/// `Hybrid` is the B/A behaviour from the design: a tab whose floating layer is
/// visible overlays each float graphically on its tiled minimap, and a tab whose
/// layer is hidden shows one corner chip per float. `Off` restores the pre-#110
/// look — floating panes are invisible on the bar, exactly as before.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FloatingMode {
    /// Overlay visible floats; chip hidden floats — the default (#110).
    #[default]
    Hybrid,
    /// Draw no floating panes at all — the pre-#110 bar.
    Off,
}

impl std::str::FromStr for FloatingMode {
    type Err = ();

    /// `"hybrid"` / `"off"` (exact match); any other value errors so the config
    /// parser falls back to the documented default rather than panicking.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "hybrid" => Ok(Self::Hybrid),
            "off" => Ok(Self::Off),
            _ => Err(()),
        }
    }
}
```

- [ ] **Step 5: テストを実行して成功を確認**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib floating`
Expected: PASS

- [ ] **Step 6: コミット**

```bash
git add src/floating.rs src/lib.rs
git commit -m "feat(floating): add FloatingMode config enum (#110)"
```

### Task 2: `Config.floating` フィールドを配線する

**Files:**

- Modify: `src/config.rs`

- [ ] **Step 1: 失敗するテストを書く**

`src/config.rs` の `mod tests` に追加（既存 `use super::*;` に加え、`use crate::floating::FloatingMode;` をテスト mod 冒頭へ）:

```rust
#[test]
fn parses_floating_modes() {
    assert_eq!(config_from(&[("floating", "hybrid")]).floating, FloatingMode::Hybrid);
    assert_eq!(config_from(&[("floating", "off")]).floating, FloatingMode::Off);
}

#[test]
fn floating_defaults_to_hybrid() {
    assert_eq!(config_from(&[]).floating, FloatingMode::Hybrid);
}

#[test]
fn malformed_floating_falls_back_to_hybrid() {
    // Unknown / wrong-case / empty values keep the on-by-default hybrid look.
    assert_eq!(config_from(&[("floating", "strip")]).floating, FloatingMode::Hybrid);
    assert_eq!(config_from(&[("floating", "Off")]).floating, FloatingMode::Hybrid);
    assert_eq!(config_from(&[("floating", "")]).floating, FloatingMode::Hybrid);
}
```

- [ ] **Step 2: テストを実行して失敗を確認**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib config`
Expected: FAIL（`floating` フィールド未定義）

- [ ] **Step 3: 最小実装を書く**

`src/config.rs` 冒頭の use 群に追加:

```rust
use crate::floating::FloatingMode;
```

`Config` 構造体（`pub scroll: ScrollMode,` の直後）にフィールドを追加:

```rust
    /// How the bar depicts floating panes (#110). `hybrid` (default) overlays a
    /// tab's visible floats on its minimap and chips its hidden ones in the
    /// bottom-right corner; `off` draws no floating panes (the pre-#110 look).
    /// Rides the already-granted `ChangeApplicationState` (a hidden-float chip
    /// reveals+focuses via `focus_terminal_pane`), so it triggers no new
    /// permission prompt on auto-update. See [`FloatingMode`].
    pub floating: FloatingMode,
```

`impl Config` の定数群（`DEFAULT_SCROLL` の隣）に:

```rust
    /// Default floating depiction — `Hybrid`, so floating panes show out of the
    /// box (#110). A tab with no floating panes renders identically to before.
    pub const DEFAULT_FLOATING: FloatingMode = FloatingMode::Hybrid;
```

`from_configuration` の末尾フィールド（`scroll: ...` の隣、`}` の前）に:

```rust
            floating: configuration
                .get("floating")
                .and_then(|raw| raw.parse().ok())
                .unwrap_or(Self::DEFAULT_FLOATING),
```

- [ ] **Step 4: テストを実行して成功を確認**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib config`
Expected: PASS

既存の `defaults_when_empty` テストにも1行足しておく（`assert_eq!(config.floating, FloatingMode::Hybrid);`）と網羅的。

- [ ] **Step 5: コミット**

```bash
git add src/config.rs
git commit -m "feat(config): parse the floating key (#110)"
```

### Task 3: `projection::project_floating` でフロートを抽出する

**Files:**

- Modify: `src/projection.rs`

- [ ] **Step 1: 失敗するテストを書く**

`src/projection.rs` の `mod tests` に追加:

```rust
#[test]
fn is_floating_terminal_selects_only_floats() {
    // A floating terminal pane passes; tiled, plugin, and suppressed panes do not.
    assert!(is_floating_terminal(&PaneInfo { is_floating: true, ..Default::default() }));
    assert!(!is_floating_terminal(&PaneInfo::default())); // tiled
    assert!(!is_floating_terminal(&PaneInfo {
        is_floating: true,
        is_plugin: true,
        ..Default::default()
    }));
    assert!(!is_floating_terminal(&PaneInfo {
        is_floating: true,
        is_suppressed: true,
        ..Default::default()
    }));
}

#[test]
fn project_floating_keeps_only_floats_with_geometry() {
    // Two floats and one tiled pane: only the floats survive, carrying id and
    // geometry (so a visible-layer overlay can place them). The tiled pane is
    // dropped — `project` (not this) handles the tiled layer.
    let panes = [
        content_pane(0, 1, 80, 24, true),                       // tiled
        PaneInfo { id: 7, is_floating: true, pane_x: 10, pane_y: 5,
                   pane_columns: 30, pane_rows: 10, is_focused: true,
                   title: "top".to_string(), ..Default::default() },
        PaneInfo { id: 9, is_floating: true, pane_x: 40, pane_y: 8,
                   pane_columns: 20, pane_rows: 6,
                   title: "bot".to_string(), ..Default::default() },
    ];
    let floats = project_floating(&panes);
    assert_eq!(floats.len(), 2);
    assert_eq!((floats[0].id, floats[1].id), (7, 9));
    assert_eq!((floats[0].x, floats[0].y, floats[0].w, floats[0].h), (10, 5, 30, 10));
    assert!(floats[0].focused);
}

#[test]
fn project_floating_is_empty_without_floats() {
    assert!(project_floating(&[content_pane(0, 1, 80, 24, true)]).is_empty());
    assert!(project_floating(&[]).is_empty());
}
```

（`content_pane` fixture は既存 `projection.rs` テストにある。無ければ既存の `content_pane(x,y,w,h,focused)` を確認して合わせる。）

- [ ] **Step 2: テストを実行して失敗を確認**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib projection`
Expected: FAIL（`is_floating_terminal` / `project_floating` 未定義）

- [ ] **Step 3: 最小実装を書く**

`src/projection.rs` の `is_tiled_terminal` の直後に、姉妹関数を追加（`is_tiled_terminal` は変更しない — タイル集合の単一情報源という契約を守る）:

```rust
/// Whether a pane is a floating **terminal** pane — the set the bar overlays or
/// chips (#110). The floating sibling of [`is_tiled_terminal`]: it keeps
/// `is_floating` panes but still drops plugin and suppressed ones, so the
/// floating layer never picks up chrome or background panes. `is_suppressed` is
/// excluded on purpose — suppressed panes stay out of scope for #110.
pub fn is_floating_terminal(pane: &PaneInfo) -> bool {
    pane.is_floating && !(pane.is_plugin || pane.is_suppressed)
}

/// Project a tab's **floating** panes into renderer rectangles — the parallel of
/// [`project`] for the floating layer (#110). Carries id, geometry, title, and
/// focus so a visible-layer overlay can place each float; a hidden layer uses
/// only the ids (for corner chips). Order follows the manifest, which is stable
/// per frame.
pub fn project_floating(panes: &[PaneInfo]) -> Vec<PaneRect> {
    panes
        .iter()
        .filter(|pane| is_floating_terminal(pane))
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

- [ ] **Step 4: テストを実行して成功を確認**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib projection`
Expected: PASS

- [ ] **Step 5: コミット**

```bash
git add src/projection.rs
git commit -m "feat(projection): extract floating panes into PaneRects (#110)"
```

---

## Phase 2 — チップ＋非表示選択（TDD）

> **P0-1 分岐:** 以下は「非表示フロートが manifest に id 付きで残る（YES）」前提。NO の場合は `FloatLayer::Hidden(&[usize])` を `Hidden(usize)`（count）に置換し、チップ描画をカウントバッジ `◰N` 1個に縮退（ヒットテストは層トグル `show_floating_panes(None)` を呼ぶ `ClickIntent` へ）。

### Task 4: `floating.rs` にチップのレイアウト＆ヒットテスト純関数を作る

**Files:**

- Modify: `src/floating.rs`

チップは「1フロート=1セル」を最終テキスト行の右端から左へ詰める。バッジ/close と同じ「予約セル」方式。budget を超える分は末尾を `+k` に畳む（可視 = silent truncation ではない）。ここではレイアウト（どの列に何を描くか）とヒットテスト（列→チップindex）だけを純粋に決める。

- [ ] **Step 1: 失敗するテストを書く**

`src/floating.rs` の `mod tests` に追加:

```rust
#[test]
fn chip_cells_pack_from_the_right_edge() {
    // 3 floats in a 12-wide block: chips occupy the 3 rightmost cells, one per
    // float, left-to-right in id order (columns 9,10,11 → float indices 0,1,2).
    let cells = chip_cells(12, 3);
    assert_eq!(cells, vec![(9, Chip::Float(0)), (10, Chip::Float(1)), (11, Chip::Float(2))]);
}

#[test]
fn chip_cells_collapse_overflow_to_a_plus_k_marker() {
    // Budget is at most `cols` cells but we cap chips so a `+k` marker fits: with
    // a 4-cell budget and 10 floats, show 3 float chips then a "+7" marker
    // occupying the remaining cells (the marker is NOT individually selectable).
    let cells = chip_cells(4, 10);
    // 3 float chips + a PlusK(7) marker, all within the 4 rightmost columns.
    assert_eq!(cells.iter().filter(|(_, c)| matches!(c, Chip::Float(_))).count(), 3);
    assert!(cells.iter().any(|(_, c)| *c == Chip::PlusK(7)));
    // Never exceeds the block width.
    assert!(cells.iter().all(|(col, _)| *col < 4));
}

#[test]
fn chip_cells_empty_without_floats() {
    assert!(chip_cells(12, 0).is_empty());
    assert!(chip_cells(0, 3).is_empty());
}

#[test]
fn chip_index_at_cell_resolves_only_the_last_row() {
    // A click on a float chip's cell in the LAST text row resolves to that
    // float's index; the same column on another row misses (chips ride only the
    // bottom row). A PlusK marker cell resolves to None (not selectable).
    let cols = 12;
    let text_rows = 3;
    let count = 3;
    // bottom row = text_rows - 1 = 2; chips at cols 9,10,11.
    assert_eq!(chip_index_at_cell(cols, text_rows, count, 10, 2), Some(1));
    assert_eq!(chip_index_at_cell(cols, text_rows, count, 10, 1), None, "not the bottom row");
    assert_eq!(chip_index_at_cell(cols, text_rows, count, 3, 2), None, "left of the chips");
}

#[test]
fn chip_index_at_cell_ignores_the_plus_k_marker() {
    // With overflow, the +k marker cells are not individually selectable.
    let cols = 4;
    let text_rows = 3;
    let count = 10;
    // Only the 3 real float chips resolve; every marker cell is None.
    let hits: Vec<_> = (0..cols)
        .filter_map(|c| chip_index_at_cell(cols, text_rows, count, c, text_rows - 1))
        .collect();
    assert_eq!(hits.len(), 3, "only the 3 shown float chips are selectable");
    assert_eq!(hits, vec![0, 1, 2]);
}
```

- [ ] **Step 2: テストを実行して失敗を確認**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib floating`
Expected: FAIL（`chip_cells` / `Chip` / `chip_index_at_cell` 未定義）

- [ ] **Step 3: 最小実装を書く**

`src/floating.rs` に（`FloatingMode` の後、`mod tests` の前）:

```rust
/// One drawn chip cell's content (#110). `Float(i)` is a selectable chip for the
/// `i`-th hidden float (its glyph is colored by that float's id); `PlusK(k)` is
/// the non-selectable overflow marker standing in for `k` floats that did not
/// fit. Returned by [`chip_cells`] paired with the block-local column it sits in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chip {
    Float(usize),
    PlusK(usize),
}

/// The chip glyph — a small quadrant marker that reads as "a floating pane
/// docked in the corner". Single display column.
pub const CHIP_GLYPH: char = '◲';

/// Lay out `count` hidden-float chips into the bottom-right corner of a
/// `cols`-wide block (#110): one selectable [`Chip::Float`] per float, packed
/// right-to-left in id order, capped so a `+k` overflow marker fits when there
/// are more floats than columns. Returns `(block_local_column, chip)` pairs in
/// ascending column order, or empty when there is nothing to draw / no width.
///
/// Budget: chips never use more than `cols` columns. When `count` fits, all
/// `count` cells are `Float`; otherwise the leftmost shown cell(s) become a
/// `PlusK` marker so the total never exceeds the budget and the overflow is
/// *visible*, never silently dropped.
pub fn chip_cells(cols: usize, count: usize) -> Vec<(usize, Chip)> {
    if cols == 0 || count == 0 {
        return Vec::new();
    }
    // Reserve at most the whole width. If everything fits, all cells are floats.
    if count <= cols {
        let start = cols - count;
        return (0..count).map(|i| (start + i, Chip::Float(i))).collect();
    }
    // Overflow: show `cols - 1` float chips and a single `+k` marker cell at the
    // left of the reserved run. `k` counts every float the marker stands in for.
    let shown = cols - 1;
    let hidden = count - shown;
    std::iter::once((0usize, Chip::PlusK(hidden)))
        .chain((0..shown).map(|i| (1 + i, Chip::Float(i))))
        .collect()
}

/// The hidden-float index at block-local cell (`col`, `row`) in a
/// `cols`-by-`text_rows` block with `count` floats, or `None` when the cell is
/// not a selectable float chip (#110). Chips ride only the bottom text row
/// (`text_rows - 1`); a `+k` marker cell and every other cell resolve to `None`.
/// Mirrors [`chip_cells`] exactly, so draw and hit-test never disagree.
pub fn chip_index_at_cell(
    cols: usize,
    text_rows: usize,
    count: usize,
    col: usize,
    row: usize,
) -> Option<usize> {
    if text_rows == 0 || row != text_rows - 1 {
        return None;
    }
    chip_cells(cols, count).into_iter().find_map(|(c, chip)| match chip {
        Chip::Float(i) if c == col => Some(i),
        _ => None,
    })
}
```

- [ ] **Step 4: テストを実行して成功を確認**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib floating`
Expected: PASS

- [ ] **Step 5: コミット**

```bash
git add src/floating.rs
git commit -m "feat(floating): chip layout and hit-test for hidden floats (#110)"
```

### Task 5: `FloatLayer` 入力型を定義し `minimap::render` にチップ描画を足す

**Files:**

- Modify: `src/floating.rs`（`FloatLayer` 型）
- Modify: `src/minimap.rs`（`render` 引数追加＋チップ stamp）

- [ ] **Step 1: `FloatLayer` を `floating.rs` に定義**

```rust
use crate::minimap::PaneRect;

/// The floating-pane layer handed to [`crate::minimap::render`] for one tab
/// (#110). Chosen per tab from `TabInfo.are_floating_panes_visible`:
/// - `None` — the tab has no floats, or `floating = off`: draw nothing extra.
/// - `Hidden` — the layer is hidden: draw one corner chip per float id.
/// - `Visible` — the layer is shown: overlay each float's rect on the grid (P3).
#[derive(Clone, Copy, Debug)]
pub enum FloatLayer<'a> {
    None,
    Hidden(&'a [usize]),
    Visible(&'a [PaneRect]),
}
```

（`use crate::minimap::PaneRect;` は `floating.rs` 冒頭へ。）

- [ ] **Step 2: 失敗するテストを書く（`minimap.rs`）**

`src/minimap.rs` の `mod tests` に追加:

```rust
#[test]
fn render_stamps_a_chip_for_each_hidden_float() {
    // Two hidden floats (ids 7, 9) over a lone tiled pane in a 12-wide, 3-row
    // block: the bottom row's two rightmost cells carry the chip glyph, colored
    // by each float's id. Width per row stays exactly 12 (chips never widen it).
    let palette = test_palette();
    let panes = one_focused();
    let hidden = [7usize, 9usize];
    let out = render(
        &panes, &palette, 12, 3, 0,
        LabelMode::None, None, Close::Off, GradientSpec::OFF, true,
        crate::floating::FloatLayer::Hidden(&hidden),
    );
    assert!(out.contains(crate::floating::CHIP_GLYPH), "chips are drawn");
    // Two chips → the glyph appears twice.
    assert_eq!(out.matches(crate::floating::CHIP_GLYPH).count(), 2);
    // Each row still measures exactly 12 display columns. Use the established
    // minimap test pattern: `visible_lines` strips ANSI, then measure width.
    for line in visible_lines(&out) {
        assert_eq!(unicode_width::UnicodeWidthStr::width(line.as_str()), 12);
    }
}

#[test]
fn render_without_floats_is_byte_identical_to_none() {
    // `FloatLayer::None` must reproduce the pre-#110 output exactly.
    let palette = test_palette();
    let panes = one_focused();
    let with_none = render(
        &panes, &palette, 12, 3, 0, LabelMode::None, None, Close::Off,
        GradientSpec::OFF, true, crate::floating::FloatLayer::None,
    );
    let empty: [usize; 0] = [];
    let with_empty_hidden = render(
        &panes, &palette, 12, 3, 0, LabelMode::None, None, Close::Off,
        GradientSpec::OFF, true, crate::floating::FloatLayer::Hidden(&empty),
    );
    assert_eq!(with_none, with_empty_hidden, "no floats draws no chips");
}
```

> **Note（幅計測ヘルパー — 検証済み）:** 新規ヘルパーは追加しない。既存の minimap テスト流儀に合わせ、`visible_lines(&out)`（`src/minimap.rs` にある ANSI/CSI 除去ヘルパー、行を `Vec<String>` で返す）で行に分解し、`unicode_width::UnicodeWidthStr::width(line.as_str())` で表示幅を測る（実例: `labels_with_wide_glyphs_are_placed_width_aware`）。`crate::line::display_width` は **ANSI を除去しない**ので使わない — `render()` 出力の SGR エスケープ（`\x1b[38;2;R;G;Bm` 等）を桁数に数えてしまい、12/16 に一致しない。`tab_block::display_width_ignoring_ansi` は `pub` でなく `minimap.rs` から到達不可。この幅計測パターンは Task 10 の render テストでも同一に使う。

- [ ] **Step 3: テストを実行して失敗を確認**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib minimap`
Expected: FAIL（`render` の引数不一致でコンパイルエラー）

- [ ] **Step 4: `render` に `floats` 引数を足し、チップを stamp**

`render` シグネチャ（`src/minimap.rs:733`）の末尾に引数を追加:

```rust
    active: bool,
    floats: crate::floating::FloatLayer,
) -> String {
```

`render` 冒頭の予約計算（`close_col` の計算あたり、816行目のペインループの前）に、チップの予約列を追加:

```rust
    // Hidden floating panes are chipped into the bottom text row's right corner
    // (#110). Compute the chip cells once, up front, so the paint loop can stamp
    // them and the badge/label placement stays clear of the chip columns. Only
    // the `Hidden` layer draws chips; `None`/`Visible` yield an empty layout.
    let chip_layout: Vec<(usize, crate::floating::Chip)> = match floats {
        crate::floating::FloatLayer::Hidden(ids) => {
            crate::floating::chip_cells(pw, ids.len())
        }
        _ => Vec::new(),
    };
    let chip_ids: &[usize] = match floats {
        crate::floating::FloatLayer::Hidden(ids) => ids,
        _ => &[],
    };
    let chip_row = text_rows.saturating_sub(1);
```

ペイントの二重ループ内、`put_halfblock` フォールバック（1063–1065行目）の**手前**に、チップ分岐を追加（バッジ/close と同じ「pixel_color で背景1回サンプル → put_bg → 前景色で1文字」パターン）:

```rust
            if tr == chip_row {
                if let Some((_, chip)) = chip_layout.iter().find(|(cc, _)| *cc == c) {
                    let bg = pixel_color(&grid, &ring, panes, palette, &sweeps, pw, c, 2 * chip_row);
                    match bg {
                        Some(f) => put_bg(&mut out, f),
                        None => put_default_bg(&mut out),
                    }
                    let (glyph, key) = match chip {
                        crate::floating::Chip::Float(i) => (crate::floating::CHIP_GLYPH, chip_ids[*i]),
                        // The +k marker reads as an accent-toned digit run; use the
                        // accent as its color key so it stands apart from the float
                        // chips. Rendered "+k" would need >1 cell, so a single-cell
                        // marker glyph stands in and the overflow count is conveyed
                        // by there being fewer float chips than floats.
                        crate::floating::Chip::PlusK(_) => (crate::floating::CHIP_MORE_GLYPH, usize::MAX),
                    };
                    let base = if key == usize::MAX {
                        palette.accent()
                    } else {
                        palette.color_for(key)
                    };
                    let fg = if active {
                        base
                    } else {
                        crate::color::mixed(base, bg.unwrap_or(crate::color::CANVAS), INACTIVE_LABEL_BLEND)
                    };
                    put_fg(&mut out, fg);
                    out.push(glyph);
                    continue;
                }
            }
```

`CHIP_MORE_GLYPH` を `floating.rs` に追加（`+k` を1セルで表す代替。単一セル制約のため専用グリフ）:

```rust
/// The overflow marker glyph — stands in for the floats that did not fit as
/// chips. Single display column (a real "+k" needs several cells).
pub const CHIP_MORE_GLYPH: char = '⋯';
```

> **重要（幅の不変条件）:** チップは1セル=1グリフで、バッジ/close と同じく行幅を増やさない。チップ列はタイル塗りの上に乗るので `pixel_color` で背景をサンプルして put_bg する。`chip_row` の下側ピクセル行（`2*chip_row+1`）はここでは触らない（1セル1文字方式）。

- [ ] **Step 5: `render` の既存呼び出し元を更新**

`render` を呼ぶ箇所を洗い出し、`FloatLayer::None` を渡すよう更新（この時点では tab_block からは常に None）:

```bash
grep -rn "minimap::render\|render(" src/tab_block.rs
```

`src/tab_block.rs` の `grid_lines`（`minimap::render(...)` 呼び出し）に、まだ `floats` 引数が無いので **Task 6** で正式に配線する。それまで minimap.rs 内の既存 render テストは全て末尾に `crate::floating::FloatLayer::None` を追加してコンパイルを通す。

- [ ] **Step 6: テストを実行して成功を確認**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib minimap`
Expected: PASS（新テスト2件＋既存 render テストが全て緑）

- [ ] **Step 7: コミット**

```bash
git add src/floating.rs src/minimap.rs
git commit -m "feat(minimap): stamp hidden-float chips in the bottom-right corner (#110)"
```

### Task 6: `tab_block::assemble` と `paint::bar` に `FloatLayer` を通す

**Files:**

- Modify: `src/tab_block.rs`
- Modify: `src/paint.rs`

- [ ] **Step 1: 失敗するテストを書く（`tab_block.rs`）**

`src/tab_block.rs` の `mod tests` に追加:

```rust
#[test]
fn grid_rung_draws_hidden_float_chips() {
    // An L0 grid rung with two hidden floats stamps the chip glyph; a narrow
    // L3/L4 rung (no minimap) draws none — the caller only hands chips to grid
    // rungs, mirroring how labels/badges degrade.
    let palette = test_palette();
    let hidden = [7usize, 9usize];
    let block = assemble(
        &one_pane("shell"), &palette, 16, 3, 0, "\u{2318}",
        GradientSpec::OFF, true, false, Close::Off,
        crate::floating::FloatLayer::Hidden(&hidden),
    );
    let joined: String = block.lines.iter().map(StyledLine::as_str).collect();
    assert!(joined.contains(crate::floating::CHIP_GLYPH), "L0 rung stamps chips");
    // Width contract still holds on every row.
    for line in &block.lines {
        assert_eq!(measured(line), 16);
    }
}
```

- [ ] **Step 2: テストを実行して失敗を確認**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib tab_block`
Expected: FAIL（`assemble` の引数不一致）

- [ ] **Step 3: `assemble` に `floats` 引数を通す**

`assemble`（`src/tab_block.rs:159`）シグネチャ末尾に追加:

```rust
    perspective: bool,
    close: Close,
    floats: crate::floating::FloatLayer,
) -> TabBlock {
```

`grid_lines` 呼び出し（L0/L1/L2 の各 `grid_lines(...)`）に `floats` を転送し、`glyph_lines`/`hint_lines`（L3/L4）には渡さない（狭ラングはチップ非対象）。`grid_lines` 自身のシグネチャにも `floats` を足し、内部の `minimap::render(...)` 呼び出し末尾へ渡す:

```rust
fn grid_lines(
    panes: &[PaneRect],
    palette: &Palette,
    width: usize,
    rows: usize,
    vinset: usize,
    mode: LabelMode,
    badge: Option<&str>,
    close: Close,
    gradient: GradientSpec,
    active: bool,
    floats: crate::floating::FloatLayer,
) -> Vec<StyledLine> {
    let block = minimap::render(
        panes, palette, width, rows, vinset, mode, badge, close, gradient, active, floats,
    );
    padded_rows(block.lines().map(str::to_string), width, rows)
}
```

- [ ] **Step 4: `assemble` の既存呼び出し元（`paint::bar` とテスト）を更新**

`paint::bar`（`src/paint.rs:43`）にタブごとの `FloatLayer` を渡す引数を追加。`panes_by_position` と対になる `floats_by_position: &BTreeMap<usize, FloatSpec>` を受け取り、各タブの `assemble` へ変換して渡す。ここで `FloatSpec` は「そのタブが visible か hidden か＋データ」を持つ軽量型。P2 では hidden の ids だけ使うが、P3 の visible も同じ経路で通すため enum で定義:

`src/floating.rs` に:

```rust
/// Per-tab floating data captured at the render site (#110), turned into a
/// borrowed [`FloatLayer`] inside `paint::bar`. Owns the float ids (hidden) or
/// rects (visible) so `lib.rs` can build it once per frame from the manifest.
#[derive(Clone, Debug)]
pub enum FloatSpec {
    None,
    Hidden(Vec<usize>),
    Visible(Vec<PaneRect>),
}

impl FloatSpec {
    /// Borrow this spec as the layer `render` consumes.
    pub fn layer(&self) -> FloatLayer<'_> {
        match self {
            FloatSpec::None => FloatLayer::None,
            FloatSpec::Hidden(ids) => FloatLayer::Hidden(ids),
            FloatSpec::Visible(rects) => FloatLayer::Visible(rects),
        }
    }
}
```

`paint::bar` シグネチャに追加:

```rust
    perspective: bool,
    close: Close,
    floats_by_position: &BTreeMap<usize, crate::floating::FloatSpec>,
) -> String {
```

`bar` 内で各タブの `assemble` 呼び出しに、そのタブの spec を渡す:

```rust
let floats = floats_by_position
    .get(&hit.position)
    .map(crate::floating::FloatSpec::layer)
    .unwrap_or(crate::floating::FloatLayer::None);
```

（`button_block` 呼び出しはフロート非対象なので変更なし。）

- [ ] **Step 5: テストを実行して成功を確認**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib tab_block paint`
Expected: PASS（既存 `assemble`/`bar` テストは末尾に `FloatLayer::None` / 空の `&BTreeMap::new()` を追加して緑に）

- [ ] **Step 6: コミット**

```bash
git add src/tab_block.rs src/paint.rs src/floating.rs
git commit -m "feat(tab_block,paint): thread the float layer to grid rungs (#110)"
```

### Task 7: `router` にフロートチップのヒットテストと `FocusFloatingPane` を足す

**Files:**

- Modify: `src/router.rs`

`route_click` の優先順位を「button > close > **float** > tiled pane > switch」に変える（spec §7.1 フロート優先）。`TabPaneGeom` にフロートチップの情報（ids と block ジオメトリ）を持たせ、`float_chip_at` で解決する。

- [ ] **Step 1: 失敗するテストを書く（`router.rs`）**

```rust
#[test]
fn float_chip_at_resolves_a_hidden_float_click() {
    // A grid-rung tab at cols 10..30, 3 rows, with two hidden floats (ids 7, 9).
    // A click on the bottom row's rightmost chip cell resolves to float 9; the
    // next chip to 7; a click above the bottom row misses.
    let tab_layout = vec![hit_active(0, 10, 20)];
    let mut geom = geom(10, 20, &[]);
    geom.hidden_floats = vec![7, 9];
    let tab_panes: BTreeMap<usize, TabPaneGeom> = [(0usize, geom)].into_iter().collect();
    // bottom row = MIN_ROWS - 1 = 2; block-local cols 18,19 hold chips 0,1.
    assert_eq!(float_chip_at(&tab_layout, &tab_panes, 2, 29), Some(9), "rightmost chip → float 9");
    assert_eq!(float_chip_at(&tab_layout, &tab_panes, 2, 28), Some(7));
    assert_eq!(float_chip_at(&tab_layout, &tab_panes, 1, 29), None, "not the bottom row");
}

#[test]
fn route_click_prefers_a_float_chip_over_the_tiled_pane_under_it() {
    // The bottom-right corner holds both a tiled pane (whole block) and a float
    // chip. route_click resolves the chip first (float priority, spec §7.1).
    let tab_layout = vec![hit_active(0, 10, 20)];
    let mut geom = geom(10, 20, &[(5, 0, 0, 80, 24)]); // one tiled pane fills the block
    geom.hidden_floats = vec![7];
    let tab_panes: BTreeMap<usize, TabPaneGeom> = [(0usize, geom)].into_iter().collect();
    assert_eq!(
        route_click(None, &[], &tab_layout, &tab_panes, 2, 29),
        ClickIntent::FocusFloatingPane(7),
        "the chip beats the tiled pane beneath it",
    );
    // A click elsewhere in the block still focuses the tiled pane.
    assert_eq!(
        route_click(None, &[], &tab_layout, &tab_panes, 0, 12),
        ClickIntent::FocusPane(5),
    );
}
```

`geom` fixture（`router.rs` テスト内）に `hidden_floats` を初期化する必要があるので、fixture を更新（Step 3 で `TabPaneGeom` にフィールド追加後）。

- [ ] **Step 2: テストを実行して失敗を確認**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib router`
Expected: FAIL（`hidden_floats` / `float_chip_at` / `FocusFloatingPane` 未定義）

- [ ] **Step 3: `TabPaneGeom` にフロート情報を足し、ヒットテスト＋intent を追加**

`TabPaneGeom`（`src/router.rs:17`）にフィールド追加:

```rust
pub(crate) struct TabPaneGeom {
    pub(crate) start: usize,
    pub(crate) width: usize,
    pub(crate) rows: usize,
    pub(crate) vinset: usize,
    pub(crate) panes: Vec<minimap::PaneRect>,
    /// Hidden floating pane ids drawn as corner chips this frame (#110), in the
    /// same order [`crate::floating::chip_cells`] lays them out. Empty when the
    /// tab has no hidden floats (or its layer is visible / off).
    pub(crate) hidden_floats: Vec<usize>,
}
```

`ClickIntent` にバリアント追加:

```rust
    /// Focus this floating pane id (#110) — a click landed on its corner chip
    /// (hidden layer) or its overlay (visible layer). Dispatched with
    /// `should_float_if_hidden = true`, so a hidden float is revealed+focused.
    FocusFloatingPane(usize),
```

ヒットテスト関数を追加:

```rust
/// The hidden floating-pane id whose corner chip is at click (`row`, `column`),
/// or `None` when the click missed every chip (#110). Resolves against the
/// exact chip layout [`crate::floating::chip_index_at_cell`] computed for the
/// tab under the cursor, so draw and hit-test never disagree.
pub(crate) fn float_chip_at(
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
    let index = crate::floating::chip_index_at_cell(
        geom.width,
        geom.rows,
        geom.hidden_floats.len(),
        col,
        row,
    )?;
    geom.hidden_floats.get(index).copied()
}
```

`route_click` に、close の後・`pane_at` の前で挿入:

```rust
    if let Some(position) = clicked_close_button(close_layout, row, column) {
        return ClickIntent::CloseTab(position);
    }
    // Floating panes sit on top of the tiled minimap, so a chip (or, later, an
    // overlay) wins over the tiled pane in the same cell (spec §7.1).
    if let Some(id) = float_chip_at(tab_layout, tab_panes, row, column) {
        return ClickIntent::FocusFloatingPane(id);
    }
    if let Some(id) = pane_at(tab_layout, tab_panes, row, column) {
        return ClickIntent::FocusPane(id);
    }
```

`router.rs` テストの `geom` fixture を更新（`hidden_floats: Vec::new()` を初期化）:

```rust
fn geom(start: usize, width: usize, panes: &[(usize, u32, u32, u32, u32)]) -> TabPaneGeom {
    TabPaneGeom {
        start,
        width,
        rows: MIN_ROWS,
        vinset: 0,
        panes: panes.iter()
            .map(|&(id, x, y, w, h)| minimap::PaneRect::new(id, x, y, w, h, "sh", false))
            .collect(),
        hidden_floats: Vec::new(),
    }
}
```

- [ ] **Step 4: テストを実行して成功を確認**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib router`
Expected: PASS

- [ ] **Step 5: コミット**

```bash
git add src/router.rs
git commit -m "feat(router): resolve hidden-float chip clicks before tiled panes (#110)"
```

### Task 8: `lib.rs` でフロートデータを構築・記録し、reveal+focus を dispatch

**Files:**

- Modify: `src/lib.rs`

- [ ] **Step 1: 失敗するテストを書く（`lib.rs`）**

`src/lib.rs` の `mod tests` に追加:

```rust
#[test]
fn render_records_hidden_float_chips_for_a_tab_with_a_hidden_layer() {
    // A tab whose floating layer is hidden and holds two floats records their
    // ids as chips in the tab's geometry, so a later click can reveal+focus one.
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
            content_pane(0, 1, 80, 24),                                    // tiled
            PaneInfo { id: 7, is_floating: true, ..Default::default() },    // hidden float
            PaneInfo { id: 9, is_floating: true, ..Default::default() },    // hidden float
        ],
    );

    state.render(MIN_ROWS, 80);

    assert_eq!(
        state.tab_panes.get(&0).map(|g| g.hidden_floats.clone()),
        Some(vec![7, 9]),
        "hidden floats are recorded as chips for click-to-reveal",
    );
}

#[test]
fn floating_off_records_no_chips() {
    // With `floating = off` the bar ignores floats entirely (pre-#110 look).
    let mut state = State::default();
    state.permitted = true;
    state.config = Config { floating: crate::floating::FloatingMode::Off, ..Default::default() };
    state.tabs = vec![TabInfo {
        active: true,
        are_floating_panes_visible: false,
        ..tab(0, 1)
    }];
    state.panes.panes.insert(
        0,
        vec![content_pane(0, 1, 80, 24), PaneInfo { id: 7, is_floating: true, ..Default::default() }],
    );

    state.render(MIN_ROWS, 80);
    assert_eq!(state.tab_panes.get(&0).map(|g| g.hidden_floats.len()), Some(0));
}

#[test]
fn left_click_on_a_chip_dispatches_a_floating_focus() {
    // A recorded chip click resolves to FocusFloatingPane and dispatches the host
    // effect (a no-op stub off-wasm) without panicking, deferring the repaint.
    let mut state = State::default();
    state.tab_layout = vec![hit_active(0, 10, 20)];
    let mut g = geom(10, 20, &[]);
    g.hidden_floats = vec![7];
    state.tab_panes = [(0usize, g)].into_iter().collect();
    // bottom row = MIN_ROWS - 1 = 2; rightmost chip cell = col 10+20-1 = 29.
    assert!(!state.update(Event::Mouse(Mouse::LeftClick(2, 29))));
}
```

`geom` fixture（`lib.rs` テストの `router::TabPaneGeom` を作るもの、820行目付近）にも `hidden_floats: Vec::new()` を追加。

- [ ] **Step 2: テストを実行して失敗を確認**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib`
Expected: FAIL（`FloatSpec` 構築・`hidden_floats` 記録・`FocusFloatingPane` dispatch 未実装）

- [ ] **Step 3: `render` でフロート spec を構築・記録**

`src/lib.rs` の `render` 内、`panes_by_position` 構築（259–271行目）の直後に、タブごとの `FloatSpec` を構築:

```rust
        // Build each visible tab's floating layer spec from its live pane set and
        // the tab's `are_floating_panes_visible` flag (#110). `floating = off`
        // yields `None` for every tab, reproducing the pre-#110 bar. A hidden
        // layer becomes chips (ids); a visible layer becomes an overlay (rects,
        // painted in P3). Keyed by position like `panes_by_position`.
        let floats_by_position: BTreeMap<usize, floating::FloatSpec> = match self.config.floating {
            floating::FloatingMode::Off => BTreeMap::new(),
            floating::FloatingMode::Hybrid => layout
                .tabs
                .iter()
                .map(|hit| {
                    let panes = self
                        .panes
                        .panes
                        .get(&hit.position)
                        .map(Vec::as_slice)
                        .unwrap_or_default();
                    let floats = projection::project_floating(panes);
                    let visible = self
                        .tabs
                        .iter()
                        .find(|t| t.position == hit.position)
                        .map(|t| t.are_floating_panes_visible)
                        .unwrap_or(false);
                    let spec = if floats.is_empty() {
                        floating::FloatSpec::None
                    } else if visible {
                        floating::FloatSpec::Visible(floats)
                    } else {
                        floating::FloatSpec::Hidden(floats.iter().map(|f| f.id).collect())
                    };
                    (hit.position, spec)
                })
                .collect(),
        };
```

`paint::bar` 呼び出しに `&floats_by_position` を追加（末尾引数）。

`tab_panes` 構築（331–355行目）の `router::TabPaneGeom { ... }` に `hidden_floats` を足す:

```rust
                        hidden_floats: match floats_by_position.get(&hit.position) {
                            Some(floating::FloatSpec::Hidden(ids)) => ids.clone(),
                            _ => Vec::new(),
                        },
```

- [ ] **Step 4: `FocusFloatingPane` を dispatch**

`update` の `LeftClick` match（176–180行目付近）に arm を追加:

```rust
                    // A floating chip (hidden layer) or overlay (visible) — reveal
                    // and focus it. Unlike the tiled `FocusPane` arm, a hidden
                    // float has no on-screen footprint, so `should_float_if_hidden
                    // = true` both reveals its layer and focuses it in one step
                    // (#110). Rides the already-granted `ChangeApplicationState`.
                    router::ClickIntent::FocusFloatingPane(id) => {
                        focus_terminal_pane(id as u32, true, false);
                    }
```

- [ ] **Step 5: テストを実行して成功を確認**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib`
Expected: PASS（全モジュール緑）

- [ ] **Step 6: wasm ビルドを確認**

Run: `cargo build --target wasm32-wasip1`
Expected: 成功（プラグインがビルドできる）

- [ ] **Step 7: コミット**

```bash
git add src/lib.rs
git commit -m "feat(lib): record hidden-float chips and reveal+focus on click (#110)"
```

---

## Phase 3 — 表示中フロートのグラフィカルオーバーレイ（TDD）

> **P0-2 前提:** 表示中フロートの `x/y/w/h` がタイル座標系で妥当。オーバーレイはタイルの bbox を**拡張させず**、同じ変換でフロートを写像しタイルの上に塗る。

### Task 9: `minimap` に bbox 抽出と `project_floats_into` を作る

**Files:**

- Modify: `src/minimap.rs`

`project_panes`（`src/minimap.rs:602`）は自分の `panes` から `minx/miny/bw/bh` を計算している。この bbox 計算を小関数に切り出し、フロートを**同じ bbox** で写像する `project_floats_into` を追加する（タイルの bbox は絶対に変えない）。

- [ ] **Step 1: 失敗するテストを書く**

```rust
#[test]
fn project_floats_into_maps_through_the_tiled_bbox_without_expanding_it() {
    // A tiled pane spans (0,0,100,40); a float sits at (50,20,20,10) inside it.
    // Mapped through the tiled bbox into an 8x8-pixel canvas, the float lands in
    // the lower-right quadrant — and the tiled bbox is unchanged by the float.
    let tiled = [PaneRect::new(0, 0, 0, 100, 40, "t", false)];
    let (_, tiled_boxes) = project_panes(&tiled, 8, 8, 0);
    let bbox = bbox_of(&tiled);
    let floats = [PaneRect::new(7, 50, 20, 20, 10, "f", false)];
    let (fgrid, fboxes) = project_floats_into(&floats, bbox, 8, 8, 0);
    // The tiled pane still fills the whole canvas (float did not expand its bbox).
    assert_eq!(tiled_boxes[0], PaneBox { px0: 0, px1: 8, py0: 0, py1: 8 });
    // The float occupies a sub-rectangle in the lower-right, not the whole canvas.
    let fb = fboxes[0];
    assert!(fb.px0 >= 4 && fb.py0 >= 4, "float maps to the lower-right quadrant: {fb:?}");
    assert!(fb.px1 <= 8 && fb.py1 <= 8);
    // Its grid cells point back to float index 0.
    assert_eq!(fgrid[fb.py0 * 8 + fb.px0], Some(0));
}
```

- [ ] **Step 2: テストを実行して失敗を確認**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib minimap`
Expected: FAIL（`bbox_of` / `project_floats_into` 未定義）

- [ ] **Step 3: bbox 抽出＋フロート写像を実装**

`project_panes` の `minx/miny/maxx/maxy` 計算部を `bbox_of` に切り出し、`project_panes` はそれを呼ぶ形にリファクタ:

```rust
/// The tiled group's bounding box as `(minx, miny, bw, bh)` — origins and
/// clamped span. The single source both the tiled projection and the floating
/// overlay ([`project_floats_into`]) map through, so a float is placed relative
/// to exactly the same box the tiles were, never expanding it (#110).
pub(crate) fn bbox_of(panes: &[PaneRect]) -> (u32, u32, f64, f64) {
    let minx = panes.iter().map(|p| p.x).min().unwrap_or(0);
    let miny = panes.iter().map(|p| p.y).min().unwrap_or(0);
    let maxx = panes.iter().map(|p| p.x + p.w).max().unwrap_or(1);
    let maxy = panes.iter().map(|p| p.y + p.h).max().unwrap_or(1);
    (minx, miny, (maxx - minx).max(1) as f64, (maxy - miny).max(1) as f64)
}
```

`project_panes` 内の該当計算を `let (minx, miny, bw, bh) = bbox_of(panes);` に置換（`map` クロージャはそのまま）。

`project_floats_into` を新設 — `project_panes` の写像式（`map`/クランプ）を**渡された bbox** で再利用:

```rust
/// Map floating panes into their block-local pixel boxes and ownership grid
/// through a **given** bounding box (the tiled group's), never recomputing it —
/// so floats overlay the tiled minimap without shifting it (#110). Same rounding
/// and edge-clamp as [`project_panes`]; a float outside the tiled bbox clamps to
/// the block edge and never changes `pw`/`ph`. `grid[i]` is the float slice
/// index (not id), mirroring `project_panes`.
pub(crate) fn project_floats_into(
    floats: &[PaneRect],
    bbox: (u32, u32, f64, f64),
    pw: usize,
    ph: usize,
    vinset: usize,
) -> (Vec<Option<usize>>, Vec<PaneBox>) {
    let mut grid = vec![None::<usize>; ph * pw];
    if pw == 0 || ph == 0 || floats.is_empty() {
        return (grid, Vec::new());
    }
    let content_ph = ph.saturating_sub(2 * vinset).max(1);
    let vinset = (ph - content_ph) / 2;
    let (minx, miny, bw, bh) = bbox;
    let map = |v: u32, lo: u32, span: f64, out: usize| -> usize {
        (((v.saturating_sub(lo)) as f64) / span * out as f64).round() as usize
    };
    let boxes: Vec<PaneBox> = floats
        .iter()
        .map(|p| {
            let px0 = map(p.x, minx, bw, pw).min(pw);
            let px1 = match map(p.x + p.w, minx, bw, pw).min(pw) {
                hi if hi <= px0 => (px0 + 1).min(pw),
                hi => hi,
            };
            let py0 = (vinset + map(p.y, miny, bh, content_ph).min(content_ph - 1)).min(ph);
            let py1 = match (vinset + map(p.y + p.h, miny, bh, content_ph).min(content_ph)).min(ph) {
                hi if hi <= py0 => (py0 + 1).min(ph),
                hi => hi,
            };
            PaneBox { px0, px1, py0, py1 }
        })
        .collect();
    for (i, b) in boxes.iter().enumerate() {
        for py in b.py0..b.py1 {
            for px in b.px0..b.px1 {
                grid[py * pw + px] = Some(i);
            }
        }
    }
    (grid, boxes)
}
```

> **Note:** `map` に `saturating_sub` を使うのは、フロートの `x/y` がタイルの `minx/miny` より小さい（タイル bbox の外側左上）ケースで underflow しないため。既存 `project_panes` は tiled のみで `v >= lo` が保証されるが、フロートは保証されないので防御する。

- [ ] **Step 4: テストを実行して成功を確認**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib minimap`
Expected: PASS（`project_panes` リファクタで既存テストも緑）

- [ ] **Step 5: コミット**

```bash
git add src/minimap.rs
git commit -m "feat(minimap): map floats through the tiled bbox without expanding it (#110)"
```

### Task 10: `render` の `Visible` アームでオーバーレイを合成し、ヒットテストを足す

**Files:**

- Modify: `src/minimap.rs`

- [ ] **Step 1: 失敗するテストを書く**

```rust
#[test]
fn render_overlays_a_visible_float_on_top_of_the_tiled_grid() {
    // A tiled pane (id 0) fills the block; a visible float (id 7) sits in the
    // middle. The float's color (keyed on id 7) must appear in the output, on top
    // of the tiled fill, and the row width stays exact.
    let palette = test_palette();
    let tiled = [PaneRect::new(0, 0, 0, 100, 40, "t", false)];
    let floats = [PaneRect::new(7, 30, 12, 40, 16, "f", false)];
    let out = render(
        &tiled, &palette, 16, 4, 0, LabelMode::None, None, Close::Off,
        GradientSpec::OFF, true, crate::floating::FloatLayer::Visible(&floats),
    );
    let float_fg = format!("\x1b[38;2;{};{};{}m", palette.color_for(7).0, palette.color_for(7).1, palette.color_for(7).2);
    assert!(out.contains(&float_fg), "the visible float paints its own color on top");
    // Width contract, measured the same way as Task 5 (ANSI stripped via
    // `visible_lines`, then `UnicodeWidthStr::width`).
    for line in visible_lines(&out) {
        assert_eq!(unicode_width::UnicodeWidthStr::width(line.as_str()), 16);
    }
}

#[test]
fn float_pane_at_cell_resolves_a_visible_float_over_the_tiled_pane() {
    // Same geometry: a click in the float's box resolves to float id 7 (float
    // priority); a click outside it falls through to None (caller then tries the
    // tiled hit-test).
    let tiled = [PaneRect::new(0, 0, 0, 100, 40, "t", false)];
    let floats = [PaneRect::new(7, 30, 12, 40, 16, "f", false)];
    // Center of the float's box in a 16x4 block.
    assert_eq!(float_pane_at_cell(&tiled, &floats, 16, 4, 0, 8, 1), Some(7));
    // Top-left corner is tiled-only → the float hit-test misses.
    assert_eq!(float_pane_at_cell(&tiled, &floats, 16, 4, 0, 0, 0), None);
}
```

- [ ] **Step 2: テストを実行して失敗を確認**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib minimap`
Expected: FAIL

- [ ] **Step 3: `render` の `Visible` 合成と `float_pane_at_cell` を実装**

`render` の `let (grid, bounds) = project_panes(...)`（754行目）の直後に、フロートの並行グリッドを計算:

```rust
    // The visible floating layer overlays the tiled grid, mapped through the same
    // bbox so it sits in place without shifting the tiles (#110). Kept in its own
    // grid/boxes so the tiled `grid[i]`/`panes[i]` index space is never mixed.
    let float_rects: &[PaneRect] = match floats {
        crate::floating::FloatLayer::Visible(f) => f,
        _ => &[],
    };
    let (float_grid, float_bounds) = if float_rects.is_empty() {
        (Vec::new(), Vec::new())
    } else {
        project_floats_into(float_rects, bbox_of(panes), pw, ph, vinset)
    };
    // Float border pixels: the outline of each float box, drawn in the float's
    // `ring_for` shade so it reads as a distinct pane floating above the tiles.
    let mut float_ring = vec![false; float_grid.len()];
    for b in &float_bounds {
        for py in b.py0..b.py1 {
            for px in b.px0..b.px1 {
                let edge = px == b.px0 || px == b.px1 - 1 || py == b.py0 || py == b.py1 - 1;
                if edge {
                    float_ring[py * pw + px] = true;
                }
            }
        }
    }
```

画素解決を「フロート優先 → タイル」にするヘルパーを `render` 内クロージャ（または小関数）で用意し、`put_halfblock` に渡す `top`/`bottom` を差し替え。二重ループ末尾の

```rust
            let top = pixel_color(&grid, &ring, panes, palette, &sweeps, pw, c, 2 * tr);
            let bottom = pixel_color(&grid, &ring, panes, palette, &sweeps, pw, c, 2 * tr + 1);
            put_halfblock(&mut out, top, bottom);
```

を、フロート画素を先に試すよう変更:

```rust
            let float_px = |py: usize| -> Option<Rgb> {
                let i = *float_grid.get(py * pw + c)?; // Option<usize> element
                let i = i?;
                Some(if float_ring[py * pw + c] {
                    palette.ring_for(float_rects[i].id)
                } else {
                    palette.color_for(float_rects[i].id)
                })
            };
            let top = float_px(2 * tr).or_else(|| pixel_color(&grid, &ring, panes, palette, &sweeps, pw, c, 2 * tr));
            let bottom = float_px(2 * tr + 1).or_else(|| pixel_color(&grid, &ring, panes, palette, &sweeps, pw, c, 2 * tr + 1));
            put_halfblock(&mut out, top, bottom);
```

> **Note:** `float_grid` は空のとき長さ0なので `.get()` が `None` を返し、タイルへフォールバックする（`Visible` 以外・フロート無しの経路は byte-identical のまま）。フロートのフォーカスリング（focus ring）は P3 のこのボーダーと視覚的に近いので、まずはボーダーのみ（`ring_for`）で十分。フォーカス中フロートの追加強調は必要なら follow-up。

ヒットテスト関数を追加（`pane_at_cell` と対称、フロート優先）:

```rust
/// The visible floating pane id drawn at block-local cell (`col`, `row`) over a
/// tiled minimap of `tiled` with `floats` overlaid, or `None` when the cell is
/// not on any float (#110). Uses the same `project_floats_into` mapping `render`
/// paints with, so draw and hit-test never disagree. The caller tries this
/// before the tiled `pane_at_cell` (float priority, spec §7.1).
pub fn float_pane_at_cell(
    tiled: &[PaneRect],
    floats: &[PaneRect],
    cols: usize,
    text_rows: usize,
    vinset: usize,
    col: usize,
    row: usize,
) -> Option<usize> {
    let pw = cols;
    let ph = text_rows * 2;
    if pw == 0 || text_rows == 0 || col >= pw || row >= text_rows || floats.is_empty() {
        return None;
    }
    let (grid, _) = project_floats_into(floats, bbox_of(tiled), pw, ph, vinset);
    let at = |py: usize| grid[py * pw + col];
    at(2 * row).or_else(|| at(2 * row + 1)).map(|i| floats[i].id)
}
```

- [ ] **Step 4: テストを実行して成功を確認**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib minimap`
Expected: PASS

- [ ] **Step 5: コミット**

```bash
git add src/minimap.rs
git commit -m "feat(minimap): overlay visible floats and hit-test them (#110)"
```

### Task 11: `router` と `lib.rs` で表示中フロートのオーバーレイをクリック解決に配線

**Files:**

- Modify: `src/router.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: 失敗するテストを書く（`router.rs`）**

```rust
#[test]
fn route_click_prefers_a_visible_float_overlay_over_the_tiled_pane() {
    // A tab whose visible float overlay covers part of the block. A click on the
    // float resolves to FocusFloatingPane; elsewhere falls to the tiled pane.
    let tab_layout = vec![hit_active(0, 10, 20)];
    let mut g = geom(10, 20, &[(0, 0, 0, 100, 40)]); // tiled fills block
    g.visible_floats = vec![minimap::PaneRect::new(7, 30, 12, 40, 16, "f", false)];
    let tab_panes: BTreeMap<usize, TabPaneGeom> = [(0usize, g)].into_iter().collect();
    // A click at the float's center (block-local ~col 8, row 1 → absolute col 18).
    assert_eq!(
        route_click(None, &[], &tab_layout, &tab_panes, 1, 18),
        ClickIntent::FocusFloatingPane(7),
    );
}
```

- [ ] **Step 2: テストを実行して失敗を確認**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib router`
Expected: FAIL（`visible_floats` 未定義）

- [ ] **Step 3: `TabPaneGeom` に `visible_floats` を足し、`float_at` を拡張**

`TabPaneGeom` にフィールド追加:

```rust
    /// Visible floating panes overlaid this frame (#110), for the visible-layer
    /// hit-test. Empty when the tab's layer is hidden / off / has no floats.
    pub(crate) visible_floats: Vec<minimap::PaneRect>,
```

`float_chip_at` の隣に、表示中フロートのヒットテストを追加し、`route_click` のフロート判定を「チップ or オーバーレイ」に統合:

```rust
/// The visible floating pane id drawn at click (`row`, `column`) — the overlay
/// counterpart of [`float_chip_at`] (#110). `None` when the tab has no visible
/// floats or the cell is off every float box.
pub(crate) fn float_overlay_at(
    tab_layout: &[line::TabHit],
    tab_panes: &BTreeMap<usize, TabPaneGeom>,
    row: isize,
    column: usize,
) -> Option<usize> {
    let row = usize::try_from(row).ok()?;
    let position = line::position_at_column(tab_layout, column)?;
    let geom = tab_panes.get(&position)?;
    if geom.visible_floats.is_empty() {
        return None;
    }
    let col = column.checked_sub(geom.start)?;
    minimap::float_pane_at_cell(
        &geom.panes,
        &geom.visible_floats,
        geom.width,
        geom.rows,
        geom.vinset,
        col,
        row,
    )
}
```

`route_click` のフロート段を、チップ→オーバーレイ両対応に:

```rust
    if let Some(id) = float_chip_at(tab_layout, tab_panes, row, column)
        .or_else(|| float_overlay_at(tab_layout, tab_panes, row, column))
    {
        return ClickIntent::FocusFloatingPane(id);
    }
```

`router.rs` テストの `geom` fixture に `visible_floats: Vec::new()` を追加。

- [ ] **Step 4: `lib.rs` で `visible_floats` を記録**

`render` の `tab_panes` 構築の `router::TabPaneGeom { ... }` に:

```rust
                        visible_floats: match floats_by_position.get(&hit.position) {
                            Some(floating::FloatSpec::Visible(rects)) => rects.clone(),
                            _ => Vec::new(),
                        },
```

`lib.rs` テストの `geom` fixture にも `visible_floats: Vec::new()` を追加。

- [ ] **Step 5: テストを実行して成功を確認**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib`
Expected: PASS

- [ ] **Step 6: wasm ビルドを確認**

Run: `cargo build --target wasm32-wasip1`
Expected: 成功

- [ ] **Step 7: コミット**

```bash
git add src/router.rs src/lib.rs
git commit -m "feat(router,lib): resolve visible-float overlay clicks (#110)"
```

---

## Phase 4 — ホイール walk へフロート合流（TDD、P0-3 でゲート）

> **P0-3 分岐:** 自動 hide が確認できた場合のみ、非表示フロートを walk に含める（reveal は一時表示）。確認できなければ walk はタイル＋表示中フロートのみ。以下は「自動 hide = YES」前提。

### Task 12: `pane_focus_order` にフロートを合流し、種別で reveal を切替

**Files:**

- Modify: `src/lib.rs`

- [ ] **Step 1: 失敗するテストを書く（`lib.rs`）**

`scroll_state` fixture を拡張してフロートを持たせるバリアントを追加し:

```rust
#[test]
fn pane_focus_order_appends_floats_after_each_tabs_tiled_panes() {
    // Tab 0 has tiled 10, 20 and a float 15; tab 1 has tiled 30. The walk visits
    // each tab's tiled panes in reading order, then its floats: [10, 20, 15, 30].
    let mut state = scroll_state(scroll::ScrollMode::Pane, Some(10));
    state.panes.panes.get_mut(&0).unwrap().push(PaneInfo {
        id: 15,
        is_floating: true,
        pane_x: 5,
        pane_y: 5,
        ..Default::default()
    });
    assert_eq!(state.pane_focus_order(), vec![10, 20, 15, 30]);
}
```

- [ ] **Step 2: テストを実行して失敗を確認**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib pane_focus_order`
Expected: FAIL（現状フロートは含まれない）

- [ ] **Step 3: `pane_focus_order` にフロートを合流**

`src/lib.rs` の `pane_focus_order`（469–476行目）を、タブごとにタイル→フロートの順で連結するよう変更:

```rust
    fn pane_focus_order(&self) -> Vec<u32> {
        let mut tabs: Vec<&TabInfo> = self.tabs.iter().collect();
        tabs.sort_by_key(|tab| tab.position);
        tabs.into_iter()
            .filter_map(|tab| self.panes.panes.get(&tab.position))
            .flat_map(|panes| {
                // Tiled panes in reading order, then this tab's floats (#110). A
                // float reached by the wheel is revealed+focused; if zellij auto-
                // hides the layer on the next focus move, the reveal is transient.
                let mut order = projection::pane_ids_in_reading_order(panes);
                order.extend(
                    projection::project_floating(panes)
                        .into_iter()
                        .map(|f| f.id as u32),
                );
                order
            })
            .collect()
    }
```

- [ ] **Step 4: `scroll_panes` の reveal を種別で切替**

`scroll_panes`（436–446行目）の `focus_terminal_pane(target, false, false)` を、対象がフロートなら `true` にする:

```rust
        // A float target may be hidden, so reveal it (#110); a tiled target is
        // always visible (#74). Look the id up in the live manifest to classify.
        let is_float = self
            .panes
            .panes
            .values()
            .flatten()
            .any(|p| p.id == target && projection::is_floating_terminal(p));
        focus_terminal_pane(target, is_float, false);
```

- [ ] **Step 5: テストを実行して成功を確認**

Run: `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib`
Expected: PASS

- [ ] **Step 6: コミット**

```bash
git add src/lib.rs
git commit -m "feat(lib): walk floating panes with the wheel, revealing hidden ones (#110)"
```

---

## Phase 5 — ドキュメント

### Task 13: `floating` config キーを README に文書化

**Files:**

- Modify: `README.md`

- [ ] **Step 1: config キー表に `floating` を追記**

`README.md` の設定キー一覧（`scroll` / `close_button` などが並ぶ表 or 節）に、同じ体裁で追加:

```markdown
| `floating` | `hybrid` \| `off` | `hybrid` | フロートペインの描画。`hybrid` は表示中のフロートをミニマップ上にオーバーレイし、非表示のフロートを右下隅の個別チップで表示（チップのクリックで表示＋フォーカス）。`off` はフロートを描かない（#110 以前の見た目）。 |
```

（実際の表の列構成・言語は既存 README に合わせる。英語 README ならこの説明文も英語にする — リポジトリの慣例に従う。）

- [ ] **Step 2: 変更を確認してコミット**

Run: `git diff README.md`

```bash
git add README.md
git commit -m "docs: document the floating config key (#110)"
```

---

## Self-Review（プラン執筆後の自己点検）

**1. Spec coverage（設計仕様の各節にタスクが対応するか）:**

- §4 デフォルト ON → Task 2（`DEFAULT_FLOATING = Hybrid`）。
- §4 個別チップ → Task 4/5/7/8。
- §4 フローティングのみ（suppressed 除外）→ Task 3（`is_floating_terminal` が `is_suppressed` を除外）。
- §5 タブ単位 B/A → Task 8（`are_floating_panes_visible` で Hidden/Visible を選択）。
- §6.1 オーバーレイ（bbox 非拡張）→ Task 9/10。
- §6.2 チップ（`+k` 畳み）→ Task 4/5。
- §6.3 `floating.rs` dependency-free → Task 1/4/5。
- §7.1 クリック解決順（フロート優先）→ Task 7/11。
- §7.2 `focus_terminal_pane(id, true, false)` → Task 8。
- §7.3 ホイール walk → Task 12（P0-3 ゲート）。
- §8 `FloatingMode`/config → Task 1/2。
- §9 P0 スパイク → Task 0。
- §10 段階実装 → Phase 0–4 と一致。

**2. Placeholder scan:** 「TODO/後で」等の未定義参照なし。P0 分岐（チップ→バッジ縮退、walk 絞り込み）は「実機結果で確定する条件付き設計」であり placeholder ではない。

**3. Type consistency:** `FloatingMode`（floating.rs）、`FloatLayer<'a>`（None/Hidden(&[usize])/Visible(&[PaneRect])）、`FloatSpec`（None/Hidden(Vec)/Visible(Vec)＋`layer()`）、`Chip`（Float(usize)/PlusK(usize)）、`ClickIntent::FocusFloatingPane(usize)`、`TabPaneGeom.hidden_floats: Vec<usize>` / `.visible_floats: Vec<PaneRect>`。`project_floating -> Vec<PaneRect>`、`chip_cells -> Vec<(usize, Chip)>`、`chip_index_at_cell -> Option<usize>`、`float_chip_at`/`float_overlay_at -> Option<usize>`、`bbox_of -> (u32,u32,f64,f64)`、`project_floats_into -> (Vec<Option<usize>>, Vec<PaneBox>)`、`float_pane_at_cell -> Option<usize>`。全タスク間で一貫。

**4. Ambiguity:** チップグリフ `◲`（`CHIP_GLYPH`）、overflow `⋯`（`CHIP_MORE_GLYPH`）を明示。チップは最終テキスト行のみ。フロート優先＝route_click で close の後・tiled pane の前。

## 実行方法（このプラン完了後）

Phase 0（スパイク）は実機検証タスクなので、subagent-driven ではなくインタラクティブに実施し、結果で P2/P4 の分岐を確定してから Phase 1 以降を進めること。

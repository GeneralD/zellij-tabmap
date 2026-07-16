# 設計仕様: +k オーバーフローマーカーの到達可能化（#113）

> 内部計画ドキュメント。`docs/design.md` と同じく日本語で記述する（本リポジトリの
> 計画ドキュメントの慣例）。コード識別子・zellij 型名は原語のまま。

## 1. 動機

タブの隠れフロートがチップ列（`floating::chip_cells`）に収まらないとき、末尾は
`⋯`（`Chip::PlusK(k)`）に畳まれる。PR #112 のレビュー修正で、マーカーへのクリックは
下のタイルペインへフォールスルーしない **no-op** として消費されるようになったが、
畳まれたフロート自体は**数として見えるのにバーから到達できない**。#113 はこの
「count が約束する到達可能性」を届ける。

## 2. 調査結果（既存コード）

- `chip_cells(cols, count)`（`src/floating.rs`）: `count <= cols` は全部 `Float(i)` を
  右詰め。オーバーフロー時は `shown = cols - 1` 個のチップ（index `0..shown`）＋
  block-local col 0 に `PlusK(k)`（`k = count - shown`）。**畳まれるのは index
  `shown..count`** — つまり折り畳み先頭 index は `shown = count - k`。
- `chip_marker_k(cols, text_rows, count, col, row)`: マーカーセルなら `Some(k)`。
  doc は「the marker itself selects nothing」と明記（#113 で書き換え対象）。
- `router::chip_marker_at` はこの `k` を引き、`route_click` は
  `is_some() → ClickIntent::NoOp` でクリックを消費（`src/router.rs` ~L224）。
- チップ→id 解決は `TabPaneGeom.hidden_floats: Vec<usize>`（**chip_cells と同順**の
  id 列）経由。`float_chip_at` が `chip_index_at_cell` の index で
  `hidden_floats.get(index)` を引く。
- `lib.rs` の `ClickIntent::FocusFloatingPane(id)` ディスパッチは
  `focus_terminal_pane(id, /*should_float_if_hidden=*/true, false)` — **隠れレイヤー
  ごと表示**する（rule #18: 隠れフロートの id は隠れている間も有効）。

## 3. 決定事項（ブレインストーミングで確定）

| 論点 | 決定 |
|---|---|
| 方式 | **reveal+focus**。`⋯` クリック＝**先頭の折り畳みフロート**を `FocusFloatingPane`。reveal がレイヤーごと表示するので、残りの折り畳み分もオーバーレイ矩形として即・個別クリック可能（全フロート ≤2 クリック到達）。#110 の「チップクリック＝reveal+focus を1ステップで」という設計言語の延長。 |
| 却下案 | ページング（タブ毎 page 状態・リセット規則・1セルでのページ表示が必要で、純粋パイプラインに可変 UI 状態が入る）／ピッカー（ミニマップにポップアップ語彙を新設、稀なエッジケースに過大）。 |
| intent / 状態 / 権限 / config | **すべて追加なし**。既存 `FocusFloatingPane` を再利用（`ChangeApplicationState` グラント内）。 |
| 見た目 | **変更なし**。純インタラクション（マーカーの描画・チップ列はそのまま）。 |

## 4. 設計

### 4.1 折り畳み先頭 index の導出

マーカー自身の `k` から純粋に導ける: `first_folded_index = hidden_floats.len() - k`
（`chip_cells` の畳み境界 `shown = count - k` と恒等）。`cols == 1` の全畳み
（`shown = 0`, `k = count`）でも `len - k = 0` で成立。`chip_cells` をミラーする
新ヘルパーは不要 — draw と hit-test の単一情報源は `chip_marker_k` のまま。

### 4.2 router

- `chip_marker_at` を **`chip_marker_target_at`** に改名・改務: マーカーセルなら
  `k` を引いて `hidden_floats.get(len - k)` の **id** を返す（従来の「k を返して
  no-op 消費させる」役目は消滅、両立させない）。
- `route_click` のマーカー分岐は `ClickIntent::FocusFloatingPane(id)` を返す。
  分岐順は不変（float chip / overlay → **marker** → tiled `pane_at` → switch）
  なので、マーカーが下のタイルペインを遮蔽する既存挙動（#110）は保たれる。

### 4.3 doc 整合

- `src/floating.rs`: `chip_marker_k` doc の「selects nothing / consume as no-op」、
  `Chip::PlusK` 周辺 doc を #113 の挙動（先頭折り畳みへの reveal+focus）に更新。
- `src/router.rs`: L83 付近（「decorative stand-in」）・route_click コメント・
  マーカーテストのコメントを更新。
- `docs/superpowers/specs/2026-07-12-floating-panes-in-minimap-design.md`:
  L102「`+k` は個別選択不可」／ L177 リスクテーブル「個別選択はチップ表示分のみ」を
  #113 反映に書き換え（到達手段: マーカー→reveal→オーバーレイで個別クリック）。

## 5. インタラクション

LeftClick のみ。ホイール・描画・チップ列レイアウトは変更しない。

## 6. Config / 権限

追加なし（§3）。既存ユーザーの権限グラントに触れない（#15 の凍結トラップ回避）。

## 7. テスト方針

`router::route_click` の回帰ピン（native `cargo test --lib`）:

1. オーバーフロー構成（`chip_cells(3, 5)`、ids `[101..105]`）でマーカーセルクリック →
   `FocusFloatingPane(103)`（= 先頭折り畳み: index `5 - 3 = 2`）。タイルへ落ちない
   （遮蔽維持）ことも同アサートで担保。
2. 同構成で実チップのクリックは従来どおり（`Float(1)` → id 102）。
3. `cols = 1` の全畳み（`chip_cells(1, 3)`）→ マーカーは index 0 の id を返す。
4. `count <= cols`（マーカー不在）の挙動は不変（既存テストが担保）。

wasm ビルドと CI-exact clippy（`cargo clippy --target wasm32-wasip1 --all-features
--lib`）も通す。

## 8. スコープ外 / 先送り

- ページング / ピッカー UI（§3 で却下）。
- マーカーの見た目変更（`⋯` のまま）。
- 隠れフロートのホイール到達（floating 設計の既存方針: wheel はチップを歩かない）。

# pinned フロートのミニマップ上での区別 — 設計 (#119)

- Issue: [#119](https://github.com/GeneralD/zellij-tabmap/issues/119)
- 日付: 2026-07-17
- 前提バージョン: zellij 0.44.3 / zellij-tile 0.44.3

## 1. 背景と目的

zellij の pin 機能（`toggle-pane-pinned`）はフロートを「フローティング
レイヤーを隠しても画面に残り続ける」状態にする。現状のミニマップは
pinned と通常のフロートを見分けられず、さらに **レイヤー hidden 時の描写
が実態とずれる**: pinned フロートは実画面に残っているのに、バーは全フロー
トをチップ（◲）に畳んでしまう。

本設計は次の 2 点を実現する:

1. **視覚 cue** — pinned フロートのオーバーレイ右上角に 1 セルのピン
   マーカーを描き、通常フロートと区別する。
2. **描写の正確性修正** — レイヤー hidden 時、pinned フロートはチップに
   畳まずオーバーレイのまま描く（実画面と一致させる）。非 pinned のみ
   チップ化する。

`PaneInfo` には pinned 相当のフィールドが無いが、スパイク
（2026-07-16 実施、issue #119 のコメント参照）で
`dump_session_layout_for_tab` 経由で読み取れることを実機確認済み。

## 2. スパイクで確認済みの事実

| # | 事実 | 帰結 |
|---|---|---|
| S1 | `dump_session_layout_for_tab(tab_index)` は `ReadApplicationState` のみで呼べる（zellij 本体 `check_command_permission` で確認） | 新規パーミッション不要 → トラップ #15（既存ユーザーのバー凍結）を踏まない |
| S2 | pinned フロートは KDL に子ノード `pinned true` として出る。unpinned では**ノード自体が無い** | 文字列パースで判定可能 |
| S3 | `toggle-pane-pinned` は即座（<1s）に `PaneUpdate` を発火する | PaneUpdate 駆動で pin 変化を追従できる |
| S4 | KDL の float の `x/y/width/height` は `PaneInfo.pane_x/pane_y/pane_columns/pane_rows` と**同一セル座標系で完全一致** | ジオメトリ相関で id ↔ pinned を対応付けできる（KDL に pane id は無い） |
| S5 | `LayoutMetadata` は pinned を運ばない | 返り値の KDL **文字列**をパースするしかない |

## 3. 設計

### 3.1 データ取得 — PaneUpdate 毎に dump、キャッシュしない

`Event::PaneUpdate` 処理時、マニフェスト上フロート（`is_floating &&
!is_plugin`、visible/hidden 問わず）を 1 つ以上持つ各タブについて
`dump_session_layout_for_tab(tab_index)` を呼ぶ。

- **キャッシュしない理由**: pin のトグルは `PaneInfo` のどのフィールドに
  も現れない（S3 で PaneUpdate は来るが差分が無い）。マニフェスト内容を
  キーにした「変化した時だけ dump」は pin トグルを取りこぼす。
- dump はホスト呼び出しとして同期的に返る（stdin 読み戻し）ので、同じ
  `update()` 内でパース・相関まで完結する。非同期状態機械は不要。
- フロートを持たないタブは呼ばない（大半のタブでコストゼロ）。

### 3.2 KDL パース — 純粋・zellij 型非依存・native テスト対象

新モジュール `src/pinned.rs`（純粋関数のみ）:

```text
pinned_float_rects(kdl: &str) -> Vec<CellRect>
```

- KDL 文字列から `floating_panes` ブロック内の各 pane の
  `x/y/width/height` と `pinned true` の有無を**文字列レベル**で抽出し、
  pinned なものの矩形だけ返す。
- `width 60%` のようなパーセント座標は**セル値に解決できないので当該
  float をスキップ**（pinned 判定なし＝劣化許容）。実セッションの dump
  は実測セル値を出すことを S4 で確認済みだが、防御的に許容する。
- kdl クレート等の依存は追加しない。必要な形（`pane` ノードの引数と
  子ノード）だけを狙った軽量パースに留める。
- renderer と同じ規律で zellij 型を持ち込まず、`cargo test --lib`
  （native）で網羅する。

### 3.3 相関 — ジオメトリ完全一致で id に pinned を付与

```text
pinned_ids(pinned: &[CellRect], floats: &[PaneRect]) -> Vec<usize>
```

- S4 の座標一致を根拠に、pinned 矩形と完全一致する manifest フロートの
  id を pinned 扱いにする。
- **同一ジオメトリのフロートが複数ある場合は一致した全 id に付与**
  （曖昧さは劣化として許容。誤爆しても cue が 1 個余分に付くだけで、
  クラッシュや誤操作にはつながらない）。
- `lib.rs` が保持する状態はタブ index → pinned id 集合のマップ 1 つ。
  PaneUpdate 毎に全置換する（3.1 の通り増分更新はしない）。

### 3.4 描画 — 右上角 1 セルのピンマーカー

- pinned フロートのオーバーレイ矩形の**右上角セル**にマーカーグリフを
  1 文字描く（テキストセル描画 — float ラベルと同じ機構）。
- グリフは**標準 Unicode・単幅**から選ぶ: 第一候補 `⌖` (U+2316
  POSITION INDICATOR)、次点 `⚲` (U+26B2)、`◉` (U+25C9)。Nerd Font には
  依存しない（既存の ◲ ◳ ⋯ と同じ方針）。最終選定は実装時に実描画で
  確認して決める。
- fg 色は**コーナーマーカー語彙**（#118 の suppressed マーカーと同族）:
  ベースは `float_ring_for(id, true)` — フォーカス状態に依らず常に
  全強度のリング色。bg はフロートの塗り。非アクティブタブは fg を
  `INACTIVE_LABEL_BLEND` でブレンド。ラベルの `ACTIVE_FG` 規則は
  適用しない（マーカーは「どのフロートか」を色で示す記号であり、
  ラベルの可読性規則とは目的が異なる）。
- **サイズゲート**: マーカーセルがフロートの内側に確保できない極小
  フロート（内側幅 < 2 セル等、閾値は実装時に確定）では黙って省略する。
  float ラベルの min サイズゲートと同じ流儀。
- ラベルとマーカーが同じ行にかかる場合はマーカー優先でラベル側を
  切り詰める（チップセル予約と同じ「予約セル」扱い）。

### 3.5 レイヤー hidden 時の描写修正

- projection のフロート分類を「レイヤー可視 → 全部オーバーレイ / 不可視
  → 全部チップ」から、「**不可視でも pinned はオーバーレイ**、非 pinned
  のみチップ」に変える。
- チップ数（+k マーカーの k を含む）は非 pinned の hidden フロートのみ
  で数える。
- hidden 中でもオーバーレイとして描かれる pinned フロートは、実画面でも
  見えているので**ヒットテスト・ホイール対象としても可視フロートと同じ
  扱い**にする（クリックで `FocusFloatingPane(id)` — 既存 intent）。
- **検証結果**（2026-07-17、zellij v0.44.3 ソースの静的読解で確認済み）:
  - V1 = **YES** — レイヤー hidden 時も pinned フロートは描画され続ける
    （`floating_panes/mod.rs:438-447` が `!show_panes` 時に描画対象を
    `is_pinned` のみへフィルタ。e2e スナップショット
    `pin_floating_panes.snap` も同挙動を固定）。
  - V2 = **YES** — hidden 状態のフロートも dump に含まれる
    （`screen.rs::get_layout_metadata` は `get_floating_panes()` を
    無フィルタで走査。`hide_floating_panes` はタブの KDL 属性になる
    だけで float の列挙には影響しない）。
  - いずれも YES のため、当初用意したフォールバック（V1 否なら 3.5 を
    落とし cue のみ出荷 / V2 否なら可視時相関の保持で代替）は不要だった。

### 3.6 エラー処理・劣化

| 状況 | 挙動 |
|---|---|
| `dump_session_layout_for_tab` が `Err` | そのタブは pinned 無しとして描画（従来動作）。ログのみ、リトライしない |
| KDL パース不能・想定外構造 | パーサは空集合を返す（従来動作に劣化） |
| パーセント座標 | 当該 float のみ pinned 判定スキップ |
| 同一ジオメトリ | 一致全部に cue（3.3） |

いずれもバーの描画自体は止めない。pinned cue は常に「無くても機能が
壊れない付加情報」として扱う。

## 4. テスト戦略

- **native (`cargo test --lib`)**: KDL パーサ（正常系・pinned 無し・
  パーセント座標・壊れた入力）、相関（一致・不一致・同一ジオメトリ）、
  描画（マーカー位置・サイズゲート・ラベルとの共存・hidden 時の
  オーバーレイ/チップ分類）。
- **wasm-only（カバレッジ対象外）**: `dump_session_layout_for_tab` の
  呼び出し行そのもの。stdin 読み戻し系ホスト呼び出しは native テストで
  panic する（トラップ #17）ため、決定ロジックを純粋関数に押し出して
  ホスト呼び出し 1 行だけを未カバーに残す — #17 と同じ分割。
- 実機確認: 検証ゲート V1/V2（3.5）と cue グリフの見た目（3.4）。

## 5. 非目標

- pin/unpin を**操作する** UI（クリックでピン留め等）— 読み取りのみ。
- `set_floating_pane_pinned` の利用（書き込み系、スコープ外）。
- チップ（◲）上での pinned 区別 — pinned は 3.5 によりそもそもチップに
  ならないため不要。
- パフォーマンス最適化（dump 呼び出しの間引き等）— 問題が観測されたら
  別 issue。

## 6. 影響範囲

| ファイル | 変更 |
|---|---|
| `src/pinned.rs` | 新規: KDL パース + 相関（純粋） |
| `src/lib.rs` | PaneUpdate で dump → 相関、pinned マップ保持、projection への受け渡し |
| `src/projection.rs` | pinned id 集合を受けて float 分類（3.5）と pinned フラグ付与 |
| `src/minimap.rs` | 右上角マーカー描画（3.4） |
| `src/floating.rs` / `src/router.rs` | チップ数の分母変更（非 pinned hidden のみ）に伴う追従 |
| `README.md` | pinned cue の説明追記 |

新規パーミッション: **無し**（S1）。config キー: **無し**。

# 設計仕様: オーバーレイ内のフロートラベル（#120）

> 内部計画ドキュメント。`docs/design.md` と同じく日本語で記述する（本リポジトリの
> 計画ドキュメントの慣例）。コード識別子・config キー・zellij 型名は原語のまま。

## 1. 動機

表示中フロートのオーバーレイ（#110・PR #112、focus ring #116、いずれもマージ済み）は、
各フロートを **色付き矩形＋1px リング**で描くが **ペインラベルを持たない**（floating 設計
§6.1・§12 で「面積が小さい」ため見送り）。タイルペインはラベルを持つのに対し、フロートは
色でしか区別できず、バーだけでは「どのフロートがどれか」が分かりにくい。

本仕様は、オーバーレイが**十分大きいとき**に限り、フロートのタイトルをオーバーレイ内部に
描く（タイルラベルの扱いをフロート層に写す）。小さなフロートは従来どおり色＋リングのみ。

## 2. 調査結果（既存コード）

- フロートオーバーレイは `render()` 内で **ピクセル単位**に描かれる（`float_px` クロージャ：
  境界ピクセル＝`float_ring_for`、内部＝`color_for`）。タイルの `overlay`（ラベル）とは別系統。
- `project_floats_into()` が各フロートの block-local 矩形 `float_bounds: Vec<PaneBox>`
  （`float_rects` と index 整合）と、ピクセル→フロート index の `float_grid: Vec<Option<usize>>`
  を返す。`float_bounds[i]` がラベルを置くべき矩形そのもの。
- `float_rects: &[PaneRect]` は id / title / focused を持つ。
- オーバーレイは**アクティブタブの表示中レイヤーのみ**描かれるので、ラベルは常にアクティブ配色
  （`ACTIVE_FG`）でよい。
- `LabelMode`（`None`/`Focused`/`All`）は **config ではなく** L0–L4 劣化ラダー
  （`tab_block.rs`）がタブの割当幅から選ぶ。L0=All / L1=Focused / L2+=None。
- タイルラベルの配置は `render()` 内インライン: `title::summarize` で内部幅に切り詰め →
  `charwise_width` で表示幅センタリング → `OverlayCell::Glyph`/`Continuation` を `overlay` に書く。
  全角/CJK もこの幅計算で1グリフ=占有列ぶんの Continuation を張るので崩れない。

## 3. 決定事項（ブレインストーミングで確定）

| 論点 | 決定 |
|---|---|
| ゲーティング | **`LabelMode` を再利用**（config 追加なし）。All=収まるフロート全部 / Focused=フォーカス中フロートのみ / None=出さない。幅に応じて自動劣化。 |
| サイズしきい値 | **控えめ**。オーバーレイ box が **内部幅 ≥ 4 cols（`fw-2 ≥ 4`）かつ 高さ ≥ 6px（3 text rows）** の時だけラベル。小・中フロートは色＋リングのみ。 |
| 配置 | tiled と DRY。`title::summarize` ＋ `charwise_width` ＋ `OverlayCell` を流用し、縦中央の**内部**行にセンタリング。 |
| 配色 | bg=フロート内部色 `color_for(id)`、fg=`ACTIVE_FG`。フォーカス中フロートは **bold**（タイル同様）。 |
| 権限 / config | **新規 zellij 権限なし・config キー追加なし**（純レンダリング、既存グラント内）。 |

## 4. 設計

### 4.1 サイズしきい値（控えめ）

`float_bounds[i] = PaneBox { px0, px1, py0, py1 }`（block ピクセル）に対し:

- **幅**: `fw = px1 - px0`。リングが左右1列ずつを占めるので内部幅 = `fw - 2`。ラベルは
  内部幅 `≥ 4`（＝ `fw ≥ 6`）を要求 → 切り詰めても3〜4字の可読ラベルが載る。
- **高さ**: `fh = py1 - py0`。リングが上下1px ずつを占めるので、ラベル（1 text row＝2px）が
  **上下リングを潰さず内部行に載る**には `fh ≥ 6`（3 text rows）が必要。
  - 実測: `fh=4`（px 0–3、リング px0/px3）は完全内部の text row が取れない。`fh=6`
    （px 0–5、リング px0/px5）で内部 text row `tr=1`（px2,3）が取れる。よって `fh ≥ 6`。
- 定数（内部幅 4 / 高さ 6px）は名前付き定数にして後から調整可能にする。

いずれかを満たさないフロートはラベルを描かない（潰れた描画をしない、floating 設計 §6.1 の
「面積が小さい時は色＋ボーダーのみ」を踏襲）。

### 4.2 配置

しきい値を満たすフロートについて:

- ラベル行 = オーバーレイの**縦中央の内部 text row**（`2*row > py0` かつ `2*row+1 < py1-1`
  を満たす中央行）。3行以上あるので必ず1つ存在する。
- `title::summarize(title, inner_width, false)` で内部幅（`fw-2`）に切り詰め、`charwise_width`
  で表示幅を測ってセンタリング開始列 `px0 + 1 + (inner - label_width)/2` を求める。
- 幅つき char スキャン（タイルと同じ `UnicodeWidthChar::width` フィルタ＋`scan`＋`take_while`）で
  専用オーバーレイ `float_overlay: Vec<Option<OverlayCell>>`（`text_rows * pw`）へ、先頭セルに
  `Glyph(ch, i)`、占有列に `Continuation` を書く。`i` はフロート index。
- 角の予約セル（バッジ/close/チップ/suppressed マーカー）は**気にしない**: それらの branch は
  float 描画より前で発火し `continue` するので、フロートラベルがその列に来ても描かれず自然に
  譲る（フロート塗り自体もそれらに負けるのと同じ挙動）。

### 4.3 レンダリング

`render()` のペイン塗りループ、**タイルラベル match の後・`put_halfblock` の直前**（float 塗り段）に
フロートラベル分岐を追加する:

- 当該セルに `float_overlay` があり、かつ **その cell が当該フロートの内部**
  （`float_grid[2*tr*pw+c] == Some(i)` かつ `float_grid[(2*tr+1)*pw+c] == Some(i)`）なら:
  - `Glyph(ch, i)`: bg=`color_for(float_rects[i].id)`、fg=`ACTIVE_FG`、`focused` なら bold で `ch` を出力、`continue`。
  - `Continuation`: 無出力で `continue`（先頭の全角グリフが advance 済み。#118 で直した行幅崩れの再発防止）。
- 所有チェックを両ピクセルに課すことで、フロートが重なった領域では**最上位フロートのラベルだけ**が
  出る（`float_ring` と同じ所有判定）。フロートは drop-shadow を受けない（既存方針）ので、ラベル
  背景に影は掛けない。
- タイルラベルは、フロートに覆われたセルでは `resolve_label_plan` により `Fill` に落ちる（#110 の
  occlusion）ので、フロートラベルと二重描画しない。

### 4.4 モジュール境界 / テスト（rule #7 の二分割）

- ミニマップは依存ゼロのレンダラ（zellij 型なし）。フロートラベルも `render()` の文字列出力に対する
  単体テストで検証する（#118 と同じ paint 側検証）。データ/投影側は既存 `project_floats_into` の
  テストで担保済み。

## 5. インタラクション

なし。ラベルは**描画のみ**。クリック解決順（floating 設計 §7.1、`router.rs`）は変更しない。

## 6. Config / 権限

- **config キー追加なし**。ラベルの ON/OFF は既存 `LabelMode`（ラダー）にぶら下がる。
- **新規 zellij 権限なし**（read-only、既存グラント内）。既存ユーザーの権限グラントを変えない
  （#15 の凍結トラップに触れない）。

## 7. テスト方針

`render()` の文字列アサートで検証する:

1. 大きい表示中フロート ＋ `LabelMode::All` → そのタイトル文字が出力に現れる。
2. 小さいフロート（幅 or 高さがしきい値未満）→ タイトル文字が出ない（色のみ）。
3. `LabelMode::Focused` → フォーカス中フロートのみラベル、非フォーカスは出ない。
4. `LabelMode::None` → どのフロートもラベルなし。
5. フォーカス中フロートのラベルは bold（`\x1b[1m`）で出る。
6. 全角/CJK タイトルのフロートで、行の可視セル数（列幅）が崩れない。
7. 重なる2フロートで、下側フロートのラベルが上側にはみ出さない（最上位のみ描画）。

native ホストトリプルで `cargo test --lib`。wasm ビルドと CI-exact clippy も通す。

## 8. スコープ外 / 先送り

- 隠れフロート**チップ**のラベル（チップは1セルなので不可）。
- **非アクティブタブ**のフロートラベル（オーバーレイ自体がアクティブタブ限定）。
- **pinned フロート**の識別（#119、zellij の `is_pinned` 待ちでブロック）。
- ラベルの**複数行**表示（1 text row のみ、タイル同様）。

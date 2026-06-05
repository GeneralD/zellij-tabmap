# zellij-tabmap 設計書

3 行高さの横並びタブブロックに、各タブのペインレイアウトを **色分けハーフブロック・ミニマップ**として描く zellij プラグイン (Rust/WASM)。各ペインは色で識別し、幅に余裕があれば**要約タイトル**を重ねる。`⌘N` ショートカット番号を併記し、v2 で**ドラッグ&ドロップ並べ替え**を載せる。

- **由来**: 個人 dotfiles のアイデア(タブ名へのペインレイアウト・ミニグリフ案)を、`precmd` 起点の zsh + `rename-tab` 方式からイベント駆動プラグイン方式へ昇華したもの。
- **golden repository**: `KiryuuLight/zellij-attention`(実運用中の Rust 製 zellij プラグイン)を構造リファレンスに採用。

---

## 0. 確定事項(ブレストで決定済み)

| 項目 | 決定 |
|---|---|
| リポ名 | `zellij-tabmap`(新規 public、MIT) |
| 言語 / 配布 | Rust → WASM、`zellij-tile = 0.44.3` pin、GitHub Release の wasm URL 直読み |
| タブ配置 | **上に横並び・3 行ブロック**(全タブ高さ 3 行で揃う、幅のみ可変) |
| ミニマップ描画 | **色分けハーフブロック・ピクセルグリッド**(`▀` で縦 2 倍解像度=縦 6px) |
| タイトル | **要約ラベル**(先頭コマンド名 / Nerd Font アイコン / 幅依存切詰)、幅に余裕のあるペインに重畳 |
| 色 | テーマパレット循環割当(Tokyonight 整合)、focused は accent / bold |
| attention 連携 | **coexist**(共存)。ミニマップ幾何は全 plugin pane を無条件除外 |
| D&D 並べ替え | **実装可能、v2 配置**(`run_action(MoveTabByTabId)`、`RunActionsAsUser` 権限) |
| v1 スコープ | 3 行ブロック + 精密色グリッド + 要約タイトル。**最初のタスクは座標原点+色出力スパイク** |

---

## 1. モチベーション — なぜ plugin / なぜ色グリッド

### zsh 方式を廃してプラグイン方式にする理由

| 観点 | zsh + rename-tab(削除済み) | プラグイン方式(本設計) |
|---|---|---|
| レイアウト変更検知 | `precmd` 起点のみ(分割/クローズは次コマンドまで反映遅延) | `Event::PaneUpdate` を**実イベント**で受信。分割した瞬間に再描画 |
| 描画予算 | タブ名 1〜2 セルに圧縮、本物のミニマップ不可 | **3 行 × タブ幅 = 縦 6px の 2D キャンバス** |
| ガード複雑性 | 自リネーム / attention の 󰔟󰄵 / 手動リネームを正規表現で区別(脆い) | **タブ名を書き換えないのでガード問題が消滅** |
| タイトル取得 | `list-panes --json` を毎回フォーク | `PaneInfo.title` をイベントで受領、フォークなし |

### なぜ色グリッド(ハーフブロック)か

box-drawing 罫線は「行と行の境目」にしか線を引けないが、`▀`(上半分=前景色 / 下半分=背景色)を使うと **1 セル内で色が変わる**ため、半端な位置の分割境界も忠実に出る。色でペインを識別するので「細かく区切られたパネル」も潰れず表現できる。これは画像→端末描画ツール(chafa / timg 等)が使う **ハーフブロック・レンダリング**の応用。

```
 テキスト3行        左A(全高)    右: 上B / 下C を ▀ で縦2分割
   行1   │ █ A █ │ ▀▀▀   fg=B bg=B  (上も下もB)
   行2   │ █ A █ │ ▀▀▀   fg=B bg=C  (上半分B / 下半分C ← 分割線がセル中央)
   行3   │ █ A █ │ ▀▀▀   fg=C bg=C  (上も下もC)
```

端末セルは縦長(約 1:2)なので、ハーフブロックで縦を割ると 1 サブセルがほぼ正方になり、レイアウト形状の歪みも減るオマケがある。

---

## 2. アーキテクチャ(イベント駆動)

```
load():
  set_selectable(false)                      // size=N 固定の安定条件(必須)
  request_permission([ReadApplicationState, ChangeApplicationState])  // 読取=購読 / 変更=switch_tab_to。v2 D&D で RunActionsAsUser 追加
  subscribe([TabUpdate, PaneUpdate, ModeUpdate, Mouse])
  Config::from_configuration(&cfg)

update(Event) -> bool(再レンダ要否):
  TabUpdate(Vec<TabInfo>)   -> self.tabs  = tabs;            true
  PaneUpdate(PaneManifest)  -> self.panes = manifest;        true
  ModeUpdate(ModeInfo)      -> self.style = mode.style;      差分時 true
  Mouse::LeftClick(_, col)  -> click_to_switch(col);         false  // v1: クリックでタブ切替
  Mouse::Hold/Release       -> (v2: ドラッグ並べ替え状態機械)
  PermissionResult          -> set_selectable(false);        true

render(rows=3, cols):
  1. タブ幅予算配分(active=満額, inactive=圧縮)
  2. アクティブ中央寄せパッキング + `← +N` / `+N →` オーバーフロー
  3. 各タブ -> 3 行の色グリッドブロックへ描画(後述 §4)
  4. 行ごとにカーソル配置 + 行末クリア(\u{1b}[0K)
```

### ファイル構成(golden repo の「薄い bin + 全ロジック lib + 純データ別ファイル」3 分割を踏襲)

| ファイル | 役割 | 区分 |
|---|---|---|
| `src/main.rs` | `register_plugin!(zellij_tabmap::State);` のみ(wasm bin エントリ) | golden 踏襲 |
| `src/lib.rs` | `State` + `ZellijPlugin` 実装(イベント処理) | golden 踏襲(構成) |
| `src/minimap.rs` | **純関数**: ペイン矩形群 → 色付きセル行列 `[[Cell; W]; 3]`(テスト主戦場) | 新規 |
| `src/title.rs` | **純関数**: ペインタイトル → 要約ラベル(コマンド名 / アイコン / 切詰) | 新規 |
| `src/tab_block.rs` | 1 タブ → 3 行ブロック組立(色グリッド + ラベル + ⌘N) | 公式 `tab.rs` 相当を縦化 |
| `src/line.rs` | タブ列パッキング + オーバーフロー(幅予算配分) | 公式 `line.rs` 移植 |
| `src/color.rs` | テーマパレットからペイン色を循環割当、focus 強調 | 新規 |
| `src/config.rs` | `Config::from_configuration(BTreeMap)` パース + テスト | golden 踏襲 |
| `src/tests.rs` | FFI スタブ(`host_run_plugin_command`)+ 単体テスト | golden 踏襲 |

### loading model は attention と「別物」(誤継承しない)

zellij-attention は `load_plugins{}` で起動する**不可視常駐シングルトン**で `zellij pipe` 駆動。本プラグインは `default_tab_template` 内の**レイアウト埋め込み UI バー**。この違いから:

- **権限は最小限**: v1 = `ReadApplicationState`(イベント購読)+ `ChangeApplicationState`(クリック切替の `switch_tab_to`)。Mouse 購読自体は無権限で受け取れるが、切替は状態変更なので `ChangeApplicationState` が要る。v2 の D&D 並べ替え(`run_action`)で `RunActionsAsUser` を追加。attention の `ReadCliPipes` / `MessageAndLaunchOtherPlugins` はコピーしない。
- `render` は空ではない(attention は空 render の不可視常駐)。
- `pipe` ハンドラは coexist 方針では不要。
- golden のリスク(`#4156` load_plugins 消失 / broadcast pipe / `rename_tab` 再入)は**いずれも非該当**。

---

## 3. データモデル

### 入力イベント(zellij-tile 0.44.3、一次ソース確認済み)

```rust
// Event::TabUpdate(Vec<TabInfo>)
struct TabInfo {
    position: usize,             // 0-indexed
    name: String,                // attention が付けた 󰔟/󰄵 を含む場合あり
    active: bool,
    display_area_rows: usize,    // タブ全体の高さ(正規化の分母)
    display_area_columns: usize, // タブ全体の幅
    // ...
}

// Event::PaneUpdate(PaneManifest)
struct PaneManifest { panes: HashMap<usize /*tab pos*/, Vec<PaneInfo>> }
struct PaneInfo {
    id: u32,
    is_plugin: bool,      // ← ミニマップから無条件除外
    is_focused: bool,     // ← focus 強調
    is_floating: bool,    // ← v1 は除外
    is_suppressed: bool,  // ← 除外
    title: String,        // ← 要約ラベルの元
    pane_x: usize, pane_y: usize, pane_columns: usize, pane_rows: usize, // 枠込み矩形
    // ...
}
```

### 内部状態

```rust
#[derive(Default)]
struct State {
    tabs: Vec<TabInfo>,
    panes: HashMap<usize, Vec<PaneInfo>>,
    style: Style,                 // ModeInfo 由来。テーマパレット / capabilities
    config: Config,
    tab_layout: Vec<TabHit>,      // 直近レンダの列範囲(クリック/ドラッグのヒットテスト用)
    drag: Option<DragState>,      // v2
}
```

### 変換パイプライン(タブごと、すべて純関数)

```
TabInfo + panes[pos]
  ▼ フィルタ: is_floating / is_suppressed / is_plugin を除外
  ▼ 正規化: 各矩形を bounding-box 基準で (W cols × 6 px) グリッドへ独立軸線形スケール
  ▼ 色割当: ペインに安定色(パレット循環)、focused は accent
  ▼ ピクセル塗り: 各セル = ▀(fg=上px色, bg=下px色)。境界は色変化で表現
  ▼ ラベル重畳: 面積が閾値以上のペインに要約タイトル(focused 優先)
  ▼ = TabBlock { lines: [StyledLine; 3], width, position }
```

> **タブの identity は `position`(zellij `TabInfo.position`、0-based)に一本化する。** `switch_tab_to` は **1-indexed** なので、クリック切替では `position + 1` に変換する(§8 のヒットテスト純関数 `line::switch_target_at_column` がこの変換を担い、単体テストで off-by-one を固定する)。ペインと違いタブには安定 id が無く、別個の `tab_id` を持つと §4.3 の「段は幅のみで決まる」純関数と矛盾するため持たない。クリック切替 / 並べ替えのヒットテストも同じ `position` を使う(§8, §11)。

> **座標原点(要実機検証)**: `pane_x/pane_y` は端末左上原点の絶対セル座標と server ソース(`pane_info_for_pane`)から論理確定。ただし doc 明記なし。bounding-box 正規化はタブ内トポロジのみ依存で原点ずれに頑健だが、着手前にスパイクで裏取りする(§9)。

---

## 4. レンダリング設計

### 4.1 色グリッド・ミニマップ

- **キャンバス**: タブ幅 `W` cols × 3 text rows → `▀` で **W × 6 px** のピクセルグリッド。各 text セル = 上 px(fg) / 下 px(bg)。
- **色割当**: テーマパレット(ModeInfo.style 由来)から色を循環割当。ペイン id で安定化(再描画でちらつかない)。focused ペイン = テーマの accent 色 or bold。color 非対応経路の保険に focused へ細枠/マーカーも可。
  - 任意 RGB ではなく**テーマパレット循環**を採用 → Tokyonight 整合 + RGB 対応有無の不確実性回避。
- **境界**: 色変化が境界。視認性のため隣接ペイン間に 1px の暗いガター(オプション)。

### 4.2 要約タイトル(`src/title.rs` 純関数)

ペインタイトル(既定=実行コマンド)を短いラベルへ要約してペイン領域に重畳する。**ラベルを置くセルはピクセルではなくテキストセル**(bg=ペイン色, fg=ラベル)になるため、面積に応じて出し分ける。

要約規則(優先順):
1. **Nerd Font アイコン写像** — 既知コマンド(nvim / cargo / node / git / python / docker …)はアイコン 1 グリフ。
2. **先頭トークン** — `nvim ~/.config/...` → `nvim`、パスは basename。
3. **幅依存切詰** — 残り幅に合わせ `car…` のように `…` 切詰(最小 3 桁)。
4. カスタムリネーム済みタイトルはそのまま尊重(要約せず切詰のみ)。

配置規則: ペインの描画面積(cols)が閾値以上ならラベル、未満なら色のみ。focused ペインは可能な限りラベルを出す。

### 4.3 縮退ラダー(段はタブ幅予算で一意決定 / ラベル局所縮退は領域高さ)

```
L0 色グリッド + 全(大)ペインに要約ラベル        active タブ(幅広)
L1 色グリッド + focused ペインのみラベル
L2 色グリッドのみ(ラベル無し)                  inactive タブ(中)
L3 単一代表グリフ(分割方向 / グリッド記号)      inactive タブ(狭)= 当初のグリフ案
L4 ⌘N のみ                                        極小
```

段は**タブの割当幅のみ**で一意に決まる純関数 (`tab_block::level_for`)。タブが増えて幅が縮めば自動で L3→L4 に落ちる(= 当初の単一グリフ案を同一コードパスで内包)。ペイン数による縮退は段選択には入れず、ラベル配置の**局所縮退**として下層 (`minimap` のラベルゲート) に分離した: 正規化後に 1 テキスト行高しかない領域はラベルを諦め色のみにする (`cell_text_rows >= 2` を要求)。これは垂直深さ ≥3 の縦スタックだけでなく、非対称な 2 分割で小さい側が 1 行高に潰れる場合も同様に効く — 「ペイン数」ではなく「領域高さ」が基準。

### 4.4 幅予算配分

```
total_W = cols - prefix(⌘ロゴ等)
active_block = clamp(満額, 16..=28)   // タイトル入り精密ミニマップ
per_inactive = max(2, (total_W - active_block) / (タブ数 - 1))
```

アクティブ中央寄せ + 左右均等パッキング。入りきらなければ `← +N` / `+N →` を端に挿入(公式 `line.rs` ロジック移植)。

**行アンカーの切替(設定キー `align`)**: 全タブが収まる場合の行頭位置は `align` で選べる。

- `center`(既定): アクティブブロックを帯の中央に寄せる。フォーカスが変わるたびに `row_start` が再計算され、帯全体が水平にスライドする(= v0.1.0 の挙動)。
- `left`: 行の**左端**を列0(プレフィックス直後)に固定する。これで帯全体のスライドはなくなる。ただし各タブの桁位置すべてが固定されるわけではない — アクティブは依然として幅広に描かれるため、フォーカスがアクティブを横切るとその右側のタブは再配置される(`active_w - inactive_w` 桁ぶん)。真に固定されるのは最左端のタブだけ。`align` が消すのは帯全体の平行移動であって、幅差による再配置ではない。

**スコープ — `align` は「全タブが収まるケース」専用**。タブが入りきらない場合は `packed_with_overflow` がアクティブ追従の可視ウィンドウを使い、`align` に依らず常にアクティブを画面内に保つ(左寄せにするとアクティブが画面外へ押し出されてしまうため、オーバーフロー時はアクティブ相対が必然)。

既定を `center` に据えているのは、自動更新で既存レイアウトの見えを変えないため(`reorder` の既定オフと同じ「v0.1.0 の挙動を保つ」方針)。`align` は `line::Alignment`(`config.rs` が `"left"` / `"center"` をパース、不正値は既定にフォールバック)として `line::pack` に渡る。

### 4.5 ⌘N ショートカット番号(表示のみ。切替バインドは既存)

各タブブロックに `⌘N`(位置ベースのタブ切替を可視化する意図)を描画。タブ名を書き換えないのでリネームガード問題は発生しない。

**役割分担**: 実際のタブ切替バインドは**プラグインではなくユーザーの `config.kdl`** が担う。多くの環境では既に `bind "Super 1" { GoToTab 1; }` 〜 `Super 9` 相当(macOS では Super=Cmd)が定義済みのため、**プラグイン側に追加のキーバインド実装は不要**で、「ヒント表示」専任とする(プレフィックス文字列は設定可能)。

**整合**: `Super N → GoToTab N` は 1-indexed、プラグイン表示は `TabInfo.position + 1`。両者は自動的に一致する。N≥10 はバインドが無い(`Super 10` 不可)ため、10 番目以降は `⌘` 接頭辞を付けず位置番号のみ表示する。

### 4.6 行末処理

各行末に背景色 + `\u{1b}[0K`。公式 1 行用 `print!` を流用すると 3 行で崩れるので、行ごとにカーソル位置制御 + 行末クリア。

### 4.7 レンダ例(概念。実際は色付き)

```
[1] 1pane     [2] 2col      [3] 2row      [4] 2x2       [5] main+stack
████████      ██▀▀████      ████████      ██▀▀████      ████▀▀██
██ nvim █      ██  ██ █      ██ log ██      ██ ██ ██      ██   ██ █
████████      ██▄▄████      ▀▀▀▀▀▀▀▀      ██▄▄████      ██   ██▄█
              (左右2色)    ██ test█      (4色)        ██   ████
                          ████████                   (左主+右2段)
```
(各 █/▀ は実際にはペイン色。ラベルは幅が許す大ペインのみ。)

---

## 5. レイアウト統合

### default.kdl の差し替え

現状(`~/.config/zellij/layouts/default.kdl`):
```kdl
default_tab_template {
    pane size=1 borderless=true { plugin location="tab-bar" }
    children
    pane size=1 borderless=true { plugin location="status-bar" }
}
```

差し替え後:
```kdl
default_tab_template {
    pane size=3 borderless=true {                       // 1 → 3
        plugin location="https://github.com/GeneralD/zellij-tabmap/releases/download/v0.1.0/zellij-tabmap.wasm" {
            shortcut_prefix "⌘"
            active_width "24"
            // gutter "true" など
        }
    }
    children
    pane size=1 borderless=true { plugin location="status-bar" }
}
```

- `size=3` 安定化のため `load()` で必ず `set_selectable(false)`。
- 配布は attention 実績(release URL 直読み)と同方式。zellij が初回 fetch + キャッシュ。
- **初回許可ダイアログ非表示問題(#4982)**: `default_tab_template` 起動プラグインは許可ダイアログを出せない。プラグインキャッシュへの事前許可記述を初回セットアップ手順に明記。

### zellij-attention との共存(coexist)

- attention は従来どおりタブ名に 󰔟/󰄵 を付与(pipe 駆動、レイヤー独立)。
- 本プラグインは `TabInfo.name`(= attention 付与の名前)+ 色グリッドを描画。
- ミニマップ幾何フィルタは **全 plugin pane を `is_plugin==true` で無条件除外**(自身・status-bar・attention すべて)。attention pane は不可視で描画すべき幾何を持たないため。
- v3 候補: attention の pipe を吸収して 󰔟/󰄵 相当をブロック内に直接描く absorb(権限拡張が必要、v1/v2 では採らない)。

---

## 6. ビルド / CI / リリース(golden 踏襲)

### Cargo.toml(lib + bin の 2 ターゲット、cdylib にしない)

```toml
[package]
name = "zellij-tabmap"      # 成果物 zellij-tabmap.wasm(bin 出力)
edition = "2021"

[lib]                       # rlib。全ロジック。x86_64 ネイティブテスト可
name = "zellij_tabmap"
path = "src/lib.rs"

[[bin]]                     # wasm エントリ。register_plugin! のみ
name = "zellij-tabmap"
path = "src/main.rs"

[dependencies]
zellij-tile = "0.44.3"      # 実機合わせ(golden の 0.43.1 は踏襲しない)

[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
panic = "abort"
```

`.cargo/config.toml`:
```toml
[build]
target = "wasm32-wasip1"    # 旧 wasm32-wasi ではない
```

FFI スタブ(ネイティブテストのリンクに必須):
```rust
// src/tests.rs
#[no_mangle]
pub extern "C" fn host_run_plugin_command() {}
```

### CI(2 本立て)

- **ci.yml**(push、3 ジョブ並列): clippy(`--target wasm32-wasip1 --lib`, `-Dwarnings`)/ test(`cargo test --lib --target x86_64-unknown-linux-gnu`)/ build-wasm + upload-artifact。
- **release.yml**(`v*.*.*` タグ): toolchain(wasm32-wasip1)+ cache → test → `cargo build --release` → git-cliff チェンジログ → `softprops/action-gh-release@v2` で `zellij-tabmap.wasm` 添付(`permissions: contents: write`)。

---

## 7. golden repo マッピング

| zellij-attention | zellij-tabmap | 区分 |
|---|---|---|
| `main.rs`(register のみ) | 同型 | 踏襲 |
| `lib.rs`(全ロジック) | `lib.rs`(State + ZellijPlugin) | 踏襲(構成) |
| `state.rs`(純データ) | `minimap.rs`/`title.rs`/`color.rs`(純関数) | 踏襲(分割思想) |
| `config.rs` | 同型 | 踏襲 |
| `tests.rs`(FFI スタブ) | 同型 + 変換テスト | 踏襲 |
| Cargo lib+bin / release プロファイル | 同一 | 踏襲 |
| ci.yml / release.yml / cliff.toml | 同一構成 | 踏襲 |
| `zellij-tile 0.43.1` | `0.44.3` | 変更 |
| loading: load_plugins 常駐 | default_tab_template 埋め込み | 逸脱 |
| 権限 4 種 | v1=2 種(Read + Change) / v2=3 種(+ RunActions) | 逸脱 |
| render 空 | 3 行描画 | 逸脱 |

---

## 8. 段階的スコープ(YAGNI)

### v1 — 色グリッド + 要約タイトル(MVP)
- **最初のタスク = スパイク**(§9): 座標原点 + 色出力の実機裏取り
- 3 行ブロック化 + `set_selectable(false)` で `size=3` 安定化
- 色グリッド・ミニマップ(`▀` 縦 2 倍)、テーマパレット色割当、focus 強調
- 要約タイトル(コマンド名 / アイコン / 切詰)、面積閾値で出し分け
- 縮退ラダー L0→L4、幅予算パッキング + `+N` オーバーフロー
- `⌘N` 併記、クリックでタブ切替(Mouse::LeftClick)
- attention と coexist

### v2 — ドラッグ&ドロップ並べ替え
- `RunActionsAsUser` 権限追加
- ドラッグ状態機械: LeftClick(掴む)→ Hold(追従)→ Release(確定)
- 列→position ヒットテスト(`line::TabHit` のスパン)
- `run_action(Action::MoveTabByTabId { id, direction })` を差分回数ぶん発行
- ドラッグ中のゴースト表示 / ドロップ位置インジケータ

### v3 候補(要判断)
- ModeInfo capabilities(arrow_fonts)分岐、色テーマ完全連動
- `▌` 横 2 倍 / quadrant で角の精密化
- floating pane の別レイヤー描画
- attention の absorb(pipe 消費 + マーカー描画)
- 設定での 1 行/3 行トグル(狭端末向け)

### やらないこと(v1)
- 比率保存スケール(極小予算で潰れる。独立軸線形で十分)
- glyph family 切替 UI(色グリッド固定)
- 多クライアント cursor 描画

---

## 9. 着手前スパイク(v1 の最初のタスク、必須)

最小プラグインで以下を実機(zellij 0.44.3)裏取りしてから本実装に入る:

1. `PaneUpdate` を 1 回受信し `pane_x/pane_y/pane_columns/pane_rows` を実ダンプ。tab-bar(1 行)直下の通常ペインで `pane_y==1` を確認 → 座標原点の前提を裏取り。
2. `render()` から **セル単位の fg+bg 色(テーマパレット / 可能なら RGB)が実際に出るか**を確認。`▀` + fg/bg で 2 色が表示されるか目視。
3. `default_tab_template` で `size=3` + `set_selectable(false)` が安定して 3 行を受け取れるか実機確認。

このスパイク自体を捨てプロトタイプとし、結果で §4 の色割当(パレット vs RGB)を最終確定する。

---

## 10. リスクと緩和

| リスク | 影響 | 緩和 |
|---|---|---|
| 座標原点が推論ベース(doc 未明記) | 描画位置全面ずれ | bounding-box 正規化はトポロジ依存で頑健 + スパイクで裏取り(§9-1) |
| **セル単位 fg/bg 色が plugin render で出るか未確証** | 色グリッドが成立しない | スパイク(§9-2)で最優先確認。最悪 box-drawing 罫線へフォールバック設計を温存 |
| 縦 6px で深さ≥3 縦スタックが潰れる | 深い縦割りで識別低下 | 局所縮退で色のみ表示、ラベルは諦める。L3 代表グリフへ |
| 幅会計 len と実セル占有(全角/アイコン)の不一致 | クリック/ドラッグ判定ずれ | `UnicodeWidthStr::width` で property test(§11) |
| `size=3` が非 selectable 必須 | 将来 unstable | `load()` 冒頭で必ず `set_selectable(false)` |
| zellij-tile API バージョン差 | フィールド/Event 変化 | 0.44.3 pin(known-good)、本体更新時に同期 |
| 初回許可ダイアログ非表示(#4982) | バックグラウンド起動で許可不可 | キャッシュ事前許可を初回手順に明記 |
| 3 行が画面を食う | 狭端末で作業領域圧迫 | 既存 tab+status の 2 行から +2 行のみ。v3 で 1/3 行トグル検討 |

---

## 11. テスト戦略

### 純関数を単体テスト(主戦場、x86_64 ネイティブ `cargo test --lib`)
- `minimap.rs`: 矩形群 → `[[Cell; W]; 3]` 変換。bounding-box 正規化、独立軸スケール、ピクセル塗り、境界、縮退判定
- `title.rs`: コマンド名抽出 / アイコン写像 / 切詰
- `color.rs`: パレット循環の安定性(同 id → 同色)、focus 強調
- `line.rs`: 幅予算配分、active 中央寄せ、`+N` オーバーフロー
- ヒットテスト: 列 → switch target(`line::switch_target_at_column`; `TabHit.position` は 0-based、`switch_tab_to` は 1-indexed なので `+1` して返す純関数。境界列 / ギャップ / 空レイアウトと 0→1 変換を単体テストで固定)

```rust
#[test]
fn two_column_grid() -> Result<(), Box<dyn std::error::Error>> {
    let panes = vec![rect(0,1,8,10,"nvim"), rect(8,1,8,10,"zsh")];
    let cells = render_minimap(&panes, 3, 16, &palette())?;
    // 左半分が pane0 色、右半分が pane1 色
    assert_eq!(cells[1][3].bg, palette.color_for(panes[0].id));
    assert_eq!(cells[1][12].bg, palette.color_for(panes[1].id));
    Ok(())
}
```
> `Result + ?`、`unwrap()` 不使用(clippy::unwrap_used 整合)。

### スナップショット(レンダリング)
5 基本レイアウト + focus 版 + 縮退各段を、色を文字記号に写像した行列(または ANSI)で `insta` スナップショット固定。

### property test(幅一致)
`len`(幅会計)と実端末セル占有が一致(クリック/ドラッグずれ・オフバイワンの早期検出)。

---

## 12. リポジトリ初期構成(承認後に scaffold)

- `gh repo create GeneralD/zellij-tabmap --public`、topics: `zellij` `zellij-plugin` `rust` `wasm` `webassembly` `tab-bar` `minimap` `terminal` `tui` `developer-tools`
- LICENSE(MIT)、README(英語、hero 画像、バッジ、組込み手順、#4982 注意)
- `src/`(§2 構成)、`.cargo/config.toml`、`Cargo.toml`、ci.yml / release.yml / cliff.toml
- issue-breakdown スキルで依存順 issue 群を作成(スパイク → 色グリッド → ラベル → パッキング → クリック切替 → リリース → v2 D&D)
- 由来となった個人 dotfiles のトラッキング issue から本リポジトリへクロスリンク(片方向)

<p align="center">
  <img src="assets/hero.png" alt="zellij-tabmap" width="600">
</p>

<p align="center">
  <img src="https://img.shields.io/github/v/tag/GeneralD/zellij-tabmap?label=version" alt="Version">
  <img src="https://img.shields.io/badge/zellij--tile-0.44.3-blue" alt="zellij-tile">
  <img src="https://img.shields.io/badge/Rust-2021-orange?logo=rust" alt="Rust">
  <img src="https://img.shields.io/badge/target-wasm32--wasip1-purple?logo=webassembly" alt="Target">
  <img src="https://img.shields.io/github/license/GeneralD/zellij-tabmap" alt="License">
  <img src="https://img.shields.io/github/actions/workflow/status/GeneralD/zellij-tabmap/ci.yml?label=tests" alt="Tests">
  <a href="https://codecov.io/gh/GeneralD/zellij-tabmap"><img src="https://codecov.io/gh/GeneralD/zellij-tabmap/graph/badge.svg" alt="Coverage"></a>
  <a href="https://coderabbit.ai"><img src="https://img.shields.io/coderabbit/prs/github/GeneralD/zellij-tabmap?utm_source=oss&utm_medium=github&utm_campaign=GeneralD%2Fzellij-tabmap&labelColor=171717&color=FF570A&label=CodeRabbit+Reviews" alt="CodeRabbit Reviews"></a>
  <img src="https://img.shields.io/badge/open%20source-%E2%9D%A4-red" alt="Open Source">
</p>

# zellij-tabmap

A [zellij](https://zellij.dev) plugin that replaces the thin one-row tab bar with a **taller, multi-row tab bar** in which every tab is drawn as a **color-coded minimap of its own pane layout** — a tiny pixel-grid thumbnail of how that tab's terminal is split. Panes are identified by color; where a tab is wide enough, a summarized title is overlaid; the `⌘N` switch hint is shown per tab. The active tab stands out — vivid fills, a focus ring on its focused pane, and an optional **perspective lift** that floats it forward.

## Preview

![renderer preview](assets/demo.png)

> The renderer rendered standalone in a terminal — five tabs of varied layouts (a single pane, a 2-column split, a 2×2 grid, a main+stack, and a 2-row split) drawn as color-coded pane minimaps. The active tab (`⌘3`) is **lifted forward by the perspective depth cue** while the inactive tabs recede a half-row at top and bottom; pane fills carry the **gradient sheen**, the focused pane wears its outline ring, and each tab shows its `⌘N` switch hint. Reproduce it with the [`render_demo`](examples/render_demo.rs) example. This renderer is wired into the **live** zellij tab bar, including click-to-switch — see [Status](#status).

## Why a color half-block grid?

Box-drawing rules can only place a line on a *cell boundary*. The upper-half-block glyph `▀` paints its **foreground color on the top half of a cell and its background color on the bottom half**, so the color can change *within* a single cell. That doubles the vertical resolution (a 3-text-row block becomes a 6-pixel-tall grid) and lets even finely split layouts render as distinct color bands instead of collapsing into noise. It's the same half-block technique image-to-terminal tools (chafa, timg) use, applied to a pane map.

```text
 3 text rows        left A (full height)   right: top B / bottom C, split by ▀
   row 1   │ █ A █ │ ▀▀▀   fg=B bg=B   (top & bottom both B)
   row 2   │ █ A █ │ ▀▀▀   fg=B bg=C   (top half B / bottom half C — split mid-cell)
   row 3   │ █ A █ │ ▀▀▀   fg=C bg=C   (top & bottom both C)
```

A focused pane is marked with an outline ring and a bold label — its fill keeps the same identity hue as when unfocused, so a pane never changes color as focus moves. The ring is a luminance-shifted shade of the pane's own fill (a blue pane gets a slightly different blue outline), so the highlight stays in the pane's hue family. Titles degrade gracefully — labels that cannot fit are dropped rather than truncated into noise.

## Status

✨ **Usable today, actively developed.** Installable from a prebuilt wasm (no build step), with gradients, active-tab cues, and perspective depth all shipped.

- ✅ The minimap renderer ([`src/minimap.rs`](src/minimap.rs)) is feature-complete and unit-tested (HSL palette, half-block grid, gradient sheen, focus ring + active-tab emphasis, perspective depth, label degradation). It has **no zellij dependency**, so it runs and is tested on the native host.
- ✅ The full render pipeline is wired: every tab is projected from zellij's live `PaneManifest`, packed into column spans ([`src/line.rs`](src/line.rs)), assembled into a per-tab block at its budgeted width ([`src/tab_block.rs`](src/tab_block.rs)), and composed into the multi-row bar ([`src/paint.rs`](src/paint.rs)). By default the active tab is centered, so the strip slides to follow focus; set `align "left"` to anchor the row instead. Tabs that don't fit collapse into `← +N` / `+N →` end markers.
- ✅ Mouse click-to-switch is wired: a left click anywhere inside a tab's column span focuses that tab. The hit-test ([`src/line.rs`](src/line.rs)) maps the clicked column to the tab drawn there and converts its 0-based position to the 1-based index `switch_tab_to` expects, so it needs the `ChangeApplicationState` permission (see the first-run note below).
- ✅ The [latest release](https://github.com/GeneralD/zellij-tabmap/releases/latest) ships a prebuilt `zellij-tabmap.wasm` asset, so you can install the plugin without building it — see [Use it in zellij](#use-it-in-zellij).

The full design — architecture, rendering pipeline, degradation ladder, golden-repo mapping, risks, and test strategy — lives in [`docs/design.md`](docs/design.md).

## Build from source

```bash
rustup target add wasm32-wasip1     # one time
cargo build --release               # .cargo/config.toml targets wasm32-wasip1
# artifact: target/wasm32-wasip1/release/zellij-tabmap.wasm
```

## Use it in zellij

The robust way to run it is to **install the prebuilt wasm to a local path and load it with `file:`**: a local file loads instantly (no first-launch download wait), updates cleanly, and the permission grant persists across versions. (Prefer to try it before installing? A no-install option that loads straight from the release URL is in the collapsed note at the end of this section — handy for a first look, but it does not auto-update.)

**1. Download the wasm** to a local path. Any absolute path works; `~/.config/zellij/plugins/` matches zellij's own plugin convention:

```bash
mkdir -p ~/.config/zellij/plugins
curl -fL https://github.com/GeneralD/zellij-tabmap/releases/latest/download/zellij-tabmap.wasm -o ~/.config/zellij/plugins/zellij-tabmap.wasm
```

`-f` makes `curl` fail on an HTTP error instead of silently saving the error page as your wasm. To pin a version, swap `latest/download` for `download/vX.Y.Z`.

**2. Grant permissions once.** The bar needs `ReadApplicationState` (pane/tab layout data) and `ChangeApplicationState` (click-to-switch), but a plugin loaded from `default_tab_template` gets no usable permission prompt ([zellij#4982](https://github.com/zellij-org/zellij/issues/4982) tracks this dead-end for background plugins). Load it once in a **regular pane**, where the prompt can be focused and answered (the bar stays selectable until the permission flow resolves):

```bash
zellij plugin -- file:$HOME/.config/zellij/plugins/zellij-tabmap.wasm
```

Press <kbd>y</kbd> to accept, then close the pane. The grant is keyed on the exact location string, and because the file path stays the same across versions this is a **one-time** step — a per-tag URL would need re-granting on every release. (As a fallback, add the entry by hand to `permissions.kdl` in zellij's cache directory — Linux: `~/.cache/zellij/permissions.kdl`, macOS: `~/Library/Caches/org.Zellij-Contributors.Zellij/permissions.kdl` — which is read once at server startup, so manual edits apply only in a fresh session.)

**3. Wire it into your layout.** In `default_tab_template`, give the tab-bar pane a height of 3 rows and point it at the file. KDL does not expand `~`, so use the **absolute** path:

```kdl
default_tab_template {
    pane size=3 borderless=true {                       // 3 rows (the floor); raise to 4+ to enable perspective
        plugin location="file:/Users/you/.config/zellij/plugins/zellij-tabmap.wasm" {
            shortcut_prefix "⌘"
            active_width "24"
            align "center"                              // "center" slides to keep the active tab centered; "left" anchors the row (all-fit only)
            reorder "false"                             // drag a tab to reorder; "true" also needs RunActionsAsUser
            tab_gap "2"                                 // cleared columns between tab blocks; "0" packs them flush
            gradient "sheen"                            // pane fill sweep: "sheen" (L→R, default) / "weave" (alternating rows) / "off" (flat)
            inactive_dim "true"                         // dim inactive tabs so the active one stands out; "false" to opt out
            perspective "true"                          // lift the active tab with depth (needs pane size 4+); "false" to opt out
        }
    }
    children
    pane size=1 borderless=true { plugin location="status-bar" }
}
```

Restart the session. Because the wasm is already local, the bar paints on the **first** tab immediately — there is no first-launch download wait (a remote-URL plugin is blank until its initial download lands, which can read as a broken bar).

**4. Update** later by re-running step 1 — the download overwrites the file in place. zellij reads a local file from disk at each session start, so a **fresh session** picks up the new build automatically; a remote URL, by contrast, is cached by zellij **by URL and never re-fetched**, so updates silently never arrive. An already-running session keeps serving the old build until you start a new one — there is no in-place reload for the tab bar (`zellij action start-or-reload-plugin` opens a *stray plugin pane* instead of refreshing a layout-loaded plugin, [zellij#3927](https://github.com/zellij-org/zellij/issues/3927)), so simply start a fresh session to pick up the update.

Contributors hacking on the plugin [build from source](#build-from-source) and point `file:` at their own `target/wasm32-wasip1/release/zellij-tabmap.wasm` artifact instead of the downloaded wasm.

> **`align` — center vs left.** When every tab fits, `align` decides how the row is anchored: `center` (default) re-centers the active block on each focus change, so the whole strip slides horizontally; `left` pins the row's **left edge** at the start of the tab area (column 0, or just after any reserved prefix columns), removing that whole-strip slide. Note `left` does not freeze every tab's column — the active tab is still drawn wider than the inactives, so the tabs drawn after it shift right as focus crosses them; only the leftmost tab is truly fixed. `align` governs the all-fit case **only** — when tabs overflow, the visible window always follows the active tab (with `← +N` / `+N →` markers) regardless of `align`, because the active tab must stay on screen. The default stays `center` so existing layouts render unchanged on update.
>
> **`tab_gap` — space between tabs.** Leaves the given number of cleared columns between adjacent tab blocks so the boundary between screens reads clearly (default `2`). Set `0` to pack the blocks flush.
>
> **`gradient` — per-pane fill sweep.** `sheen` (default) sweeps each pane block's fill left-to-right from its base color toward a luminance-shifted shade (lighter for dark themes, darker for light ones); `weave` alternates the sweep direction on each half-block pixel row for a woven texture. The focus ring, labels, and the `⌘N` badge stay solid on top, so readability is unchanged. Set `off` for flat fills.
>
> **`inactive_dim` — visual cue for the active tab.** When `true` (default), inactive tabs are dimmed toward the terminal background so the active tab stands out clearly: its pane fills stay vivid, its shortcut badge and focused pane label are drawn in white, and no focus ring appears on other tabs. Set `false` to disable the dimming and treat all tabs with equal intensity.
>
> **`perspective` — lift the active tab with depth.** When `true` (default) **and** the bar is at least **4 rows tall**, every inactive tab recedes by one row — a half-row of terminal background inset at its top and bottom — while the active tab fills the full height, so the selected tab appears to float forward. The height comes from the layout's `pane size=N`, which the plugin can only read, not set: bump the tab-bar pane to `size=4` (or more) to see the effect. Below 4 rows the option is a no-op (every tab fills the bar), and `false` always renders every tab at full height. Pairs naturally with `inactive_dim` — color recede plus depth recede. The bar renders nothing if it is given fewer than 3 rows (the minimap needs that floor to stay legible).
>
> **Enabling `reorder`** requests a third permission, `RunActionsAsUser` (for the `MoveTabByTabId` action a tab drag performs). Granting is all-or-nothing for tab-template plugins, so when you set `reorder "true"` you must **re-run step 2** (the grant prompt then lists all three permissions) and restart — otherwise the bar freezes with no prompt. Left at the default (`false`), the plugin requests only the two permissions above, so an existing install keeps working unchanged across updates.

<details>
<summary>Load straight from the release URL (quick try — does not auto-update)</summary>

For a first look without downloading anything, point the layout's `plugin location` directly at the release URL — zellij fetches and caches the wasm on first use:

```kdl
plugin location="https://github.com/GeneralD/zellij-tabmap/releases/latest/download/zellij-tabmap.wasm"
```

Grant this URL once before relying on the layout — the step-2 grant does **not** carry over, because zellij keys permissions on the exact location string and a `default_tab_template` plugin gets no usable prompt ([zellij#4982](https://github.com/zellij-org/zellij/issues/4982)). Load the URL in a regular pane (this also pre-warms the download):

```bash
zellij plugin -- https://github.com/GeneralD/zellij-tabmap/releases/latest/download/zellij-tabmap.wasm
```

Press <kbd>y</kbd> and close the pane. Two caveats still make this unsuitable as a permanent install:

- **Updates never arrive.** zellij caches the downloaded wasm **by URL and never re-fetches it**, so the `latest` URL keeps serving whatever version you first loaded; clearing zellij's cache is the only way to move forward. (A version-pinned `releases/download/vX.Y.Z/` URL avoids the stale-cache problem but then needs a fresh permission grant on every release.)
- **Blank bar on first launch.** If you skip the warm-up above, the wasm downloads on the first session for an uncached URL while the bar sits empty — and since the bar *is* the tab bar, that can read as broken until the download lands.

If a fetch ever returns a non-wasm body (e.g. a 404 page when the release asset is not published yet), zellij caches that error text **as the wasm**, permanently — the log then shows `magic header not detected`. Recover by deleting both cache traces for that URL (a hashed blob directly under zellij's cache root, and the `https:/github.com/GeneralD/zellij-tabmap/releases/…` directory tree beneath it) and starting a fresh session.

</details>

## Development

```bash
cargo test  --lib --target "$(rustc -vV | sed -n 's/host: //p')"   # native unit tests
cargo clippy --target wasm32-wasip1 --all-features --lib            # lint (CI denies warnings)
cargo build --release --target wasm32-wasip1                        # the loadable wasm
```

CI runs the same three on every push; tagging `vX.Y.Z` builds the wasm, generates a changelog with [git-cliff](https://github.com/orhun/git-cliff), and attaches the artifact to a GitHub Release.

## Acknowledgements

Structured after [`KiryuuLight/zellij-attention`](https://github.com/KiryuuLight/zellij-attention), used as a golden-repository reference for the Rust/WASM zellij-plugin layout (thin `register_plugin!` bin + native-testable lib + FFI-stubbed tests + CI/release workflows).

## License

[MIT](LICENSE)

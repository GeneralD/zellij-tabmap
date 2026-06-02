# zellij-tabmap

![hero](assets/hero.png)

[![CI](https://github.com/GeneralD/zellij-tabmap/actions/workflows/ci.yml/badge.svg)](https://github.com/GeneralD/zellij-tabmap/actions/workflows/ci.yml) ![license](https://img.shields.io/badge/license-MIT-green) ![zellij-tile](https://img.shields.io/badge/zellij--tile-0.44.3-blue) ![rust](https://img.shields.io/badge/rust-2021-orange?logo=rust) ![target](https://img.shields.io/badge/target-wasm32--wasip1-purple?logo=webassembly) ![status](https://img.shields.io/badge/status-early%20development-yellow)

A [zellij](https://zellij.dev) plugin that replaces the thin one-row tab bar with a **taller, multi-row tab bar** in which every tab is drawn as a **color-coded minimap of its own pane layout** — a tiny pixel-grid thumbnail of how that tab's terminal is split. Panes are identified by color; where a tab is wide enough, a summarized title is overlaid; the `⌘N` switch hint is shown per tab.

## Preview

![renderer preview](assets/demo.png)

> The renderer rendered standalone in a terminal: five sample layouts (a single pane, a 2-column split, a 2-row split, a 2×2 grid, and a main+stack) shown as color-coded minimaps — pixel-only on top, with overlaid labels below at two widths. Wiring this renderer into the **live** zellij tab bar is in progress (see [Status](#status)).

## Why a color half-block grid?

Box-drawing rules can only place a line on a *cell boundary*. The upper-half-block glyph `▀` paints its **foreground color on the top half of a cell and its background color on the bottom half**, so the color can change *within* a single cell. That doubles the vertical resolution (a 3-text-row block becomes a 6-pixel-tall grid) and lets even finely split layouts render as distinct color bands instead of collapsing into noise. It's the same half-block technique image-to-terminal tools (chafa, timg) use, applied to a pane map.

```
 3 text rows        left A (full height)   right: top B / bottom C, split by ▀
   row 1   │ █ A █ │ ▀▀▀   fg=B bg=B   (top & bottom both B)
   row 2   │ █ A █ │ ▀▀▀   fg=B bg=C   (top half B / bottom half C — split mid-cell)
   row 3   │ █ A █ │ ▀▀▀   fg=C bg=C   (top & bottom both C)
```

A focused pane is emphasized with a bright outline ring and a more vivid hue; non-focused panes keep their hue but mute their saturation. Titles degrade gracefully — labels that cannot fit are dropped rather than truncated into noise.

## Status

🚧 **Early development.**

- ✅ The minimap renderer ([`src/minimap.rs`](src/minimap.rs)) is complete and unit-tested (HSL palette, half-block grid, focus ring, label degradation). It has **no zellij dependency**, so it runs and is tested on the native host.
- ✅ The full render pipeline is wired: every tab is projected from zellij's live `PaneManifest`, packed into column spans ([`src/line.rs`](src/line.rs)), assembled into a per-tab block at its budgeted width ([`src/tab_block.rs`](src/tab_block.rs)), and composed into the multi-row bar ([`src/paint.rs`](src/paint.rs)). The active tab is centered; tabs that don't fit collapse into `← +N` / `+N →` end markers.
- 🔜 Mouse click-to-switch and a published `.wasm` release are the next milestones — tracked in the [issues](https://github.com/GeneralD/zellij-tabmap/issues).

The full design — architecture, rendering pipeline, degradation ladder, golden-repo mapping, risks, and test strategy — lives in [`docs/design.md`](docs/design.md).

## Build from source

```bash
rustup target add wasm32-wasip1     # one time
cargo build --release               # .cargo/config.toml targets wasm32-wasip1
# artifact: target/wasm32-wasip1/release/zellij-tabmap.wasm
```

## Use it in zellij

In your layout's `default_tab_template`, give the tab-bar pane a height of 3 rows and point it at the plugin. Build from source and reference the local `.wasm` while the plugin is in early development:

```kdl
default_tab_template {
    pane size=3 borderless=true {                       // 1 → 3 rows
        plugin location="file:/absolute/path/to/zellij-tabmap.wasm" {
            shortcut_prefix "⌘"
            active_width "24"
        }
    }
    children
    pane size=1 borderless=true { plugin location="status-bar" }
}
```

Once a tagged release is published, you can instead reference the hosted artifact directly (zellij fetches and caches it):

```kdl
plugin location="https://github.com/GeneralD/zellij-tabmap/releases/latest/download/zellij-tabmap.wasm"
```

> **First-run permission note** ([zellij#4982](https://github.com/zellij-org/zellij/issues/4982)): plugins started from `default_tab_template` cannot show the interactive permission dialog. If the plugin appears inert on first launch, grant it `ReadApplicationState` / `ChangeApplicationState` in zellij's plugin permission cache (`permissions` under the plugin cache) and reload.

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

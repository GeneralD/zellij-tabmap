# zellij-tabmap — Instructions for Claude Code

A zellij plugin (Rust → `wasm32-wasip1`) that replaces the one-row tab bar
with a multi-row bar, rendering each tab as a color-coded half-block minimap
of its pane layout.

## Conventions

- Repository content — code, comments, README, commit messages, issues, PRs
  — is **English** (international OSS audience). `docs/design.md` is the lone
  exception (internal planning doc, Japanese).
- Build the plugin: `cargo build --target wasm32-wasip1`.
- Run native unit tests on the host triple (the lib links zellij-tile's host
  imports): `CARGO_BUILD_TARGET=<host-triple> cargo test --lib`
  (e.g. `aarch64-apple-darwin`).

## Project Rules

Read the relevant file in `.claude/rules/` before working in the area it
covers:

- [`.claude/rules/zellij-plugin-development.md`](rules/zellij-plugin-development.md)
  — zellij plugin development pitfalls & tips: the permissions cache, the
  `set_selectable(false)` focus trap, the headless verification harness
  (eprintln oracle + `expect` PTY), `TMPDIR`/log locations, plugin caching,
  and build/test targets. Accumulated from real debugging — **append new
  traps as they're hit**, don't prune.

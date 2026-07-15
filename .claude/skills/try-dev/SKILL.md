---
name: try-dev
description: >-
  Build the zellij-tabmap bar plugin from a branch (or the current worktree) and
  launch a throwaway nested zellij session that loads it via file:, without
  touching the real config.kdl / default.kdl or the release install. Use to try
  a dev build of the bar before releasing it — triggers: "try the dev build",
  "preview this branch's bar", "devビルドを試したい", "floatingを体験したい",
  "このブランチのバーを見たい".
allowed-tools:
  - Bash(.claude/skills/try-dev/try-dev.sh*)
  - Bash(zellij*)
---

# try-dev — preview a dev build of the bar

A release goes out through `install.sh` (a pinned release URL, no local wasm).
This skill is the opposite end: try an **unreleased** build of the bar from a
branch, in a **throwaway** zellij session, without disturbing your real setup.
Your `config.kdl`, `default.kdl`, and any release install are never touched.

It wraps `.claude/skills/try-dev/try-dev.sh`.

## Usage

```bash
.claude/skills/try-dev/try-dev.sh [BRANCH|WORKTREE] [--release] [--no-build] [--logs] [--no-launch]
```

- **no arg** → build the **current** worktree's HEAD (the common dev case)
- **`BRANCH`** → build that branch (reuses its worktree, or creates a temp one
  under `.worktrees/try-dev-<branch>`)
- **`WORKTREE`** (a path) → build in that worktree
- **`--release`** → optimized build (default is a fast **debug** build)
- **`--no-build`** → skip cargo; use the last-built wasm as-is
- **`--logs`** → add a `tail -F zellij.log` pane to the dev tab (pairs with the
  eprintln oracle in `.claude/rules/zellij-plugin-development.md` §4)
- **`--no-launch`** → prepare only; print the launch command

## What it does

1. **Build** (debug by default) → `target/wasm32-wasip1/<profile>/zellij-tabmap.wasm`.
2. **Refresh the permission grant** for that exact wasm in `permissions.kdl`,
   stripping stale dev-build grants so they never accumulate. The bar is loaded
   from `default_tab_template`, which gets no usable prompt (zellij#4982), so the
   grant is pre-seeded rather than answered interactively.
3. **Generate a throwaway layout** in `$TMPDIR` pointing `file:` at the build
   (bar at `size=4` with `floating "hybrid"`, `perspective`, `close_button`,
   `scroll "pane"`; a single empty tab; `--logs` adds a log pane).
4. **Kill any stale `tabmap-dev` session** so a fresh server reads the new build.
5. **Launch**:
   - **inside zellij** (`$ZELLIJ` set) → open the dev session **nested in a new
     tab** of your current session (falls back to a new pane, then to printing
     the command).
   - **outside** → print `env ZELLIJ=0 zellij -s tabmap-dev -n <layout>` to run.

## Agent procedure

When the user invokes this skill:

1. Parse any branch name / flags from their request into the script arguments
   (e.g. "floatingを試したい" with the branch known → `try-dev.sh feat/110-floating-panes`).
2. Run the script from the repo. Relay its summary.
3. If it printed a launch command (outside zellij, or auto-launch fell through),
   present that command for the user to run — offer the `!` prefix so it runs in
   this session. If it auto-launched a nested tab, just confirm.
4. **Iterate**: after a code change, re-run the skill (it rebuilds and kills the
   stale session), then start a fresh dev session to pick up the new build —
   `file:` is re-read from disk at server start, so there is no blank-bar race.

## Teardown

The dev session is disposable: exit it (`Ctrl+q`) and close the tab. Nothing
persists except the single refreshed dev grant in `permissions.kdl` and the
build in `target/` — both overwritten on the next run, so nothing accumulates.
A temp worktree created for a named branch stays until you remove it
(`git worktree remove .worktrees/try-dev-<branch>`).

## Why a separate session (not a new tab in your current one)

The tab bar is a **session-wide** plugin (one instance per server, from
`default_tab_template`), so a new tab in your current session still shows your
*installed* bar, not the dev build. A nested session gets its own server and
its own `default_tab_template`, so the dev bar renders inside that tab. Setting
`ZELLIJ=0` bypasses zellij's nested-session guard.

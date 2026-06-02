# Zellij Plugin Development — Pitfalls & Tips

Hard-won lessons from building this plugin against `zellij-tile 0.44.x`.
This file grows as we hit new traps — append, don't prune. Each entry is a
trap we actually fell into, with the diagnosis and the way out.

---

## 1. Permissions are read **once at server start**, never on reload

zellij caches granted permissions per plugin in a KDL file:

- macOS: `~/Library/Caches/org.Zellij-Contributors.Zellij/permissions.kdl`
- Linux: `$XDG_CACHE_HOME/zellij/permissions.kdl` (`~/.cache/zellij/...`)

The running **server loads this file into memory at startup** and does *not*
re-read it when a plugin reloads. So:

- Editing `permissions.kdl` and then reloading the plugin in the **same**
  session does **nothing** — the in-memory copy wins.
- A **fresh session = a fresh server**, which *does* read the edited file.
  This is the only reliable way to pre-grant a permission without the
  interactive prompt.

Format — one block per plugin, keyed by the plugin's location string:

```kdl
"file:/abs/path/to/plugin.wasm" {
    ReadApplicationState
}
```

**Key-match ambiguity:** the same plugin can appear keyed both as a bare
path *and* with a `file:` prefix (zellij writes whichever form the load
site used). When pre-seeding, add **both** forms so the match can't miss:

```kdl
"/abs/path/to/plugin.wasm"      { ReadApplicationState }
"file:/abs/path/to/plugin.wasm" { ReadApplicationState }
```

Permission names seen in the wild: `ReadApplicationState`,
`ChangeApplicationState`, `OpenFiles`, `OpenTerminalsOrPlugins`,
`ReadCliPipes`, `MessageAndLaunchOtherPlugins`.

**Confirmed:** a *cache*-granted permission still emits
`Event::PermissionRequestResult(PermissionStatus::Granted)` — your
`permitted` flag flips exactly as it would after an interactive `y`.

---

## 2. `set_selectable(false)` makes the pane **unfocusable** — including its own permission prompt

A tab-bar/status-bar style plugin pins itself with `set_selectable(false)`
so keyboard navigation skips it (this matches zellij's official tab-bar).

The trap: this also means you **cannot focus the pane to press `y`** on the
permission prompt — not by keyboard, not by `zellij action move-focus`. The
prompt is on-screen but unreachable. There is no `zellij action` to grant a
permission either, and `write-chars y` goes to pane stdin, which the
permission overlay ignores.

**Way out:** don't try to grant interactively for a non-selectable plugin.
Pre-seed `permissions.kdl` (see #1) and start a fresh server. In normal use
the plugin loads from a layout (config-trusted), so this only bites
ad-hoc CLI loads and live verification.

---

## 3. Detached sessions **don't pump plugin events**

`zellij attach --create-background <name>` (`-b`) creates a *detached*
session — great for scripting because it needs no TTY. But:

- `load()` runs and the plugin's host calls (e.g. `subscribe`,
  `request_permission`) fire.
- Subscribed **Screen events (`TabUpdate`, `PaneUpdate`) are NOT delivered
  to `update()`** until a real client is attached. The event/render loop is
  driven by an attached client; with none, your `update()` never runs even
  as you mutate panes via `zellij action`.

Symptom: the plugin loads (you see `Loaded plugin '...' in N ms` in the
log), splits succeed, but your `update()` logging stays silent.

**Way out:** attach a client (see #4). The moment a client attaches, the
backlog pumps and `update()` starts firing.

---

## 4. Headless verification harness (eprintln oracle + `expect` PTY)

To drive a plugin and observe its internal state **without occupying your
live terminal**:

1. **Oracle:** temporarily add `eprintln!("DBG ...")` lines in `update()`,
   **before** any `permitted`/state gate, so they log unconditionally.
   Plugin stderr lands in `zellij.log` at `DEBUG`, tagged `[id: N]`. Revert
   (`git checkout`) when done — never commit the oracle.
   - Put the oracle in `update()`, not `render()`: a cache-granted
     permission that (hypothetically) emitted no `PermissionRequestResult`
     would leave `permitted=false` and a `render()`-gated oracle silent
     forever, even with data flowing.

2. **Pre-seed** `permissions.kdl` (see #1).

3. **Attach a client that stays open.** Use `expect` (real PTY); a plain
   background launch won't work:

   ```tcl
   # create-hold.exp
   set timeout -1
   set env(TMPDIR) "<real-os-temp-dir>"      ;# see #5
   set stty_init "rows 40 cols 120"
   spawn zellij -s tabmap-verify -n <layout.kdl>   ;# new session + layout + attached client
   sleep 180                                  ;# hold the PTY open; drive splits from outside
   ```

   Run it in the background. `-s NAME -n LAYOUT` creates a **new** session
   with the layout *and* attaches the expect PTY as its client, so all
   initial events pump.

4. **Drive state from outside** (a separate shell) while the client holds:

   ```bash
   zellij --session tabmap-verify action new-pane -d right   # 2-col
   zellij --session tabmap-verify action new-pane -d down    # 2-row
   zellij --session tabmap-verify action close-pane          # shrink
   ```

   Then read the new `DBG` lines from `zellij.log`.

**Do NOT use `script -q /dev/null zellij ...`** as the PTY: `script` sends
an immediate EOF (`^D`) to zellij, which misreads the invocation as
*attach-to-existing*, prints `Session '<name>' not found`, and exits without
creating anything. `expect` keeps the PTY open and avoids this.

`zellij action` itself talks to the server socket and works **without**
attaching — but remember #3: the events it triggers only reach the plugin
once a client is attached.

---

## 5. `TMPDIR` / socket / log location

zellij keeps its IPC socket and `zellij.log` under the **real OS temp dir**.
Some host environments override `TMPDIR` to a private dir, which makes every
`zellij`/`zellij action` call fail with *"No active sessions found"*.

- macOS: `TMPDIR="$(getconf DARWIN_USER_TEMP_DIR)" zellij list-sessions`
  - log: `$(getconf DARWIN_USER_TEMP_DIR)/zellij-<uid>/zellij-log/zellij.log`
- Linux: socket under `$XDG_RUNTIME_DIR` or `/tmp`; if your env overrides
  `TMPDIR`, run `env -u TMPDIR zellij list-sessions`.

Plugin `eprintln!`/`dbg!` output is written into that same `zellij.log` at
`DEBUG` — tail it to observe the plugin headlessly.

---

## 6. zellij caches the wasm **by path** within a session

Within one session, reloading the same wasm path serves the **cached**
build, ignoring your rebuild. To pick up a fresh `cargo build`:

- `zellij action new-pane --plugin file:<wasm> --skip-plugin-cache`, or
- `zellij action start-or-reload-plugin file:<wasm>`.

---

## 7. `dump-screen` **cannot read plugin panes**

`zellij action dump-screen` returns terminal-pane contents only — a plugin
pane (including this tab bar) dumps as blank. So you cannot snapshot the
plugin's rendered truecolor output through `dump-screen`.

**Way out:** split verification in two —
- **Data / projection** (what feeds the renderer): verify live via the
  eprintln oracle (#4).
- **Visual / paint** (the half-block truecolor cells): verify with unit
  tests on the dependency-free render module (it takes plain rectangles and
  emits a string, no zellij types — testable off-wasm).

---

## 8. Build targets & native tests

- Default build target is `wasm32-wasip1` (set in `.cargo/config.toml`).
  Build the plugin with `cargo build --target wasm32-wasip1`.
- Native unit tests need the **host** triple, because linking the lib pulls
  in `zellij-tile`'s host imports:
  `CARGO_BUILD_TARGET=aarch64-apple-darwin cargo test --lib`.
- Those host imports all route through `host_run_plugin_command`; provide a
  `#[cfg(test)] #[no_mangle] extern "C" fn host_run_plugin_command() {}`
  stub so the native test binary links. On wasm the real host supplies it,
  so the stub is compiled only under `cfg(test)`.
- Keep the renderer free of any zellij type (translate `PaneInfo`/`TabInfo`
  into a local rectangle type in one adapter module). That is what keeps
  `cargo test --lib` runnable off-wasm.

---

## 9. `PaneInfo` geometry & what to filter

- `pane_x` / `pane_y` / `pane_columns` / `pane_rows` are the pane's
  tab-relative cell geometry. A top-pinned bar plugin occupies the first
  rows, so **content panes start below it** — e.g. with a `size=3` bar a
  full-tab pane reports `y=3`, and a vertical split becomes
  `(x=0,y=3,h=19)` over `(x=0,y=22,h=18)`.
- A horizontal (side-by-side) split keeps `y` equal and halves the width:
  `(x=0,…,w=60)` and `(x=60,…,w=60)` at cols=120.
- Filter out `is_plugin || is_floating || is_suppressed` to get just the
  user's tiled content. The active tab's raw pane count therefore exceeds
  the projected count (the bar/status-bar plugins are part of the raw set).
- Use `is_focused` to mark the active pane; the newest split is focused.

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

**Fixed in this plugin (#54):** `load()` no longer pins early — the pane
stays selectable until `PermissionRequestResult` arrives (the handler pins
on both Granted and Denied), so an ad-hoc CLI load can be focused and
answered with `y`. The trap above still applies to any plugin that calls
`set_selectable(false)` before its permission flow resolves.

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

---

## 10. zellij caches HTTP **error bodies** as the wasm for remote-URL plugins

When a layout points at an `https://` plugin location, zellij downloads the
URL **without validating the HTTP status or the wasm magic number** and
caches whatever bytes came back. If the asset 404s (e.g. the release isn't
published yet), the literal 9-byte body `Not Found` is cached **as the
plugin wasm** — permanently, keyed by URL.

Symptom in `zellij.log`:

```text
failed to parse plugin ...: magic header not detected:
bad magic number — actual=[0x4e, 0x6f, 0x74, 0x20]
```

`[0x4e, 0x6f, 0x74, 0x20]` is ASCII `"Not "` — the start of `Not Found`.
**Restarting zellij does NOT recover**: the poisoned download is on disk and
is served on every subsequent load of that URL.

**Way out:** delete the cached artifacts for that URL, then start a fresh
session. On macOS the cache root is
`~/Library/Caches/org.Zellij-Contributors.Zellij/` (Linux:
`$XDG_CACHE_HOME/zellij/`), and the URL leaves two traces:

1. A **hashed blob** directly under the cache root — a file whose name is a
   long decimal hash of the URL. Find the poisoned one by size/content
   (`file`/`head` shows it's the HTML/text error body, e.g. 9 bytes,
   instead of a multi-MB wasm).
2. A **URL-derived directory tree**, e.g.
   `<cache-root>/https:/github.com/<owner>/<repo>/releases/download/vX.Y.Z/`.

Delete both, restart, and zellij re-downloads. (`permissions.kdl` in the
same directory is unrelated — leave it alone.)

---

## 11. Remote-URL permission grants are **per exact URL string**

`permissions.kdl` (#1) keys on the literal plugin location string. For
remote plugins that means:

- A **version-pinned URL** (`.../releases/download/v0.3.0/...`) needs a
  fresh grant for **every release** — the v0.2.x grant does not carry over.
- The **`latest` URL** (`.../releases/latest/download/...`) needs one grant
  ever, but combines badly with #10's URL-keyed wasm cache: the cached wasm
  for that URL is never invalidated, so updates are held back until the
  cache is cleared.

Standard onboarding for users (and for yourself after a release bump):
load the plugin **once in a regular pane** —

```bash
zellij plugin -- https://github.com/<owner>/<repo>/releases/download/vX.Y.Z/plugin.wasm
```

— press `y`, close the pane, restart the session. The interactive prompt
works fine there; zellij#4982 only prevents the dialog for plugins loaded
from `default_tab_template`. Hand-editing `permissions.kdl` (#1) is the
fallback, not the primary flow.

---

## 12. Release-timing trap: never point a layout at a tag URL before the asset exists

The release workflow takes ~6 minutes after the tag push to build and
attach the wasm. If a layout (or `zellij plugin --`) hits the tag URL in
that window, GitHub returns 404 and zellij **poisons its cache with the
error body** (#10) — the plugin then stays broken even after the asset is
published, until the cache is manually cleared.

Order of operations for a release bump in your own layout:

1. Push the tag; wait for the release workflow to finish
   (`gh run watch`, or `gh release view vX.Y.Z` shows the `.wasm` asset).
2. Only then update the layout's plugin URL.
3. Grant the permission for the new URL (#11).
4. Start a fresh session (#1 — permissions are read at server start).

---

## 13. First-launch download race: a remote-URL bar is blank until its first download lands

A layout-loaded plugin at a **not-yet-cached** remote URL is downloaded on the
first session that uses it. zellij broadcasts its one-shot initial
`TabUpdate`/`PaneUpdate` at server start; if the download is still in flight
then, the plugin misses that broadcast and loads with an empty `self.tabs`, so
`render()` bails and the bar stays blank until the **next** event delivers a
fresh `TabUpdate` — e.g. opening a second tab. The symptom reads as "the bar
only activates on the 2nd tab, and only right after a version upgrade."

This is a **one-time, first-download-only** artifact, and it is **not fixable
in plugin code**: while the download is in flight the wasm isn't running yet,
so zellij paints its own loading placeholder, not ours. Evidence from a real
run (`zellij.log`): server start at 16:16:18, the freshly-pinned wasm finished
loading at 16:16:25 (~7 s — the download); the very next session, cache warm,
loaded the same wasm in **2.95 ms** at tab-0 spawn, with the bar present from
the first tab.

Why it surfaces right after a version bump specifically: a new tag URL (or a
cache-cleared `latest`) is uncached, so the first session pays the download;
every later session hits the warm URL cache (#6/#10) and loads instantly.

Remedies (both sidestep the race; neither is plugin code):

- **Pre-warm the URL.** The one-time permission step (`zellij plugin -- <url>`,
  press `y`) downloads and caches the wasm as a side effect, so the later
  template-loaded bar starts from a warm cache. This is why the README's
  permission step incidentally hides the race for README-followers.
- **Distribute by `file:` (preferred).** A local path loads instantly — there
  is no download window at all — and also sidesteps the URL-cache traps
  #10/#11: the file is re-read from disk so updates apply, and the permission
  grant persists because the path is stable across versions. Still subject to
  #6 (within-session wasm cache): overwrite the file, then start a **fresh
  session** to pick it up — for the template-loaded bar there is no in-place
  reload (`start-or-reload-plugin` spawns a stray pane, #14).

---

## 14. `start-or-reload-plugin` reloads a *pane* plugin in place, but spawns a stray pane for a *layout-loaded* one

`zellij action start-or-reload-plugin file:<wasm>` reloads a plugin **only if
that plugin is considered already loaded**. A plugin started from a layout —
`default_tab_template`, a `pane { plugin }` in the layout, or `load_plugins` —
is *not* tracked as a reloadable instance, so the action falls through to its
"start" branch and **opens a new plugin pane** instead of refreshing the
running one ([zellij#3927](https://github.com/zellij-org/zellij/issues/3927),
open as of 0.4x). For the tab bar (always layout-loaded) this means a stray
content pane, not a refreshed bar.

So the `start-or-reload-plugin` advice in #6 applies **only to plugins you
loaded into a pane yourself** (ad-hoc `zellij plugin --` / `new-pane
--plugin` during dev). To pick up a new build of the *bar*, there is no
in-place reload — **start a fresh session** (with `file:` the local wasm is
re-read from disk at server start, so the fresh session has the new build;
see #13). Don't put `start-or-reload-plugin` in user-facing update docs for a
template-loaded plugin — it sends users into a duplicated-pane recovery path.

---

## 15. Adding a new permission **silently freezes the bar** for existing users (zellij#4982)

Once a user grants permissions for a plugin, zellij stores the approved
**set of permissions** in `permissions.kdl`, keyed by the plugin's **location
string** (see #1, #11). If a new release adds a *new* permission to the
`request_permission()` call, the cached set does **not** cover it — and
because the bar is layout-loaded from `default_tab_template`, zellij cannot
present the interactive prompt again (zellij#4982, see #2). zellij withholds
event delivery until the *full* requested set is granted, so `permitted`
never flips and `render()` keeps bailing: the **whole bar freezes** (it draws
nothing, not just the new feature) for **all existing users** until they
manually re-grant.

**Way out (design rule):** any feature that requires a permission beyond the
current default set (`ReadApplicationState` + `ChangeApplicationState`) must
be gated behind an **opt-in config flag**. When the flag is on, the plugin
requests the extra permission and the user gets the prompt on first use (they
opted in — the feature is expected). When off, no new permission is requested
and the existing grant keeps working. This mirrors how `reorder` gates
`RunActionsAsUser` (#23).

**Verify before shipping any new action** that it does NOT require a third
permission. `close_tab_with_index` and `new_tab` both fall under the existing
`ChangeApplicationState` — no extra permission needed. But any hypothetical
future action (e.g. opening a URL, reading the clipboard) must be audited
against this rule.

---

## 16. `close_tab_with_index(usize)` closes a tab **by index** without focusing first

zellij-tile 0.44.3 exposes `close_tab_with_index(position: usize)` as a
host-call shim. Unlike `close_focused_tab()`, it does **not** require the
tab to be focused first — no go-to-then-close dance needed.

Confirmed to fall under the existing `ChangeApplicationState` grant (see #15):
no new permission is required.

The `position` argument is the **0-based tab index** matching
`TabInfo::position` — already carried on the `TabHit` geometry struct as
`TabHit.position` (`src/line.rs`), so the `LeftClick` close-button handler
passes it straight through.

---

## 17. `new_tab` reads a value back from stdin — it **panics** in native tests, so it is wasm-only coverage

Most zellij-tile host calls are fire-and-forget: `focus_terminal_pane`,
`switch_tab_to`, `close_tab_with_index`, `set_selectable`,
`request_permission`, `subscribe` all push a command to the host and return
`()`. The native test stub (see #8 — `#[cfg(test)] #[no_mangle] extern "C" fn
host_run_plugin_command() {}`) absorbs them, so each can be driven through
`update()` and asserted on in `cargo test --lib`.

`new_tab::<&str>(None, None)` is different: its shim **reads a return value
back from the plugin's stdin** (`zellij_tile`'s `shim.rs` does a
`Result::unwrap()` on a deserialize-from-stdin). On wasm the host supplies
those bytes; in a native test there is nothing on stdin, so the deserialize
fails and the `unwrap()` **panics** ("failed to deserialize bytes from stdin
/ EOF"). The empty stub cannot help — the panic is on the *return* path,
inside the shim, not in `host_run_plugin_command`.

Consequence: the `Event::Mouse(LeftClick)` arm that dispatches
`ClickIntent::NewTab => { new_tab::<&str>(None, None); }` (`src/lib.rs`)
**cannot** be exercised off-wasm — that one host-effect line is inherently
wasm-only coverage, and llvm-cov will always report it as missed. Don't try
to "fix" the gap with a native test; it can only panic.

**Way out (keep coverage honest):** push the *decision* out of the host-effect
arm and into a pure, fully-covered function. The routing that resolves a
click to `ClickIntent::NewTab` lives in `src/router.rs::route_click` and **is**
unit-tested; only the final `new_tab(...)` call stays uncovered. So the
projection/decision is verified natively and just the irreducible host call is
left to wasm — the same split as #7 (data via the eprintln oracle, paint via
renderer unit tests).

---

## 18. Hidden floating panes stay in the `PaneManifest` with ids (verified 0.44.3)

Toggling a tab's floating layer off (`toggle-floating-panes`) does **not** drop
its floating panes from the `PaneManifest`. Verified empirically with the
eprintln oracle (#4) against zellij 0.44.3 while building #110:

- Each float keeps its entry — `is_floating = true`, its **id**, and its full
  `pane_x/y/columns/rows` geometry — even after the layer is hidden, and it
  survives unrelated `PaneUpdate`s (e.g. a tiled split) while hidden.
- `TabInfo::are_floating_panes_visible` flips to `false` when the layer hides,
  but `TabInfo::selectable_floating_panes_count` still counts the hidden floats.
- Float geometry is in the **same tab-relative cell space as tiled panes** (a
  float reports e.g. `x=30 y=12 w=60 h=18` next to a tiled `x=0 y=3 w=120 h=37`
  under a `size=3` bar — `y` starts below the bar, exactly like tiled content).

Consequences for a bar plugin: you can depict hidden floats individually (one
chip per id) and reveal-and-focus a specific one with
`focus_terminal_pane(id, /*should_float_if_hidden=*/true, false)` — the id is
still valid while the layer is hidden. Terminal-pane ids and plugin-pane ids are
**separate spaces**, so a float can share a numeric id with the bar's own plugin
pane (`id=1` terminal float vs `id=1` plugin bar); filter with
`is_floating && !is_plugin` to get just the terminal floats.

**Not verified:** whether the layer auto-hides when focus moves off an unpinned
float (the expect PTY detached before that probe). Design float wheel-walk to
not depend on it — walk only tiled + already-visible floats; reach hidden floats
via their chip, never by a wheel-triggered reveal.

---

## 19. Suppressed panes stay in the `PaneManifest`, sharing their cover's exact rect (verified 0.44.3)

A **suppressed** (background) pane is one hidden behind the pane that replaced
its slot — e.g. `edit-scrollback` opens `$EDITOR` over the current pane, or
`new-pane --in-place` *suspends* (does not close, unless `--close-replaced-pane`)
the pane it replaces. Verified empirically with the eprintln oracle (#4) against
zellij 0.44.3 while building #118:

- The suppressed pane keeps its entry — `is_suppressed = true`, its **id**, and
  its full `pane_x/y/columns/rows` — while hidden, and survives unrelated
  `PaneUpdate`s.
- Its geometry is the **exact rect of its cover** (the replacement pane). Observed:
  suppressed `id=0 x=0 y=3 w=120 h=37` under a cover `id=1` (focused,
  `title="EDITING SCROLLBACK"`) reporting the *same* `x=0 y=3 w=120 h=37`. Since
  tiled panes partition the space without overlap, exactly one visible tiled pane
  contains a suppressed pane's rect — its cover.
- **Both plugins and terminals can be suppressed.** A suppressed *plugin* overlay
  also appears (the zellij-attention plugin: `is_suppressed=true is_plugin=true
  x=30 y=10 w=60 h=20`). Filter with `is_suppressed && !is_plugin` to get just the
  user's suppressed terminal panes — the suppressed sibling of the tiled/floating
  filters in `src/projection.rs`.

Consequence for a bar plugin (#118): you can mark the *cover* pane (the visible
tiled pane whose rect contains a suppressed pane) to signal "something is hidden
behind this", by matching suppressed rects against the tiled set via geometry
containment. `is_suppressed` is read-only within the existing `ReadApplicationState`
grant — no new permission (so no #15 freeze).

**Harness note:** driving this headlessly needs a client attached when the
suppress happens (#3) — `zellij action edit-scrollback` / `new-pane --in-place`
land as no-ops on the plugin's `update()` until an `expect` PTY re-attaches
(#4); the attach and the trigger must overlap in time.

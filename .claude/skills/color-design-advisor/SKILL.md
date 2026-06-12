---
name: color-design-advisor
description: >
  Get visual color-design feedback on the minimap palette from Gemini.
  Builds the render_active_cue example, takes a screenshot, reads the
  current blend constants, then delegates to Gemini's tabmap-color-advisor
  skill for analysis and suggestions. Use when tweaking ACTIVE_UNFOCUSED_BLEND
  or INACTIVE_LABEL_BLEND and wanting an outside visual opinion.
allowed-tools:
  - Bash(cargo build*)
  - Bash(vhs*)
  - Bash(grep*)
  - Bash(mktemp*)
  - Bash(rm*)
  - Bash(gemini*)
  - Read(src/minimap.rs)
---

# color-design-advisor

Orchestrates: build → screenshot → ask Gemini → show result.

## Step 1 — Build the visual harness

Detect the host target triple at runtime:

```bash
HOST_TRIPLE="$(rustc -vV | awk '/host:/ {print $2}')"
CARGO_BUILD_TARGET="$HOST_TRIPLE" cargo build --example render_active_cue -q
```

If it fails, report the error and stop.

## Step 2 — Generate a PNG screenshot

Write a VHS tape to a tempfile and run it. Use absolute paths (expand the
repo root yourself before writing):

```
Output /tmp/tabmap-color-check-dummy.gif
Set Shell "bash"
Set Width 920
Set Height 220
Set Theme "TokyoNight"
Hide
Type "export COLORTERM=truecolor"
Enter
Sleep 100ms
Type "<absolute-path-to-binary>"
Enter
Sleep 2s
Show
Sleep 500ms
Screenshot /tmp/tabmap-color-check.png
Sleep 100ms
```

Binary path: `<repo-root>/target/<host-triple>/debug/examples/render_active_cue`

## Step 3 — Read current constants

From `src/minimap.rs`, extract:
- `ACTIVE_UNFOCUSED_BLEND` value — **may be absent** if the three-level
  brightness feature has not been implemented yet; use empty string if missing
- `INACTIVE_LABEL_BLEND` value

## Step 4 — Call Gemini

```bash
gemini -p "tabmap-color-check: image=/tmp/tabmap-color-check.png ACTIVE_UNFOCUSED_BLEND=<n> INACTIVE_LABEL_BLEND=<n>"
```

Pass the value of `ACTIVE_UNFOCUSED_BLEND` as empty if the constant is absent.
This triggers the `tabmap-color-advisor` Gemini skill. If the output does NOT
contain `ASSESSMENT:`, the skill may not have loaded — inform the user that the
Gemini skill may need to be installed globally (see `.gemini/skills/tabmap-color-advisor/`).

## Step 5 — Present the result

Show Gemini's full output. Before offering to apply a suggested constant change,
verify that the constant actually exists in `src/minimap.rs`. If it doesn't:

- Tell the user the constant is not yet wired into the renderer
- Offer to implement it (this requires a code change beyond editing a constant)

If the constant does exist, offer to update its value in `src/minimap.rs`.

## Install note (first time)

The Gemini skill lives at `.gemini/skills/tabmap-color-advisor/` in this repo.
For Gemini to pick it up globally, symlink or copy it:

```bash
ln -s "$(pwd)/.gemini/skills/tabmap-color-advisor" ~/.gemini/skills/
```

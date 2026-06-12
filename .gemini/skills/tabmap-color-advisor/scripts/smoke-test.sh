#!/usr/bin/env bash
# Smoke-test: verify that `gemini -p "tabmap-color-check: ..."` triggers the
# tabmap-color-advisor skill and returns a structured assessment.
# Run from the repo root after installing the skill globally (see README).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../../../.." && pwd)"
HOST_TRIPLE="$(rustc -vV | awk '/host:/ {print $2}')"
PNG="$(mktemp /tmp/tabmap-smoke-XXXXXX.png)"
DUMMY_GIF="$(mktemp /tmp/tabmap-smoke-XXXXXX.gif)"

echo "Building render_active_cue (target: $HOST_TRIPLE)..."
CARGO_BUILD_TARGET="$HOST_TRIPLE" cargo build --example render_active_cue \
  --manifest-path "$REPO_ROOT/Cargo.toml" -q

echo "Generating screenshot..."
TAPE="$(mktemp /tmp/tabmap-smoke-XXXXXX.tape)"
cat > "$TAPE" << EOF
Output $DUMMY_GIF
Set Shell "bash"
Set Width 920
Set Height 220
Set Theme "TokyoNight"
Hide
Type "export COLORTERM=truecolor"
Enter
Sleep 100ms
Type "$REPO_ROOT/target/$HOST_TRIPLE/debug/examples/render_active_cue"
Enter
Sleep 2s
Show
Sleep 500ms
Screenshot $PNG
Sleep 100ms
EOF
vhs "$TAPE" 2>/dev/null
rm -f "$TAPE" "$DUMMY_GIF"

# Extract current constants (empty string if not found — constant may not be
# implemented yet; Gemini still returns a suggestion as a recommendation)
BLEND_UNFOCUSED="$(grep -o 'ACTIVE_UNFOCUSED_BLEND[[:space:]]*:[[:space:]]*u8[[:space:]]*=[[:space:]]*[0-9]*' \
  "$REPO_ROOT/src/minimap.rs" | grep -o '[0-9]*$' || true)"
BLEND_INACTIVE="$(grep -o 'INACTIVE_LABEL_BLEND[[:space:]]*:[[:space:]]*u8[[:space:]]*=[[:space:]]*[0-9]*' \
  "$REPO_ROOT/src/minimap.rs" | grep -o '[0-9]*$' || true)"

printf 'Querying Gemini (trigger: tabmap-color-check)...\n'
printf '  image=%s\n' "$PNG"
printf '  ACTIVE_UNFOCUSED_BLEND=%s\n' "$BLEND_UNFOCUSED"
printf '  INACTIVE_LABEL_BLEND=%s\n' "$BLEND_INACTIVE"
printf '\n'

gemini -p "tabmap-color-check: image=$PNG ACTIVE_UNFOCUSED_BLEND=$BLEND_UNFOCUSED INACTIVE_LABEL_BLEND=$BLEND_INACTIVE"

rm -f "$PNG"

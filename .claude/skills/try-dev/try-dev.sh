#!/usr/bin/env bash
#
# try-dev.sh — build the zellij-tabmap bar from a branch/worktree and launch a
# throwaway zellij session that loads it via file:, WITHOUT touching your real
# config.kdl / default.kdl or any release install.
#
# The bar is loaded from default_tab_template, so zellij shows no usable
# permission prompt for it (zellij#4982) and its pane is non-selectable — so the
# grant is pre-seeded here instead. The dev session is disposable: exit it and
# close the tab; nothing persists but the single refreshed dev grant and the
# build in target/ (both overwritten next run — nothing accumulates).
#
# Usage:
#   try-dev.sh [BRANCH|WORKTREE] [--release] [--no-build] [--logs] [--no-launch]
#     BRANCH|WORKTREE  build that branch (reusing/creating a worktree) or a
#                      worktree path; omit to build the current worktree's HEAD
#     --release        optimized build (default: fast debug build)
#     --no-build       skip cargo; use the last-built wasm as-is
#     --logs           add a `tail -F zellij.log` pane to the dev tab
#     --no-launch      prepare only; print the launch command
#
#   Env override: ZELLIJ_PERMISSIONS  path to permissions.kdl (default: OS cache)

set -euo pipefail

PROFILE=debug          # debug (default, fast) | release
BUILD=1
LOGS=0
LAUNCH=1
SRC_ARG=""

die() { printf 'try-dev: %s\n' "$1" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case "$1" in
    --release)   PROFILE=release ;;
    --debug)     PROFILE=debug ;;
    --no-build)  BUILD=0 ;;
    --logs)      LOGS=1 ;;
    --no-launch) LAUNCH=0 ;;
    -h|--help)   awk 'NR>1 && /^#/{sub(/^# ?/,"");print;next} NR>1{exit}' "$0"; exit 0 ;;
    -*)          die "unknown option: $1" ;;
    *)           [ -z "$SRC_ARG" ] || die "unexpected extra argument: $1"; SRC_ARG="$1" ;;
  esac
  shift
done

# --- locate the repo root (this script lives in .claude/skills/try-dev/) ------
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel 2>/dev/null)" \
  || die "not inside the zellij-tabmap git repo"

# --- resolve the build source directory ---------------------------------------
if [ -z "$SRC_ARG" ]; then
  SRC="$REPO_ROOT"
elif [ -d "$SRC_ARG" ] && git -C "$SRC_ARG" rev-parse --show-toplevel >/dev/null 2>&1; then
  SRC="$(git -C "$SRC_ARG" rev-parse --show-toplevel)"
else
  BRANCH="$SRC_ARG"
  # reuse an existing worktree checked out at this branch, if any
  SRC="$(git -C "$REPO_ROOT" worktree list --porcelain \
        | awk -v b="refs/heads/$BRANCH" '/^worktree /{w=substr($0,10)} /^branch /{if(substr($0,8)==b) print w}' \
        | head -1)"
  if [ -z "$SRC" ]; then
    git -C "$REPO_ROOT" show-ref --verify --quiet "refs/heads/$BRANCH" \
      || git -C "$REPO_ROOT" ls-remote --exit-code --heads origin "$BRANCH" >/dev/null 2>&1 \
      || die "no such branch or worktree: $BRANCH"
    SRC="$REPO_ROOT/.claude/worktrees/try-dev-$(printf '%s' "$BRANCH" | tr '/' '-')"
    if [ ! -d "$SRC" ]; then
      git -C "$REPO_ROOT" fetch -q origin "$BRANCH" 2>/dev/null || true
      git -C "$REPO_ROOT" worktree add -q "$SRC" "$BRANCH" \
        || die "failed to create a worktree for $BRANCH"
      echo "created worktree: $SRC"
    fi
  fi
fi

# --- build (or locate the existing build) -------------------------------------
WASM="$SRC/target/wasm32-wasip1/$PROFILE/zellij-tabmap.wasm"
if [ "$BUILD" = 1 ]; then
  command -v cargo >/dev/null 2>&1 || die "cargo not found — install Rust: https://rustup.rs"
  rustup target list --installed 2>/dev/null | grep -qx wasm32-wasip1 \
    || die "the wasm32-wasip1 target is missing — run: rustup target add wasm32-wasip1"
  echo "building ($PROFILE) in $SRC ..."
  if [ "$PROFILE" = release ]; then
    ( cd "$SRC" && cargo build --target wasm32-wasip1 --release ) || die "cargo build failed"
  else
    ( cd "$SRC" && cargo build --target wasm32-wasip1 ) || die "cargo build failed"
  fi
fi
[ -f "$WASM" ] || die "wasm not found: $WASM (drop --no-build to build it)"

# --- permissions.kdl: strip stale dev-build grants, add this build's grant -----
case "$(uname -s)" in
  Darwin) PERM_DIR="$HOME/Library/Caches/org.Zellij-Contributors.Zellij" ;;
  *)      PERM_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/zellij" ;;
esac
PERM="${ZELLIJ_PERMISSIONS:-$PERM_DIR/permissions.kdl}"
mkdir -p "$(dirname "$PERM")"; [ -f "$PERM" ] || : > "$PERM"
tmp="$(mktemp)"
awk '
  skip==0 && $0 ~ /target\/wasm32-wasip1\/(debug|release)\/zellij-tabmap\.wasm"[[:space:]]*\{/ {
    skip=1; depth=0; depth+=gsub(/\{/,"{"); depth-=gsub(/\}/,"}"); if(depth<=0)skip=0; next
  }
  skip==1 { depth+=gsub(/\{/,"{"); depth-=gsub(/\}/,"}"); if(depth<=0)skip=0; next }
  { print }
' "$PERM" > "$tmp"
{
  cat "$tmp"
  printf '"file:%s" {\n    ReadApplicationState\n    ChangeApplicationState\n}\n' "$WASM"
  printf '"%s" {\n    ReadApplicationState\n    ChangeApplicationState\n}\n' "$WASM"
} > "$PERM"
rm -f "$tmp"

# --- resolve zellij.log (for --logs) ------------------------------------------
case "$(uname -s)" in
  Darwin) ZLOG="$(getconf DARWIN_USER_TEMP_DIR)zellij-$(id -u)/zellij-log/zellij.log" ;;
  *)      ZLOG="${XDG_RUNTIME_DIR:-/tmp}/zellij-$(id -u)/zellij-log/zellij.log" ;;
esac

# --- throwaway dev layout (never touches your config.kdl / default.kdl) --------
LAYOUT="${TMPDIR:-/tmp}/tabmap-dev.kdl"
{
  echo 'layout {'
  echo '    default_tab_template {'
  echo '        pane size=4 borderless=true {'
  echo "            plugin location=\"file:$WASM\" {"
  echo '                floating "hybrid"'
  echo '                perspective "true"'
  echo '                close_button "true"'
  echo '                scroll "pane"'
  echo '            }'
  echo '        }'
  echo '        children'
  echo '        pane size=1 borderless=true { plugin location="status-bar" }'
  echo '    }'
  echo '    tab name="dev" focus=true {'
  if [ "$LOGS" = 1 ]; then
    echo '        pane split_direction="horizontal" {'
    echo '            pane'
    echo '            pane size="30%" command="tail" {'
    echo "                args \"-n0\" \"-F\" \"$ZLOG\""
    echo '            }'
    echo '        }'
  else
    echo '        pane'
  fi
  echo '    }'
  echo '}'
} > "$LAYOUT"

# --- kill any stale dev session so a fresh server reads the new build ----------
zellij delete-session --force tabmap-dev >/dev/null 2>&1 || true

cat <<EOF
try-dev ready:
  source:  $SRC
  wasm:    $WASM ($PROFILE)
  layout:  $LAYOUT$([ "$LOGS" = 1 ] && echo ' (+ log pane)')
  grant:   $PERM (dev grants refreshed)
EOF

# --- launch -------------------------------------------------------------------
LAUNCH_CMD="env ZELLIJ=0 zellij -s tabmap-dev -n \"$LAYOUT\""
if [ "$LAUNCH" = 1 ] && [ -n "${ZELLIJ:-}" ]; then
  # nested: run the dev session inside a new tab/pane of the current session
  TABLAUNCH="${TMPDIR:-/tmp}/tabmap-dev-launch.kdl"
  {
    echo 'layout {'
    echo '    pane {'
    echo '        command "sh"'
    echo "        args \"-c\" \"ZELLIJ=0 exec zellij -s tabmap-dev -n '$LAYOUT'\""
    echo '    }'
    echo '}'
  } > "$TABLAUNCH"
  if zellij action new-tab --name tabmap-dev --layout "$TABLAUNCH" 2>/dev/null; then
    echo "launched nested dev session in a new tab 'tabmap-dev' — exit it with Ctrl+q, then close the tab"
  elif zellij run --name tabmap-dev --close-on-exit -- env ZELLIJ=0 zellij -s tabmap-dev -n "$LAYOUT" 2>/dev/null; then
    echo "launched nested dev session in a new pane — fullscreen it if you like, exit with Ctrl+q"
  else
    echo "auto-launch failed; run this in a pane:"
    echo "  $LAUNCH_CMD"
  fi
else
  echo "run this to launch:"
  echo "  $LAUNCH_CMD"
fi

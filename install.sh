#!/usr/bin/env bash
#
# install.sh — point your zellij config at a zellij-tabmap release and grant
# it permissions, without the interactive y/n dance.
#
# The tab bar is loaded from `default_tab_template`, so zellij never shows a
# usable permission prompt for it (zellij#4982) — and its pane is
# non-selectable, so even an ad-hoc prompt cannot be focused. That makes every
# version-up and first install a chore: you have to hand-grant the plugin.
# This script removes that chore. It does NOT place the wasm locally; zellij
# still fetches it from the release URL. It only:
#
#   1. rewrites the tabmap `location=` URL in your config.kdl to the target
#      release version (the reference style stays a release URL), and
#   2. writes the ReadApplicationState + ChangeApplicationState grant for that
#      exact URL into zellij's permissions.kdl.
#
# permissions.kdl is read once at server start, so restart zellij (a fresh
# session) after running this to apply the grant.
#
# Usage:
#   ./install.sh [VERSION]
#     VERSION   release tag to install, e.g. v0.14.0 (a bare 0.14.0 is
#               accepted too). Defaults to the latest published release.
#
# Environment overrides:
#   TABMAP_REPO          owner/repo             (default: GeneralD/zellij-tabmap)
#   ZELLIJ_CONFIG        path to config.kdl     (default: $XDG_CONFIG_HOME/zellij/config.kdl)
#   ZELLIJ_PERMISSIONS   path to permissions.kdl (default: OS zellij cache dir)

set -euo pipefail

REPO="${TABMAP_REPO:-GeneralD/zellij-tabmap}"
ASSET="zellij-tabmap.wasm"

die() { printf 'install.sh: %s\n' "$1" >&2; exit 1; }

# --- locate config.kdl ------------------------------------------------------
CONFIG="${ZELLIJ_CONFIG:-${XDG_CONFIG_HOME:-$HOME/.config}/zellij/config.kdl}"
[ -f "$CONFIG" ] || die "zellij config not found: $CONFIG (set ZELLIJ_CONFIG)"

# --- locate permissions.kdl (OS-specific cache dir) -------------------------
case "$(uname -s)" in
  Darwin) PERM_DIR="$HOME/Library/Caches/org.Zellij-Contributors.Zellij" ;;
  *)      PERM_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/zellij" ;;
esac
PERM="${ZELLIJ_PERMISSIONS:-$PERM_DIR/permissions.kdl}"

# --- resolve the target version ---------------------------------------------
VERSION="${1:-}"
if [ -z "$VERSION" ]; then
  VERSION="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
    | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -1)"
  [ -n "$VERSION" ] || die "could not resolve the latest release tag; pass VERSION explicitly"
fi
# normalize a bare "0.14.0" to "v0.14.0"
case "$VERSION" in [0-9]*) VERSION="v$VERSION" ;; esac

URL="https://github.com/$REPO/releases/download/$VERSION/$ASSET"

# --- verify the asset exists before pointing zellij at it -------------------
# zellij downloads a plugin URL without checking the HTTP status and caches the
# bytes as the wasm; a 404 body then poisons the cache permanently. Never point
# the config at a URL whose asset is missing.
if ! curl -fsL -o /dev/null -r 0-0 "$URL" 2>/dev/null \
  && ! curl -fsL -o /dev/null "$URL" 2>/dev/null; then
  die "release asset not reachable: $URL (is $VERSION published?)"
fi

# --- 1. bump the tabmap location URL in config.kdl --------------------------
url_re="location=\"https://github.com/$REPO/releases/(download/[^/\"]+|latest/download)/$ASSET\""
grep -qE "$url_re" "$CONFIG" \
  || die "no tabmap release-URL 'location=' found in $CONFIG
        (this tool manages release-URL installs; a file: install needs no bump)"

tmp="$(mktemp)"
sed -E "s#(location=\")https://github.com/$REPO/releases/(download/[^/\"]+|latest/download)/$ASSET(\")#\1$URL\3#g" \
  "$CONFIG" > "$tmp"
if cmp -s "$tmp" "$CONFIG"; then
  config_note="already $VERSION — unchanged"
else
  cp "$CONFIG" "$CONFIG.bak"   # snapshot the pre-bump config; revert with: mv "$CONFIG.bak" "$CONFIG"
  cat "$tmp" > "$CONFIG"       # rewrite in place (preserves a symlinked config.kdl)
  config_note="bumped (backup: $CONFIG.bak)"
fi
rm -f "$tmp"

# --- 2. write the permission grant for the exact URL ------------------------
mkdir -p "$(dirname "$PERM")"
[ -f "$PERM" ] || : > "$PERM"
tmp="$(mktemp)"
# strip any existing zellij-tabmap grant block(s) — old versions, latest, file:
awk -v asset="$ASSET" '
  skip==0 && $0 ~ ("^[[:space:]]*\"[^\"]*" asset "\"[[:space:]]*\\{") {
    skip=1; depth=0
    depth += gsub(/\{/, "{"); depth -= gsub(/\}/, "}")
    if (depth<=0) skip=0
    next
  }
  skip==1 {
    depth += gsub(/\{/, "{"); depth -= gsub(/\}/, "}")
    if (depth<=0) skip=0
    next
  }
  { print }
' "$PERM" > "$tmp"
{
  cat "$tmp"
  printf '"%s" {\n    ReadApplicationState\n    ChangeApplicationState\n}\n' "$URL"
} > "$PERM"
rm -f "$tmp"

# --- report -----------------------------------------------------------------
cat <<EOF
zellij-tabmap $VERSION installed.
  config:      $CONFIG  ($config_note)
  permissions: $PERM
  location:    $URL

Restart zellij (a fresh session) to apply — permissions.kdl is read only at
server start. On that first post-bump session the uncached URL downloads while
the bar sits empty; if it stays blank, open a second tab once (a one-time
download race, not a failure).
EOF

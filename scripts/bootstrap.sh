#!/usr/bin/env bash
# Fetch pinned upstream sources into vendor/ and apply Tether's patches.
set -euo pipefail
cd "$(dirname "$0")/.."
# A tag can be moved upstream; the commit cannot. When updating a tag, update its commit too.
RUSTDESK_TAG=1.5.1 RUSTDESK_COMMIT=23e5bb471a040d62f0a6ad52591fb30784265b73
SERVER_TAG=1.1.16 SERVER_COMMIT=73523b31cfd25d77dee862e6fc9f5e1fb5e485ef

fetch() { # url tag dir commit
  if [ ! -d "$3/.git" ]; then
    # Plain LF checkouts: Windows runners convert to CRLF by default, which makes the patches
    # (stored with LF) no longer match.
    git -c core.autocrlf=false clone -q --depth 1 --branch "$2" --recurse-submodules --shallow-submodules "$1" "$3"
  fi
  # Fresh clone or existing tree: build only the reviewed commit.
  local head; head=$(git -C "$3" rev-parse HEAD)
  if [ "$head" != "$4" ]; then
    echo "$3 is at $head, expected $4 (tag $2): remove $3 and run again, or update the pinned commit in $0" >&2
    exit 1
  fi
}
fetch https://github.com/rustdesk/rustdesk.git "$RUSTDESK_TAG" vendor/rustdesk "$RUSTDESK_COMMIT"
fetch https://github.com/rustdesk/rustdesk-server.git "$SERVER_TAG" vendor/rustdesk-server "$SERVER_COMMIT"

apply() { # dir patch
  local p="$PWD/$2"
  if git -C "$1" apply --check "$p" 2>/dev/null; then
    git -C "$1" apply "$p" && echo "applied $2"
  elif git -C "$1" apply --check --ignore-whitespace "$p" 2>/dev/null; then
    # Line-ending differences only (e.g. a CRLF checkout of the patch itself).
    git -C "$1" apply --ignore-whitespace "$p" && echo "applied $2 (ignoring whitespace)"
  elif git -C "$1" apply --reverse --check "$p" 2>/dev/null \
    || git -C "$1" apply --reverse --check --ignore-whitespace "$p" 2>/dev/null; then
    # The second form: a tree patched on Windows and copied with CRLF line endings.
    echo "already applied $2"
  else
    echo "cannot apply $2 cleanly:" >&2
    git -C "$1" apply --check -v "$p" >&2 || true
    exit 1
  fi
}
apply vendor/rustdesk patches/rustdesk.patch
apply vendor/rustdesk/libs/hbb_common patches/hbb_common.patch
apply vendor/rustdesk-server patches/rustdesk-server.patch
apply vendor/rustdesk-server/libs/hbb_common patches/rustdesk-server-hbb_common.patch
# rustdesk-server's Cargo.toml patches the turn crate from patched/turn (build-relay.sh copies it too).
rm -rf vendor/rustdesk-server/patched && cp -r server/patched vendor/rustdesk-server/patched

#!/usr/bin/env bash
# Fetch pinned upstream sources into vendor/ and apply Tether's patches.
set -euo pipefail
cd "$(dirname "$0")/.."
RUSTDESK_TAG=1.5.1
SERVER_TAG=1.1.16

fetch() { # url tag dir
  if [ ! -d "$3/.git" ]; then
    # Plain LF checkouts: Windows runners convert to CRLF by default, which makes the patches
    # (stored with LF) no longer match.
    git -c core.autocrlf=false clone -q --depth 1 --branch "$2" --recurse-submodules --shallow-submodules "$1" "$3"
  fi
}
fetch https://github.com/rustdesk/rustdesk.git "$RUSTDESK_TAG" vendor/rustdesk
fetch https://github.com/rustdesk/rustdesk-server.git "$SERVER_TAG" vendor/rustdesk-server

apply() { # dir patch
  local p="$PWD/$2"
  if git -C "$1" apply --check "$p" 2>/dev/null; then
    git -C "$1" apply "$p" && echo "applied $2"
  elif git -C "$1" apply --check --ignore-whitespace "$p" 2>/dev/null; then
    # Line-ending differences only (e.g. a CRLF checkout of the patch itself).
    git -C "$1" apply --ignore-whitespace "$p" && echo "applied $2 (ignoring whitespace)"
  elif git -C "$1" apply --reverse --check "$p" 2>/dev/null; then
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

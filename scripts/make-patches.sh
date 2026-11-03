#!/usr/bin/env bash
# Regenerate patches/ from the working trees in vendor/ (run after changing vendored code).
set -euo pipefail
cd "$(dirname "$0")/.."
git -C vendor/rustdesk add -N src/tauri_ui
git -C vendor/rustdesk diff --ignore-submodules -- . > patches/rustdesk.patch
git -C vendor/rustdesk/libs/hbb_common diff > patches/hbb_common.patch
wc -l patches/*.patch

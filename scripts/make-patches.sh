#!/usr/bin/env bash
# Regenerate patches/ from the working trees in vendor/ (run after changing vendored code).
set -euo pipefail
cd "$(dirname "$0")/.."
git -C vendor/rustdesk add -N src/tauri_ui libs/scrap/src/dxgi/wgc.rs libs/scrap/examples/tearprobe.rs
git -C vendor/rustdesk diff --ignore-submodules -- . > patches/rustdesk.patch
git -C vendor/rustdesk/libs/hbb_common diff > patches/hbb_common.patch
git -C vendor/rustdesk-server add -N -f src/bin  # src/bin matches its .gitignore (bin)
git -C vendor/rustdesk-server diff -- src > patches/rustdesk-server.patch
git -C vendor/rustdesk-server/libs/hbb_common diff > patches/rustdesk-server-hbb_common.patch
wc -l patches/*.patch

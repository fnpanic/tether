#!/usr/bin/env bash
# Fails when the NSIS hooks name Tether literally outside comments. An instance build
# (TETHER_INSTANCE) installs the same hooks: a literal `Tether` / `tether.exe` would stop, delete or
# wipe the default build's service, firewall rule or app data. Use ${PRODUCTNAME} / ${MAINBINARYNAME}.
set -euo pipefail
hooks=$(dirname "$0")/../apps/desktop/src-tauri/windows/hooks.nsh
if grep -nE '\bTether\b|tether\.exe' "$hooks" | grep -vE '^[0-9]+:[[:space:]]*[;#]'; then
  echo "error: $hooks names Tether literally on the lines above (use \${PRODUCTNAME} or \${MAINBINARYNAME})" >&2
  exit 1
fi
echo "hooks.nsh: no literal Tether outside comments"

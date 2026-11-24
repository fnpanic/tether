#!/usr/bin/env bash
# Fails when the NSIS hooks name Tether literally outside comments. An instance build
# (TETHER_INSTANCE) installs the same hooks: a literal `Tether` / `tether.exe` (any case) would stop,
# delete or wipe the default build's service, firewall rule or app data. Use ${PRODUCTNAME} /
# ${MAINBINARYNAME}. Optional argument: the file to check (default: the app's hooks.nsh).
set -euo pipefail
hooks=${1:-$(dirname "$0")/../apps/desktop/src-tauri/windows/hooks.nsh}
# The word in any case; TETHER_SC, $TetherRelaunch and the like are one longer word and do not match.
if grep -niE '(^|[^A-Za-z0-9_])tether([^A-Za-z0-9_]|$)' "$hooks" | grep -vE '^[0-9]+:[[:space:]]*[;#]'; then
  echo "error: $hooks names Tether literally on the lines above (use \${PRODUCTNAME} or \${MAINBINARYNAME})" >&2
  exit 1
fi
echo "hooks.nsh: no literal Tether outside comments"

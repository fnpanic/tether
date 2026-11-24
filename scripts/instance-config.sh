#!/usr/bin/env bash
# Prints the Tauri `--config` JSON of an instance build (TETHER_INSTANCE, see
# apps/desktop/src-tauri/build.rs), nothing for the default build:
#
#   npx tauri build --config "$(bash scripts/instance-config.sh north)"
#
# The names must match what build.rs compiles in (APP_NAME Tether-<i>).
set -euo pipefail
i=${1-}
[ -n "$i" ] || exit 0
if ! [[ $i =~ ^[a-z0-9]{1,16}$ ]]; then
  echo "error: TETHER_INSTANCE must be empty or 1 to 16 characters a-z, 0-9 (got '$i')" >&2
  exit 2
fi
printf '{"productName":"Tether-%s","mainBinaryName":"tether-%s","identifier":"app.tether.desktop.%s","plugins":{"deep-link":{"desktop":{"schemes":["tether-%s"]}}}}\n' "$i" "$i" "$i" "$i"

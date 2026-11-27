#!/usr/bin/env bash
# Sign the published download/<channel>/monitor.json again with a new expiry (30 days from now) on
# the given relays via tools/monitor-publish (built here, rebuilt when its sources changed). An
# admin step, about once a month, from the admin's own machine: the index expires 30 days after
# the last publish or refresh.
#
#   bash scripts/monitor-refresh.sh <channel> <https://relay>[,<https://relay>...]
#
# Env as scripts/status-token.sh: MANAGE_KEYS, MANAGE_PASSPHRASE (asked when not set).
set -euo pipefail
[ $# -eq 2 ] || { sed -n '7p' "$0" | sed 's/^# *//'; exit 2; }
root=$(cd "$(dirname "$0")/.." && pwd)
bin="$root/tools/monitor-publish/target/release/monitor-publish"
# Always: cargo rebuilds only when the sources changed (a no-op otherwise).
cargo build --release --quiet --manifest-path "$root/tools/monitor-publish/Cargo.toml" --target-dir "$root/tools/monitor-publish/target" >&2
if [ -z "${MANAGE_PASSPHRASE:-}" ]; then
  read -rsp "Tether Manage passphrase: " MANAGE_PASSPHRASE; echo >&2
fi
export MANAGE_PASSPHRASE
out=$("$bin" refresh --channel "$1" --relay "$2") && rc=0 || rc=$?
printf '%s\n' "$out"
if [ "$rc" -ne 0 ] || grep -q 'FAILED' <<<"$out"; then
  exit 1
fi

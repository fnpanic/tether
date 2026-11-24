#!/usr/bin/env bash
# Publish the device-status agent (monitor) or its check packs (monitor-packs) to
# download/<channel>/monitor.json on the given relays via tools/monitor-publish (built here, rebuilt when
# its sources changed). <file>.sig (the build's updater signature) must lie next to <file>. Re-running the same
# command repairs relays a partial publish missed.
#
#   bash scripts/monitor-publish.sh <channel> <monitor|monitor-packs> <version> <file> <https://relay>[,<https://relay>...] [standard|fast|now]
#
# Env as scripts/status-token.sh: MANAGE_KEYS, MANAGE_PASSPHRASE (asked when not set).
set -euo pipefail
[ $# -ge 5 ] && [ $# -le 6 ] || { sed -n '7p' "$0" | sed 's/^# *//'; exit 2; }
root=$(cd "$(dirname "$0")/.." && pwd)
bin="$root/tools/monitor-publish/target/release/monitor-publish"
# Always: cargo rebuilds only when the sources changed (a no-op otherwise).
cargo build --release --quiet --manifest-path "$root/tools/monitor-publish/Cargo.toml" --target-dir "$root/tools/monitor-publish/target" >&2
if [ -z "${MANAGE_PASSPHRASE:-}" ]; then
  read -rsp "Tether Manage passphrase: " MANAGE_PASSPHRASE; echo >&2
fi
export MANAGE_PASSPHRASE
out=$("$bin" index --channel "$1" --app "$2" --version "$3" --file "$4" --relay "$5" --rollout "${6:-standard}") && rc=0 || rc=$?
printf '%s\n' "$out"
if [ "$rc" -ne 0 ] || grep -q 'FAILED' <<<"$out"; then
  exit 1
fi

#!/usr/bin/env bash
# Device-status enrolment token, signed with the admin key from Tether Manage's key file, via
# tools/monitor-publish (built here, rebuilt when its sources changed). Prints the token; its id, label and expiry go
# to stderr and to the tool's journal (monitor-tokens.json next to the key file).
#
#   bash scripts/status-token.sh <label> [days=7] [tether-id] [customer]   # days 1-30; a Tether ID: one device, that ID ("" for none); customer: 1-32 of A-Za-z0-9._-
#
# Env: MANAGE_KEYS (key file; default %APPDATA%\Tether-Manage\keys.json), MANAGE_PASSPHRASE (asked
# when not set; never pass it on the command line).
set -euo pipefail
[ $# -ge 1 ] && [ $# -le 4 ] || { sed -n '6p' "$0" | sed 's/^# *//'; exit 2; }
root=$(cd "$(dirname "$0")/.." && pwd)
bin="$root/tools/monitor-publish/target/release/monitor-publish"
# Always: cargo rebuilds only when the sources changed (a no-op otherwise).
cargo build --release --quiet --manifest-path "$root/tools/monitor-publish/Cargo.toml" --target-dir "$root/tools/monitor-publish/target" >&2
if [ -z "${MANAGE_PASSPHRASE:-}" ]; then
  read -rsp "Tether Manage passphrase: " MANAGE_PASSPHRASE; echo >&2
fi
export MANAGE_PASSPHRASE
"$bin" token --label "$1" --days "${2:-7}" ${3:+--tether-id "$3"} ${4:+--customer "$4"}

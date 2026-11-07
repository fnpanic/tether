#!/usr/bin/env bash
# tether-api under device load, straight to its port with X-Forwarded-For (as from Caddy):
#   tools/load/api-run.sh <bin-dir> <out-prefix> <seconds> [tether-load api args...]
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"; BIN="$1"; OUT="$2"; SECS="$3"; shift 3
W=/tmp/lt-api; "$HERE/stack.sh" stop "$W" 2>/dev/null || true; rm -rf "$W"
ulimit -n 1048576
"$HERE/stack.sh" start "$BIN" "$W" >/dev/null
"$HERE/sample.sh" "$((SECS + 5))" 5 api="$(cat "$W/api.pid")" > "$OUT.sample.jsonl" &
"${LOAD:-$HOME/scale-build/bin/tether-load}" mode=api addr=127.0.0.1:25120 seconds="$SECS" "$@" > "$OUT.load.jsonl"
wait
ls -la "$W/api/devices.json" 2>/dev/null || true
"$HERE/stack.sh" stop "$W"

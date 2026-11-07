#!/usr/bin/env bash
# Fresh stack from <bin-dir>, then tether-load relay with the given args; samples hbbr and Caddy.
#   tools/load/relay-run.sh <bin-dir> <out-prefix> <seconds> [tether-load args...]
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"; BIN="$1"; OUT="$2"; SECS="$3"; shift 3
W=/tmp/lt-relay; "$HERE/stack.sh" stop "$W" 2>/dev/null || true; rm -rf "$W"
ulimit -n 1048576
"$HERE/stack.sh" start "$BIN" "$W" >/dev/null
K="$(cat "$W/hb/id_ed25519.pub")"
"${LOAD:-$HOME/scale-build/bin/tether-load}" mode=relay key="$K" seconds="$SECS" "$@" > "$OUT.load.jsonl" &
lg=$!
"$HERE/sample.sh" "$((SECS + 15))" 5 hbbr="$(cat "$W/hbbr.pid")" caddy="$(cat "$W/caddy.pid")" caddy_client="$(cat "$W/caddy2.pid")" load="$lg" > "$OUT.sample.jsonl" || true
wait "$lg" || true
"$HERE/stack.sh" stop "$W"
grep -c "Downgrade" "$W/hbbr.log" || true

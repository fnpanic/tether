#!/usr/bin/env bash
# Fresh stack from <bin-dir>, then tether-load hosts with the given args against it; samples hbbs
# (and Caddy) meanwhile. Output: <out>.load.jsonl, <out>.sample.jsonl.
#   tools/load/hosts-run.sh <bin-dir> <out-prefix> <seconds> [tether-load args...]
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"; BIN="$1"; OUT="$2"; SECS="$3"; shift 3
W=/tmp/lt-run; "$HERE/stack.sh" stop "$W" 2>/dev/null || true; rm -rf "$W"
ulimit -n 1048576
"$HERE/stack.sh" start "$BIN" "$W" >/dev/null
K="$(cat "$W/hb/id_ed25519.pub")"
sleep 1
"$HERE/sample.sh" "$((SECS + 5))" 5 hbbs="$(cat "$W/hbbs.pid")" caddy="$(cat "$W/caddy.pid")" caddy_client="$(cat "$W/caddy2.pid")" > "$OUT.sample.jsonl" &
"${LOAD:-$HOME/scale-build/bin/tether-load}" mode=hosts key="$K" seconds="$SECS" "$@" > "$OUT.load.jsonl"
wait
grep -c "TOO_FREQUENT\|too frequent" "$W/hbbs.log" || true
"$HERE/stack.sh" stop "$W"
tail -n 2 "$OUT.load.jsonl"; tail -n 2 "$OUT.sample.jsonl"

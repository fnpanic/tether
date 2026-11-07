#!/usr/bin/env bash
# Release download storm: <conns> clients fetch a 35 MB release from /download/ over and over,
# through the client-side Caddy (TLS to the Caddy under test), unthrottled or at <kbps> each.
#   tools/load/dl-run.sh <bin-dir> <out-prefix> <conns> [kbps]
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"; BIN="$1"; OUT="$2"; N="$3"; KBPS="${4:-0}"
W=/tmp/lt-dl; "$HERE/stack.sh" stop "$W" 2>/dev/null || true; rm -rf "$W"
d="$W/api/download/stable/customer/windows-x86_64"; mkdir -p "$d"
head -c 35000000 /dev/urandom > "$d/TetherQS-9.9.9.exe"
ulimit -n 1048576
"$HERE/stack.sh" start "$BIN" "$W" >/dev/null
"${LOAD:-$HOME/scale-build/bin/tether-load}" mode=get addr=127.0.0.1:28081 host=127.0.0.1:28081 path=/download/stable/customer/windows-x86_64/TetherQS-9.9.9.exe conns="$N" kbps="$KBPS" seconds=40 threads=4 report=5 > "$OUT.get.jsonl" &
g=$!
"$HERE/sample.sh" 40 5 caddy="$(cat "$W/caddy.pid")" caddy_client="$(cat "$W/caddy2.pid")" load="$g" > "$OUT.sample.jsonl"
wait "$g" || true
"$HERE/stack.sh" stop "$W"

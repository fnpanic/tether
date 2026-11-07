#!/usr/bin/env bash
# Slow custom-app downloads (/q/<token>, 35 MB) through Caddy while devices heartbeat through it:
#   tools/load/qdl-run.sh <bin-dir> <out-prefix> [slow downloads, default 30] [kbit/s each, default 2000]
# The custom app is written straight into the data dir (tether-api does not re-verify the stored
# profile when it serves it).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"; BIN="$1"; OUT="$2"; N="${3:-30}"; KBPS="${4:-2000}"
W=/tmp/lt-qdl; "$HERE/stack.sh" stop "$W" 2>/dev/null || true; rm -rf "$W"
mkdir -p "$W/api/custom" "$W/api/download/stable/customer/windows-x86_64"
TOK=abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQ
d="$W/api/download/stable/customer/windows-x86_64"
head -c 35000000 /dev/urandom > "$d/TetherQS-9.9.9.exe"
echo '{"version":"9.9.9","platforms":{"windows-x86_64":{"signature":"x","url":"https://r/download/stable/customer/windows-x86_64/TetherQS-9.9.9.exe"}}}' > "$d/latest.json"
echo "{\"id\":\"acme\",\"name\":\"ACME\",\"token\":\"$TOK\",\"expires\":$(( $(date +%s) + 86400 )),\"channel\":\"stable\",\"created\":1,\"downloads\":0,\"last_download\":null,\"profile\":{\"x\":1}}" > "$W/api/custom/acme.json"
ulimit -n 1048576
"$HERE/stack.sh" start "$BIN" "$W" >/dev/null
L="${LOAD:-$HOME/scale-build/bin/tether-load}"
"$L" mode=get addr=127.0.0.1:28080 host=localhost:28080 path=/q/$TOK conns="$N" kbps="$KBPS" seconds=60 report=10 > "$OUT.get.jsonl" &
g=$!
sleep 3
"$HERE/sample.sh" 55 5 api="$(cat "$W/api.pid")" caddy="$(cat "$W/caddy.pid")" > "$OUT.sample.jsonl" &
"$L" mode=api addr=127.0.0.1:28080 host=localhost:28080 devices=3000 period=50 seconds=50 conns=8 ips=1 report=10 > "$OUT.api.jsonl"
wait
"$HERE/stack.sh" stop "$W"
tail -n 1 "$OUT.api.jsonl"; tail -n 1 "$OUT.get.jsonl"; ls -la "$W/api/qcache" 2>/dev/null || true

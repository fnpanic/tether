#!/usr/bin/env bash
# hbbs restart under load: hosts register with raised per-IP limits (database filled), then hbbs
# is restarted from <bin-dir> with the default limits and the hosts re-register. The first hbbs
# is always the fixed build in $FILL_BIN (default ~/scale-build/bin), so both runs start equal.
#   tools/load/restart-run.sh <bin-dir> <out-prefix> <n> <ips>
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"; BIN="$1"; OUT="$2"; N="$3"; IPS="$4"
W=/tmp/lt-restart; "$HERE/stack.sh" stop "$W" 2>/dev/null || true; rm -rf "$W"
ulimit -n 1048576
HBBS_ENV="IP_BLOCK_PER_MIN=1000000 IP_BLOCK_IDS_PER_DAY=1000000" "$HERE/stack.sh" start "${FILL_BIN:-$HOME/scale-build/bin}" "$W" >/dev/null
K="$(cat "$W/hb/id_ed25519.pub")"
"${LOAD:-$HOME/scale-build/bin/tether-load}" mode=hosts key="$K" url=ws://127.0.0.1:25118 n="$N" rate=1000 ips="$IPS" seconds=300 report=5 > "$OUT.load.jsonl" &
lg=$!
sleep 60
pkill -9 -f -- "-p 25116 -k _"; sleep 1
(cd "$W/hb" && HEALTH_PORT=25122 setsid "$BIN/hbbs" -p 25116 -k _ -r ws://127.0.0.1:25119 > "$W/hbbs2.log" 2>&1 & echo $! > "$W/hbbs.pid")
echo "restarted at $(date +%s)" >&2
wait "$lg"
"$HERE/stack.sh" stop "$W"
grep -o '"registered":[0-9]*,' "$OUT.load.jsonl" | tr -d '"registered:,' | tr '\n' ' '; echo

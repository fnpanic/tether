#!/usr/bin/env bash
# One benchmark of one front proxy (docs/proxy-benchmark.md), fresh stack each time:
#   tools/proxy-bench/run.sh <front> idle <hosts>        idle TLS WebSockets on /ws/id (90 s)
#   tools/proxy-bench/run.sh <front> relay <pairs>       1 Mbit/s + 50 kbit/s per pair on /ws/relay
#   tools/proxy-bench/run.sh <front> storm <hosts>       all hosts connect within 1 s (TLS handshakes)
#   tools/proxy-bench/run.sh <front> dl <conns>          35 MB file from /download/, unthrottled
#   tools/proxy-bench/run.sh <front> api <devices>       every device's API cycle within 20 s
# <front>: caddy, haproxy, nginx, envoy, traefik (FRONT for tools/load/stack.sh). BIN (relay
# binaries, default ~/scale-build/bin), TAG (result name suffix). Results: results/<front><TAG>-<test>-<n>.*
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"; L="$HERE/../load"
F="$1"; T="$2"; N="$3"; BIN="${BIN:-$HOME/scale-build/bin}"; LOAD="${LOAD:-$BIN/tether-load}"
mkdir -p "$HERE/results"; OUT="$HERE/results/$F${TAG:-}-$T-$N"
W=/tmp/pb-run; "$L/stack.sh" stop "$W" 2>/dev/null || true; rm -rf "$W"
ulimit -n 1048576
if [ "$T" = dl ]; then
  d="$W/api/download/stable/customer/windows-x86_64"; mkdir -p "$d"
  head -c 35000000 /dev/urandom > "$d/TetherQS-9.9.9.exe"
fi
[ "$T" = relay ] && export HBBR_ENV="TOTAL_BANDWIDTH=100000"
# Every host reaches hbbs from the front's 127.0.0.1 (X-Forwarded-For is the last hop): lift the per-IP limits.
export HBBS_ENV="IP_BLOCK_PER_MIN=100000000 IP_BLOCK_IDS_PER_DAY=100000000"
FRONT="$F" "$L/stack.sh" start "$BIN" "$W" >/dev/null
for _ in $(seq 50); do curl -sk -o /dev/null https://localhost:28443/ && break; sleep 0.2; done
K="$(cat "$W/hb/id_ed25519.pub")"
P="front=$(cat "$W/caddy.pid") client=$(cat "$W/caddy2.pid") hbbs=$(cat "$W/hbbs.pid") hbbr=$(cat "$W/hbbr.pid") api=$(cat "$W/api.pid")"
[ -s "$W/nginx-files.pid" ] && P+=" files=$(cat "$W/nginx-files.pid")"
U=ws://127.0.0.1:28081
case "$T" in
idle)  secs=${SECS:-90}; args=(mode=hosts key="$K" url=$U/ws/id n="$N" rate=2000 ips=250 seconds=$secs threads=4 report=5) ;;
storm) secs=40; args=(mode=hosts key="$K" url=$U/ws/id n="$N" rate="$N" ips=250 seconds=$secs threads=6 report=5) ;;
relay) secs=45; args=(mode=relay key="$K" url=$U/ws/relay pairs="$N" kbps=1000 up_kbps=50 fps=30 rate=500 seconds=$secs threads=6 report=5) ;;
dl)    secs=30; args=(mode=get addr=127.0.0.1:28081 host=localhost path=/download/stable/customer/windows-x86_64/TetherQS-9.9.9.exe conns="$N" seconds=$secs threads=4 report=5) ;;
api)   secs=40; args=(mode=api addr=127.0.0.1:28081 host=localhost devices="$N" period=20 burst=1 seconds=$secs conns=64 ips=200 threads=4 report=5) ;;
*) echo "unknown test $T" >&2; exit 2 ;;
esac
"$LOAD" "${args[@]}" > "$OUT.load.jsonl" &
lg=$!
# shellcheck disable=SC2086
"$L/sample.sh" "$((secs + 5))" 5 $P load=$lg > "$OUT.sample.jsonl" || true
wait "$lg" || true
tail -n 20 "$W"/nginx-error.log "$W"/front.log "$W"/caddy.log 2>/dev/null > "$OUT.front.log" || true
"$L/stack.sh" stop "$W"
"$HERE/summary.sh" "$OUT"

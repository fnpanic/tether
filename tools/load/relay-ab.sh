#!/usr/bin/env bash
# hbbr CPU per relayed Gbit/s, two builds side by side (same load generator), no total cap:
#   tools/load/relay-ab.sh <bin-a> <bin-b> <out-prefix> [pairs ...]   (1 Mbit/s + 50 kbit/s per pair)
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"; A="$1"; B="$2"; OUT="$3"; shift 3
for p in "${@:-500 1000 1500}"; do
  for bin in "$A" "$B"; do
    n="$(basename "$(dirname "$bin")")-$(basename "$bin")"
    HBBR_ENV="TOTAL_BANDWIDTH=100000" "$HERE/relay-run.sh" "$bin" "$OUT-$n-$p" 45 url=ws://127.0.0.1:25119 pairs="$p" kbps=1000 up_kbps=50 fps=30 rate=500 threads=6 >/dev/null 2>&1
    # Steady state: samples 6..9 (after the ramp, before the end).
    cpu=$(sed -n '6,9p' "$OUT-$n-$p.sample.jsonl" | grep -o '"hbbr":{"cpu":[0-9.]*' | grep -o '[0-9.]*$' | awk '{s+=$1} END {printf "%.0f", s/NR}')
    lat=$(sed -n '7p' "$OUT-$n-$p.load.jsonl" | grep -o '"down_latency_ms":{[^}]*}')
    mb=$(sed -n '7p' "$OUT-$n-$p.load.jsonl" | grep -o '"down_mbit_s":[0-9.]*')
    echo "$p pairs  $bin  hbbr_cpu=${cpu}%  $mb  $lat"
  done
done

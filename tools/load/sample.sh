#!/usr/bin/env bash
# Samples CPU (% of one core), RSS (MB), open fds and threads of processes every <interval> s:
#   tools/load/sample.sh <seconds> <interval> name=pid [name=pid ...]   (one JSON line per sample)
set -uo pipefail
secs="$1"; iv="$2"; shift 2
hz=$(getconf CLK_TCK)
declare -A last
cpu() { awk '{print $14+$15}' "/proc/$1/stat" 2>/dev/null || echo 0; }
for kv in "$@"; do last[${kv%%=*}]=$(cpu "${kv#*=}"); done
end=$((SECONDS + secs))
while [ $SECONDS -lt $end ]; do
  sleep "$iv"
  line="{\"t\":$SECONDS"
  for kv in "$@"; do
    n=${kv%%=*}; p=${kv#*=}; c=$(cpu "$p")
    pct=$(awk -v a="$c" -v b="${last[$n]}" -v hz="$hz" -v iv="$iv" 'BEGIN{printf "%.1f", (a-b)/hz/iv*100}')
    last[$n]=$c
    rss=$(awk '/VmRSS/{printf "%.1f", $2/1024}' "/proc/$p/status" 2>/dev/null || echo 0)
    thr=$(awk '/Threads/{print $2}' "/proc/$p/status" 2>/dev/null || echo 0)
    fds=$(ls "/proc/$p/fd" 2>/dev/null | wc -l)
    line+=",\"$n\":{\"cpu\":$pct,\"rss_mb\":${rss:-0},\"fds\":$fds,\"thr\":${thr:-0}}"
  done
  echo "$line}"
done

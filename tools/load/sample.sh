#!/usr/bin/env bash
# Samples CPU (% of one core), RSS (MB), open fds and threads of processes every <interval> s
# (each with its direct children, e.g. nginx's workers):
#   tools/load/sample.sh <seconds> <interval> name=pid [name=pid ...]   (one JSON line per sample)
set -uo pipefail
secs="$1"; iv="$2"; shift 2
hz=$(getconf CLK_TCK)
declare -A last
tree() { echo "$1"; cat /proc/"$1"/task/*/children 2>/dev/null | tr ' ' '
' | grep .; }
cpu() { for q in $(tree "$1"); do awk '{print $14+$15}' "/proc/$q/stat" 2>/dev/null; done | awk '{s+=$1} END {print s+0}'; }
sum() { for q in $(tree "$1"); do awk -v k="$2" '$1==k {print $2}' "/proc/$q/status" 2>/dev/null; done | awk '{s+=$1} END {print s+0}'; }
for kv in "$@"; do last[${kv%%=*}]=$(cpu "${kv#*=}"); done
end=$((SECONDS + secs))
while [ $SECONDS -lt $end ]; do
  sleep "$iv"
  line="{\"t\":$SECONDS"
  for kv in "$@"; do
    n=${kv%%=*}; p=${kv#*=}; c=$(cpu "$p")
    pct=$(awk -v a="$c" -v b="${last[$n]}" -v hz="$hz" -v iv="$iv" 'BEGIN{printf "%.1f", (a-b)/hz/iv*100}')
    last[$n]=$c
    rss=$(sum "$p" VmRSS: | awk '{printf "%.1f", $1/1024}')
    # PSS: shared pages (code, nginx's shared zones) counted once across the processes.
    pss=$(for q in $(tree "$p"); do awk '/^Pss:/ {print $2}' "/proc/$q/smaps_rollup" 2>/dev/null; done | awk '{s+=$1} END {printf "%.1f", s/1024}')
    thr=$(sum "$p" Threads:)
    fds=$(for q in $(tree "$p"); do ls "/proc/$q/fd" 2>/dev/null; done | wc -l)
    line+=",\"$n\":{\"cpu\":$pct,\"rss_mb\":${rss:-0},\"pss_mb\":${pss:-0},\"fds\":$fds,\"thr\":${thr:-0}}"
  done
  echo "$line}"
done

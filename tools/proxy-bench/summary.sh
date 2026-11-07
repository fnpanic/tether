#!/usr/bin/env bash
# One line per result: steady-state CPU (% of one core, mean of the samples after the ramp) and
# peak RSS (MB) of the front and the others, plus the load generator's last full report.
#   tools/proxy-bench/summary.sh <result-prefix>...
for o in "$@"; do
  s="$o.sample.jsonl"; l="$o.load.jsonl"
  n=$(wc -l < "$s"); from=$(( n > 6 ? n / 2 : 1 ))
  cpu() { tail -n +"$from" "$s" | head -n $(( n - from )) | grep -o "\"$1\":{\"cpu\":[0-9.]*" | grep -o '[0-9.]*$' | awk '{s+=$1} END {if (NR) printf "%.0f", s/NR; else print "-"}'; }
  rss() { grep -o "\"$1\":{\"cpu\":[0-9.]*,\"rss_mb\":[0-9.]*,\"pss_mb\":[0-9.]*" "$s" | grep -o '[0-9.]*$' | sort -n | tail -1; }
  last=$(grep -v '"final"' "$l" | tail -n 2 | head -n 1)
  # Relay: a report from the middle of the run (all pairs streaming), not the closing one.
  grep -q down_mbit "$l" && last=$(sed -n "$(( ($(wc -l < "$l") + 1) / 2 ))p" "$l")
  printf '%s front cpu=%s%% pss=%sMB files=%s%% | client cpu=%s%% | hbbs %s%% %sMB | hbbr %s%% | api %s%% | %s\n' "$(basename "$o")" \
    "$(cpu front)" "$(rss front)" "$(cpu files)" "$(cpu client)" "$(cpu hbbs)" "$(rss hbbs)" "$(cpu hbbr)" "$(cpu api)" "$last"
done

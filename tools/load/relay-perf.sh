#!/usr/bin/env bash
# perf profile of hbbr under relay load (WSL perf path below; unstripped hbbr in $BIN for symbols,
# then addr2line -f -C -e $BIN/hbbr 0x<addr>): TB (TOTAL_BANDWIDTH) and PAIRS optional.
cd "$(dirname "$0")/../.."
P="${PERF:-$(ls /usr/lib/linux-tools/*/bin/perf 2>/dev/null | head -1)}"
HBBR_ENV="TOTAL_BANDWIDTH=${TB:-100000}" tools/load/relay-run.sh "${BIN:-$HOME/scale-build/bin}" /tmp/perf-relay 50 url=ws://127.0.0.1:25119 pairs=${PAIRS:-1500} kbps=1000 up_kbps=50 fps=30 rate=300 threads=6 2>/dev/null &
sleep 30
pid=$(pgrep -f -- "hbbr -p 25117" | head -1)
$P record -F 999 -p $pid -o /tmp/perf.data -- sleep 8 2>/dev/null >/dev/null
$P report -i /tmp/perf.data --sort dso --stdio 2>/dev/null | grep -v "^#" | grep . | head -8
$P report -i /tmp/perf.data --sort sym --stdio 2>/dev/null | grep -v "^#" | grep . | head -30
wait

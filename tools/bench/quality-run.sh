#!/bin/bash
# quality-run.sh static|scroll <label> <viewer> <outdir>: one picture-quality run against the VM (docs/performance.md,
# "Image quality vs AnyDesk"). The VM shows tools/bench/quality.html (static: ?cycle=1, scroll: ?mode=scroll).
#   viewer = tether:<cdp-port>   a Tether viewer (quality-view.mjs reads its session canvas, 1:1)
#          = anydesk             the AnyDesk Linux viewer on Xvfb :98 in WSL (xgrab.py)
# static: the page re-appears at :00 (VM clock) with new content; grabs at :02 and :40, VM bytes/CPU from :59
#         for 30 s, then the VM's own screenshot of the same minute's page (the reference).
# scroll: 60 s of barcode latency samples, VM bytes/CPU for 50 s, three grabs during the scroll.
# Env: REF (scroll: reference PNG of quality.html?mode=static, the VM's own screenshot), PY (python with opencv/scikit-image), WSLD (distro, default Ubuntu).
MODE=$1; LABEL=$2; VIEWER=$3; OUT=$4
D=$(cd "$(dirname "$0")" && pwd); WSLD=${WSLD:-Ubuntu}; PY=${PY:-python}; mkdir -p "$OUT"
# The VM side runs through the elevated broker's vm-perf-exec.ps1, which copies C:/vms/perf into the VM.
cp "$D/quality.html" "$D/qgrab.ps1" "$D/qperf.ps1" /c/vms/perf/
WOUT=$(MSYS_NO_PATHCONV=1 wsl -d "$WSLD" -e wslpath -a "$(cygpath -w "$OUT")")
WD=$(MSYS_NO_PATHCONV=1 wsl -d "$WSLD" -e wslpath -a "$(cygpath -w "$D")")
sec() { echo $((10#$(date +%S))); }
at() { while [ "$(sec)" -ne "$1" ]; do sleep 0.1; done; }
grab() {
  case $VIEWER in
    tether:*) node "$D/quality-view.mjs" "${VIEWER#tether:}" grab "$OUT/$LABEL-$1.png" > "$OUT/$LABEL-$1.stats" ;;
    anydesk) MSYS_NO_PATHCONV=1 wsl -d "$WSLD" -e env DISPLAY=:98 python3 "$WD/xgrab.py" shot "$WOUT/$LABEL-$1.png" ;;
  esac
}
vmperf() { # <seconds> <start second>
  echo "$1 $2" > /c/vms/perf/qperf-sec.txt
  powershell -NoProfile -ExecutionPolicy Bypass -File 'C:\vms\broker\run-elevated.ps1' vm-perf-exec.ps1 -Script qperf.ps1 > /dev/null
  cp /c/vms/perf-exec.log "$OUT/$LABEL.vmperf.json"
}
vmref() { # the VM's own screenshot (C:\vms\perf\qgrab.ps1)
  echo '-File C:\bench\perf\qgrab.ps1' > /c/vms/perf/cmd.txt; echo 20 > /c/vms/perf/wait.txt
  powershell -NoProfile -ExecutionPolicy Bypass -File 'C:\vms\broker\run-elevated.ps1' vm-perf-exec.ps1 -Script interactive.ps1 -Pull 'C:\bench\perf\qref.png' > /dev/null
  cp /c/vms/perf-out/qref.png "$OUT/$LABEL-ref.png"
}
if [ "$MODE" = static ]; then
  at 48; vmperf 30 59 & P=$!
  at 2; M0=$(date +%M); grab early
  wait $P; vmref
  # The reference must show the same minute's page (new content every minute, cover from :45).
  if [ "$(date +%M)" != "$M0" ] || [ "$(sec)" -ge 44 ]; then echo "$LABEL: reference too late, run discarded" >&2; exit 1; fi
  [ "$(sec)" -lt 40 ] && at 40; grab settled
  REF=$(cygpath -w "$OUT/$LABEL-ref.png")
  for g in early settled; do "$PY" "$D/quality-measure.py" "$REF" "$OUT/$LABEL-$g.png" --label "$LABEL-$g" --out "$OUT/quality.jsonl" --crops "$OUT/crops" > /dev/null; done
else
  vmperf 50 & P=$!
  case $VIEWER in
    tether:*) node "$D/stress-sampler.mjs" --port "${VIEWER#tether:}" --seconds 60 --every 15 --out "$OUT/$LABEL.lat.jsonl" > /dev/null & S=$! ;;
    anydesk) # sampled in WSL, time-stamped on Windows (udp-stamp.mjs): the WSL clock is not trusted
      MSYS_NO_PATHCONV=1 wsl -d "$WSLD" -e env DISPLAY=:98 python3 "$WD/xgrab.py" sample --seconds 62 --every 5 --serve 47124 --out "$WOUT/$LABEL.wsl.jsonl" > /dev/null & S=$!
      sleep 2; node "$D/udp-stamp.mjs" --ip "$(MSYS_NO_PATHCONV=1 wsl -d "$WSLD" -e hostname -I | cut -d' ' -f1)" --seconds 60 --out "$OUT/$LABEL.lat.jsonl" & S="$S $!" ;;
  esac
  sleep 15; grab s1; sleep 10; grab s2; sleep 10; grab s3
  wait $P $S
  for g in s1 s2 s3; do "$PY" "$D/quality-measure.py" "$REF" "$OUT/$LABEL-$g.png" --label "$LABEL-$g" --out "$OUT/quality.jsonl" > /dev/null; done
fi
cat "$OUT/$LABEL.vmperf.json"

#!/usr/bin/env bash
# Device status load test of tether-api (spec 9.6: health reporting must never endanger the
# relay's other work). Starts a local tether-api with a temp data dir and a throwaway admin key and
# pins DEVICES devices: ENROLL through POST /api/status/enroll, the rest written before the start
# (to the earlier status/keys.json format, which the relay migrates to keys.jsonl on start).
# Entries are realistic (4 products, 3 faults; every 20th device 8 and 10).
#
# Then: WARMUP s of heartbeat and policy probes (not measured); the RSS reference; BASELINE s of
# probes alone; SECS s of probes while every device posts at the pace the relay answers (`next`,
# 5 % full reports), 2 new devices enrol per second for the first 300 s, a roster member fetches
# GET /api/team/status every 30 s and /metrics is scraped every 15 s.
#
# PASS (exit 0) if, during the run, for POST /api/heartbeat and GET /api/policy: p99 <= 1.2 x
# baseline p99 + 2 ms, p99.9 <= 1.5 x baseline p99.9 + 5 ms, no request slower than 50 ms; no 5xx
# (no 503 busy) on any route; 429 <= 0.1 % of status posts; peak RSS below 48 MiB (MemoryMax=64M);
# RSS at the end within 10 % of the reference (taken after enrolment and the warm-up).
# STATUS_MAX_DEVICES is raised to DEVICES + 1000 for the run, so the new enrolments fit.
#
# Session signalling (hbbs) is not measured: it is a separate process that the status store does
# not touch (tools/load/hosts-run.sh measures it).
#
#   tools/load/status-run.sh [out-file]     (Linux; env: DEVICES=20000 SECS=600 WARMUP=60
#                                            BASELINE=60 ENROLL=100 API_BIN=… LOAD_BIN=…
#                                            SELF_CHECK=1: also the generator's own pauses;
#                                            STATUS_TRACE_LOCK=1: tether-api logs state-lock holds over 20 ms)
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"; ROOT="$(cd "$HERE/../.." && pwd)"
OUT="${1:-/tmp/status-load.txt}"
DEVICES="${DEVICES:-20000}"; SECS="${SECS:-600}"; WARMUP="${WARMUP:-60}"; BASELINE="${BASELINE:-60}"; ENROLL="${ENROLL:-100}"
T="${CARGO_TARGET_DIR:-${TMPDIR:-/tmp}/tether-status-load}"
if [ -z "${API_BIN:-}" ]; then
  CARGO_TARGET_DIR="$T/api" cargo build --release --quiet --manifest-path "$ROOT/server/api/Cargo.toml"
  API_BIN="$T/api/release/tether-api"
fi
if [ -z "${LOAD_BIN:-}" ]; then
  CARGO_TARGET_DIR="$T/load" cargo build --release --quiet --manifest-path "$HERE/status-load/Cargo.toml"
  LOAD_BIN="$T/load/release/status-load"
fi
ulimit -n 65536 2>/dev/null || true

W="$(mktemp -d /tmp/status-load.XXXXXX)"; API_PID=""
cleanup() { [ -n "$API_PID" ] && kill "$API_PID" 2>/dev/null; wait 2>/dev/null; rm -rf "$W"; }
trap cleanup EXIT
PUB="$("$LOAD_BIN" keygen "$W/admin.key")"
"$LOAD_BIN" prep "$W/data" devices="$DEVICES" enroll="$ENROLL"
ADMIN_PUB_KEY="$PUB" RELAY_HOST=relay.test STATUS_MAX_DEVICES=$((DEVICES + 1000)) STATUS_TRACE_LOCK="${STATUS_TRACE_LOCK:-}" \
  "$API_BIN" --data "$W/data" --listen 127.0.0.1:0 > "$W/api.out" 2> "$W/api.log" &
API_PID=$!
for _ in $(seq 300); do grep -q '^listening on' "$W/api.out" 2>/dev/null && break; sleep 0.1; done
ADDR="$(sed -n 's/^listening on //p' "$W/api.out")"
[ -n "$ADDR" ] || { cat "$W/api.log"; echo "tether-api did not start"; exit 1; }
echo "tether-api pid $API_PID on $ADDR; $DEVICES devices, warm-up $WARMUP s, baseline $BASELINE s, run $SECS s" | tee "$OUT"
set +e
"$LOAD_BIN" run addr="$ADDR" key="$W/admin.key" pid="$API_PID" devices="$DEVICES" enroll="$ENROLL" \
  seconds="$SECS" warmup="$WARMUP" baseline="$BASELINE" ${SELF_CHECK:+--self-check} | tee -a "$OUT"
rc=${PIPESTATUS[0]}
grep -v -e '^warning' -e '^status: ' -e '^lock held' "$W/api.log" | head -20 | sed 's/^/tether-api: /' | tee -a "$OUT"
if [ -n "${STATUS_TRACE_LOCK:-}" ]; then
  echo "state-lock holds over 20 ms: $(grep -c '^lock held' "$W/api.log" || true)" | tee -a "$OUT"
  grep '^lock held' "$W/api.log" | sort -k3 -n -r | head -10 | sed 's/^/  /' | tee -a "$OUT"
fi
exit "$rc"

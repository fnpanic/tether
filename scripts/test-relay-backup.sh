#!/usr/bin/env bash
# Backup relay failover end to end, on localhost with the binaries from scripts/build-relay.sh:
# hbbs with a primary and a backup hbbr; primary stopped -> new sessions get the backup,
# primary back -> new sessions get the primary again.
#   scripts/test-relay-backup.sh [build-dir]
set -euo pipefail
OUT="${1:-$HOME/ha-build}"
BIN="$OUT/bin"
W="$(mktemp -d)"
cd "$W"
pids=()
trap 'kill "${pids[@]}" 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$W"' EXIT
# Ports away from the defaults so a running relay on this machine does not interfere.
P=ws://127.0.0.1:41219 B=ws://127.0.0.1:41319
start_primary() { HEALTH_PORT=41221 "$BIN/hbbr" -p 41217 -k _ >primary.log 2>&1 & primary=$!; pids+=($primary); }
HEALTH_PORT=0 "$BIN/hbbs" -p 41116 -k _ \
  -r "$P|http://127.0.0.1:41221/readyz,$B|http://127.0.0.1:41321/readyz|backup" >hbbs.log 2>&1 &
pids+=($!)
for _ in $(seq 50); do [ -s id_ed25519.pub ] && break; sleep 0.1; done
KEY="$(cat id_ed25519.pub)"
start_primary
HEALTH_PORT=41321 "$BIN/hbbr" -p 41317 -k _ >backup.log 2>&1 &
pids+=($!)
sleep 4  # first health round

relay_of_new_session() {
  "$BIN/relay-harness" hbbs=ws://127.0.0.1:41118 key="$KEY" sessions=1 seconds=2 |
    tr -d ' \n' | grep -o '"relays":\["[^"]*"' | cut -d'"' -f4
}
expect() {
  local got; got="$(relay_of_new_session)"
  if [ "$got" != "$1" ]; then
    echo "FAIL ($2): expected $1, got '${got}'"; tail -n 30 hbbs.log; exit 1
  fi
  echo "ok: $2 -> $got"
}

expect "$P" "both healthy: primary"
kill "$primary"; wait "$primary" 2>/dev/null || true
sleep 7  # health every 3 s, 2.5 s timeout
expect "$B" "primary stopped: backup"
grep -q "relay failover: no healthy primary relay, using backups" hbbs.log || { echo "FAIL: no failover log"; exit 1; }
start_primary
sleep 7
expect "$P" "primary back: primary"
grep -q "relay failover: primary relay healthy again" hbbs.log || { echo "FAIL: no recovery log"; exit 1; }
echo "relay backup failover: all passed"

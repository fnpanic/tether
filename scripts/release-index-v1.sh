#!/usr/bin/env bash
# Back to the single release index releases.json (docs/updates.md, "Two index files"):
#   1. Tether Manage stops writing releases-v2.json (marker file release-index-v1-only in its folder),
#   2. every relay deletes download/<channel>/releases-v2.json (tether-api answers 404 for a
#      deleted index at once), and the script checks over HTTPS that it is gone.
# Apps from TetherQS 0.5.31 / Tether 0.5.33 then find no v2 and use releases.json, as older builds
# always do: macOS updates come from the relays again, nothing else changes. The .xz files stay
# (Clean up removes them with their version).
#
#   bash scripts/release-index-v1.sh          # switch to releases.json only
#   bash scripts/release-index-v1.sh --on     # write releases-v2.json again from the next publish
#
# Env: MANAGE_DIR (default %APPDATA%\tether-manage), RELAY_SSH (targets, default both production
# relays), SSH_KEY (default ~/.ssh/tether_oracle). For a test against other relays: RELAY_EXEC
# (runs "<target> <shell command>", default ssh), RELAY_DATA (data folder or glob, default
# /var/lib/tether-api), SUDO (default sudo), RELAY_HTTPS (base URLs to check, default Manage's relays).
set -euo pipefail
MANAGE_DIR=${MANAGE_DIR:-${TETHER_MANAGE_DIR:-$APPDATA/tether-manage}}
RELAY_SSH=${RELAY_SSH:-"opc@158.101.205.185 opc@92.5.231.71"}
SSH_KEY=${SSH_KEY:-$HOME/.ssh/tether_oracle}
RELAY_EXEC=${RELAY_EXEC:-"ssh -o LogLevel=ERROR -i $SSH_KEY"}
RELAY_DATA=${RELAY_DATA:-/var/lib/tether-api}
SUDO=${SUDO-sudo}
MARKER="$MANAGE_DIR/release-index-v1-only"
[ -d "$MANAGE_DIR" ] || { echo "no Tether Manage folder at $MANAGE_DIR (set MANAGE_DIR)" >&2; exit 1; }

if [ "${1:-}" = --on ]; then
  rm -f "$MARKER"
  echo "releases-v2.json is on again: the next publish in Tether Manage writes it (xz copies for the"
  echo "release it publishes; earlier releases get theirs when they are published again)."
  exit 0
fi
[ -z "${1:-}" ] || { echo "usage: $0 [--on]" >&2; exit 2; }

: >"$MARKER"
echo "Tether Manage: writes releases.json only from now on ($MARKER)"
# Runs on each relay: delete every channel's releases-v2.json.
remote='set -e
n=0
for f in RELAY_DATA/download/*/releases-v2.json; do
  [ -e "$f" ] || continue
  SUDO rm -f "$f"; echo "deleted $f"; n=$((n+1))
done
echo "$n file(s) deleted"'
remote=${remote//RELAY_DATA/$RELAY_DATA}
remote=${remote//SUDO/$SUDO}
for t in $RELAY_SSH; do
  echo "== $t"
  $RELAY_EXEC "$t" "$remote"
done
# Every relay must answer 404 for it now (the channels Manage publishes to: stable and test).
host_list=${RELAY_HTTPS:-$(python -c "import json,sys;print(' '.join(s.strip() for s in json.load(open(sys.argv[1])).get('server','https://tether-desk.duckdns.org').split(',') if s.strip()))" "$MANAGE_DIR/settings.json" 2>/dev/null || echo https://tether-desk.duckdns.org)}
fail=0
for h in $host_list; do
  for ch in stable test; do
    code=$(curl -s -o /dev/null -m 15 -w '%{http_code}' "$h/download/$ch/releases-v2.json" || true)
    v1=$(curl -s -o /dev/null -m 15 -w '%{http_code}' "$h/download/$ch/releases.json" || true)
    echo "$h $ch: releases-v2.json $code, releases.json $v1"
    [ "$code" = 404 ] || fail=1
  done
done
if [ $fail = 0 ]; then echo "done: single releases.json on every relay"; else echo "releases-v2.json still answers somewhere (see above)" >&2; exit 1; fi

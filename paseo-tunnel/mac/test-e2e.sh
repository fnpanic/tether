#!/bin/bash
# End-to-end test on the Mac behind Zscaler, without touching the user's Paseo setup:
# a throwaway Paseo daemon (own PASEO_HOME, port 6799) is started twice, without and with the CA
# bundle from paseo-corp-ca.sh, and must
#   - fail to reach the relay / the PC without the CA (negative control: proves Zscaler is the cause),
#   - with the CA: register its own relay link, hold the PC as a connected host, and list PC agents.
# Usage: bash test-e2e.sh <file-with-relay-uri> [label]
set -uo pipefail
cd "$(dirname "$0")"
URI=$(cat "${1:?relay uri file}"); LABEL=${2:-GameMachine}
CLI=/Applications/Paseo.app/Contents/Resources/bin/paseo
SID=$(sed -E 's#^relay://[^/]+/([^?]+).*#\1#' <<<"$URI")
T=$PWD/t; PEM=$T/corp-ca.pem
fail=0; check() { if eval "$2"; then echo "PASS $1"; else echo "FAIL $1"; fail=1; fi; }
to() { perl -e 'alarm shift; exec @ARGV' "$@"; }

host_state() { # prints the PC host entry's state fields (from host ls --json)
  /usr/bin/python3 -c '
import json, sys
try: hs = json.loads(sys.argv[1])
except Exception: print("no-json"); sys.exit()
hs = hs if isinstance(hs, list) else hs.get("hosts", [])
h = [h for h in hs if sys.argv[2] in json.dumps(h)]
print(json.dumps(h[0]) if h else "absent")' "$1" "$SID"
}
connected() { grep -qiE '"(status|state|connection(State|Status)?)": *"(connected|online|ready)"' <<<"$1"; }

run_daemon() { # $1 = home, $2 = CA file or ""; starts a temp daemon, adds the PC, prints host ls
  local H=$1; rm -rf "$H"; mkdir -p "$H"
  printf '{"version":1,"daemon":{"listen":"127.0.0.1:6799","relay":{"enabled":true}}}\n' > "$H/config.json"
  ( [ -n "$2" ] && export NODE_EXTRA_CA_CERTS=$2; export PASEO_HOME=$H
    to 120 "$CLI" daemon start --home "$H" --timeout 90 >"$H/start.out" 2>&1
    to 60 "$CLI" --home "$H" host add "$URI" --label "$LABEL" --json >"$H/add.out" 2>&1
    local out=""; for _ in $(seq 1 15); do
      out=$(to 30 "$CLI" --home "$H" host ls --json 2>/dev/null); connected "$out" && break; sleep 3; done
    echo "$out" > "$H/hosts.json"
    to 30 "$CLI" daemon stop --home "$H" --force >/dev/null 2>&1 )
}

mkdir -p "$T"
PASEO_CORP_CA_PEM=$PEM bash paseo-corp-ca.sh export
echo "== tool doctor (read-only)"; PASEO_CORP_CA_PEM=$PEM bash paseo-corp-ca.sh doctor "$URI" 2>/dev/null | tee "$T/doctor.txt"
check "tls to relay fails without CA" 'grep -q "tls w/o CA:   error" "$T/doctor.txt"'
check "tls to relay ok with CA (issuer Zscaler)" 'grep -q "tls with CA:  ok issuer=Zscaler" "$T/doctor.txt"'
check "mac -> relay -> pc via CLI with CA" 'grep -q "e2e relay:    OK" "$T/doctor.txt"'

echo "== temp daemon WITHOUT CA"; run_daemon "$T/h-noca" ""
echo "start: $(tail -2 "$T/h-noca/start.out")"; echo "host: $(host_state "$(cat "$T/h-noca/hosts.json")" | cut -c1-600)"
grep -E 'relay_control_|issuer' "$T/h-noca/daemon.log" 2>/dev/null | tail -2 | cut -c1-300
check "negative control: no CA -> pc host not connected" '! connected "$(cat "$T/h-noca/hosts.json")"'

echo "== temp daemon WITH CA"; run_daemon "$T/h-ca" "$PEM"
echo "start: $(tail -2 "$T/h-ca/start.out")"; echo "add: $(head -c 600 "$T/h-ca/add.out")"
echo "host: $(host_state "$(cat "$T/h-ca/hosts.json")" | cut -c1-800)"
grep -E 'relay_control_' "$T/h-ca/daemon.log" 2>/dev/null | tail -2 | cut -c1-300
check "with CA: temp daemon's relay link connects" 'grep -q relay_control_connected "$T/h-ca/daemon.log"'
check "with CA: pc registered and connected on the mac daemon" 'connected "$(cat "$T/h-ca/hosts.json")"'
check "with CA: list pc agents through the relay" 'NODE_EXTRA_CA_CERTS=$PEM to 45 "$CLI" --host "$URI" ls -a 2>&1 | grep -q "AGENT ID"'

echo; [ $fail = 0 ] && echo "ALL PASS" || echo "SOME FAILED"; exit $fail

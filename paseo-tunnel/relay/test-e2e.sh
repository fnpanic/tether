#!/bin/bash
# End-to-end check of the self-hosted relay without touching the user's Paseo setup: a throwaway
# daemon (own PASEO_HOME, port 6799) registers on the relay, then a CLI client reaches it only
# through the relay link. Windows (Git Bash) with Paseo Desktop installed. Usage: bash test-e2e.sh
set -u
RELAY=${RELAY:-tether-desk.duckdns.org:443}
R="$LOCALAPPDATA/Programs/Paseo/resources"
paseo() { ELECTRON_RUN_AS_NODE=1 PASEO_NODE_ENV=production "$R/../Paseo.exe" --disable-warning=DEP0040 \
  "$R/app.asar.unpacked/dist/daemon/node-entrypoint-runner.js" node-script "$R/app.asar/node_modules/@getpaseo/cli/dist/index.js" "$@"; }
H=$(mktemp -d); FAIL=0
check() { if eval "$2"; then echo "PASS $1"; else echo "FAIL $1"; FAIL=1; fi; }
trap 'paseo daemon stop --home "$H" >/dev/null 2>&1' EXIT

printf '{"version":1,"daemon":{"listen":"127.0.0.1:6799","relay":{"enabled":true,"endpoint":"%s","publicEndpoint":"%s","useTls":true}}}\n' "$RELAY" "$RELAY" > "$H/config.json"
PASEO_HOME=$H paseo daemon start --home "$H" --timeout 90 > "$H/start.out" 2>&1
URI=$(PASEO_HOME=$H paseo daemon pair --home "$H" --json 2>/dev/null | python -c "import json,sys;print(json.load(sys.stdin)['connectionUri'],end='')")
echo "pairing link: ${URI%%\?*}?…"
check "pairing link points at the own relay with TLS" '[[ "$URI" == "relay://$RELAY/"* && "$URI" == *ssl=true* ]]'
sleep 3
check "daemon control link connected to the relay" 'grep -rq relay_control_connected "$H"/*.log "$H"/logs 2>/dev/null'
check "client -> own relay -> daemon (list agents)" 'paseo --host "$URI" ls -a --json > "$H/ls.out" 2>&1 && python -c "import json,sys;json.load(open(sys.argv[1]))" "$H/ls.out"'
[ $FAIL = 0 ] && echo "ALL PASS" || { echo "--- logs in $H"; tail -5 "$H/ls.out"; exit 1; }

#!/bin/bash
# Build-runner job (user "tether", submit.sh --diag-upload): a real diagnostics upload from the Mac.
# Debug builds of Tether and TetherQS (only debug builds take TETHER_DEV_API), a local tether-api
# on 127.0.0.1 with a throw-away admin key made here, a ticket per app opened with a signed admin
# request (crates/tether-trust example admin_request + curl), then each app's `--diag-upload`
# (the same code as Send to support) with a dead relay listed first. The relay must list the upload
# and serve a zip that opens and holds diagnostics.json; a second upload of a used code must be
# refused. The key, the relay's data and the apps' HOME are deleted at the end. No signing secrets,
# nothing installed, /Applications untouched.
#
# work/: diagupload.sh src.tgz     secrets/: keys.env      cache: $JOB_CACHE/src (shared with build.sh)
set -uo pipefail
B=$JOB_CACHE
log() { echo "[$(date +%H:%M:%S)] $*"; }
fail() { log "FAIL $*"; exit 1; }
log "start (job $JOB_ID)"
mkdir -p "$B/src"
rm -rf "$B/src/vendor" && tar -xzf "$JOB_WORK/src.tgz" -C "$B/src" || fail "untar"
rm -f "$JOB_WORK/src.tgz"
# shellcheck source=/dev/null
. "$JOB_SECRETS/keys.env"
rm -f "$JOB_SECRETS/keys.env"
export TETHER_SERVER TETHER_PUB_KEY TETHER_ADMIN_PUB_KEY TETHER_UPDATER_PUB_KEY
eval "$(/opt/homebrew/bin/brew shellenv)"
# shellcheck source=/dev/null
. "$HOME/.cargo/env"
export LIBCLANG_PATH="$(brew --prefix llvm)/lib" PATH="$B/src/.tools/bin:$PATH" VCPKG_ROOT="$B/src/.tools/vcpkg" MACOSX_DEPLOYMENT_TARGET=11.0
cd "$B/src" || fail "src"
bash scripts/bootstrap.sh || fail "bootstrap"
# The UIs are not needed for a command line upload, but the build wants the folders.
rm -rf apps/desktop/dist apps/quick/dist
cp -R prebuilt/desktop-dist apps/desktop/dist && cp -R prebuilt/quick-dist apps/quick/dist || fail "prebuilt UI"

log "build tether-api, admin_request, Tether and TetherQS (debug)"
(cd server/api && cargo build -q) || fail "tether-api build"
(cd crates/tether-trust && cargo build -q --example admin_request) || fail "admin_request build"
(cd apps/desktop/src-tauri && cargo build -q --target aarch64-apple-darwin) || fail "Tether debug build"
(cd apps/quick/src-tauri && cargo build -q --target aarch64-apple-darwin) || fail "TetherQS debug build"
API=$B/src/server/api/target/debug/tether-api
SIGN=$B/src/crates/tether-trust/target/debug/examples/admin_request
TECH=$B/src/apps/desktop/src-tauri/target/aarch64-apple-darwin/debug/tether
QS=$B/src/apps/quick/src-tauri/target/aarch64-apple-darwin/debug/TetherQS
file "$TECH" "$QS" | sed 's/^/  /'

T=$(mktemp -d)
KEY=$T/test-admin.key
RELAY=
cleanup() {
  [ -n "$RELAY" ] && kill "$RELAY" 2>/dev/null
  rm -rf "$T"
  [ -e "$KEY" ] && log "key NOT deleted" || log "throw-away admin key, relay data and app HOME deleted"
}
trap cleanup EXIT
PUB=$("$SIGN" "$KEY" --pub) || fail "test key"
log "throw-away admin key $PUB (this job only)"
PORT=$((24000 + RANDOM % 1000))
BASE=http://127.0.0.1:$PORT
mkdir -p "$T/data/diag" "$T/home"
ADMIN_PUB_KEY=$PUB "$API" --data "$T/data" --listen "127.0.0.1:$PORT" >"$T/relay.log" 2>&1 &
RELAY=$!
for _ in $(seq 1 40); do grep -q listening "$T/relay.log" && break; sleep 0.25; done
grep -q listening "$T/relay.log" || { cat "$T/relay.log"; fail "relay did not start"; }
for _ in $(seq 1 40); do curl -s -o /dev/null "$BASE/api/policy" && break; sleep 0.25; done
log "tether-api $(sed -n 1p "$T/relay.log") (pid $RELAY)"

# admin <METHOD> <path> [<json body>] [<out file>]: signed request, prints the HTTP status.
admin() {
  local sig a
  sig=$("$SIGN" "$KEY" "$1" "$2" "${3:-}") || return 1
  a=(-s -o "${4:-/dev/null}" -w '%{http_code}' -X "$1" -H "Tether-Sig: $sig")
  [ -n "${3:-}" ] && a+=(-H 'Content-Type: application/json' --data-binary "$3")
  curl "${a[@]}" "$BASE$2"
}
fails=0
check() { # name ok? detail
  if [ "$2" = 1 ]; then log "PASS $1 $3"; else log "FAIL $1 $3"; fails=$((fails + 1)); fi
}
for which in tech qs; do
  [ $which = tech ] && bin=$TECH || bin=$QS
  st=$(admin POST /api/admin/diag-ticket "{\"note\":\"mac job $JOB_ID $which\"}" "$T/ticket.json")
  code=$("$JOB_PYTHON" -c 'import json,sys;print(json.load(open(sys.argv[1]))["ticket"])' "$T/ticket.json" 2>/dev/null)
  check "$which: signed admin request opens a ticket" "$([ "$st" = 200 ] && [ -n "$code" ] && echo 1)" "(HTTP $st, code $code)"
  # A dead relay first: the app must move on to the next one.
  out=$(HOME=$T/home TETHER_PROFILE=diagupload TETHER_DEV_API="http://127.0.0.1:9,$BASE" "$bin" --diag-upload "$code" 2>&1)
  rc=$?
  check "$which: --diag-upload (dead relay first, then the test relay)" "$([ $rc = 0 ] && echo 1)" "(exit $rc: $(echo "$out" | tail -1))"
  st=$(admin GET /api/admin/diag "" "$T/list.json")
  row=$("$JOB_PYTHON" -c 'import json,sys;t=[x for x in json.load(open(sys.argv[1])) if x["ticket"]==sys.argv[2]][0];print(t.get("state"),t.get("size"))' "$T/list.json" "$code" 2>/dev/null)
  check "$which: relay lists the upload" "$([ "$st" = 200 ] && [ "${row%% *}" = uploaded ] && echo 1)" "($row)"
  st=$(admin GET "/api/admin/diag/$code" "" "$T/$which.zip")
  z=$("$JOB_PYTHON" - "$T/$which.zip" <<'PY'
import json, sys, zipfile
z = zipfile.ZipFile(sys.argv[1])
assert z.testzip() is None
names = z.namelist()
info = json.loads(z.read("diagnostics.json"))
print(f"{len(names)} files, diagnostics.json version {info.get('version')} os {info.get('os', '')[:40]}")
PY
  )
  check "$which: the stored upload is a valid zip with diagnostics.json" "$([ "$st" = 200 ] && [ -n "$z" ] && echo 1)" "(HTTP $st, $(wc -c <"$T/$which.zip" | tr -d ' ') bytes: $z)"
  out=$(HOME=$T/home TETHER_PROFILE=diagupload TETHER_DEV_API="$BASE" "$bin" --diag-upload "$code" 2>&1)
  rc=$?
  check "$which: a used code is refused" "$([ $rc != 0 ] && echo "$out" | grep -q 'not valid' && echo 1)" "(exit $rc: $(echo "$out" | tail -1))"
done
log "relay log:"
sed 's/^/  /' "$T/relay.log" | tail -20
[ $fails = 0 ] && log "DIAG UPLOAD OK" || fail "$fails checks failed"

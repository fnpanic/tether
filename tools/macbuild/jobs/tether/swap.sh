#!/bin/bash
# On-unlock step of the Tether build job (the runner starts it once the screen is unlocked):
# replace /Applications/Tether.app with $1, and put the old one back if the new one does not come up.
# Renames instead of rm -rf, so a refused change (macOS App Management) leaves the old app intact.
# No AppleScript (it would need Automation consent): quit = SIGTERM.
set -u
NEW=$1
A=/Applications/Tether.app
STAGE=/Applications/.Tether.app.new
PREV=/Applications/.Tether.app.prev
PAT='^/Applications/\.?Tether\.app[^/]*/Contents/MacOS/tether'
log() { echo "[$(date +%H:%M:%S)] $*"; }
# This user's processes only: the root daemon of unattended access restarts itself.
running() { pgrep -U "$(id -u)" -f "$PAT" >/dev/null; }
# By pid: with unattended access launchd starts a new agent at once, so "no process left" never comes
# true (2026-11-23: 20 s wait, then KILL of the agent that already ran the new build).
stop() {
  local old
  old=$(pgrep -U "$(id -u)" -f "$PAT") || return 0
  # shellcheck disable=SC2086
  kill -TERM $old 2>/dev/null
  # shellcheck disable=SC2086
  for _ in $(seq 20); do kill -0 $old 2>/dev/null || return 0; sleep 1; done
  log "still running: KILL"
  # shellcheck disable=SC2086
  kill -KILL $old 2>/dev/null; sleep 1
}
# Up = running, the host started its rendezvous connection, and still running 30 s later.
up() {
  touch "$JOB_DIR/.launch"
  # LaunchServices can refuse the first open right after the bundle was replaced (error -600): retry once.
  open "$A" || { sleep 3; open "$A"; }
  for _ in $(seq 60); do
    sleep 1
    if running && find ~/Library/Logs/Tether -name '*.log' -newer "$JOB_DIR/.launch" -exec grep -l -E "start rendezvous mediator|start tcp:|keep_alive" {} + 2>/dev/null | grep -q .; then
      sleep 30
      running && return 0
      log "exited after start"; return 1
    fi
  done
  log "no rendezvous start within 60 s"; return 1
}
log "swap start: $NEW"
# Unattended access (docs/unattended.md, macOS): the user's launchd agent (--server) is ended by `stop` and
# started again by launchd from the new bundle; the root daemon (--service) is no host and restarts itself
# once the bundle changed. Only a root-owned host (--server, at the login window) cannot be stopped from
# here: two hosts with one ID would push each other out at the relay.
if pgrep -U 0 -f "$PAT --server" >/dev/null; then
  log "SKIP: a root-owned Tether host is running (login window); this user cannot stop it."
  exit 1
fi
codesign --verify --deep --strict "$NEW" || { log "FAIL new app signature"; exit 1; }
new=$(codesign -d -r- "$NEW" 2>&1 | grep designated)
old=$(codesign -d -r- "$A" 2>&1 | grep designated)
[ -n "$new" ] && [ "$new" = "$old" ] || { log "FAIL requirement differs: $new / $old"; exit 1; }
rm -rf "$STAGE" "$PREV"
ditto "$NEW" "$STAGE" || { log "FAIL copy into /Applications"; rm -rf "$STAGE"; exit 1; }
if ! mv "$A" "$PREV"; then
  log "FAIL cannot rename $A (System Settings > Privacy & Security > App Management: allow python3/bash?); nothing changed"
  rm -rf "$STAGE"; exit 1
fi
mv "$STAGE" "$A" || { log "FAIL rename new app; restoring"; mv "$PREV" "$A"; exit 1; }
stop # the old process still runs from $PREV
if up; then
  rm -rf "$PREV"
  if pgrep -U 0 -f "$PAT --server" >/dev/null; then log "RESTART REQUIRED: a root-owned Tether host appeared next to the new build"; exit 1; fi
  log "SWAP OK: new build running"
  exit 0
fi
log "RESTORE previous build"
stop
rm -rf "$STAGE" && mv "$A" "$STAGE" && mv "$PREV" "$A"
open "$A"
sleep 20
if running; then log "restored and running"; else log "RESTORE FAILED: not running"; fi
rm -rf "$STAGE"
exit 1

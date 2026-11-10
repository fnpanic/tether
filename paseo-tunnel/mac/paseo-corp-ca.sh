#!/bin/bash
# paseo-corp-ca: make Paseo (Electron/Node) on macOS trust the corporate TLS-inspection CA (Zscaler & co).
#
# Why: Zscaler re-signs every TLS connection (also wss://relay.paseo.sh) with "Zscaler Root CA", which
# MDM installs into the System keychain. Safari/curl trust it; Node/Electron use their own CA list and
# fail with "unable to get local issuer certificate", so the Paseo daemon can neither reach the relay
# nor connect to other hosts (your PC). Fix: NODE_EXTRA_CA_CERTS = the System keychain's certificates.
#
#   paseo-corp-ca.sh install [--restart]   export CAs, verify they fix TLS, persist (LaunchAgent + ~/.zprofile)
#   paseo-corp-ca.sh export                only write the CA bundle (PASEO_CORP_CA_PEM overrides the path)
#   paseo-corp-ca.sh session               export + restart Paseo with the CA, nothing persisted (until next Paseo restart)
#   paseo-corp-ca.sh refresh               re-export CAs + launchctl setenv (what the LaunchAgent runs)
#   paseo-corp-ca.sh restart               restart Paseo app + daemon so they pick up the CA (skips if agents run)
#   paseo-corp-ca.sh doctor [relay://...]  show TLS with/without the CA, env of the running daemon, optional e2e
#   paseo-corp-ca.sh uninstall             undo everything
set -euo pipefail

APP=${PASEO_APP:-/Applications/Paseo.app}
EL="$APP/Contents/MacOS/Paseo"
CLI="$APP/Contents/Resources/bin/paseo"
DIR="$HOME/.paseo"
PEM=${PASEO_CORP_CA_PEM:-$DIR/corp-ca.pem}
LABEL=local.paseo-corp-ca
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
SELF_DST="$DIR/bin/paseo-corp-ca.sh"
ZPROFILE="$HOME/.zprofile"
MARK="# paseo-corp-ca"
PROBE_HOST=${PASEO_PROBE_HOST:-relay.paseo.sh}
SYSTEM_KC=/Library/Keychains/System.keychain

log() { echo "[paseo-corp-ca] $*"; }
to() { perl -e 'alarm shift; exec @ARGV' "$@"; } # macOS has no timeout(1)

export_pem() {
  # Admin/MDM-installed roots (Zscaler, Netskope, Palo Alto, ...) live in the System keychain.
  local tmp; tmp=$(mktemp)
  security find-certificate -a -p "$SYSTEM_KC" > "$tmp"
  local n; n=$(grep -c "BEGIN CERTIFICATE" "$tmp" || true)
  [ "$n" -gt 0 ] || { rm -f "$tmp"; log "no certificates in $SYSTEM_KC"; return 1; }
  mkdir -p "$(dirname "$PEM")"; chmod 644 "$tmp"; mv "$tmp" "$PEM"
  log "exported $n certificate(s) from the System keychain to $PEM"
}

# probe <ca-file|""> -> prints "ok issuer=<O>" or "error <msg>"; exit 0 only on a verified handshake
probe() {
  local ca=$1
  env -u NODE_EXTRA_CA_CERTS ELECTRON_RUN_AS_NODE=1 ${ca:+NODE_EXTRA_CA_CERTS="$ca"} perl -e 'alarm shift; exec @ARGV' 20 "$EL" -e '
    const h = process.argv[1], tls = require("tls");
    const s = tls.connect({ host: h, port: 443, servername: h }, () => {
      const c = s.getPeerCertificate(); console.log("ok issuer=" + ((c.issuer && (c.issuer.O || c.issuer.CN)) || "?")); s.end(); });
    s.on("error", (e) => { console.log("error " + e.message); process.exit(1); });' "$PROBE_HOST" 2>/dev/null
}

zprofile_line() { echo "[ -f \"\$HOME/.paseo/corp-ca.pem\" ] && export NODE_EXTRA_CA_CERTS=\"\$HOME/.paseo/corp-ca.pem\" $MARK"; }

refresh() {
  export_pem
  launchctl setenv NODE_EXTRA_CA_CERTS "$PEM"
  log "launchctl setenv NODE_EXTRA_CA_CERTS=$PEM (apps started from now on inherit it)"
}

running_agents() { # count of running agents on the local daemon; 0 if the daemon is down
  NODE_EXTRA_CA_CERTS="$PEM" to 20 "$CLI" ls --json 2>/dev/null | /usr/bin/python3 -c '
import json, sys
try: d = json.load(sys.stdin)
except Exception: print(0); sys.exit()
items = d if isinstance(d, list) else d.get("agents", d.get("data", []))
print(sum(1 for a in items if str(a.get("status", "")).lower() == "running"))' 2>/dev/null || echo 0
}

restart() {
  local n; n=$(running_agents)
  if [ "${FORCE:-0}" != 1 ] && [ "$n" -gt 0 ]; then
    log "NOT restarting: $n agent(s) running on this Mac's daemon. Quit Paseo yourself later, or FORCE=1 $0 restart"
    return 0
  fi
  log "restarting Paseo (app + desktop-managed daemon)"
  pkill -TERM -x Paseo 2>/dev/null || true
  for _ in $(seq 1 20); do pgrep -x Paseo >/dev/null || break; sleep 0.5; done
  NODE_EXTRA_CA_CERTS="$PEM" to 30 "$CLI" daemon stop >/dev/null 2>&1 || true
  open --env NODE_EXTRA_CA_CERTS="$PEM" -a "$APP" # open(1) does not pass the caller env; --env does
  for _ in $(seq 1 60); do
    NODE_EXTRA_CA_CERTS="$PEM" to 10 "$CLI" daemon status --json 2>/dev/null | grep -q '"localDaemon": "running"' && { log "daemon is back"; return 0; }
    sleep 2
  done
  log "WARNING: daemon not running 120 s after restart; open Paseo manually"
}

install() {
  [ -x "$EL" ] || { log "Paseo not found at $APP (set PASEO_APP)"; exit 1; }
  export_pem
  local before after
  before=$(probe "" || true); after=$(probe "$PEM" || true)
  log "TLS to $PROBE_HOST without CA: $before"
  log "TLS to $PROBE_HOST with CA:    $after"
  case $after in ok*) ;; *) log "the exported CAs do not fix TLS to $PROBE_HOST; nothing changed"; exit 1 ;; esac

  mkdir -p "$DIR/bin" "$(dirname "$PLIST")"
  cp "$0" "$SELF_DST"; chmod 755 "$SELF_DST"
  cat > "$PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>$LABEL</string>
  <key>ProgramArguments</key><array><string>/bin/bash</string><string>$SELF_DST</string><string>refresh</string></array>
  <key>RunAtLoad</key><true/>
  <key>StartInterval</key><integer>86400</integer>
  <key>StandardOutPath</key><string>$DIR/corp-ca.log</string>
  <key>StandardErrorPath</key><string>$DIR/corp-ca.log</string>
</dict></plist>
EOF
  launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true
  launchctl bootstrap "gui/$(id -u)" "$PLIST"
  log "LaunchAgent $LABEL installed (runs at login and daily: re-export + launchctl setenv)"
  refresh

  # Paseo applies the login-shell env to the daemon and its agents; terminals' Node tools benefit too.
  touch "$ZPROFILE"
  if ! grep -qF "$MARK" "$ZPROFILE"; then zprofile_line >> "$ZPROFILE"; log "added NODE_EXTRA_CA_CERTS to $ZPROFILE"; fi

  [ "${1:-}" = --restart ] && restart
  log "installed. Check with: $SELF_DST doctor"
}

doctor() {
  local uri=${1:-} rc=0
  echo "app:          $APP ($(defaults read "$APP/Contents/Info" CFBundleShortVersionString 2>/dev/null || echo ?))"
  echo "ca bundle:    $( [ -f "$PEM" ] && echo "$PEM ($(grep -c 'BEGIN CERT' "$PEM") certs)" || echo missing)"
  local le; le=$(launchctl getenv NODE_EXTRA_CA_CERTS 2>/dev/null || true); echo "launchd env:  ${le:-unset}"
  echo "zprofile:     $(grep -qF "$MARK" "$ZPROFILE" 2>/dev/null && echo set || echo unset)"
  echo "launchagent:  $(launchctl print "gui/$(id -u)/$LABEL" >/dev/null 2>&1 && echo loaded || echo missing)"
  echo "tls w/o CA:   $(probe "" || true)"
  local w; w=$(probe "$PEM" || true); echo "tls with CA:  $w"; case $w in ok*) ;; *) rc=1 ;; esac
  local pid; pid=$(cat "$DIR/paseo.pid" 2>/dev/null | tr -dc 0-9 || true)
  if [ -n "$pid" ] && ps -p "$pid" >/dev/null 2>&1; then
    local envd; envd=$(ps eww -o command= -p "$pid" 2>/dev/null | tr ' ' '\n' | grep '^NODE_EXTRA_CA_CERTS=' || true)
    echo "daemon:       pid $pid, ${envd:-NODE_EXTRA_CA_CERTS NOT in its env (restart Paseo)}"
    [ -n "$envd" ] || rc=1
  else
    echo "daemon:       not running (or not visible from a sandbox)"
  fi
  echo "daemon relay: $(grep -E 'relay_control_(connected|disconnected)' "$DIR/daemon.log" 2>/dev/null | tail -1 | grep -oE '"msg":"[^"]*"' || echo none)"
  if [ -n "$uri" ]; then
    echo "e2e relay:    $CLI --host <uri> daemon status"
    if NODE_EXTRA_CA_CERTS="$PEM" to 45 "$CLI" --host "$uri" daemon status --json 2>&1 | tee /dev/stderr | grep -q '"serverId"'; then
      echo "e2e relay:    OK"
    else echo "e2e relay:    FAILED"; rc=1; fi
  fi
  return $rc
}

uninstall() {
  launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true
  rm -f "$PLIST" "$SELF_DST"
  launchctl unsetenv NODE_EXTRA_CA_CERTS || true
  if [ -f "$ZPROFILE" ]; then grep -vF "$MARK" "$ZPROFILE" > "$ZPROFILE.tmp" || true; mv "$ZPROFILE.tmp" "$ZPROFILE"; fi
  rm -f "$PEM"
  log "removed (restart Paseo to drop the env from running processes)"
}

case "${1:-}" in
  install) shift; install "$@" ;;
  export) export_pem ;;
  session) export_pem; restart ;;
  refresh) refresh ;;
  restart) restart ;;
  doctor) shift; doctor "$@" ;;
  uninstall) uninstall ;;
  *) sed -n '2,17p' "$0"; exit 2 ;;
esac

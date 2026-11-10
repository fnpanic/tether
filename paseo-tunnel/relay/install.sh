#!/bin/bash
# Paseo relay on the Tether Hetzner node. Run as root from a dir holding this script, paseo.caddy,
# paseo-relay.service and the binary paseo-relay (zenghongtu/paseo-relay v0.5.0 linux-amd64).
#   sudo bash install.sh            install or update
#   sudo bash install.sh uninstall  remove everything this script added
# Tether itself is not edited: its Caddyfile already imports /etc/tether/caddy.d/*.caddy, and the
# relay is reached at wss://<RELAY_HOST>:443/ws. Tether's Caddy runs with the admin API off, so a
# changed drop-in needs a restart of tether-https (about 1 s without HTTPS; clients reconnect).
set -euo pipefail
cd "$(dirname "$0")"
DROPIN=/etc/tether/caddy.d/paseo.caddy
H=$(grep -h '^RELAY_HOST=' /etc/tether/server.env | cut -d= -f2)
up() { curl --http1.1 -s -o /dev/null -m 3 -w '%{http_code}' -H 'Connection: Upgrade' -H 'Upgrade: websocket' \
  -H 'Sec-WebSocket-Version: 13' -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==' "https://$H$1" || true; }
restart_tether_caddy() {  # restart; if Tether is not healthy afterwards, drop the drop-in and restart again
  systemctl restart tether-https; sleep 3
  if [ "$(up /ws/id)" != 101 ] || [ "$(up /ws/relay)" != 101 ]; then
    rm -f "$DROPIN"; systemctl restart tether-https; echo "Tether unhealthy with the drop-in: removed, rolled back"; exit 1
  fi
}

if [ "${1:-}" = uninstall ]; then
  if [ -f "$DROPIN" ]; then rm -f "$DROPIN"; restart_tether_caddy; fi
  systemctl disable --now paseo-relay 2>/dev/null || true
  rm -rf /etc/systemd/system/paseo-relay.service /usr/local/bin/paseo-relay /etc/paseo-relay
  systemctl daemon-reload
  echo "uninstalled"; exit 0
fi

echo "325be2e7e74424e347002b29845ff812f15fd827fda1c15aa59f4ffe0553fa5c  paseo-relay" | sha256sum -c
if ss -Htlnp 'sport = :8411' | grep -v paseo-relay | grep -q .; then echo "8411 is in use, aborting"; exit 1; fi

install -m 755 paseo-relay /usr/local/bin/
install -m 644 paseo-relay.service /etc/systemd/system/
install -d -m 755 /etc/paseo-relay
install -m 755 install.sh /etc/paseo-relay/install.sh  # kept for "uninstall"
restorecon -F /usr/local/bin/paseo-relay 2>/dev/null || true
systemctl daemon-reload
systemctl enable paseo-relay
systemctl restart paseo-relay

if ! cmp -s paseo.caddy "$DROPIN"; then
  install -m 644 paseo.caddy "$DROPIN.new"
  t=$(mktemp -d); cp /etc/tether/caddy.d/*.caddy "$t/" 2>/dev/null || true; cp "$DROPIN.new" "$t/paseo.caddy"
  sed "s|/etc/tether/caddy.d/|$t/|" /etc/tether/Caddyfile > "$t/Caddyfile"
  if ! RELAY_HOST=$H /usr/local/bin/caddy validate --config "$t/Caddyfile" --adapter caddyfile >/dev/null 2>&1; then
    rm -rf "$t" "$DROPIN.new"; echo "drop-in invalid: Tether not touched"; exit 1; fi
  rm -rf "$t"; mv "$DROPIN.new" "$DROPIN"; restorecon "$DROPIN" 2>/dev/null || true
  restart_tether_caddy
fi
sleep 1
curl -fsS http://127.0.0.1:8411/health && echo
echo "tether /ws/id $(up /ws/id), paseo /ws $(up '/ws?role=client&v=2') (400 = relay answers)"

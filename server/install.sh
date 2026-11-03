#!/usr/bin/env bash
# Install the Tether relay on a systemd Linux host from prebuilt hbbs/hbbr binaries.
#   sudo ./install.sh relay.example.com [path/to/binaries]
set -euo pipefail
HOST="${1:?usage: install.sh <public-host-or-ip> [bin-dir]}"
BIN="${2:-$(dirname "$0")/bin}"
[ "$(id -u)" = 0 ] || { echo "run as root" >&2; exit 1; }
for b in hbbs hbbr; do [ -x "$BIN/$b" ] || { echo "missing $BIN/$b" >&2; exit 1; }; done

id tether >/dev/null 2>&1 || useradd --system --home /var/lib/tether --shell /usr/sbin/nologin tether
install -d -o tether -g tether -m 0750 /var/lib/tether
install -d -m 0755 /etc/tether
install -m 0755 "$BIN/hbbs" "$BIN/hbbr" /usr/local/bin/
echo "RELAY_HOST=$HOST" > /etc/tether/server.env
install -m 0644 "$(dirname "$0")/systemd/tether-hbbs.service" "$(dirname "$0")/systemd/tether-hbbr.service" /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now tether-hbbr tether-hbbs

# hbbs creates the key pair on first start.
for _ in $(seq 1 20); do [ -s /var/lib/tether/id_ed25519.pub ] && break; sleep 0.5; done
if command -v ufw >/dev/null && ufw status | grep -q active; then
  ufw allow 21115:21117/tcp && ufw allow 21116/udp
fi
echo
echo "Tether relay is running on $HOST"
echo "Open firewall ports: TCP 21115-21117, UDP 21116"
echo "Build clients with:"
echo "  TETHER_SERVER=$HOST TETHER_PUB_KEY=$(cat /var/lib/tether/id_ed25519.pub)"

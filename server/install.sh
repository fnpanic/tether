#!/usr/bin/env bash
# Install the Tether relay on a systemd Linux host from prebuilt hbbs/hbbr binaries.
#   sudo ./install.sh relay.example.com [path/to/binaries]
# Picks bin/ (x86_64) or bin-arm64/ (aarch64, e.g. Oracle Cloud Ampere) automatically.
set -euo pipefail
HOST="${1:?usage: install.sh <public-host-or-ip> [bin-dir]}"
HERE="$(cd "$(dirname "$0")" && pwd)"
case "$(uname -m)" in
  x86_64 | amd64) DEFAULT_BIN="$HERE/bin" ;;
  aarch64 | arm64) DEFAULT_BIN="$HERE/bin-arm64" ;;
  *) echo "unsupported CPU: $(uname -m)" >&2; exit 1 ;;
esac
BIN="${2:-$DEFAULT_BIN}"
[ "$(id -u)" = 0 ] || { echo "run as root" >&2; exit 1; }
for b in hbbs hbbr; do [ -x "$BIN/$b" ] || { echo "missing $BIN/$b" >&2; exit 1; }; done

id tether >/dev/null 2>&1 || useradd --system --home /var/lib/tether --shell /usr/sbin/nologin tether
install -d -o tether -g tether -m 0750 /var/lib/tether
install -d -m 0755 /etc/tether
install -m 0755 "$BIN/hbbs" "$BIN/hbbr" /usr/local/bin/
echo "RELAY_HOST=$HOST" > /etc/tether/server.env
install -m 0644 "$HERE/systemd/tether-hbbs.service" "$HERE/systemd/tether-hbbr.service" /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now tether-hbbr tether-hbbs
systemctl restart tether-hbbr tether-hbbs

# Host firewall. ufw if active; otherwise raw iptables, as on Oracle Cloud's Ubuntu images,
# which ship an INPUT chain that rejects everything except SSH.
if command -v ufw >/dev/null && ufw status | grep -q "Status: active"; then
  ufw allow 21115:21117/tcp
  ufw allow 21116/udp
elif command -v iptables >/dev/null && iptables -S INPUT | grep -q -- "-j REJECT"; then
  for rule in "-p tcp -m multiport --dports 21115:21117" "-p udp --dport 21116"; do
    # shellcheck disable=SC2086
    iptables -C INPUT $rule -j ACCEPT 2>/dev/null || iptables -I INPUT 5 $rule -m state --state NEW -j ACCEPT
  done
  if command -v netfilter-persistent >/dev/null; then
    netfilter-persistent save
  else
    echo "note: install iptables-persistent to keep the firewall rules across reboots" >&2
  fi
fi

# hbbs creates the key pair on first start.
for _ in $(seq 1 40); do [ -s /var/lib/tether/id_ed25519.pub ] && break; sleep 0.5; done
systemctl --no-pager --lines=0 status tether-hbbs tether-hbbr | grep -E "Active:" || true
echo
echo "Tether relay is running on $HOST"
echo "Cloud firewall (e.g. Oracle VCN security list) must allow: TCP 21115-21117, UDP 21116"
echo "Build clients with:"
echo "  TETHER_SERVER=$HOST TETHER_PUB_KEY=$(cat /var/lib/tether/id_ed25519.pub)"

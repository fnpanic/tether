#!/usr/bin/env bash
# Install or update the Tether relay on a systemd Linux host from prebuilt binaries:
# hbbs/hbbr (ID + relay), tether-api (relay API) and Caddy (HTTPS/WebSocket front end on 443).
#   sudo ADMIN_PUB_KEY=<b64>[,<b64>] ./install.sh relay.example.com [path/to/binaries]
# Optional env: DUCKDNS_DOMAIN + DUCKDNS_TOKEN (keep the name pointed at this host),
# RELAY_SERVERS (every relay hbbs hands out, for several relays; see docs/relay-ha.md),
# CADDY_VERSION (default below). Safe to re-run; an existing /etc/tether/api.env is kept unless
# ADMIN_PUB_KEY is given.
# Picks bin/ (x86_64) or bin-arm64/ (aarch64, e.g. Oracle Cloud Ampere) automatically.
set -euo pipefail
HOST="${1:?usage: install.sh <public-host-name> [bin-dir]}"
HERE="$(cd "$(dirname "$0")" && pwd)"
CADDY_VERSION="${CADDY_VERSION:-2.10.2}"
case "$(uname -m)" in
  x86_64 | amd64) DEFAULT_BIN="$HERE/bin"; GOARCH=amd64 ;;
  aarch64 | arm64) DEFAULT_BIN="$HERE/bin-arm64"; GOARCH=arm64 ;;
  *) echo "unsupported CPU: $(uname -m)" >&2; exit 1 ;;
esac
BIN="${2:-$DEFAULT_BIN}"
[ "$(id -u)" = 0 ] || { echo "run as root" >&2; exit 1; }
for b in hbbs hbbr tether-api; do [ -x "$BIN/$b" ] || { echo "missing $BIN/$b" >&2; exit 1; }; done

sysuser() { id "$1" >/dev/null 2>&1 || useradd --system --home "$2" --shell /usr/sbin/nologin "$1"; }
sysuser tether /var/lib/tether
sysuser tether-api /var/lib/tether-api
sysuser tether-https /var/lib/tether-https
# Caddy serves /download/* straight from tether-api's data dir (group-readable, UMask=0027).
usermod -aG tether-api tether-https
install -d -o tether -g tether -m 0750 /var/lib/tether
install -d -o tether-api -g tether-api -m 0750 /var/lib/tether-api
install -d -o tether-https -g tether-https -m 0750 /var/lib/tether-https
install -d -m 0755 /etc/tether
install -m 0755 "$BIN/hbbs" "$BIN/hbbr" "$BIN/tether-api" /usr/local/bin/

# Caddy: official static release binary, checked against the release's checksum file.
if ! /usr/local/bin/caddy version 2>/dev/null | grep -q "^v$CADDY_VERSION "; then
  tmp="$(mktemp -d)"
  base="https://github.com/caddyserver/caddy/releases/download/v$CADDY_VERSION"
  tarball="caddy_${CADDY_VERSION}_linux_$GOARCH.tar.gz"
  curl -fsSL -o "$tmp/$tarball" "$base/$tarball"
  curl -fsSL -o "$tmp/sums" "$base/caddy_${CADDY_VERSION}_checksums.txt"
  (cd "$tmp" && grep " $tarball\$" sums | sha512sum -c -)
  tar -xzf "$tmp/$tarball" -C "$tmp" caddy
  install -m 0755 "$tmp/caddy" /usr/local/bin/caddy
  rm -rf "$tmp"
fi

# Relay list: this relay first (checked locally), then the others, e.g.
#   RELAY_SERVERS="a.example|http://127.0.0.1:21121/readyz,b.example|https://b.example/readyz/relay"
printf 'RELAY_HOST=%s\nRELAY_SERVERS=%s\n' "$HOST" "${RELAY_SERVERS:-$HOST|http://127.0.0.1:21121/readyz}" > /etc/tether/server.env
install -m 0644 "$HERE/caddy/Caddyfile" /etc/tether/Caddyfile
if [ -n "${ADMIN_PUB_KEY:-}" ]; then
  echo "ADMIN_PUB_KEY=$ADMIN_PUB_KEY" > /etc/tether/api.env
elif [ ! -f /etc/tether/api.env ]; then
  echo "ADMIN_PUB_KEY=" > /etc/tether/api.env
  echo "note: /etc/tether/api.env has no ADMIN_PUB_KEY; admin endpoints stay disabled until it is set" >&2
fi
chmod 0644 /etc/tether/api.env

units=(tether-hbbs.service tether-hbbr.service tether-https.service tether-api.service)
for u in "${units[@]}"; do install -m 0644 "$HERE/systemd/$u" /etc/systemd/system/; done
if [ -n "${DUCKDNS_DOMAIN:-}" ] && [ -n "${DUCKDNS_TOKEN:-}" ]; then
  (umask 077; printf 'DUCKDNS_DOMAIN=%s\nDUCKDNS_TOKEN=%s\n' "$DUCKDNS_DOMAIN" "$DUCKDNS_TOKEN" > /etc/tether/duckdns.env)
  install -m 0644 "$HERE/systemd/tether-ddns.service" "$HERE/systemd/tether-ddns.timer" /etc/systemd/system/
fi

# SELinux (Oracle Linux): give the new files their default contexts.
if command -v restorecon >/dev/null; then
  restorecon -R /usr/local/bin /etc/tether /etc/systemd/system /var/lib/tether /var/lib/tether-api /var/lib/tether-https
fi

# Host firewall first, so Caddy can get its certificate on the first try: relay ports, 443, and 80 forwarded to 443 (fallback route for clients).
if command -v firewall-cmd >/dev/null && firewall-cmd --state >/dev/null 2>&1; then
  # Oracle Linux / RHEL family.
  firewall-cmd --permanent --add-port=21115-21117/tcp --add-port=21116/udp --add-port=443/tcp --add-port=80/tcp
  firewall-cmd --permanent --add-forward-port=port=80:proto=tcp:toport=443
  firewall-cmd --reload
elif command -v ufw >/dev/null && ufw status | grep -q "Status: active"; then
  ufw allow 21115:21117/tcp
  ufw allow 21116/udp
  ufw allow 443/tcp
  ufw allow 80/tcp
  if ! grep -q "tether: 80->443" /etc/ufw/before.rules; then
    rules="$(cat /etc/ufw/before.rules)"
    printf '# tether: 80->443\n*nat\n:PREROUTING ACCEPT [0:0]\n-A PREROUTING -p tcp --dport 80 -j REDIRECT --to-ports 443\nCOMMIT\n\n%s\n' "$rules" > /etc/ufw/before.rules
  fi
  ufw reload
elif command -v iptables >/dev/null && iptables -S INPUT | grep -q -- "-j REJECT"; then
  # Raw iptables, as on Oracle Cloud's Ubuntu images (INPUT rejects everything except SSH).
  for rule in "-p tcp -m multiport --dports 21115:21117" "-p udp --dport 21116" "-p tcp --dport 443"; do
    # shellcheck disable=SC2086
    iptables -C INPUT $rule -m state --state NEW -j ACCEPT 2>/dev/null || iptables -I INPUT 5 $rule -m state --state NEW -j ACCEPT
  done
  iptables -t nat -C PREROUTING -p tcp --dport 80 -j REDIRECT --to-ports 443 2>/dev/null \
    || iptables -t nat -A PREROUTING -p tcp --dport 80 -j REDIRECT --to-ports 443
  if command -v netfilter-persistent >/dev/null; then
    netfilter-persistent save
  else
    echo "note: install iptables-persistent to keep the firewall rules across reboots" >&2
  fi
fi

systemctl daemon-reload
systemctl enable tether-hbbr tether-hbbs tether-api tether-https
systemctl restart tether-hbbr tether-hbbs tether-api tether-https
[ -f /etc/tether/duckdns.env ] && systemctl enable --now tether-ddns.timer
# dnf's metadata refresh needs ~700 MB: on a 1 GB VM (Oracle Always Free micro) it runs the box
# out of memory and stalls SSH and HTTPS for minutes. Updates still work with a manual `dnf upgrade`.
if [ "$(awk '/MemTotal/ {print $2}' /proc/meminfo)" -lt 1500000 ]; then
  systemctl disable --now dnf-makecache.timer 2>/dev/null || true
fi

# hbbs creates the key pair on first start.
for _ in $(seq 1 40); do [ -s /var/lib/tether/id_ed25519.pub ] && break; sleep 0.5; done
systemctl --no-pager --lines=0 status tether-hbbs tether-hbbr tether-api tether-https | grep -E "Active:" || true
echo
echo "Tether relay is running on $HOST"
echo "Cloud firewall (e.g. Oracle VCN security list) must allow: TCP 443, TCP 80, and for stock RustDesk clients TCP 21115-21117, UDP 21116"
echo "DNS for $HOST must point here before Caddy can get its certificate (TLS-ALPN-01 on 443)."
echo "Build clients with:"
echo "  TETHER_SERVER=$HOST TETHER_PUB_KEY=$(cat /var/lib/tether/id_ed25519.pub)"

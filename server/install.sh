#!/usr/bin/env bash
# Install or update the Tether relay on a systemd Linux host from prebuilt binaries:
# hbbs/hbbr (ID + relay), tether-api (relay API) and the HTTPS/WebSocket front end on 443 (Caddy,
# or nginx with PROXY=nginx).
#   sudo ADMIN_PUB_KEY=<b64>[,<b64>] ./install.sh relay.example.com [path/to/binaries]
# Optional env: DUCKDNS_DOMAIN + DUCKDNS_TOKEN (keep the name pointed at this host),
# RELAY_SERVERS (every relay hbbs hands out, for several relays; default: the list installed
# before, else this host; see docs/relay-ha.md),
# PROXY=caddy|nginx (front end; default: the one installed before, else caddy; nginx comes from
# nginx.org's signed repository, see docs/proxy-benchmark.md), CADDY_VERSION (default below).
# Safe to re-run; an existing /etc/tether/api.env is kept unless ADMIN_PUB_KEY is given.
# Picks bin/ (x86_64) or bin-arm64/ (aarch64, e.g. Oracle Cloud Ampere) automatically.
set -euo pipefail
HOST="${1:?usage: install.sh <public-host-name> [bin-dir]}"
HERE="$(cd "$(dirname "$0")" && pwd)"
CADDY_VERSION="${CADDY_VERSION:-2.10.2}"
# nginx-prometheus-exporter (PROXY=nginx): official release, SHA-256 from its checksums file.
NGX_EXPORTER_VERSION=1.5.3
declare -A NGX_EXPORTER_SUM=(
  [amd64]=3a0dd1ea6db57cd360544d0aed38df4adabcfeb31e931dc481d535c66f92641d
  [arm64]=a0b1a5f5bba09483bd2e04c759c1a75fffe46ca72c0314ee2bb925d1742ff23b
)
# The front end: as asked, else the one this host already runs, else Caddy.
PROXY="${PROXY:-$(sed -n 's/^PROXY=//p' /etc/tether/server.env 2>/dev/null)}"
PROXY="${PROXY:-caddy}"
# The relay list: as asked, else the one this host already uses (a re-run must not drop the other
# relays of an HA setup), else this host alone (below).
RELAY_SERVERS="${RELAY_SERVERS:-$(sed -n 's/^RELAY_SERVERS=//p' /etc/tether/server.env 2>/dev/null)}"
case "$PROXY" in caddy | nginx) ;; *) echo "PROXY must be caddy or nginx" >&2; exit 1 ;; esac
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
# hbbs reads tether-api's release-generation for its update push hint (docs/update-design.md).
usermod -aG tether-api tether
install -d -o tether -g tether -m 0750 /var/lib/tether
install -d -o tether-api -g tether-api -m 0750 /var/lib/tether-api
install -d -o tether-https -g tether-https -m 0750 /var/lib/tether-https
install -d -m 0755 /etc/tether
# Optional Caddy snippets (the monitoring installer puts /_metrics/* and /grafana/* there).
install -d -m 0755 /etc/tether/caddy.d
install -m 0755 "$BIN/hbbs" "$BIN/hbbr" "$BIN/tether-api" /usr/local/bin/

# Caddy: official static release binary, checked against the release's checksum file. Installed
# with PROXY=nginx too, so switching back is a re-run without downloads.
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
printf 'RELAY_HOST=%s\nRELAY_SERVERS=%s\nPROXY=%s\n' "$HOST" "${RELAY_SERVERS:-$HOST|http://127.0.0.1:21121/readyz}" "$PROXY" > /etc/tether/server.env
install -m 0644 "$HERE/caddy/Caddyfile" /etc/tether/Caddyfile

if [ "$PROXY" = nginx ]; then
  # nginx mainline + its ACME module from nginx.org (signed packages; the OS's automatic updates
  # keep them patched). The packaged nginx.service stays off: tether-https runs nginx.
  if command -v dnf >/dev/null; then
    [ -f /etc/yum.repos.d/nginx.repo ] || printf '%s\n' '[nginx-mainline]' 'name=nginx mainline repo' \
      'baseurl=https://nginx.org/packages/mainline/rhel/$releasever/$basearch/' 'gpgcheck=1' 'enabled=1' \
      'gpgkey=https://nginx.org/keys/nginx_signing.key' 'module_hotfixes=true' > /etc/yum.repos.d/nginx.repo
    dnf -y install nginx nginx-module-acme
    command -v semanage >/dev/null || dnf -y install policycoreutils-python-utils
  elif command -v apt-get >/dev/null; then
    if [ ! -f /etc/apt/sources.list.d/nginx.list ]; then
      curl -fsSL https://nginx.org/keys/nginx_signing.key | gpg --dearmor > /usr/share/keyrings/nginx-archive-keyring.gpg
      os="$(. /etc/os-release && echo "$ID $VERSION_CODENAME")"
      echo "deb [signed-by=/usr/share/keyrings/nginx-archive-keyring.gpg] https://nginx.org/packages/mainline/${os% *} ${os#* } nginx" > /etc/apt/sources.list.d/nginx.list
      apt-get update
    fi
    apt-get install -y nginx nginx-module-acme
  else
    echo "PROXY=nginx needs dnf or apt-get" >&2; exit 1
  fi
  systemctl disable --now nginx.service 2>/dev/null || true
  if ! /usr/local/bin/nginx-prometheus-exporter --version 2>&1 | grep -q "version $NGX_EXPORTER_VERSION "; then
    tmp="$(mktemp -d)"
    tarball="nginx-prometheus-exporter_${NGX_EXPORTER_VERSION}_linux_$GOARCH.tar.gz"
    curl -fsSL -o "$tmp/$tarball" "https://github.com/nginx/nginx-prometheus-exporter/releases/download/v$NGX_EXPORTER_VERSION/$tarball"
    echo "${NGX_EXPORTER_SUM[$GOARCH]}  $tmp/$tarball" | sha256sum -c --quiet -
    tar -xzf "$tmp/$tarball" -C "$tmp" nginx-prometheus-exporter
    install -m 0755 "$tmp/nginx-prometheus-exporter" /usr/local/bin/
    rm -rf "$tmp"
  fi
  install -d -m 0755 /etc/tether/nginx.d
  install -d -o tether-https -g tether-https -m 0750 /var/lib/tether-https/nginx /var/lib/tether-https/acme
  # nginx resolves the ACME server itself: the first resolver of this host.
  resolver="$(awk '$1 == "nameserver" {print $2; exit}' /etc/resolv.conf)"
  case "$resolver" in *:*) resolver="[$resolver]" ;; esac
  sed -e "s|@RELAY_HOST@|$HOST|g" -e "s|@RESOLVER@|${resolver:-127.0.0.53}|g" "$HERE/nginx/nginx.conf" > /etc/tether/nginx.conf.new
  chmod 0644 /etc/tether/nginx.conf.new
  if ! /usr/sbin/nginx -e stderr -t -q -c /etc/tether/nginx.conf.new; then
    rm -f /etc/tether/nginx.conf.new
    echo "nginx config invalid: nothing changed" >&2; exit 1
  fi
  mv /etc/tether/nginx.conf.new /etc/tether/nginx.conf
fi
if [ -n "${ADMIN_PUB_KEY:-}" ]; then
  echo "ADMIN_PUB_KEY=$ADMIN_PUB_KEY" > /etc/tether/api.env
elif [ ! -f /etc/tether/api.env ]; then
  echo "ADMIN_PUB_KEY=" > /etc/tether/api.env
  echo "note: /etc/tether/api.env has no ADMIN_PUB_KEY; admin endpoints stay disabled until it is set" >&2
fi
chmod 0644 /etc/tether/api.env

units=(tether-hbbs.service tether-hbbr.service tether-api.service)
for u in "${units[@]}"; do install -m 0644 "$HERE/systemd/$u" /etc/systemd/system/; done
# Same unit name for either front end, so runbooks (systemctl restart tether-https) stay as they are.
if [ "$PROXY" = nginx ]; then
  install -m 0644 "$HERE/systemd/tether-https-nginx.service" /etc/systemd/system/tether-https.service
  install -m 0644 "$HERE/systemd/tether-nginx-exporter.service" /etc/systemd/system/
else
  install -m 0644 "$HERE/systemd/tether-https.service" /etc/systemd/system/
  if [ -f /etc/systemd/system/tether-nginx-exporter.service ]; then
    systemctl disable --now tether-nginx-exporter.service 2>/dev/null || true
    rm -f /etc/systemd/system/tether-nginx-exporter.service
  fi
fi
# Kernel limits for 10 000 hosts / 3 000 sessions per node (docs/scaling.md); -e skips keys this
# kernel lacks (nf_conntrack before the firewall loaded it).
if [ -f "$HERE/sysctl/90-tether.conf" ]; then
  install -m 0644 "$HERE/sysctl/90-tether.conf" /etc/sysctl.d/90-tether.conf
  sysctl -e -q -p /etc/sysctl.d/90-tether.conf || true
fi
if [ -n "${DUCKDNS_DOMAIN:-}" ] && [ -n "${DUCKDNS_TOKEN:-}" ]; then
  (umask 077; printf 'DUCKDNS_DOMAIN=%s\nDUCKDNS_TOKEN=%s\n' "$DUCKDNS_DOMAIN" "$DUCKDNS_TOKEN" > /etc/tether/duckdns.env)
  install -m 0644 "$HERE/systemd/tether-ddns.service" "$HERE/systemd/tether-ddns.timer" /etc/systemd/system/
fi

# SELinux (Oracle Linux): give the new files their default contexts.
if command -v restorecon >/dev/null; then
  # nginx runs confined (httpd_t), unlike Caddy and the Tether services (binaries in /usr/local/bin,
  # unconfined): let it proxy to the local services, read the release files, keep its state and
  # serve stub_status on 21126. Caddy does not care about these labels.
  if [ "$PROXY" = nginx ] && command -v semanage >/dev/null && selinuxenabled 2>/dev/null; then
    setsebool -P httpd_can_network_connect 1
    semanage port -a -t http_port_t -p tcp 21126 2>/dev/null || semanage port -m -t http_port_t -p tcp 21126
    fc() { semanage fcontext -a -t "$1" "$2" 2>/dev/null || semanage fcontext -m -t "$1" "$2"; }
    fc httpd_sys_content_t '/var/lib/tether-api(/.*)?'
    fc httpd_var_lib_t '/var/lib/tether-https(/.*)?'
    fc httpd_var_run_t '/run/tether-https(/.*)?'
    fc httpd_config_t '/etc/tether/nginx\.conf'
    fc httpd_config_t '/etc/tether/nginx\.d(/.*)?'
  fi
  restorecon -R /usr/local/bin /etc/tether /etc/systemd/system /var/lib/tether /var/lib/tether-api /var/lib/tether-https
  [ -f /etc/sysctl.d/90-tether.conf ] && restorecon /etc/sysctl.d/90-tether.conf
fi

# Host firewall first, so the front end can get its certificate on the first try: relay ports, 443
# and 80 (the front end listens on 80 itself and redirects to https; never forward 80 to the TLS port).
if command -v firewall-cmd >/dev/null && firewall-cmd --state >/dev/null 2>&1; then
  # Oracle Linux / RHEL family.
  firewall-cmd --permanent --add-port=21115-21117/tcp --add-port=21116/udp --add-port=443/tcp --add-port=80/tcp
  firewall-cmd --reload
elif command -v ufw >/dev/null && ufw status | grep -q "Status: active"; then
  ufw allow 21115:21117/tcp
  ufw allow 21116/udp
  ufw allow 443/tcp
  ufw allow 80/tcp
  ufw reload
elif command -v iptables >/dev/null && iptables -S INPUT | grep -q -- "-j REJECT"; then
  # Raw iptables, as on Oracle Cloud's Ubuntu images (INPUT rejects everything except SSH).
  for rule in "-p tcp -m multiport --dports 21115:21117" "-p udp --dport 21116" "-p tcp --dport 443" "-p tcp --dport 80"; do
    # shellcheck disable=SC2086
    iptables -C INPUT $rule -m state --state NEW -j ACCEPT 2>/dev/null || iptables -I INPUT 5 $rule -m state --state NEW -j ACCEPT
  done
  if command -v netfilter-persistent >/dev/null; then
    netfilter-persistent save
  else
    echo "note: install iptables-persistent to keep the firewall rules across reboots" >&2
  fi
fi

systemctl daemon-reload
systemctl enable tether-hbbr tether-hbbs tether-api tether-https
systemctl restart tether-hbbr tether-hbbs tether-api tether-https
if [ "$PROXY" = nginx ]; then systemctl enable tether-nginx-exporter && systemctl restart tether-nginx-exporter; fi
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
echo "DNS for $HOST must point here before $PROXY can get its certificate (TLS-ALPN-01 on 443)."
echo "Build clients with:"
echo "  TETHER_SERVER=$HOST TETHER_PUB_KEY=$(cat /var/lib/tether/id_ed25519.pub)"

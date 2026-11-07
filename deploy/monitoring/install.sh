#!/usr/bin/env bash
# Tether monitoring installer (docs/monitoring.md). Run as root on the relay nodes, from a copy
# of this repository's deploy/monitoring (and server/caddy for the Caddyfile):
#
#   Every node scraped from the OTHER node (here: the Oracle backup):
#     sudo PEER_HOST=<monitoring node's public name> SCRAPE_FROM=<its IP> SCRAPE_HASH='<bcrypt hash>' ./install.sh target
#   The monitoring node (here: the Hetzner primary), after the line above was printed for it:
#     sudo REMOTE_HOST=<other node's public name> ./install.sh monitor
# The front end (Caddy or nginx, PROXY= in /etc/tether/server.env) gets the matching snippets;
# REMOTE_PROXY=nginx on the monitoring node when the other node runs nginx (default caddy).
#
# target:  node_exporter + blackbox_exporter on localhost; with SCRAPE_FROM/SCRAPE_HASH also
#          /_metrics/* in Caddy for the Prometheus on the monitoring node.
# monitor: target, plus Prometheus, Alertmanager and Grafana (RPM repo) on localhost and Grafana
#          at https://<this host>/grafana/ behind Caddy basic auth + Grafana login. Generates
#          the secrets under /etc/tether-monitoring on first run and prints what to keep.
# Optional for monitor: LOCAL_NAME / REMOTE_NAME (node labels, default: first label of the host
# names), REMOTE_ADDR (default <REMOTE_HOST>:443), TELEGRAM_CHAT_ID + TELEGRAM_BOT_TOKEN.
# Safe to re-run. Everything listens on 127.0.0.1; no firewall port is opened.
set -euo pipefail
ROLE="${1:?usage: install.sh target|monitor}"
HERE="$(cd "$(dirname "$0")" && pwd)"
ETC=/etc/tether-monitoring
[ "$(id -u)" = 0 ] || { echo "run as root" >&2; exit 1; }
case "$(uname -m)" in
  x86_64 | amd64) ARCH=amd64 ;;
  aarch64 | arm64) ARCH=arm64 ;;
  *) echo "unsupported CPU: $(uname -m)" >&2; exit 1 ;;
esac
[ -f /etc/tether/server.env ] || { echo "no /etc/tether/server.env: install the Tether relay first (server/install.sh)" >&2; exit 1; }
HOST="$(sed -n 's/^RELAY_HOST=//p' /etc/tether/server.env)"
PROXY="$(sed -n 's/^PROXY=//p' /etc/tether/server.env)"; PROXY="${PROXY:-caddy}"
# The node whose public endpoints this node probes: the other one.
[ "$ROLE" = monitor ] && PEER_HOST="${REMOTE_HOST:-}"
: "${PEER_HOST:?PEER_HOST=<public host name of the other node> is required (monitor: REMOTE_HOST)}"

# Official release binaries, checked against these SHA-256 sums (from each release's sha256sums.txt).
declare -A SUM=(
  [prometheus-3.13.5.linux-amd64]=0388822ace519c5a3a8ef556a541ce1b536e21d872f82deded465bd27a5a0699
  [prometheus-3.13.5.linux-arm64]=e705357ffa5b3189e98bf145d044364d75dd994470c706a07731292932b2ee0f
  [alertmanager-0.34.2.linux-amd64]=17ee14acd0f6ba6e25a436ff223d9c17ddd5cf484f543ad12f0932eaa89ba186
  [alertmanager-0.34.2.linux-arm64]=5aaad82e3d167c30c06ce7d38677d2892a6f66f4c3613ea17af7481750fdd0b0
  [node_exporter-1.12.1.linux-amd64]=b51d8a76aa2a9156a55d501aca6276fae09e262259a5e4e831d2c2222f084e63
  [node_exporter-1.12.1.linux-arm64]=ad35b605f9954b9f1ffddf5ba054bdc5a98d790b9eae5291e1eeb83f1ecbd0e7
  [blackbox_exporter-0.29.1.linux-amd64]=b82b81839e6317e84fdd367f46d7ddd3b773fa09cbe2e3d5633a59171d741e76
  [blackbox_exporter-0.29.1.linux-arm64]=c597b0d442c02c0f2c8f691cbc7784216c786fada28f300c7b914e1f94523834
)
# fetch <name> <version> <binaries...>: installs /usr/local/bin/<binary> unless that version is there.
fetch() {
  local name=$1 ver=$2; shift 2
  if /usr/local/bin/"$1" --version 2>&1 | grep -q "version $ver "; then return; fi
  local base="$name-$ver.linux-$ARCH" tmp; tmp="$(mktemp -d)"
  curl -fsSL -o "$tmp/$base.tar.gz" "https://github.com/prometheus/$name/releases/download/v$ver/$base.tar.gz"
  echo "${SUM[$base]}  $tmp/$base.tar.gz" | sha256sum -c --quiet -
  tar -xzf "$tmp/$base.tar.gz" -C "$tmp"
  for b in "$@"; do install -m 0755 "$tmp/$base/$b" /usr/local/bin/; done
  rm -rf "$tmp"
}
sysuser() { id "$1" >/dev/null 2>&1 || useradd --system --no-create-home --home-dir "${2:-/}" --shell /usr/sbin/nologin "$1"; }
fill() { # fill <src> <dest> <mode> [owner]: copy with the @...@ placeholders filled in
  sed -e "s|@ETC@|$ETC|g" -e "s|@HOST@|$HOST|g" -e "s|@PEER_HOST@|$PEER_HOST|g" -e "s|@LOCAL@|${LOCAL_NAME:-}|g" -e "s|@REMOTE@|${REMOTE_NAME:-}|g" \
      -e "s|@LOCAL_HOST@|$HOST|g" -e "s|@REMOTE_HOST@|${REMOTE_HOST:-}|g" -e "s|@REMOTE_ADDR@|${REMOTE_ADDR:-}|g" \
      -e "s|@REMOTE_SCHEME@|https|g" -e "s|@SCRAPE_FROM@|${SCRAPE_FROM:-}|g" -e "s|@SCRAPE_HASH@|${SCRAPE_HASH:-}|g" \
      -e "s|@GRAFANA_USER@|${GRAFANA_USER:-}|g" -e "s|@GRAFANA_HASH@|${GRAFANA_HASH:-}|g" \
      -e "s|@LOCAL_PROXY@|$PROXY|g" -e "s|@REMOTE_PROXY@|${REMOTE_PROXY:-caddy}|g" \
      -e "s|@TELEGRAM_CHAT_ID@|${TELEGRAM_CHAT_ID:-0}|g" "$1" > "$2.new"
  chmod "$3" "$2.new"
  [ -z "${4:-}" ] || chown "$4" "$2.new"
  mv "$2.new" "$2"
}
secret() { [ -s "$1" ] || (umask 077; head -c 24 /dev/urandom | base64 | tr -d '/+=' > "$1"); }
# bcrypt hash for the basic-auth passwords, accepted by Caddy and nginx. Cost 5: the passwords are
# 24 random bytes (no guessing at any cost), and nginx checks the hash on every request inside a
# worker that also carries thousands of WebSockets.
bcrypt() { python3 -W ignore -c 'import crypt,sys; print(crypt.crypt(sys.argv[1], crypt.mksalt(crypt.METHOD_BLOWFISH, rounds=32)))' "$1" 2>/dev/null || /usr/local/bin/caddy hash-password --plaintext "$1"; }

install -d -m 0755 $ETC /etc/tether/caddy.d /etc/tether/nginx.d

# ---- every node: host metrics and probes ----
sysuser node_exporter
sysuser blackbox_exporter
fetch node_exporter 1.12.1 node_exporter
fetch blackbox_exporter 0.29.1 blackbox_exporter
fill "$HERE/blackbox/blackbox.yml" $ETC/blackbox.yml 0644
for u in node_exporter blackbox_exporter; do install -m 0644 "$HERE/systemd/$u.service" /etc/systemd/system/; done

# The relay's Caddyfile must have the metrics listener and the caddy.d import (server/caddy/Caddyfile
# since monitoring); install it from this checkout if the node still has an older one.
caddy_changed=0
nginx_changed=0
if [ "$PROXY" = caddy ] && ! grep -q 'caddy.d/\*.caddy' /etc/tether/Caddyfile; then
  [ -f "$HERE/../../server/caddy/Caddyfile" ] || { echo "/etc/tether/Caddyfile is older than monitoring: copy server/caddy/ along" >&2; exit 1; }
  cp -p /etc/tether/Caddyfile "/etc/tether/Caddyfile.bak-$(date +%Y%m%d%H%M)"
  install -m 0644 "$HERE/../../server/caddy/Caddyfile" /etc/tether/Caddyfile.new
  caddy_changed=1
fi
if [ -n "${SCRAPE_FROM:-}" ] || [ -n "${SCRAPE_HASH:-}" ]; then
  [ -n "${SCRAPE_FROM:-}" ] && [ -n "${SCRAPE_HASH:-}" ] || { echo "SCRAPE_FROM and SCRAPE_HASH go together" >&2; exit 1; }
  case "$SCRAPE_HASH" in '$2'*) ;; *) echo "SCRAPE_HASH must be a bcrypt hash" >&2; exit 1 ;; esac
  # bcrypt hashes contain no | (the sed delimiter in fill)
  if [ "$PROXY" = nginx ]; then
    fill "$HERE/nginx/metrics-remote.conf" /etc/tether/nginx.d/metrics-remote.conf.new 0644
    printf 'prometheus:%s\n' "$SCRAPE_HASH" > /etc/tether/nginx.d/metrics.htpasswd.new
    chgrp tether-https /etc/tether/nginx.d/metrics.htpasswd.new && chmod 0640 /etc/tether/nginx.d/metrics.htpasswd.new
    for f in metrics-remote.conf metrics.htpasswd; do
      cmp -s /etc/tether/nginx.d/$f.new /etc/tether/nginx.d/$f 2>/dev/null || nginx_changed=1
    done
  else
    fill "$HERE/caddy/metrics-remote.caddy" /etc/tether/caddy.d/metrics-remote.caddy.new 0644
    if ! cmp -s /etc/tether/caddy.d/metrics-remote.caddy.new /etc/tether/caddy.d/metrics-remote.caddy 2>/dev/null; then caddy_changed=1; fi
  fi
fi

# ---- monitoring node ----
if [ "$ROLE" = monitor ]; then
  : "${REMOTE_HOST:?REMOTE_HOST=<public host name of the other node> is required}"
  LOCAL_NAME="${LOCAL_NAME:-${HOST%%.*}}"
  REMOTE_NAME="${REMOTE_NAME:-${REMOTE_HOST%%.*}}"
  REMOTE_ADDR="${REMOTE_ADDR:-$REMOTE_HOST:443}"
  sysuser prometheus /var/lib/prometheus
  sysuser alertmanager /var/lib/alertmanager
  fetch prometheus 3.13.5 prometheus promtool
  fetch alertmanager 0.34.2 alertmanager amtool
  install -d -o prometheus -g prometheus -m 0750 /var/lib/prometheus
  install -d -o alertmanager -g alertmanager -m 0750 /var/lib/alertmanager
  install -d -m 0755 $ETC/rules $ETC/grafana $ETC/grafana/dashboards $ETC/grafana/provisioning/datasources $ETC/grafana/provisioning/dashboards

  # Secrets (kept on re-runs): the scrape password Prometheus sends to the other node, Grafana's
  # admin password and the password of Caddy's basic auth in front of Grafana.
  secret $ETC/scrape.pass
  chgrp prometheus $ETC/scrape.pass && chmod 0640 $ETC/scrape.pass
  secret $ETC/grafana-web.pass
  GRAFANA_USER=tether
  GRAFANA_HASH="$(bcrypt "$(cat $ETC/grafana-web.pass)")"

  fill "$HERE/prometheus/prometheus.yml" $ETC/prometheus.yml 0644
  install -m 0644 "$HERE/prometheus/rules/tether.yml" $ETC/rules/tether.yml
  /usr/local/bin/promtool check config $ETC/prometheus.yml
  if [ -n "${TELEGRAM_CHAT_ID:-}" ] && [ -n "${TELEGRAM_BOT_TOKEN:-}" ]; then
    (umask 027; printf '%s' "$TELEGRAM_BOT_TOKEN" > $ETC/telegram.token); chgrp alertmanager $ETC/telegram.token
    fill "$HERE/alertmanager/alertmanager.yml" $ETC/alertmanager.yml.tmp 0644
    sed '0,/^  receiver: none$/s//  receiver: telegram/' $ETC/alertmanager.yml.tmp > $ETC/alertmanager.yml && rm $ETC/alertmanager.yml.tmp
  elif ! grep -q 'receiver: telegram' $ETC/alertmanager.yml 2>/dev/null; then
    # No notification channel yet: drop the telegram receiver (it needs a chat id).
    sed '/^  - name: telegram$/,$d' "$HERE/alertmanager/alertmanager.yml" > $ETC/alertmanager.yml
  fi
  chmod 0644 $ETC/alertmanager.yml
  /usr/local/bin/amtool check-config $ETC/alertmanager.yml
  for u in prometheus alertmanager; do install -m 0644 "$HERE/systemd/$u.service" /etc/systemd/system/; done

  # Grafana OSS from Grafana's RPM repository: dnf-automatic keeps it patched like the OS.
  if ! rpm -q grafana >/dev/null 2>&1; then
    cat > /etc/yum.repos.d/grafana.repo <<'EOF'
[grafana]
name=grafana
baseurl=https://rpm.grafana.com
repo_gpgcheck=1
enabled=1
gpgcheck=1
gpgkey=https://rpm.grafana.com/gpg.key
sslverify=1
sslcacert=/etc/pki/tls/certs/ca-bundle.crt
EOF
    dnf -y install grafana
  fi
  secret $ETC/grafana-admin.pass
  chgrp grafana $ETC/grafana-admin.pass && chmod 0640 $ETC/grafana-admin.pass
  fill "$HERE/grafana/grafana.env" $ETC/grafana.env 0644
  fill "$HERE/grafana/provisioning/datasources/tether.yaml" $ETC/grafana/provisioning/datasources/tether.yaml 0644
  fill "$HERE/grafana/provisioning/dashboards/tether.yaml" $ETC/grafana/provisioning/dashboards/tether.yaml 0644
  install -d -m 0755 $ETC/grafana/provisioning/plugins $ETC/grafana/provisioning/alerting $ETC/grafana/provisioning/notifiers
  install -m 0644 "$HERE"/grafana/dashboards/*.json $ETC/grafana/dashboards/
  install -d -m 0755 /etc/systemd/system/grafana-server.service.d
  install -m 0644 "$HERE/systemd/grafana-server.override.conf" /etc/systemd/system/grafana-server.service.d/tether.conf

  if [ "$PROXY" = nginx ]; then
    install -m 0644 "$HERE/nginx/grafana.conf" /etc/tether/nginx.d/grafana.conf.new
    # Keep the existing hash when the password is unchanged (bcrypt salts differ per run).
    if [ -f /etc/tether/nginx.d/grafana.htpasswd ] && python3 -W ignore -c 'import crypt,sys; h=open(sys.argv[2]).read().split(":",1)[1].strip(); sys.exit(crypt.crypt(sys.argv[1], h) != h)' "$(cat $ETC/grafana-web.pass)" /etc/tether/nginx.d/grafana.htpasswd; then
      cp -p /etc/tether/nginx.d/grafana.htpasswd /etc/tether/nginx.d/grafana.htpasswd.new
    else
      printf '%s:%s\n' "$GRAFANA_USER" "$GRAFANA_HASH" > /etc/tether/nginx.d/grafana.htpasswd.new
    fi
    chgrp tether-https /etc/tether/nginx.d/grafana.htpasswd.new && chmod 0640 /etc/tether/nginx.d/grafana.htpasswd.new
    for f in grafana.conf grafana.htpasswd; do
      cmp -s /etc/tether/nginx.d/$f.new /etc/tether/nginx.d/$f 2>/dev/null || nginx_changed=1
    done
  else
    fill "$HERE/caddy/grafana.caddy" /etc/tether/caddy.d/grafana.caddy.new 0644
    if ! cmp -s /etc/tether/caddy.d/grafana.caddy.new /etc/tether/caddy.d/grafana.caddy 2>/dev/null; then caddy_changed=1; fi
  fi
fi

if command -v restorecon >/dev/null; then
  restorecon -R /usr/local/bin $ETC /etc/tether /etc/systemd/system
  [ "$ROLE" = monitor ] && restorecon -R /var/lib/prometheus /var/lib/alertmanager
fi

# New Caddy config: validate the new set in a scratch copy first, then swap it in and restart
# (the admin API is off, so no reload; about a second without HTTPS).
if [ "$caddy_changed" = 1 ]; then
  t="$(mktemp -d)"
  cp /etc/tether/caddy.d/*.caddy "$t/" 2>/dev/null || true
  for f in /etc/tether/caddy.d/*.new; do [ -f "$f" ] && cp "$f" "$t/$(basename "${f%.new}")"; done
  cf=/etc/tether/Caddyfile; [ -f $cf.new ] && cf=$cf.new
  sed "s|/etc/tether/caddy.d/|$t/|" $cf > "$t/Caddyfile"
  if ! RELAY_HOST="$HOST" /usr/local/bin/caddy validate --config "$t/Caddyfile" --adapter caddyfile > "$t/log" 2>&1; then
    tail -n 5 "$t/log" >&2
    rm -rf "$t" /etc/tether/Caddyfile.new /etc/tether/caddy.d/*.new
    echo "new Caddy config invalid: nothing changed, Caddy not restarted" >&2
    exit 1
  fi
  rm -rf "$t"
  for f in /etc/tether/Caddyfile.new /etc/tether/caddy.d/*.new; do [ -f "$f" ] && mv "$f" "${f%.new}"; done
fi
rm -f /etc/tether/caddy.d/*.new

# New nginx snippets: test the whole config with them in place, keep the old set if it fails, then
# reload (graceful: no WebSocket is dropped).
if [ "$nginx_changed" = 1 ]; then
  t="$(mktemp -d)"
  cp -p /etc/tether/nginx.d/* "$t/" 2>/dev/null || true
  for f in /etc/tether/nginx.d/*.new; do mv "$f" "${f%.new}"; done
  if ! /usr/sbin/nginx -e stderr -t -q -c /etc/tether/nginx.conf; then
    rm -f /etc/tether/nginx.d/*; cp -p "$t"/* /etc/tether/nginx.d/ 2>/dev/null || true; rm -rf "$t"
    echo "new nginx config invalid: nothing changed, nginx not reloaded" >&2
    exit 1
  fi
  rm -rf "$t"
fi
rm -f /etc/tether/nginx.d/*.new

systemctl daemon-reload
systemctl enable --now node_exporter blackbox_exporter
systemctl restart node_exporter blackbox_exporter
if [ "$ROLE" = monitor ]; then
  systemctl enable --now prometheus alertmanager grafana-server
  systemctl restart prometheus alertmanager grafana-server
fi
[ "$caddy_changed" = 1 ] && systemctl restart tether-https
[ "$nginx_changed" = 1 ] && systemctl reload tether-https

echo
echo "Tether monitoring ($ROLE) installed on $HOST."
if [ "$ROLE" = monitor ]; then
  echo "On $REMOTE_HOST run:"
  echo "  sudo PEER_HOST=$HOST SCRAPE_FROM=<public IP of this node> SCRAPE_HASH='$(bcrypt "$(cat $ETC/scrape.pass)")' ./install.sh target"
  echo
  echo "Grafana: https://$HOST/grafana/"
  echo "  browser prompt ($PROXY): user tether, password in $ETC/grafana-web.pass"
  echo "  Grafana login: user admin, password in $ETC/grafana-admin.pass (first start only)"
  echo "Keep both passwords in your password manager: sudo cat $ETC/grafana-web.pass $ETC/grafana-admin.pass"
fi

#!/usr/bin/env bash
# Official monitoring releases for scripts/test-monitoring.sh, each unpacked in its own dir under
# TOOLS (default ~/mon-tools) and checked against the release's checksums. Linux x86_64.
set -euo pipefail
T="${1:-$HOME/mon-tools}"
mkdir -p "$T"
prom() { # name version
  local f="$1-$2.linux-amd64.tar.gz" d="$T/dl-$1"
  [ -d "$d/$1-$2.linux-amd64" ] && return
  mkdir -p "$d" && cd "$d"
  curl -fsSLO "https://github.com/prometheus/$1/releases/download/v$2/$f"
  curl -fsSL "https://github.com/prometheus/$1/releases/download/v$2/sha256sums.txt" | grep " $f\$" | sha256sum -c -
  tar xzf "$f" && rm "$f"
}
# Same versions as deploy/monitoring/install.sh.
prom prometheus 3.13.5
prom alertmanager 0.34.2
prom node_exporter 1.12.1
prom blackbox_exporter 0.29.1
if [ ! -x "$T/dl-caddy/caddy" ]; then
  mkdir -p "$T/dl-caddy" && cd "$T/dl-caddy"
  f=caddy_2.10.2_linux_amd64.tar.gz
  curl -fsSLO "https://github.com/caddyserver/caddy/releases/download/v2.10.2/$f"
  curl -fsSL https://github.com/caddyserver/caddy/releases/download/v2.10.2/caddy_2.10.2_checksums.txt | grep " $f\$" | sha512sum -c -
  tar xzf "$f" caddy && rm "$f"
fi
if [ ! -d "$T/dl-grafana/grafana-13.3.0" ]; then
  mkdir -p "$T/dl-grafana" && cd "$T/dl-grafana"
  f=grafana-13.3.0.linux-amd64.tar.gz
  curl -fsSLO "https://dl.grafana.com/oss/release/$f"
  echo "$(curl -fsSL "https://dl.grafana.com/oss/release/$f.sha256")  $f" | sha256sum -c -
  tar xzf "$f" && rm "$f"
fi
ls "$T"

#!/usr/bin/env bash
# Front proxy under test for tools/load/stack.sh (FRONT=<name>), docs/proxy-benchmark.md:
#   front.sh start <name> <work-dir>   writes <work-dir>/caddy.pid (the run scripts sample "caddy")
#   front.sh client <work-dir>         the client-side TLS layer (HAProxy) -> caddy2.pid
# Names: caddy (production Caddyfile, done by stack.sh), haproxy, nginx, envoy, traefik.
# Binaries: $PB (default ~/proxy-bench): src/haproxy-*/haproxy, nginx/sbin/nginx, dl/envoy, dl/traefik.
# NGINX_EXTRA: directives added to the bench nginx http block (e.g. "ssl_buffer_size 4k;").
# Every front gets the same ECDSA P-256 certificate for "localhost" in <work-dir>/tls.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"; PB="${PB:-$HOME/proxy-bench}"
HAPROXY="${HAPROXY:-$(ls -d "$PB"/src/haproxy-*/haproxy | tail -1)}"
cmd="$1"
fill() { sed -e "s#@W@#$W#g" -e "s#@WORKERS@#${WORKERS:-auto}#g" -e "s#^	server_tokens off;#	server_tokens off;${NGINX_EXTRA:-}#" "$HERE/conf/$1" > "$W/$1"; }
tls() {
  [ -s "$W/tls/localhost.pem" ] && return
  mkdir -p "$W/tls"
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 30 -subj /CN=localhost \
    -addext subjectAltName=DNS:localhost -keyout "$W/tls/localhost.key" -out "$W/tls/localhost.crt" 2>/dev/null
  cat "$W/tls/localhost.crt" "$W/tls/localhost.key" > "$W/tls/localhost.pem"
}
# Files for fronts without a file server: a one-worker plain nginx on 127.0.0.1:28126 (bench config).
files() {
  fill nginx.conf; mkdir -p "$W/ngx"
  sed -i -e 's/^worker_processes .*/worker_processes 1;/' -e '/listen 127.0.0.1:28443/d' -e '/http2 on/d' -e '/ssl_certificate/d'     -e "s#$W/nginx.pid#$W/nginx-files.pid#" "$W/nginx.conf"
  "$PB/nginx/sbin/nginx" -c "$W/nginx.conf" -e "$W/nginx-error.log"
}
case "$cmd" in
client)
  W="$2"; fill client-haproxy.cfg
  setsid "$HAPROXY" -f "$W/client-haproxy.cfg" > "$W/caddy2.log" 2>&1 & echo $! > "$W/caddy2.pid"
  ;;
start)
  name="$2"; W="$3"; tls
  case "$name" in
  haproxy)
    fill haproxy.cfg; files
    setsid "$HAPROXY" -f "$W/haproxy.cfg" > "$W/front.log" 2>&1 & echo $! > "$W/caddy.pid"
    ;;
  haproxy-l4)
    fill haproxy-l4.cfg
    setsid "$HAPROXY" -f "$W/haproxy-l4.cfg" > "$W/front.log" 2>&1 & echo $! > "$W/caddy.pid"
    ;;
  nginx)
    fill nginx.conf; mkdir -p "$W/ngx"
    setsid "$PB/nginx/sbin/nginx" -c "$W/nginx.conf" -e "$W/nginx-error.log" -g 'daemon off;' > "$W/front.log" 2>&1 & echo $! > "$W/caddy.pid"
    cp "$W/caddy.pid" "$W/front.sid"
    ;;
  envoy)
    fill envoy.yaml; files
    setsid "$PB/dl/envoy" -c "$W/envoy.yaml" --log-level error ${ENVOY_ARGS:-} > "$W/front.log" 2>&1 & echo $! > "$W/caddy.pid"
    ;;
  traefik)
    fill traefik.yml; fill traefik-routes.yml; files
    setsid env ${TRAEFIK_ENV:-} "$PB/dl/traefik" --configFile="$W/traefik.yml" > "$W/front.log" 2>&1 & echo $! > "$W/caddy.pid"
    ;;
  *) echo "unknown front $name" >&2; exit 2 ;;
  esac
  ;;
stop)
  W="$2"
  # nginx masters lead their own sessions (setsid / daemon): kill them with their workers.
  for f in "$W/nginx-files.pid" "$W/front.sid"; do [ -s "$f" ] && pkill -9 -s "$(cat "$f")" 2>/dev/null; done
  rm -f "$W/nginx-files.pid" "$W/front.sid"
  for c in "-f $W/haproxy.cfg" "-f $W/haproxy-l4.cfg" "-f $W/client-haproxy.cfg" "$W/envoy.yaml" "$W/traefik.yml"; do pkill -9 -f -- "$c" 2>/dev/null || true; done
  ;;
esac

#!/usr/bin/env bash
# Ports stay below 32768, the start of Linux's ephemeral range: 10 000 test connections would
# otherwise occupy them as source ports.
# Local Tether server stack for load tests (docs/scaling.md "Method"), on ports 25xxx/28xxx so a
# relay running on the default ports is not touched:
#   tools/load/stack.sh start <bin-dir> <work-dir>   hbbs, hbbr, tether-api, Caddy (production
#                                                    Caddyfile with `tls internal`) + a client-side
#                                                    Caddy that adds TLS for the load generator
#   tools/load/stack.sh stop <work-dir>
# Ports: hbbs 25116 (tcp/udp), ws 25118, health 25122; hbbr 25117, ws 25119, health 25121;
# tether-api 25120; Caddy https://localhost:28443 and http://localhost:28080 (same routes);
# client Caddy http://127.0.0.1:28081 -> https://localhost:28443.
# Env: CADDY (default ~/caddy-dl/caddy), CADDYFILE (default server/caddy/Caddyfile), CADDY_ENV (front Caddy), HBBS_ENV / HBBR_ENV / API_ENV (extra "K=V ..." words).
# FRONT=caddy|haproxy|nginx|envoy|traefik: the front under test (tools/proxy-bench/front.sh) with an
# HAProxy client-side TLS layer instead of the client Caddy (docs/proxy-benchmark.md). caddy.pid is
# then the front, whichever it is.
set -euo pipefail
cmd="$1"; HERE="$(cd "$(dirname "$0")" && pwd)"
case "$cmd" in
start)
  BIN="$(cd "$2" && pwd)"; W="$3"; CADDY="${CADDY:-$HOME/caddy-dl/caddy}"
  mkdir -p "$W"/{hb,api,caddy,caddy2}; W="$(cd "$W" && pwd)"
  ulimit -n 1048576
  # Same rewrite as the systemd units' environment: ports moved, data dir moved, ACME -> internal CA.
  sed -e '/^\ttls {/,/^\t}/c\	tls internal' \
      -e 's/^{\$RELAY_HOST} {/https:\/\/localhost:28443, http:\/\/localhost:28080 {/' \
      -e 's/^\tadmin off/\tadmin off\n\tskip_install_trust\n\thttp_port 28082\n\thttps_port 28443/' \
      -e 's/2111\([5-9]\)/2511\1/g; s/2112\([0-6]\)/2512\1/g' \
      -e "s#/var/lib/tether-api#$W/api#g" "${CADDYFILE:-$HERE/../../server/caddy/Caddyfile}" > "$W/Caddyfile"
  cat > "$W/Caddyfile.client" <<C
{
	admin off
	http_port 28091
	https_port 28092
}
http://127.0.0.1:28081 {
	reverse_proxy https://127.0.0.1:28443 {
		header_up Host localhost
		flush_interval -1
		transport http {
			tls_insecure_skip_verify
			tls_server_name localhost
		}
	}
}
C
  cd "$W/hb"
  setsid env ${HBBR_ENV:-} HEALTH_PORT=25121 "$BIN/hbbr" -p 25117 -k _ > "$W/hbbr.log" 2>&1 & echo $! > "$W/hbbr.pid"
  setsid env ${HBBS_ENV:-} HEALTH_PORT=25122 "$BIN/hbbs" -p 25116 -k _ -r 'ws://127.0.0.1:25119' > "$W/hbbs.log" 2>&1 & echo $! > "$W/hbbs.pid"
  setsid env ${API_ENV:-} ADMIN_PUB_KEY= "$BIN/tether-api" --data "$W/api" --listen 127.0.0.1:25120 > "$W/api.log" 2>&1 & echo $! > "$W/api.pid"
  if [ -z "${FRONT:-}" ] || [ "$FRONT" = caddy ]; then
    XDG_DATA_HOME="$W/caddy" XDG_CONFIG_HOME="$W/caddy" setsid env ${CADDY_ENV:-} "$CADDY" run --config "$W/Caddyfile" --adapter caddyfile > "$W/caddy.log" 2>&1 & echo $! > "$W/caddy.pid"
  else
    "$HERE/../proxy-bench/front.sh" start "$FRONT" "$W"
  fi
  if [ -n "${FRONT:-}" ]; then
    "$HERE/../proxy-bench/front.sh" client "$W"
  else
    XDG_DATA_HOME="$W/caddy2" XDG_CONFIG_HOME="$W/caddy2" setsid "$CADDY" run --config "$W/Caddyfile.client" --adapter caddyfile > "$W/caddy2.log" 2>&1 & echo $! > "$W/caddy2.pid"
  fi
  for _ in $(seq 100); do [ -s "$W/hb/id_ed25519.pub" ] && curl -sk -o /dev/null https://localhost:28443/ && curl -sf -o /dev/null localhost:25121/readyz && curl -sf -o /dev/null localhost:25122/readyz && break; sleep 0.1; done
  echo "key=$(cat "$W/hb/id_ed25519.pub")"
  ;;
stop)
  W="$2"
  pids=()
  for p in caddy2 caddy api hbbs hbbr; do [ -f "$W/$p.pid" ] && pids+=("$(cat "$W/$p.pid")"); rm -f "$W/$p.pid"; done
  [ ${#pids[@]} -gt 0 ] && kill -9 "${pids[@]}" 2>/dev/null || true
  # Wait until they are gone, so the next start can bind the ports.
  for p in "${pids[@]}"; do for _ in $(seq 100); do [ -d "/proc/$p" ] || break; sleep 0.1; done; done
  # Anything else left on the test ports (e.g. an hbbs restarted by a scenario script).
  pkill -9 -f -- "-p 2511[67] -k _" 2>/dev/null || true
  pkill -9 -f -- "--listen 127.0.0.1:25120" 2>/dev/null || true
  pkill -9 -f -- "--config $W/Caddyfile" 2>/dev/null || true
  "$HERE/../proxy-bench/front.sh" stop "$W" 2>/dev/null || true
  sleep 0.5
  ;;
*) echo "usage: stack.sh start <bin-dir> <work-dir> | stop <work-dir>" >&2; exit 2 ;;
esac

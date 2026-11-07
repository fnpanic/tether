#!/usr/bin/env bash
# End-to-end test of the production nginx front (server/nginx/nginx.conf + the monitoring
# snippets) in WSL/Linux without root: every route, ACME (TLS-ALPN-01 against a local Pebble),
# WebSockets through it (load generator + relay-harness), graceful reload with open WebSockets,
# tether-api upload/download, the /q/ X-Accel-Redirect path, basic auth, the /_metrics source-IP
# restriction and the exporter. docs/proxy-benchmark.md "End-to-end test".
#   tools/proxy-bench/e2e-nginx.sh [bin-dir]      (relay binaries, default ~/scale-build/bin)
# Needs (see docs/proxy-benchmark.md): $NGX = the nginx.org mainline nginx + nginx-module-acme
# packages unpacked (default ~/proxy-bench/ngx-official/root), $PEBBLE dir with pebble + its test
# certs (default ~/proxy-bench/pebble), nginx-prometheus-exporter and HAProxy (client TLS layer)
# under ~/proxy-bench. Ports as in tools/load/stack.sh: 2511x/2512x services, nginx on 28443/28080.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"; REPO="$HERE/../.."
BIN="$(cd "${1:-$HOME/scale-build/bin}" && pwd)"; PB="${PB:-$HOME/proxy-bench}"
NGX="${NGX:-$PB/ngx-official/root}"; PEBBLE="${PEBBLE:-$PB/pebble}"
HAPROXY="${HAPROXY:-$(ls -d "$PB"/src/haproxy-*/haproxy | tail -1)}"
W=/tmp/pb-e2e
fails=0; pass=0
ok() { pass=$((pass + 1)); echo "ok   $*"; }
bad() { fails=$((fails + 1)); echo "FAIL $*"; }
check() { local name="$1"; shift; if "$@"; then ok "$name"; else bad "$name"; fi; }
stop() {
  for f in "$W"/*.pid; do [ -s "$f" ] && kill -9 "$(cat "$f")" 2>/dev/null; done
  [ -s "$W/run/nginx.pid" ] && pkill -9 -s "$(cat "$W/nginx.sid" 2>/dev/null || echo 0)" 2>/dev/null
  pkill -9 -f -- "-c $W/nginx.conf" 2>/dev/null; pkill -9 -f "nginx: worker process" -U "$(id -u)" 2>/dev/null
  pkill -9 -f -- "--listen 127.0.0.1:25120" 2>/dev/null; pkill -9 -f -- "-p 2511[67] -k _" 2>/dev/null
  pkill -9 -f -- "$W/" 2>/dev/null
  sleep 0.5
}
stop; rm -rf "$W"; mkdir -p "$W"/{hb,api/download/stable/customer/windows-x86_64,api/custom,api/diag,https/nginx,https/acme,run,nginx.d,pebble}
ulimit -n 65536

# ---- relay services (as tools/load/stack.sh) ----
cd "$W/hb"
HEALTH_PORT=25121 METRICS_PORT=25124 setsid "$BIN/hbbr" -p 25117 -k _ > "$W/hbbr.log" 2>&1 & echo $! > "$W/hbbr.pid"
IP_BLOCK_PER_MIN=100000 IP_BLOCK_IDS_PER_DAY=100000 HEALTH_PORT=25122 METRICS_PORT=25123 setsid "$BIN/hbbs" -p 25116 -k _ -r 'ws://127.0.0.1:28081/ws/relay|http://127.0.0.1:25121/readyz' > "$W/hbbs.log" 2>&1 & echo $! > "$W/hbbs.pid"
ADMIN_PUB_KEY= setsid "$BIN/tether-api" --data "$W/api" --listen 127.0.0.1:25120 > "$W/api.log" 2>&1 & echo $! > "$W/api.pid"
cd "$W"

# ---- Pebble (local ACME CA), validating TLS-ALPN-01 on nginx's TLS port ----
sed -e 's#"tlsPort": 5001#"tlsPort": 28443#' -e 's#"httpPort": 5002#"httpPort": 28080#' \
    -e "s#test/certs/localhost/#$PEBBLE/#g" -e 's#0.0.0.0:14000#127.0.0.1:14000#' -e 's#0.0.0.0:15000#127.0.0.1:15000#' \
    "$PEBBLE/pebble-config.json" > "$W/pebble/config.json"
PEBBLE_VA_NOSLEEP=1 setsid "$PEBBLE/pebble" -config "$W/pebble/config.json" > "$W/pebble.log" 2>&1 & echo $! > "$W/pebble.pid"

# ---- nginx: the production config, ports and paths moved (same rewrite as stack.sh) ----
sed -e "s#^load_module modules/#load_module $NGX/usr/lib/nginx/modules/#" \
    -e "s#include /etc/nginx/mime.types#include $NGX/etc/nginx/mime.types#" \
    -e 's#listen 443 #listen 127.0.0.1:28443 #; s#listen \[::\]:443 #listen [::1]:28443 #' \
    -e 's#listen 80 #listen 127.0.0.1:28080 #; s#listen \[::\]:80 #listen [::1]:28080 #' \
    -e 's/2111\([5-9]\)/2511\1/g; s/2112\([0-7]\)/2512\1/g' \
    -e "s#/var/lib/tether-api#$W/api#g; s#/var/lib/tether-https#$W/https#g; s#/run/tether-https#$W/run#g; s#/etc/tether/nginx.d#$W/nginx.d#g" \
    -e 's#@RELAY_HOST@#localhost#; s#@RESOLVER@#127.0.0.53#' \
    -e "s#uri https://acme-v02.api.letsencrypt.org/directory;#uri https://localhost:14000/dir;\n\t\tssl_trusted_certificate $PEBBLE/pebble.minica.pem;#" \
    "$REPO/server/nginx/nginx.conf" > "$W/nginx.conf"
# The monitoring snippets as install.sh writes them: scraper 127.0.0.1, bcrypt (cost 5) passwords.
hash() { python3 -W ignore -c 'import crypt,sys; print(crypt.crypt(sys.argv[1], crypt.mksalt(crypt.METHOD_BLOWFISH, rounds=32)))' "$1"; }
echo "prometheus:$(hash scrape-secret)" > "$W/nginx.d/metrics.htpasswd"
echo "tether:$(hash grafana-secret)" > "$W/nginx.d/grafana.htpasswd"
sed -e 's#@SCRAPE_FROM@#127.0.0.1#' -e 's/2111\([5-9]\)/2511\1/g; s/2112\([0-7]\)/2512\1/g' -e 's#127.0.0.1:9100#127.0.0.1:29100#; s#127.0.0.1:9115#127.0.0.1:29115#' \
    -e "s#/etc/tether/nginx.d#$W/nginx.d#g" "$REPO/deploy/monitoring/nginx/metrics-remote.conf" > "$W/nginx.d/metrics-remote.conf"
sed -e 's#127.0.0.1:3000#127.0.0.1:23000#' -e "s#/etc/tether/nginx.d#$W/nginx.d#g" "$REPO/deploy/monitoring/nginx/grafana.conf" > "$W/nginx.d/grafana.conf"
# Fake Grafana / node_exporter: echo the request headers back.
cat > "$W/echo.py" <<'PY'
import http.server, sys
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        b = ("path=%s\n" % self.path + "".join("%s: %s\n" % kv for kv in self.headers.items())).encode()
        self.send_response(200); self.send_header("Content-Length", str(len(b))); self.end_headers(); self.wfile.write(b)
    def log_message(self, *a): pass
http.server.ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
PY
for p in 23000 29100 29115; do setsid python3 -I "$W/echo.py" $p > /dev/null 2>&1 & echo $! > "$W/echo$p.pid"; done
for _ in $(seq 50); do curl -sk -o /dev/null https://localhost:14000/dir && break; sleep 0.2; done

check "nginx -t accepts the production config" "$NGX/usr/sbin/nginx" -e stderr -t -q -p "$W/https/" -c "$W/nginx.conf"
setsid "$NGX/usr/sbin/nginx" -e stderr -p "$W/https/" -c "$W/nginx.conf" -g 'daemon off;' > "$W/nginx.log" 2>&1 & echo $! > "$W/nginx.sid"
"$PB/dl/nginx-prometheus-exporter" --web.listen-address=127.0.0.1:25125 --nginx.scrape-uri=http://127.0.0.1:25126/stub_status > "$W/exporter.log" 2>&1 & echo $! > "$W/exporter.pid"
for _ in $(seq 100); do [ -s "$W/hb/id_ed25519.pub" ] && curl -sf -o /dev/null localhost:25121/readyz && curl -sf -o /dev/null localhost:25122/readyz && break; sleep 0.1; done

# ---- ACME: certificate from Pebble via TLS-ALPN-01 ----
curl -sk https://localhost:15000/roots/0 > "$W/pebble-root.pem"
got=0; for _ in $(seq 60); do curl -s --cacert "$W/pebble-root.pem" -o /dev/null https://localhost:28443/ && { got=1; break; }; sleep 0.5; done
check "ACME (TLS-ALPN-01, Pebble): certificate issued and trusted" [ $got = 1 ]
C=(curl -4 -s --cacert "$W/pebble-root.pem")
issuer=$(echo | openssl s_client -connect 127.0.0.1:28443 -servername localhost 2>/dev/null | openssl x509 -noout -issuer 2>/dev/null)
echo "     issuer: $issuer"
check "certificate kept across restart (state_path)" test -n "$(ls -A "$W/https/acme" 2>/dev/null)"

# ---- TLS ----
check "TLS 1.3" bash -c "echo | openssl s_client -connect 127.0.0.1:28443 -servername localhost -tls1_3 2>/dev/null | grep -q 'TLSv1.3'"
check "TLS 1.2 (ECDHE AEAD)" bash -c "echo | openssl s_client -connect 127.0.0.1:28443 -servername localhost -tls1_2 2>/dev/null | grep -qE 'Cipher is ECDHE-.*(GCM|CHACHA)'"
check "TLS 1.1 refused" bash -c "! echo | openssl s_client -connect 127.0.0.1:28443 -servername localhost -tls1_1 2>/dev/null | grep -q 'Cipher is ECDHE'"
check "TLS 1.2 CBC cipher refused" bash -c "! echo | openssl s_client -connect 127.0.0.1:28443 -servername localhost -tls1_2 -cipher ECDHE-ECDSA-AES128-SHA 2>/dev/null | grep -q 'Cipher is ECDHE'"
check "HTTP/2 on normal routes" [ "$("${C[@]}" --http2 -o /dev/null -w '%{http_version}' https://localhost:28443/)" = 2 ]
h=$("${C[@]}" -D - -o /dev/null https://localhost:28443/ | tr -d '\r')
check "HSTS header" grep -qi '^strict-transport-security: max-age=31536000' <<< "$h"
check "Server header has no version" bash -c "! grep -qiE '^server: .*[0-9]' <<< \"\$1\"" _ "$h"
r=$(curl -s -o /dev/null -w '%{http_code} %{redirect_url}' http://localhost:28080/x?y=1)
check "HTTP on 80 redirects to HTTPS ($r)" [ "$r" = "301 https://localhost/x?y=1" ]

# ---- plain routes ----
check "/ marker" [ "$("${C[@]}" https://localhost:28443/)" = "Tether relay" ]
check "unknown path marker" [ "$("${C[@]}" https://localhost:28443/nothing/here)" = "Tether relay" ]
echo '<h1>download page</h1>' > "$W/api/download/index.html"
check "/ serves the published download page" grep -q "download page" <<< "$("${C[@]}" https://localhost:28443/)"
check "/readyz/relay -> hbbr /readyz" grep -q "^ready" <<< "$("${C[@]}" https://localhost:28443/readyz/relay)"
check "/rendezvous/pubkey -> hbbs /pubkey" [ "$("${C[@]}" https://localhost:28443/rendezvous/pubkey)" = "$(cat "$W/hb/id_ed25519.pub")" ]

# ---- /download/ ----
d="$W/api/download/stable/customer/windows-x86_64"
head -c 6000000 /dev/urandom > "$d/TetherQS-9.9.9.exe"  # 6 MB: downloads run at 512 KB/s after the first MB
echo '{"version":"9.9.9","platforms":{"windows-x86_64":{"signature":"x","url":"https://r/download/stable/customer/windows-x86_64/TetherQS-9.9.9.exe"}}}' > "$d/latest.json"
h=$("${C[@]}" -D - -o "$W/dl.exe" https://localhost:28443/download/stable/customer/windows-x86_64/TetherQS-9.9.9.exe | tr -d '\r')
check "download 6 MB intact" cmp -s "$W/dl.exe" "$d/TetherQS-9.9.9.exe"
check "download content type" grep -qi '^content-type: application/octet-stream' <<< "$h"
h=$("${C[@]}" -D - -o /dev/null https://localhost:28443/download/stable/customer/windows-x86_64/latest.json | tr -d '\r')
check "latest.json: application/json + no-cache + HSTS" bash -c "grep -qi '^content-type: application/json' <<< \"\$1\" && grep -qi '^cache-control: no-cache' <<< \"\$1\" && grep -qi '^strict-transport-security' <<< \"\$1\"" _ "$h"
# Download budget: the first MB at full speed, then 512 KB/s; 32 at a time, the 33rd gets 503.
t0=$(date +%s.%N); "${C[@]}" -o /dev/null -r 0-3145727 https://localhost:28443/download/stable/customer/windows-x86_64/TetherQS-9.9.9.exe; t1=$(date +%s.%N)
check "download rate limit (3 MB in >= 3.5 s)" awk -v a="$t0" -v b="$t1" 'BEGIN { exit !(b - a >= 3.5) }'
par=()
for i in $(seq 33); do "${C[@]}" -o /dev/null -w '%{http_code}' https://localhost:28443/download/stable/customer/windows-x86_64/TetherQS-9.9.9.exe > "$W/par$i" & par+=($!); done
sleep 2
check "33rd parallel download: 503, latest.json still served" bash -c "grep -q 503 '$W'/par* && [ \"\$('${C[0]}' ${C[*]:1} -o /dev/null -w '%{http_code}' https://localhost:28443/download/stable/customer/windows-x86_64/latest.json)\" = 200 ]"
kill "${par[@]}" 2>/dev/null; wait "${par[@]}" 2>/dev/null
check "range request (resumed download)" [ "$("${C[@]}" -r 100-199 -o /dev/null -w '%{http_code} %{size_download}' https://localhost:28443/download/stable/customer/windows-x86_64/TetherQS-9.9.9.exe)" = "206 100" ]
echo secret > "$W/api/devices.json"
for u in /download/../devices.json /download/%2e%2e/devices.json /download/..%2fdevices.json; do
  b=$("${C[@]}" --path-as-is "https://localhost:28443$u")
  check "no traversal: $u" bash -c "! grep -q secret <<< \"\$1\"" _ "$b"
done

# ---- /api/: X-Forwarded-For overwritten, bodies buffered, upload ----
hb='{"uuid":"e2e00001-0000-4000-8000-000000000000","id":"123456789","profile":"customer","version":"0.5.21","os":"windows 11","serial":1}'
st=$("${C[@]}" --interface 127.0.0.9 -o /dev/null -w '%{http_code}' -H 'X-Forwarded-For: 6.6.6.6' -H 'X-Real-IP: 6.6.6.6' -H 'Content-Type: application/json' --data "$hb" https://localhost:28443/api/heartbeat)
sleep 0.3
check "heartbeat through nginx ($st)" [ "$st" = 204 ] || [ "$st" = 200 ]
check "/api/policy reaches tether-api (404 no policy)" [ "$("${C[@]}" -o /dev/null -w '%{http_code}' https://localhost:28443/api/policy)" = 404 ]
TK=e2eticket0123456789
echo "[{\"ticket\":\"$TK\",\"note\":\"e2e\",\"created\":$(date +%s),\"uploaded\":null,\"size\":null}]" > "$W/api/diag/tickets.json"
# tether-api writes devices.json when it stops (SIGTERM).
p=$(cat "$W/api.pid"); kill "$p"; for _ in $(seq 100); do [ -d /proc/$p ] || break; sleep 0.1; done
check "tether-api sees the real peer, not the client's X-Forwarded-For" grep -q '"ip": *"127.0.0.9"' "$W/api/devices.json"
ADMIN_PUB_KEY= setsid "$BIN/tether-api" --data "$W/api" --listen 127.0.0.1:25120 > "$W/api.log" 2>&1 & echo $! > "$W/api.pid"
for _ in $(seq 50); do curl -s -o /dev/null 127.0.0.1:25120/api/policy && break; sleep 0.1; done
head -c 1900000 /dev/urandom > "$W/diag.zip"
# A slow client: 1.9 MB at 2 MB/s. nginx holds the whole body before tether-api gets it.
st=$("${C[@]}" -o /dev/null -w '%{http_code}' --limit-rate 2M -H 'Content-Type: application/zip' --data-binary @"$W/diag.zip" "https://localhost:28443/api/diag/$TK")
check "diag upload 1.9 MB ($st), stored intact" bash -c "[ '$st' = 204 ] && cmp -s '$W/diag.zip' '$W/api/diag/$TK.zip'"

# ---- /q/: token check in tether-api, file sent by nginx (X-Accel-Redirect) ----
TOK=abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQ
echo "{\"id\":\"acme\",\"name\":\"ACME\",\"token\":\"$TOK\",\"expires\":$(( $(date +%s) + 86400 )),\"channel\":\"stable\",\"created\":1,\"downloads\":0,\"last_download\":null,\"profile\":{\"x\":1}}" > "$W/api/custom/acme.json"
kill -9 "$(cat "$W/api.pid")"; sleep 0.3
ADMIN_PUB_KEY= setsid "$BIN/tether-api" --data "$W/api" --listen 127.0.0.1:25120 > "$W/api.log" 2>&1 & echo $! > "$W/api.pid"
for _ in $(seq 50); do curl -s -o /dev/null 127.0.0.1:25120/api/policy && break; sleep 0.1; done
h=$("${C[@]}" -D - -o "$W/q.exe" "https://localhost:28443/q/$TOK" | tr -d '\r')
echo "$h" | grep -iE '^(HTTP|content-type|content-disposition|content-length|cache-control|x-accel|x-content)' | sed 's/^/     /'
check "/q/: 200, exe with trailer" bash -c "grep -qE '^HTTP/[0-9.]+ 200' <<< \"\$1\" && cmp -n 6000000 -s '$W/q.exe' '$d/TetherQS-9.9.9.exe' && [ \$(stat -c %s '$W/q.exe') -gt 6000000 ]" _ "$h"
check "/q/: sent by nginx from qcache (X-Accel-Redirect not leaked)" bash -c "ls '$W/api/qcache/' | grep -q acme-- && ! grep -qi '^x-accel' <<< \"\$1\"" _ "$h"
check "/q/: content type, disposition, no-store, nosniff, HSTS" bash -c "grep -qi '^content-type: application/vnd.microsoft.portable-executable' <<< \"\$1\" && grep -qi '^content-disposition: attachment' <<< \"\$1\" && grep -qi '^cache-control: no-store' <<< \"\$1\" && grep -qi '^x-content-type-options: nosniff' <<< \"\$1\" && grep -qi '^strict-transport-security' <<< \"\$1\"" _ "$h"
check "/q/ bad token 404" [ "$("${C[@]}" -o /dev/null -w '%{http_code}' https://localhost:28443/q/wrongtokenwrongtokenwrongtokenwrongtoken123)" = 404 ]
q=$(ls "$W/api/qcache/" | head -1)
for u in "/qcache/$q" "/download/../qcache/$q" "/download/%2e%2e/qcache/$q"; do
  check "qcache not reachable by URL: $u" [ "$("${C[@]}" --path-as-is -o /dev/null -w '%{http_code}' "https://localhost:28443$u")" = 404 ]
done

# ---- /grafana/ behind basic auth ----
check "/grafana redirects to /grafana/" grep -qE "^308 https://localhost(:28443)?/grafana/$" <<< "$("${C[@]}" -o /dev/null -w "%{http_code} %{redirect_url}" https://localhost:28443/grafana)"
check "/grafana/ without password: 401" [ "$("${C[@]}" -o /dev/null -w '%{http_code}' https://localhost:28443/grafana/)" = 401 ]
check "/grafana/ wrong password: 401" [ "$("${C[@]}" -u tether:nope -o /dev/null -w '%{http_code}' https://localhost:28443/grafana/)" = 401 ]
b=$("${C[@]}" -u tether:grafana-secret https://localhost:28443/grafana/d/x)
check "/grafana/ with password reaches Grafana, Authorization stripped" bash -c "grep -q '^path=/grafana/d/x' <<< \"\$1\" && ! grep -qi '^authorization' <<< \"\$1\"" _ "$b"

# ---- /_metrics/<job>: source IP + basic auth ----
check "/_metrics/hbbs from the scraper with password" grep -q '^hbbs_' <<< "$("${C[@]}" -u prometheus:scrape-secret https://localhost:28443/_metrics/hbbs)"
check "/_metrics/hbbr, /_metrics/api" bash -c "'${C[0]}' ${C[*]:1} -u prometheus:scrape-secret https://localhost:28443/_metrics/hbbr | grep -q '^hbbr_' && '${C[0]}' ${C[*]:1} -u prometheus:scrape-secret https://localhost:28443/_metrics/api | grep -q '^tether_api_'"
check "/_metrics/nginx (exporter)" grep -q '^nginx_connections_active' <<< "$("${C[@]}" -u prometheus:scrape-secret https://localhost:28443/_metrics/nginx)"
check "/_metrics/probe keeps the query" grep -q '^path=/probe?module=ws&target=x' <<< "$("${C[@]}" -u prometheus:scrape-secret 'https://localhost:28443/_metrics/probe?module=ws&target=x')"
check "/_metrics/node" grep -q '^path=/metrics' <<< "$("${C[@]}" -u prometheus:scrape-secret https://localhost:28443/_metrics/node)"
# The echo upstream shows what nginx forwards: client-sent IP headers replaced by the peer,
# X-Tether-Accel never passed on outside /q/ (the server-level proxy_set_header set is inherited).
b=$("${C[@]}" -u prometheus:scrape-secret -H 'X-Forwarded-For: 6.6.6.6' -H 'X-Real-IP: 6.6.6.6' -H 'X-Tether-Accel: 1' https://localhost:28443/_metrics/node)
check "forwarded headers: X-Forwarded-For / X-Real-IP = peer, no client X-Tether-Accel" bash -c "grep -qx 'X-Forwarded-For: 127.0.0.1' <<< \"\$1\" && grep -qx 'X-Real-IP: 127.0.0.1' <<< \"\$1\" && ! grep -qi 'tether-accel\|6.6.6.6' <<< \"\$1\"" _ "$b"
check "/_metrics/ without password: 401" [ "$("${C[@]}" -o /dev/null -w '%{http_code}' https://localhost:28443/_metrics/hbbs)" = 401 ]
check "/_metrics/ from another IP, right password: 403" [ "$("${C[@]}" --interface 127.0.0.2 -u prometheus:scrape-secret -o /dev/null -w '%{http_code}' https://localhost:28443/_metrics/hbbs)" = 403 ]
check "/_metrics/unknown: 404" [ "$("${C[@]}" -u prometheus:scrape-secret -o /dev/null -w '%{http_code}' https://localhost:28443/_metrics/x)" = 404 ]
check "exporter on localhost: requests counted" grep -q '^nginx_http_requests_total [1-9]' <<< "$(curl -s 127.0.0.1:25125/metrics)"
check "stub_status not public" bash -c "! '${C[0]}' ${C[*]:1} https://localhost:28443/stub_status | grep -q 'Active connections'"

# ---- WebSockets through nginx (client TLS layer: HAProxy, as in the benchmark) ----
sed -e "s#@W@#$W#g" "$HERE/conf/client-haproxy.cfg" | sed -e 's/nbthread 4/nbthread 2/' -e 's/maxconn 400000/maxconn 20000/g' > "$W/client.cfg"
setsid "$HAPROXY" -f "$W/client.cfg" > "$W/client.log" 2>&1 & echo $! > "$W/client.pid"
sleep 0.5
K="$(cat "$W/hb/id_ed25519.pub")"
# 200 hosts for 40 s with 5 viewers/s; nginx is reloaded (SIGHUP) twice in the middle.
"$BIN/tether-load" mode=hosts key="$K" url=ws://127.0.0.1:28081/ws/id n=200 rate=200 ips=20 probe=5 seconds=40 threads=2 report=40 > "$W/hosts.jsonl" &
lg=$!
sleep 12; kill -HUP "$(cat "$W/nginx.sid")"; sleep 10; kill -HUP "$(cat "$W/nginx.sid")"
wait $lg
f=$(tail -n 1 "$W/hosts.jsonl")
echo "     $f" | cut -c1-400
check "200 WebSocket hosts registered through nginx" grep -q '"registered":200,' <<< "$f"
check "two graceful reloads: no WebSocket dropped" grep -q '"closed":0,.*"silent_drops":0' <<< "$f"
check "viewers reached hosts (PunchHole + RelayResponse)" bash -c "grep -q '\"punch_fail\":0' <<< \"\$1\" && ! grep -q '\"rr_ok\":0,' <<< \"\$1\"" _ "$f"
# relay-harness: host + viewer pairs through hbbs and hbbr, both behind nginx.
"$BIN/relay-harness" hbbs=ws://127.0.0.1:28081/ws/id key="$K" sessions=4 seconds=15 > "$W/harness.json" 2>"$W/harness.log"
echo "     $(head -c 400 "$W/harness.json")"
# hbbs hands out ws://127.0.0.1:28081/ws/relay: the pairs reach hbbr through nginx too.
check "relay-harness through nginx: 4 sessions, frames delivered, no breaks" python3 -I -c 'import json,sys; r=json.load(open(sys.argv[1])); s=r["per_session"]; sys.exit(0 if r["sessions"] == 4 and r["incomplete_sessions"] == 0 and r["reconnects"] == 0 and all(x["frames"] > 100 for x in s) else 1)' "$W/harness.json"

stop
echo "passed $pass, failed $fails"
[ "$fails" = 0 ]

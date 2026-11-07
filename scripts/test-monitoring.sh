#!/usr/bin/env bash
# Monitoring end to end on localhost (WSL/Linux), with the binaries from scripts/build-relay.sh and
# the official Prometheus/Alertmanager/node_exporter/blackbox_exporter/Grafana/Caddy releases
# unpacked under TOOLS (one dir each, as scripts/fetch-monitoring-tools.sh leaves them):
#   scripts/test-monitoring.sh [build-dir] [tools-dir] [seconds]
# One machine plays both nodes: node "a" is scraped over localhost, node "b" through Caddy's
# /_metrics/* (basic auth, IP allow-list) exactly like the other node in production. Starts
# hbbs + primary and backup hbbr + tether-api + Caddy (tls internal on :18443), generates load
# with relay-harness and API requests, fails the primary relay over and back, then checks:
# every target up, every dashboard panel query returns data, the alert rules evaluate, and
# promtool's rule unit tests. Leaves everything running with KEEP=1 (Grafana on :3000).
set -euo pipefail
SRC="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:-$HOME/mon-build}"
TOOLS="${2:-$HOME/mon-tools}"
SECS="${3:-150}"
BIN="$OUT/bin"
M="$SRC/deploy/monitoring"
W="$HOME/mon-e2e"
tool() { find "$TOOLS" -maxdepth 4 -type f -name "$1" -perm -u+x | head -n1; }
PROM="$(tool prometheus)" PROMTOOL="$(tool promtool)" AM="$(tool alertmanager)" NODEX="$(tool node_exporter)"
BBX="$(tool blackbox_exporter)" CADDY="$(tool caddy)" GRAFANA="$(tool grafana)"
for t in PROM PROMTOOL AM NODEX BBX CADDY GRAFANA; do [ -n "${!t}" ] || { echo "missing $t under $TOOLS" >&2; exit 1; }; done

pkill -f "$W/" 2>/dev/null || true
pkill -f "$TOOLS/" 2>/dev/null || true  # a stack left by an earlier KEEP=1 run
pkill -f "$BIN/" 2>/dev/null || true
sleep 1
rm -rf "$W"
mkdir -p "$W"/{etc/rules,caddy.d,api,hbbs,prom,am,grafana,logs,xdg}
cd "$W"
pids=()
cleanup() { [ "${KEEP:-0}" = 1 ] || { kill "${pids[@]}" 2>/dev/null || true; wait 2>/dev/null || true; }; }
trap cleanup EXIT
run() { local name=$1; shift; "$@" >"$W/logs/$name.log" 2>&1 & pids+=($!); }

# ---- the relay ----
# Relay ports still in TIME_WAIT from an earlier run (sessions closed by a kill) cannot be bound.
busy() { ss -tan | awk '{print $4}' | grep -qE ":($1)\$"; }
for _ in $(seq 90); do busy "2111[5-9]|2112[0-4]|4731[79]|47321" || break; sleep 1; done
(cd hbbs && RELAY_HOST=localhost TURN_PUBLIC_IP=127.0.0.1 TURN_ALLOW_PRIVATE_PEERS=1 HEALTH_PORT=21122 \
  exec "$BIN/hbbs" -k _ -r "ws://127.0.0.1:21119|http://127.0.0.1:21121/readyz,ws://127.0.0.1:47319|http://127.0.0.1:47321/readyz|backup") >logs/hbbs.log 2>&1 &
pids+=($!)
for _ in $(seq 50); do [ -s hbbs/id_ed25519.pub ] && break; sleep 0.1; done
KEY="$(cat hbbs/id_ed25519.pub)"
# (Re)start the primary hbbr, retrying while its ports are still in TIME_WAIT.
start_primary() {
  for _ in $(seq 40); do
    (cd hbbs && exec "$BIN/hbbr" -k _) >>logs/hbbr.log 2>&1 &
    primary=$!
    sleep 2
    kill -0 "$primary" 2>/dev/null && break
  done
  pids+=($primary)
}
start_primary
(cd hbbs && HEALTH_PORT=47321 METRICS_PORT=0 exec "$BIN/hbbr" -p 47317 -k _) >logs/hbbr-backup.log 2>&1 &
pids+=($!)
[ -f "$HOME/e2e-api/d1/policy.json" ] && cp "$HOME/e2e-api/d1/policy.json" api/  # a signed test policy, if one is around
run api "$BIN/tether-api" --data "$W/api" --listen 127.0.0.1:21120

# ---- Caddy: the relay's Caddyfile, on :18443 with its own CA, plus the monitoring snippets ----
PASS=e2e-scrape-pass WEBPASS=e2e-web-pass
HASH="$("$CADDY" hash-password --plaintext "$PASS")" WEBHASH="$("$CADDY" hash-password --plaintext "$WEBPASS")"
sed -e "s|@SCRAPE_FROM@|127.0.0.1|; s|@SCRAPE_HASH@|$HASH|" "$M/caddy/metrics-remote.caddy" > caddy.d/metrics-remote.caddy
sed -e "s|@GRAFANA_USER@|tether|; s|@GRAFANA_HASH@|$WEBHASH|" "$M/caddy/grafana.caddy" > caddy.d/grafana.caddy
perl -0pe "s/\tadmin off\n/\tadmin off\n\thttps_port 18443\n\thttp_port 18080\n\tlocal_certs\n\tskip_install_trust\n/; s/\ttls \{.*?\n\t\}\n/\ttls internal\n/s; s|/etc/tether/caddy.d/|$W/caddy.d/|; s|/var/lib/tether-api|$W/api|g" \
  "$SRC/server/caddy/Caddyfile" > Caddyfile
RELAY_HOST=localhost XDG_DATA_HOME="$W/xdg" XDG_CONFIG_HOME="$W/xdg" "$CADDY" validate --config Caddyfile --adapter caddyfile >logs/caddy-validate.log 2>&1
RELAY_HOST=localhost XDG_DATA_HOME="$W/xdg" XDG_CONFIG_HOME="$W/xdg" run caddy "$CADDY" run --config "$W/Caddyfile" --adapter caddyfile
CA="$W/xdg/caddy/pki/authorities/local/root.crt"
for _ in $(seq 100); do [ -s "$CA" ] && break; sleep 0.1; done

# ---- exporters ----
run node "$NODEX" --web.listen-address=127.0.0.1:9100 --collector.systemd \
  '--collector.systemd.unit-include=(tether-.*|prometheus|alertmanager|grafana-server|node_exporter|blackbox_exporter|fail2ban|firewalld|sshd|dnf-automatic.*|nightly-reboot|systemd-journald|cron)\.(service|timer)'
sed "s/@PEER_HOST@/localhost/" "$M/blackbox/blackbox.yml" > etc/blackbox.yml
SSL_CERT_FILE="$CA" run blackbox "$BBX" --config.file="$W/etc/blackbox.yml" --web.listen-address=127.0.0.1:9115

# ---- Prometheus + Alertmanager: the production files, filled in for localhost ----
echo -n "$PASS" > etc/scrape.pass
sed -e "s|@ETC@|$W/etc|g; s|@LOCAL@|a|g; s|@REMOTE@|b|g; s|@LOCAL_HOST@|localhost:18443|g; s|@REMOTE_HOST@|localhost:18443|g; s|@REMOTE_ADDR@|localhost:18443|g; s|@LOCAL_PROXY@|caddy|g; s|@REMOTE_PROXY@|caddy|g" \
    -e "s|scheme: @REMOTE_SCHEME@|scheme: https\n    tls_config: { ca_file: $CA }|" \
    -e "s|localhost:18443:2111\([67]\)|127.0.0.1:2111\1|; s|localhost:18443:443|localhost:18443|" \
    -e "s|scrape_interval: 30s|scrape_interval: 5s|; s|scrape_timeout: 10s|scrape_timeout: 4s|; s|evaluation_interval: 30s|evaluation_interval: 5s|" \
    "$M/prometheus/prometheus.yml" > etc/prometheus.yml
cp "$M/prometheus/rules/tether.yml" etc/rules/
"$PROMTOOL" check config etc/prometheus.yml
"$PROMTOOL" test rules "$M/prometheus/rules/rules.test.yml"
sed '/^  - name: telegram$/,$d' "$M/alertmanager/alertmanager.yml" > etc/alertmanager.yml
"$(dirname "$AM")/amtool" check-config etc/alertmanager.yml >/dev/null
run am "$AM" --config.file=etc/alertmanager.yml --storage.path=am --web.listen-address=127.0.0.1:9093 --cluster.listen-address=
run prom "$PROM" --config.file=etc/prometheus.yml --storage.tsdb.path=prom --web.listen-address=127.0.0.1:9090

# ---- Grafana with the production env file and provisioning ----
mkdir -p etc/grafana/provisioning/{datasources,dashboards,plugins,alerting,notifiers} etc/grafana/dashboards
echo -n admin-e2e > etc/grafana-admin.pass
sed -e "s|@ETC@|$W/etc|g; s|@HOST@|localhost:18443|g" "$M/grafana/grafana.env" > etc/grafana.env
for f in datasources/tether.yaml dashboards/tether.yaml; do sed "s|@ETC@|$W/etc|g" "$M/grafana/provisioning/$f" > "etc/grafana/provisioning/$f"; done
cp "$M"/grafana/dashboards/*.json etc/grafana/dashboards/
GH="$(cd "$(dirname "$GRAFANA")/.." && pwd)"
( set -a; . etc/grafana.env; set +a; GF_PATHS_DATA="$W/grafana" GF_PATHS_LOGS="$W/grafana" GF_PATHS_PLUGINS="$W/grafana/plugins" \
  exec "$GRAFANA" server --homepath "$GH" ) >logs/grafana.log 2>&1 &
pids+=($!)

# ---- load ----
sleep 5
run harness "$BIN/relay-harness" hbbs=ws://127.0.0.1:21118 key="$KEY" sessions=4 seconds="$SECS" session_seconds=20 frame_ms=10 frame_bytes=4096
R=(--resolve localhost:18443:127.0.0.1)
api() { curl -s -o /dev/null --cacert "$CA" "${R[@]}" "$@"; }
(
  i=0
  while [ $i -lt $((SECS / 2)) ]; do
    i=$((i + 1))
    api -X POST -H 'Content-Type: application/json' -d "{\"uuid\":\"e2e-device-$i\",\"id\":\"1$i\",\"profile\":\"customer\",\"version\":\"0.5.21\",\"os\":\"Linux\",\"serial\":0}" https://localhost:18443/api/heartbeat
    api https://localhost:18443/api/policy; api https://localhost:18443/api/server-keys
    api -X POST -d '{}' https://localhost:18443/api/audit/conn; api https://localhost:18443/q/nonexistent
    api https://localhost:18443/readyz/relay
    api https://localhost:18443/api/admin/status  # 503: admin API off (a 5xx for the dashboards)
    api -X POST -H "Content-Type: application/json" -d "{\"uuid\":\"e2e-burst-$i\",\"id\":\"2$i\",\"profile\":\"customer\",\"version\":\"0.5.21\",\"os\":\"Linux\",\"serial\":0}" https://localhost:18443/api/heartbeat  # over 30/min: 429s
    sleep 2
  done
) >/dev/null 2>&1 &
pids+=($!)
echo "load running for ${SECS}s; primary relay stopped from 40 s to 85 s"
sleep 40
kill "$primary"; wait "$primary" 2>/dev/null || true
sleep 45
start_primary
sleep $((SECS - 85))
sleep 15  # last scrapes

# ---- checks ----
fail=0
echo "== Caddy /_metrics: refused without password, closed for other IPs"
curl -s -o /dev/null --cacert "$CA" --resolve localhost:18443:[::1] -u prometheus:$PASS https://localhost:18443/_metrics/node && { echo "FAIL: /_metrics answered ::1 (not the allowed IP)"; fail=1; }
code="$(curl -s -o /dev/null -w '%{http_code}' --cacert "$CA" "${R[@]}" https://localhost:18443/_metrics/node)"; [ "$code" = 401 ] || { echo "FAIL: /_metrics without auth -> $code"; fail=1; }
code="$(curl -s -o /dev/null -w '%{http_code}' --cacert "$CA" "${R[@]}" -u prometheus:$PASS https://localhost:18443/_metrics/node)"; [ "$code" = 200 ] || { echo "FAIL: /_metrics with auth -> $code"; fail=1; }
code="$(curl -s -o /dev/null -w '%{http_code}' --cacert "$CA" "${R[@]}" -u tether:$WEBPASS https://localhost:18443/grafana/login)"; [ "$code" = 200 ] || { echo "FAIL: /grafana/ through Caddy -> $code"; fail=1; }
code="$(curl -s -o /dev/null -w '%{http_code}' --cacert "$CA" "${R[@]}" https://localhost:18443/grafana/login)"; [ "$code" = 401 ] || { echo "FAIL: /grafana/ without Caddy password -> $code"; fail=1; }
code="$(curl -s -o /dev/null -w '%{http_code}' --cacert "$CA" "${R[@]}" https://localhost:18443/metrics)"; [ "$code" = 200 ] && curl -s --cacert "$CA" "${R[@]}" https://localhost:18443/metrics | grep -q '^# TYPE' && { echo "FAIL: metrics public"; fail=1; }
curl -s http://127.0.0.1:21120/metrics | grep -q '^tether_api_requests_total' || { echo "FAIL: tether-api /metrics"; fail=1; }
python3 -I - "$M/grafana/dashboards" <<'EOF' || fail=1
import json, sys, os, urllib.request, urllib.parse
P = "http://127.0.0.1:9090/api/v1/"
def get(path, **q):
    with urllib.request.urlopen(P + path + "?" + urllib.parse.urlencode(q), timeout=20) as r:
        return json.load(r)["data"]
bad = 0
print("== targets")
for t in get("targets")["activeTargets"]:
    ok = t["health"] == "up"
    bad += not ok
    print(f'  {"ok  " if ok else "DOWN"} {t["labels"]["job"]:<18} node={t["labels"].get("node")} {t["scrapeUrl"][:90]} {t.get("lastError","")}')
for r in get("query", query="scrape_samples_scraped == 0")["result"]:
    bad += 1
    print(f'  EMPTY {r["metric"]["job"]:<18} node={r["metric"].get("node")} scrape returned no samples')
print("== dashboard panel queries (last 3 minutes)")
for f in sorted(os.listdir(sys.argv[1])):
    d = json.load(open(os.path.join(sys.argv[1], f)))
    for p in d["panels"]:
        for t in p.get("targets", []):
            e = t["expr"].replace("$node", ".*").replace("$__rate_interval", "1m")
            res = get("query", query=e)["result"]
            # a range panel shows data if the series had a value at any point of the window
            if not res and p["type"] != "stat":
                r = get("query_range", query=e, start=f'{__import__("time").time()-180}', end=f'{__import__("time").time()}', step="15")
                res = r["result"]
            ok = bool(res)
            # systemd units of a relay node (tether-*, firewalld, ...) do not exist on the test machine
            # the test stack runs Caddy; the nginx panels fill on nodes installed with PROXY=nginx
            if not ok and p["title"].startswith("nginx "):
                print(f"  skip  {d['uid']:<17} {p['title'][:42]} (front end here is Caddy)"); continue
            if not ok and p["title"] == "Service units":
                print(f"  skip  {d['uid']:<17} Service units (no relay units here)"); continue
            bad += not ok
            print(f'  {"ok   " if ok else "EMPTY"} {d["uid"]:<17} {p["title"][:42]:<42} {t["legendFormat"][:30]}')
print("== alert rules")
for g in get("rules")["groups"]:
    for r in g["rules"]:
        ok = r["health"] == "ok"
        bad += not ok
        print(f'  {"ok " if ok else "ERR"} {r["name"]:<24} {r["state"]:<8} {r.get("lastError","")}')
alerts = {a["labels"]["alertname"] for a in get("alerts")["alerts"]}
hist = get("query", query='max_over_time(ALERTS{alertname="RelayFailover"}[10m])')["result"]
print("  firing/pending now:", sorted(alerts))
print("  RelayFailover during the failover:", "yes" if hist else "NO")
bad += "Watchdog" not in alerts or not hist
with urllib.request.urlopen("http://127.0.0.1:9093/api/v2/alerts", timeout=10) as r:
    am = {a["labels"]["alertname"] for a in json.load(r)}
print("  Alertmanager has:", sorted(am))
bad += "Watchdog" not in am
print("== result:", "all passed" if not bad else f"{bad} problem(s)")
sys.exit(1 if bad else 0)
EOF
echo "== Grafana"
G=http://localhost:3000/grafana
code="$(curl -s -o /dev/null -w '%{http_code}' $G/api/health || true)"; [ "$code" = 200 ] || { echo "FAIL: grafana health -> $code"; fail=1; }
code="$(curl -s -o /dev/null -w '%{http_code}' "$G/api/search" || true)"; [ "$code" = 401 ] || { echo "FAIL: anonymous search -> $code"; fail=1; }
code="$(curl -s -o /dev/null -w '%{http_code}' -u admin:admin-e2e "$G/api/search" || true)"; [ "$code" = 401 ] || { echo "FAIL: basic auth to Grafana accepted -> $code"; fail=1; }
curl -s -c "$W/jar" -o /dev/null -H 'Content-Type: application/json' -d '{"user":"admin","password":"admin-e2e"}' $G/login || true
n="$(curl -s -b "$W/jar" "$G/api/search?tag=tether" | grep -o '"type":"dash-db"' | wc -l || true)"
[ "$n" -ge 6 ] || { echo "FAIL: provisioned dashboards: $n"; fail=1; }
echo "grafana dashboards provisioned: $n"
if [ $fail = 0 ]; then echo "monitoring e2e: all passed"; else echo "monitoring e2e: FAILED (logs in $W/logs)"; fi
if [ "${KEEP:-0}" = 1 ]; then echo "KEEP=1: stack running (Grafana http://127.0.0.1:3000/grafana/, admin / admin-e2e); Ctrl-C ends it"; wait; fi
exit $fail

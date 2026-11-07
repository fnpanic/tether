# HTTPS front end: Caddy alternatives, benchmark and choice

The scaling work ([scaling.md](scaling.md)) found the TLS/WebSocket front end, not hbbs or hbbr,
to be the limit of a relay node: Caddy needs ~145 KB per idle TLS WebSocket (10 000 devices =
1.4 GB) and ~3.3 cores per relayed Gbit/s. This page compares the alternatives on the same machine
with the same method, checks each against every feature Tether uses, and prepares the winner.

**Result: nginx (mainline from nginx.org, with its official ACME module) replaces Caddy as a full
drop-in**, selectable per node with `PROXY=nginx` in `server/install.sh`; Caddy stays the default
until a node is switched. Per idle TLS WebSocket nginx needs **41 KB instead of Caddy's 111 KB**
(2.7× less) and 45 % less CPU, it covers every route (static files and `X-Accel-Redirect`
natively, TLS-ALPN-01 certificates in-process like Caddy), reloads without dropping WebSockets
and is patched by `dnf` from nginx.org's signed repository. HAProxy needs only 20 KB per
WebSocket but cannot serve files, has no official EL9 packages of 3.x and its ACME client is
experimental: it would need a second server next to it for `/download/` and `/q/`.

**What the benchmark also showed**: relaying costs every candidate about the same, 2.1–2.8 PC
cores per Gbit/s at 1 Gbit/s (Caddy 2.5, nginx 2.1, HAProxy 2.1, HAProxy in pure TCP mode 2.1),
because TLS and the socket copies dominate, not HTTP. The "3.3 cores per Gbit/s" of scaling.md
was a 0.5 Gbit/s run, where fixed costs weigh more (Caddy 3.5, nginx 2.6 there). Changing the
proxy therefore fixes the **signalling** cost (memory and idle CPU per device), not the relay CPU.
Tuning Caddy helps little: 2.11's `stream_buffer_size 4096` saves 4 % memory, `GOGC=50` another
9 %, both at ~45 % more CPU.

| Per front node | Caddy 2.10 (today) | Caddy 2.11 tuned | **nginx 1.31** | HAProxy 3.4 | Envoy 1.40 | Traefik 3.7 |
|---|---|---|---|---|---|---|
| memory per idle TLS WebSocket | 111 KB | 97–107 KB | **41 KB** | 20 KB | 55 KB | 116 KB |
| 10 000 idle WebSockets: memory / CPU | 1.16 GB / 76 % | 1.02–1.12 GB / 109–112 % | **0.47–0.57 GB / 43–48 %** | 0.27 GB / 48 % | 0.61 GB / 83 % | 1.25 GB / 115 % |
| relay, front cores per Gbit/s at 0.5 / 1 Gbit/s | 3.5 / 2.5 | 3.5 / 2.5 | **2.6 / 2.1** | 2.7 / 2.1 | 3.4 / 2.8 | 3.6 / 2.5 |
| front + hbbr per Gbit/s at 1 Gbit/s | 3.3 | 3.3 | **3.2** | 3.2 | 3.8 | 3.3 |
| 10 000 new TLS WebSockets in 1 s: front CPU-seconds | 12 | – | **7.8** | 11 | 9 | 14 |
| downloads, cores per Gbit/s (TLS) | 0.08 | – | **0.10** | 0.13¹ | 0.14¹ | 0.13¹ |
| API at 1 500 req/s: front CPU / p99 | 42 % / 1.1 ms | – | **12 % / 1.0 ms** | 15 % / 1.2 ms | 26 % / 0.9 ms | 38 % / 1.1 ms |
| every route built in | ✓ | ✓ | **✓** | ✗ files, X-Accel | ✗ files, X-Accel, bcrypt | ✗ files, X-Accel |
| reload keeps WebSockets | ✗ (admin API off) | ✗ | **✓** | ✓ | ✓ | ◐ |

¹ including the local nginx that serves the files for it. CPU in % of one PC core (i5-13600KF);
memory is PSS. Details, latencies and caveats below.

## Candidates and versions

All checked on 2026-11-07, each the newest release at least two weeks old (except where noted):

| Candidate | Version | How obtained here | Production source | Licence |
|---|---|---|---|---|
| Caddy (today) | 2.10.2, and 2.11.7 | official static release (SHA-512 checked) | same | Apache-2.0 |
| Caddy tuned | 2.11.7 + `stream_buffer_size 4096` (new in 2.11) [+ `GOGC=50`] | same | same | Apache-2.0 |
| nginx mainline | 1.31.7 (2026-10-01) + nginx-module-acme 0.4.1 | built from the nginx.org source for the benchmark; the nginx.org `noble` packages for the end-to-end test | nginx.org RPM repository for RHEL 9 (signed), `dnf` updates | BSD-2-Clause (module Apache-2.0) |
| freenginx | 1.31.5 | not benchmarked (same code base as nginx up to 2024) | source only | BSD-2-Clause |
| HAProxy | 3.4.7 LTS (2026-10-14, supported to Q2 2031) | built from source (OpenSSL 3.0, `USE_PROMEX`) | no official EL9 packages of 3.x (EL9 ships 2.4) | GPL-2.0 |
| Envoy | 1.40.1 (2026-10-23) | official static binary (BoringSSL) | same | Apache-2.0 |
| Traefik | 3.7.15 (2026-10-15) | official release (SHA-256 checked) | same | MIT |
| hbbr/hbbs terminating TLS | – | effort and gain estimated from the HAProxy TCP-mode run | code change | AGPL (rustdesk-server) |

The 1.31.7 security release (2026-10-01, five medium CVEs) is why nginx is the only candidate a
few weeks younger than the two-week rule: every older mainline is vulnerable.

## Method

Same machine and method as [scaling.md](scaling.md#method): i5-13600KF, WSL2 Ubuntu 24.04 (12 CPUs,
16 GB), everything on loopback, CPU as % of one PC core (5 s samples, `tools/load/sample.sh`, now
summing a process and its children, e.g. nginx's workers), memory as **PSS** (shared pages counted
once across nginx's workers; for single-process servers PSS ≈ RSS). One candidate at a time, a
fresh stack per run, every process killed after it.

- `tools/load/stack.sh` with `FRONT=<name>` starts hbbs, hbbr, tether-api and the front under test
  (`tools/proxy-bench/front.sh`, configs in `tools/proxy-bench/conf/`): TLS on 127.0.0.1:28443
  with the same ECDSA P-256 certificate for every candidate, TLS 1.2/1.3 AEAD ciphers, HTTP/2
  offered, the production routes for `/ws/id`, `/ws/relay`, `/api/`, `/download/`.
- The client-side TLS layer is **HAProxy** for every candidate (`conf/client-haproxy.cfg`): the
  load generator speaks plain HTTP/WebSocket to it, it speaks TLS 1.3 to the front. It replaces
  the client-side Caddy of scaling.md, which would have needed another 4 GB for 30 000 idle
  connections. The Caddy baseline was re-measured with it (below).
- `tools/proxy-bench/run.sh <front> <test> <n>` runs one test, `summary.sh` prints steady-state
  CPU (mean of the second half of the samples) and peak PSS. Raw results:
  `tools/proxy-bench/results/*.jsonl`.
- Front ends without a file server (HAProxy, Envoy, Traefik) pass `/download/` to a one-worker
  local nginx on 28126, whose CPU is reported separately ("files").
- hbbs's per-IP limits are lifted for the runs (`IP_BLOCK_*`): every host reaches hbbs from the
  front's 127.0.0.1. WSL's ephemeral port range was widened to 10000–65000 (test ports reserved)
  for 30 000 connections per hop; `somaxconn` 16384.
- Limits: loopback, one PC for load and servers (they share CPUs: at 2 Gbit/s the PC itself is
  near saturation), OpenSSL 3.0.13 for nginx/HAProxy (EL9.6+ ships 3.5, which is faster for
  HAProxy's locking; nginx is unaffected), 45–90 s runs.

Tests: (a) idle TLS WebSockets on `/ws/id` (each host pings every second, as the client does),
10 000 and 30 000, plus a 1-host run for the fixed cost; (b) relay through `/ws/relay`, 500 / 1 000 /
2 000 pairs at 1 Mbit/s + 50 kbit/s (0.5 / 1 / 2 Gbit/s), 30 frames/s, frame latency p50/p99;
(c) connection storm: 10 000 hosts connect within one second (10 000 TLS handshakes + WebSocket
upgrades + registrations); (d) 100 parallel downloads of a 35 MB file from `/download/`;
(e) API: 10 000 devices run their whole cycle (server-keys, policy, heartbeat) within 20 s
(~1 500 req/s) through the front.

Reproduce (in WSL, binaries under `~/proxy-bench` as in the table above, relay binaries from
`scripts/build-relay.sh`; one run at a time, ~1–2 minutes each):

```sh
R=tools/proxy-bench/run.sh
for f in caddy nginx haproxy envoy traefik; do
  $R $f idle 1; $R $f idle 10000; $R $f relay 500; $R $f relay 1000; $R $f relay 2000
  $R $f storm 10000; $R $f dl 100; $R $f api 10000
done
$R haproxy idle 30000; $R nginx idle 30000; $R haproxy-l4 relay 1000
WORKERS=4 TAG=-w4 $R nginx idle 10000                      # nginx worker count
NGINX_EXTRA="ssl_buffer_size 4k;" TAG=-sb4k $R nginx idle 10000
CADDY=~/proxy-bench/dl/c211/caddy TAG=-2.11 $R caddy idle 10000
CADDY=~/proxy-bench/dl/c211/caddy CADDYFILE=$PWD/tools/proxy-bench/conf/Caddyfile.tuned [CADDY_ENV=GOGC=50] TAG=-tuned $R caddy idle 10000
```

## Results

Raw data: `tools/proxy-bench/results/<front>[-tag]-<test>-<n>.{load,sample}.jsonl`.

### (a) Idle TLS WebSockets on `/ws/id`

Each host registers and pings every second (10 000 pings and pongs per second each way through
the proxy). Fixed cost from a 1-host run; per WebSocket = (PSS at n − fixed) / n.

| Front | fixed | 10 000: PSS / CPU | 30 000: PSS / CPU | per WebSocket | ping RTT p99 at 10k / 30k |
|---|---|---|---|---|---|
| Caddy 2.10.2 | 46 MB | 1 158 MB / 76 % | not run (≈ 3.4 GB) | **111 KB** | 0.6 ms |
| Caddy 2.11.7 | 46 MB | 1 276 MB / 117 % | – | 123 KB | |
| Caddy 2.11.7 + `stream_buffer_size 4096` | 46 MB | 1 117 MB / 109 % | – | 107 KB | 1.6 ms |
| same + `GOGC=50` | 46 MB | 1 017 MB / 112 % | – | 97 KB | 1.7 ms |
| nginx, 12 workers | 170 MB | 572 MB / 43 % | 1 391 MB / 117–132 % | **40.5 KB** | 0.4 / 1.8 ms |
| nginx, 4 workers | 61 MB | 471 MB / 48 % | 1 305 MB / 102 % | **41.5 KB** | 1.1 / 1.8 ms |
| nginx, 12 workers, `ssl_buffer_size 4k` (production) | 170 MB | 530 MB / 61 % | 1 247 MB / 123 % | **36 KB** | |
| HAProxy 3.4.7 | 67 MB | 265 MB / 48 % | 658 MB / 130 % | **19.7 KB** | 0.5 / 1.7 ms |
| Envoy 1.40.1 | 65 MB | 611 MB / 83 % | – | 54.6 KB | 1.2 ms |
| Traefik 3.7.15 | 91 MB | 1 249 MB / 115 % | – | 116 KB | 1.5 ms |
| (hbbs behind any of them) | | 238 MB / 20–33 % | 696 MB / 62–67 % | 24 KB | |

- nginx's fixed cost is mostly its per-worker connection table (`worker_connections`, ~14 MB
  per worker at 32 768 here); production uses `auto` workers (one per core).
- `ssl_buffer_size 4k` (TLS records of 4 KB instead of 16 KB) saves 5 KB per WebSocket and costs
  nothing on the relay (1 000 pairs: 207 % vs 210 %), so the production config sets it. The
  planning numbers keep 41 KB.
- nginx runs show ~20 failed connects at the very start of the ramp (retried at once). One run
  (12 workers, 30 000) had 7 241 failed connects and 122 three-second reconnects while its ramp
  passed ~28 700 connections, with the client layer at 300–450 % CPU; all 30 000 connected in the
  end. Two repeats (4 workers; 12 workers again) had 22 and 20. Not explained; see "Not verified".
- Caddy 2.11.7 needs more memory than 2.10.2 (123 vs 111 KB); `stream_buffer_size` brings it back
  to 2.10's level, not below.
- Caddy: scaling.md had 1 415–1 430 MB at 10 000 (RSS, client-side Caddy); here 1 158 MB PSS with
  the HAProxy client layer. Same build; the difference was not investigated further.

### (b) Relay through `/ws/relay`

Steady state (middle report of the run), 1 Mbit/s + 50 kbit/s per pair, 30 frames/s:

| Front | 500 pairs (0.5 Gbit/s): front / hbbr CPU | p50 / p99 | 1 000 pairs: front / hbbr | p50 / p99 | 2 000 pairs: front / hbbr | p50 / p99 |
|---|---|---|---|---|---|---|
| Caddy 2.10.2 | 177 % / 51 % | 0.5 / 1.7 ms | 248 % / 78 % | 0.7 / 2.1 ms | 415 % / 173 % | 46 / 140 ms² |
| Caddy 2.11.7 tuned | 177 % / 50 % | 0.5 / 1.5 ms | 249 % / 77 % | 0.7 / 2.1 ms | 417 % / 174 % | 48 / 150 ms² |
| nginx | 130 % / 69 % | 0.4 / 2.2 ms | 210 % / 108 % | 0.7 / 2.5 ms | 440 % / 199 % | 54 / 160 ms² |
| HAProxy (HTTP mode) | 135 % / 65 % | 0.4 / 2.2 ms | 213 % / 102 % | 0.7 / 3.8 ms | 453 % / 187 % | 53 / 160 ms² |
| HAProxy TCP mode (TLS only, no HTTP) | 132 % / 67 % | 0.4 / 1.8 ms | 206 % / 101 % | 0.7 / 2.6 ms | 426 % / 187 % | 51 / 180 ms² |
| Envoy | 168 % / 63 % | 0.5 / 1.9 ms | 276 % / 99 % | 0.8 / 5.5 ms | 547 % / 176 % | 59 / 160 ms² |
| Traefik | 182 % / 51 % | 0.5 / 1.5 ms | 253 % / 78 % | 0.7 / 8.0 ms | 392 % / 153 % | 51 / 180 ms² |

² At 2 000 pairs the PC itself is saturated (front + client layer + hbbr + load generator ≈ all
12 CPUs): these latencies are queueing on the PC, not in the proxy. Throughput was met in every
run (1 976–2 017 Mbit/s), no pair failed, no stalls.

- The Go proxies (Caddy, Traefik) copy with 32 KB buffers and hand hbbr bigger reads: hbbr needs
  ~25 % less CPU behind them, which makes front + hbbr equal (3.2–3.3 cores per Gbit/s at
  1 Gbit/s) for Caddy, nginx and HAProxy.
- HTTP mode vs TCP mode in HAProxy: 213 vs 206 %. The cost is TLS plus copying, not HTTP.

### (c) Connection storm: 10 000 new TLS WebSockets in one second

Every front accepted all 10 000 connections without a failed connect (backlog 16 384). Front CPU
in the first 5 s: nginx 156 %, Envoy 181 %, HAProxy 219 % (OpenSSL 3.0's locking, which HAProxy's
documentation warns about; EL9.6+ has 3.5), Caddy 243 %, Traefik 272 %: 7.8 to 14 CPU-seconds per
10 000 handshakes + upgrades (nginx ≈ 1 300 per core-second). For every front the registration
tail was the same (p99 3 s, ~5 000 three-second reconnects): hbbs's single SQLite writer inserting
10 000 *new* keys, as scaling.md found; known hosts come back within 5–10 s (scaling.md,
restart runs). The proxy is not what limits a reconnect storm.

### (d) 100 parallel downloads of 35 MB from `/download/`

| Front | throughput | front CPU | cores per Gbit/s | download p50 / p99 |
|---|---|---|---|---|
| Caddy | 41.3 Gbit/s | 314 % | 0.08 | 690 / 790 ms |
| nginx | 37.8 Gbit/s | 363 % | 0.10 | 740 / 880 ms |
| HAProxy + local nginx | 36.4 Gbit/s | 440 % + 39 % | 0.13 | 770 / 890 ms |
| Envoy + local nginx | 31.8 Gbit/s | 417 % + 42 % | 0.14 | 860 / 1 100 ms |
| Traefik + local nginx | 36.4 Gbit/s | 426 % + 40 % | 0.13 | 760 / 990 ms |

Limited by the client layer (430–470 %). Downloads are cheap for every front; what matters is the
port (the production nginx config caps downloads per node, scaling.md "Criteria 3–5").

### (e) API: 10 000 device cycles within 20 s (~1 500 req/s)

Front CPU (first 20 s): nginx 12 %, HAProxy 15 %, Envoy 26 %, Traefik 38 %, Caddy 42 %; request
latency p50 0.4 ms, p99 0.9–1.2 ms for all, no errors. tether-api answered most heartbeats with
429: every device came from the front's 127.0.0.1 and the per-IP limit for *new* devices applied.
nginx and Envoy got some 204s because their bench configs let the generator's `X-Forwarded-For`
through: Envoy appends to it (tether-api reads the first hop), and the bench nginx config had the
`proxy_set_header` inheritance mistake (a `proxy_set_header` in a location drops the server's)
that the end-to-end test then caught in the production config draft too (fixed, and tested for
every route since). Same work for the proxy either way.

## Feature check

Every route and feature of `server/caddy/Caddyfile` and the monitoring snippets, per candidate.
✓ built in, ◐ possible with an extra component or caveat, ✗ not possible.

| Requirement | Caddy | nginx | HAProxy 3.4 | Envoy | Traefik |
|---|---|---|---|---|---|
| Automatic Let's Encrypt | ✓ TLS-ALPN-01 (today) | ✓ `ngx_http_acme_module` (official, `nginx-module-acme` package): HTTP-01 and **TLS-ALPN-01**, renews in-process, no reload [^acme] | ◐ built-in ACME is *experimental* (`expose-experimental-directives`), HTTP-01/DNS-01, certificates live in memory and must be dumped [^hacme]; otherwise lego/acme.sh + `set ssl cert` over the runtime API | ✗ none (SDS / cert-manager, or an external client + file watch) | ✓ built in (all challenges, DuckDNS DNS-01) |
| `/ws/id`, `/ws/relay` WebSocket | ✓ | ✓ | ✓ | ✓ | ✓ |
| `/api/*` with 512 KiB request buffering | ✓ `request_buffers` | ✓ whole body buffered (512 KiB in memory, rest temp file) | ✓ `wait-for-body … use-large-buffer` + `tune.bufsize.large` | ◐ buffer filter rejects bodies above its limit (uploads are up to 200 MiB) | ✓ buffering middleware |
| `X-Real-IP` = peer, `X-Forwarded-For` = last hop only | ✓ | ✓ `proxy_set_header … $remote_addr` | ✓ | ✓ `use_remote_address` | ✓ (default, untrusted) |
| `/download/*` static files, content types | ✓ | ✓ (`mime.types`, `sendfile`, ranges) | ✗ no file server | ✗ | ✗ |
| `/q/` `X-Accel-Redirect` to an internal path | ✓ `handle_response` | ✓ native (nginx invented it) | ✗ (Lua only, blocking file I/O) | ✗ | ✗ |
| `/readyz/relay`, `/rendezvous/pubkey` rewrites | ✓ | ✓ | ✓ | ✓ | ✓ |
| `/grafana/` basic auth (bcrypt), header stripped | ✓ | ✓ `auth_basic` via crypt(3) (libxcrypt bcrypt) [^bcrypt] | ✓ `userlist` via crypt(3) | ✗ SHA-1 htpasswd only | ✓ |
| `/_metrics/<job>` source IP + bcrypt | ✓ | ✓ `allow`/`deny` + `auth_basic` (403 instead of a closed connection) | ✓ | ◐ RBAC + no bcrypt | ✓ |
| Prometheus metrics of the proxy, localhost | ✓ built in | ◐ `stub_status` + official `nginx-prometheus-exporter` (connections, requests; no latency histograms) | ✓ built in (`USE_PROMEX`) | ✓ built in | ✓ built in |
| HTTP→HTTPS redirect on 80 | ✓ | ✓ | ✓ | ✓ | ✓ |
| TLS 1.2/1.3 modern ciphers, HTTP/2 | ✓ | ✓ | ✓ | ✓ | ✓ |
| Reload without dropping WebSockets | ✗ admin API off: config change = restart, all WebSockets reconnect | ✓ `SIGHUP`: old workers keep their connections (`worker_shutdown_timeout 1h`), tested | ✓ master-worker reload, old process drains (`hard-stop-after`) | ✓ hot restart + drain | ✓ dynamic config hot, static config = restart |
| SELinux enforcing (EL9) | ✓ unconfined (`/usr/local/bin`) | ✓ confined `httpd_t` from the RPM: one boolean + labels, set by install.sh (not run on an EL9 host here) | ✓ unconfined if self-built | ✓ unconfined | ✓ unconfined |
| Updates | manual (pinned version) | **dnf/apt from nginx.org, same day as advisories** | self-built: we track HAProxy and OpenSSL ourselves | manual | manual |

[^acme]: nginx.org, [ngx_http_acme_module](https://nginx.org/en/docs/http/ngx_http_acme_module.html): `challenge http-01 | tls-alpn-01`; prebuilt `nginx-module-acme` packages ([nginx.org packages, RHEL 9 mainline](https://nginx.org/packages/mainline/rhel/9/x86_64/RPMS/)); [NGINX blog: native ACME support](https://blog.nginx.org/?p=74612).
[^hacme]: HAProxy 3.4 `doc/configuration.txt` §12.8 "ACME" ("This feature is experimental …", dump with `dump ssl cert`); [HAProxy wiki: ACME native](https://github.com/haproxy/wiki/wiki/ACME:--native-haproxy).
[^bcrypt]: nginx checks the hash on every request inside the worker, which also carries thousands of WebSockets: the installers write bcrypt cost 5 (~2 ms). The passwords are 24 random bytes, so the cost factor adds no security.

### Maintenance and security record

| | Track record (sources) |
|---|---|
| nginx | F5-run, mainline + stable branches, fixes released for both on the same day, signed packages for EL/Debian/Ubuntu. 2026 so far: ~20 advisories ([nginx.org security advisories](https://nginx.org/en/security_advisories.html)), almost all in modules Tether does not load or use (HTTP/3, SSI, DAV, slice, charset, mp4, rewrite with `alias`, map with regex, scgi/uwsgi); two touch it: TLS session tickets (CVE-2026-97629, tickets are off here) and AIO (CVE-2026-97205, not used). Fixed in 1.31.7/1.30.6. freenginx ([advisories](https://freenginx.org/en/security_advisories.html)) shares most of them; one maintainer, no ACME module, no packages. |
| HAProxy | Very conservative code base, few CVEs, LTS branches for 5 years. 2026: CVE-2026-55203/55204 (HPACK/HTTP/2 null dereference, June, [HAProxy blog](https://www.haproxy.com/blog/june-2026-cve-2026-55204-null-pointer-dereference-in-haproxys-hpack-header-handling)), CVE-2026-26080/26081 (QUIC), CVE-2026-90678 (HTTP/3 smuggling, [Red Hat](https://access.redhat.com/security/cve/CVE-2026-90678)). The catch for us is packaging: no official EL9 RPM of 3.x ([COPR builds](https://copr.fedorainfracloud.org/coprs/frederickding/haproxy) are third-party), so every fix means a rebuild by us. |
| Envoy | CNCF graduated, large attack surface, regular coordinated security releases on four branches (the latest on 2026-10-27). Heavy for a single node. |
| Traefik | Frequent releases, Go; regular CVEs (header handling, middleware bypasses). Same Go memory model as Caddy. |
| Caddy | Few own CVEs, Go standard library CVEs arrive through Caddy releases; pinned binary, updated by hand. |

## "No proxy on the hot path": TLS in hbbr / hbbs

Idea: hbbr (and hbbs) terminate TLS for their WebSockets themselves (rustls, tokio-tungstenite),
either on their own port behind an L4 SNI split or, on relay-only nodes, directly on 443.

- **Gain** (estimated from measurements): HAProxy in TCP mode, which only terminates TLS and
  copies, costs the same as in HTTP mode (206 vs 213 % at 1 Gbit/s). Without a proxy its
  loopback leg goes away (one plain read and write per frame and direction in the proxy, and the
  matching ones in hbbr), but hbbr then does the TLS work itself. Estimate: 0.8–1.0 of the 3.2
  cores per relayed Gbit/s (25–30 %) and ~40 KB per relayed WebSocket. Not prototyped.
- **Effort**: rustls acceptor and certificate hot reload in hbbr (and hbbs), an ACME client next to
  them (or certificates shared from the front through a file + reload), PROXY protocol or other
  client-IP handling (today `X-Real-IP` from the proxy), `/readyz/relay` on the same TLS port, an
  extra DNS name or port where a node keeps a proxy for `/api` and downloads, and a patch to
  maintain on top of rustdesk-server. Roughly one to two weeks with tests.
- **Verdict: not now.** In the target architecture (scaling.md) the relay nodes are bound by their
  1 Gbit/s port, not CPU (53 % CPU in the worst case): the saving would not remove a single node.
  Worth it only for CPU-bound relays (shared-vCPU cloud nodes, 10G uplinks). kTLS + splice
  (HAProxy's `ktls`, experimental in 3.4) is a cheaper route to the same saving later.

## Recommendation

1. **nginx as the front end on every node**, switched one node at a time (`PROXY=nginx`), Caddy
   kept installed as the rollback. Reasons, in order: 2.7× less memory and 45 % less CPU per
   device (each front holds every device), every feature built in without a second server,
   graceful reload, ACME in-process with the same challenge as today, distribution packages with
   same-day security fixes.
2. Not HAProxy, although it is the leanest: it would need Caddy or nginx next to it for files and
   `X-Accel-Redirect` (two front ends to configure, patch and monitor), a self-built binary or a
   third-party repository for 3.x on EL9, and an experimental ACME client. The 21 KB per device
   it saves over nginx are 0.2 GB at 10 000 devices.
3. Not Envoy (no file server, no bcrypt basic auth, more CPU in every test) and not Traefik (Go,
   memory like Caddy, more CPU than Caddy).
4. Keep Caddy as the default until the first node has run nginx for a week; make nginx the
   default in a later change.

## The nginx front end (`server/nginx/`)

- `server/nginx/nginx.conf`: every Caddyfile route (table above), rendered by install.sh to
  `/etc/tether/nginx.conf` (`@RELAY_HOST@`, `@RESOLVER@`). Extras from the monitoring installer go
  into `/etc/tether/nginx.d/*.conf` (included in the TLS server), like `caddy.d/*.caddy`.
- `server/systemd/tether-https-nginx.service` is installed as **`tether-https.service`** (same
  name, so `systemctl restart tether-https` and the runbooks stay valid): nginx as user
  `tether-https` with `CAP_NET_BIND_SERVICE`, `ExecReload` = `nginx -t` + `SIGHUP` (graceful).
- `server/systemd/tether-nginx-exporter.service`: nginx-prometheus-exporter 1.5.3 (official
  release, SHA-256 pinned in install.sh) on 127.0.0.1:21125, reading `stub_status` on 127.0.0.1:21126.
- Certificates: the ACME module asks Let's Encrypt with **TLS-ALPN-01 on 443** (no port 80, the
  firewall's 80→443 forward stays) and keeps account key and certificate in
  `/var/lib/tether-https/acme`; renewal happens inside nginx without a reload.
- Differences to Caddy: `Server: nginx` without version instead of no `Server` header; wrong IP on
  `/_metrics/*` gets 403 instead of a closed connection; request bodies are buffered whole (temp
  file above 512 KiB) instead of the first 512 KiB; no per-request latency histograms in the
  metrics (connections and request counters only).
- SELinux: the nginx RPM runs nginx confined as `httpd_t` (Caddy runs unconfined). install.sh sets
  `httpd_can_network_connect`, labels `/var/lib/tether-api` `httpd_sys_content_t` (read-only),
  `/var/lib/tether-https` `httpd_var_lib_t`, `/run/tether-https` `httpd_var_run_t`,
  `/etc/tether/nginx.*` `httpd_config_t` and port 21126 `http_port_t`. **Not run on an
  enforcing EL9 host here** (WSL has no SELinux): check `ausearch -m avc -ts recent` after the
  first start (Migration, step 3).

## End-to-end test (`tools/proxy-bench/e2e-nginx.sh`)

Runs the **production** `server/nginx/nginx.conf` and the monitoring snippets (ports and paths
moved as in `tools/load/stack.sh`) with the official nginx.org 1.31.7 `noble` packages (nginx +
nginx-module-acme 0.4.1, unpacked, no root), Let's Encrypt's test CA **Pebble** 2.10.1 validating
**TLS-ALPN-01** against nginx, hbbs/hbbr/tether-api built from main (`scripts/build-relay.sh`),
nginx-prometheus-exporter 1.5.3 and small echo servers standing in for Grafana, node_exporter and
blackbox. Result on 2026-11-07: **55 passed, 0 failed** (`results/e2e-nginx.txt`):

- ACME: certificate issued by Pebble via TLS-ALPN-01 and served; account/cert state kept on disk.
- TLS 1.3 and 1.2 (ECDHE AEAD) accepted, TLS 1.1 and CBC suites refused, HTTP/2 on normal routes,
  HSTS, `Server` without version, HTTP on 80 → 301 to HTTPS.
- `/` marker and published download page, unknown paths, `/readyz/relay`, `/rendezvous/pubkey`.
- `/download/`: file intact, content types, `latest.json` with `no-cache`, ranges, the download
  budget (rate after the first MB, 503 for the 33rd parallel download while `latest.json` stays
  served), no path traversal (`..`, `%2e%2e`, `..%2f`).
- `/api/`: heartbeat with a forged `X-Forwarded-For`/`X-Real-IP` from 127.0.0.9: tether-api
  stores 127.0.0.9; a 1.9 MB diagnostics upload from a 2 MB/s client stored intact.
- `/q/`: token checked by tether-api, file sent by nginx from `qcache/` (`X-Accel-Redirect` not
  leaked), content type, `Content-Disposition`, `no-store`, `nosniff`, HSTS; wrong token 404;
  `qcache` unreachable by URL (three spellings).
- `/grafana/`: 308 redirect, 401 without / with a wrong password, the right one reaches Grafana
  with `Authorization` removed.
- `/_metrics/<job>`: hbbs, hbbr, tether-api, nginx (exporter), node, probe (query kept); 401
  without password, **403 from another source IP with the right password**, 404 for unknown jobs;
  forwarded headers carry the peer, never the client's IP headers or `X-Tether-Accel`;
  `stub_status` not public; the exporter counts requests.
- WebSockets through nginx: 200 hosts registered via `/ws/id`, viewers got PunchHole and
  RelayResponse, **two graceful reloads (SIGHUP) during the run dropped no WebSocket**;
  relay-harness: 4 sessions paired through hbbs and `/ws/relay` (hbbs hands out the relay URL
  behind nginx), frames delivered, no breaks.

Not covered here: SELinux (WSL has none), the real Let's Encrypt, the systemd unit itself
(capabilities, `RuntimeDirectory`), install.sh's package installation (no root/dnf in WSL).

## Migration (per node) and rollback

One node at a time, the Oracle backup first, a quiet hour (install.sh restarts every Tether
service: relayed sessions on that node reconnect through another relay within seconds; the
TLS front is down until nginx has its certificate, typically under a minute).

1. **Prepare** (on the PC): a checkout of this branch; copy `server/` (with `nginx/`, `systemd/`,
   `sysctl/`, the binaries if they change) and `deploy/monitoring/` to the node, LF line endings.
2. **Back up** on the node: `sudo cp -a /etc/tether /etc/tether.bak-$(date +%F)` and
   `sudo cp /etc/systemd/system/tether-https.service /etc/tether.bak-$(date +%F)/`. Caddy's
   certificate stays where it is (`/var/lib/tether-https/caddy/…`), so a rollback issues no new
   certificate.
3. **Switch**: `sudo PROXY=nginx ./server/install.sh <host>`
   (install.sh rewrites `server.env` and keeps its `RELAY_SERVERS` unless you pass one; the install.sh of
   0.5.25 and older fell back to this host alone, so pass it to those; `ADMIN_PUB_KEY` only to
   change it). It adds the nginx.org repository, installs `nginx` + `nginx-module-acme`, the
   exporter, renders `/etc/tether/nginx.conf`, tests it, sets the SELinux labels, installs the nginx
   `tether-https.service` and restarts the services.
4. **Check**:
   - `systemctl status tether-https tether-nginx-exporter`; `journalctl -u tether-https -n 50`
     (ACME: no `failed` lines after the first minute);
   - certificate: `echo | openssl s_client -connect <host>:443 -servername <host> 2>/dev/null | openssl x509 -noout -issuer -enddate` (Let's Encrypt);
   - routes from outside: `curl -sI https://<host>/`, `/readyz/relay`, `/rendezvous/pubkey`,
     `/api/policy`, a `/download/…/latest.json`, one `/q/` link (headers + size), port 80 (TLS on
     80 still answers through the firewall forward);
   - a session through this relay (Tether app), and a host registering (Tether Manage → devices);
   - SELinux: `getenforce` = Enforcing and `sudo ausearch -m avc -ts recent | grep -c nginx` = 0;
     if not, `sudo ausearch -m avc -ts recent | audit2why` and report (do not set permissive);
   - metrics: `curl -s 127.0.0.1:21125/metrics | grep '^nginx_up'` = 1.
5. **Monitoring**: on this node `sudo PEER_HOST=… SCRAPE_FROM=… SCRAPE_HASH='…' ./deploy/monitoring/install.sh target`
   (same values as before; it now writes `/etc/tether/nginx.d/`), and on the monitoring node
   re-run `install.sh monitor` with `REMOTE_PROXY=nginx` once the other node runs nginx (its own
   front is read from its `server.env`). Prometheus' job for this node becomes `nginx`.
6. Watch a week (dashboards, `tether-headroom` alerts, probes), then the next node.

**Rollback** (minutes): `sudo PROXY=caddy ./server/install.sh <host>`
(Caddy's binary and certificate are still there; the Caddy unit replaces the nginx one and the
exporter is removed), then the monitoring installers again (`REMOTE_PROXY=caddy` on the
monitoring node). Without the checkout: `sudo cp /etc/tether.bak-<date>/tether-https.service
/etc/systemd/system/ && sudo systemctl disable --now tether-nginx-exporter && sudo systemctl daemon-reload && sudo systemctl restart tether-https`.
The nginx.org repository and packages can stay (the packaged `nginx.service` is disabled).

### Risks

| Risk | Mitigation |
|---|---|
| SELinux denials on the first EL9 start (not testable in WSL) | step 4 check; labels and boolean in install.sh; rollback in minutes |
| No TLS until the first certificate is issued (nginx fails handshakes while `$acme_certificate` is empty) | quiet hour; the other front keeps every device; seconds to a minute |
| Let's Encrypt duplicate-certificate limit (5 per week per name) when switching back and forth | Caddy keeps its own certificate; nginx keeps its in `/var/lib/tether-https/acme` |
| nginx mainline instead of stable | the ACME module and security fixes ship for mainline first; nginx recommends mainline; stable (1.30) works with the same config |
| Download budget returns 503 to the 33rd parallel download | updaters retry; release files belong on object storage (scaling.md) |
| Basic-auth hash cost lowered to bcrypt 5 | the passwords are 24 random bytes |
| `Server: nginx` header visible | no version; Caddy removed the header entirely |
| `dnf install nginx` on the 1 GB Oracle VM needs ~700 MB for metadata (install.sh disables `dnf-makecache` there for that reason) | run the switch when the VM is idle, or `dnf install` first with Caddy stopped for a minute |

## Not verified

- nginx under SELinux enforcing on AlmaLinux / Oracle Linux 9 (labels, boolean, `httpd_t` with
  the ACME module and the `/var/lib/tether-*` paths), and install.sh's `dnf`/`apt` path: both need
  a real EL9 host (step 4 of the migration checks them). Not run on the production servers or the VM.
- One unexplained nginx run with 7 241 failed connects during a 30 000 ramp (not reproduced in
  two repeats); the ~20 failed connects at the start of every nginx ramp.
- OpenSSL 3.0.13 here; EL9.6+ ships 3.5 (faster for HAProxy's handshakes, little effect on nginx).
- Real networks (RTT, loss, NICs), and production TLS clients (the Tether client uses rustls;
  the client layer here was HAProxy/OpenSSL).
- The "TLS in hbbr" gain is an estimate from the TCP-mode run, not a prototype.
- Helm chart / Kubernetes (`deploy/helm`): still Caddy; not changed.

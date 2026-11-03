# Connection resilience: how AnyDesk "always connects", and what Tether does

## Method

Black-box only, in line with the clean-room rule (see `docs/fluxrt-spec.md`). AnyDesk's code was
not decompiled or disassembled.

- **Binary metadata (REA `inspect_pe_imports`):** `AnyDesk.exe` 8.5 MB, PE32 x86, SHA-256
  `979a295b…5de9`. It has **no import table**, so the binary is packed and resolves APIs at
  runtime. Static analysis would show little; runtime behaviour is the useful evidence.
- **Live sockets on the user's PC:** the AnyDesk service keeps one outbound TCP 443 connection to a
  relay (`138.199.27.251:443`), listens on TCP 7070 (direct and LAN connections) and has UDP 50001
  open (LAN discovery).
- **Proxy experiments:** the benchmark VM runs in "corporate" mode. Its firewall blocks all outbound
  traffic except the proxy and DNS, and the system proxy points to `tools/bench/corp_proxy.py`
  (mitmproxy). Each experiment changes the proxy policy and logs every CONNECT
  (`tools/bench/corp_experiment.sh`).

## What AnyDesk does

| Experiment | Proxy policy | AnyDesk behaviour |
| --- | --- | --- |
| Zscaler-style TLS inspection | 443 only, TLS re-signed by the corporate CA (trusted by Windows) | **Fails.** It tries `boot.net.anydesk.com:443` and `:80`, and the TLS handshake is refused with `unknown ca`. AnyDesk checks its server certificate against its own trust anchor, so real deployments must exempt `*.net.anydesk.com` from TLS inspection. |
| Inspection bypass, 443 | 443 only, AnyDesk passed through | Connects to the bootstrap server `boot.net.anydesk.com:443`, which hands it an assigned relay (`relay-259c175b.net.anydesk.com:443`). Then one long-lived connection. |
| Port 80 only | 80 only | Goes straight to its **cached relay** and tries **443 and 80 in the same instant**; port 80 wins. |
| Everything blocked | nothing allowed | Cached relay on 443 and 80, then bootstrap on 443 and 80, then retries about every **60 s**. |
| Recovery | 443 reopened after the block | Still not reconnected after **100 s** (it made only one port-80 attempt in that window). |

The recipe behind "always connects":
1. Several routes are raced at once (443 and 80, proxy if configured).
2. The last working endpoint is cached and tried first.
3. The system proxy is used automatically.
4. Everything is TLS on web ports.

## What Tether does now

| Mechanism | Implementation |
| --- | --- |
| **Route racing (Happy Eyeballs)** | Up to four routes to the own relay: `wss://relay` on 443 and on 80, each through the detected proxy and direct. They start 300 ms apart; the first completed TLS + WebSocket handshake wins and the rest are dropped. Code: `hbb_common::websocket::{connect_routes, WsFramedStream::race}`. |
| Cached winner | The winning route (port and proxy/direct) is stored per relay host and tried first next time, for both `/ws/id` and `/ws/relay`. |
| TLS on port 80 | On the relay, firewalld forwards TCP 80 to Caddy's 443 listener, so it is the same certificate and the same endpoints. The OCI security list allows TCP 80. |
| Dead or wrong proxy | The direct routes run alongside the proxy routes, so a broken proxy setting cannot keep Tether offline. |
| TLS inspection | Works (OS trust store fallback), unlike AnyDesk. The session stays end-to-end encrypted inside it. |
| Quiet operation | No upstream `/api/heartbeat` polling. Since 0.3 the policy client sends one small heartbeat per policy check to `tether-api` (see `docs/api.md`). |
| Recovery | The rendezvous loop retries every ≤18 s, racing every route each time. |

Only Tether's own compiled-in relay gets alternative routes. Other servers keep upstream's single
route.

## Results (2026-11-03)

| Scenario | AnyDesk | Tether |
| --- | --- | --- |
| Zscaler-style TLS inspection, 443 only | ✗ (needs an inspection exemption) | ✓, 62.8 ms glass-to-glass, 27.9 fps |
| Port 80 only, TLS inspection on | ✗ (same reason); ✓ with an exemption | ✓, **56–59 ms, 28 fps** (two 30 s runs) |
| Proxy configured but dead (user's PC) | not tested | ✓ E2E 4/4 (direct route wins) |
| Proxy allows only port 80 (user's PC) | – | ✓ E2E 4/4: register, login, 64 MB file transfer with SHA-256 verified, E2E encryption |
| Recovery after a 90 s total block | > 100 s | **2.7 s** after reopening (worst case ≤ 18 s) |

## Reconnecting after a relay loss (2026-11-03)

| Step | Before | Now |
| --- | --- | --- |
| Viewer notices a dead relay | TCP close, else 30 s without data | TCP close, else 3 s without the host's once-a-second delay test (`RELAY_SILENCE_LIMIT`) |
| First retry | after 0.5 s, then 1, 2, 4, 8, 10 s | at once, then 0.25, 0.5, 1, 2, 4, 8, 10 s (`lib/reconnect.ts`) |
| "Failed to connect via relay…" while the relay is still restarting | core flags it not retryable: the series ended with an error dialog | counts as transient: the series goes on |
| Host whose relay leg failed | viewer waited 18 s for it | viewer gives up after 5 s (`RELAY_PAIR_TIMEOUT`), host retries its relay dial 4× at 0.3 s |
| Host answer to a relay request | WebSocket to hbbs, then dial the relay | both at once |
| Decoder / picture | kept | kept; the first key frame of the new stream resumes it, a stray delta asks for one once |
| Password | kept in the session (`lc.password`) | unchanged, no prompt |
| Host loses its hbbs registration | retry after up to 18 s | at once, then 0.2, 0.2, 0.3, 0.3, 0.5, 0.5, 1, 2, 4, 8, 16 s; WebSocket ping every second, 3 s silence = dead |

The relay-only path has no punch or NAT steps left on the critical path: the "TCP punch attempt" in
the log is only the label of the `PunchHoleRequest`, which hbbs answers with a relay because the
request carries `force_relay`.

Measured with `apps/desktop/e2e/reconnect-bench.mjs` (host and viewer on one PC, DevTools only).
"loss→frame" is from the moment the viewer shows the reconnect overlay to the first frame of the
new connection drawn on the canvas.

| Run | Before (0.3.0) | Now |
| --- | --- | --- |
| Production, `systemctl restart tether-hbbr` | 2.27 s and 17.8 s (frame gap); 1 of 3 runs gave up with an error | 0.93, 0.90, 0.90 s (3 runs, median 0.90 s) |
| Local hbbs/hbbr pairs in WSL, both relays killed | – | median 0.90 s, max 1.00 s (6 runs) |
| Local, the session's server stopped for good | – | 0.34 s, re-paired through the other server |
| Local, both hbbs killed (5 restarts) | – | the running session was never interrupted; the host re-registered 0.3–1.7 s after hbbs was listening again |

## Several rendezvous servers

`TETHER_SERVER` may list several servers (`tether-desk.duckdns.org,tether-desk2.duckdns.org`), and
the policy feature `servers.list` adds more (host names or `wss://host/path` URLs; no ports, IPs,
plain `ws://` or credentials; all verified with the same compiled-in server key). Hosts keep one
independent registration loop per server. Viewers ask the last server that worked first and the
next one as soon as one fails (peer not registered there, server or its relay down) or after 1 s
without an answer; the first that reaches the peer wins and its hbbs picks the relay. The policy
API, heartbeats and updates use the first server that answers.

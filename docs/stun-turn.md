# STUN, TURN and the UDP relay

Status: hbbs answers STUN on UDP 21116 since the WebRTC release, and hbbs now also runs a
TURN server **on the same port** (`vendor/rustdesk-server/src/turn.rs`). This page records
why, how it fits the transport ladder, what was measured and how to deploy it.

## The transport ladder

| Rung | Transport | Gets through | Reliability on a lossy path |
| --- | --- | --- | --- |
| 1 | Direct LAN (TCP 21118) | same subnet | TCP, but LAN loss is ~0 |
| 2 | WebRTC P2P (IPv6 host / NAT punch, UDP) | no proxy, UDP allowed, NATs that punch | KCP-style SCTP (no cwnd, RTO floor ~ RTT, SACK per chunk; see `vendor/rustdesk/Cargo.toml`) |
| 3 | **WebRTC via TURN on hbbs UDP 21116 (new)** | no proxy, outbound UDP to the relay allowed, any NAT (incl. symmetric/CGNAT) | same SCTP as rung 2 |
| 4 | hbbr relay, WebSocket over TLS on TCP 443 | everything incl. HTTP CONNECT / TLS-inspecting proxies | TCP: one loss stalls everything behind it (head-of-line) and shrinks the window |

The viewer races all rungs; a ready rung-4 relay waits 0.4 s for a direct attempt, and once more
`TURN_HOLD_EXTRA_MS` (0.8 s) when the host has offered a TURN candidate. Behind a proxy no offer
is made (ICE would bypass the proxy), so corporate networks go straight to rung 4 exactly as
before: the new rung costs them nothing.

## Options considered (November 2026)

| Option | Language / license | Fit | Verdict |
| --- | --- | --- | --- |
| coturn | C, BSD-3 | The reference server; REST credentials (`use-auth-secret`), TLS/TCP/UDP, `denied-peer-ip`. Its own process and port range (UDP 3478 + 49152–65535 by default). | Long CVE list in 2025–2026: weak RNG for nonces/ports (CVE-2025-69217), `denied-peer-ip` and loopback checks bypassed with IPv4-mapped IPv6 (CVE-2026-27624, CVE-2026-53450), STUN length overflow over TCP/TLS (CVE-2026-68552), relay-port exhaustion (CVE-2026-73215). Needs new ports, which are not open. Rejected. |
| eturnal | Erlang, Apache-2.0 | Clean REST-credential server, `blacklist_peers`. | Erlang VM on a 1 GB micro VM, new ports, another runtime to patch. Rejected. |
| turn-rs (mycrl) | Rust, MIT (current crate; older versions were LGPL/GPL) | Fast standalone server with its own control API. | Separate process and ports; nothing it adds that the webrtc-rs crate (already in our tree) lacks. Not chosen. |
| Pion turn / LiveKit's embedded TURN | Go, MIT / Apache-2.0 | LiveKit runs TURN/TLS on 443 beside its SFU. | Go, separate service. Design reference only. |
| webrtc-rs `turn` 0.10 | Rust, MIT/Apache-2.0 | Same code base as the client's WebRTC stack (fork rev 49c89bd8). Server is pluggable: listener `Conn`, relay-address generator, auth handler. The 0.17 line is in bug-fix maintenance, the new sans-I/O `rtc` crate is the future. | **Chosen, embedded in hbbs** with our own `Conn`s (single port, peer filter, caps) and auth handler. |
| str0m | Rust | Sans-I/O WebRTC; ICE client only, no TURN server. | n/a |
| TURN over TLS/TCP 443 (TURNS) | – | Would reach some 443-only networks. | webrtc-ice (rustdesk fork) gathers relay candidates for `turn:` + UDP only (`agent_gather.rs`: TURNS/TCP is a TODO), and with a proxy no offer is made at all. TCP to the TURN server would also bring back the head-of-line blocking it is meant to remove. Not useful here. |
| QUIC relay (quinn), MASQUE CONNECT-UDP (RFC 9298), WebTransport, HTTP/3 WebSocket | Rust (quinn) | Unreliable datagrams on UDP 443. | Corporate proxies are HTTP/1.1 CONNECT (TCP); MASQUE proxies are not deployed there, and networks that block UDP block QUIC (browsers fall back to TCP). Where UDP works, TURN to 21116 gives the same benefit with the transport the core already has. Would mean a second session transport in the core. Not now. |
| Partial reliability / FEC for video | – | Drop stale frames instead of retransmitting. | The session protocol is one ordered, reliable stream (P-frames depend on earlier frames; the data channel rejects unordered channels on purpose). With the KCP-style SCTP a loss costs about one RTT, measured below; FEC/partial reliability would save at most that RTT at the price of keyframes. Not worth the risk. |
| How others do it | – | Tailscale: direct UDP → peer relays (UDP, since 2025) → DERP (HTTPS/TCP, documented head-of-line blocking). Teams/Zoom/Cloudflare TURN: UDP 3478 → TCP 443 → TLS 443 (proxy aware). | Same ladder as above; our rung 4 is their TCP/TLS fallback. |

Trust model: the TURN secret is 32 random bytes made by each hbbs at start, held in memory only.
hbbs issues TURN REST credentials (`<expiry>:<id>` / `base64(HMAC-SHA1)`) for itself in
`PunchHole.turn_server`, which reaches the host over its TLS WebSocket. No secret is shared between
nodes or with clients, and a restart invalidates every credential (the allocations die with the
process anyway).

Abuse limits (TURN is a classic open-relay/SSRF vector, see the coturn CVEs above):
- no relaying to or from private, loopback, link-local, CGNAT, multicast, reserved and
  documentation ranges, IPv4-mapped/compatible, NAT64 and 6to4 forms of them, or the relay's own
  public address (unit test `peers_filtered`);
- credentials open allocations for 300 s; afterwards only the client address that first used a
  credential may keep refreshing its allocation (12 h at most);
- `TURN_MAX_ALLOCATIONS` (64), 16 peers per allocation, `TURN_BANDWIDTH_MBPS` (32) per
  allocation and direction, extra datagrams dropped;
- IPv4 relays only, UDP only.

Single port: every relayed address is `<public IP>:21116`. hbbs passes each datagram to
`Turn::dispatch` first: TURN requests (Allocate, Refresh, Send, CreatePermission, ChannelBind) and
ChannelData from a known TURN client go to the TURN server, datagrams from an address an
allocation has sent to go to that allocation; everything else (rendezvous, STUN binding) stays
with hbbs as before. A peer is told apart by its address (normally the relayed port does that);
a NAT that maps per destination is covered when exactly one allocation knows that peer's IP.

## Measurements

`tools/turn-bench` (2026-11-05, WSL network namespace, `netem` on its loopback with MTU 1500 and
offloads off, so each IP packet is lost on its own): the "host" sends 30 frames/s (12 KB, an 80 KB
keyframe every 2 s, about 3.7 Mbit/s), the "viewer" 60 input events/s (50 B). TCP = two
connections paired by a relay process and copied, as hbbr does; TURN = the core's `WebRTCStream`,
host relay-only, through `turn.rs` on one shared UDP port, as in production. Delay and loss are
per hop (viewer to relay, relay to host, both directions), so the 2 x 10 ms rows are an RTT of
40 ms and 2 hops of loss each way. 30 s per cell; tail values of single runs are noisy.

Frame latency, sender to receiver, p50 / p95 / p99 in ms:

| Hop delay, loss | TCP, CUBIC | TCP, relay BBR + ends CUBIC (production) | TCP, BBR everywhere | TURN, stock SCTP | TURN, tuned SCTP (this change) |
| --- | --- | --- | --- | --- | --- |
| 10 ms, 0 % | 20 / 20 / 21 | – | 21 / 22 / 74 | 21 / 22 / 26 | 21 / 21 / 24 |
| 10 ms, 1 % | 20 / 95 / 157 | 41 / 844 / 1654 | 32 / 177 / 320 | 22 / 95 / 127 | 21 / 62 / 97 |
| 10 ms, 2 % | 60 / 191 / 272 | 59 / 349 / 804 | 40 / 319 / 474 | 29 / 95 / 129 | 28 / 94 / 128 |
| 10 ms, 5 % | 742 / 1219 / 1420 | 393 / 740 / 939 | 65 / 335 / 614 | 62 / 161 / 227 | 61 / 128 / 161 |
| 20 ms, 1 % | 108 / 757 / 1025 | 72 / 428 / 687 | 47 / 149 / 345 | – | 42 / 122 / 156 |
| 20 ms, 2 % | 2496 / 4096 / 4449 | 517 / 927 / 1212 | 58 / 180 / 243 | 89 / 155 / 222 | 89 / 188 / 254 |

Input latency p95 / p99 in ms: 10 ms 1 %: TCP CUBIC 24 / 57, TURN stock 77 / 112, TURN tuned
45 / 78; 10 ms 5 %: 62 / 206, 144 / 211, 78 / 112; 20 ms 2 %: TCP 80 / 98 (CUBIC and BBR),
TURN tuned 137 / 154. Largest frame gap at 10 ms 5 %: TCP CUBIC 392 ms, production mix 575 ms,
TURN tuned 233 ms. TURN connected 0.5 s (no delay) to 1.5-2.4 s (lossy) after the offer.

Reading:
- Under loss, TCP's problem is less the single head-of-line stall than throughput: CUBIC (what
  Windows and macOS send with) cannot hold 3.7 Mbit/s at 2-4 % path loss, frames queue and the
  latency runs into seconds. BBR on the relay alone does not fix the host-to-relay leg, which is
  the host's own TCP. BBR on every socket is decent but Tether cannot set the clients' TCP stack.
- The UDP path keeps the median at the path delay up to 5 % loss per hop on a 40 ms RTT and is
  close to all-BBR TCP on an 80 ms RTT. It costs a little input latency at low loss (one ordered
  stream: a lost event holds the next ones back until its resend).
- SCTP tuning (`crates/webrtc-sctp`): RACK-style detection (one later chunk acked beyond the
  reordering window marks a chunk lost, instead of three) and a 40 ms RTO floor instead of 100 ms.
  A lower variance floor (5 ms) measured worse and was dropped. Partial reliability for video and
  FEC were not built (see options above): the remaining tail is the ordered-stream wait for one
  resend, which a video stream of dependent P-frames needs anyway.
- Not measured: bursty loss, reordering, real Oracle paths; the evening Amsterdam-Telekom loss
  has not been characterised.

Production, 2026-11-05 (both relays on the TURN hbbs `c19db1d5…`): technician (this branch) on
the PC, TetherQS (this branch) in the Hyper-V VM with egress direct and a firewall rule that lets
it send UDP only to the two relays (`C:msm-turn.ps1 -On`), so no direct pair can form:
`direct-e2e.mjs vm 5` gave **Relay (UDP)** 5/5, connection secured 1.90-2.34 s and first frame
2.23-2.67 s after `--connect` (the same build without TURN, WSS relay: 1.98-2.32 s / 2.31-2.54 s).
Old builds (0.5.4 technician, 0.5.5 TetherQS) against the new hbbs: corporate VM `vm-ft-e2e`
19/19 after each relay, direct egress 18/19 (the failing check, the TetherQS window re-centring
after Disconnect, fails the same with the previous hbbs on both relays). hbbs RSS 11 MB.

## Configuration (hbbs environment, `/etc/tether/server.env`)

| Variable | Default | Meaning |
| --- | --- | --- |
| `RELAY_HOST` | (written by `install.sh`) | This node's name; TURN is on when it is set. Resolved every 60 s for the relayed address. |
| `TURN` | on | `0` turns TURN off. |
| `TURN_PUBLIC_IP` | resolve `RELAY_HOST` | Fixed relayed address. |
| `TURN_MAX_ALLOCATIONS` | 64 | Concurrent allocations. |
| `TURN_BANDWIDTH_MBPS` | 32 | Per allocation and direction. |
| `TURN_ALLOW_PRIVATE_PEERS` | off | `1` = lab only (relay to private addresses and to itself). |

## Deploy (hbbs only; no port, firewall, OCI, key or `api.env` change)

TURN uses UDP 21116, which firewalld and the OCI security list already open for STUN. Build with
`scripts/build-relay.sh` (WSL), then per relay, one at a time:

```bash
ip=158.101.205.185   # then 92.5.231.71
scp -i ~/.ssh/tether_oracle hbbs opc@$ip:/tmp/hbbs.new
ssh -i ~/.ssh/tether_oracle opc@$ip 'set -e
  sudo cp -p /usr/local/bin/hbbs /usr/local/bin/hbbs.bak-p2p
  sudo install -m 0755 /tmp/hbbs.new /usr/local/bin/hbbs && rm /tmp/hbbs.new
  sudo restorecon /usr/local/bin/hbbs
  sudo systemctl restart tether-hbbs
  sleep 3; systemctl is-active tether-hbbs; curl -s localhost:21122/readyz
  sudo journalctl -u tether-hbbs -n 50 --no-pager | grep -i turn'
python tools/turn-bench/turn-check.py $ip   # STUN ok ..., TURN ok: Allocate -> 401, realm 'tether'
```

Rollback: `sudo install -m 0755 /usr/local/bin/hbbs.bak-p2p /usr/local/bin/hbbs && sudo systemctl restart tether-hbbs`;
or keep the binary and set `TURN=0` in `/etc/tether/server.env`, then restart hbbs. Old clients
ignore `PunchHole.turn_server` (field 1001); new clients against an old hbbs get no TURN entry.

## Code

- `vendor/rustdesk-server/src/turn.rs`: server, `Turn::dispatch`, `RestAuth`, `forbidden_peer`,
  unit tests incl. a real Allocate → relay → peer → back round trip over the shared port.
- `vendor/rustdesk-server/src/rendezvous_server.rs`: start, `set_socket`, dispatch before STUN,
  `PunchHole.turn_server` for each WebRTC offer.
- `rendezvous.proto` (both trees): `PunchHole.turn_server = 1001`.
- `vendor/rustdesk/libs/hbb_common/src/webrtc.rs`: `new_with_turn`, `session_turn_server` (only
  credentialed `turn://` is accepted from the server), relay acceptance wait 250 ms on the
  offerer, `remote_relay_candidate_within`.
- `vendor/rustdesk/src/rendezvous_mediator.rs`: the host passes `ph.turn_server` to its answerer.
- `vendor/rustdesk/src/client.rs`: `TURN_HOLD_EXTRA_MS`.
- `crates/webrtc-sctp`: the fork's SCTP with `set_rack_loss_detection` and
  `set_no_congestion_control_rto_min` (both called from `webrtc.rs`); patched into the apps and
  the core with `[patch."https://github.com/rustdesk-org/webrtc"]`.
- `apps/desktop/src/lib/route.ts`: badge **Relay (UDP)** for a TURN session.
- `tools/turn-bench`: the benchmark above (`netem-bench.sh`, WSL as root) and `turn-check.py`.

# Relay high availability

Tether can run any number of **independent relay nodes**. Each node is the full stack that runs
on the production VM today (hbbs, hbbr, Caddy, tether-api, see [deployment.md](deployment.md)).
The nodes share no state: there is no cluster protocol and no shared database. What they share is
the server key pair, so clients' pinned `TETHER_PUB_KEY` (or the signed server key list) works with
all of them. During a key rotation the nodes may sign with different keys for a while: devices with
the list trust both, and every node accepts both ([key-rotation.md](key-rotation.md)). Availability
comes from the clients, which register with and race across all nodes, and from each hbbs being
able to hand out any node's relay.

Running nodes: `tether-desk.duckdns.org` (relay 1) and `tether-desk2.duckdns.org` (relay 2), both
Oracle Always Free E2.1.Micro VMs. Kubernetes runs the same design with per-pod nodes
([kubernetes.md](kubernetes.md)).

## Topology

```mermaid
flowchart LR
  subgraph N1["node 1  tether-desk"]
    C1[Caddy :443/:80] --> S1[hbbs]
    C1 --> R1[hbbr]
    C1 --> A1[tether-api]
  end
  subgraph N2["node 2  tether-desk2"]
    C2[Caddy :443/:80] --> S2[hbbs]
    C2 --> R2[hbbr]
    C2 --> A2[tether-api]
  end
  H[host] -- "registers (WS /ws/id)" --> C1
  H -- "registers (WS /ws/id)" --> C2
  V[viewer] -- "races PunchHoleRequest" --> C1
  V -.-> C2
  S1 -. "/readyz every 3 s" .-> R1
  S1 -. "https /readyz/relay" .-> C2
  S2 -. "/readyz every 3 s" .-> R2
  S2 -. "https /readyz/relay" .-> C1
```

## Connection flow

1. **Hosts register with every node** (client list, client side). Each node's hbbs keeps the
   host's WebSocket as its signalling path, the same as today.
2. **The viewer races its PunchHoleRequest across the nodes.** The first hbbs that knows the host
   wins. That hbbs picks a relay and sends `PunchHole{relay_server}` to the host over the host's
   registration.
3. **Relay choice (hbbs, server side).** Each hbbs has a relay list (`-r`, systemd
   `RELAY_SERVERS`). An entry has the form `<address>[|<health URL>]`:
   - Every 3 s hbbs fetches every health URL. hbbr's `/readyz` answers `200 ready sessions=N`, or
     `503 draining sessions=N` while it drains. A failed fetch (2.5 s timeout) or a non-200 status
     removes the relay from the healthy set until a later check passes.
   - hbbs hands out the **least loaded** healthy relay, by open sessions. Ties rotate. Locality
     does not matter: the relay path is client to relay, whichever hbbs answered.
   - Entries without a health URL keep the old TCP connect check on port 21117. `ws://`/`wss://`
     entries without one are always treated as healthy.
4. The host connects to the relay and returns `RelayResponse` **to the same hbbs**
   (`rendezvous_mediator` sends it to the server the PunchHole came from). That hbbs forwards it
   to the viewer, which connects to the same relay. hbbr pairs the two connections by uuid.

Production relay lists:

```
relay 1: tether-desk.duckdns.org|http://127.0.0.1:21121/readyz,tether-desk2.duckdns.org|https://tether-desk2.duckdns.org/readyz/relay
relay 2: tether-desk2.duckdns.org|http://127.0.0.1:21121/readyz,tether-desk.duckdns.org|https://tether-desk.duckdns.org/readyz/relay
```

The node checks its own hbbr directly and the other node through Caddy's `/readyz/relay` route.
Until relay 1 runs the new Caddyfile, `/readyz/relay` on relay 1 hits Caddy's catch-all
`200 Tether relay`. Relay 2's hbbs reads that as healthy with load 0, so the two versions work
together.

## Direct paths beside the relay

With policy `network.direct = auto` (default, [policy.md](policy.md#direct-connections-networkdirect))
the viewer races direct paths against the relay flow above.

| Path | Badge | Status | Needs |
| --- | --- | --- | --- |
| Same LAN | `Direct (LAN)` | shipped | host with the direct listener (see below); no relay change |
| IPv6, both global | `Direct (IPv6)` | WebRTC/ICE host candidates | hbbs with WebRTC signalling (below), both ends this version, no proxy |
| NAT hole punching | `P2P` | WebRTC/ICE server-reflexive candidates | same, plus UDP 21116 to the relays (STUN) |
| UDP relay (TURN on hbbs UDP 21116) | `Relay (UDP)` | WebRTC/ICE relay candidate of the host | new hbbs, both ends this version, no proxy ([stun-turn.md](stun-turn.md)) |
| Relay (WSS/443) | `Relay` | unchanged | – |

LAN flow:

1. When the session starts, the viewer broadcasts a lookup for the host's ID (UDP 21119, every
   IPv4 interface) while it sends the PunchHoleRequest. A host with direct connections on answers
   with its listener port (21118), nothing else.
2. When the relay answer (`RelayResponse`, step 4 above) arrives, the viewer connects to the relay
   **and** to the host's LAN address at the same time. The LAN attempt runs the normal key
   exchange (the host proves the key hbbs signed for its ID, `RelayResponse.pk`) before it
   can win; a ready relay is held up to 300 ms while it is in flight.
3. The first secured path carries the session; the other is dropped (the host's relay leg times
   out at hbbr, no Accept prompt appears for it because no login is sent).

Measured 2026-11-04 (Windows PC → TetherQS in the Hyper-V VM, same subnet): LAN lookup answered in
5 ms, LAN connection secured 2–56 ms after the relay answer, badge 1.3–1.7 s and first frame
1.5–1.9 s after `--connect` (relay run before: 1.8 s / 2.5 s). Corporate VM (technician inside
the VM, egress blocked): relay as before, relay leg 118–183 ms, 19/19 `vm-ft-e2e` checks.

When nobody answers the lookup within 300 ms (other network, corporate PC whose firewall blocks
outbound UDP, also a host whose firewall blocks its own outbound answer) there is no LAN attempt and the relay is used exactly as before. The host's
password / Accept / companion-secret and permission checks run on the LAN connection like on a
relayed one (same `Connection` code; an unencrypted handshake is refused).


### IPv6 and NAT hole punching (WebRTC/ICE)

Both use the core's WebRTC transport (the path RustDesk built for WebSocket deployments): ICE
host candidates cover LAN and global IPv6, server-reflexive candidates cover NAT hole punching,
and the DTLS channel is bound to the identity the server signed (`IdPk.dtls_fingerprint`, checked
fail-closed in `secure_connection`). RustDesk's older UDP punch / `socket_addr_v6` punch stays off
(`enable-udp-punch`, `enable-ipv6-punch` = N): it needs plain TCP/UDP rendezvous instead of the
WebSocket on 443 and does not run under the forced relay of WebSocket mode.

Flow (both ends with `network.direct = auto`, no proxy):

1. The viewer builds a WebRTC offer per rendezvous server it asks and sends it in
   `PunchHoleRequest.webrtc_sdp_offer` over the WebSocket (TLS, so the candidates are never in
   the clear).
2. hbbs forwards it in `PunchHole` to the host (over the host's WebSocket registration).
3. The host answers in `RelayResponse.webrtc_sdp_answer` (and dials the relay as before). hbbs
   forwards the answer and, because the response carries one, **keeps the viewer's socket** for
   the trickle.
4. `IceCandidate` (rendezvous field 29) both ways: from the host on its own WebSocket with
   `socket_addr` = the viewer's punch socket, from the viewer with `id` = the host. hbbs only
   routes them (size-capped, 2 KB); the peers drop candidates with a foreign session key.
5. The viewer races WebRTC (and the LAN attempt) against the relay. A ready relay is held
   `relay-fallback-delay` = 0.4 s while a direct attempt is still in flight; then the relay wins
   and WebRTC is closed. The first secured path carries the session; login, Accept and
   permissions are the same `Connection` code as on the relay.

STUN: hbbs answers STUN binding requests (RFC 5389, XOR-MAPPED-ADDRESS) on its UDP port 21116,
which both relays already open (firewalld + OCI security list). Clients use
`ice-servers = stun:<each rendezvous server>:21116` and never the core's public defaults
(Google/Cloudflare would see who connects to whom), so no extra service, port or cost.

TURN: hbbs also runs a TURN server on the same UDP port 21116 and hands the host a 5-minute
credential with each offer, so when no direct pair forms the session can still run over UDP
through the relay (badge `Relay (UDP)`) instead of the WebSocket relay, which suffers under packet
loss. Design, limits, measurements and deployment: [stun-turn.md](stun-turn.md).

With a proxy (detected system proxy or one set by the user) neither side makes or answers an
offer (ICE would leave past the proxy with the real IP); the session goes to the relay at once,
as before. Policy `network.direct = off` makes the viewer send no offer and the host decline
one. Hosts before this version answer offers whenever they have no proxy (upstream behaviour);
an org that sets `off` sets it for both ends.

Measured 2026-11-04 (local relays: new hbbs/hbbr on the Hyper-V host 172.30.240.1 plus one in WSL, two servers in the client list; Windows PC technician → TetherQS in the VM, VM egress direct, 3 runs each, badge = connection secured, from `--connect`):

| Case | Path | Badge | First frame | Race detail (technician log) |
| --- | --- | --- | --- | --- |
| LAN listener off (no `Tether QS` rule), WebRTC | `P2P` | 1.45–1.64 s | 1.67–1.97 s | WebRTC connected 50–264 ms after the host's answer |
| QS outbound UDP blocked, no proxy | `Relay` | 1.70–1.80 s | 2.14–2.46 s | relay held 408–417 ms (the 0.4 s window), then used |
| LAN listener on | `Direct (LAN)` | 1.18–1.39 s | 1.53–1.83 s | LAN TCP beats WebRTC on the same subnet |

Same day, production relays (old hbbs), builds with the production keys: corporate VM (technician and TetherQS inside, proxy, egress blocked) `vm-ft-e2e` 19/19, punch type `TCP` (no offer), relay leg 120–148 ms after the answer (old build the same hour: 125–131 ms). VM egress direct, technician on the PC: the offer goes out, the old hbbs drops it, `Relay` 1.56–1.78 s, relay leg 116–140 ms (no hold, since no answer came back).

PC and VM share a subnet, so ICE can pair their host candidates directly (the nominated pair type is not logged); a punch through two real NATs (server-reflexive pair) was not reproducible locally. STUN (hbbs on UDP 21116) answered locally and through the WSL NAT (mapped address 172.17.192.1, the NAT's).

Compatibility: a new client against an old hbbs loses only the offer (the old hbbs rebuilds
`PunchHole` without it), so the session is relayed as before; an old client against the new
hbbs sends no offer.

## Failures

| Event | Effect | Recovery |
| --- | --- | --- |
| hbbr process or node dies | Sessions on it drop. | The client notices (connection closed, or 3 s of silence with WS ping liveness), asks any live hbbs again and re-pairs on a healthy relay. kind measurements: 20–40 ms after a pod kill, 1.02 s after a silent node freeze with 1 s detection ([kubernetes.md](kubernetes.md)). The dead relay leaves every hbbs's healthy set within 3 s + 2.5 s; a pick that lands on it before then fails fast and the client asks again. |
| hbbs or node dies | Hosts stay registered with the other nodes. New sessions go through them (viewers race). Sessions that are already relayed do not use hbbs and continue. | The host re-registers when the node returns. |
| Planned hbbr restart (drain) | On SIGTERM hbbr reports not-ready at once and keeps relaying open sessions. For the first 5 s it still pairs new uuids (hbbs may hand it out until its next check); after that it refuses new pairings. It exits when the last session ends or after `DRAIN_TIMEOUT` (systemd 300 s, k8s 3600 s). A second signal exits at once. | Sessions still open at the timeout reconnect through another relay. |
| Planned hbbs restart | hbbs is not-ready for `DRAIN_TIMEOUT` (default 0), then exits. | Hosts re-register. Viewers use the other nodes meanwhile. |
| Caddy restart | The WebSockets through it are cut. | Clients reconnect (kind: 20–40 ms). Restart Caddy only for config changes. |
| DuckDNS or IP change | The name follows the VM (ddns timer every 5 min). | Clients use the other node meanwhile. |

Drain on a VM, with 2 nodes: `sudo systemctl restart tether-hbbr` on node 1 waits up to 300 s for
the sessions on it to end. New sessions go to node 2 within 3 s. Watch progress with
`curl -s localhost:21121/readyz`. `sudo systemctl kill -s KILL tether-hbbr` skips the wait.

## Health endpoints

Plain HTTP on every node. The OCI security list and firewalld do not open these ports; Caddy
exposes only `/readyz/relay` (hbbr) and `/rendezvous/pubkey` (hbbs `/pubkey`).

| Port | Process | `/healthz` | `/readyz` | `/pubkey` |
| --- | --- | --- | --- | --- |
| 21121 | hbbr | 200 while running | 200 `ready sessions=<paired sessions>` / 503 `draining …` | 404 |
| 21122 | hbbs | 200 while running | 200 `ready sessions=<hosts registered over WS>` / 503 `draining …` | 200 the public key hbbs signs with ([key-rotation.md](key-rotation.md#checking-the-switch)) |

`HEALTH_PORT=<port>` overrides the port, and `HEALTH_PORT=0` turns the endpoint off.
`DRAIN_TIMEOUT=<s>` sets the drain window (default 0: exit at once, the old behaviour).

## Policy, releases, devices and diagnostics on N nodes

Chosen approach: **Tether Manage writes to every node; the nodes stay independent.** Everything
Manage publishes is either signed and versioned (policy) or verified by the client (release
signatures). A node that missed a write is stale, not wrong, and Manage can see that and repair
it. Server-to-server replication would add a client, peer trust and failure modes to each
tether-api for no gain at this scale.

Server half (implemented):
- `POST /api/admin/diag-ticket` accepts an optional `"ticket":"<8 chars>"`. Manage gets the ticket
  from the first node and opens the **same ID** on the others, so the client's upload works on
  whichever node it uses. A wrong format is rejected with `400`, an existing ID with `409`
  (`server/api/tests/api.rs`).
- `GET /readyz/relay` per node (Caddy). Manage can show node health from it.

Tether Manage (implemented in 0.4.0; the server setting is a comma-separated relay list):
1. Setting: a relay list instead of one URL (default `tether-desk.duckdns.org`,
   `tether-desk2.duckdns.org`).
2. Policy: `PUT /api/admin/policy` to every node. Show the per-node result and
   `policy_serial` from `/api/admin/status`. Offer "re-publish" for nodes behind; the same signed
   document can be PUT again, and any node with a lower serial accepts it.
3. Releases and download page: upload to every node. Write each node's `latest.json` with that
   node's own host in `platforms.*.url`, so an update never depends on another node. Upload
   artifacts first and `latest.json` last on each node.
4. Devices: `GET /api/admin/devices` from every node. Merge by `uuid` and keep the newest
   `last_seen`, because a client heartbeats to whichever node it uses.
5. Diagnostics: create the ticket on the first reachable node, then `{"ticket": …}` on the others.
   List `GET /api/admin/diag` from all nodes and download from the node that reports `uploaded`.

## Capacity

- Relays carry the session traffic; hbbs carries only signalling. One E2.1.Micro has 1/8 OCPU,
  1 GB RAM and up to 480 Mbit/s network. Oracle's Always Free outbound allowance is 10 TB/month.
  Estimate (not measured): a remote-desktop session averages 1–5 Mbit/s, so one micro VM is
  bandwidth-bound at a few dozen concurrent sessions before CPU limits matter. hbbr holds two
  WebSockets per session and a small buffer; memory is not the limit.
- Least-loaded choice spreads sessions across nodes. Load is sampled every 3 s, so a burst inside
  one interval goes to the same relay (`shortcut:` comment in `rendezvous_server.rs`).
- Add a node: provision as in [deployment.md](deployment.md), copy the key pair (during a rotation
  also `/etc/tether/keys.env`), then add the node
  to every node's `RELAY_SERVERS` and the client server list. Restarting hbbs to load the list
  does not interrupt relayed sessions.

## Code

- `vendor/rustdesk-server/src/common.rs`: `listen_signal_and_drain`, `serve_health`.
- `vendor/rustdesk-server/src/relay_server.rs`: session count, refuse new pairings while
  draining, health server, drain on SIGTERM.
- `vendor/rustdesk-server/src/rendezvous_server.rs`: `<address>|<health URL>` entries, HTTP(S)
  checks, `pick_relay` (least loaded, then rotation), hbbs drain and health.
- `vendor/rustdesk-server/src/rendezvous_server.rs`: WebRTC/IPv6 fields forwarded in
  `PunchHole`/`FetchLocalAddr`/`PunchHoleResponse`, `send_to_tcp_keep`, `relay_ice_candidate`;
  `common.rs`: `stun_binding_response` (unit test `stun_tests`).
- Patches: `patches/rustdesk-server.patch`, `patches/rustdesk-server-hbb_common.patch` (the
  rendezvous.proto of the client, without `IdPk`, which message.proto defines; regenerate with
  `scripts/make-patches.sh`).
- Builds and tests: `scripts/build-relay.sh`, `scripts/test-relay.sh` (WSL, static musl).

## Enabling HA on relay 1 (applied for 0.4.0)

Relay 1 and relay 2 both run the 0.4.0 bundle. How relay 1 was joined to relay 2:

```bash
# bundle from a checkout of this branch (LF line endings, see .gitattributes), with the new binaries
#   dist/linux-relay/{install.sh,bin/{hbbs,hbbr,tether-api},systemd/,caddy/}
scp -i ~/.ssh/tether_oracle -r dist/linux-relay opc@tether-desk.duckdns.org:/tmp/
ssh -i ~/.ssh/tether_oracle opc@tether-desk.duckdns.org
cd /tmp/linux-relay && sudo RELAY_SERVERS='tether-desk.duckdns.org|http://127.0.0.1:21121/readyz,tether-desk2.duckdns.org|https://tether-desk2.duckdns.org/readyz/relay' ./install.sh tether-desk.duckdns.org
# keeps /etc/tether/api.env and duckdns.env; rewrites server.env; restarts all services
curl -s https://tether-desk.duckdns.org/readyz/relay     # -> ready sessions=0
```

The installer restarts hbbr, hbbs and Caddy, so sessions open on relay 1 at that moment
reconnect (through relay 2 once clients know it). Do it at a quiet time. Later restarts of the
new hbbr drain.

## Server update for key rotation (2026-11-06)

Both relays run hbbs/hbbr/tether-api from main 2e969d1 (static musl, `scripts/build-relay.sh`,
`scripts/test-relay.sh` green): hbbs `9055beb7`, hbbr `df8df398`, tether-api `a741296a`. Only the
binaries were swapped (relay 2 first, then relay 1; both had 0 relayed sessions, so no drain
wait) and hbbs/hbbr got a systemd drop-in `/etc/systemd/system/tether-hbb{s,r}.service.d/keys.conf`
with `EnvironmentFile=-/etc/tether/keys.env` (KEYS_ACCEPT, [key-rotation.md](key-rotation.md));
`server.env`, `api.env`, Caddy and DuckDNS were left alone. The previous binaries are kept as
`/usr/local/bin/<name>.bak-0515`; `/tmp/relay-0516/rollback.sh` on each node restores them.
Checks after each node: services active, `/api/policy` 200, `/api/server-keys` "no server key
list published" (new endpoint, no list yet), manifests and download page served, VM connection
through the corporate proxy OK.

The `Audit post dropped … 404` lines in client logs are unrelated: the upstream client posts to
`/api/audit/…`, an endpoint only RustDesk Pro has; tether-api does not implement it.

## Server update 0.5.17 (2026-11-06)

Both relays run hbbs `daead622`, hbbr `6e4caaa6` and tether-api `730e728e` (static musl from main,
`scripts/build-relay.sh` + `scripts/test-relay.sh` green) and the Caddyfile with
`/rendezvous/pubkey`. New on the relays: hbbs reports the key it signs with (Tether Manage's check
before "Every relay is switched"), and tether-api stores the session audit log
(`/api/audit/conn|file|alarm`, Tether Manage › Sessions). Order: relay 2, then relay 1; hbbr
drained the one open session (2.5 and 5 min). Backups on each node: `/usr/local/bin/<name>.bak-0516`
and `/etc/tether/Caddyfile.bak-0516`; `/tmp/relay-0517/rollback.sh` restores them.

Two things learned: `caddy validate` needs `RELAY_HOST` from `server.env` (the Caddyfile reads
`{$RELAY_HOST}`; sourcing the whole file in a shell fails on the `|` in `RELAY_SERVERS`), and
Caddy runs with the admin API off, so a new Caddyfile needs `systemctl restart tether-https`
(about a second without HTTPS), not `caddy reload`.

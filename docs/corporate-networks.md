# Corporate networks: proxies, zero-trust filters, 443-only egress

Tether uses **exactly one outbound connection type: TCP 443 (WebSocket over TLS)** to the relay
name. Behind a proxy nothing else goes to the internet: no STUN, no TURN, no hole punching. That
is how it gets through corporate proxies and zero-trust gateways such as Zscaler, Netskope or
Palo Alto Prisma. Only traffic that never leaves the LAN may bypass the proxy (see
[LAN paths behind a proxy](#lan-paths-behind-a-proxy)).

## How it works

| Part | What it does | Where |
| --- | --- | --- |
| 443-only policy | Overrides that cannot be changed in the UI: `allow-websocket=Y`, `disable-udp=Y`, `api-server=https://<relay>`. Legacy UDP/IPv6 hole punching and LAN discovery are off. Behind a proxy (detected or configured) WebRTC runs LAN-only: no STUN or TURN server, only private addresses, so every internet-bound packet is the 443 connection through the proxy. Without a proxy and with policy `network.direct = auto`, direct paths (LAN, WebRTC/ICE with STUN and TURN on UDP 21116 of our relays) are raced beside the relay; a network that drops UDP costs at most 0.4 s ([relay-ha.md](relay-ha.md#ipv6-and-nat-hole-punching-webrtcice)). | `apps/desktop/src-tauri/src/netpolicy.rs`, `vendor/rustdesk/src/common.rs` |
| Proxy auto-detection | Checked in this order: `HTTPS_PROXY` / `ALL_PROXY`, then the system proxy. On Windows that is the WinINet/IE settings, PAC files and WPAD auto-detect (via WinHTTP). On macOS it is `scutil --proxy`. Detection repeats every 60 s, so a VPN or a network change is picked up. Supports HTTP CONNECT, HTTPS and SOCKS5 proxies. | `netpolicy.rs` |
| TLS-inspecting proxies | If the proxy re-signs TLS with a company CA (as Zscaler does), Tether falls back from rustls to the OS TLS stack. That stack trusts the CA the company deployed to the machine. There is never an "accept any certificate" fallback. | `hbb_common::websocket` |
| Relay front end | Caddy on 443 with a Let's Encrypt certificate. `/ws/id` goes to hbbs (rendezvous) and `/ws/relay` to hbbr (relay). | `server/caddy/Caddyfile`, `server/systemd/tether-https.service` |
| Host registration over WebSocket | Upstream open-source hbbs accepts hosts only over UDP. Tether's patched hbbs adds the following: it accepts `RegisterPeer`/`RegisterPk` over TCP/WebSocket with the same UUID, key, IP-change and rate-limit checks; keeps the host's connection as its signalling path; sends empty-frame heartbeats (every 20 s) so proxies don't close idle connections; and gives each proxied client a unique address (forwarded IP, the last `X-Forwarded-For` hop that Caddy sets, plus the connection port). | `patches/rustdesk-server.patch` |

What the inspecting proxy can see: the relay host name, connection timing and sizes, and
rendezvous signalling (device ID, public key, connect requests), because that WebSocket relies on
TLS alone. **It cannot see the session.** Screen, input, clipboard and files are end-to-end
encrypted between the two devices (X25519 + XSalsa20-Poly1305) inside the TLS tunnel. A
Zscaler-style proxy that terminates TLS sees only ciphertext there.

Tether also races port 443 against port 80, and proxy against direct, so the first route that
works wins. See [connection-resilience.md](connection-resilience.md).

## Requirements for IT

- Allow `https://tether-desk.duckdns.org` and `https://tether-desk2.duckdns.org` (TCP 443, WebSocket upgrade on `/ws/id` and `/ws/relay`; the second relay node, see [relay-ha.md](relay-ha.md)).
  With more servers (`TETHER_SERVER` list, policy `servers.list`) allow each of them.
- If WebSockets are inspected, allow long-lived connections. Hosts keep one open permanently and
  send a heartbeat every 20 s.
- Nothing else is needed: no UDP, no inbound rules, no other ports.

## Verification (2026-11-03)

| Test | Setup | Result |
| --- | --- | --- |
| Direct, 443-only | User's PC, host + viewer, `tools/bench/netaudit.ps1` sampling every socket of `tether.exe` | E2E 4/4: registration, login with first frame, 64 MB file transfer with SHA-256 verified, end-to-end encryption reported. Sockets seen: only `158.101.205.185:443`, **no UDP**. |
| Strict proxy | Same, through `HTTPS_PROXY` to a CONNECT proxy that rejects every port except 443 (`tools/bench/corp_proxy.py`) | E2E 4/4, 22 CONNECTs to `:443`, 0 blocked, no UDP, no direct connection. |
| **Zscaler-style corporate PC** | Hyper-V VM: Windows Firewall **blocks all outbound** except the proxy and DNS. The system proxy (WinINet) points to a **TLS-inspecting** mitmproxy whose CA is in `LocalMachine\Root`. Direct TCP 443 and 21116 to the relay are blocked (checked). Your PC is the viewer, the VM is the host. | The VM host registered through the inspecting proxy and the session ran. Glass-to-glass latency **62.8 ms mean** (median 62, p95 87), **27.9 fps**. The proxy saw 1,454 relay frames; their byte statistics look like random data (printable-byte ratio 0.37 ≈ uniform random), meaning ciphertext. |

Reproduce the corporate test:

```powershell
# host: TLS-inspecting, 443-only proxy
mitmdump -s tools/bench/corp_proxy.py --listen-host 0.0.0.0 --listen-port 8080
# elevated: lock down the VM, set its proxy, trust the proxy CA, install the build
tools\bench\vm-corp.ps1
node apps/desktop/e2e/bench-vm.mjs --only tether --id <vm-id>
```

## LAN paths behind a proxy

A corporate rule "internet traffic only through the proxy" says nothing against two PCs on the
same LAN talking to each other, and remote support of the PC next door should not take a detour
through the relay. So with policy `network.direct = auto`, a Tether behind a proxy (viewer or
host, each decides for itself) still offers/answers WebRTC, but **LAN-only** (`WebRTCStream::lan_only` in
`vendor/rustdesk/libs/hbb_common/src/webrtc.rs`):

- no ICE servers at all: no STUN binding request and no TURN allocation is sent, so the public
  address is never learnt or disclosed and nothing goes to UDP 21116 of the relay;
- local candidates only on private addresses (RFC 1918, link-local, IPv6 ULA `fc00::/7`); a
  global IPv6 or public IPv4 address of the PC is never offered;
- remote candidates outside those ranges (a peer's public/STUN address) are dropped, so no
  connectivity check is ever addressed to the internet;
- the offer, answer and candidates travel inside the existing 443 WebSocket through the proxy;
- the session is end-to-end encrypted and authenticated exactly as on the relay (DTLS bound to the
  relay-signed key of the host, then the same password / Accept / permissions).

If no LAN pair forms (different sites, VPN without a route, client isolation, UDP blocked
between the PCs) the relay that was already connected is used after at most 0.4 s. The LAN
lookup and direct TCP (21118/21119) also stay on behind a proxy; they never leave the subnet.
`network.direct = off` turns all of it off: no lookup, no offer, no answer, relay only.

The session window says **Direct (LAN)** when ICE nominated a pair between two private addresses;
the viewer log has the pair: `WebRTC selected pair: (local) udp host 172.30.240.1:... <-> (remote)
udp host 172.30.252.30:... (lan: true)`.

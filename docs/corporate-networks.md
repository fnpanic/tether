# Corporate networks: proxies, zero-trust filters, 443-only egress

Tether uses **exactly one outbound connection type: TCP 443 (WebSocket over TLS)** to the relay
name. It sends no UDP, opens no listening ports for P2P, and makes no direct peer connections. That
is how it gets through corporate proxies and zero-trust gateways such as Zscaler, Netskope or
Palo Alto Prisma.

## How it works

| Part | What it does | Where |
| --- | --- | --- |
| 443-only policy | Overrides that cannot be changed in the UI: `allow-websocket=Y`, `disable-udp=Y`, `api-server=https://<relay>`. P2P, UDP hole punching, IPv6 punching, WebRTC and LAN discovery are all off. NAT and STUN probes are skipped. | `apps/desktop/src-tauri/src/netpolicy.rs`, `vendor/rustdesk/src/common.rs` |
| Proxy auto-detection | Checked in this order: `HTTPS_PROXY` / `ALL_PROXY`, then the system proxy. On Windows that is the WinINet/IE settings, PAC files and WPAD auto-detect (via WinHTTP). On macOS it is `scutil --proxy`. Detection repeats every 60 s, so a VPN or a network change is picked up. Supports HTTP CONNECT, HTTPS and SOCKS5 proxies. | `netpolicy.rs` |
| TLS-inspecting proxies | If the proxy re-signs TLS with a company CA (as Zscaler does), Tether falls back from rustls to the OS TLS stack. That stack trusts the CA the company deployed to the machine. There is never an "accept any certificate" fallback. | `hbb_common::websocket` |
| Relay front end | Caddy on 443 with a Let's Encrypt certificate. `/ws/id` goes to hbbs (rendezvous) and `/ws/relay` to hbbr (relay). | `server/caddy/Caddyfile`, `server/systemd/tether-https.service` |
| Host registration over WebSocket | Upstream open-source hbbs accepts hosts only over UDP. Tether's patched hbbs adds the following: it accepts `RegisterPeer`/`RegisterPk` over TCP/WebSocket with the same UUID, key, IP-change and rate-limit checks; keeps the host's connection as its signalling path; sends empty-frame heartbeats (every 20 s) so proxies don't close idle connections; and gives each proxied client a unique address (forwarded IP plus the connection port). | `patches/rustdesk-server.patch` |

What the inspecting proxy can see: the relay host name, connection timing and sizes, and
rendezvous signalling (device ID, public key, connect requests), because that WebSocket relies on
TLS alone. **It cannot see the session.** Screen, input, clipboard and files are end-to-end
encrypted between the two devices (X25519 + XSalsa20-Poly1305) inside the TLS tunnel. A
Zscaler-style proxy that terminates TLS sees only ciphertext there.

## Requirements for IT

- Allow `https://tether-desk.duckdns.org` (TCP 443, WebSocket upgrade on `/ws/id` and `/ws/relay`).
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

# Production relay deployment

Tether's relay runs on an **Oracle Cloud Always Free** VM and is reached through a permanent free
DNS name. Clients are built against the **name**, never the IP, so an IP change does not break
installed clients.

| Item | Value |
| --- | --- |
| Relay name | `tether-desk.duckdns.org` |
| Server key (`TETHER_PUB_KEY`) | `fe8A7xTX72RJlJypn8aBxG3NZHHxygCLPQqCWxbeev8=` |
| Host | OCI `VM.Standard.E2.1.Micro` (Always Free), Oracle Linux 9, region eu-amsterdam-1 |
| Public IP | ephemeral (free); the DNS name follows it |
| Ports | **TCP 443** (Caddy → `/ws/id`, `/ws/relay`; what Tether clients use) and **TCP 80** (firewalld forwards it to 443, the fallback route; see [connection-resilience.md](connection-resilience.md)). Legacy TCP 21115–21117 and UDP 21116 are still open for stock RustDesk clients and can be closed. |
| Services | `tether-hbbs`, `tether-hbbr` (patched for WebSocket hosts), `tether-https` (Caddy, Let's Encrypt), `tether-api` (policy, heartbeats, releases, download page, diagnostics; `MemoryMax=64M`), `tether-ddns.timer` (systemd, enabled at boot) |
| Admin key | `ADMIN_PUB_KEY` in `/etc/tether/api.env`: the public half of the Tether Manage admin key (comma-separated to trust several during a rotation). Without it the admin endpoints answer 503. |
| Download page | `https://tether-desk.duckdns.org/` (published from Tether Manage; files under `/var/lib/tether-api/download/<channel>/…`) |
| Data | `/var/lib/tether` (key pair `id_ed25519` + peer DB). Back up `id_ed25519`; losing it means rebuilding every client. |

## Cost

Everything used is in Oracle's Always Free tier and costs nothing: one E2.1.Micro instance, the
default VCN, an ephemeral public IP and the default boot volume. DuckDNS is free with no expiry.
No other paid resources were created.

## How it was set up

1. `dist/linux-relay` copied to the VM, then `sudo ./install.sh tether-desk.duckdns.org`. The
   installer opens firewalld and creates the hardened systemd units.
2. OCI security list: ingress rules for TCP 21115–21117 and UDP 21116 from `0.0.0.0/0`.
3. DuckDNS: `/etc/tether/duckdns.env` (root-only, holds the token), plus
   `server/systemd/tether-ddns.{service,timer}`, which refresh the name every 5 minutes and at boot.
4. HTTPS front end (443-only clients, see [corporate-networks.md](corporate-networks.md)):
   - Caddy static binary at `/usr/local/bin/caddy`, system user `tether-https`;
   - `server/caddy/Caddyfile` copied to `/etc/tether/Caddyfile`, with `RELAY_HOST=tether-desk.duckdns.org` in `/etc/tether/server.env`;
   - `server/systemd/tether-https.service` enabled; TCP 443 opened in the OCI security list and firewalld;
   - port 80 fallback: `firewall-cmd --permanent --add-forward-port=port=80:proto=tcp:toport=443 --add-port=80/tcp` plus an OCI ingress rule for TCP 80;
   - the certificate comes from Let's Encrypt via TLS-ALPN-01 on 443 (no port 80 needed) and renews automatically.
5. Relay API (`tether-api`, 0.3.0+): binary at `/usr/local/bin/tether-api` (`install`, not `mv`,
   so SELinux labels are right), user `tether-api`, data in `/var/lib/tether-api` (0750; Caddy's
   user is in group `tether-api` to read `download/`), `server/systemd/tether-api.service`,
   `/etc/tether/api.env` with `ADMIN_PUB_KEY=<base64>`. Restart `tether-https` after changing the
   Caddyfile (its admin API is off). A fresh server gets all of this from
   `sudo ADMIN_PUB_KEY=<key> ./install.sh tether-desk.duckdns.org`.
6. VM tuning for the 1 GB micro shape:
   - kdump disabled and the `crashkernel` reservation removed (it reserved 448 MB, which starved
     the VM until it froze);
   - a 2 GB swap file added as a safety net.

## Building clients for this relay

```bash
TETHER_SERVER=tether-desk.duckdns.org \
TETHER_PUB_KEY=fe8A7xTX72RJlJypn8aBxG3NZHHxygCLPQqCWxbeev8= \
TETHER_ADMIN_PUB_KEY=<admin public key from Tether Manage> \
TETHER_UPDATER_PUB_KEY=<updater public key from Tether Manage> \
npx tauri build --bundles nsis           # apps/desktop (technician installer)
# apps/quick: same env, `npx tauri build` -> TetherQS.exe (portable, no installer)
```

Release builds refuse to compile without the four values. They are stored as GitHub Actions
secrets in `fnpanic/tether` for the macOS and Windows CI builds. Publish the artifacts with
Tether Manage (Releases); see [updates.md](updates.md).

## Operations

```bash
ssh -i ~/.ssh/tether_oracle opc@tether-desk.duckdns.org
sudo systemctl status tether-hbbs tether-hbbr tether-https tether-api tether-ddns.timer
sudo journalctl -u tether-hbbs -f
```

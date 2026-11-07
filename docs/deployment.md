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
| Data | `/var/lib/tether` (key pair `id_ed25519` + peer DB). Back up `id_ed25519`; losing it means a key rotation ([key-rotation.md](key-rotation.md)). |
| Server key rotation | `/etc/tether/keys.env` (`KEYS_ACCEPT=<old public key>` while a new key signs), read by hbbs and hbbr; `server/rotate-key.sh switch|retire|rollback`. Steps: [key-rotation.md](key-rotation.md#operator-steps). |

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
   - Alternative front end: `sudo PROXY=nginx ./install.sh <host>` installs nginx from nginx.org
     with its ACME module instead (same routes, same unit name, 2.7× less memory per device);
     why and how to switch and back: [proxy-benchmark.md](proxy-benchmark.md).
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
   - `dnf-makecache.timer` disabled (`install.sh` does this on VMs under 1.5 GB): its metadata refresh
     needs ~700 MB and stalled relay 1 for several minutes on 2026-11-04 (OOM killer, SSH and HTTPS
     timing out; clients moved to relay 2). On an existing relay:
     `sudo systemctl disable --now dnf-makecache.timer`.

## Relay 2 (high availability)

A second, independent node with the same stack and the **same key pair**
([relay-ha.md](relay-ha.md)).

| Item | Value |
| --- | --- |
| Name | `tether-desk2.duckdns.org` (DuckDNS, `tether-ddns.timer`) |
| Host | OCI instance `tether-relay-2`, `ocid1.instance.oc1.eu-amsterdam-1.anqw2ljr2k2q4vycutwdi4p62yb5ee6ody3rzy6k3kgvyfcpoqiau4sndjtq`, `VM.Standard.E2.1.Micro` (Always Free), Oracle Linux 9.8, AD-1, same subnet and security list as relay 1 |
| Public IP | `92.5.231.71` (ephemeral, free) |
| Key | `/var/lib/tether/id_ed25519{,.pub}` copied from relay 1 (`fe8A7xTX…`) |
| Relay list | `/etc/tether/server.env`: `RELAY_SERVERS=tether-desk2.duckdns.org\|http://127.0.0.1:21121/readyz,tether-desk.duckdns.org\|https://tether-desk.duckdns.org/readyz/relay` |
| Admin key | `ADMIN_PUB_KEY=LdVr9FeDLNbgO6T8PPHddPsicLPr5Hkly1MXqmaLwKI=` |
| Tuning | crashkernel removed, kdump disabled, 2 GB `/swapfile` (as relay 1) |

Cost check at creation (2026-11-03): `vm-standard-e2-1-micro-count` limit 2, 1 used before; the
Always Free block storage allowance is 200 GB, 47 GB used before, plus the new 47 GB default boot
volume. No other resources were created.

Set up with the bundle from this repository (binaries with drain support):

```bash
sudo DUCKDNS_DOMAIN=tether-desk2 DUCKDNS_TOKEN=<token> ADMIN_PUB_KEY=<key> \
  RELAY_SERVERS='tether-desk2.duckdns.org|http://127.0.0.1:21121/readyz,tether-desk.duckdns.org|https://tether-desk.duckdns.org/readyz/relay' \
  ./install.sh tether-desk2.duckdns.org      # after copying the key pair into /var/lib/tether
```

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
secrets in `fnpanic/tether` for the macOS and Windows CI builds. After a server key rotation
(phase 2 of [key-rotation.md](key-rotation.md)) set `TETHER_PUB_KEY` (secret and
`scripts/build-macos.sh`) to the new key, so new installs start with it; installed clients follow
the signed key list and need no reinstall. Publish the artifacts with
Tether Manage (Releases); see [updates.md](updates.md).

## Operations

```bash
ssh -i ~/.ssh/tether_oracle opc@tether-desk.duckdns.org
sudo systemctl status tether-hbbs tether-hbbr tether-https tether-api tether-ddns.timer
sudo journalctl -u tether-hbbs -f
```

## Update: hbbs with WebRTC signalling and STUN (direct IPv6 / NAT hole punching)

Only `hbbs` changes (bundle `dist/linux-relay`, checksums in `dist/SHA256SUMS`; since 2026-11-05 it also contains the TURN server, see [stun-turn.md](stun-turn.md)). No firewall, OCI,
key, `api.env` or Caddy change: the STUN responder uses UDP 21116, which firewalld and the OCI
security list (`Default Security List for vcn-20261103-1048`, ingress `udp 0.0.0.0/0 21116`)
already open on both relays. Relayed sessions do not use hbbs and keep running; hosts
re-register within seconds. One relay at a time, relay 2 first:

```bash
# from the PC, Git Bash, in C:\install\remote-support
grep linux-relay/bin/hbbs dist/SHA256SUMS; sha256sum dist/linux-relay/bin/hbbs   # must match
for ip in 92.5.231.71 158.101.205.185; do
  scp -i ~/.ssh/tether_oracle dist/linux-relay/bin/hbbs opc@$ip:/tmp/hbbs.new
  ssh -i ~/.ssh/tether_oracle opc@$ip 'set -e
    sudo cp -p /usr/local/bin/hbbs /usr/local/bin/hbbs.bak-0.5
    sudo install -m 0755 /tmp/hbbs.new /usr/local/bin/hbbs && rm /tmp/hbbs.new
    sudo restorecon /usr/local/bin/hbbs
    sudo systemctl restart tether-hbbs
    sleep 3; systemctl is-active tether-hbbs; curl -s localhost:21122/readyz'
  python tools/turn-bench/turn-check.py $ip   # STUN ok + TURN Allocate -> 401
done
```

Rollback per relay: `sudo install -m 0755 /usr/local/bin/hbbs.bak-0.5 /usr/local/bin/hbbs && sudo systemctl restart tether-hbbs`.
Clients from 0.5.x keep working with the new hbbs (they send no offer); new clients against the
old hbbs stay on the relay (see [relay-ha.md](relay-ha.md#ipv6-and-nat-hole-punching-webrtcice)).

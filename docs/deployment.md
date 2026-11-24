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
| Ports | **TCP 443** (Caddy → `/ws/id`, `/ws/relay`; what Tether clients use) and **TCP 80**. `install.sh` only opens port 80; the front end listens there and answers with a redirect to https. On the production relays a firewalld forward of 80 to 443 was added by hand (step 4 below), which is what serves TLS on port 80 for the fallback route in [connection-resilience.md](connection-resilience.md); `install.sh` does not add it (its comment says never to forward 80 to the TLS port). Legacy TCP 21115–21117 and UDP 21116 are still open for stock RustDesk clients and can be closed. |
| Services | `tether-hbbs`, `tether-hbbr` (patched for WebSocket hosts), `tether-https` (Caddy, Let's Encrypt), `tether-api` (policy, heartbeats, releases, download page, diagnostics; `MemoryMax=64M`), `tether-ddns.timer` (systemd, enabled at boot) |
| Admin key | `ADMIN_PUB_KEY` in `/etc/tether/api.env`: the public half of the Tether Manage admin key (comma-separated to trust several during a rotation). Without it the admin endpoints answer 503. |
| Device status | Optional health reports from a monitoring agent ([health.md](health.md)); on by default, needs `ADMIN_PUB_KEY`. Settings in `/etc/tether/api.env`: `STATUS=off` switches it off (every status route answers 503), `STATUS_TARGET_RPS` (default 10 posts per second the devices are paced to), `STATUS_MAX_DEVICES` (default 20000; more needs a larger `MemoryMax` in `tether-api.service`, [scaling.md](scaling.md)). Data in `/var/lib/tether-api/status/`: back up `keys.jsonl` and `revoked.json` with the rest of `/var/lib/tether-api`. Without `keys.jsonl` every device has to enrol again; without `revoked.json` revoked enrolment tokens work again until they expire. The other files there are reports and caches. An unreadable `keys.jsonl` or `revoked.json` locks only the status routes (503) until it is repaired and tether-api restarted ([health.md](health.md#the-locked-state)). Deploy order before the first monitor publish, on every relay: the new tether-api binary first, then the proxy configuration (Caddyfile or nginx.conf, which sends `monitor.json` to tether-api) ([updates.md](updates.md#device-status-agent-monitorjson-and-enrolment-tokens)). |
| Download page | `https://tether-desk.duckdns.org/` (published from Tether Manage; files under `/var/lib/tether-api/download/<channel>/…`) |
| Data | `/var/lib/tether` (key pair `id_ed25519` + peer DB). Back up `id_ed25519`; losing it means a key rotation ([key-rotation.md](key-rotation.md)). |
| Several organisations | One deployment (relay, keys, roster, builds) per organisation whose technicians must not see another's machines: [several-organisations.md](several-organisations.md). |
| Server key rotation | `/etc/tether/keys.env` (`KEYS_ACCEPT=<old public key>` while a new key signs), read by hbbs and hbbr; `server/rotate-key.sh switch|retire|rollback`. Steps: [key-rotation.md](key-rotation.md#operator-steps). |

## Cost

Everything used is in Oracle's Always Free tier and costs nothing: one E2.1.Micro instance, the
default VCN, an ephemeral public IP and the default boot volume. DuckDNS is free with no expiry.
No other paid resources were created.

## How it was set up

1. `dist/linux-relay` copied to the VM, then `sudo ./install.sh tether-desk.duckdns.org`. The
   installer opens firewalld and creates the hardened systemd units. It gives existing key files
   in `/var/lib/tether` to user `tether` (private ones 0600; hbbs and hbbr exit when they cannot read
   the key), and ends with an error when one of the four services does not stay up (active 4
   seconds in a row within 30 s). On hosts with raw iptables it inserts its rules before the first
   REJECT rule.
2. OCI security list: ingress rules for TCP 21115–21117 and UDP 21116 from `0.0.0.0/0`.
3. DuckDNS: `/etc/tether/duckdns.env` (root-only, holds the token), plus
   `server/systemd/tether-ddns.{service,timer}`, which refresh the name every 5 minutes and at boot. The service runs as a systemd
   `DynamicUser` (not root) and passes the URL with the token to curl on stdin, so the token is in
   no command line.
4. HTTPS front end (443-only clients, see [corporate-networks.md](corporate-networks.md)):
   - Caddy static binary at `/usr/local/bin/caddy`, system user `tether-https`;
   - `server/caddy/Caddyfile` copied to `/etc/tether/Caddyfile`, with `RELAY_HOST=tether-desk.duckdns.org` in `/etc/tether/server.env`;
   - `server/systemd/tether-https.service` enabled; TCP 443 opened in the OCI security list and firewalld;
   - port 80 fallback, added by hand on these relays (not by `install.sh`, which only opens 80 so the front end can redirect to https): `firewall-cmd --permanent --add-forward-port=port=80:proto=tcp:toport=443 --add-port=80/tcp` plus an OCI ingress rule for TCP 80;
   - the certificate comes from Let's Encrypt via TLS-ALPN-01 on 443 (no port 80 needed) and renews automatically.
   - Alternative front end: `sudo PROXY=nginx ./install.sh <host>` installs nginx from nginx.org
     with its ACME module instead (same routes, same unit name, 2.7× less memory per device);
     why and how to switch and back: [proxy-benchmark.md](proxy-benchmark.md). The switch also works
     on a fresh node, and `server.env` (which names the proxy the next run keeps) is written only
     after the new proxy config passed its test. nginx sends `releases.json`, `releases-v2.json` and `monitor.json`
     to tether-api and has no `gzip_static`.
5. Relay API (`tether-api`, 0.3.0+): binary at `/usr/local/bin/tether-api` (`install`, not `mv`,
   so SELinux labels are right), user `tether-api`, data in `/var/lib/tether-api` (0750; Caddy's
   user is in group `tether-api` to read `download/`), `server/systemd/tether-api.service`,
   `/etc/tether/api.env` with `ADMIN_PUB_KEY=<base64>`. The unit also reads `/etc/tether/server.env`
   for `RELAY_HOST`: tether-api accepts the signed requests of current apps (`Tether-Sig: v2`) only
   for that name, and answers them with 401 without it. `TETHER_SIG_V1=off` in `api.env` refuses the
   older signatures once every Tether Manage and technician app is updated
   ([api.md](api.md#admin-endpoints-signed-requests)). **Update order:** first every relay, with
   `RELAY_HOST` set to the host name(s) entered for it in Tether Manage; then Tether Manage and
   the apps; last `TETHER_SIG_V1=off`. The apps do not fall back to v1 by themselves.
   Roll out the new `Caddyfile` / `nginx.conf` together with the new tether-api: tether-api closes
   an idle connection after 30 s, and the proxies now keep idle connections to it for 20 s only
   (`keepalive 20s` / `keepalive_timeout 20s`).
   Docker: the containers run as uid 65532. On an **existing** docker-compose relay run
   `sudo chown -R 65532:65532 data && sudo chmod 0700 data` before `docker compose up -d` with the
   new image; otherwise hbbs and hbbr cannot open their database and key and restart in a loop. Restart `tether-https` after changing the
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

A technician app for a second organisation that must install beside this one on the same PC is
built with `TETHER_INSTANCE` and the `--config` of `scripts/instance-config.sh`
([several-organisations.md](several-organisations.md#two-organisations-on-one-pc)).

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

## Update: review fixes (hbbs, tether-api, Caddyfile, ddns unit)

Rolled out on 2026-11-10 to Oracle relay 2 (backup) and then Hetzner (primary), see [relay-ha.md](relay-ha.md). The bundle `dist/linux-relay` is built with
`scripts/build-relay.sh` and tested with `scripts/test-relay.sh`; `dist/SHA256SUMS` lists it.
`scripts/deploy-relay-update.sh` rolls it out, one relay at a time, the backup first (its `RELAYS` default names both nodes and their SSH keys):

```bash
# from the PC, Git Bash, in C:\installemote-support
bash scripts/deploy-relay-update.sh            # HBBR=1 also replaces hbbr (ends relayed sessions on that node)
```

Per relay it checks first (stops without changing anything when a check fails): the `tether` user
can read `/var/lib/tether/id_ed25519` (the new hbbs and hbbr exit otherwise), `RELAY_HOST` is in
`/etc/tether/server.env` (tether-api needs it for the request signatures of current apps), Caddy
is the front end, and the new Caddyfile validates. Then it installs tether-api with its unit (the
unit now also reads `server.env`), the Caddyfile (request buffers, 20 s keep-alive to tether-api)
and hbbs, restarts tether-api, tether-hbbs and tether-https, replaces the ddns unit where one
exists (and puts the old one back if the update does not answer OK), and checks that all four
services are active, that tether-api got `RELAY_HOST`, `localhost:21122/readyz` and
`https://<host>/api/policy`. Hosts re-register within seconds; WebSockets through Caddy reconnect.
Relayed sessions keep running unless `HBBR=1`. The replaced files stay beside the new ones as
`*.bak-review`; the rollback commands are at the end of the script.

After both relays: update Tether Manage and the apps, and when every Manage and technician app is
updated set `TETHER_SIG_V1=off` in `/etc/tether/api.env` and restart tether-api
([management.md](management.md), [api.md](api.md)).

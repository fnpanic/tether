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
| Ports | TCP 21115–21117, UDP 21116 (OCI security list + firewalld) |
| Services | `tether-hbbs`, `tether-hbbr`, `tether-ddns.timer` (systemd, enabled at boot) |
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
4. VM tuning for the 1 GB micro shape:
   - kdump disabled and the `crashkernel` reservation removed (it reserved 448 MB, which starved
     the VM until it froze);
   - a 2 GB swap file added as a safety net.

## Building clients for this relay

```bash
TETHER_SERVER=tether-desk.duckdns.org \
TETHER_PUB_KEY=fe8A7xTX72RJlJypn8aBxG3NZHHxygCLPQqCWxbeev8= \
npx tauri build --bundles nsis
```

The same two values are stored as GitHub Actions secrets in `fnpanic/tether` for the macOS and
Windows CI builds.

## Operations

```bash
ssh -i ~/.ssh/tether_oracle opc@tether-desk.duckdns.org
sudo systemctl status tether-hbbs tether-hbbr tether-ddns.timer
sudo journalctl -u tether-hbbs -f
```

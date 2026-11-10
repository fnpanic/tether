# Paseo on a Zscaler Mac → Windows PC

> Own Paseo relay on the Hetzner node (`tether-desk.duckdns.org:443`, path `/ws` via a Tether Caddy drop-in):
> setup guide [`relay/setup.html`](relay/setup.html), deploy files in [`relay/`](relay/).

**Problem:** Paseo on the Mac (managed: Zscaler Client Connector + Microsoft Defender) cannot
connect to the Paseo daemon on the Windows PC. The PC shows up as *offline: "Cannot reach host.
Check the connection; retrying automatically."*

**Cause:** Zscaler inspects TLS. Every HTTPS/WSS connection, including `wss://relay.paseo.sh`, is
re-signed by **"Zscaler Root CA"**, which MDM installed into the macOS System keychain. Safari and
`curl` trust it. Paseo's daemon (the process that connects to other hosts) and CLI are
Electron/Node and only trust **Node's built-in CA list**, so the TLS handshake fails with
`unable to get local issuer certificate` and the relay is never reached. Defender is not involved,
and Zscaler does **not** block the relay (WebSocket upgrade → `101`).

**Fix:** `mac/paseo-corp-ca.sh` exports the System keychain's certificates and points Node's
`NODE_EXTRA_CA_CERTS` at them for every Paseo process. Nothing changes on the PC.

## Install (on the Mac, once)

The script was copied to `~/paseo-corp-ca.sh` on the Mac (source: [`mac/paseo-corp-ca.sh`](mac/paseo-corp-ca.sh)). In Terminal:

```bash
bash ~/paseo-corp-ca.sh install --restart
```

It:
1. exports the certificates of `/Library/Keychains/System.keychain` to `~/.paseo/corp-ca.pem`;
2. **verifies** that a TLS handshake with `relay.paseo.sh` fails without and works with the bundle
   (aborts and changes nothing if the bundle doesn't fix it);
3. installs the LaunchAgent `local.paseo-corp-ca` (at login + daily: re-export the CAs, so a Zscaler
   CA rotation is picked up, and `launchctl setenv NODE_EXTRA_CA_CERTS`);
4. adds one guarded line to `~/.zprofile`. Paseo Desktop copies the login-shell environment into
   the daemon and its agents, and Node tools in Terminal (npm, claude, …) benefit too;
5. `--restart`: restarts Paseo (app + desktop-managed daemon) so they pick it up. It refuses to
   restart while agents run on the Mac (`FORCE=1 bash ~/.paseo/bin/paseo-corp-ca.sh restart`
   overrides).

Then, in Paseo on the Mac: **Settings → Add host → paste the PC's pairing link**. On the PC:
Settings → GameMachine → Pair a device, or `paseo daemon pair` in a terminal. Or from Terminal:

```bash
/Applications/Paseo.app/Contents/Resources/bin/paseo host add 'relay://relay.paseo.sh:443/<serverId>?key=…' --label GameMachine
/Applications/Paseo.app/Contents/Resources/bin/paseo host ls      # status: online
```

Check anytime: `bash ~/.paseo/bin/paseo-corp-ca.sh doctor ['relay://…']`. It prints the TLS result with and
without the CA, the launchd/zprofile/LaunchAgent state, whether the running daemon has the variable,
its relay link, and (with a pairing URI) an end-to-end check against the PC.

Undo: `bash ~/.paseo/bin/paseo-corp-ca.sh uninstall` (then restart Paseo).

## Pairing the iOS app with the Mac

The phone talks to the Mac daemon through the same relay, so it needs the CA fix above on the
Mac (the phone itself does not). Watch out for **which host's QR code** you show. The Mac app shows
the QR code of the host selected in Settings. With **GameMachine** (the PC) selected, the phone
pairs with the PC again ("nothing happens", because it already knows the PC). Select the Mac's own host
(This Mac), then **Pair a device**. Or print the Mac's offer in Terminal on the Mac:

```bash
/Applications/Paseo.app/Contents/Resources/bin/paseo daemon pair   # QR in the terminal + link
```

Scan it from **inside the Paseo iOS app** (Add host → Scan QR code), not with the Camera app, or
paste the link (Add host → Paste pairing link). Seen on 2026-11-10: the Mac app fetched the
PC's offer (`Loaded daemon keypair` in the PC log) and 7 s later the phone connected to the
PC; the Mac daemon never saw a relay client. The PC reached the Mac through the relay in 1.1 s,
so the Mac side was fine.

## Evidence (2026-11-10, Mac 15.7.4 arm64, Paseo 0.15.3 on both sides)

| Check | Result |
| --- | --- |
| `curl -v https://relay.paseo.sh` on the Mac | issuer `Zscaler Intermediate Root CA (zscloud.net)`; system trust OK; WS upgrade `101 Switching Protocols` |
| Mac daemon log (every 30 s) | `relay-transport … unable to get local issuer certificate` → `relay_control_disconnected` |
| `paseo --host relay://… daemon status` (Mac → PC) | `DAEMON_UNREACHABLE … unable to get local issuer certificate` |
| same with `NODE_EXTRA_CA_CERTS=<Zscaler Root CA>` | PC status returned; `ls -a` lists the PC's agents |
| `NODE_OPTIONS=--use-system-ca` | does not work: Electron disables `NODE_OPTIONS` ("invoked by other apps") |
| PC on the LAN (192.168.178.30:6767) | closed: the PC daemon listens on `127.0.0.1` only (Paseo default; Wi-Fi profile "Public"). No direct-LAN change needed. |

[`mac/test-e2e.sh`](mac/test-e2e.sh) runs on the Mac without touching the user's Paseo setup. It
starts a throwaway daemon (own `PASEO_HOME`, port 6799) twice and adds the PC as a host:

```
PASS tls to relay fails without CA
PASS tls to relay ok with CA (issuer Zscaler)
PASS mac -> relay -> pc via CLI with CA
PASS negative control: no CA -> pc host not connected     ("status": "offline", host_offline)
PASS with CA: temp daemon's relay link connects
PASS with CA: pc registered and connected on the mac daemon ("status": "online")
PASS with CA: list pc agents through the relay
ALL PASS
```

Run it from the PC through the Mac build runner ([docs/mac-build-runner.md](../docs/mac-build-runner.md)):

```bash
W=$(mktemp -d); cp paseo-tunnel/mac/*.sh "$W"/
paseo daemon pair --json | python -c "import json,sys;print(json.load(sys.stdin)['connectionUri'],end='')" > "$W/uri.txt"
python tools/macbuild/macbuild.py submit --user tether --work "$W" --timeout 900 -- bash test-e2e.sh uri.txt
python tools/macbuild/macbuild.py wait --user tether <id> --interval 45
```

## Security notes

- Relay traffic stays **end-to-end encrypted** (Curve25519 + NaCl box between the Mac and the PC
  daemon). Zscaler terminates the outer TLS but only sees ciphertext, as does the relay.
- The bundle only adds certificates that the Mac's administrator already trusts system-wide; Node
  keeps its own CA list too (`NODE_EXTRA_CA_CERTS` adds, it does not replace).
- The pairing link (`relay://…?key=…`) is the trust anchor: share it like a password.
- If corporate policy ever blocks `relay.paseo.sh`, there is no workaround here. Ask IT to allow it,
  or add `relay.paseo.sh` to the SSL-inspection bypass (then even the CA fix is unnecessary).

# Server key rotation

How to replace the relay's server key pair (`/var/lib/tether/id_ed25519`, the key clients know as
`TETHER_PUB_KEY`) without reinstalling a single client.

## The problem

hbbs signs every device identity it hands out (`IdPk`: device ID + device public key) with the
server key. A viewer accepts a host only when that signature verifies with the server public key
it trusts, and it presents that public key to hbbs and hbbr as `licence_key` ("this is a Tether
client"). Up to 0.5.14 the only trusted key is the one compiled in (`TETHER_PUB_KEY`), so a new
server key meant rebuilding and reinstalling every client.

## Design

**Trust anchor: the admin key.** Clients already carry the admin public key
(`TETHER_ADMIN_PUB_KEY`) and verify every policy with it. The same key now signs a small
**server key list**: a separate signed document, published and cached exactly like the policy.

| | |
| --- | --- |
| Type | `tether_trust::serverkeys::KeyList` inside the usual `envelope::Signed` |
| Payload | `{"serial": 3, "issued_at": 1794000000, "keys": [{"key": "<b64 pubkey>", "not_before": 0, "expires": 0}, …]}` |
| Published | `PUT /api/admin/server-keys` (signed admin request) on every relay, by Tether Manage |
| Fetched | `GET /api/server-keys` (public, `ETag: "<serial>"`), on every policy check (both apps, every process role that checks the policy) |
| Cached | `server-keys.json` in the config folder (the signed document, re-verified on every start) |

Why a separate document and not a field of the policy: `policy::Bundle` rejects unknown fields,
so a policy with a new field would be refused **as a whole** by every client from before this
change, which would freeze their policy during the rotation. Old clients simply never ask for the
key list.

Rules (`tether_trust::serverkeys`, unit-tested there):

- The list is valid only if signed by a compiled-in admin key, 1–8 distinct Ed25519 keys (canonical
  base64, the exact string hbbs/hbbr compare), and at least one key has no `expires` (so no device clock can leave a device without any key).
- **Monotonic serial:** a list with a lower serial than the cached one is rejected; the same
  serial changes nothing. A relay can withhold a newer list but never roll a device back or
  widen its key set. The highest accepted serial is also kept in `server-keys.serial` beside the
  cache (`<serial> <signing admin key>`), so a device whose `server-keys.json` was deleted or
  replaced still refuses older lists (and does not load an older cached one). That floor counts
  only while the admin key that signed it is trusted, so a build with another admin key starts
  over.
- A key is **active** when `not_before <= now < expires` (0 = no limit). `expires` is strict;
  `not_before` is soft: if no key is active at the device's clock, the keys that have not expired
  are used.
- **Trusted keys** = the active keys of the latest verified list. Without a list: only the
  compiled-in key. The compiled-in key is *not* added implicitly, so a list without it retires it.
- **Presented key** (`licence_key` to hbbs and hbbr) = the **first** active key. The order is the
  admin's choice and is what moves devices from the old to the new key.
- Where the core uses the server key (`crate::get_key` → `licence_key` of `PunchHoleRequest` and
  `RequestRelay`, `secure_connection` verifying the signed `IdPk`, the rendezvous key exchange on
  plain TCP), it now uses `hbb_common::config::server_key()` (presented key) and
  `server_keys_for(key)` (every trusted key when `key` is one of them; a custom server's own key
  stays the only key for that server). WebSocket transport, TURN and the API (TLS with WebPKI)
  do not use the server key.

**Servers accept several keys.** hbbs and hbbr still sign with `id_ed25519` (`-k _`), and accept
a `licence_key` equal to their own public key **or** any key in `KEYS_ACCEPT` (comma-separated
base64 public keys, environment; malformed entries are logged and ignored). VMs:
`/etc/tether/keys.env`, read by both units; Helm: `key.accept`. One restart switches.

**Devices report their list.** The heartbeat carries `server_keys_serial` (left out while a device
has no list, so older relay APIs keep accepting the heartbeat). Tether Manage counts the devices
active in the last 24 h that report the live list.

## Phases

Old key **A** (in every installed client), new key **B**.

| Phase | Who | What | Devices with the list | Devices without it (old version, offline the whole time) |
| --- | --- | --- | --- | --- |
| 0. Prepare | operator | Relays run hbbs/hbbr/tether-api with `KEYS_ACCEPT` support; clients run a version with key-list support (Manage: Devices, version column). | – | – |
| 1. Announce | Manage | Generate B (sealed in the vault). Publish list **[A, B]** (serial n). Wait until Manage shows enough devices on serial n. | present A, trust A and B | present A, trust A |
| 2. Switch | operator | Each relay: `id_ed25519` := B, `KEYS_ACCEPT=A`, restart (one relay at a time). | work (signatures by B are trusted) | **fail**: B's signature is not trusted ("server key not trusted" message) |
| 3a. Retire on devices | Manage | Publish list **[B]** (serial n+1). Wait for the count again. | present B, trust only B | still fail as in 2 |
| 3b. Retire on relays | operator | Each relay: `KEYS_ACCEPT=` (empty), restart. | work | also rejected by hbbs ("server did not accept this app's key" message) |
| 4. New installs | builder | Build with `TETHER_PUB_KEY=B` from now on. | – | – |

Every step is safe to run in this order, and a device that misses a step heals on its next policy
check: the list comes over HTTPS from the relay API, which does not depend on the server key. So a
device only stays broken if it cannot fetch the list at all (versions from before this change, or
a device offline for the whole window that is then too old). `tether --policy-refresh` on a device
fetches policy and key list at once.

Optional: in phase 1 Manage can give A an expiry date (at least an hour ahead; Manage refuses
anything sooner). Devices then stop trusting A at that time
on their own (useful after a suspected key compromise, see below), so phase 3a is only needed
for devices whose clock is wrong. Relays must have switched (phase 2) before that date, otherwise
devices present B to relays that do not accept it yet.

## Threat model

| Threat | Result |
| --- | --- |
| Compromised or malicious relay / API serves a forged list | Rejected: not signed by the admin key. |
| Relay serves an older list (rollback to a retired key) | Rejected: serial lower than the cached one, or than the floor in `server-keys.serial` when the cache is gone. |
| Relay reports or serves a huge serial (to exhaust the serial space) | Manage takes the next serial (key list and policy) only from documents that verify with its admin key and from its own published ones, with an overflow check. |
| Relay sends an endless or huge answer | Clients read at most 16 KiB (key list) / 512 KiB, in TetherQS 256 KiB (policy) plus one byte, then fail. |
| Operator marks the switch done while a relay still signs with the old key | Manage enables "Every relay is switched" only when every relay's hbbs reports the new key (`/rendezvous/pubkey`). |
| Saved `id_ed25519` read by another user of the admin PC | Created with an ACL for the current user only (0600 on unix); Manage keeps warning until **Delete key files**. |
| Relay withholds the newest list | Devices keep the last list (they keep trusting what was trusted). Mitigation: the expiry on the old key; Manage shows per relay which serial it serves. |
| Stolen old server key (e.g. a relay was compromised) | With it an attacker who also gets into the path (a relay, or DNS plus a certificate for the relay name) can forge device identities for devices that still trust it. Rotate: phase 1 with an expiry on A, phase 2 at once, phase 3a. |
| Stolen admin key | It could already push any policy; now it can also make devices trust another server key. Still needs the path into the relay connection (TLS) to MITM. Admin key stays offline in the passphrase-sealed vault. |
| Local attacker writes `server-keys.json` | Must be signed by the admin key and not below the floor; deleting it falls back to the compiled-in key until the next fetch, which cannot bring back an older list. Deleting the floor too, or anything else a local admin does, is out of scope (endpoint security). |
| Device clock wrong | Far ahead: expired keys drop out, but one key never expires. Behind: `not_before` is ignored when nothing else is active. |

The server key never protects session content: that is end-to-end encrypted between the devices
(SECURITY.md). It authenticates which device public key belongs to an ID.

## Failure cases

| Case | What the user sees | Fix |
| --- | --- | --- |
| Viewer without the list after phase 2 | "Connection failed: the server's key is not trusted by this Tether …" | Policy check (automatic within `policy.refresh-minutes`, or `tether --policy-refresh`), else update / reinstall |
| Viewer still presenting A after phase 3b | "The server did not accept this Tether's server key …" | Same |
| Host still presenting A after phase 3b | hbbr drops its relay connection; the viewer gets a timeout | Same, on the host |
| List published before the relays accept its first key | Viewers present a key the relays refuse (message above) | Publish the previous order again (Manage), or finish phase 2 |
| A relay was skipped in phase 2 | Devices trust A and B, so both relays work | Switch it later |
| Rollback of phase 2 | `sudo bash rotate-key.sh rollback` puts A back as signer; devices with [A, B] keep working | – |
| Old hbbs/hbbr without `KEYS_ACCEPT` | `rotate-key.sh` refuses to switch | Update the binaries first (phase 0) |
| `switch` again while a switch is in progress (`id_ed25519.old` is another key) | `rotate-key.sh` refuses: it would drop the key devices still present from `KEYS_ACCEPT` | `retire` or `rollback` first |
| Relay API without the endpoint | Manage shows the error per relay; devices get 404 and keep their list | Update tether-api |

## Checking the switch

hbbs reports the public key it signs with on its health port: `GET /pubkey` on `:21122` (one
base64 line; `404` when hbbs runs without a private key). Caddy exposes it as
`https://<relay>/rendezvous/pubkey` (Helm: the proxy routes it to `hbbs-0`; all hbbs pods share the
key). The key is public (clients get it with every signed identity), so the route needs no
authentication. It shows what the running hbbs uses, not what is on disk, so a relay whose
restart failed still shows the old key. Tether Manage reads it for every relay and enables
**Every relay is switched** only when all of them report the new key; a relay with an older hbbs
or Caddyfile reports nothing (Caddy's catch-all answers "Tether relay") and keeps the step closed
until it is updated. The check trusts the relay's answer: it guards against a forgotten relay, not
against a malicious one.

```bash
curl -s https://tether-desk.duckdns.org/rendezvous/pubkey   # -> the key in id_ed25519.pub
```

## Operator steps

### VM relays (`dist/linux-relay`)

`server/rotate-key.sh` (copy it with the bundle) does the file work, checks that the binaries
support `KEYS_ACCEPT`, adds `/etc/tether/keys.env` to both units (systemd drop-in on relays
installed before this change) and restarts hbbs and hbbr. hbbr drains (up to 300 s): run it on
one relay at a time.

```bash
# phase 2, per relay: Tether Manage › Keys › Rotate server key › "Save key files…" wrote
# id_ed25519 and id_ed25519.pub (the new key B) into a folder on this PC (id_ed25519 readable by
# your user only; "Delete key files" in Manage removes both once every relay is switched)
scp -i ~/.ssh/tether_oracle id_ed25519 id_ed25519.pub server/rotate-key.sh opc@<relay>:/tmp/
ssh -i ~/.ssh/tether_oracle opc@<relay> 'sudo bash /tmp/rotate-key.sh switch /tmp/id_ed25519 && rm /tmp/id_ed25519 /tmp/id_ed25519.pub'
#   -> B signs, A is accepted (KEYS_ACCEPT in /etc/tether/keys.env), A kept as id_ed25519.old
# phase 3b, per relay, after the retire list reached enough devices:
ssh -i ~/.ssh/tether_oracle opc@<relay> 'sudo bash /tmp/rotate-key.sh retire'
# undo phase 2 (A signs again):
ssh -i ~/.ssh/tether_oracle opc@<relay> 'sudo bash /tmp/rotate-key.sh rollback'
```

### Kubernetes (Helm)

```bash
# phase 2: new private key, old public key accepted
helm upgrade tether deploy/helm/tether-relay --reuse-values \
  --set key.privateKey="$(cat id_ed25519)" --set key.accept="<A public key>"
# (key.existingSecret: update the Secret's id_ed25519, set key.accept, then
#  kubectl rollout restart statefulset -l app.kubernetes.io/instance=tether)
# phase 3b
helm upgrade tether deploy/helm/tether-relay --reuse-values --set key.accept=""
```

The pods roll one at a time (hbbr drains), as for every chart change ([kubernetes.md](kubernetes.md)).

### New builds

Set `TETHER_PUB_KEY=<B>` for every build after phase 2 (GitHub secret `TETHER_PUB_KEY`,
`scripts/build-macos.sh` default, [deployment.md](deployment.md)). A new install with B works with
switched relays at once; a new install that still has A works as long as the live list names A or
relays accept it, and picks up the list on its first policy check.

## Code

- `crates/tether-trust/src/serverkeys.rs`: list type, validation, active/presented keys, cache.
- `vendor/rustdesk/libs/hbb_common/src/config.rs`: `POLICY_SERVER_KEYS`, `server_key()`,
  `server_keys_for()`; `vendor/rustdesk/src/common.rs` (`get_key`, `key_exchange`),
  `vendor/rustdesk/src/client.rs` (`secure_connection`, `LICENSE_MISMATCH`) → `patches/hbb_common.patch`, `patches/rustdesk.patch`.
- `vendor/rustdesk-server/src/common.rs` `key_accepted` (hbbs `handle_punch_hole_request`, hbbr
  `make_pair_`), `SIGNING_PUB_KEY` / `/pubkey` on the health endpoint → `patches/rustdesk-server.patch`;
  `server/caddy/Caddyfile` and `deploy/helm/tether-relay/templates/_caddyfile.tpl` (`/rendezvous/pubkey`).
- `server/api`: `GET /api/server-keys`, `PUT /api/admin/server-keys`, heartbeat
  `server_keys_serial` ([api.md](api.md)).
- Apps: `apps/desktop/src-tauri/src/policy.rs`, `apps/quick/src-tauri/src/policy.rs`.
- Tether Manage: Keys › Server key ([management.md](management.md#server-key)).
- `server/rotate-key.sh`, `server/systemd/tether-hbb{s,r}.service`, `deploy/helm/tether-relay`.
- End-to-end: `apps/desktop/e2e/keyrot-e2e.mjs` (local hbbs/hbbr, two clients, three phases).

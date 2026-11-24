# Tether security model

Privacy and security come before convenience. This file lists what Tether protects, how, and where
each control lives in the code. If you change one of these controls, update this file in the same
change.

## Threat model

How a connection is encrypted, step by step with code references, is in
[docs/encryption.md](docs/encryption.md). This section lists what we protect, from whom, and what
is left.

### Assets

| Asset | Where it lives |
| --- | --- |
| Session content: screen, keystrokes, clipboard, files, audio, chat | Only on the two endpoints; in transit end-to-end encrypted |
| Control of a host | Granted by password, 2FA and/or the Accept click on the host |
| Device identity key (Ed25519) | Each device's config file (`Tether.toml`), unencrypted, `0600` on Unix |
| Server key (Ed25519) | `/var/lib/tether/id_ed25519` on every relay node |
| Admin key, updater key | Tether Manage vault on the admin's PC (argon2id + XChaCha20-Poly1305) |
| Host secrets at rest | Permanent password hash, 2FA secret, remembered passwords (obfuscated, see below) |
| Metadata | Device IDs, IPs, machine uuids, connection times on hbbs/hbbr/tether-api; audit log |
| Device health reports | `status/` on each relay: pinned device keys, the last report and the history of each device that runs a monitoring agent; readable by the relay operator and team members |
| Mac build signing material | Sent per job to the Mac build runner ([docs/mac-build-runner.md](docs/mac-build-runner.md)) |

### Trust boundaries

1. **Endpoint ↔ endpoint**: the only place session content is in clear. Each endpoint trusts its
   own OS and user account.
2. **Endpoint ↔ relay node** (Caddy/nginx, hbbs, hbbr, TURN, tether-api): TLS on 443. The relay
   sees metadata, not content. hbbs is trusted to vouch for host keys (it signs them with the
   server key), which is the one place a relay node is trusted with more than availability.
3. **Admin PC ↔ everything**: the admin key signs policies, server key lists, team rosters and
   custom apps; the updater key signs code. Clients verify both against compiled-in public keys.
4. **Local user ↔ Tether processes**: the local IPC and config folder; Tether's own windows are
   guarded against remote input ([below](#tethers-own-windows-are-for-the-local-user-only-056)).

### Actors

| Actor | Can | Cannot (mitigation) | Residual risk |
| --- | --- | --- | --- |
| Remote attacker (internet, knows or guesses an ID) | Request sessions; try passwords; flood rendezvous | Get in without password/Accept; guess the 50-bit one-time password (per-IP throttling and blocking, rotation after 10 wrong attempts, [Authentication](#authentication)); downgrade to plaintext (refused on both sides) | Weak permanent passwords if policy allows them; social engineering of the Accept click; the viewer's ID and name in the prompt are self-reported |
| Malicious technician (legitimate session) | Everything the session permissions allow, as the logged-in user | Click or type into Tether's own windows (`own_ui_guard`); read or change Tether's config/install folders via file transfer; use Tether's own recorder without the host user's consent; change host settings remotely (this build has no remote path for that); open the camera view or the remote terminal (`enable-camera`, `enable-terminal` forced `N`) | Full control of the OS as that user by design: start programs (a command prompt included), files outside the guarded folders, admin rights if UAC is approved. Windows Mouse Keys and the On-Screen Keyboard still reach a Tether window ([below](#tethers-own-windows-are-for-the-local-user-only-056), W18). Recording with any other tool (screen recorder, modified viewer) is not prevented: the consent gate is enforced by the honest viewer |
| Malicious customer (host side) | Show fake content; record their own screen; keep what the technician sends (files, clipboard); put content into the technician's clipboard while clipboard sync is on | Inject keyboard or mouse input into the technician's PC (input messages flow from viewer to host only); read files the technician does not send | The host is the trusted endpoint for what it shows; treat files and clipboard from a host as untrusted |
| Compromised relay host **without** the server key (hbbr only, Caddy, TURN) | See IPs, timing, sizes, host IDs; drop or delay traffic; serve stale downloads | Decrypt or alter session content (session keys are sealed to the host's ephemeral key; [docs/encryption.md §8](docs/encryption.md#8-who-can-decrypt-who-can-attack)); forge policies, key lists or updates (admin/updater signatures) | Denial of service; metadata; legacy TCP tunnels (W2) and version-0 sessions with non-Tether peers (W1) are readable |
| Compromised hbbs **with** the server key | Everything above, plus sign any key for any ID | Decrypt past or running sessions (forward secrecy) | **Undetected man-in-the-middle of new sessions** by impersonating hosts: no host-key pinning and no viewer authentication ([docs/encryption.md](docs/encryption.md#active-attacks-with-the-server-key)). Mitigated only by keeping the key secret, hardening and monitoring the relays, and rotating the key on suspicion ([key rotation](docs/key-rotation.md)) |
| Network attacker (Wi-Fi, ISP, LAN) | See IPs, timing, TLS record sizes, SNI; block traffic | Read rendezvous or session data (TLS with verified certificates, no unverified fallback, then end-to-end encryption); answer as a LAN host without its signed key | Denial of service |
| TLS-inspecting corporate proxy | Read the rendezvous signalling (IDs, machine uuids, device public keys, connect requests) and the clear handshake at the start of each relay stream | Decrypt session content (except W1, W2) or swap the host key without the server key | With a stolen server key, the MITM above. Clients behind one proxy share an IP as hbbs sees it, which matters for the `RegisterPk` key replacement in [docs/encryption.md](docs/encryption.md#related-replacing-a-hosts-key-at-hbbs-without-the-server-key) |
| Local malware on the host | Read the screen and input like any program; read the config (device key, permanent password hash, which is password-equivalent); use Tether's IPC as the user | – (endpoint security is out of scope) | Full compromise of that host's sessions |
| Local malware on a Mac host with unattended access, running as an `admin`-group user | Replace `/Applications/Tether.app` (no password needed), which the root LaunchDaemon and the login-window host then run: root on that Mac | – (macOS lets the admin group change `/Applications`; an update signed by Tether must stay installable without a prompt) | Root on that Mac. Only where *Install as service* was used ([docs/unattended.md](docs/unattended.md#macos)) |
| Local malware on the viewer | Read the session, inject input into it, read remembered passwords | – | Full compromise of that viewer's sessions |
| Stolen technician laptop | Remembered peer passwords (obfuscated with the machine uid, so readable), trusted-device tokens, the team member key | Admin actions: the technician app has no admin key; if Tether Manage is on the same laptop, its vault still needs the passphrase | Unattended access to remembered hosts until their passwords change; remove the team key from the roster ([Team access](#team-access)) |
| Malicious update server (compromised relay serving `/download/`) | Withhold updates; serve an old copy of the signed release index (a device that already accepted a newer serial refuses it) | Make a client run code not signed by the updater key (minisign key `1AD0C3FBDCA3B4EA`, verified before install); change version, minimum version or rollout (the release index is signed by the admin key; the unsigned `latest.json` is not read by builds with `REQUIRE_SIGNED_INDEX`); offer an older signed build under a higher version (the signed trusted comment names file and version) | Denial of service: no updates while the device reaches no other source (after 30 days the app warns that the update information is out of date). Builds before the signed index: W9 |
| Holder of the admin and updater keys | Change policy within the floor, trust another server key, ship signed code | Turn off TLS verification or end-to-end encryption by policy (`FLOOR`) | Fully trusted: guard the vault and passphrase |
| Mac build runner sender | Run signed jobs on the build Mac | Run unsigned jobs, act as another sender, replay jobs ([docs/mac-build-runner.md](docs/mac-build-runner.md#security-model)) | A job is arbitrary code as the Mac user |

The relay operator is **not trusted with content**: hbbr and Caddy forward ciphertext. hbbs is
trusted to sign the right host key; with the server key, a compromised hbbs is the one relay
compromise that reaches content.

## Encryption everywhere

| Layer | Mechanism | Enforced in |
| --- | --- | --- |
| Transport to the relay | TLS WebSocket on 443 only (rustls default versions, TLS 1.2 or 1.3 with rustls; the native-tls retry uses the OS stack's range, and Caddy/nginx accept 1.2 or later; Let's Encrypt certificate, verified against the bundled Mozilla roots plus the OS root store, retried with the OS TLS stack, so a corporate inspection CA installed in the OS is accepted; never an unverified fallback). A fallback route uses TLS on port 80; it works only on relays where port 80 is forwarded to the TLS listener ([Server hardening](#server-hardening)). TLS is chosen only for domain names: an endpoint given as an IP address gets plain `ws://` (W4 below). With policy `network.direct = auto` and no proxy: direct paths beside it (LAN listener on the local subnet only; WebRTC/ICE for IPv6 and NAT hole punching, STUN only against our own relays, signalling only over the TLS WebSocket). Never with a proxy. | `apps/desktop/src-tauri/src/netpolicy.rs`, `docs/corporate-networks.md` |
| Client ↔ rendezvous server | The WebSocket to hbbs is authenticated by TLS alone (the upstream rendezvous key exchange is skipped on WebSocket, `common.rs::secure_tcp_impl`). The compiled-in Ed25519 server public key (`TETHER_PUB_KEY`, release builds refuse to compile without it) is what viewers verify **host keys** with: hbbs signs each host's public key with the server key. A server key list signed by the **admin key** can replace it (key rotation without reinstalling): while the configured server key is one of the listed keys, clients trust the active listed keys; lower serials are rejected, so a relay can neither widen nor roll back the set. A `key` option outside the list is trusted on its own; the UI cannot set it unless policy allows `settings.server` (default N), but a machine administrator can (W3 below). Upstream's custom-client routes are off in Tether builds: server and key are not taken from the exe file name or the environment variable `RUSTDESK_APPNAME`, and `custom.txt` beside the exe is not read (`platform/windows.rs::get_license_from_exe_name`, `common.rs::load_custom_client`). | `apps/desktop/src-tauri/build.rs`, `hbb_common::config::RS_PUB_KEY` / `server_keys_for`, `tether_trust::serverkeys`, [docs/key-rotation.md](docs/key-rotation.md) |
| Server key check | `hbbs -k _` / `hbbr -k _` reject clients that do not present the matching **public** key (`licence_key`), or one of `KEYS_ACCEPT` during a rotation. The public key is in every client, so this is a configuration check, not client authentication. A server asked for the check never runs without it: when `id_ed25519` exists but cannot be read (or cannot be created), hbbs and hbbr exit instead of accepting every client, and they do not write a new `id_ed25519.pub` beside it. A new private key file is created `0600`. | `server/systemd/*.service`, `server/docker-compose.yml`, `vendor/rustdesk-server/src/common.rs::key_accepted`, `require_key`, `gen_sk_in` |
| Peer ↔ peer (end-to-end) | Each device has an Ed25519 identity whose public key hbbs signs. The host signs a fresh X25519 key; the viewer seals a random 32-byte session key to it (`crypto_box`), and both derive one key per direction (BLAKE2b over the handshake transcript). Relay and LAN traffic is XSalsa20-Poly1305 (libsodium secretbox) with counter nonces. On WebRTC the stream is protected by DTLS only (the secretbox key is not applied there); the host signs its DTLS fingerprint into its identity (`IdPk.dtls_fingerprint`) and a mismatch or missing fingerprint fails closed. Forward secrecy: yes (ephemeral keys). Not covered: hbbs with the server key can impersonate a host ([docs/encryption.md](docs/encryption.md#active-attacks-with-the-server-key)); key exchange version 0 with non-Tether peers is broken (W1); legacy TCP tunnels run without the inner encryption (W2). | `client.rs::secure_connection`, `server.rs::identity_handshake`, `hbb_common::tcp::Encrypt`, `hbb_common::webrtc` |
| **No plaintext fallback (viewer)** | Upstream asks the user whether to "continue insecurely". Tether refuses the connection outright. **Exception:** a connection typed as an IP address or `host:port` skips the rendezvous, the handshake and this check; against a Tether host it fails, against a stock RustDesk host with direct IP access it would run unencrypted. | `vendor/rustdesk/src/client.rs::confirm_insecure_connection` (`#[cfg(feature = "tauri")]`), `client/io_loop.rs` (`is_direct_ip_access`) |
| **No plaintext fallback (host)** | The host aborts the handshake when the peer offers no key, sends no key exchange, the transport is not flagged secure, or this device's key pair is missing. The direct listener uses the same handshake. | `vendor/rustdesk/src/server.rs::identity_handshake`, `rendezvous_mediator.rs::direct_server` |
| Secrets at rest | Permanent password (stored as `SHA256(password ‖ salt)`, which is password-equivalent for login), 2FA secret and remembered peer passwords are wrapped with secretbox under a key made from the machine uid (`symmetric_crypt`). The machine uid is not secret (readable by local users, sent to hbbs at registration), so this is obfuscation, not protection against someone who can read the config. The device's Ed25519 secret key is stored unencrypted. Config files are `0600` on Linux/macOS; on Windows they inherit the profile folder's ACL. | `hbb_common::password_security::symmetric_crypt`, `hbb_common::config` |

## Authentication

| Control | Default | Where |
| --- | --- | --- |
| Approval | Incoming sessions need either a click on **Accept** on the host or a valid password (configurable to click-only or password-only). Accept is never pre-selected in either app. The request names the viewer's ID, says that the name is chosen by the other side and not verified, and says what is asked for (screen control, file transfer, camera, command line, network access). In the desktop app another request or a chat message never changes the request that is shown, and when the shown connection ends, a waiting request is never shown by itself; in TetherQS a new request appears above the running sessions' cards and Help closes. | Settings › Security |
| One-time password | **10 characters** from a 32-character alphabet, 50 bits (upstream default is 6). Rotated after 10 consecutive wrong attempts. Never sent: the viewer answers a per-connection challenge with `SHA256(SHA256(password ‖ salt) ‖ challenge)` inside the encrypted stream. | `security.rs` DEFAULTS, core `server/connection.rs` (`check_update_temporary_password`, `verify_h1`) |
| Permanent password | **≥ 12 characters, ≥ 3 character classes, ≤ 128.** Validated in Rust (authoritative) and in the UI. | `security.rs::check_password_strength`, `src/lib/security.ts` |
| Two-factor authentication | Optional TOTP (RFC 6238, 6 digits) required after the password. Enrolment needs a verified code before it takes effect. | Settings › Unattended access, core `auth_2fa.rs` |
| Other ways in | Bearer secrets, not identities. Resume ticket: a connection with the session's key and `companion_secret` skips password and Accept, **not 2FA**. It is valid while the session's connection runs (that connection is closed when the new one is authorised, so a session never runs on two) and for 120 s after it dropped; not after either side ended the session on purpose (Disconnect on the host, closing the viewer). The resumed connection keeps the permissions the host user last set. A file-transfer companion of a running session whose file permission is on skips password and Accept the same way. A trusted-device token skips 2FA; upstream's recent-session rule accepts the previous one-time password for 30 s. A server-key MITM learns `companion_secret`. | core `server/connection.rs` (`resume_of_authed_session`, `resume_register`, `resume_forget`, `RESUME_WINDOW`, `is_file_transfer_of_authed_session`, `is_recent_session`, `send_logon_response_and_keep_alive`), [docs/encryption.md §6](docs/encryption.md#6-password-authentication) |
| Brute-force protection | Failed logins are counted per IP and per IPv6 /64, /56 and /48 prefix: more than 6 in a minute are throttled, more than 30 (IPv6 prefixes: 60/80/100) blocked. Counters live in memory and reset on host restart. For relayed sessions the IP is the one hbbs reports. The one-time password rotates after 10 consecutive failures. | core `server/connection.rs` (`LOGIN_FAILURES`, `check_failure`) |
| Identity verification | Viewers see whether the session is end-to-end encrypted and which path it uses, and can hover the shield for the host key's fingerprint. No Tether app shows a device its own fingerprint, so an out-of-band comparison is not supported yet, and the viewer does not remember host keys between sessions. The host cannot verify the viewer: its ID and name are self-reported. | `Session.tsx`, `client/io_loop.rs` |

## Central management (0.3)

| Control | Mechanism | Where |
| --- | --- | --- |
| Policy integrity | Policies are signed by the **admin key** (Ed25519), which never leaves the admin's PC. Clients verify against compile-time public keys, so a compromised relay cannot push or alter a policy. Lower serials are rejected (no rollback); both apps keep the highest accepted serial beside the cache (`policy-serial`). Layers resolve as defaults < profile < custom app < group < machine, with one exception: a custom app's setting is ignored for every feature the policy profile locks (`off`, `locked` or `hidden`), so a custom app can add restrictions and defaults but never unlock what the policy locks. The relay and Tether Manage validate a bundle strictly and refuse to publish a bad rule. In an app one rule never fails the policy: a rule with an unknown feature or a bad value is skipped and named in the log (`policy rules not applied`), a rule with a state or field from a newer Tether Manage is skipped silently, and the rest applies; TetherQS opens a custom app's attached profile the same way. Still refused as a whole: broken JSON, unknown top-level or group fields, an assignment to a group that does not exist. A policy that locks a permission off also takes it from running sessions and from sessions that resume, in both apps (desktop: in the connection window's process, at its next policy check), and a running file-transfer connection is closed when file transfer is locked off. | `crates/tether-trust` (`policy::resolve_with`, `Bundle::validate`, `Bundle::from_json`, `custom::open_app`), `apps/*/src-tauri/src/policy.rs`, desktop `sink::enforce_permission_locks` / `revoke_locked`, TetherQS `commands::revoke_denied`, `sink::take_denied` |
| Policy in the background service | The Windows service (`--server`) refuses **new** incoming sessions while policy `connect.incoming` is off or policy `update.minimum-version` is above its version, also at the logon screen: the app writes the refusal text into the core option `tether-refuse-incoming`, and the host sends it as the login error before any password exchange. Running sessions are not ended. The release index's mandatory/minimum state is enforced by the app and the connection window, not at the logon screen. Group policy reaches the service only after the signed-in user has opened Tether (the service adopts that app's device id from option `tether-install-uuid` at its next policy check); before that only profile and machine policy apply. | desktop `policy::apply_incoming`, `incoming_refusal`, `publish_uuid`, `pick_uuid`; core `connection.rs::on_open`; [docs/unattended.md](docs/unattended.md#policy-in-the-service) |
| Server key list | The admin key also signs which server keys devices trust (`GET /api/server-keys`). Whoever holds the admin key can therefore make devices trust another server key; it still needs a path into the TLS relay connection to use it. New server private keys are sealed in the vault like the other keys. Devices keep the highest accepted serial beside the cache (`server-keys.serial`), so deleting the cache does not reopen a rollback. The private key file exported for the relays is created readable by the current user only, and Manage keeps asking to delete it. Manage refuses to publish a key list that lacks the key a relay reports it signs with (when no relay reports its key, it publishes and says the list could not be checked), starts a new rotation from the relays' signing key, refuses to save key files into a folder that already holds another `id_ed25519` pair, and deletes only a pair whose public key is a server key of the vault. | `tether_trust::serverkeys`, `apps/manage/src-tauri/src/keys.rs`, [docs/key-rotation.md](docs/key-rotation.md) |
| Security floor | TLS verification, end-to-end encryption, 443-only networking and "no silent recording" are not policy features. The apps apply them on top of any policy. | `tether_trust::policy::FLOOR`, `security.rs`, `netpolicy.rs` |
| Serials | Tether Manage takes the next policy and key list serial only from documents that verify with its admin key and from its own published copies (never from what a relay reports), with an overflow check. | `apps/manage/src-tauri/src/main.rs` `next_serial` |
| Response size | Policy and key list answers are read with a hard limit (stop after limit + 1 bytes), so a relay cannot make a client buffer an endless answer. | `tether_trust::read_limited` |
| Admin API | Every admin request is signed over the relay's host name, method, path with query, timestamp, nonce and body hash (`Tether-Sig: v2 host=… ts=… nonce=… sig=…`). A relay accepts v2 only for the names in its `RELAY_HOST` (from `/etc/tether/server.env`; without it v2 gets `401`), so a captured request cannot be replayed at another relay. The signed host is a plain host name or IPv6 literal (`reqauth::host_of`); Tether Manage takes a relay address only as `https://host` with an optional port (no path, user name, query or fragment; `server_ok`), so the host that is signed is the host the request goes to. No app falls back to v1 by itself. The server allows ±300 s clock skew and rejects replayed nonces (kept 601 s, persisted across restarts; a failed write of `nonces.json` is logged and does not fail the request). Tether Manage and the technician app sign v2 and follow no HTTP redirects. v1 signatures (no host name, older apps) are accepted until `TETHER_SIG_V1=off` is set (W14). | `tether_trust::reqauth`, `server/api` (`sig_relay`), [docs/api.md](docs/api.md#admin-endpoints-signed-requests) |
| Keys at rest | Admin and updater private keys are encrypted with argon2id + XChaCha20-Poly1305 under the admin's passphrase. The vault has a format version: Tether Manage never saves a vault newer than it understands and keeps unknown fields, so it cannot drop keys it does not know. | `apps/manage/src-tauri/src/keys.rs` |
| Update integrity | Installers and TetherQS are signed with the updater key (minisign, key id `1AD0C3FBDCA3B4EA`) and verified against the compiled-in `TETHER_UPDATER_PUB_KEY` before installation. Clients below the minimum version cannot start or accept sessions. Release metadata (version, file, size, sha256, mandatory, minimum version, rollout, halt) comes from the release index signed by the **admin key**; a serial floor per channel refuses older indexes, and the unsigned `latest.json` is not read (`REQUIRE_SIGNED_INDEX`). Both apps also check the signed trusted comment (file name and version) of every artifact, so an older signed build cannot be offered under a higher version (W9). A device below a release's or the policy's minimum version is offered the release whatever its place in a staged rollout or `update.rollout = late`, never a halted release (both apps). TetherQS never swaps its exe while a session runs and counts a download as staged only after it verified. | `update.rs` (both apps), `tether_trust::release`, [docs/updates.md](docs/updates.md) |
| Recording consent | Tether's recorder requires an explicit Allow from the other party for every connection, regardless of policy. The viewer never assumes the permission. This binds honest viewers only: the host just withholds the permission, so a screen recorder or a modified viewer is not stopped. A viewer that keeps asking is shown at most once per 10 s per connection, and Allow is never pre-selected. | core `client.rs`, `server/connection.rs`, `docs/recording.md` |
| Diagnostics | Exports redact passwords, salts, key pairs and tokens. A system proxy's user name and password are removed before the proxy URL is logged (`without_credentials`). Uploads are accepted only for tickets an admin opened (8 characters, 24 h, one upload, kept 7 days). | `diag.rs`, `server/api` |

### Session audit log

Every host reports its incoming connections to the first relay of its build
(`POST /api/audit/conn|file|alarm`, [docs/api.md](docs/api.md)); Tether Manage › Sessions shows them.

- **What is logged**: time (relay clock), host Tether ID and base64 machine uuid, connection
  number, the technician's Tether ID, display name and IP address as the host saw them, the
  address the host posted from, connection kind and how it was approved (click, password), file
  transfers with folder or file path, count and up to 10 file names with sizes, disconnects, and
  alarms such as IP whitelist blocks. No session content, no file content, no passwords.
- **Not authenticated.** Hosts have no device key, so a post is unauthenticated device telemetry
  like the heartbeat: anyone who can reach a relay can add records under any ID, or fill the log
  within the limits (8 KiB per record, 120/min per IP with an IPv6 client counted by its /64,
  60/min per device, size cap). Records are
  never used for anything else (no device, policy or access decision); read them as a log of what
  hosts reported, not as proof. A host that cannot reach the first relay within 120 s drops the
  record (its log says "Audit post dropped").
- **Privacy and retention**: names, IP addresses and file paths are personal data. Files are
  `0600` under `/var/lib/tether-api/audit/`, readable only with an admin-signed request, kept 90
  days or 50 MB (`AUDIT_RETENTION_DAYS`, `AUDIT_MAX_MB`; `AUDIT_RETENTION_DAYS=0` stores nothing).
  CSV exports leave the relay: handle them like the records. One query answers at most about
  8 MiB of records and sets `X-Audit-Truncated: 1` when it cut; the Sessions page of Tether
  Manage then names the relay.

### Team access

Technicians get an org-scoped identity without the admin key: each technician app generates its own
Ed25519 key, the admin adds its public key to the team roster in Tether Manage and publishes the
roster to the relays (`PUT /api/admin/team`, admin-signed). Member requests (`/api/team/*`) are
signed with the member key in the admin request format (relay host name, method, path with query,
timestamp, nonce, body hash; ±300 s; replayed nonces rejected, same persisted cache as admin
requests; v1 without the host name as for admin requests, W14). The member key file is generated
only when it is missing (an unreadable or damaged file is reported, never replaced). It is written
to a temp file, `0600` on Unix, and linked into place without replacing, so an existing key is
never replaced (`apps/desktop/src-tauri/src/team.rs::key_at`).

- **What a member can read**: the device list without IP addresses or policy serials (uuid, ID,
  profile, version, OS, last seen), the shared tag document, and only with the `audit` flag the
  session audit records of one device at a time (names, IPs and paths are personal data, so the
  flag is off by default). A member cannot change policies, releases, the roster or anything else
  admin-signed; a member key is never accepted as an admin key.
- **What a member can write**: with `write`, the shared tag document (one JSON object, ≤ 512 KiB,
  replaced as a whole with optimistic concurrency on `rev`). The relay does not interpret it, and
  it must not be used for any security decision. Tags are **not secrets**: every member reads them;
  never put passwords or keys in tags.
- **Key theft**: a stolen member key gives its holder that member's rights until the admin removes
  it from the roster and publishes; that takes effect at once on each relay that gets the roster.
  Only the admin key changes the roster. Compare the fingerprint with the technician out of band
  before adding a key.
- **The roster is per relay**: each relay authorises against the roster it was given. A relay that
  missed a publish still accepts a removed key, so publish to every relay (Tether Manage names relays
  with a different roster).
- **Limits**: 120 requests/min per member key, 60 failed signatures/min per IP (a failure tries every
  roster key, at most 500, under the state lock), body limits 64 KiB (roster) and 512 KiB (tags);
  `team/roster.json` and `team/tags.json` are `0600`.

### Device status (health reports)

A monitoring agent on a machine can report its health to the relay ([docs/health.md](docs/health.md),
[docs/api.md](docs/api.md#device-status-health-reports)). It adds two kinds of caller to the relay:
devices that sign with their own key, and members that read what the devices posted. The feature
runs nothing on a device and opens no port on it.

- **Device keys are pinned.** A device generates an Ed25519 key and enrols with an admin-signed
  token, the request signed with that key: enrolling needs a valid token **and** the private key of
  the key being pinned (without the signature: `401 enrolment needs the device's signature`). The
  relay pins the key to the device's uuid in `status/keys.jsonl`. Every enrolment, report and removal is signed (`Tether-Sig: device-v1`, message prefix
  `tether-device-v1`) over the relay's host name, the method, the path, the time, a nonce and the
  body hash, so a captured request is worth nothing at another relay and cannot be replayed
  (±300 s, nonces kept 601 s in memory, `seq` must increase on every post). A device key never
  passes as an admin or member key, and an admin or member signature never passes on a device
  route (separate prefix and header version, tested both ways). A second key for a pinned uuid or
  Tether ID is refused (`409`). Only an admin-signed `DELETE /api/admin/status/<uuid>`, a
  member-signed `DELETE /api/team/status/<uuid>` from a member with `write`, or the device's own
  signed `DELETE /api/status` releases a pin; the journal names who did it.
- **Tether IDs are verified, not claimed.** A device's Tether ID, the only thing that puts its
  health on a host row in the technician app, is set in two ways only: the admin issued the token
  for that Tether ID (`tether_id` in the signed token; it enrols one device per relay, a second uuid gets
  `409 token already used` there, and a request naming another ID `409 token is issued for another
  id`; only its first use links, so it cannot undo a member's Unlink), or a roster member with `write` confirmed the device's claim
  (`POST /api/team/status/<uuid>/link`, after one explicit confirmation in the app; the pin records
  the member's name and the time). A Tether ID the device names with any other token is kept as
  `claimed_id`, shown as a claim on a row of its own, and never joins a host row by itself. A
  report can neither set nor change a Tether ID. A member's Unlink also dismisses the
  claim (`no_claim`) until a member allows a new one; the same key under another uuid, with the
  dismissed pin still there or after the device dropped it itself, comes back with `no_claim`
  and claims nothing (the relay remembers dismissed keys). The boundary: a
  holder of a valid enrolment token not issued for an ID can always enrol a *new* device (new
  key) that claims any Tether ID. A claim joins nothing and is fixed for that device until a
  technician acts; to stop claim rows from an unwanted source, revoke the token and remove its
  devices (`monitor-publish devices --token <id>`, `remove`). The app refuses to link a claim to
  a host that already has a linked health agent, so a confirmed impostor cannot hide that host's
  health behind a link conflict. Pin records from before verified IDs are
  read with their Tether ID as a claim. **Single use is per relay:** each relay records a
  token's use itself, so a copy of a token issued for one ID can link a second device on a relay
  the first device has not enrolled on; the technician app then shows a link conflict and joins
  neither device to the host. Give such tokens only to that machine and keep them short-lived.
  **What this does not prove:** both ways are the word of
  a trusted person (the admin who issued the token, the technician who confirmed), not a
  cryptographic proof by Tether on that machine. A technician who confirms the claim of the wrong
  agent shows that agent's reports on the wrong host until someone unlinks it.
- **What a token holder can do.** Enrol devices until the token expires or is revoked: any uuid
  that is not pinned yet, up to 30 new pins per minute per IP and `STATUS_MAX_DEVICES` in total;
  with a token issued for a Tether ID, one device with that ID. A token is valid 1 to 30 days and
  can be **revoked** before that (`POST /api/admin/status/revoke`, `monitor-publish revoke`):
  the relay then answers `401 token revoked`. Revoking does not remove the devices that enrolled
  with it already; `monitor-publish devices --token <id>` lists them and `remove` removes them.
  Issue tokens for as short as the rollout takes and treat them like a password. A holder can
  fill the device cap, which makes `507` for devices that enrol later, and can make devices that
  claim any Tether ID; a claim shows only as a claim until a member confirms it. With a token that
  names a customer, every device it enrols is under that token's customer.
- **The customer reference is a grouping, not a boundary.** An admin-signed token may name a
  customer (`customer`, 1 to 32 characters `[A-Za-z0-9._-]`); the relay sets it on the pin of the
  device that enrols with it. A device cannot name or change it: neither the enrolment request nor
  a report has the field, and a request with it is refused (`400`). A token holder can enrol new
  devices, with new keys of their own, under the token's customer. Every enrolment is signed with
  the private key of the key it pins (`Tether-Sig: device-v1`, as for a status post), so an
  existing device's pin, link and customer cannot be taken or changed without that device's key:
  the same device enrolling again with a token that names another customer (or any customer, when
  it had none) gets it (journal: `status: customer of <uuid> set to <customer> by token <id>`; the
  pin keeps the token it first enrolled with), and knowing a device's uuid and public key is not
  enough to create a pin for it either, also where a relay has none (removed, dropped after 180
  days without a report, or never enrolled there). Every roster member sees every device whatever its customer: the customer
  groups devices inside one deployment, and organisations that must not see each other's machines
  need separate deployments ([docs/several-organisations.md](docs/several-organisations.md)).
- **What a token holder cannot do.** Read any report, summary or history (those routes need a
  roster member's key). Replace or change an existing pin (its customer included, above), post as another device, remove a pin,
  link a Tether ID, set or change the Tether ID of a pinned device (a report cannot), sign an
  admin or member request, or reach anything outside the status routes. A device key holder can post, and remove
  the pin of, that device only. A device's reports can therefore be forged by whoever controls
  that machine (as for any agent); they are shown as what the device said, never as a fact the
  relay checked.
- **Report content is untrusted.** The relay never parses the report's `body`: it checks that it
  is a JSON object in valid UTF-8, stores it byte for byte (64 KiB at most) and returns it as it
  is. The technician app shows every text from a report as plain text, shortened to fit. Nothing a
  report says changes a device, a policy or an access decision.
- **Who can read reports.** The relay operator, who can read `status/`, and every member of the
  team roster (no `write` or `audit` flag needed; linking, unlinking and removing a device need
  `write`, refused with `403` otherwise). Reports are readable on the relay, not
  end-to-end encrypted: machine name, installed product names and versions, check titles, values
  and hints ([PRIVACY.md](PRIVACY.md)). An unauthenticated caller can tell whether a uuid is
  enrolled (`401 unknown device` against `401 signature does not match`); uuids are not secrets.
- **Limits** (all before the signature check and the parser, so the cheap path stays cheap):
  64 KiB per post, one accepted post per uuid per 60 s, 10 correctly signed but refused posts per
  uuid per minute, 60 failed signatures or unknown uuids per minute per IP (IPv6: per /64), 30 new
  enrolments, refused tokens or unsigned enrolments per minute per IP. A forged post that names a device's uuid is not
  counted against that device. The relay paces devices with `next` (`STATUS_TARGET_RPS`) and
  `STATUS=off` turns every status route into `503` at once, before any body is read.
- **Fail closed.** When `status/keys.jsonl` or `status/revoked.json` cannot be read (anything but
  a last line of `keys.jsonl` cut short by a crash), the status routes answer
  `503 status store unavailable` and nothing under `status/` is written: an empty pin list would
  hand every uuid to the next token holder, an empty revocation list would make revoked tokens
  valid again. Sessions, policy and
  updates are not affected. Recovery: restore or repair the file and restart tether-api
  ([docs/health.md](docs/health.md#the-locked-state)).
- **The monitor release index** is signed like the Tether one and refused in the other direction:
  `monitor.json` takes only the apps `monitor` and `monitor-packs`, `releases.json` only Tether's,
  so the agent's files can never be offered to an installed Tether client. The agent verifies the
  index, the size, the SHA-256 and the minisign signature of each artifact before installing.

### The software list

With `STATUS_SBOM=on` a device can also upload a list of its installed software
([docs/health.md](docs/health.md#the-software-list), [docs/api.md](docs/api.md#put-apistatussbom-device-signed)).
It is off by default; while off, its three routes answer `404`. `STATUS=off` wins over it.

- **Who can read it.** The relay operator, who can read `status/`, and every member of the team
  roster (no `write` or `audit` flag needed), as for reports. It is not end-to-end encrypted. It
  names the installed programs with their versions and publishers and the machine's hardware
  identity, which is more than a report holds ([PRIVACY.md](PRIVACY.md)). A device key and a
  member key do not pass on each other's routes, as for reports.
- **What an enrolled device can do.** Store one document of its own choosing: a JSON object of at
  most `STATUS_SBOM_MAX_KB` (512 KiB), one accepted upload per hour, in place of its earlier one.
  The relay checks only that it is a JSON object and stores it byte for byte, so junk, a document
  that is not CycloneDX, and one nested very deeply are all stored. Whatever reads a list (the
  technician app, scripts, the bridge) must treat it as untrusted text and survive that. Nothing
  in a list is run or changes a policy or an access decision. A device can be made to look like it
  has software it has not, as with a report.
- **What a hostile enrolled device can not do.** Store more than that one document, or touch
  another device's list: the key is pinned per uuid, and a signature for another uuid, an unsigned
  request or a replayed one is refused with `401`.
- **Limits, in the order of work.** `STATUS=off`, the switch, `Content-Length` required (`411`) and
  at most `STATUS_SBOM_MAX_KB` (`413`), a locked store, all before the body is read; the header,
  the per-IP limit of 60 failed requests a minute and the key lookup (a uuid that is not enrolled
  here is `401` and counts as a failure for the IP); one accepted upload per uuid per hour; a
  global budget `STATUS_SBOM_RPS` (default 2 per second, all devices); only then the body, read for
  exactly the declared bytes; then the signature and the JSON-object check. The hash, the
  signature check and the write happen outside the state lock. No file is written for a refused
  request.
- **Who can use up the global budget.** A request takes a slot only after it passed the header,
  the IP's limit, the key lookup and the device's hour, and a head without a body no longer holds
  one: a body that does not arrive or a signature that is refused gives the slot back and counts
  as a failure for the sender's IP. A device with a valid key can use its one upload per hour,
  and one that is refused after a valid signature (`400 not a JSON object`, `507 sbom store full`)
  uses that hour too, so one device key cannot keep the budget used up with refused uploads.
  Someone who only knows the uuid of an enrolled device (uuids are not secrets) can take a slot
  for as long as their request takes to be refused, 60 times a minute from one IP, after which that
  IP gets `429 too many failed requests` before it reaches the budget. What remains: many source
  addresses, each sending 60 heads a minute for enrolled uuids, together could delay real
  uploads with `429 busy` (the device retries with backoff); and a holder of an enrolment token
  can enrol many devices and use the budget with real uploads of their own (revoke the token and
  remove its devices). Nothing else on the relay is affected.
- **The per-IP counter is shared.** The 60 failed requests a minute per address (IPv6: per /64)
  are the same counter that every device-signed status route uses, so an address that trips it
  (failed signatures, unknown uuids, bodies that do not arrive) is refused for every status post,
  removal and upload for the rest of the minute. A site behind one address needs sixty failed
  requests in a minute to be affected; a source that can forge such requests can silence the
  health reports of a site behind a shared address for as long as it keeps sending them.
- **Disk.** `STATUS_SBOM_MAX_TOTAL_MB` (2 048 MiB) limits the sum of all stored lists. A holder of
  an enrolment token can enrol devices (up to `STATUS_MAX_DEVICES`) and fill that sum with junk
  lists of 512 KiB; uploads that would exceed it then get `507`, while devices that already have a
  list can still replace it with one that is not larger. Revoking the token and removing its
  devices (`monitor-publish devices --token <id>`, `remove`) gives the room back.
- **Lifetime.** The file is removed with the pin: the device's own removal, an administrator's or
  a member's, and the 180 days without a report. Unlinking and dismissing a claim keep it.

## Least privilege and attack surface

Settings below are **forced** for every Tether process (UI, server, service and connection manager)
in both apps. Neither the user nor a remote peer can change them (`security.rs` and `netpolicy.rs`
in `apps/desktop/src-tauri/src` and `apps/quick/src-tauri/src`):

- `allow-remote-config-modification = N`: set as a precaution. No code in this build reads the
  option, and the build has no remote path that changes a host's settings.
- `enable-camera = N`, `enable-terminal = N`: no camera view and no remote terminal on any Tether
  host.
- `enable-lan-discovery = N`: the device is not discoverable on the local network.
- `enable-check-update = N`, `allow-auto-update = N`: no requests to third-party update servers.
- `allow-insecure-tls-fallback = N`: HTTPS never downgrades.
- `direct-server`: the direct listener (TCP 21118, LAN lookup answer on UDP 21119) runs only with
  policy `network.direct = auto` **and** where it raises no firewall prompt (Windows inbound rule
  `Tether` from the installer, limited to the local subnet; `Tether QS` created by an admin; macOS
  application firewall off). It accepts only the same end-to-end key exchange as the relay (the
  host proves the key the rendezvous server signed for its ID; an unencrypted handshake is
  refused) and the same password / Accept / permission checks. The LAN lookup answer carries the
  ID and port only (no host or user names).
- `enable-remote-printer = N`: no virtual printer driver.
- `allow-auto-record-incoming/outgoing = N`: nothing is written to disk without an explicit action.
  The outgoing one is forced in the local settings map, where the core reads it
  (`security.rs::ENFORCED_LOCAL`).

Permissions a rendezvous server sends (`control_permissions`) take precedence over the local
`enable-*` options in the core (`connection.rs::permission`). Tether's hbbs sends none.

## Tether's own windows are for the local user only (0.5.6)

A technician controls the host's mouse and keyboard, so without a guard they could click in the
host's own Tether windows: grant themselves permissions on the session card, approve a consent
prompt (recording, admin access), or change settings. The host therefore filters every remote
event right before it is injected (`vendor/rustdesk/src/server/own_ui_guard.rs`, called from
`input_service.rs`, so it also covers the elevated portable service of TetherQS). Always on, no
setting, for every host (installed Tether with the service, TetherQS elevated or not). Local,
physical input never passes through it.

- **Mouse:** a button press or wheel over a window of a Tether process is dropped, together with
  the matching release (so no drag starts there). Windows: `WindowFromPoint` → `GetAncestor
  (GA_ROOTOWNER)` → owning process (WebView2 popups: the `msedgewebview2.exe` started by a Tether
  process). macOS: the frontmost on-screen window under the pointer (`CGWindowListCopyWindowInfo`),
  skipping the Dock's window: it covers the whole screen above all normal windows. So a click on
  the Dock itself where a Tether window lies underneath is dropped too.
  Pointer moves pass, so the pointer can still travel across.
- **Keyboard:** while a Tether window is in the foreground, keys are dropped except what is needed
  to leave it: modifier keys on their own, Alt+Tab, Win+Tab / Cmd+Tab, Ctrl+Alt+Del, lock screen.
  Enter, Space, Esc, letters, typed text (`seq`, paste as keystrokes), Alt+F4 and Cmd+Q never
  reach a Tether window. The foreground is checked per event. Windows: a lone Alt press (Alt
  pressed and released with nothing passed in between, e.g. Alt+F4 with F4 dropped) would put the
  window into keyboard menu mode and stall its page, so right before that Alt release the host
  injects the unassigned key VK 0xE8 (AutoHotkey's "menu mask"); the window sees a key between
  Alt down and up and stays out of menu mode. Alt+Tab, a lone Alt on other windows and
  Ctrl+Alt+Del behave as before.
- **Tether process** = the same executable as the host process, or any executable whose name
  starts with `tether` (TetherQS next to an installed Tether, Tether Manage). Looked up per PID,
  cached for 2 seconds.
- **Touch zoom** (Ctrl+wheel) over a Tether window is dropped.
- **File transfer** refuses to list, read, write, rename or delete Tether's configuration (this
  process's real config folder, every user's `AppData\Roaming\Tether*`, the copy under
  `ServiceProfiles\LocalService`; macOS `Library/Preferences/app.tether.*`) and its installation
  (the install folder, the `.app` bundle, or a portable TetherQS exe itself). A folder **above**
  the configuration (AppData, a user profile, the drive) can be browsed, but not downloaded as a
  whole, renamed or removed; a folder above the installation cannot be renamed or removed. Paths
  are resolved (`..`, links, short names) before the check (`is_protected_path`,
  `holds_protected`). This computer's own administrative shares (`\\localhost\C$\…`, by its name
  or any of its addresses) are checked as the drive they are; any other path that reaches this
  computer through one of its own shares or a device path cannot be checked and is refused for
  every file action (`by_drive`).
- The technician sees "The Tether window on the remote computer can only be used by the person at
  that computer." (at most every 5 s; older viewers ignore it).

What this does **not** stop (the technician still controls the computer as its user):

- Starting programs (Start menu, Win+R) and using them: a command prompt can run
  `tether.exe --password …` / `--config` / `--option` only as an administrator of an installed
  Tether (`core_main` checks it); a technician who already has admin rights (UAC approved, or the
  user is an admin) can change Tether like any other software. Programs running as the user can
  also talk to Tether's local IPC the way its own windows do.
- A junction inside an allowed folder that points into Tether's configuration is followed by a
  recursive download of that folder.
- File transfer outside the protected folders is full access by design. (The remote terminal is
  off on every Tether host: `enable-terminal = N` is forced.)
- Windows Mouse Keys: with it on, a key press becomes a click at the pointer, also while another
  window is in front, so it can click a Tether window the pointer rests on (W18).
- The Windows On-Screen Keyboard: clicks on its window type into a Tether window that has the
  focus (W18).
- A drag started outside a Tether window may be released over one (the release is not dropped, so
  no button gets stuck); Tether's windows do not act on drops.
- Windows that are not Tether's but change Tether (e.g. the Windows Services console, uninstall
  in Settings) are ordinary programs.

Tests: unit tests in `own_ui_guard.rs` (`cargo test --lib --no-default-features --features
tauri,hwcodec,use_dasp own_ui_guard` in `vendor/rustdesk`), VM end-to-end
`apps/desktop/e2e/vm-noremote-e2e.mjs` (remote clicks on every TetherQS permission switch,
Disconnect, Close and the recording consent's Allow; keys while TetherQS is in front; Notepad still
works; Alt+Tab leaves; a lone Alt and Alt+F4 on TetherQS do not stall it).

## The app shell (Tauri)

- Strict CSP: `default-src 'self'`, no remote scripts, `connect-src` limited to Tauri IPC
  (`tauri.conf.json`).
- No shell, filesystem or HTTP plugins. The updater, dialog, deep-link and single-instance plugins
  are registered (`app.rs`) and used from Rust only: the capability file
  (`capabilities/default.json`) grants the webview no plugin command, so it can only call Tether's
  own commands.
- Commands that change the machine's security posture are accepted from the main window only
  (`commands::main_only`): permanent password, 2FA (begin, confirm, disable), `set_option`,
  installing the service, saving the hosts store and the team tags. A session or connection
  window cannot call them.
- What the IPC boundary validates (`commands.rs`):
  - peer IDs match `[A-Za-z0-9_.@:-]{1,64}`;
  - session keys are 32 hex characters;
  - options must be on an allow-list (`security::UI_OPTIONS`), so a compromised webview cannot
    change other core options or 2FA through `set_option`; option values are at most 512 bytes;
  - passwords are at most 128 bytes, chat messages and typed text at most 4096;
  - connection types: `session_start` accepts the same types (`default`, `file-transfer`) and
    runs the same checks as `open_session`, the outgoing-policy check included;
  - `cm_permission` accepts known permission names only and cannot switch on what policy keeps
    off (switching off is always allowed).
- DevTools are not compiled into release builds.
- Keyboard hook (Windows technician app, docs/keyboard.md): a low-level keyboard hook exists only
  while a session window has the focus with "Send system keys" on and no text field active; it is
  removed on focus loss, minimize, close and Ctrl+Alt+Home. Keys are only forwarded to that
  session (as typing in the window would), never stored or logged. Ctrl+Alt+Del cannot be
  captured. `TETHER_DEV_KEYLOG` (host logs received keys instead of typing them) and the other
  `TETHER_DEV_*` test switches exist in debug builds only.
- Session video arrives on a per-window binary channel. One window cannot read another session's
  frames.

## Server hardening

- `hbbs` and `hbbr` run as an unprivileged `tether` user with systemd sandboxing
  (`ProtectSystem=strict`, `ProtectHome`, `NoNewPrivileges`, `PrivateTmp`, a single writable path).
- Tether clients need only TCP 443 (TLS on port 80 as a fallback route): WebSocket over TLS through
  Caddy (`docs/corporate-networks.md`), which forwards `/ws/id` and `/ws/relay` to hbbs/hbbr's
  WebSocket ports 21118/21119 on 127.0.0.1. tether-api listens on 127.0.0.1 only, and so do the
  WebSocket ports 21118/21119 of hbbs and hbbr (`WS_BIND`, default `127.0.0.1`; the Helm chart
  sets `0.0.0.0` for its separate proxy pod). hbbs and hbbr bind 21115-21117 and their health
  endpoints (hbbr 21121, hbbs 21122: `/healthz`, `/readyz`, on hbbs `/pubkey`) on all interfaces.
  The host firewall rules from `install.sh` open 443/tcp, 80/tcp, 21115-21117/tcp and 21116/udp
  (stock RustDesk clients; UDP 21116 is also STUN/TURN for WebRTC), not 21118/21119 and not the
  health ports.
  Port 80: `install.sh` only opens it, on every firewall type; the front end answers it with a
  redirect to https (nginx: `return 301`). `install.sh` adds no forward of 80 to 443. TLS on port
  80, the clients' fallback route, therefore works only where such a forward was added by hand,
  as [docs/deployment.md](docs/deployment.md) records for the production relays.
- Client IPs behind Caddy: hbbs takes the last `X-Forwarded-For` hop, which Caddy sets to the
  peer it saw; Caddy overwrites `X-Real-IP` (read by hbbr) the same way. A client that goes through
  Caddy/nginx cannot choose the IP the brute-force and IP-change checks see. Since 21118/21119
  listen on loopback, a client can no longer reach them directly and send its own IP headers (W5);
  where `WS_BIND` is set to another address, a firewall or network policy must keep clients off
  these ports.
- The front end can be nginx instead of Caddy (`PROXY=nginx`, [docs/proxy-benchmark.md](docs/proxy-benchmark.md)):
  same routes and header rules (`X-Forwarded-For` and `X-Real-IP` overwritten with the peer),
  TLS 1.2/1.3 AEAD only, session tickets off, `/qcache/` reachable only through
  `X-Accel-Redirect`, no access log. nginx and its ACME module come from nginx.org's signed
  repository and are patched by the OS's automatic updates; under SELinux nginx runs confined
  (`httpd_t`, read-only on tether-api's data). Wrong IPs on `/_metrics/*` get 403 instead of a
  closed connection. Basic-auth hashes are bcrypt cost 5 (the passwords are 24 random bytes;
  nginx checks the hash in the worker that also carries the WebSockets).
- A host's signalling path (the connection hbbs sends its punch-hole, relay and ICE messages
  over) is only a connection whose registration hbbs accepted: `RegisterPeer` from the host's IP
  with its key known, or `RegisterPk` passing upstream's UUID/key checks. Naming an ID is not
  enough, and only the current path's heartbeats keep it alive.
- `RegisterPk` (`rendezvous_server.rs::register_pk`): over UDP, where the sender chooses the
  source address, a registered key is never replaced; over TCP/WebSocket the upstream rule stands
  (same uuid from the same IP may change the key, W8). A UDP `RegisterPk` or `RegisterPeer`
  never moves the signalling path or stored address of a host whose TCP/WebSocket connection is
  open (all Tether hosts); for a host registered over UDP it still can (W17). The per-IP limiter table is
  pruned every 3 s and holds at most 50 000 source IPs; when it is full a new IP is refused, and
  a host re-sending its registered key still passes.
- TURN on hbbs's UDP port takes only datagrams from the exact address an allocation sent to in
  the last 5 minutes, never a rendezvous message. Requests that are not authenticated (no valid
  MESSAGE-INTEGRITY, or an expired credential from an address other than its own session's) share
  a budget of 50/s. Send indications and ChannelData are accepted only from client addresses that
  authenticated (the 10 000 most recent ones: at the limit the address heard of longest ago goes,
  so a new authenticated client always gets in). At most `TURN_MAX_ALLOCATIONS_PER_IP` (default
  16) allocations per client IP, besides the global `TURN_MAX_ALLOCATIONS`. The nonce table is bounded at 200 000 (patched `turn` 0.10.0 crate in
  `server/patched/turn`; upstream never removed unused nonces)
  ([docs/stun-turn.md](docs/stun-turn.md)).
- tether-api serves up to 32 requests at once with 30 s read/write timeouts and a whole-body
  deadline, reading bodies outside its state lock, so a slow or stalled client holds only its own
  request. A body that averages under 16 KiB/s after 30 s is cut off (`408`). The large uploads it
  streams to disk have separate slots, 4 for releases and 4 for diagnostics (more: `503` +
  `Retry-After`), so heartbeats always keep 24 of the 32 workers, and are refused before their
  body is read when they cannot succeed: no admin keys, no `Tether-Sig` header or its timestamp
  outside ±300 s, an invalid path, no open diagnostics ticket. A release slot is granted on the
  header alone; the signature covers the body and is verified only after it arrived, against the
  time the upload started, so made-up uploads can occupy the 4 release slots, not the diagnostics
  ones (W16). A request that is not POST/PUT and announces a body, and any upgrade request, gets
  `400 unexpected request body` before anything is read; the `503 busy` answer and `/metrics`
  read no body. A refused body is not read on: the socket's read side is shut and
  the connection closed with `Connection: close` (tiny_http, patched in
  `server/api/patched/tiny_http`, used to read the
  rest, and allocated the whole declared `Content-Length` to do so, so one request with a huge one
  ended the process). The patch also gives each accepted connection a 30 s read timeout (stalled
  request heads, small bodies and idle keep-alive connections are dropped) and serves a burst of
  connections at once. Reached directly (not through the proxies), 32 stalled POST bodies can
  still hold all workers for up to 30 s; Caddy buffers request bodies up to 600 KiB on `/api/*`,
  `/q/*` and the release-index route, and nginx buffers them except on the two upload routes.
  tether-api closes an idle connection after 30 s, and Caddy and nginx keep idle connections to
  it for 20 s only, so the new proxy configuration goes out together with the new tether-api.
  The nonce of a release upload is remembered for about 70 minutes (other requests: 601 s), so a
  captured upload cannot be replayed as a slow upload that ends after the nonce was forgotten.
  Per-IP limits count an IPv6 client by its /64 and an IPv4 client reported in the mapped form
  (`::ffff:a.b.c.d`) as that IPv4 address. `PUT /api/admin/policy` validates the bundle
  strictly. The release signature (over up to 200 MiB), audit log queries and preparing
  a `/q/` copy run outside the state lock. Device lists and `devices.json` are written from the
  device map without copying it, the lists through a temp file (MemoryMax 64M at 50 000 devices,
  [docs/scaling.md](docs/scaling.md)); custom-app profiles are read from disk when needed.
- hbbs: messages for a host on its WebSocket/TCP signalling path go through a queue of 64 per
  connection (more are dropped), and a write that does not complete within 5 s closes that
  connection, so a host that stops reading costs neither unbounded memory nor a stuck task.
  Every rendezvous write gives up after 5 s. A controller's punch socket is written by its own
  task from a queue of 64, so the main loop never waits for it. Rendezvous messages are limited
  to 512 KiB (TCP and WebSocket), SDP fields to 64 KiB and `relay_server` to 1024 bytes.
- hbbs stores its key pair and a registry of ID, machine uuid, device public key and IP
  (`vendor/rustdesk-server/src/database.rs`); tether-api stores heartbeats, policies, releases,
  the team roster and the audit log ([docs/api.md](docs/api.md)). No relay process sees session
  content; hbbs with the server key could impersonate hosts ([threat model](#threat-model)).
- Monitoring ([docs/monitoring.md](docs/monitoring.md)): every metrics endpoint (hbbs 21123, hbbr
  21124, tether-api `/metrics` on 21120, Caddy 21125, node_exporter 9100, blackbox_exporter 9115,
  Prometheus 9090, Alertmanager 9093 with clustering off, Grafana 3000) listens on 127.0.0.1;
  tether-api answers `/metrics` to loopback peers only and Caddy never forwards that path. Metrics
  carry no peer IDs, IPs, keys or tokens, only fixed label sets (plus the relay names from hbbs's
  `-r` list). Exposed through Caddy on 443: on a scraped node `/_metrics/*`, which closes the
  connection for every IP except the monitoring node's and then needs the scrape password (bcrypt
  in the Caddy snippet); on the monitoring node `/grafana/`, behind Caddy basic auth and then
  Grafana's own login (no anonymous access, no sign-up, basic auth to Grafana off). Whoever has
  the scrape password and the monitoring node's IP can also make the other node's
  blackbox_exporter probe targets of their choice (`/_metrics/probe`); the password never leaves
  the monitoring node. No firewall port is opened. The passwords
  (`/etc/tether-monitoring/*.pass`) are generated on the node and never committed.
- Keep `/var/lib/tether/id_ed25519` secret and back it up: with it, whoever controls a rendezvous
  path can impersonate any host. hbbs (or hbbr, if it starts first on a fresh node) creates it `0600` inside the `0750`
  directory; `rotate-key.sh` installs keys `0600`, and `install.sh` corrects owner and mode of
  existing key files. `rotate-key.sh rollback` refuses a second run. Losing it, or a suspected leak, means a
  server key rotation ([docs/key-rotation.md](docs/key-rotation.md)); clients without key-list
  support (before this version) must be updated before that.

## Known weaknesses

Details and code references: [docs/encryption.md §11](docs/encryption.md#11-known-weaknesses).
Update the status here and there together.

| ID | Weakness | Status |
| --- | --- | --- |
| W1 | Key exchange version 0 (one key, same nonces both ways) is accepted from and used with non-Tether peers; the shield still shows | fixed in the next release |
| W2 | Legacy (non-multiplexed) TCP tunnels switch to raw mode: no inner encryption (needs `permission.tunnel`, default N) | fixed in the next release |
| W3 | The trusted server key and custom server are local options (UI only with `settings.server`; a machine admin always) | fixed in the next release |
| W4 | Rendezvous/relay endpoints given as IP addresses use plain `ws://` | fixed in the next release |
| W5 | hbbs/hbbr WebSocket ports 21118/21119 bound all interfaces; reached directly (no host firewall) they took client-sent IP headers and had no TLS | fixed, deployed 2026-11-07 |
| W6 | Host authenticity rests on hbbs and the server key (no pinning, no own-fingerprint display, no viewer key) | open — by design today |
| W7 | Connecting by IP address skips the handshake and the refusal (plaintext against a stock host) | fixed in the next release |
| W8 | `RegisterPk` with an ID's uuid from the same IP replaces its key (upstream rule). Over UDP the source IP could be forged. Once the attacker re-registers the new key from its own IP, the real host gets `UUID_MISMATCH` and takes a new ID; the attacker keeps the old one. Side effect of the UDP fix: a stock RustDesk client that registers over UDP again with a new key but the same machine uuid (config wiped, reinstall) gets `UUID_MISMATCH` and takes a new ID; Tether clients register over WebSocket and are not affected | UDP path fixed in the next release (a registered key is never changed over UDP); TCP/WebSocket from the same IP (same NAT or proxy): open |
| W9 | Update manifest unsigned (rollback to an older signed build, blocking via `minimum_version`); neither app checked the signed trusted comment, so an older signed build could be offered under a higher version | fixed in the next release (trusted-comment check, signed release index with serial floor) |
| W10 | macOS direct listener is not limited to the local subnet | open |
| W11 | Traffic analysis: frame sizes and keystroke timing visible to relay and proxy | open |
| W12 | A captured challenge response allows offline password guessing | open |
| W13 | Audit records are unauthenticated and can be forged | open — by design today |
| W14 | A v1 `Tether-Sig` names no relay: a captured admin or team request can be replayed at another relay within ±300 s (the replayer needs a view inside TLS: a relay that received it, or an inspecting proxy) | relays accept v2 (host-bound) since 2026-11-10, apps sign it from the next release; open while relays accept v1: set `TETHER_SIG_V1=off` once every app signs v2 |
| W15 | The recording upload token travels in the public policy bundle (`GET /api/policy` needs no authentication): anyone who reaches the relay can read it | open — by design today: the token must be write-only (PUT/create; no list, read or overwrite) |
| W16 | A release upload slot is granted on an unverified `Tether-Sig` header (the signature covers the body): made-up uploads can hold the 4 release slots, about 55 min each, and block release uploads; nothing else | open |
| W17 | A forged UDP `RegisterPk` (an ID's uuid and its unchanged key) or `RegisterPeer` (from the host's IP) moves the address hbbs stores for a host registered over UDP until it registers again. Hosts registered over TCP/WebSocket (all Tether hosts) are not affected while their connection is open | fixed for TCP/WebSocket hosts, deployed 2026-11-10; open for UDP hosts (stock clients) |
| W18 | Own-window guard: Windows Mouse Keys (a key press becomes a click at the pointer while another window is in front) and the On-Screen Keyboard (clicks on its window type into a focused Tether window) get past it | open |

## Reporting

Report vulnerabilities privately to the maintainers. Do not open a public issue.

## Known limits

- An attacker with code execution on the endpoint can read whatever that endpoint displays.
  Endpoint security is out of scope.
- The ID ↔ IP registry on the relay is metadata visible to the relay operator. Self-host the relay
  to keep it in-house.
- Host authenticity rests on hbbs and the server key: no host-key pinning, no viewer identity key
  ([docs/encryption.md](docs/encryption.md#active-attacks-with-the-server-key)).
- hbbs lets a registration over TCP/WebSocket with an ID's uuid from the same IP replace that
  ID's key (upstream rule; over UDP no longer); the uuid is not secret
  ([docs/encryption.md](docs/encryption.md#related-replacing-a-hosts-key-at-hbbs-without-the-server-key)).
- The recording upload token is public: it is delivered in the policy bundle, which every device
  and anyone else who reaches the relay can read. Use a token that can only create objects (no
  list, read or overwrite) ([docs/recording.md](docs/recording.md)).
- While a relay accepts v1 request signatures, a captured admin or team request can be replayed at
  another relay within ±300 s. Remedy: update the relays first (with `RELAY_HOST`), then the apps,
  then set `TETHER_SIG_V1=off` (W14).
- A device's Tether ID in the health view rests on a token issued for that ID or on a
  technician's confirmation, both the word of a trusted person, not a cryptographic proof by
  Tether on that machine ([Device status](#device-status-health-reports)). Reports are readable
  by the relay operator and every team member, not end-to-end encrypted. The same holds for the
  software list ([The software list](#the-software-list)), which also holds the hardware identity.
- A custom app's settings apply above the policy profile, except for features the profile locks:
  it can add restrictions and defaults, and its branding (title, accent, welcome text) applies,
  but it cannot unlock what the policy locks.
- The background service takes its device identity for group policy from a core option
  (`tether-install-uuid`) that the signed-in user's app sets. Any local process that can write
  core options over the service's IPC can set it and so choose which signed policy group applies
  to the service. It cannot create or change a policy, machine policy (GPO/MDM) still wins, and
  the webview cannot write this option.
- File transfer: a junction inside an allowed folder that points into Tether's configuration is
  followed by a recursive download.
- Uninstalling with "Delete the application data" removes `%APPDATA%\Tether` of the signed-in user
  and of the elevating administrator and the service's copy; other users' profiles on the PC keep
  theirs.
- macOS builds must be signed and notarized, and Windows builds code-signed, before distribution.
  Unsigned binaries cannot be authenticated by users. Signing keys go into CI secrets, never into
  the repo.

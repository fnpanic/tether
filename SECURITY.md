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
| Malicious technician (legitimate session) | Everything the session permissions allow, as the logged-in user | Click or type into Tether's own windows (`own_ui_guard`); read or change Tether's config/install folders via file transfer; use Tether's own recorder without the host user's consent; change host settings remotely (`allow-remote-config-modification = N`) | Full control of the OS as that user by design: start programs, terminal, files outside the guarded folders, admin rights if UAC is approved. Recording with any other tool (screen recorder, modified viewer) is not prevented: the consent gate is enforced by the honest viewer |
| Malicious customer (host side) | Show fake content; record their own screen; keep what the technician sends (files, clipboard); put content into the technician's clipboard while clipboard sync is on | Inject keyboard or mouse input into the technician's PC (input messages flow from viewer to host only); read files the technician does not send | The host is the trusted endpoint for what it shows; treat files and clipboard from a host as untrusted |
| Compromised relay host **without** the server key (hbbr only, Caddy, TURN) | See IPs, timing, sizes, host IDs; drop or delay traffic; serve stale downloads | Decrypt or alter session content (session keys are sealed to the host's ephemeral key; [docs/encryption.md §8](docs/encryption.md#8-who-can-decrypt-who-can-attack)); forge policies, key lists or updates (admin/updater signatures) | Denial of service; metadata; legacy TCP tunnels (W2) and version-0 sessions with non-Tether peers (W1) are readable |
| Compromised hbbs **with** the server key | Everything above, plus sign any key for any ID | Decrypt past or running sessions (forward secrecy) | **Undetected man-in-the-middle of new sessions** by impersonating hosts: no host-key pinning and no viewer authentication ([docs/encryption.md](docs/encryption.md#active-attacks-with-the-server-key)). Mitigated only by keeping the key secret, hardening and monitoring the relays, and rotating the key on suspicion ([key rotation](docs/key-rotation.md)) |
| Network attacker (Wi-Fi, ISP, LAN) | See IPs, timing, TLS record sizes, SNI; block traffic | Read rendezvous or session data (TLS with verified certificates, no unverified fallback, then end-to-end encryption); answer as a LAN host without its signed key | Denial of service |
| TLS-inspecting corporate proxy | Read the rendezvous signalling (IDs, machine uuids, device public keys, connect requests) and the clear handshake at the start of each relay stream | Decrypt session content (except W1, W2) or swap the host key without the server key | With a stolen server key, the MITM above. Clients behind one proxy share an IP as hbbs sees it, which matters for the `RegisterPk` key replacement in [docs/encryption.md](docs/encryption.md#related-replacing-a-hosts-key-at-hbbs-without-the-server-key) |
| Local malware on the host | Read the screen and input like any program; read the config (device key, permanent password hash, which is password-equivalent); use Tether's IPC as the user | – (endpoint security is out of scope) | Full compromise of that host's sessions |
| Local malware on the viewer | Read the session, inject input into it, read remembered passwords | – | Full compromise of that viewer's sessions |
| Stolen technician laptop | Remembered peer passwords (obfuscated with the machine uid, so readable), trusted-device tokens, the team member key | Admin actions: the technician app has no admin key; if Tether Manage is on the same laptop, its vault still needs the passphrase | Unattended access to remembered hosts until their passwords change; remove the team key from the roster ([Team access](#team-access)) |
| Malicious update server (compromised relay serving `/download/`) | Withhold updates; edit the unsigned `latest.json`: claim a higher version for an **older** signed build (rollback), or raise `minimum_version` so clients block sessions | Make a client run code not signed by the updater key (minisign key `1AD0C3FBDCA3B4EA`, verified before install) | Rollback to an older signed release; denial of service |
| Holder of the admin and updater keys | Change policy within the floor, trust another server key, ship signed code | Turn off TLS verification or end-to-end encryption by policy (`FLOOR`) | Fully trusted: guard the vault and passphrase |
| Mac build runner sender | Run signed jobs on the build Mac | Run unsigned jobs, act as another sender, replay jobs ([docs/mac-build-runner.md](docs/mac-build-runner.md#security-model)) | A job is arbitrary code as the Mac user |

The relay operator is **not trusted with content**: hbbr and Caddy forward ciphertext. hbbs is
trusted to sign the right host key; with the server key, a compromised hbbs is the one relay
compromise that reaches content.

## Encryption everywhere

| Layer | Mechanism | Enforced in |
| --- | --- | --- |
| Transport to the relay | TLS WebSocket on 443 only (rustls default versions, TLS 1.2 or 1.3 with rustls; the native-tls retry uses the OS stack's range, and Caddy/nginx accept 1.2 or later; Let's Encrypt certificate, verified against the bundled Mozilla roots plus the OS root store, retried with the OS TLS stack, so a corporate inspection CA installed in the OS is accepted; never an unverified fallback). A fallback route uses TLS on port 80. TLS is chosen only for domain names: an endpoint given as an IP address gets plain `ws://` (W4 below). With policy `network.direct = auto` and no proxy: direct paths beside it (LAN listener on the local subnet only; WebRTC/ICE for IPv6 and NAT hole punching, STUN only against our own relays, signalling only over the TLS WebSocket). Never with a proxy. | `apps/desktop/src-tauri/src/netpolicy.rs`, `docs/corporate-networks.md` |
| Client ↔ rendezvous server | The WebSocket to hbbs is authenticated by TLS alone (the upstream rendezvous key exchange is skipped on WebSocket, `common.rs::secure_tcp_impl`). The compiled-in Ed25519 server public key (`TETHER_PUB_KEY`, release builds refuse to compile without it) is what viewers verify **host keys** with: hbbs signs each host's public key with the server key. A server key list signed by the **admin key** can replace it (key rotation without reinstalling): while the configured server key is one of the listed keys, clients trust the active listed keys; lower serials are rejected, so a relay can neither widen nor roll back the set. A `key` option outside the list is trusted on its own; the UI cannot set it unless policy allows `settings.server` (default N), but a machine administrator can (W3 below). | `apps/desktop/src-tauri/build.rs`, `hbb_common::config::RS_PUB_KEY` / `server_keys_for`, `tether_trust::serverkeys`, [docs/key-rotation.md](docs/key-rotation.md) |
| Server key check | `hbbs -k _` / `hbbr -k _` reject clients that do not present the matching **public** key (`licence_key`), or one of `KEYS_ACCEPT` during a rotation. The public key is in every client, so this is a configuration check, not client authentication. | `server/systemd/*.service`, `server/docker-compose.yml`, `vendor/rustdesk-server/src/common.rs::key_accepted` |
| Peer ↔ peer (end-to-end) | Each device has an Ed25519 identity whose public key hbbs signs. The host signs a fresh X25519 key; the viewer seals a random 32-byte session key to it (`crypto_box`), and both derive one key per direction (BLAKE2b over the handshake transcript). Relay and LAN traffic is XSalsa20-Poly1305 (libsodium secretbox) with counter nonces. On WebRTC the stream is protected by DTLS only (the secretbox key is not applied there); the host signs its DTLS fingerprint into its identity (`IdPk.dtls_fingerprint`) and a mismatch or missing fingerprint fails closed. Forward secrecy: yes (ephemeral keys). Not covered: hbbs with the server key can impersonate a host ([docs/encryption.md](docs/encryption.md#active-attacks-with-the-server-key)); key exchange version 0 with non-Tether peers is broken (W1); legacy TCP tunnels run without the inner encryption (W2). | `client.rs::secure_connection`, `server.rs::identity_handshake`, `hbb_common::tcp::Encrypt`, `hbb_common::webrtc` |
| **No plaintext fallback (viewer)** | Upstream asks the user whether to "continue insecurely". Tether refuses the connection outright. **Exception:** a connection typed as an IP address or `host:port` skips the rendezvous, the handshake and this check; against a Tether host it fails, against a stock RustDesk host with direct IP access it would run unencrypted. | `vendor/rustdesk/src/client.rs::confirm_insecure_connection` (`#[cfg(feature = "tauri")]`), `client/io_loop.rs` (`is_direct_ip_access`) |
| **No plaintext fallback (host)** | The host aborts the handshake when the peer offers no key, sends no key exchange, the transport is not flagged secure, or this device's key pair is missing. The direct listener uses the same handshake. | `vendor/rustdesk/src/server.rs::identity_handshake`, `rendezvous_mediator.rs::direct_server` |
| Secrets at rest | Permanent password (stored as `SHA256(password ‖ salt)`, which is password-equivalent for login), 2FA secret and remembered peer passwords are wrapped with secretbox under a key made from the machine uid (`symmetric_crypt`). The machine uid is not secret (readable by local users, sent to hbbs at registration), so this is obfuscation, not protection against someone who can read the config. The device's Ed25519 secret key is stored unencrypted. Config files are `0600` on Linux/macOS; on Windows they inherit the profile folder's ACL. | `hbb_common::password_security::symmetric_crypt`, `hbb_common::config` |

## Authentication

| Control | Default | Where |
| --- | --- | --- |
| Approval | Incoming sessions need either a click on **Accept** on the host or a valid password (configurable to click-only or password-only) | Settings › Security |
| One-time password | **10 characters** from a 32-character alphabet, 50 bits (upstream default is 6). Rotated after 10 consecutive wrong attempts. Never sent: the viewer answers a per-connection challenge with `SHA256(SHA256(password ‖ salt) ‖ challenge)` inside the encrypted stream. | `security.rs` DEFAULTS, core `server/connection.rs` (`check_update_temporary_password`, `verify_h1`) |
| Permanent password | **≥ 12 characters, ≥ 3 character classes, ≤ 128.** Validated in Rust (authoritative) and in the UI. | `security.rs::check_password_strength`, `src/lib/security.ts` |
| Two-factor authentication | Optional TOTP (RFC 6238, 6 digits) required after the password. Enrolment needs a verified code before it takes effect. | Settings › Unattended access, core `auth_2fa.rs` |
| Other ways in | Bearer secrets, not identities: a reconnect within 120 s with the session's `companion_secret` (resume ticket) and a file-transfer companion of a running session skip password, 2FA and Accept; a trusted-device token skips 2FA; upstream's recent-session rule accepts the previous one-time password for 30 s. A server-key MITM learns `companion_secret`. | core `server/connection.rs` (`is_resume_of_authed_session`, `is_file_transfer_of_authed_session`, `is_recent_session`), [docs/encryption.md §6](docs/encryption.md#6-password-authentication) |
| Brute-force protection | Failed logins are counted per IP and per IPv6 /64, /56 and /48 prefix: more than 6 in a minute are throttled, more than 30 (IPv6 prefixes: 60/80/100) blocked. Counters live in memory and reset on host restart. For relayed sessions the IP is the one hbbs reports. The one-time password rotates after 10 consecutive failures. | core `server/connection.rs` (`LOGIN_FAILURES`, `check_failure`) |
| Identity verification | Viewers see whether the session is end-to-end encrypted and which path it uses, and can hover the shield for the host key's fingerprint. No Tether app shows a device its own fingerprint, so an out-of-band comparison is not supported yet, and the viewer does not remember host keys between sessions. The host cannot verify the viewer: its ID and name are self-reported. | `Session.tsx`, `client/io_loop.rs` |

## Central management (0.3)

| Control | Mechanism | Where |
| --- | --- | --- |
| Policy integrity | Policies are signed by the **admin key** (Ed25519), which never leaves the admin's PC. Clients verify against compile-time public keys, so a compromised relay cannot push or alter a policy. Lower serials are rejected (no rollback). | `crates/tether-trust`, `apps/*/src-tauri/src/policy.rs` |
| Server key list | The admin key also signs which server keys devices trust (`GET /api/server-keys`). Whoever holds the admin key can therefore make devices trust another server key; it still needs a path into the TLS relay connection to use it. New server private keys are sealed in the vault like the other keys. Devices keep the highest accepted serial beside the cache (`server-keys.serial`), so deleting the cache does not reopen a rollback. The private key file exported for the relays is created readable by the current user only, and Manage keeps asking to delete it. | `tether_trust::serverkeys`, `apps/manage/src-tauri/src/keys.rs`, [docs/key-rotation.md](docs/key-rotation.md) |
| Security floor | TLS verification, end-to-end encryption, 443-only networking and "no silent recording" are not policy features. The apps apply them on top of any policy. | `tether_trust::policy::FLOOR`, `security.rs`, `netpolicy.rs` |
| Serials | Tether Manage takes the next policy and key list serial only from documents that verify with its admin key and from its own published copies (never from what a relay reports), with an overflow check. | `apps/manage/src-tauri/src/main.rs` `next_serial` |
| Response size | Policy and key list answers are read with a hard limit (stop after limit + 1 bytes), so a relay cannot make a client buffer an endless answer. | `tether_trust::read_limited` |
| Admin API | Every admin request is signed over method, path, timestamp, nonce and body hash. The server allows ±300 s clock skew and rejects replayed nonces (persisted across restarts). | `tether_trust::reqauth`, `server/api` |
| Keys at rest | Admin and updater private keys are encrypted with argon2id + XChaCha20-Poly1305 under the admin's passphrase. The vault has a format version: Tether Manage never saves a vault newer than it understands and keeps unknown fields, so it cannot drop keys it does not know. | `apps/manage/src-tauri/src/keys.rs` |
| Update integrity | Installers and TetherQS are signed with the updater key (minisign, key id `1AD0C3FBDCA3B4EA`) and verified against the compiled-in `TETHER_UPDATER_PUB_KEY` before installation. Clients below the minimum version cannot start or accept sessions. The manifest (`latest.json`: version, mandatory, minimum version) is **not** signed: a compromised relay can offer an older signed build under a higher version number or block sessions with a high minimum version, but cannot get unsigned code installed. | `update.rs` (both apps), [docs/updates.md](docs/updates.md) |
| Recording consent | Tether's recorder requires an explicit Allow from the other party for every connection, regardless of policy. The viewer never assumes the permission. This binds honest viewers only: the host just withholds the permission, so a screen recorder or a modified viewer is not stopped. | core `client.rs`, `server/connection.rs`, `docs/recording.md` |
| Diagnostics | Exports redact passwords, salts, key pairs and tokens. Uploads are accepted only for tickets an admin opened (8 characters, 24 h, one upload, kept 7 days). | `diag.rs`, `server/api` |

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
  within the limits (8 KiB per record, 120/min per IP, 60/min per device, size cap). Records are
  never used for anything else (no device, policy or access decision); read them as a log of what
  hosts reported, not as proof. A host that cannot reach the first relay within 120 s drops the
  record (its log says "Audit post dropped").
- **Privacy and retention**: names, IP addresses and file paths are personal data. Files are
  `0600` under `/var/lib/tether-api/audit/`, readable only with an admin-signed request, kept 90
  days or 50 MB (`AUDIT_RETENTION_DAYS`, `AUDIT_MAX_MB`; `AUDIT_RETENTION_DAYS=0` stores nothing).
  CSV exports leave the relay: handle them like the records.

### Team access

Technicians get an org-scoped identity without the admin key: each technician app generates its own
Ed25519 key, the admin adds its public key to the team roster in Tether Manage and publishes the
roster to the relays (`PUT /api/admin/team`, admin-signed). Member requests (`/api/team/*`) are
signed with the member key in the admin request format (method, path with query, timestamp, nonce,
body hash; ±300 s; replayed nonces rejected, same persisted cache as admin requests).

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

## Least privilege and attack surface

Settings below are **forced** for every Tether process (UI, server, service and connection manager).
Neither the user nor a remote peer can change them (`apps/desktop/src-tauri/src/security.rs`):

- `allow-remote-config-modification = N`: a connected peer can never change this machine's settings.
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

Privacy defaults that the user can change:

- `enable-lan-discovery = N`: the device is not discoverable on the local network.

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
- **File transfer** refuses to read, write, rename or delete Tether's configuration (`AppData  Roaming\Tether*`, also under `ServiceProfiles\LocalService`; macOS `Library/Preferences/
  app.tether.*`) and its installation (the install folder, the `.app` bundle, or a portable
  TetherQS exe itself). Paths are resolved (`..`, links, short names) before the check.
- The technician sees "The Tether window on the remote computer can only be used by the person at
  that computer." (at most every 5 s; older viewers ignore it).

What this does **not** stop (the technician still controls the computer as its user):

- Starting programs (Start menu, Win+R) and using them: a command prompt can run
  `tether.exe --password …` / `--config` / `--option` only as an administrator of an installed
  Tether (`core_main` checks it); a technician who already has admin rights (UAC approved, or the
  user is an admin) can change Tether like any other software. Programs running as the user can
  also talk to Tether's local IPC the way its own windows do.
- The terminal feature (when the host allows it) and file transfer outside the protected folders
  are full access by design.
- A drag started outside a Tether window may be released over one (the release is not dropped, so
  no button gets stuck); Tether's windows do not act on drops.
- Windows that are not Tether's but change Tether (e.g. the Windows Services console, uninstall
  in Settings) are ordinary programs.
- macOS: the filter is compile-unverified until the next Mac build (0.5.6).

Tests: unit tests in `own_ui_guard.rs` (`cargo test --lib --no-default-features --features
tauri,hwcodec,use_dasp own_ui_guard` in `vendor/rustdesk`), VM end-to-end
`apps/desktop/e2e/vm-noremote-e2e.mjs` (remote clicks on every TetherQS permission switch,
Disconnect, Close and the recording consent's Allow; keys while TetherQS is in front; Notepad still
works; Alt+Tab leaves; a lone Alt and Alt+F4 on TetherQS do not stall it).

## The app shell (Tauri)

- Strict CSP: `default-src 'self'`, no remote scripts, `connect-src` limited to Tauri IPC
  (`tauri.conf.json`).
- No shell, filesystem, HTTP or updater plugins. The webview can only call Tether's own commands.
- Every IPC command validates its input at the boundary (`commands.rs`):
  - peer IDs match `[A-Za-z0-9_.@:-]{1,64}`;
  - session keys are 32 hex characters;
  - options must be on an allow-list (`security::UI_OPTIONS`), so a compromised webview cannot
    enable remote configuration or change 2FA directly;
  - message, password and value lengths are bounded;
  - connection types are an explicit enum.
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
  WebSocket ports 21118/21119 on 127.0.0.1. tether-api listens on 127.0.0.1 only; hbbs and hbbr (including 21118/21119)
  bind all interfaces, and the host firewall rules from `install.sh` open 21115-21117/tcp and
  21116/udp (stock RustDesk clients; UDP 21116 is also STUN/TURN for WebRTC), not 21118/21119.
  Port 80 is Caddy/nginx itself (ACME and redirect); on ufw/iptables hosts `install.sh` still
  redirects 80 to 443.
- Client IPs behind Caddy: hbbs takes the last `X-Forwarded-For` hop, which Caddy sets to the
  peer it saw; Caddy overwrites `X-Real-IP` (read by hbbr) the same way. A client that goes through
  Caddy/nginx cannot choose the IP the brute-force and IP-change checks see; one that reaches
  21118/21119 directly (no active host firewall) can, and gets no TLS (W5).
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
- TURN on hbbs's UDP port takes only datagrams from the exact address an allocation sent to in
  the last 5 minutes, never a rendezvous message; requests without a valid MESSAGE-INTEGRITY are
  capped at 50/s ([docs/stun-turn.md](docs/stun-turn.md)).
- tether-api serves up to 32 requests at once with 30 s read/write timeouts and a whole-body
  deadline, reading bodies outside its state lock, so a slow or stalled client holds only its own
  request.
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
  path can impersonate any host. hbbs (or hbbr, if it starts first on a fresh node) creates it with the process umask inside the `0750`
  directory; `rotate-key.sh` installs keys `0600`. Losing it, or a suspected leak, means a
  server key rotation ([docs/key-rotation.md](docs/key-rotation.md)); clients without key-list
  support (before this version) must be updated before that.

## Known weaknesses

Details and code references: [docs/encryption.md §11](docs/encryption.md#11-known-weaknesses).
Update the status here and there together.

| ID | Weakness | Status |
| --- | --- | --- |
| W1 | Key exchange version 0 (one key, same nonces both ways) is accepted from and used with non-Tether peers; the shield still shows | open — tracked |
| W2 | Legacy (non-multiplexed) TCP tunnels switch to raw mode: no inner encryption (needs `permission.tunnel`, default N) | open — tracked |
| W3 | The trusted server key and custom server are local options (UI only with `settings.server`; a machine admin always) | open — tracked |
| W4 | Rendezvous/relay endpoints given as IP addresses use plain `ws://` | open — tracked |
| W5 | hbbs/hbbr WebSocket ports 21118/21119 bind all interfaces; reached directly (no host firewall) they take client-sent IP headers and have no TLS | open — tracked |
| W6 | Host authenticity rests on hbbs and the server key (no pinning, no own-fingerprint display, no viewer key) | open — by design today |
| W7 | Connecting by IP address skips the handshake and the refusal (plaintext against a stock host) | open |
| W8 | `RegisterPk` with an ID's uuid from the same IP replaces its key (upstream rule; impact untested) | open |
| W9 | Update manifest unsigned (rollback to an older signed build, blocking via `minimum_version`) | open |
| W10 | macOS direct listener is not limited to the local subnet | open |
| W11 | Traffic analysis: frame sizes and keystroke timing visible to relay and proxy | open |
| W12 | A captured challenge response allows offline password guessing | open |
| W13 | Audit records are unauthenticated and can be forged | open — by design today |

## Reporting

Report vulnerabilities privately to the maintainers. Do not open a public issue.

## Known limits

- An attacker with code execution on the endpoint can read whatever that endpoint displays.
  Endpoint security is out of scope.
- The ID ↔ IP registry on the relay is metadata visible to the relay operator. Self-host the relay
  to keep it in-house.
- Host authenticity rests on hbbs and the server key: no host-key pinning, no viewer identity key
  ([docs/encryption.md](docs/encryption.md#active-attacks-with-the-server-key)).
- hbbs lets a registration with an ID's uuid from the same IP replace that ID's key (upstream
  rule); the uuid is not secret ([docs/encryption.md](docs/encryption.md#related-replacing-a-hosts-key-at-hbbs-without-the-server-key)).
- macOS builds must be signed and notarized, and Windows builds code-signed, before distribution.
  Unsigned binaries cannot be authenticated by users. Signing keys go into CI secrets, never into
  the repo.

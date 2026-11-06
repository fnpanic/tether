# Tether security model

Privacy and security come before convenience. This file lists what Tether protects, how, and where
each control lives in the code. If you change one of these controls, update this file in the same
change.

## Assets and threats

| Asset | Threat |
| --- | --- |
| Screen contents, keystrokes, clipboard, files | Network eavesdroppers, a malicious or compromised relay, Wi-Fi/LAN attackers |
| Control of a device | Password guessing, social engineering, stolen IDs, replay |
| Device identity | Impersonating a device or the rendezvous server (MITM) |
| Settings on a host | A connected peer silently weakening security |
| User privacy | Telemetry, update checks or discovery leaking data to third parties |

The relay operator is **not trusted with content**. The relay only forwards ciphertext.

## Encryption everywhere

| Layer | Mechanism | Enforced in |
| --- | --- | --- |
| Transport to the relay | TLS 1.3 WebSocket on 443 only (Let's Encrypt certificate, verified by rustls, falling back to the OS trust store for corporate TLS inspection; never an unverified fallback). With policy `network.direct = auto` and no proxy: direct paths beside it (LAN listener on the local subnet only; WebRTC/ICE for IPv6 and NAT hole punching, STUN only against our own relays, signalling only over the TLS WebSocket). Never with a proxy. | `apps/desktop/src-tauri/src/netpolicy.rs`, `docs/corporate-networks.md` |
| Client ↔ rendezvous server | Server identity pinned by a compiled-in ed25519 public key (`TETHER_PUB_KEY`). Release builds refuse to compile without it. A server key list signed by the **admin key** can replace it (key rotation without reinstalling): clients then trust exactly the listed keys; lower serials are rejected, so a relay can neither widen nor roll back the set. | `apps/desktop/src-tauri/build.rs`, `hbb_common::config::RS_PUB_KEY` / `server_keys_for`, `tether_trust::serverkeys`, [docs/key-rotation.md](docs/key-rotation.md) |
| Server key enforcement | `hbbs -k _` / `hbbr -k _` reject clients that do not present the matching key, or one of `KEYS_ACCEPT` during a rotation | `server/systemd/*.service`, `server/docker-compose.yml`, `vendor/rustdesk-server/src/common.rs::key_accepted` |
| Peer ↔ peer (end-to-end) | Each device has an ed25519 identity, signed via the rendezvous server. A session key comes from an X25519 box key exchange. Traffic is encrypted with XSalsa20-Poly1305 (libsodium secretbox) on TCP/relay and inside the WebRTC data channel, whose DTLS fingerprint the host signs into its identity (`IdPk.dtls_fingerprint`); a mismatch or missing fingerprint fails closed. | RustDesk core `client.rs::secure_connection`, `server.rs::identity_handshake` |
| **No plaintext fallback (viewer)** | Upstream asks the user whether to "continue insecurely". Tether refuses the connection outright. | `vendor/rustdesk/src/client.rs::confirm_insecure_connection` (`#[cfg(feature = "tauri")]`) |
| **No plaintext fallback (host)** | The host aborts the handshake when the peer offers no key, sends no key exchange, uses an unsecured transport (direct IP), or this device's key pair is missing | `vendor/rustdesk/src/server.rs::identity_handshake` |
| Secrets at rest | Permanent password, 2FA secret and remembered peer passwords are stored encrypted with a machine-bound key (core `encrypt_vec_or_original`) in the per-user config directory. | RustDesk core config |

## Authentication

| Control | Default | Where |
| --- | --- | --- |
| Approval | Incoming sessions need either a click on **Accept** on the host or a valid password (configurable to click-only or password-only) | Settings › Security |
| One-time password | **10 characters** (upstream default is 6). Rotated after repeated wrong attempts. | `security.rs` DEFAULTS, core `password_security.rs` |
| Permanent password | **≥ 12 characters, ≥ 3 character classes, ≤ 128.** Validated in Rust (authoritative) and in the UI. | `security.rs::check_password_strength`, `src/lib/security.ts` |
| Two-factor authentication | Optional TOTP (RFC 6238, 6 digits) required after the password. Enrolment needs a verified code before it takes effect. | Settings › Unattended access, core `auth_2fa.rs` |
| Brute-force protection | Failed logins are counted per IP and per IPv6 /64, /56 and /48 prefix. Attempts are throttled and then blocked. The one-time password rotates after too many failures. | core `server/connection.rs` (`LOGIN_FAILURES`) |
| Identity verification | Viewers see whether the session is end-to-end encrypted and whether it runs P2P or through the relay. Device fingerprints are exposed for out-of-band checks. | `Session.tsx` |

## Central management (0.3)

| Control | Mechanism | Where |
| --- | --- | --- |
| Policy integrity | Policies are signed by the **admin key** (Ed25519), which never leaves the admin's PC. Clients verify against compile-time public keys, so a compromised relay cannot push or alter a policy. Lower serials are rejected (no rollback). | `crates/tether-trust`, `apps/*/src-tauri/src/policy.rs` |
| Server key list | The admin key also signs which server keys devices trust (`GET /api/server-keys`). Whoever holds the admin key can therefore make devices trust another server key; it still needs a path into the TLS relay connection to use it. New server private keys are sealed in the vault like the other keys. | `tether_trust::serverkeys`, `apps/manage/src-tauri/src/keys.rs`, [docs/key-rotation.md](docs/key-rotation.md) |
| Security floor | TLS verification, end-to-end encryption, 443-only networking and "no silent recording" are not policy features. The apps apply them on top of any policy. | `tether_trust::policy::FLOOR`, `security.rs`, `netpolicy.rs` |
| Admin API | Every admin request is signed over method, path, timestamp, nonce and body hash. The server allows ±300 s clock skew and rejects replayed nonces (persisted across restarts). | `tether_trust::reqauth`, `server/api` |
| Keys at rest | Admin and updater private keys are encrypted with argon2id + XChaCha20-Poly1305 under the admin's passphrase. | `apps/manage/src-tauri/src/keys.rs` |
| Update integrity | Installers and TetherQS are signed with the updater key (minisign) and verified before installation. Clients below the minimum version cannot start or accept sessions. | `update.rs` (both apps) |
| Recording consent | Recording requires an explicit Allow from the other party for every connection, regardless of policy. The viewer never assumes the permission. | core `client.rs`, `server/connection.rs`, `docs/recording.md` |
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
- Session video arrives on a per-window binary channel. One window cannot read another session's
  frames.

## Server hardening

- `hbbs` and `hbbr` run as an unprivileged `tether` user with systemd sandboxing
  (`ProtectSystem=strict`, `ProtectHome`, `NoNewPrivileges`, `PrivateTmp`, a single writable path).
- Only TCP 443 (and 80, forwarded to 443) needs to be open: clients use WebSocket over TLS through
  Caddy (`docs/corporate-networks.md`). hbbs/hbbr/tether-api listen on localhost behind it.
- The server stores only its key pair and an ID → public-key/IP registry. It never sees session
  content.
- Keep `/var/lib/tether/id_ed25519` secret and back it up. Losing it, or a suspected leak, means a
  server key rotation ([docs/key-rotation.md](docs/key-rotation.md)); clients without key-list
  support (before this version) must be updated before that.

## Reporting

Report vulnerabilities privately to the maintainers. Do not open a public issue.

## Known limits

- An attacker with code execution on the endpoint can read whatever that endpoint displays.
  Endpoint security is out of scope.
- The ID ↔ IP registry on the relay is metadata visible to the relay operator. Self-host the relay
  to keep it in-house.
- macOS builds must be signed and notarized, and Windows builds code-signed, before distribution.
  Unsigned binaries cannot be authenticated by users. Signing keys go into CI secrets, never into
  the repo.

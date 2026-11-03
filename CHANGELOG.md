# Changelog

All notable changes to Tether. Versions follow [Semantic Versioning](https://semver.org/);
the desktop app version is kept in sync in `apps/desktop/package.json`,
`apps/desktop/src-tauri/Cargo.toml` and `apps/desktop/src-tauri/tauri.conf.json`.

## [0.3.0] — 2026-11-03

Central management, policies, updates, a customer module, recording with consent, CLI,
diagnostics and automatic reconnect.

### Added
- **Tether Manage** (`apps/manage`): a Tauri management app. It holds the admin key and the
  update signing key on the admin's PC (encrypted with a passphrase, with backup export and
  import), and handles policy editing, devices, releases, the download page and diagnostics
  tickets. See `docs/management.md`.
- **Relay API** (`server/api`, `tether-api`): a small service behind Caddy. It serves signed
  policy, heartbeats, diagnostics uploads, releases and the download page from flat files,
  using about 1 MB of RAM. Admin requests are signed (Ed25519, ±300 s clock skew, replay
  protection). See `docs/api.md`.
- **Signed policies** (`crates/tether-trust`):
  - Every setting is a feature that can be on, off, locked or hidden.
  - There are technician and customer profiles, plus device groups with overrides.
  - A machine layer can be set via GPO or MDM.
  - Clients verify the admin signature and reject older serials. Policy can never weaken the
    security floor (TLS, end-to-end encryption, 443-only). See `docs/policy.md`.
- **Locked settings** are shown with a "Managed by your organisation" badge. The backend
  refuses changes to them (`locked:` errors).
- **Automatic and enforced updates:**
  - The Tauri updater runs per channel, with optional, mandatory and forced modes.
  - Clients below the minimum version cannot start or accept sessions.
  - TetherQS updates itself (portable, signature-checked). See `docs/updates.md`.
- **TetherQS** (`apps/quick`): a portable customer module. It needs no install and no admin
  rights, accepts incoming connections only, and takes its branding from policy. "Allow admin
  access" elevates during a session. See `docs/quick-support.md`.
- **Session recording with consent:** the other party is always asked first, and saying no
  means nothing is recorded. Recordings are stored locally, with optional upload to a UNC share
  or HTTPS PUT and a retention setting. See `docs/recording.md`.
- **Automatic reconnect** after a connection loss: backoff, then the relay, and a "Reconnecting…"
  overlay over the last frame. The password is not asked again.
- **Command line:**
  - `tether --connect <id> [--password-stdin | --password-env VAR] [--file-transfer] [--view-only] [--fullscreen]`
  - `--get-id`, `--version`, `--diagnostics`, `--check-update`, `--policy-refresh`
  - Exit codes and `tether://connect/<id>` links. See `docs/cli.md`.
- **Diagnostics page:**
  - shows proxy, DNS and route attempts with timings;
  - "Test connection" and verbose logging;
  - export as a redacted zip, and "Send to support" with a ticket code.
  See `docs/troubleshooting.md`.
- `server/install.sh` now sets up a complete relay: hbbs, hbbr, Caddy, tether-api, the port-80
  fallback and DuckDNS.

### Changed
- The technician app trusts compile-time admin and updater keys. Release builds fail without
  `TETHER_ADMIN_PUB_KEY` and `TETHER_UPDATER_PUB_KEY`.
- The installer stops and restarts the background service around in-place updates.

### Fixed
- A password-only session could start recording before the host user was asked: the host's
  initial "recording off" is sent before login and was missed. The viewer now never assumes
  recording is allowed.

### Known limits
- Downloading an update tries only TLS on port 443 (through the proxy or direct). It has none of
  the port-80 or proxy-to-direct fallbacks the relay and API connections use.
- A host that only runs as a background service, with nobody signed in, learns about a required
  update from the cache the app window writes. It may not block incoming sessions until the app
  has been opened once.
- macOS: the builds compile in CI, but the updater, the MDM policy path and TetherQS are only
  tested on Windows.

### Verified
End to end between a PC and a Hyper-V VM in corporate mode (outbound blocked except a
TLS-inspecting proxy):
- TetherQS ran as a standard user.
- Policy and group overrides were applied.
- A CLI connection was made.
- Recording consent worked: Deny produced no file; Allow produced a playable file.
- After a relay restart the session reconnected in 2.7 s.
- TetherQS updated itself from 0.3.0 to 0.3.1 in 47 s.
- The technician app updated itself through the proxy.
- The diagnostics ticket round trip completed in about 7 s.
- Only port 443 tunnels were used.

## [0.2.1] — 2026-11-03

### Fixed
- **The app stayed offline after an upgrade.** Installing over an older version runs the old
  uninstaller, whose `--uninstall-service` step writes `stop-service = Y` to the user config even
  when no background service exists. Tether then never connected to the relay. Tether now always
  forces `stop-service` off (it has no "stop service" control), and the uninstaller only removes
  the background service when one is installed.

## [0.2.0] — 2026-11-03

Works in corporate networks and connects as reliably as AnyDesk.

### Added
- **443-only mode:** all traffic is WebSocket over TLS on TCP 443 to the relay. No UDP, no
  direct P2P, no listening ports, no LAN discovery. See `docs/corporate-networks.md`.
- **Automatic proxy detection:** `HTTPS_PROXY` / `ALL_PROXY`; on Windows the WinINet settings,
  PAC files and WPAD auto-detect; on macOS `scutil --proxy`. Rechecked every 60 s.
- **TLS-inspecting proxies** (Zscaler-style) work through the OS trust store. Sessions stay
  end-to-end encrypted inside the tunnel.
- **Connection racing:** to its own relay, Tether tries 443 and 80, each through the proxy and
  direct, 300 ms apart. The first route that works wins and is tried first next time. See
  `docs/connection-resilience.md`.
- Relay: Caddy front end on 443 (Let's Encrypt), with TLS on port 80 forwarded to it.
- Relay: hosts can register and receive connection requests over WebSocket. This is a patched
  `hbbs`, `patches/rustdesk-server.patch`.

### Changed
- The "Discoverable on local network" setting was removed (LAN discovery is off).
- Connections are always relayed; there is no direct P2P path.

### Removed
- Polling of `/api/heartbeat` (the relay has no API server).

### Upgrade notes
- 0.1.x installers use the old ports (21115–21117, UDP 21116) and will not get through
  443-only networks. Replace them with this version.

## [0.1.0] — 2026-11-02

First release: a self-hosted AnyDesk replacement on the RustDesk core.
- Tauri desktop app for Windows (macOS via CI) with an Apple-style design (`DESIGN.md`).
- FluxRT video pipeline: hardware encoding, WebCodecs decoding, 60 fps.
- Pinned server key, mandatory end-to-end encryption, no insecure fallbacks (`SECURITY.md`).
- Relay on Oracle Cloud Always Free at `tether-desk.duckdns.org`.
- Benchmarks against AnyDesk (`docs/benchmarks.md`).

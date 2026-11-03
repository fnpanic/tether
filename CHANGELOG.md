# Changelog

All notable changes to Tether. Versions follow [Semantic Versioning](https://semver.org/);
the desktop app version is kept in sync in `apps/desktop/package.json`,
`apps/desktop/src-tauri/Cargo.toml` and `apps/desktop/src-tauri/tauri.conf.json`.

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

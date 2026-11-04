# Changelog

All notable changes to Tether. Versions follow [Semantic Versioning](https://semver.org/);
the desktop app version is kept in sync in `apps/desktop/package.json`,
`apps/desktop/src-tauri/Cargo.toml` and `apps/desktop/src-tauri/tauri.conf.json`.

## [Unreleased]

### Fixed
- **TetherQS did not notice a release published while it was running.** It checked only at start
  and then every `update.check-hours` (6 h), with no way to check by hand and no result shown.
  **Help → Check for updates** now checks at once and shows the result (up to date, version
  available, or why the check failed per server); Help also shows the result of the automatic
  check. A failed check is retried after 15 minutes instead of 6 hours.
- **TetherQS asked only the first relay for updates.** It now asks each relay in turn; the first
  one with a release wins, so a relay that is down no longer hides the update.
- **"Updating…" showed no progress** while the 35 MB download ran (a minute or more at the relays'
  speed); the button now shows the percentage. The download is also capped while streaming.

## [0.5.4] — 2026-11-04

### Added
- **Tether Manage 0.5.3: progress while publishing releases.** Publish the latest builds, Sign
  and publish and Publish to missing relays used to show only "Publishing…" for minutes while
  30–40 MB files went to every relay. The sheet now shows which build is being signed or
  uploaded, to which relay (1 of 2) and which file, with a progress bar in MB and percent, and
  lists each finished build × relay with its error as it happens. The result lists per relay
  what it now publishes (or every error).

### Fixed
- **Mac: "cannot see this screen yet" and the Screen Recording prompt kept coming back
  although System Settings showed the app as allowed.** The switch belonged to the earlier
  ad-hoc signed build; macOS does not count it for the newly signed app and asks again. The
  warning's button is now **Fix Screen Recording** / **Fix Accessibility**: it clears this app's
  old entry (`tccutil reset`, no admin), lets macOS add it afresh and opens System Settings;
  switch the app on and choose Quit & Reopen. Needed once; later updates keep the permission.
- **The macOS permission prompt appeared on every connection.** It now appears once per app run;
  the technician is still told on every connection what is missing.

## [0.5.3] — 2026-11-04

### Fixed
- **Mac host: still only the top-left part of the screen.** The viewer's Statistics showed
  `frame 1280×720 (coded 3456×2240)`: WebView2's HEVC decoder reports a visible area of
  1280×720 for a Retina Mac's 3456×2234 picture (0.5.2 only handled "exactly half"). The viewer
  now sizes the picture from the host's display size when the coded frame matches it (plus
  alignment padding), and otherwise draws the whole coded frame when the reported visible area
  is far smaller. Clicks land where they are shown.
- **Fit to window did not enlarge a picture smaller than the window** (it stayed small in the
  middle, also in full screen). Fit now scales up as well as down.
- **Mac host: "Waiting for the first image…" forever, with no reason given.** An updated
  ad-hoc signed Mac app loses its Screen Recording permission (macOS ties it to the code
  signature; the switch can still look on). The Mac then sends no picture, silently.
  - The Mac host now checks Screen Recording and Accessibility when a session starts, lets
    macOS show its prompt, and tells the technician what is missing.
  - The technician's window says so ("The Mac does not share its screen yet") with the steps,
    and after 8 s without a picture explains the likely cause for any host.
  - Tether and TetherQS on a Mac show a banner while either permission is missing, with buttons
    that open the right System Settings page.
- **Mac: the Screen Recording prompt came back after every update although System Settings
  showed it allowed.** CI now signs both Mac apps with one self-signed code-signing certificate
  (repository secrets `MACOS_SIGNING_P12`, `MACOS_SIGNING_PASSWORD`), so macOS keeps the
  permissions across updates. One last re-grant is needed when moving from an ad-hoc build
  (`tccutil reset ScreenCapture app.tether.desktop` / `net.tether.quick`, docs/macos.md).

## [0.5.2] — 2026-11-04

### Fixed
- **macOS host: only the top-left quarter visible, clicks missed.** WebView2 reports half the
  real visible size for a Retina Mac's stream; the viewer sized the canvas right but drew only
  that half, scaled up. It now draws the whole picture, so clicks land where they are shown.
- **File transfer from a session asked the customer again and showed a second connection.** The
  technician app sends a random 128-bit companion secret with each session login (only inside
  the encrypted connection; new `LoginRequest.companion_secret`), and the file-transfer window
  opened from that session presents the same secret and session key. The host lets such a file
  transfer in without asking only while that accepted session runs and its **Transfer files**
  permission is on; otherwise the customer is asked as before. TetherQS shows it on the session
  card ("Transferring files"), not as a second request; switching Files off or Disconnect ends
  it. The file-transfer window also no longer stays on "Password required" once the customer
  accepted by click (that is what made file transfer look broken).
- **Session toolbar hid while a menu was open** (the menu vanished under the pointer). It stays
  up while the pointer is over the bar or one of its menus.
- **TetherQS diagnostics showed one server.** Help lists every server it registers with, each
  with its own reachability check.

### Changed
- **Connecting is explained on both sides.** TetherQS: who is asking, that Accept lets them in
  without a password (and that typing the one-time password works too), a confirmation after
  Accept. The window moves to the bottom-right corner for a request or session and back to the
  centre afterwards, so it is always in the same place. Technician: the password screen says the
  customer may click Accept instead; the wait screen says the customer has to click Accept.
- **Remote actions menu** (session toolbar): Ctrl+Alt+Del, Lock screen and Restart in one menu.
  Ctrl+Alt+Del is disabled with the reason when the remote side has no service (TetherQS); lock
  and restart ask first and warn when the remote side cannot show the lock screen (you would not
  be able to sign back in) or will not start again by itself.
- **Audio is off by default** (`permission.audio` default `N`; turn it on per policy or per
  session).
- **Download page:** one Download button per card, all at the same height; signatures and
  updater archives are no longer on the page.
- **Tether Manage, Releases:** files are listed one line per release (installer, signature, disk
  image, updater archive together), expandable; **Delete release** removes all of a release's
  files after a warning.

## [0.5.1] — 2026-11-04

### Added
- **Tether Manage 0.5.1: Publish the latest builds.** Releases scans a build folder (e.g. `dist`,
  remembered), shows the newest build of each app and platform next to what the relays publish,
  and one click signs and publishes every newer one, then rebuilds the download page.

### Fixed
- **macOS: no window.** Tether and TetherQS started with only a Dock/menu-bar icon: the window is
  created hidden and shown by the page after its first paint, but macOS runs no animation frames
  in a hidden web view. A timer now shows it as well. CI starts both apps on a Mac after every
  build and fails without a window or with an admin prompt open (`scripts/smoke-macos.sh`).
- **TetherQS diagnostics without a network.** The Help page waited for the server check (up to
  30 s, longer while proxy detection hung) before showing anything. Local facts now show at once;
  the server check runs separately with a 5 s limit and **Retry**.
- **TetherQS on macOS: no "Allow admin access".** That button is Windows UAC elevation; on a Mac
  it has no function and could raise an admin dialog.

### Changed
- `docs/macos.md`: the admin password macOS asks for on first start of an unsigned app
  (Privacy & Security → Open Anyway) and how to avoid it.

## [0.5.0] — 2026-11-04

Branded TetherQS downloads, a much clearer Tether Manage, and HTML documentation.

### Added
- **Custom TetherQS apps.** Tether Manage creates a branded TetherQS (name, logo, accent colour,
  welcome text, customer settings) signed with the admin key, and a private download link
  `https://<relay>/q/<token>` (256-bit random, expires after at most 366 days, revocable, new link
  on demand, download counter). TetherQS verifies the signature itself; a damaged or foreign
  profile runs as plain TetherQS. Self-updates keep the branding. Windows only. See
  `docs/api.md` ("Custom apps") and `docs/quick-support.md`.
- **Live policy view** in Tether Manage: what each relay serves now (serial, time, signature check),
  effective values per profile with their source, and the draft ↔ live difference.
- **Devices filters:** active today (since local midnight), OS family, version, profile, group;
  devices from all relays merged; "stale" badge when a device runs an older policy serial.
- **Releases overview:** per channel × app × platform which version is published on each relay and
  which is the latest; "Publish to missing relays" copies a release to relays that lack it.
- HTML documentation: `docs/site/index.html` (user guide) and `docs/site/architecture.html`
  (architecture with diagrams).

### Changed
- **Publishing in Tether Manage** is one flow: a status bar on every screen ("Draft has N changes ·
  Live: serial S on X/Y relays"), one **Publish…** button, a review sheet listing the changes and
  checking every relay, then a per-relay result that is only ✓ once the relay serves the new
  serial, and a live count of devices that picked it up. Warns when the policy names a relay that
  is not in the relay list. The relay list is a setting on "Status and relays".
- Policy editor: selected state filled in the accent colour, changed features marked, "Changed
  only" / "Not published" filters. Selections, tabs, toggles and focus rings are clearly visible
  throughout Tether Manage, light and dark.
- Download page: one card per app with one download button; signatures and updater archives under
  "More files".
- TetherQS masks the one-time password by default (eye to show, copy still works).

### Fixed
- CI: patches apply on Windows runners (LF), macOS links (VRAM encoding Windows-only, clang runtime).

## [0.4.0] — 2026-11-04

Faster, steadier sessions, a working toolbar and file transfer, and relays that keep sessions
alive when one of them fails. Adds a macOS build and an OS-aware download page.

### Fixed
- **Picture froze while dragging a window on hosts without a GPU.**
  - The viewer asked for a key frame whenever its queue briefly passed 3, and every request
    restarted the host's video service. The result was refresh storms, and the picture stayed
    frozen for good if one request got lost.
  - Now a key frame is requested only when the decoder is really behind, and the request is
    repeated until one arrives.
  - Hosts with 4 GB of RAM or less keep AV1 instead of falling back to VP8, which managed ~3
    fps on screen content.
  - Software encoders use up to 4 cores instead of half of the idle ones.
  - Same window drag, same optical meter, AnyDesk 9.8.3 for comparison: AnyDesk p95 95 ms,
    Tether p95 98 ms; worst gap 509 vs 507 ms (on par, see `docs/performance.md`).
  - Measured during a window drag in the VM, egress only through the TLS-inspecting proxy:
    20–23 fps, a new picture at least every 99 ms, latency p95 59–66 ms. Before: 3–4 fps and freezes of 1.6 s or longer. See
    `docs/performance.md`.
- **Session toolbar:**
  - Every action now either works or shows why not.
  - The chat panel crashed the window ("white screen").
  - Failing calls were silent.
  - An error screen now replaces an empty window.
- **File transfer:**
  - It no longer asks for the password again: it reuses the live session's login.
  - Messages appeared twice because events reached every window instead of their own.
  - A replaced session is now closed instead of leaked.
- **Installer:**
  - It no longer asks to close Tether. It closes every Tether process from the install folder
    itself and reopens Tether afterwards.
  - Upgrades install in place: the old version's uninstaller, whose hooks could not close
    Tether, is no longer run.
- **Only one Tether.**
  - A second start brings the running window to the front.
  - `--connect` and `tether://` links from a second start open the session in the running
    app. The password goes over local IPC only.
  - The RustDesk tray icon no longer appears.
- **Recording consent:** a pending recording request now survives until the host answers.

### Added
- **Fast reconnect:**
  - Retries start at once.
  - A silent relay is detected within 3 s.
  - Waiting for the host on the relay is cut from 18 s to 5 s.
  - Host registration reconnects in parallel with backoff.
  - The decoder and canvas are kept, so the picture continues from the next key frame.
  - Relay restart: ~0.9 s (before: 2.3–17.8 s).
- **Several relays** (`docs/relay-ha.md`):
  - Clients take a list of servers (`TETHER_SERVER` comma-separated, policy `servers.list`).
  - Hosts register with every relay.
  - Viewers race their requests across relays.
  - Each relay hands out the least-loaded healthy relay (`/readyz`).
  - Measured with two production relays, the viewer behind the proxy: the session's relay
    crashing costs 0.6–0.8 s; a whole relay node going down costs 0.6–1.6 s; the session
    continues through the other relay.
- **Second production relay** `tether-desk2.duckdns.org` (Oracle Always Free, same server key).
- **Graceful relay drain:** on stop, hbbr refuses new sessions but keeps running ones for up to
  `DRAIN_TIMEOUT` (300 s). hbbs and hbbr expose `/healthz` and `/readyz`.
- **Kubernetes:** Helm chart `deploy/helm/tether-relay` (per-pod relay addressing, disruption
  budgets, drain on stop). Tested on kind: rolling updates of hbbr and hbbs with 0 of 30
  sessions dropped; a force-deleted relay pod caused a 39 ms gap. See `docs/kubernetes.md`.
- **Tether Manage with several relays:**
  - Policy, releases, the download page and diagnostics tickets are published to every relay.
  - Each relay's update manifest points at that relay.
  - The next policy serial is the highest on any relay plus 1.
- **macOS:** `scripts/build-macos.sh` builds an unsigned universal `.dmg` and the updater
  archive for the technician app, plus TetherQS as a portable `.app`. Tether Manage publishes
  macOS releases. See `docs/macos.md`.
- **Download page:**
  - detects the visitor's OS and highlights the matching installers;
  - has a Windows/macOS switch that also works without JavaScript;
  - shows "Not available yet" for an OS without a release.
- Policy feature `video.codec` now takes effect (it was written where the viewer didn't read it).

### Known limits
- The macOS build is written but not compiled or run here (no Mac available). Ad-hoc signed
  builds may need Screen Recording and Accessibility to be granted again after each update.
- When a whole Kubernetes node freezes silently while a proxy pod runs on it, sessions can
  stall until Kubernetes marks the node NotReady (~40 s). Mitigation in `docs/kubernetes.md`.
- Installer: when Tether was closed for an interactive upgrade, it comes back via "Run Tether"
  on the finish page; automatic reopening is guaranteed only for the in-app updater (`/R`).

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

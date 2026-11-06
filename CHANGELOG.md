# Changelog

All notable changes to Tether. Versions follow [Semantic Versioning](https://semver.org/);
the desktop app version is kept in sync in `apps/desktop/package.json`,
`apps/desktop/src-tauri/Cargo.toml` and `apps/desktop/src-tauri/tauri.conf.json`.

## [Unreleased]

### Fixed
- **No more freezes in ~3 s steps when the screen content suddenly gets heavy** (seen on a Mac
  host with H.265: a switch to full-screen noise or photos froze the picture 4 times for ~2.7 s).
  The host's video loop now treats a frame the connection has not taken within 1 s as a full
  link and halves the bitrate at once (without adjustable bitrate: the frame rate), instead of
  waiting seconds for the delay probes to confirm it (docs/performance.md).

### Added
- Host log: one `fluxrt video/min` line per minute while video is sent (encode time avg / p95,
  fps, kbit/s, largest frame, key frames, frames not taken within 1 s / 3 s, QoS fps / ratio /
  target), so long sessions can be diagnosed from the host log. Replaces the 5 s `fluxrt encode`
  line.

## [0.5.16] — 2026-11-06

Windows and macOS. macOS: the server key rotation client (0.5.15 already had the AV1 change).

### Added
- **Server key rotation without reinstalling clients** ([docs/key-rotation.md](docs/key-rotation.md)).
  The admin key now also signs a **server key list** (`GET /api/server-keys`, cached as
  `server-keys.json`): Tether and TetherQS trust exactly its keys (without a list: the built-in
  `TETHER_PUB_KEY`), present its first key to hbbs/hbbr, and reject lists with a lower serial.
  hbbs and hbbr accept the keys in `KEYS_ACCEPT` besides their own (`/etc/tether/keys.env`, Helm
  `key.accept`), so relays can sign with a new key while devices still present the old one.
  Tether Manage › Keys › **Server key**: generate the new key (sealed in the vault), publish the
  list to every relay, count the devices that report it (heartbeat `server_keys_serial`), save the
  key files and the relay commands (`server/rotate-key.sh switch|retire|rollback`), retire the old
  key. Regression check: `apps/desktop/e2e/keyrot-e2e.mjs`.
- A viewer whose server key is not trusted, or not accepted, now says "Server key mismatch: …"
  instead of the generic "could not be end-to-end encrypted".

### Changed
- **Auto codec picks AV1 on more hosts without a GPU.** The one-time AV1 speed test allowed 90 ms
  per key frame and 30 ms per frame; a GPU-less 1080p host failed it and stayed on VP9 for good,
  although AV1 was smoother there in every stress phase (docs/performance.md). Limits are now
  400 ms / 50 ms and every host reruns the test once (`av1-test-2`). Measured on a GPU-less
  Windows host: scrolling p95 +475 ms -> +121 ms, no more freezes under full-screen changes.

### Upgrade order
1. Relays: new hbbs, hbbr and tether-api (the heartbeat accepts `server_keys_serial`).
2. Clients: this version on every device. Older versions cannot follow a rotation.
3. Only then rotate.

## [0.5.15] — 2026-11-06

### Fixed
- **Heavy screen changes stay within the bitrate.** Once the connection cannot keep up (quality
  lowered by the bitrate control), the VP8/VP9/AV1 encoders may now use their coarsest steps, so a
  photo or a full-screen change is coarse for a moment instead of producing multi-second frames.
  A VP8 session behind a TLS-inspecting proxy no longer drops into "Reconnecting" under heavy
  changes (docs/performance.md).
- **Mac hosts: smooth picture under heavy screen changes.** A Retina display is streamed at its
  logical size, which can be odd (1728x1117 on a 14" MacBook Pro). The viewer's hardware video
  decoders (WebCodecs) reject odd frame sizes, so every Mac session fell back to decoding in the
  app and shipping raw pictures (about 8 MB each) to the window. The Mac host now captures at even
  dimensions (1728x1116), so WebCodecs decodes Mac sessions again.
- **No more minute-long delay or viewer crash when the native decoder is used.** The fallback
  path sent every decoded picture to the window without waiting; under heavy changes the queue
  grew without bound (measured on a Mac: 67 s median delay, then the viewer quit). It now sends a
  picture only after the window drew the previous one (newest picture wins).

## [0.5.14] — 2026-11-05

Windows only (Tether and TetherQS); macOS stays on 0.5.13.

### Fixed
- **One connect could open two session windows to the same device** (Windows technician app,
  since the main window and `--connect`/`tether://` open sessions in the running Tether): a
  double-clicked Recent card, Enter pressed twice or a link opened twice each started its own
  session, and a window stuck on "Connecting…" could stay in front of the working one. A request
  for a device that already has a session window of that type (remote control or file transfer)
  now brings that window to the front. Regression check: `apps/desktop/e2e/connect-once-e2e.mjs`.

## [0.5.13] — 2026-11-05

macOS only (Tether and TetherQS); Windows stays on 0.5.12 (no Windows change).

### Fixed
- **A Mac could not be controlled by keyboard after a session started.** Since 0.5.12 the host
  minimizes its Tether windows when a session starts, but Tether stays the active app. The
  protection of Tether's own windows treated "Tether is the active app" as "a Tether window has
  the keyboard" and dropped every key from the technician (with the notice "The Tether window on
  the remote computer can only be used by the person at that computer."). Keys are now dropped
  only while the active Tether also has a window on screen.
- **Mouse and keyboard stopped reaching a Mac shortly after a session started.** macOS injects
  remote input on Tether's main thread; with every Tether window minimized (0.5.12) App Nap
  throttled that thread, so the first clicks worked and then nothing arrived. Tether now opts out
  of App Nap (`NSProcessInfo beginActivity`, user-initiated, latency-critical; idle system sleep
  stays allowed).

## [0.5.12] — 2026-11-05

### Fixed
- **Host windows during a session:** when someone connects, the main window now minimizes
  together with the connection window. On Windows the connection window has its own taskbar
  button, so a click on it restores and minimizes it (before, both windows shared one grouped
  button whose click only showed thumbnails, so neither could be minimized from the taskbar). On
  macOS a click on the Dock icon during a session toggles only the connection window (minimize
  when shown, restore when minimized, any number of times) instead of only ever restoring it.
  Chat, a new request and a recording request still bring the connection window back.
- **Tether Manage 0.5.4, Download page:** clicking inside the preview (the Windows / macOS switch or a
  Download button) no longer blanks it. Its relative links resolved against Tether Manage itself
  and navigated the preview frame away; they now do nothing in the preview. **Preview** always
  redraws the page, even when the texts did not change (before, it stayed blank until the page was
  left and reopened).

## [0.5.11] — 2026-11-05

### Changed
- **Main window:** the recordings list moved out of the main window into **Recordings**, opened
  with the new camera button next to Settings in the title bar (Settings-style dialog, same
  features: Play, Show in folder, Delete, Open folder, folder path, live updates). The main
  window no longer scrolls: This device and Remote device always sit side by side and only the
  Recent list scrolls.

### Fixed
- Session window: minimized from full screen (toolbar Full screen, then Minimize), it comes back in
  full screen when restored from the taskbar or Dock instead of as a normal window. A maximized
  window already came back maximized; an e2e (`e2e/session-maxrestore-e2e.mjs`) now checks both.

## [0.5.10] — 2026-11-05

### Fixed
- Host window: a second session (e.g. a file transfer next to the screen session) no longer
  brings the minimized window back on macOS. The published 0.5.9 builds were made before this fix.

## [0.5.9] — 2026-11-05

### Added
- **File transfer: select several files and folders at once.** Click, Ctrl/Cmd+click, Shift+click,
  Ctrl/Cmd+A and the arrow keys select in either pane; Upload/Download ("Download 3 items") and
  drag and drop send the whole selection, one job per item, two at a time with the rest queued,
  and the transfer list shows how many finished and the overall progress. Delete on the remote
  pane removes all selected items after one in-app confirmation (no browser dialog).

### Changed
- **The host window gets out of the way during a session** (TetherQS and Tether's connection
  window on the computer being supported): when a session starts it minimizes to the taskbar
  (Dock on macOS), never to the tray, so it can always be reopened; restored by hand it stays
  open. A new incoming request or a recording request restores it and brings it to the front.
- **Chat messages are always seen:** each message from the technician restores the host window,
  switches to the Chat tab, scrolls to the message and flashes the taskbar button (no focus
  change).
- **macOS: one Tether icon in the Dock.** The connection window on a controlled Mac no longer adds
  a second Tether icon while the main Tether runs; a click on the Tether Dock icon brings it back
  (also from minimized) and opens the main window if no window is shown. Details:
  [docs/macos.md](docs/macos.md#windows-and-the-dock).

### Fixed
- Tether's connection window kept showing Accept/Decline after the user accepted an incoming
  session; it now shows the active session.
- File transfer: a downloaded folder's file list no longer replaces the remote pane's listing
  while the download runs; a folder's progress bar uses the folder's total size.
- A cancelled transfer is shown as cancelled instead of staying "running".
- Deleting a folder on the remote device removes it with everything in it (before, nothing
  happened: the core only listed the folder's files). New command `fs_remove_empty_dirs`.

## [0.5.8] — 2026-11-05

Windows only (Tether and TetherQS); macOS stays on 0.5.7 until the next release.

### Added
- **Recordings are easy to find.** When a recording stops, a "Recording saved" card shows the file
  name with **Open** and **Show in folder**, and the upload status when recordings are uploaded.
  The main window lists all recordings (date, device, ID, length, size) with Play, Show in folder
  and Delete, the folder in use and **Open folder**.
- **Settings → Recording:** the recording folder with a folder picker (unless policy locks
  `recording.folder`), and the recording mode, upload target and retention set by policy.

### Changed
- Recording files are named `<date>_<time>_<device name>_<ID>.webm|mp4` instead of
  `outgoing_<ID>_<time>_display0_<codec>`. Older files are still listed, uploaded and pruned.
- TetherQS shows "Recording allowed" instead of "REC" once the user allowed recording.

## [0.5.7] — 2026-11-05

Re-release of 0.5.6 with the correct update-signing key; the code is the same as 0.5.6. The 0.5.6
files first published on the relays were built before these fixes and lacked:

- **The correct updater key.** They carried the key from before the 2026-11-04 key reset
  (`43DFAF08CF9D9DD2`; releases are signed with `1AD0C3FBDCA3B4EA`), so auto-update fails with
  "The signature was created with a different key". 0.5.5 and the published 0.5.6 for Windows
  cannot update themselves: install 0.5.7 once by hand (Tether and TetherQS alike); later updates
  install by themselves.
- **The Alt freeze fix** (a lone Alt from the technician no longer puts TetherQS into keyboard menu
  mode).
- **The WebRTC Disconnect fix** (Disconnect in TetherQS over P2P / Direct (LAN) no longer makes
  the technician reconnect).
- **The macOS window-guard fix** (clicks on Tether's own windows on a Mac host are blocked and the
  technician gets the notice).

## [0.5.6] — 2026-11-05

### Security
- **The technician can no longer use Tether's own windows on the remote computer.** Clicks, wheel
  and keys from a session never reach the host's connection window, TetherQS, the main Tether
  window or Tether Manage, so a technician cannot switch on their own permissions, approve a
  consent prompt (recording, admin access) or change settings. Checked on the host for every event
  right before it is injected (also in TetherQS's elevated helper): a press over a Tether window is
  dropped with its release; keys are dropped while a Tether window is in front, except modifiers,
  Alt+Tab, Win+Tab / Cmd+Tab, Ctrl+Alt+Del. Pointer moves still pass, local input is unaffected,
  always on. The technician gets a short notice. Details and limits: SECURITY.md.
- File transfer refuses to read or change Tether's own configuration (keys, password, permissions)
  and installation folder on the host.
- macOS host: same filter (tested on a Mac, see Fixed).

### Changed
- **Direct (LAN) on the same network, also behind a proxy and for TetherQS without admin
  rights.** A WebRTC session whose selected ICE pair runs between two private addresses
  (RFC 1918, link-local, IPv6 ULA) is now labelled **Direct (LAN)** instead of P2P; the viewer
  log names the pair (`WebRTC selected pair: ...`). Behind a proxy WebRTC is no longer switched
  off but runs LAN-only: no STUN/TURN server, only private host addresses, remote candidates on
  public addresses dropped, so nothing bypasses the proxy towards the internet and two PCs on one
  LAN still connect directly (both sides need this version; `network.direct = off` still turns it
  off). TetherQS without the `Tether QS` firewall rule gets its LAN path this way too: its own
  outbound ICE checks open the stateful firewall. The LAN lookup now also sends each adapter's
  subnet broadcast (Hyper-V / VPN adapters) and skips loopback.

### Fixed
- **macOS host: clicks on Tether's own windows are now actually blocked, and the technician gets
  the notice.** The Dock owns a transparent window over the whole screen (layer 20, above normal
  and floating windows), so the hit test always found the Dock and never a Tether window. The
  Dock's window is now skipped. The notice check also waits for the pointer moves queued before
  the click (macOS injects on the main queue), so a fast move-and-click is judged at its real
  position.
- **Updates work again: 0.5.5 builds carried an outdated update-signing key.** They were built
  with the key from before the key reset, so every update signed by Tether Manage failed with
  "The signature was created with a different key than the one provided". Install 0.5.6 once by
  hand; after that updates work again.
- **A lone Alt (or Alt+F4) from the technician no longer freezes a Tether window on the host.**
  Alt passes for Alt+Tab, but with F4 dropped Windows saw a lone Alt tap and put the window into
  keyboard menu mode, where its page stalled until the next key or click. The host now sends an
  unassigned key (VK 0xE8, as AutoHotkey does) right before such an Alt release, so menu mode never
  starts. Alt+Tab, Win and Ctrl+Alt+Del work as before; other windows still get a lone Alt.
- SCTP tests of the no-congestion-control sender failed on Windows: they tick the test bridge in
  1 ms sleeps, which Windows stretches to 15.6 ms, so T3-rtx fired before the SACKs arrived. The
  tests now request 1 ms timer resolution; the transport itself was not at fault.
- **Disconnect over WebRTC (P2P / Direct (LAN)) keeps the technician disconnected.** Clicking
  Disconnect in TetherQS ended a WebRTC session with "Reset by the peer" on the technician side,
  which then reconnected (and TetherQS docked its window again); the relay path was fine. The host
  closed the peer connection while the close reason was still queued in SCTP, so it never arrived.
  Closing now waits (at most 2 s) until the peer has acknowledged everything sent, and the
  receiving side hands over data that arrived before a stream reset instead of dropping it.
  The host's session cleanup also closed the peer connection directly, without that wait, and won
  the race whenever the close reason was not acknowledged within a few milliseconds (seen with both
  apps on one machine); it now closes through the same flush.

## [0.5.5] — 2026-11-05

### Added
- **Direct connections on the same network.** Two devices on one LAN (e.g. a Windows PC and a Mac
  at home) no longer send the picture through the relay in Amsterdam: the viewer looks for the
  host on the LAN while it asks the relay, and races a direct connection against the relay. The
  first path that completes the end-to-end key exchange wins, so nothing gets slower when direct
  fails; corporate PCs (proxy, outbound UDP blocked) keep using the relay with no added delay.
  The host must prove its relay-signed key on the direct path too, and applies the same password
  / Accept / permission checks. The session window shows **Direct (LAN)** or **Relay**.
  New policy `network.direct` (`auto`, default / `off`). The host listens (TCP 21118, UDP 21119,
  local subnet) only where that raises no firewall prompt: the Windows installer adds the rule
  `Tether`; TetherQS on Windows only with an admin-created rule `Tether QS`; macOS while the
  application firewall is off (the Mac asks once for Local Network access). Both devices need
  this version.
- **Direct connections across networks (IPv6, NAT hole punching).** With `network.direct = auto`
  and no proxy, the viewer also sends a WebRTC offer through the relay's WebSocket; ICE connects
  directly over global IPv6 or by punching through both NATs, badges **Direct (IPv6)** / **P2P**.
  A ready relay waits at most 0.4 s for it. STUN is answered by our own hbbs on UDP 21116 (no
  public STUN server, no new port). With a proxy nothing changes: no offer, relay at once.
  Needs the new hbbs on the relays (forwards the WebRTC offer/answer, `socket_addr_v6`, relays
  `IceCandidate`, answers STUN; deployed to both relays 2026-11-05); new clients against an old
  hbbs simply stay on the relay.
- **UDP relay (TURN) when no direct path forms.** hbbs now also runs a TURN server on its UDP
  port 21116 (no new port: one shared port, `docs/stun-turn.md`). With each WebRTC offer the host
  gets a 5-minute TURN credential from hbbs (TURN REST format; the secret is random per hbbs
  process, nothing is stored in clients) and offers a relay candidate. Such a session beats the
  WebSocket relay in the race (a ready relay waits up to 0.8 s more for it) and shows
  **Relay (UDP)**. Measured with 1-5 % loss per hop: frame latency p95 62-128 ms instead of
  95-1219 ms over TCP. No relaying to private, loopback, link-local, CGNAT or the relay's own
  addresses; allocation, peer, bandwidth and unauthenticated-request caps. With a proxy nothing
  changes. Needs the new hbbs (deployed to both relays 2026-11-05) and this version on both ends.
- **Faster loss recovery on WebRTC sessions** (P2P and UDP relay): the data channel's SCTP marks a
  chunk lost as soon as one later chunk is acknowledged (RACK, was three) and its retransmission
  timeout floor is 40 ms (was 100 ms); our copy of the fork's SCTP is `crates/webrtc-sctp`.

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
- **Chat: the customer never saw the technician's messages and could not answer.** The core
  delivered them, but TetherQS had no chat at all (the messages were dropped in its window), and
  the installed Tether's connection window only counted them in its Chat tab label. TetherQS now
  has a **Chat** tab on the session card: the first message opens it, later ones show an unread
  badge while Permissions is shown, a toast says a message arrived, and the taskbar button flashes
  (no focus change). The customer can reply. The installed Tether's connection window shows
  unread messages on its Chat tab and flashes its taskbar button instead of trying to take the
  focus.
- **Technician: replies opened the chat panel over the remote screen.** They now show as an unread
  badge on the toolbar's Chat button, and the toolbar stays visible while there are unread
  messages.
- **The password screen came back within a running session.** The one-time password changes as
  soon as it has let the technician in, but a reconnect after a network drop (of a session
  accepted by click, or after more than 30 s), a reloaded session or file-transfer window (F5)
  and a file transfer opened from Home during a session logged in afresh and asked again for a
  password the technician no longer had. The host now lets the same technician window back in
  (same session key and companion secret) while the session runs and for 2 minutes after its
  connection ended; Disconnect on the host ends that at once. A reloaded window continues its own
  session, a file-transfer window joins the open session to that computer. Needs the new version
  on both sides (host: TetherQS / Tether).
- **"Wrong password" for a password nobody typed.** "Remember password" kept a TetherQS one-time
  password, which no longer works for the next session. A remembered password the host turns down
  is now forgotten and the screen asks for the current one, saying why.
- **The password screen says why** when it appears after the session had already been running
  (interrupted too long, or ended by the person there), and the file-transfer and reconnecting
  session windows wait 2.5 s for the host to let them in before showing it (no flash of
  "Password required").
- **Sessions over a slow relay path reconnected every few seconds (seen with a Retina Mac,
  also right after switching apps there).** A Retina key frame is ~200–260 KB; at the ~50 KB/s
  the relay path gave that evening it takes 4–5 s to arrive, and the viewer's 3 s "relay silent"
  watchdog counted only whole messages, so it dropped the connection mid-frame and the new
  connection started with another key frame. The watchdog now counts bytes on the WebSocket, so
  only a path that delivers nothing at all is treated as dead. Viewer-side fix (no host build).
- **Mac host: the picture lagged on slow paths.** Retina displays are now streamed at their
  logical size (1728×1117 instead of 3456×2234, ~4× smaller frames); host option
  `tether-full-retina` = `Y` keeps the full size. Under congestion a Mac host can lower its
  bitrate to ~0.3 Mbit/s instead of stopping at ~1 Mbit/s. Needs the new Mac build.
- **Flickering bands across Edge pages and videos on GPU-less hosts (VMs).** On a machine
  without a GPU (Hyper-V VM, display-only adapter) Windows' Desktop Duplication hands out frames
  in which a window that is redrawing is only half drawn: white/grey full-width bands across the
  Edge welcome page or a video, in up to half the frames. TetherQS and Tether now capture such
  hosts with Windows.Graphics.Capture (what the screen really shows); GPU hosts are unchanged.
  Same latency, bitrate and VM CPU on the bench VM (docs/performance.md).
  `TETHER_CAPTURE_DXGI=1` keeps the old capture.

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

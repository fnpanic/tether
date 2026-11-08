# Changelog

All notable changes to Tether. Versions follow [Semantic Versioning](https://semver.org/);
the desktop app version is kept in sync in `apps/desktop/package.json`,
`apps/desktop/src-tauri/Cargo.toml` and `apps/desktop/src-tauri/tauri.conf.json`.

## [Unreleased]

### Fixed
- **Tether Manage: no view or small write waits on a relay that never answers** (round 3 of the
  blind review; "all relay reads" in round 2 was not true). Opening a diagnostics upload asks only
  the relay that has it (or the first that sends it) and gives up when it does not start within
  10 s (60 s for the whole zip; before: every relay, 30 min). Create ticket, Close/Delete, policy,
  team, server keys, download page, release index and clean-up writes must start answering within
  10 s and finish within 30 s, and go to all relays at once with one result per relay (before:
  one relay after the other, 30 min each). The Keys page, the download page (preview, publish and
  "Then update the download page"; it read only the first relay), Diagnostics, Team, Custom apps,
  Bring relays up to date and the clean-up review read all relays at once with the 10 s limit;
  the macOS disk image facts for a custom download page come from the first relay that sends the
  file within 180 s. Only release file uploads and copies keep the 30 min limit. Measured with one
  silent relay: every call about 10 s (`apps/desktop/e2e/relay-timing.mjs`, 19 calls).
- **Diagnostics: "could not reach the support server" instead of "not valid"** when the relay
  that has the ticket is down and the other relay does not know the code (technician app and
  TetherQS; a `5xx` other than `503` counts as unreachable).
- **Tether Manage: both uploads of a code are listed**: each relay takes one upload per code, so a
  code used on one relay could be uploaded once more on the other and that upload stayed hidden.
  The ticket now lists every upload with its own Open button; Close and Delete act on all of them.
- **Tether Manage: Diagnostics table fits from 900 px** (Delete was past the card edge at 900×600
  and 911×512): note under the code, closed date under created, icon buttons with labels.
  An earlier error disappears after the next good load, and switching tabs quickly no longer shows
  the other tab's list. The policy bar marks the live serial as from an earlier check when a later
  check failed (it kept showing "Live: serial N" as if current).
- **Tether: "Send to support" and "Remove from recents" labels no longer cut** (Settings ›
  Diagnostics: the code field took the whole row and the label wrapped out of the button; the
  Recent "More" menu item wrapped onto a second line).
- **Nothing cut off at small window sizes** (all windows of Tether, TetherQS and Tether Manage
  checked at their default and minimum sizes, 1280×720 and 1366×768 at 125 % and 150 %, light and
  dark; [docs/ui-audit.md](docs/ui-audit.md), `tools/ui-audit.mjs`). A window larger than its
  screen shrinks to the work area and re-centres, so its title bar is no longer above the screen;
  the connection window is never placed above the top edge. TetherQS Help and the idle view
  scroll. Advanced: host list drops columns that do not fit, detail panel scrolls, selection bar
  wraps, status bar stays on one line, keyboard help scrolls. Session: toolbar fits a 640 px window,
  chat stays below the toolbar. File transfer and Manage tables: long host names and IDs no longer
  wrap or squeeze.
- **Tether Manage: dialogs fit the window**: every sheet is at most the window height; its title
  and buttons stay visible and the content between them scrolls (the publish sheet with a custom
  ramp, many relays and the upload progress went past the bottom at 1366 × 768 with 125-150 %
  scaling).
- **Tether Manage: "Live: checking…" no longer waits on one slow relay**: the live policy and the
  relay health checks ask every relay at once with a 10 s limit per request (before, one relay at
  a time with the 30 min upload timeout, so a relay that accepted the connection but did not
  answer kept "checking…" up for minutes); a failed check shows "Live: could not check (…)"
  instead of staying on "checking…".
- **Tether Manage: tables fit the window** (900 px minimum and up, no sideways scrolling that cut
  through buttons): platform and channel sit under the app, version and OS under the profile, IP
  under the host or last-seen time; Rollouts stacks Halt and Everyone now; "Publish to missing"
  (full text in its tooltip); a missing release reads "Missing" under its relay's column.
- **Tether Manage: one relay that never answers no longer stalls Rollouts, Devices, Sessions,
  Releases or a release-index publish**: every relay read asks all relays at once with the 10 s
  limit (uploads keep the long one), the publish skips a relay that did not answer, and Rollouts
  names the relay's error.
- **Tether Manage: custom ramp checks match the signer**: hours and percent are plain decimal
  numbers in the form and in Tether Manage's backend (`1e2`, `0x10`, `100.0` were accepted by one
  and refused by the other); with Security release on, an unfinished Custom ramp no longer blocks
  publishing (the ramp is not used).
- **TetherQS: the permission switches are always visible before Accept**: with the update and
  macOS banners on a small screen the list shrank to nothing; the card keeps its height and the
  window scrolls.
- **Tether: the main window fits short screens on every switch**: Simple and Advanced view size
  their minimum against the screen's work area (without the taskbar) every time, never larger
  than it (1366 × 768 at 150 % pushed the window past the screen after switching back to Simple);
  a full-screen or maximized session window is no longer resized at start.
- **Full names on hover** wherever a name is cut with "…" (technician in TetherQS and the
  connection window, hosts in Advanced, recent devices, file names); the Advanced search hint is
  shorter so it fits at 911 px (full hint in its tooltip). Session keyboard menu scrolls in a
  short window.

### Added
- **`--diag-upload <code>`** for `tether` and `TetherQS`: Send to support from the command line
  ([cli.md](docs/cli.md)).
- **Tether Manage: rollout for "Publish the latest builds"**: its review sheet now has the
  Rollout (Standard / Fast / Immediately / Custom) and Security release controls of the Sign and
  publish form, so the one-click publish no longer always uses the Standard ramp.
- **Tether Manage: custom rollout ramp**: the share of devices at once, then the hours until 10 %,
  50 % and 100 % (validated: rising, up to 30 days), signed into the release index like the
  presets; clients follow it unchanged. The last rollout choice is remembered.
- **Diagnostics could not be sent with two relays.** The apps sent the upload only to the relay
  they had used last (after an update check: the last one of the list) and stopped at its `404`,
  and Tether Manage listed the tickets of its first relay only: the upload went to relay 2 and
  stayed "Waiting", or a code that one relay did not know was "not valid". The technician app and
  TetherQS now try every relay in turn (on to the next after `404` or `503`), and Manage merges the
  ticket lists of all relays and shows where the upload is.
- **Relays: an upload cut short is no longer stored.** A client that went away before its
  `Content-Length` arrived left a truncated diagnostics zip and a used-up ticket; now `400` and the
  ticket stays open (release uploads were already safe: their signature covers the whole body).
  A used, expired or closed ticket answers `404 ticket not open` (unknown: `404 not found`), so
  the apps stop there instead of uploading a second time to another relay.
- **TetherQS: Send to support** in the Help view (Windows and macOS, policy `diagnostics.upload`):
  the ticket code from Tether Manage uploads TetherQS's diagnostics (its logs of the last 24 h and
  its config with passwords, keys and tokens redacted; same zip and 2 MiB limit as the technician
  app, shared code). `TetherQS --diagnostics <out.zip>` writes the same archive.
- **Diagnostics tickets: close, archive, delete** (tether-api `POST /api/admin/diag/<ticket>/close`,
  `DELETE /api/admin/diag/<ticket>`, `GET /api/admin/diag?archived=1`, a `state` per ticket:
  open, uploaded, expired, closed). Tether Manage: **Close** (no more uploads, kept 90 days with
  the upload under **Archived**) and **Delete** (after a question, for good) on every relay, with a
  result per relay.
- Upload messages for expired and closed codes, and "server busy" when every relay is at its
  upload limit.

## [0.5.26] — 2026-11-08
Tether / TetherQS 0.5.26 (Windows and macOS), Tether Manage 0.5.9 (built with TETHER_ADMIN_PUB_KEY); relays: tether-api and hbbs (deployed 2026-11-08). Audit fixes (client and relay) and the shortcut ledger.

### Fixed
- **Sessions started together no longer slow each other down**: the extra 800 ms a session waits for
  WebRTC over TURN before taking the relay now depends on that session's own peer offering a TURN
  candidate, not on any session in the process (Advanced mode opening several at once, or screen
  and file transfer together).
- **macOS: `tether --connect <id>` while Tether runs** opens the session in the running Tether
  (one Dock icon, one session list), as on Windows; only a single-instance socket owned by the same
  user gets the request.
- **Relays: `process_cpu_seconds_total` and `process_start_time_seconds`** use the system's clock
  tick rate (`sysconf(_SC_CLK_TCK)`) instead of assuming 100 (hbbs/hbbr).
- **TetherQS (macOS custom apps)**: a profile that cannot be cached is logged instead of ignored.
- **Mac build runner**: after a restart it kills a lost job's process group only if the group
  leader is still that job's process (start time), not a process that reused the pid.
- Shortcut ledger reviewed (PONYTAIL-DEBT.md): 5 fixed, 2 markers removed, 4 kept with triggers.
- **Tether Manage: a relay only gets a release index whose files it serves** (devices there would
  have fetched files the relay does not have, HTTP 404). Before publish, Halt, Resume, ramp and
  re-sign, Manage checks every artifact and gzip copy of the new index on each relay; one whose
  upload failed or that misses an earlier release keeps its old index and is reported with the
  missing files ("use Publish to missing relays").
- **Tether Manage: release index after an admin key rotation.** Manage verifies the relays' indexes
  with its own key and the `TETHER_ADMIN_PUB_KEY` list it is built with, so the first publish with a
  new key keeps the other releases and the serial stays above the devices' floor. An index no
  trusted key verifies stops the publish with an explanation instead of being replaced.
- **Pre-release versions compare like semver** (`0.5.0-test.9 < 0.5.0-test.10 < 0.5.0`) in the
  apps and in Tether Manage (Releases list, Devices version filter use the same order).
- **Staged update downloads are cleaned up**: every update check deletes `update-*.staged` files
  other than the current candidate's (the Windows installer ends the app before it could). TetherQS
  writes the staged file atomically.
- **Download page and halted releases**: custom templates leave halted releases out; the Standard
  page refuses to build while its version is halted; after Halt on `stable` Manage reminds you to
  publish the download page again.
- **TetherQS with `update.enabled = N`** still reads the release index: a release minimum version
  blocks sessions as in the technician app, and a stale "update required" no longer blocks after
  updates were turned off.
- **Packed downloads decoded by a proxy**: the size limit for the gzip URL is the larger of the
  packed and the plain size, so a body already decoded on the way is not cut off.
- **Relays: tether-api no longer runs out of memory at 50 000 devices.** `devices.json` is written
  straight from the device map and the device lists (admin, team) through a temp file: 40 MB peak
  instead of 85 MB with 16 team lists at once (MemoryMax 64M, [docs/scaling.md](docs/scaling.md)).
  Custom-app profiles are read from `custom/<id>.json` when needed instead of kept in memory.
- **Relays: a failed release-index rename keeps the old serial** in tether-api (it was updated
  before the file was in place).
- **Relays: `install.sh` keeps `RELAY_SERVERS`** from `/etc/tether/server.env` on a re-run (it
  fell back to this host alone, dropping the other relays of an HA setup).
- **Kubernetes: per-IP limits per client again.** The chart's proxy sent `/api/*` to tether-api
  directly, which trusts `X-Forwarded-For` only from loopback, so every client counted as the proxy
  pod; `/api/*` now goes through the api pod's Caddy ([docs/kubernetes.md](docs/kubernetes.md)).
- **Server CI** builds and tests what the deploy bundle contains (`scripts/build-relay.sh`,
  `scripts/test-relay.sh`, tether-trust) and packages tether-api and the Caddy, nginx and sysctl
  files; it also runs on changes to `crates/`, `patches/`, `vendor/`.

### Security
- **Technician app: `session_policy_get` returns only the values the session window needs**
  (`connect.reconnect-minutes`, `recording.mode`); the recording upload token is no longer readable
  from the webview.
- **TetherQS: custom app profile fetch** reads at most the profile size limit instead of the whole
  answer first.
- **Relays: one request could stop tether-api.** tiny_http allocated the whole declared
  `Content-Length` to discard a body tether-api refused, so a `Content-Length` larger than the
  machine's memory aborted the process (also through Caddy, which forwards the header). tiny_http
  is now patched (`server/api/patched/tiny_http`): at most 1 MiB is discarded, and a refused body
  is not read at all (read side shut, `Connection: close`).
- **Relays: slow uploads can no longer take all of tether-api's workers.** Release and diagnostics
  uploads (streamed to disk, up to 200 MiB) run at most 4 at a time (more: `503`, `Retry-After`),
  are refused before their body is read when they cannot succeed (no admin keys, no `Tether-Sig`
  or a timestamp outside ±300 s, bad path, no open ticket), and any body averaging under 16 KiB/s
  after 30 s is cut off (`408`). Before, 32 uploads sending a byte every 25 s held every worker for
  about 55 minutes and every heartbeat got `503`; now heartbeats keep 28 workers (measured, 83 of
  83 answered). The release signature check (over up to 200 MiB), audit log queries and preparing
  a `/q/` copy no longer run under the state lock.
- **Relays: hbbs bounds what it queues for a host.** Messages for a host's WebSocket/TCP signalling
  path were queued without limit and written without a timeout, so a host that stopped reading
  plus a flood of requests for it grew hbbs's memory without bound. Now 64 per connection (more are
  dropped) and a 5 s write timeout that closes the connection.
- **Relays (nginx): request body limits.** `client_max_body_size` was unlimited for `/api/`
  (bodies buffered to disk); now 1 MiB, 201 MiB for `/api/admin/release/` and 3 MiB for
  `/api/diag/`, both unbuffered so tether-api's upload limits apply.

## [0.5.25] — 2026-11-07
Tether / TetherQS 0.5.25 (Windows and macOS), Tether Manage 0.5.8; relays: tether-api and Caddy/nginx config (deployed 2026-11-07). The signed release index is required from this release on (`REQUIRE_SIGNED_INDEX`).

### Added
- **Tether Manage: delete download page templates** you no longer need (bin next to the Template list,
  asks first). The template the published Custom page is built from cannot be deleted.
- **Tether Manage: custom download page templates** ([docs/download-page.md](docs/download-page.md)).
  Download page → **Design: Standard (built-in) | Custom HTML**. Standard stays the default and is
  byte-identical to before. Custom HTML fills your own HTML file at publish time: `{{tether.<app>.<os>.url|cdn_url|file|version|size|bytes|sha256|date|notes|minimum_version}}`,
  `{{tether.title|intro|relay|published}}`, custom-app links, and `{{#…}}` / `{{^…}}` sections for
  "not published". The release data comes from the signed `releases.json`. Values are HTML-escaped;
  unknown placeholders, unsafe positions (script, event handlers, non-URL values in `href`),
  `<base>`, external resources (unless allowed) and pages over 512 KiB are errors before publishing.
  Every chosen template version is kept in the data dir, with a sandboxed preview, a placeholder
  reference with copy buttons, and a downloadable starter template
  ([docs/examples/download-template.html](docs/examples/download-template.html)). A stable release
  publish rebuilds the page with the chosen design. No relay change.
- **Clean up old releases** (docs/release-cleanup.md): tether-api `POST /api/admin/releases/prune`
  (signed; dry run returns the plan, execute deletes only the listed versions the relay itself
  would still drop, recomputed under its lock; idempotent). Kept per channel/app/platform: the
  version in the signed index (halted or not) and in `latest.json`, newer ones, `keep` older ones
  (default 2, 0-20), versions on the download page, pinned versions, files from the last 24 h;
  only whole versions go, `latest.json`, `releases.json`, `index.html` and unknown files never.
  Log `release-prune.jsonl` and a journal line per run; `last_prune` in `/api/admin/status`;
  metrics `tether_api_release_bytes`, `tether_api_release_versions`,
  `tether_api_release_prune_*`, and two panels in the API dashboard's Releases row.
- Tether Manage › Releases: **Clean up old releases…** sheet (the relays' plans merged: a version
  is offered only if every relay would delete it; every CDN version is pinned), the "Keep N older
  versions" setting and the **Clean up after each publish** switch (off by default; the publish
  results get a Cleanup line).

### Changed
- tether-api refuses (`409`) to `DELETE` the published release's files, any `latest.json` and
  `releases.json` unless `?force=1`; Tether Manage asks a second time before forcing. Forced
  deletes are logged.

### Fixed
- Tether Manage › Releases listed `TetherQS-<v>.exe.gz` as a separate row labelled "latest.json"
  without a delete button; it is now part of its release.
- **Custom apps for macOS**: the private link `/q/<token>` serves a Mac browser the TetherQS zip
  with the `.app` folder renamed to `<Name>-Support [<mac id>].app` (the folder name is not part of
  the code signature: codesign and the Gatekeeper verdict unchanged, checked on the Mac); everything
  else gets the exe as before, `/q/<token>/mac` and `/windows` choose. TetherQS on macOS reads the
  mac id (SHA-256 of the token, 16 hex; never the token) from its folder name, fetches the signed
  profile from the new `/q/m/<mac id>`, verifies it with the admin keys, caches it and applies it
  like the Windows trailer; survives self-updates. `--print-custom-profile` diagnostic. Relays:
  tether-api, Caddyfile / nginx.conf / Helm (Content-Type of the prepared file from tether-api).
  Tether Manage: text only.

## [0.5.24] — 2026-11-07
Tether / TetherQS 0.5.24 (Windows and macOS), Tether Manage 0.5.7; relays: tether-api, hbbs, Caddy/nginx config (deployed 2026-11-07).

### Security
- **Update binaries bound to their signed version (W9 rollback)**: both apps now check the minisign
  trusted comment (`file:` and `version:` must be the offered ones). Before, neither did, so a relay
  could offer an older signed build under a higher version. The technician app installs exactly the
  bytes it verified (no second manifest fetch: the version checked is the version installed).
- **Signed release index (W9)**: `download/<channel>/releases.json`, signed with the admin key like
  the policy, names version, file, size, sha256, signature, URLs, flags and rollout. Clients verify
  every download against it (so relays and the CDN are untrusted), keep a serial floor per channel,
  and once they accepted an index never use the unsigned `latest.json` again (the next release
  requires it: `REQUIRE_SIGNED_INDEX`). Expiry after 30 days only warns.

### Added
- **Updates in about a minute**: tether-api counts a release generation (every index or manifest
  change) and answers heartbeats with `200 {"releases": G}`; hbbs pushes `ReleaseHint` (new
  rendezvous message, tag 1001, ignored by old clients) to every host on the WebSocket and once
  after registration; clients check after a random 0-60 s. Fallback polls are jittered.
- **Staged rollouts**: signed ramp per release (default 1 % → 10 % → 50 % → 100 % over 24 h,
  Fast, Immediately), stable bucket per device and release, policy `update.rollout`
  (early/default/late), security releases skip the ramp, **Halt / Resume** in Tether Manage ›
  Releases › Rollouts (devices re-check right before installing).
- **Forced updates never interrupt a session** (both apps): download in the background, install
  when no session or file transfer runs ("Will update after this session").
- **Download CDN** (optional): Tether Manage deploys the update files as Cloudflare Workers Static
  Assets (`tether-dl.thomas-schend.workers.dev`, free plan) on every publish; clients fall back to
  the relays. TetherQS.exe ships as a gzip copy (35 → 15 MB) on the CDN and from the relays
  (Caddy `precompressed`, nginx `gzip_static`).
- Settings › About and Diagnostics (technician app), Help (TetherQS): update state, rollout
  progress, "Update information is out of date".
- Metrics: `tether_api_release_generation`, `…_release_index_serial`/`…_expires_timestamp`,
  `…_devices_by_version`, `…_devices_update_state`, `…_release_downloads_total`, endpoint
  `release_index`; `hbbs_release_generation`, `hbbs_release_hints_sent_total`,
  `hbbs_release_hint_peers_notified`. Grafana API dashboard row "Releases"; alerts
  `ReleaseGenerationMismatch`, `ReleaseIndexMismatch`, `ReleaseIndexExpiringSoon`,
  `UpdateFailuresHigh`, `UpdateStale`.

### Changed
- TetherQS takes the highest version over all relays (was: the first relay with a release) and
  checks after policy changes. Both apps send `If-None-Match` per relay.
- Heartbeat: optional `release_serial`, `update_state`, `update_version` (sent only to relays that
  answer with a release generation).
- Relays: hbbs needs read access to `/var/lib/tether-api/release-generation` (group `tether-api`,
  `server/install.sh`).
- `scripts/make-patches.sh` includes `vendor/rustdesk-server/src/metrics.rs` (it was missing from
  `patches/rustdesk-server.patch`).

### Fixed
- **Unattended access: *Install as service* works on installed Tether (Windows)**. The installer wrote
  `InstallLocation` with quotes, so installing the service failed ("Installer path or name contains
  characters unsafe for cmd.exe", in a box titled "RustDesk Output") and `tether.exe --install-service`
  failed with exit code 0. The value is now unquoted (existing installs get it with the next update), the
  button only creates and starts the service (never the core's full self-install) and shows errors in
  Settings, the row is shown while the service is not running, `--install-service` exits 1 on failure.
- **The service honours the machine proxy**: as LocalSystem it has no IE proxy settings; it now also uses
  the WinHTTP proxy (`netsh winhttp set proxy`, GPO), so it is reachable behind a proxy-only network, at the
  logon screen too. Apps in the user's session are unchanged.
- **Settings made before installing the service are kept**: the permanent password, accepted passwords and
  2FA set before *Install as service* were not copied to the service (the core only imported a config older
  than the exe).
- **The viewer reconnects after *Restart remote computer*** instead of staying at "Restarting remote device".

## [0.5.23] — 2026-11-07
Tether / TetherQS 0.5.23 (Windows and macOS); hbbs and hbbr for the WebSocket bind (W5).

### Security
- **Key exchange version 0 refused outside WebRTC** (Tether and TetherQS): version 0 runs one key
  and one nonce sequence in both directions (keystream and Poly1305 key reuse, reflected frames).
  A Tether host now refuses a viewer that picks 0, and the Tether viewer refuses a host that offers
  nothing else ("This device uses an outdated, insecure encryption; update it"), so no shield is
  shown for such a session. WebRTC (DTLS, version 0 by design) is unchanged. Tether ↔ Tether is
  unaffected (both sides offer and pick 1); stock RustDesk viewers and hosts older than 1.5.1
  (no key exchange versions) are refused.
- **Legacy (non-multiplexed) port forwarding and RDP tunnels refused**: they call `set_raw()` after
  login, which removes the end-to-end cipher. A Tether host refuses them at login and the Tether
  viewer only uses the multiplexed, encrypted tunnel; stock RustDesk viewers' raw tunnels are
  refused.
- **Trust anchor locked**: while policy `settings.server` is off (the default), `key`,
  `custom-rendezvous-server` and `relay-server` are blanked in the core, so a value stored earlier or
  written into the config file is no longer used. The viewer refuses peers given as an address
  (`1.2.3.4`, `host:port`) or as `id@server` (except `id@public`), which bypassed the configured
  servers and their signed server key list, also from `tether://` links and the command line.
- **No plain `ws://` to a server**: rendezvous and relay WebSockets must be `wss://`; an IP endpoint
  (which became plain `ws://`) is refused. Loopback (`127.0.0.1`, `::1`, `localhost`) stays allowed
  for local test servers.
- **hbbs/hbbr WebSocket ports (21118/21119) listen on 127.0.0.1** (`WS_BIND`, default
  `127.0.0.1`): they take the client IP from `X-Forwarded-For`/`X-Real-IP`, which only the local
  Caddy/nginx may set. The Helm chart sets `WS_BIND=0.0.0.0` (its proxy is a separate pod). TCP/UDP
  21115-21117 are unchanged.

### Documentation

- New [docs/encryption.md](docs/encryption.md): how a connection is encrypted end to end, derived
  from the code with file:line references (identities and key storage, registration, rendezvous,
  key agreement and per-direction keys, transport paths, password challenge, who sees what), and
  what hbbs/hbbr can and cannot do. States plainly that hbbs holding the server key can
  impersonate hosts (no host-key pinning, no viewer identity key).
- SECURITY.md: a threat model (assets, trust boundaries, actors with mitigations and
  residual risks). Corrected: the rendezvous WebSocket is authenticated by TLS alone (the server
  key authenticates host keys, not hbbs); WebRTC sessions are protected by DTLS only, not by
  secretbox inside DTLS; TLS 1.2 or 1.3 (rustls defaults), not 1.3 only; `-k` compares a public
  key (configuration check, not authentication); "secrets at rest" are obfuscated with the
  machine uid, the device secret key is unencrypted; no app shows a device its own fingerprint;
  connecting by IP address skips the encryption check; hbbs/hbbr bind all interfaces and the
  installer opens 21115-21117/tcp and 21116/udp; what the relay stores; the update manifest is
  unsigned (rollback to an older signed build, blocking via `minimum_version`).
- README, user guide FAQ and architecture page: security wording matched to the code, with a
  pointer to docs/encryption.md.
- After an independent review: a "Known weaknesses" list (W1-W13, one status line each) in
  docs/encryption.md and SECURITY.md. Added or corrected: key exchange version 0 is reachable with
  non-Tether peers and broken (same key and nonces both ways); legacy port-forward tunnels run
  without the inner encryption; the exact trusted-key rule (`server_keys_for`) and that `key` /
  custom server are local options; plain `ws://` for IP endpoints; hbbs/hbbr WebSocket ports bound
  to all interfaces with client-chosen IP headers when reached directly; password-less ways in
  (resume ticket, file-transfer companion, trusted-device token, recent session); recording consent
  binds honest viewers only; offline guessing from a captured challenge response; keystroke timing;
  LAN lookup reveals the target ID; macOS direct listener not subnet-limited; machine uuid in hbbs
  logs; hbbr may create the server key; native-tls TLS versions; the h1 citation.

## [0.5.22] — 2026-11-07

Tether / TetherQS 0.5.22 (Windows and macOS), Tether Manage 0.5.6; relay parts as listed (hbbs, hbbr,
tether-api, Caddyfile, sysctl, monitoring).

Relays: scaling to 10 000 devices and 3 000 concurrent sessions ([docs/scaling.md](docs/scaling.md):
capacity model, load tests, architecture, cost, rollout). Server-side only; clients and wire
formats are unchanged and old clients keep working.

Relays: **nginx as an alternative HTTPS front end** to Caddy, after benchmarking Caddy (2.10/2.11,
tuned), nginx, HAProxy, Envoy and Traefik for Tether's load ([docs/proxy-benchmark.md](docs/proxy-benchmark.md)).
Caddy stays the default; `PROXY=nginx` switches a node.

### Added
- **`PROXY=nginx` in `server/install.sh`**: nginx mainline and its official ACME module
  (Let's Encrypt via TLS-ALPN-01 on 443, like Caddy) from nginx.org's signed repository (dnf or
  apt, patched by the OS's updates), `server/nginx/nginx.conf` with every Caddyfile route
  (`/ws/id`, `/ws/relay`, `/api/` with buffered bodies, `/download/`, `/q/` via
  `X-Accel-Redirect`, `/readyz/relay`, `/rendezvous/pubkey`, HSTS, HTTP→HTTPS), the same
  `tether-https.service` name with a graceful `reload` that keeps WebSockets, SELinux labels and
  booleans for the confined `httpd_t`, nginx-prometheus-exporter on 127.0.0.1:21125 (where
  Caddy's metrics are). Re-running with `PROXY=caddy` switches back. Per idle TLS WebSocket nginx
  needs 41 KB instead of Caddy's 111 KB (2.7×) and ~45 % less CPU; relaying costs about the same.
- Download budget in the nginx front: `/download/` and `/q/` share 32 parallel downloads × 512 KB/s
  per node (the manifest `latest.json` is exempt), so a release day cannot starve relayed sessions.
- Monitoring for nginx fronts: `deploy/monitoring/nginx/` snippets (`/_metrics/<job>` by source
  IP + bcrypt, `/grafana/` behind basic auth), installed by `deploy/monitoring/install.sh`
  according to the node's `PROXY=`; Prometheus job `nginx` (`REMOTE_PROXY=nginx` for the other
  node), nginx panels on the API dashboard, `TargetDown` covers it.
- Headroom alerts (`tether-headroom` group): egress, CPU and memory above 40 % (what still fits
  under 65 % after losing the largest location), a front above 15 000 WebSockets, listen-queue
  overflows; promtool tests.
- `tools/proxy-bench/`: front-end benchmark (`run.sh`, `summary.sh`, configs for every candidate,
  HAProxy as the uniform client-side TLS layer) and `e2e-nginx.sh`, the end-to-end test of the
  nginx front in WSL (ACME against Pebble, every route, WebSockets and relay-harness through it,
  graceful reloads, uploads, `/q/`, basic auth, metrics restriction). `tools/load/stack.sh`
  takes `FRONT=`.

### Changed
- docs/scaling.md: target architecture revised for failures with headroom: 8 × AX41 over FSN1,
  NBG1 and HEL1 (€472/month) carry the pessimistic peak at 53 % of port and CPU with a whole
  location down, with the step-by-step path from today's two nodes by device count. Relay CPU
  re-measured at 1 Gbit/s per node: 3.2–3.5 PC cores per relayed Gbit/s (was 4.4, from one
  0.5 Gbit/s run).
- Monitoring basic-auth hashes are bcrypt cost 5 (Caddy and nginx accept them; nginx checks them
  in the worker that carries the WebSockets). Existing hashes keep working.
- `ip_local_reserved_ports` now 21114-21126 (nginx `stub_status`).

### Fixed
- `tools/load/stack.sh` moved the bench Caddy's metrics port too (it took the production 21125).

### Added
- **Mac build runner** ([docs/mac-build-runner.md](docs/mac-build-runner.md)): builds on the Mac
  start from the PC through Tether file transfer alone, also while the Mac's screen is locked, for
  several projects at once. A user LaunchAgent (`tools/macbuild/mac/runner.py`, no sudo) runs only
  jobs signed with a registered sender's ed25519 key (`ssh-keygen -Y`, per-user and admin
  namespaces), refuses replays and expired jobs, and runs one job at a time, fairly across
  senders, with a log, status file, timeout, cancel, disk quota, retention, `caffeinate` and a
  per-job `sandbox-exec` profile that keeps senders out of each other's folders. Secrets (such as
  the signing p12) travel per job and are deleted when it ends. A step that must wait for an
  unlocked screen (replacing the running app) is deferred until the session is unlocked. PC client
  `tools/macbuild/macbuild.py` (keygen, register, submit, wait, log, fetch, cancel,
  update-runner); Tether's own Mac build as a job: `tools/macbuild/jobs/tether/submit.sh`
  (temporary keychain, designated-requirement check, dmg without the Finder AppleScript, app swap
  after unlock with rollback). Unit tests: `tools/macbuild/test_macbuild.py`.
- **Advanced mode** in the technician app for people with hundreds of machines
  ([docs/advanced.md](docs/advanced.md)): the home view becomes the host inventory: one list of recent, hand-added, imported and (with Team)
  organisation devices; virtualised, search under 2 ms per keystroke on 5,000 hosts; fuzzy search
  with `#tag`, field filters (`os:`, `ver:<`, `seen:<7d`, `is:online` …) and AND / OR / NOT;
  namespaced, coloured tags with bulk edit; tag rules; saved searches; pins, names and private notes;
  Ctrl+K command palette and full keyboard control (Enter connect, Shift+Enter files, Alt+Enter view
  only); open-sessions panel (Ctrl+Alt+Del, lock, restart); per-host session history; JSON/CSV
  import and export (notes left out unless chosen). Switched with **Hosts** on the home view or in
  Settings; loaded on demand, so the simple view starts as before. New policy feature `ui.advanced`
  (`off`, `available` (default), `forced`).
- **View only** sessions from Advanced mode (and `--view-only`, which was passed but ignored): the
  host refuses input for that session; the device's own view-only setting is restored afterwards.
- **Online status** for many IDs at once (batched hbbs online query). hbbs now also answers online
  queries on the `/ws/id` WebSocket path Tether clients use; without the relay update the list
  shows "Online status needs a relay update".
- **Team access** ([docs/api.md](docs/api.md#team-technician-endpoints)): each technician app has its
  own Ed25519 key; an administrator adds it to the team roster on the new **Team** page of Tether
  Manage. Members then read the organisation's devices (no IP addresses) and share tags, names,
  rules and saved searches (`/api/team/*`, signed requests, revision check with three-way merge on
  conflicts, size and rate limits); members with the audit flag see a host's session log.

- Relay monitoring ([docs/monitoring.md](docs/monitoring.md)): Prometheus metrics in hbbs
  (127.0.0.1:21123: online hosts by path, connections, registrations, RegisterPk results, punch
  holes, brute-force refusals, relay health / load / hand-outs, failover state and transitions,
  TURN allocations, traffic and drops), hbbr (127.0.0.1:21124: sessions by transport, pairing,
  waits and timeouts, relayed bytes, downgrades, blocks, draining) and tether-api (`/metrics` on
  21120 for loopback only: requests by endpoint and status, latency histogram, workers vs cap,
  busy and body timeouts, active devices, data dir size), all with process metrics; Caddy's
  metrics on 127.0.0.1:21125 with the admin API still off. `deploy/monitoring/`: installer
  (`install.sh target|monitor`), Prometheus (30 days) with alert rules and their promtool tests,
  Alertmanager (Telegram ready, off by default), node_exporter, blackbox probes of each node's
  public endpoints from the other node, six provisioned Grafana dashboards at
  `https://<monitoring node>/grafana/` behind Caddy basic auth plus Grafana login, and
  `/_metrics/*` on the other node for the monitoring node's IP and password only. Helm:
  `metrics.enabled`, `metrics.serviceMonitor.enabled`. End-to-end test
  `scripts/test-monitoring.sh`.

### Fixed
- **hbbs: hosts behind a busy NAT took minutes to come back after an hbbs restart.** Every
  WebSocket reconnect sends RegisterPk, and upstream allows 32 per source IP per minute. A
  RegisterPk that repeats a host's registered uuid and key now passes that limit (2 000 hosts
  behind 20 IPs: back in 5 s instead of ~200 s). The limits are configurable
  (`IP_BLOCK_PER_MIN`, `IP_BLOCK_IDS_PER_DAY`; `tether-hbbs.service` sets 300 and 5 000).
- **hbbs/hbbr: reconnect storms overflowed the listen backlog** (128): 18 040 SYNs dropped when
  10 000 clients reconnected at once. Now 8192 (with `net.core.somaxconn` 8192): none dropped.
- **tether-api: heartbeats of large sites were refused.** The limit of 30 heartbeats per IP per
  minute hit every site with more than ~450 devices behind one IP; it now applies only to devices
  the API does not know yet. The device list holds 50 000 devices (was 10 000).
- **tether-api: slow custom-app downloads (`/q/<token>`) blocked the API.** Each held one of 32
  workers for the whole download. With the new Caddyfile, tether-api prepares the file once and
  Caddy sends it (`X-Accel-Redirect`); old and new Caddyfile and tether-api work in any
  combination. Helm: the api pod's Caddy serves `/q/`.
- hbbs: the punch-request log (admin command `pr`) is capped at 10 000 entries instead of growing
  for as long as hbbs runs; no global lock per WebSocket ping.

### Changed
- **hbbr uses mimalloc** instead of musl's allocator: 30–38 % less CPU per relayed Gbit/s.
- `server/sysctl/90-tether.conf` (installed by `install.sh`): conntrack table, accept queues,
  UDP buffer for hbbs (`-M 8388608` in the unit), BBR, reserved service ports. tether-api unit:
  `LimitNOFILE=65536`.
- Helm: `examples/scale-10k.yaml` with measured sizes (the chart defaults are for small installs).
- `tools/load`: load generator (`tether-load`, built by `scripts/build-relay.sh`) and scripts for
  registrations, relay traffic, tether-api, downloads and restarts; results in
  `tools/load/results`.


## [0.5.21] — 2026-11-07

Clients: picture quality (colours, still-picture sharpening for AV1). Relays: the security fixes below,
deployed on both relays on 2026-11-06 (no client update needed for them).

### Security
Relay fixes from the code audit. They take effect when the relay's `hbbs`, `tether-api` and
Caddyfile are redeployed (Helm: the proxy ConfigMap and the hbbs/api images). Clients and wire
formats are unchanged.
- **hbbs: any client could take over a host's signalling path.** A connection that only named a
  host's ID (`RegisterPeer`) was kept as that host's signalling path, and its heartbeats moved
  the host's address to it, so punch-hole, relay and ICE messages for the host (with the
  controller's IP and TURN credentials) went to the attacker and the host looked offline. Now a
  connection becomes a host's path only when its registration is accepted: a `RegisterPeer` from
  the host's IP with a known key, or a `RegisterPk` that passes upstream's UUID/key checks. Only
  the connection that is the current path refreshes it.
- **hbbs trusted a client-sent `X-Real-IP`.** Caddy passed it through, so any client could pick
  the IP hbbs used for its brute-force blocker, IP-change tracking and same-IP checks. hbbs now
  takes the last `X-Forwarded-For` hop (the peer Caddy saw, set by Caddy itself), and Caddy
  overwrites `X-Real-IP` (read by hbbr) on every hbbs/hbbr route, in `server/caddy/Caddyfile`
  and the Helm proxy template.
- **TURN took rendezvous traffic from hosts behind the same NAT as a TURN peer.** Datagrams from
  any port of a peer's IP went to the TURN allocation for as long as it lived (up to 12 h): such a
  host's registrations were lost (it looked offline) and its UUID and key reached the TURN
  client. A controller could aim this at a host on purpose. Now only the exact address an
  allocation sent to within the last 5 minutes is routed to it, and nothing that parses as a
  rendezvous message is.
- **TURN request limits could be bypassed.** The cap on unauthenticated requests (50/s, each
  one makes the TURN library keep a nonce for an hour) skipped any request that merely carried a
  MESSAGE-INTEGRITY attribute; it now applies to every request whose MESSAGE-INTEGRITY does not
  verify. The first client address of a credential is recorded only for a verified request, and
  that table is capped (100 000 live entries) and pruned by expiry instead of growing without
  bound.
- **tether-api: one slow client blocked the whole relay API.** Requests were served one at a
  time with no timeout, so a client trickling a body, or a slow custom TetherQS download, held up
  every heartbeat, policy fetch, audit post and admin call. Requests are now served concurrently
  (up to 32, then 503); bodies are read and responses written outside the state lock; every read
  and write on a connection times out after 30 s and a body must arrive within 60 s plus
  64 KiB/s. Caddy reads API request bodies up to 512 KiB whole before passing them on. All size
  and rate limits are unchanged.

### Fixed
- **Wrong colours in the technician app with VP8/VP9/AV1.** The host converts the screen with the
  BT.601 matrix but the bitstream does not say so, and the app's WebCodecs decoder used BT.709:
  saturated colours were off by up to 30 levels (pure green 192 shown as 163, blue tinted
  green, reds lighter). The decoder is now told the host's matrix; colours match the remote
  screen within a level. This was the largest part of the picture-quality gap to AnyDesk
  (luma PSNR on the quality test page 32.5 -> 38.4 dB with this fix alone). Hardware H.264/H.265
  hosts are unchanged.

### Changed
- **Sharper still pictures with AV1 (GPU-less hosts).** When the remote screen stops changing,
  the host now refines the still picture with up to ten extra AV1 frames at the encoder's best
  quantizer range (upstream refined only VP8/VP9/H.264/H.265; AV1, which Auto picks on hosts
  without a GPU, kept the first, coarse picture). The refinement strength follows the bitrate the
  link allows (Balanced: full, a congested link: less), and the next change on the screen is
  coded at the normal quality again, so motion, latency and bandwidth under motion are
  unchanged. Together with the colour fix: 39.2 dB against AnyDesk's 35.3 dB on the same VM,
  at about the same bytes per page; see "Image quality vs AnyDesk" in `docs/performance.md`.

### Added
- Picture-quality bench: `tools/bench/quality.html` (deterministic test page), `quality-run.sh`,
  `quality-measure.py` (PSNR Y/Cb/Cr, SSIM, MS-SSIM, text sharpness and colour fringing against
  the host's own screenshot), `quality-table.py`, `quality-view.mjs` (Tether viewer canvas over
  CDP), `xgrab.py` + `udp-stamp.mjs` (a viewer on Xvfb in WSL, e.g. AnyDesk for Linux),
  `qgrab.ps1` / `qperf.ps1` (the VM's own screenshot; its bytes sent and CPU per second).


## [0.5.20] — 2026-11-06

### Fixed
- **TetherQS: with two connections the permissions could not be changed.** Both session cards
  were squeezed into the fixed 420x560 window, and each card's permission list shrank to a
  sliver (or to nothing with an update notice or "Allow admin access" shown), so no switch could
  be seen or clicked. With more than one session the cards now keep their full height and the
  list of cards scrolls; every switch sends the change for its own connection. The technician
  app's connection window (one tab per connection) was not affected; both are covered by
  `apps/desktop/e2e/cm-two-clients.e2e.mjs`.
- **Session toolbar popped up whenever the pointer touched the top edge**, covering title bars
  and tabs of the remote screen, so they could not be clicked. The hidden toolbar now comes back
  only from a small handle at the top centre (hover about 300 ms or click); it never appears
  while a mouse button is held, and it hides shortly after the pointer leaves it. Pin keeps it
  up as before; full screen and window behave the same. Clicks anywhere else at the top edge go
  to the remote computer.
- **Selection rectangles on the remote screen could not be moved (e.g. the screenshot tool, seen
  with the macOS technician app).** A drag that reached the top edge brought up the toolbar,
  whose bar then took the next press meant for the selection; and a quick drag sent its last
  position only after the release (moves wait for the next animation frame, which WebKit, the
  engine of the macOS app, often ran only after the button was up), so the remote side saw a
  click instead of a drag. Presses and releases now go out after the pending position, and the
  toolbar no longer appears during drags. Covered in Chromium and WebKit by
  `apps/desktop/e2e/viewer-mock.e2e.mjs` (mock mode, no Tauri).
- **macOS: minimizing the "Session active" window left the Tether main window on screen.** The
  connection window (`--cm` process) told the main window to minimize only once, when the
  session started. Now it does so every time the connection window becomes minimized during a
  session (yellow button, Dock click, session start); it checks the window every 300 ms, since
  macOS reports no minimize event. Windows is unchanged (the main window is hidden for the whole
  session there).

### Relays
- **Backup relays.** A `RELAY_SERVERS` entry may end in `|backup`
  (`<address>[|<health URL>][|backup]`). hbbs hands out healthy primary relays only, even when a
  backup is less loaded, and switches new sessions to the healthy backups only while no primary
  passes its health check (checked every 3 s, 2.5 s timeout; back to the primary on the first
  check it passes). Each switch is logged once at INFO (`relay failover: ...`); the hbbs admin
  command `rs` marks relays `primary`/`backup`. Existing entries are unchanged. Use:
  `RELAY_SERVERS='<hetzner>|https://<hetzner>/readyz/relay,<oracle>|https://<oracle>/readyz/relay|backup'`.
  See [docs/relay-ha.md](docs/relay-ha.md).

## [0.5.19] — 2026-11-06

### Fixed
- **Typing stopped working in sessions (Windows technician app 0.5.18).** With "Send system keys"
  on (the default), the session window threw away every key that came through the webview,
  trusting the keyboard hook to have sent it. When the hook was not catching keys - Windows
  silently removes a hook that answers too slowly, and turning focus away and back could leave a
  stale hook (rdev's `exit_grab` race) - nothing reached the remote computer, to Windows and Mac
  alike, until "Send system keys" was turned off. Now every key that reaches the window is sent
  (a key the hook took never gets there), the hook is installed once and kept, and it only takes
  keys while the foreground window of the active desktop belongs to that Tether: a second Tether
  (or one on another desktop) can no longer catch another window's keys. The hook also no longer
  asks Windows for the character of every key of every program (that could break dead keys such
  as ^ and ´ elsewhere); it asks only for AltGr characters towards a Mac, without changing the
  keyboard state. The keyboard end-to-end test now runs with the hook installed.
- **macOS: Accept needed two clicks.** The first click into an inactive Tether window only
  activated it. All windows of Tether and TetherQS now take that first click as a click.

## [0.5.18] — 2026-11-06

Tether / TetherQS 0.5.18 (Windows and macOS). Windows 0.5.17 was published before these
fixes were merged; macOS goes from 0.5.16 straight to 0.5.18 and gets everything in 0.5.17 too.

### Fixed
- **Host session: the app icon toggles only the connection window.** Clicking Tether on the
  taskbar or in the Dock during a host session restored the main window together with the
  connection window, and the next click minimized only the connection window, so the main window
  stayed in the user's way. Windows: the main window is now minimized and taken off the taskbar
  for the session (the connection window's button is the only Tether button), starting Tether
  again (pinned icon, Start menu) toggles the connection window, and the main window is back on
  the taskbar, minimized, when the last session ends. macOS: the connection window answers the
  Dock click before its restore animation (the main process gave up after 500 ms and opened the
  main window too), and the main process never opens the main window on a Dock click while a host
  session is active.

- **Keyboard only partly working after Alt+Tab, the Win key or Alt+Space.** A key whose release
  the session window never saw (focus taken by the local Alt+Tab, Start menu, PowerToys Run or
  Windows search) stayed pressed on the remote computer, and later presses of that modifier were
  dropped. Every held key is now released on the remote side when the window loses the focus.

### Added
- **Keyboard between PC and Mac.** Towards a Mac, Ctrl works as ⌘ (Ctrl+C, Ctrl+V…), the Win key
  as Control, Ctrl+Y as redo, Home/End as line start/end, Ctrl+arrows as word jumps, Alt+F4 as ⌘Q,
  and AltGr characters of German and other layouts (@ € [ ] { } \ | ~) arrive as typed. From a Mac
  the other way round (⌘ = Ctrl, ⌘←/→ = Home/End…). On by default when the platforms differ,
  per session in the new **Keyboard** menu of the session toolbar, which also sends ⌘⌥Esc (Force
  Quit) to a Mac. See docs/keyboard.md.

- **System keys go to the remote computer** (Windows technician app): while a session window is
  active, Win, Win+R, Alt+Space, Alt+Esc, Ctrl+Esc, Print… reach the remote computer instead of
  this PC (low-level keyboard hook, only while the window has the focus and no text field is
  active). Alt+Tab stays local unless **Keyboard → Send Alt+Tab** is on. Ctrl+Alt+Home gives the
  keys back to this PC until the next click into the picture. Turn off with **Keyboard → Send
  system keys**.

- **Viewer: sharpening.** The remote picture is scaled to the window and sharpened in one WebGL2
  pass (a luma unsharp mask with a noise gate, clamped to the neighbourhood so it cannot make
  halos or ringing; frames are uploaded from the decoder without a CPU copy). Toolbar → Image
  quality → **Sharpening**: **Auto** (default: only when the picture is scaled), **Strong**,
  **Off** (the previous 2D canvas path). Remembered per user on the PC. Without WebGL2, or when the
  GPU context is lost, the viewer falls back to the 2D canvas by itself. Measurements in
  `docs/performance.md` ("Sharpening"); fixture `apps/desktop/e2e/sharpen-quality.mjs`, session
  check `apps/desktop/e2e/sharpen-e2e.mjs`.

## [0.5.17] — 2026-11-06

Tether / TetherQS 0.5.17 (Windows only; macOS gets these changes with 0.5.18), Tether Manage 0.5.5;
on the relays hbbs, tether-api and the Caddyfile (deployed 2026-11-06).

### Security
- **Tether Manage: policy serial only from signed documents.** The next policy serial came from
  `/api/admin/status` `policy_serial`, which a relay can set to anything: `u64::MAX` overflowed and
  ended publishing for good. It now comes, like the server key list serial, only from policies a
  relay serves that verify with the admin key and from this PC's own `policy/published/*.json`,
  with an overflow check and a clear error.
- **Tether Manage: the saved server private key is readable by the current user only.**
  "Save key files…" creates `id_ed25519` with a protected ACL that grants only the current user's
  SID (0600 on unix), warns that it is the private server key, and offers **Delete key files**
  until it is deleted.
- **Tether Manage: "Every relay is switched" is checked.** hbbs reports the key it signs with
  (`/pubkey` on its health port, `https://<relay>/rendezvous/pubkey` through Caddy and the Helm
  proxy); Manage enables the step only when every relay reports the new key and names the ones
  that do not.
- **Rollback floor for the server key list.** Tether and TetherQS keep the highest accepted key
  list serial in `server-keys.serial`; deleting `server-keys.json` no longer lets an older, validly
  signed list back in.
- **Tether Manage: vault format version.** `keys.json` with server keys is written as format 2,
  which Tether Manage before 0.5.5 refuses to open (instead of dropping the server keys when it
  saves). Unknown fields are kept; a vault of a newer format can be used but is never saved.
- **Size limits while reading.** Tether and TetherQS read the policy and key list answers with a
  hard limit (stop after limit + 1 bytes) instead of reading the whole answer first.

### Relays
- hbbs (`patches/rustdesk-server.patch`) and the Caddyfile (`/rendezvous/pubkey`) must be updated
  for the switch check; until then Manage keeps the step closed for that relay. hbbr and
  tether-api are unchanged.

### Fixed
- Alt+Space in a session opened PowerToys Run / Windows search on the technician's PC.

- **No more freezes in ~3 s steps when the screen content suddenly gets heavy** (seen on a Mac
  host with H.265: a switch to full-screen noise or photos froze the picture 4 times for ~2.7 s).
  The host's video loop now treats a frame the connection has not taken within 1 s as a full
  link and halves the bitrate at once (without adjustable bitrate: the frame rate), instead of
  waiting seconds for the delay probes to confirm it (docs/performance.md).

- The host's start-up fast path doubles the frame rate per clean delay reply (up to the viewer's
  cap) instead of jumping straight to it, and only during start-up: a jump to 120 fps offered 8x
  the bytes to an unmeasured path (4 s of queue in the simulation, now 1.2 s), and a jump at any
  time undid the guard that keeps a frame rate which just congested from coming back at once.
  Clean links still reach 60 fps within about a second.

### Added
- The session shows when the person at the remote computer has turned off control of mouse and
  keyboard.

- Statistics overlay: draw time per picture and the sharpening in use. Debug measurement mode
  (`localStorage["tether.renderSync"] = "1"`): each draw waits for the GPU, so the draw time includes it.

- **Session audit log.** tether-api now accepts the connection, file-transfer and alarm records
  hosts post (`POST /api/audit/conn|file|alarm`; until now every session logged "Audit post
  dropped … 404") and stores them as daily JSON lines, 90 days or 50 MB (`AUDIT_RETENTION_DAYS`,
  `AUDIT_MAX_MB` in `api.env`), size- and rate-limited, deduplicated by nonce. Signed
  `GET /api/admin/audit?since=&limit=&device=` reads them newest first. Tether Manage › **Sessions**
  merges the records of every relay: time, host, technician, IP, action (with file paths),
  duration, search, 24 h to 90 days, CSV export. Unauthenticated telemetry: see SECURITY.md.
  Deploy: the new tether-api binary only (Caddy already routes `/api/*`). Regression check:
  `apps/desktop/e2e/audit-e2e.mjs`.
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

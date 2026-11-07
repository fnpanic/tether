# Updates

Both apps update themselves from the relays (and, for downloads, an optional CDN). Since the
release after 0.5.23 the source of truth is a **signed release index** per channel; every update
file is signed with the updater key held by Tether Manage, and clients check every byte against the
index before they install anything. Design and reasoning: [update-design.md](update-design.md).
Wire formats: [api.md](api.md#download-layout-and-update-manifests).

## Endpoint layout

```
https://<relay>/download/<channel>/releases.json                                   signed index (all apps)
https://<relay>/download/<channel>/technician/windows-x86_64/latest.json           legacy, ≤ 0.5.23
https://<relay>/download/<channel>/technician/windows-x86_64/Tether_<version>_x64-setup.exe(.sig)
https://<relay>/download/<channel>/customer/windows-x86_64/latest.json
https://<relay>/download/<channel>/customer/windows-x86_64/TetherQS-<version>.exe(.sig, .gz)
https://<relay>/download/<channel>/technician/darwin-universal/latest.json
https://<relay>/download/<channel>/technician/darwin-universal/Tether_<version>_universal.app.tar.gz(.sig)
https://<relay>/download/<channel>/technician/darwin-universal/Tether_<version>_universal.dmg(.sig)
https://<relay>/download/<channel>/customer/darwin-universal/latest.json
https://<relay>/download/<channel>/customer/darwin-universal/TetherQS-<version>-universal.zip(.sig)
https://tether-dl.thomas-schend.workers.dev/<channel>/<app>/<platform>/<file>        CDN copy (optional)
```

`<channel>` comes from policy `update.channel` (default `stable`; anything that is not 1–32
lowercase letters, digits or dashes falls back to `stable`). Windows and macOS releases are
independent entries. `TetherQS-<version>.exe.gz` is the gzip copy (35 → 15 MB): the relays send it
gzip-encoded to clients that accept it (Caddy `precompressed`, nginx `gzip_static`), and it is the
only TetherQS.exe file on the CDN (25 MiB per file limit).

## How a release reaches devices

| Path | Latency | Cost |
| --- | --- | --- |
| **hbbs push**: tether-api bumps the release generation `G` on every index or manifest change and writes `/var/lib/tether-api/release-generation`; hbbs reads it every 5 s and sends `ReleaseHint{G, spread}` to every host registered over the WebSocket, and once to every host right after it registers | < 2 min (hint within 5 s, then a random wait of 0–`RELEASE_HINT_SPREAD` s, default 60) | one 10-byte frame per device |
| **Heartbeat answer**: `POST /api/heartbeat` answers `200 {"releases": G}` | ≤ `policy.refresh-minutes` (15) | none (the heartbeat is sent anyway) |
| **Poll**: at start (random 5–60 s), every `update.check-hours` (+0–20 % jitter), after a policy change | ≤ `update.check-hours` | `If-None-Match`, mostly `304` |

A hint only means "look again" (it is unsigned): at most one hint-triggered check per relay per
minute. Each check asks **every** relay for `releases.json` (`If-None-Match: "<serial>"`); the
highest valid serial wins. On the technician app the hbbs connection lives in the `--server`
process (the service, if installed): it stores the hints in the option `tether-release-hint`, which
the UI process mirrors every second.

## Signed release index

- Signed with the **admin key** (the same envelope, keys and rotation as the policy), so installing
  code needs both keys: the updater key (the file) and the admin key (the metadata naming it).
- Clients verify it against the compiled-in `TETHER_ADMIN_PUB_KEY`, require the requested channel,
  and keep a **floor** per channel in the config dir (`releases-<channel>.json` + `.serial`):
  older serials are refused, from any relay, also after the cache is deleted.
- Once a device accepted one index it **never uses `latest.json` again** (no unsigned
  `mandatory`/`minimumVersion` can block it any more). Builds with
  `tether_trust::release::REQUIRE_SIGNED_INDEX = true` (0.5.25 and later,
  decision D2) never use `latest.json` at all.
- A download is accepted only if size, sha256, the minisign signature and the signed trusted
  comment (`file:` and `version:`) all match the entry. The download hosts are untrusted: the CDN,
  then the relay (`https://{relay}/…`, the relay the index came from) are tried in that order.
- **Expiry** (decision D3): Manage signs indexes for 30 days and re-signs those with less than 15
  days left when you unlock the keys. An expired index is still used; Settings › About and
  Diagnostics (technician app) and Help (TetherQS) say "Update information is out of date", the
  heartbeat reports `stale`, and the alert `UpdateStale` fires.

## Staged rollout, halt, security releases

Set when publishing in Tether Manage › Releases (decision D4):

| Preset | Steps |
| --- | --- |
| Standard (default) | 1 % at once → 10 % after 1 h → 50 % after 6 h → 100 % after 24 h |
| Fast | 10 % → 100 % after 1 h |
| Immediately | 100 % |

- Each device has a stable bucket per release (`SHA-256(uuid, app, version)`, 0–100): it is
  eligible once the step's percent is above its bucket. The ramp is signed once and runs while
  Manage is closed. Policy `update.rollout`: `early` (joins at the first step), `default`, `late`
  (waits for 100 %).
- **Security release**: skips the ramp.
- **Halt / Resume** (Releases › Rollouts): Manage re-signs the index with `halted` (new serial).
  Devices learn of it within about a minute (push) and re-check right before installing, so a
  halted release is not installed, even if it was already downloaded. Devices that installed it
  keep it; recovery is a newer release (versions never go down). **Everyone now** restarts the
  ramp at 100 %.
- A mandatory release blocks sessions only on devices its rollout reached; minimum versions always
  block.

## Policy keys

| Key | Default | Effect |
| --- | --- | --- |
| `update.enabled` | `Y` | Check and install updates. `N`: the app does not install updates itself; the organisation deploys them. Both apps still read the release index, so a release's minimum version keeps blocking sessions; `mandatory` and `forced` then do not apply, and nothing is offered or downloaded. |
| `update.channel` | `stable` | Channel folder to read |
| `update.check-hours` | `6` | Fallback poll interval (minimum 1 hour, +0–20 % jitter) |
| `update.minimum-version` | – | Versions below it cannot be used for sessions |
| `update.force` | `N` | Install *required* updates automatically |
| `update.restart-seconds` | `60` | Countdown before a forced update installs and restarts (runs only while no session is open) |
| `update.rollout` | `default` | `early`, `default` or `late` in staged rollouts |

## Update modes

Set per release in Tether Manage (index fields) or by policy. Clients apply the stricter of release
and policy.

| Mode | How it is set | User experience | Sessions on the old version |
| --- | --- | --- | --- |
| Optional | Release without flags | Card "Update available" with **Install and restart** / **Later** | Allowed |
| Mandatory | Release `mandatory` | Modal "Update required", no Later | New sessions blocked until installed (once the rollout reached the device) |
| Minimum version | Release `minimumVersion` or policy `update.minimum-version` | As mandatory. If no newer version is published: "Your organisation requires a newer version … none is available yet." | Blocked, even with `update.enabled = N` |
| Forced | Release `forced`, or a required update with policy `update.force = Y` | "Tether will update", countdown of `update.restart-seconds`, then install and restart. **While a session runs**: "Will update after this session"; the countdown starts when it ends (decision D7) | Blocked only if also required |

A running session is never ended by an update decision: blocking applies to new sessions, and a
forced install waits until no session (incoming or outgoing, file transfer included) is open.

### Blocking is enforced in the backend

The update state is cached in the config folder (`update-state.json`), so every process role
enforces it without network access:

- **Outgoing sessions** (UI, `--connect`, `tether://` links) fail with
  `locked:update-required: Install the required Tether update to start sessions.` The CLI exits
  with code 6.
- **Incoming sessions**: the connection manager closes every incoming connection while the update
  is required.
- TetherQS closes incoming connections in the backend and disables **Accept**.

## Technician app (installer)

1. The main window process checks as described above. The requests use the API route racer
   (443, then 80; proxy, then direct). `tether --check-update` runs the same check.
2. Once the device is eligible, the installer is downloaded in the background, verified and kept in
   the config folder (`update-<version>.staged`, verified again before use). Every check deletes
   the other `update-*.staged` files (superseded or installed versions: on Windows the installer
   ends the app before it could remove its file).
3. **Install and restart** (or the forced countdown) re-checks the index, then hands the verified
   bytes to the Tauri updater's installer (no second download, so the version checked is the
   version installed). It runs the NSIS installer in passive mode (progress window, no questions).
4. The installer (per machine, `C:\Program Files\Tether`) stops the `Tether` background service if
   it is installed, overwrites the files, starts the service again, and the app restarts. A
   per-machine install needs administrator rights, so on a PC where the user is not an
   administrator Windows shows a UAC prompt.

On macOS the same happens with `Tether_<version>_universal.app.tar.gz`; the updater replaces
`Tether.app` in place. The user must be allowed to write to the app's folder (`/Applications` for
administrators). Unsigned builds lose their Screen Recording / Accessibility permissions with every
update ([macos.md](macos.md#permissions)).

## TetherQS (portable self-update)

TetherQS has no installer. It checks like the technician app (also after policy changes and
hints) and on **Help → Check for updates**. If the device is eligible for a newer version:

1. Download (HTTPS only, at most 200 MiB): the gzip copy first (CDN, then relay), else the exe;
   the gzip copy's own size and sha256 are checked before it is unpacked.
2. Verify size, sha256, the minisign signature and the signed version against the index (legacy
   relays: signature and signed version only) with the compiled-in `TETHER_UPDATER_PUB_KEY`.
3. Write it as `TetherQS.exe.new` next to the running exe, rename the running exe to
   `TetherQS.exe.old` (Windows allows renaming, not overwriting, a running exe), and rename
   `.new` to `TetherQS.exe`. If the second rename fails, the old exe is put back.
4. Restart. The new process keeps the ID and the one-time password (hand-over file, valid 60 s),
   so a technician can reconnect with the same credentials. `.old` and `.new` are deleted at the
   next start.

If the folder of the exe is not writable, the new exe goes to the fallback folder
`%LOCALAPPDATA%\Tether-QS\TetherQS.exe` and is started from there. A forced update waits until no
session is open. No admin rights are needed.

On macOS TetherQS downloads `TetherQS-<version>-universal.zip` (verified the same way), unpacks it
with `ditto` into `TetherQS.app.new` and swaps it in like the exe.

## Signing keys

| Key | Private half | Public half |
| --- | --- | --- |
| Updater key (minisign) | Tether Manage, `keys.json`, encrypted with the passphrase | `TETHER_UPDATER_PUB_KEY` at build time, both apps |
| Admin key (Ed25519) | Tether Manage, same file | `TETHER_ADMIN_PUB_KEY`, both apps and tether-api (`ADMIN_PUB_KEY`) |

Release builds refuse to compile without the keys. Tether Manage signs files exactly like
`tauri signer sign`: the `.sig` and the `signature` fields are base64 of the minisign signature
file, with the trusted comment `timestamp:<unix>\tfile:<name>\tversion:<version>`; the apps refuse a
signature whose comment names another file or version.

## CDN (Cloudflare Workers Static Assets, decision D1)

Optional; without it the relays serve every download. Free plan, no billing and no domain: static
asset requests are free and unlimited and never run the Worker script. Limits: 25 MiB per file,
20 000 files per version. Each publish deploys a **new Worker version** whose assets are the update
files of the current and the previous release of each channel/app/platform (technician installer,
mac archives, `TetherQS-<v>.exe.gz`; the plain TetherQS.exe and the .dmg stay on the relays).
Manage refuses to publish when a CDN file is over 25 MiB. The CDN is untrusted (see above): if it
is blocked or serves something else, clients use the relay URL.

Set up once:

1. Cloudflare account with the workers.dev subdomain `thomas-schend` (exists). An **account API
   token** with *Workers Scripts: Edit* (check it: `curl -H "Authorization: Bearer $TOKEN"
   https://api.cloudflare.com/client/v4/accounts/$ACCOUNT_ID/tokens/verify`, not `/user/tokens/verify`).
   Keep it in `C:\vms\cloudflare\cf.env` (`CLOUDFLARE_API_TOKEN=…`, `CLOUDFLARE_ACCOUNT_ID=…`).
2. Tether Manage › Releases › Download CDN: address `https://tether-dl.thomas-schend.workers.dev`,
   worker `tether-dl`, **Choose…** `cf.env`, key passphrase, **Save**, then switch it on. The token
   is sealed in the key vault (format 2) and never shown again.
3. The next publish creates the Worker (the first upload creates the script `tether-dl`; it runs
   only for paths without an asset and answers 404/405). Check:
   `curl -I https://tether-dl.thomas-schend.workers.dev/stable/technician/windows-x86_64/Tether_<v>_x64-setup.exe`.

Manual fallback (same result, from a shell with Node): put the files in a folder as
`<channel>/<app>/<platform>/<file>` and run `server/cloudflare/worker/deploy.sh <folder>` (reads
`cf.env`, refuses files over 25 MiB, `npx wrangler@4 deploy`; no `wrangler login` needed with the
token). Tests of the Worker script: `npm test` in `server/cloudflare/worker`.

## Release procedure

1. Set the version in `apps/desktop/src-tauri/tauri.conf.json` / `Cargo.toml` (technician) or
   `apps/quick/src-tauri/Cargo.toml` (TetherQS).
2. Build with the four release variables (`TETHER_SERVER`, `TETHER_PUB_KEY`,
   `TETHER_ADMIN_PUB_KEY`, `TETHER_UPDATER_PUB_KEY`), see [deployment.md](deployment.md):
   technician `npx tauri build --bundles nsis` → `Tether_<version>_x64-setup.exe`; TetherQS
   `npx tauri build` → `TetherQS.exe`. macOS: `bash scripts/build-macos.sh` on a Mac →
   `dist/macos/` ([macos.md](macos.md)).
3. Test the build on one machine (install, connect, `tether --version`).
4. Tether Manage → unlock → **Releases**. Quickest: **Publish the latest builds** (Standard
   rollout). One file at a time, or with Mandatory / Minimum version / Forced / a rollout preset /
   Security release: **Sign and publish a new release**. Upload order: files to every relay (and
   the CDN), then the signed `releases.json` to every relay, then each relay's `latest.json` (for
   clients up to 0.5.23, which update at their next check, once, without rollout).
5. Watch **Releases › Rollouts** (percent, halt/resume) and **Devices** (versions), or the Grafana
   API dashboard's Releases row (generation, hints, index fetches, update states, downloads by
   version).
6. Republish the **Download page** so its links point to the new files.

Do not publish a version lower than the one already in a channel: clients only install newer
versions.

## Staged rollout with a `test` channel

The ramp covers the usual case. For pilots that should always go first:

1. **Policy → New group** `pilot` (profile `technician`, and a second one for `customer` if
   needed). Set `update.channel` to **Locked** with value `test` (or `update.rollout` to `early`).
2. **Devices**: assign the pilot machines to the group. **Publish** the policy.
3. Publish the release to channel `test`. Optionally use a pre-release version such as
   `0.3.1-test`: it sorts below `0.3.1`, so pilot devices later move to the final `0.3.1` on
   `stable` by themselves.
4. When the pilots are fine, publish the release to `stable`. Switching channel never downgrades.

Test-channel builds are not linked from the download page, which always uses `stable`.

## Relay deploy notes

- tether-api writes `/var/lib/tether-api/release-generation` (group-readable); hbbs (user
  `tether`) reads it as a member of group `tether-api` (`server/install.sh`; restart hbbs after the
  group change). Override with `RELEASE_GENERATION_FILE`; hint spread `RELEASE_HINT_SPREAD`
  (default 60 s, at most 900; raise it on small nodes).
- Caddy routes `GET /download/<channel>/releases.json` to tether-api and serves `.exe.gz` with
  `precompressed gzip`; nginx does the same (`proxy_pass`, `gzip_static on`).
- Order: tether-api (+ Caddyfile) first, then hbbs; clients and Manage after. Each step works
  without the next ones; old clients ignore `ReleaseHint` and the heartbeat body.

## End-to-end test

`node apps/desktop/e2e/update-e2e.mjs` (local hbbs/hbbr/tether-api in WSL from
`scripts/build-relay.sh`, a misbehaving Caddy stand-in, Tether Manage with the test vault, a debug
technician app, the 0.5.23 build as an old client). Verified 2026-11-07, 21/21: the hbbs push
reached the running client in 37-54 s (`RELEASE_HINT_SPREAD=10`) and it verified the signed index;
the Standard ramp left it waiting (bucket 34.65 > 1 %); "Everyone now" made it download, verify and
stage the update; a running session blocked the automatic install; halt (re-checked before install)
and resume; a tampered index, a replayed older index and a downgraded binary were refused; once the
session ended the update installed; the 0.5.23 client stayed online through the hints.

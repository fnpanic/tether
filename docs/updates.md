# Updates

Both apps update themselves from the relays (and, for downloads, an optional CDN). Since the
release after 0.5.23 the source of truth is a **signed release index** per channel; every update
file is signed with the updater key held by Tether Manage, and clients check every byte against the
index before they install anything. Design and reasoning: [update-design.md](update-design.md).
Wire formats: [api.md](api.md#download-layout-and-update-manifests).

## Endpoint layout

```
https://<relay>/download/<channel>/releases.json                                   signed index (all apps)
https://<relay>/download/<channel>/releases-v2.json                                same + xz copies (new builds)
https://<relay>/download/<channel>/technician/windows-x86_64/latest.json           legacy, ≤ 0.5.23
https://<relay>/download/<channel>/technician/windows-x86_64/Tether_<version>_x64-setup.exe(.sig)
https://<relay>/download/<channel>/customer/windows-x86_64/latest.json
https://<relay>/download/<channel>/customer/windows-x86_64/TetherQS-<version>.exe(.sig, .gz, .xz)
https://<relay>/download/<channel>/technician/darwin-universal/latest.json
https://<relay>/download/<channel>/technician/darwin-universal/Tether_<version>_universal.app.tar.gz(.sig, .xz)
https://<relay>/download/<channel>/technician/darwin-universal/Tether_<version>_universal.dmg(.sig)
https://<relay>/download/<channel>/customer/darwin-universal/latest.json
https://<relay>/download/<channel>/customer/darwin-universal/TetherQS-<version>-universal.zip(.sig, .xz)
https://tether-dl.thomas-schend.workers.dev/<channel>/<app>/<platform>/<file>        CDN copy (optional)
```

`<channel>` comes from policy `update.channel` (default `stable`; anything that is not 1–32
lowercase letters, digits or dashes falls back to `stable`). Windows and macOS releases are
independent entries. `TetherQS-<version>.exe.gz` is the gzip copy (35 → 15 MB): the TetherQS updater
fetches it by its own URL and unpacks it itself, and it is the only TetherQS.exe file on the CDN
(25 MiB per file limit). The relays serve the plain exe as it is: until 2026-11-09 Caddy's
`precompressed gzip` sent browsers the exe gzip-encoded with a 206 to a plain GET, and Zscaler's
download sandbox never finished analysing it ("Wir überprüfen diese Datei…"). Verified after the change
(both relays, 2026-11-09): the same download through Zscaler starts right away.

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
- A device below a release's minimum version or below the policy's `update.minimum-version` is
  blocked until it updates, so the release is offered to it at once, whatever its bucket and also
  with `update.rollout = late`, in both apps (`must_update` in
  `apps/desktop/src-tauri/src/update.rs`, `eligible` in `apps/quick/src-tauri/src/update.rs`). A
  halted release is never offered this way.

## Policy keys

| Key | Default | Effect |
| --- | --- | --- |
| `update.enabled` | `Y` | Check and install updates. `N`: the app does not install updates itself; the organisation deploys them. Both apps still read the release index, so a release's minimum version keeps blocking sessions; `mandatory` and `forced` then do not apply, and nothing is offered or downloaded. |
| `update.channel` | `stable` | Channel folder to read |
| `update.check-hours` | `6` | Fallback poll interval (minimum 1 hour, +0–20 % jitter) |
| `update.minimum-version` | – | Versions below it cannot be used for sessions |
| `update.force` | `N` | Install *required* updates automatically |
| `update.restart-seconds` | `60` | Countdown before a forced update installs and restarts (runs only while no session is open) |
| `update.rollout` | `default` | `early`, `default` or `late` in staged rollouts (a device below a release's minimum version gets that release at once, see above) |

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

1. Download (HTTPS only, at most 200 MiB): the packed copy first (CDN, then relay; xz from
   releases-v2.json, else gzip), else the exe; its own size and sha256 are checked before it is unpacked.
2. Verify size, sha256, the minisign signature and the signed version against the index (legacy
   relays: signature and signed version only) with the compiled-in `TETHER_UPDATER_PUB_KEY`.
3. Write it as `TetherQS.exe.new` next to the running exe, rename the running exe to
   `TetherQS.exe.old` (Windows allows renaming, not overwriting, a running exe), and rename
   `.new` to `TetherQS.exe`. If the second rename fails, the old exe is put back.
4. Restart. The new process keeps the ID and the one-time password (hand-over file, valid 60 s),
   so a technician can reconnect with the same credentials. `.old` and `.new` are deleted at the
   next start.

If the folder of the exe is not writable, the new exe goes to the fallback folder
`%LOCALAPPDATA%\Tether-QS\TetherQS.exe` and is started from there. No admin rights are needed.

TetherQS never swaps its exe while a session is open (the restart would end the session). The
**Update** button then answers "The update restarts TetherQS: end the session first." A forced
update waits until no session is open; the install checks once more right before the swap, and
when a session began while it re-checked the index or downloaded, it waits for the end of that
session again. A download kept for the install (`update-<version>.staged` in the fallback folder)
counts as staged only when it still verifies against the index entry; a broken or foreign file is
downloaded again.

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
`TetherQS-<v>.exe.gz`, and the `.xz` copies of releases-v2.json; the plain TetherQS.exe, the .dmg
and the universal mac archives, 27-36 MiB, stay on the relays). A file over 25 MiB is left to the
relays (a grey note on the CDN line of the publish result); its xz copy goes to the CDN when it
fits (mac: 17 and 22 MiB). The CDN is untrusted (see above): if it is blocked or serves something
else, clients use the relay URL.

## Two index files (releases.json and releases-v2.json)

The universal macOS apps are two complete binaries (Intel + Apple silicon, the RustDesk core with
aom, FFmpeg and libvpx linked in): 85 and 69 MiB, 36 and 27 MiB packed, over the CDN's 25 MiB. xz
gets them to 21.8 and 17.0 MiB (zstd only to 24.1 for Tether, too close to the limit). But every
build before TetherQS 0.5.31 / Tether 0.5.33 rejects the whole `releases.json` when one entry has a
packed encoding other than gzip (or any new field), and devices that are switched off for weeks or
portable TetherQS copies started once in a while cannot be counted out. So:

- `releases.json` stays as it was (gzip only, mac files from the relays): every build reads it.
- `releases-v2.json` is the same index, **same serial**, plus xz copies (`<artifact>.xz`) for
  TetherQS.exe and both macOS apps, on the CDN and the relays. Manage writes it right after
  `releases.json` on every publish, halt/resume and re-sign; the xz copies of releases that did not
  change are carried over from the relays' current v2.
- TetherQS 0.5.31 / Tether 0.5.33 and later read both from every relay and take the highest serial
  (v2 on a tie). A v2 that is stale (published with it off) or missing costs nothing: they use
  `releases.json`, like older builds. They also skip packed encodings they do not know, so the next
  change needs no third file.

**Back to the single releases.json** (from the PC, Git Bash, in the repository):

```sh
bash scripts/release-index-v1.sh          # Manage writes releases.json only; v2 deleted on every relay
bash scripts/release-index-v1.sh --on     # write releases-v2.json again from the next publish
```

The first sets the marker `release-index-v1-only` in the Manage folder (`%APPDATA%\tether-manage`),
deletes `download/<channel>/releases-v2.json` on both relays over SSH (tether-api answers 404 at
once, no restart) and checks over HTTPS that both relays answer 404. Nothing else changes: new
builds fall back to `releases.json`, macOS updates come from the relays again, the `.xz` files stay
until Clean up removes their version. After `--on` only releases published from then on get an xz
copy. Tested end to end with `apps/desktop/e2e/index-v2-e2e.mjs` (local relays, Manage, debug
TetherQS; see "End-to-end test").

Set up once:

1. Cloudflare account with the workers.dev subdomain `thomas-schend` (exists). An **account API
   token** with *Workers Scripts: Edit* (check it: `curl -H "Authorization: Bearer $TOKEN"
   https://api.cloudflare.com/client/v4/accounts/$ACCOUNT_ID/tokens/verify`, not `/user/tokens/verify`).
   Keep it in `C:\vms\cloudflare\cf.env` (`CLOUDFLARE_API_TOKEN=…`, `CLOUDFLARE_ACCOUNT_ID=…`).
2. Tether Manage › Releases › Download CDN: address `https://tether-dl.thomas-schend.workers.dev`,
   worker `tether-dl`, **Choose…** `cf.env`, key passphrase, **Save**, then switch it on. The token
   is sealed in the key vault (format 2) and never shown again. The passphrase is checked against
   the vault first: with a wrong one ("Wrong passphrase.") nothing is stored and the credentials do
   not become active.
3. The next publish creates the Worker (the first upload creates the script `tether-dl`; it runs
   only for paths without an asset and answers 404/405). Check:
   `curl -I https://tether-dl.thomas-schend.workers.dev/stable/technician/windows-x86_64/Tether_<v>_x64-setup.exe`.

Manual fallback (same result, from a shell with Node): put the files in a folder as
`<channel>/<app>/<platform>/<file>` and run `server/cloudflare/worker/deploy.sh <folder>` (reads
`cf.env`, refuses files over 25 MiB, `npx wrangler@4 deploy`; no `wrangler login` needed with the
token). Tests of the Worker script: `npm test` in `server/cloudflare/worker`.

## Release procedure

1. **One version for everything** (from 0.6.0): Tether and TetherQS (Windows and macOS) and
   Tether Manage (Windows only) carry the same number, and every release bumps and builds all of
   them, also when only one app or platform changed. Set it in `apps/{desktop,quick,manage}`:
   `package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml` and the app's own entry in
   `src-tauri/Cargo.lock`. Tether Manage is not in the release index (it is not published to
   devices): build it with the same four variables (`npx tauri build` in `apps/manage`) and keep
   `tether-manage.exe` as `dist/manage/Tether Manage <version>.exe`. Nothing enforces the rule:
   before publishing, check that `dist/` holds every file of the new version (Windows setup,
   TetherQS exe, Mac dmg, `.app.tar.gz`, TetherQS zip); "Publish the latest builds" takes the
   highest version per app and platform it finds.
2. Build with the four release variables (`TETHER_SERVER`, `TETHER_PUB_KEY`,
   `TETHER_ADMIN_PUB_KEY`, `TETHER_UPDATER_PUB_KEY`), see [deployment.md](deployment.md):
   technician `npx tauri build --bundles nsis` → `Tether_<version>_x64-setup.exe`; TetherQS
   `npx tauri build` → `TetherQS.exe`. macOS: `bash scripts/build-macos.sh` on a Mac →
   `dist/macos/` ([macos.md](macos.md)).
3. Windows: sign both files with `scripts/sign-windows.ps1` ([code-signing.md](code-signing.md#signing-a-release-built-on-the-pc)),
   then copy them to `dist/windows` and `dist/quick`. Custom TetherQS apps keep this signature.
4. Test the build on one machine (install, connect, `tether --version`).
5. Tether Manage → unlock → **Releases**. Quickest: **Publish the latest builds** (Standard
   rollout). One file at a time, or with Mandatory / Minimum version / Forced / a rollout preset /
   Security release: **Sign and publish a new release**. Upload order: files to every relay (and
   the CDN), then the signed `releases.json` to every relay, then each relay's `latest.json` (for
   clients up to 0.5.23, which update at their next check, once, without rollout).
6. Watch **Releases › Rollouts** (percent, halt/resume) and **Devices** (versions), or the Grafana
   API dashboard's Releases row (generation, hints, index fetches, update states, downloads by
   version).
7. Republish the **Download page** so its links point to the new files.

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

## Device-status agent: monitor.json and enrolment tokens

The device-status agent (`monitor/windows-x86_64`, `tether-monitor-<v>.exe`) and its check packs
(`monitor-packs/any`, `monitor-packs-<v>.json`) have their own signed index,
`download/<channel>/monitor.json`: same format, rules, admin key and updater signature as
`releases.json`, but only these two apps (tether-api refuses Tether's apps in it and these in
`releases.json` and `releases-v2.json`). Entries list relay URLs only (no CDN copy). Tether Manage
has no screen for it; `tools/monitor-publish` (wrapped by `scripts/status-token.sh`,
`scripts/monitor-publish.sh` and `scripts/monitor-refresh.sh`, which build it, rebuild it when its
sources changed, and ask the passphrase) publishes it:

- It reads the admin and updater keys from Tether Manage's key file (`MANAGE_KEYS`, default
  `%APPDATA%\Tether-Manage\keys.json`) with the passphrase from `MANAGE_PASSPHRASE`, never from
  an argument. Build it with `TETHER_ADMIN_PUB_KEY` set as for Tether Manage, so indexes signed
  with an older admin key (after a rotation) are still read.
- `index` needs the build's `<file>.sig` and checks it against the updater key before anything is
  sent. It reads every relay's `monitor.json` (all must answer), refuses a version below the
  published one and the same version with other bytes, adds the entry, signs serial + 1 and
  uploads the file, its `.sig` and the index to every relay. A relay that lacks a file the index
  names gets the file but not the index (the output says which release to publish there again).
- `refresh --channel <c> --relay <relays>` signs the published index again, unchanged but for
  serial + 1 and a new expiry, and uploads it to every relay that holds the files it names; it
  refuses when nothing is published and prints the new expiry.
- Both keep the highest serial they ever published per channel in `monitor-serials.json` next to
  the key file, and refuse to publish or refresh below it, also when every relay answers 404 or an
  older index (a relay that lost or rolled back its `monitor.json`: restore it from a backup).
  A missing `monitor-serials.json` means no floor: keep the file with the key file, and copy it
  along when another machine takes over publishing.

- `token --label <text> --days <1-30> [--tether-id <id>] [--customer <ref>]` prints an enrolment
  token; with a Tether ID it enrols one device, with that ID; with a customer reference (1 to 32
  characters `A-Z a-z 0-9 . _ -`) the relay sets it on the devices that enrol with it
  ([health.md](health.md#customer-reference)). Its id, label, expiry and customer go to stderr and to the
  journal `monitor-tokens.json` next to the key file (never the token itself); `tokens` lists the
  journal and marks expired and revoked tokens.
- `revoke (--id <token id> [--exp <unix>] | --token-file <path> | --token -) --relay <relays>`
  revokes a token on every relay (expiry from the journal or from the token itself; an `--exp`
  earlier than the journal's is refused; an expired token is refused). It prints "revoked on N of M
  relays" and marks the journal "revoked" only when every relay accepted, else "partly revoked". Devices that enrolled with it stay enrolled: `devices [--token
  <id>] [--customer <ref>] --relay <relays>` lists the enrolled devices, `remove --uuid <uuid> --relay <relays>`
  removes one from every relay. Each prints one line per relay and exits non-zero if one failed
  ([health.md](health.md#revoking-a-token)).

```
read -rs MANAGE_PASSPHRASE && export MANAGE_PASSPHRASE
bash scripts/status-token.sh office 7
bash scripts/monitor-publish.sh stable monitor 0.1.0 dist/tether-monitor-0.1.0.exe https://relay1,https://relay2
bash scripts/monitor-publish.sh stable monitor-packs 2026.11.23 dist/monitor-packs-2026.11.23.json https://relay1,https://relay2
bash scripts/monitor-refresh.sh stable https://relay1,https://relay2
```

`monitor.json` expires 30 days after the last publish or refresh. Run `refresh` (or publish a new
release) about once a month: it is a manual admin step, run from the admin's own machine, because
it needs the admin key's passphrase. `tether_api_release_index_expires_timestamp{index="monitor"}`
shows the expiry ([monitoring.md](monitoring.md)). What an agent does with an expired index is
defined by the agent, not by the relay or this tool.

**Tether reads `monitor.json` too** (from 0.7.0, Windows only), never as a Tether update: to install
the agent and to keep an agent it installed at the minimum version, when the policy feature
`health.agent` asks for it ([health.md](health.md#installing-the-agent-through-tether)). It fetches
`download/<channel>/monitor.json` (its `update.channel`; the agent has `stable` and `test`) from its
relays and opens it with the same code as its own index (`release::choose_for` with the two monitor
apps only, admin signature, serial floor `monitor-<channel>.serial` beside its state). For this it
is stricter than for its own updates: it refuses an **expired** index (so a relay cannot have an
agent version from an old index installed), a halted entry and an unexpected file name, and the
artifact is checked for size, SHA-256, minisign signature with the compiled-in updater key and the
trusted comment (file and version), then checked again right before it runs. The staged rollout
applies to installs by policy, not to a technician's install in a session.

**The minimum version and `upgrade`.** `monitor-publish index … --min-version <v>` sets the entry's
`minimumVersion` (the wrapper `scripts/monitor-publish.sh` does not pass it; call the tool). Tether's
service reads the index at most every 6 hours; when the agent it installed (service program or
worker) has been below that version for 48 hours, it downloads the entry's version and runs that
program's `upgrade`, which keeps the agent's data, key, enrolment and switches. It never installs a
version that is not above the installed one. To move every machine: publish the new version with
`--min-version` set to it. The agent's own updater keeps working as before; Tether only enforces the
floor. Agents installed by hand, and machines without Tether's service, are never upgraded by
Tether.

**What must be published first.** Tether installs only what `monitor.json` offers, so the version
published there must be 0.2.0 or later (the first that knows the commands Tether uses) before a
policy says `install`. Agents 0.1.x installed by hand keep working and update themselves.

Deploy order, on every relay, before the first monitor publish: the new tether-api binary first,
then the proxy configuration (the Caddyfile or nginx.conf route below, which sends `monitor.json`
to tether-api). An older tether-api stores an uploaded `monitor.json` like any file, without
checking its signature or serial. Rolled back to a tether-api from before `monitor.json`, a relay
no longer serves it through tether-api (with the new proxy route: 404; with the old one Caddy
serves the file from disk) and leaves the files in place; the new binary serves them again.
A publish that reached only some relays (one down, a network error) is repaired by running the
same command again: the same version with the same bytes is not published anew, the current
index goes unchanged (rollout start kept) to the relays that lack it. Tokens are valid 1-30 days
and, unless issued for one Tether ID, reusable until they expire or are revoked.

## Relay deploy notes

- tether-api writes `/var/lib/tether-api/release-generation` (group-readable); hbbs (user
  `tether`) reads it as a member of group `tether-api` (`server/install.sh`; restart hbbs after the
  group change). Override with `RELEASE_GENERATION_FILE`; hint spread `RELEASE_HINT_SPREAD`
  (default 60 s, at most 900; raise it on small nodes).
- Caddy routes `GET /download/<channel>/releases.json` and `releases-v2.json` to tether-api
  (`@index path_regexp ^/download/[a-z0-9-]+/releases(-v2)?\.json$`) and serves the files as
  they are (no `precompressed`, see above); with nginx use `proxy_pass` and no `gzip_static`.
  Without that route Caddy serves both index files from disk, which works the same for clients
  (Caddy's own ETag, 404 when absent); only tether-api's index-fetch counter and the serial ETag
  are missing. On 2026-11-09 relay 1's Caddyfile had no `@index` route at all and relay 2's still
  the releases.json-only pattern (`scripts/caddy-index-v2.sh`, run on the relay, updates it, validated
  before the reload).
- `download/<channel>/monitor.json`, the device-status agent's signed index (apps `monitor` and
  `monitor-packs` only; Tether's apps are refused in it and its apps in `releases.json`), needs
  the route too: `^/download/[a-z0-9-]+/(releases(-v2)?|monitor)\.json$` (Caddyfile and
  nginx.conf in the repo). Without it Caddy serves `monitor.json` as a plain file: still
  verifiable, but without the serial ETag. A `monitor.json` upload does not move the release
  generation G (Tether clients never take it as a Tether update; only Tether's agent manager reads
  it, above); its serials, summed over all channels, are the `releases` hint of status answers.
- Order: tether-api (+ Caddyfile) first, then hbbs; clients and Manage after. Each step works
  without the next ones; old clients ignore `ReleaseHint` and the heartbeat body.
- **Release order for the agent through Tether (0.7.0):** relays first (tether-api and hbbs 0.7.0,
  with `HBBS_PEER_URL` and `PEER_LOOKUP_FROM` where they run apart,
  [deployment.md](deployment.md#update-the-agent-through-tether-070)), then the agent 0.2.0 or later to
  `monitor.json`, then Tether Manage and the apps 0.7.0, **the policy with `health.agent` last**. An
  old relay refuses to store that policy (`400 unknown feature "health.agent"`), refuses the grant
  (`401 enrollment token: not an enrollment token`) and an enrolment with a proof (`400`); an old hbbs
  answers `404` to every lookup, so the relay answers `409 tether id unknown to this relay`. An old
  Tether skips the two features (logged as rules not applied) and does nothing; a technician asking it
  in a session gets "Tether on the remote computer is too old (needs 0.7.0)." An old viewer has no
  menu item and shows a proof link as linked by the token issued for the host.

## End-to-end test

`node apps/desktop/e2e/update-e2e.mjs` (local hbbs/hbbr/tether-api in WSL from
`scripts/build-relay.sh`, a misbehaving Caddy stand-in, Tether Manage with the test vault, a debug
technician app, the 0.5.23 build as an old client). Verified 2026-11-07, 21/21: the hbbs push
reached the running client in 37-54 s (`RELEASE_HINT_SPREAD=10`) and it verified the signed index;
the Standard ramp left it waiting (bucket 34.65 > 1 %); "Everyone now" made it download, verify and
stage the update; a running session blocked the automatic install; halt (re-checked before install)
and resume; a tampered index, a replayed older index and a downgraded binary were refused; once the
session ended the update installed; the 0.5.23 client stayed online through the hints.

Re-verified 2026-11-09 at d1b9275 (technician app 0.5.30 code, debug build against the local relay):
35/35, the hbbs push reached the running client in 30 s (`RELEASE_HINT_SPREAD=10`), it fetched and
verified the index and, after "Everyone now", downloaded, verified and staged the update.

**In production, TetherQS** (2026-11-09, publish of 0.5.28, rollout Immediately): a TetherQS
running in the test VM got hbbs's release hint about 1 s after each relay's index changed and
showed "Version 0.5.28 is available" 35 s after the final publish, without a restart; the
update then installed from the app. Where the time goes: the publish raised the generation twice
(00:45:46 and 00:46:21); the first check ran before the new index was complete ("up to date"),
the per-relay limit of one check a minute held the second hint until 00:46:47, and the random
spread (0-60 s, `RELEASE_HINT_SPREAD`) added the rest. Worst case after a publish is therefore about
two minutes; this is the intended thundering-herd protection, not a fault.

**In production, technician app** (2026-11-09): the 0.5.30 rollout halted in Manage, a 0.5.29
build (the published one, own profile, on a hidden desktop) started; its first check reported
"halted". Rollout resumed at 07:34:25 (index serial 34): the release hint reached the running app
at 07:34:25.5, 0.5.30 was downloaded and verified at 07:34:49.9 and the home page showed "Update
available. Version 0.5.30 is ready to install. You have 0.5.29." at 07:34:50.8, 25 s after the
resume, without a restart.

### Two index files (`apps/desktop/e2e/index-v2-e2e.mjs`)

`node e2e/index-v2-e2e.mjs`: two tether-api relays in WSL behind proxies that log every request,
Tether Manage of this checkout with a copy of the test vault, a debug TetherQS (test keys,
`TETHER_DEV_API`, its own `TETHER_PROFILE`), and `tether-trust` as it was before releases-v2.json
(176c62c, `examples/index_check.rs` in a worktree) standing in for every installed app. Verified
2026-11-09, 28/28:

- Manage publishes TetherQS for Windows and macOS and Tether for macOS; on both relays v1 and v2
  have the same serial, v1 is gzip only, v2 has the xz copies: TetherQS.exe 35 → 9.0 MiB, the
  TetherQS zip's stored twin 69.2 → 17.0 MiB, the Tether .app.tar.gz's stored twin 85.3 → 22.2 MiB.
  Python checks that each .xz unpacks to exactly the twin whose sha256 the index names, and that
  the twin holds the same tar / the same zip entries (names, CRCs, sizes, modes) as the original.
- The old release-index code takes releases.json and refuses releases-v2.json (`packed: gzip, …`).
- TetherQS fetches releases-v2.json, downloads `TetherQS-0.9.31.exe.xz` (not .gz, not the exe) and
  stages exactly the published exe.
- With Manage switched to releases.json only (marker file), 0.9.32 goes into v1 only: v1 is one
  serial ahead of the stale v2, TetherQS takes v1 and the .gz copy.
- `scripts/release-index-v1.sh` against these relays deletes releases-v2.json on both (404 at
  once), TetherQS updates to 0.9.33 from releases.json; `--on` and the next publish write v2 again
  (same serial) and TetherQS takes the xz copy of 0.9.34.

On the build Mac (macbuild job, 2026-11-09) the two .xz copies from this run were unpacked with
Python's lzma, the TetherQS twin extracted with `ditto -x -k`, the Tether twin with `tar -xzf`:
`codesign --verify --deep --strict` passes for both, the designated requirement is the one of the
regular builds (`certificate root = H"bcf53642…"`), both are `x86_64 arm64`, and TetherQS starts.

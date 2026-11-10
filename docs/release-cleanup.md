# Cleaning up old releases on the relays (design)

Status: **built** (branch `prune`, 2026-11-07; not deployed yet): relay endpoint, `DELETE` guard,
prune log and metrics in tether-api; the Clean up sheet, settings and auto clean-up in Tether
Manage. Decisions: §9 ("Decided 2026-11-07"); where they differ from the first design, the text
below is updated. Problem: every publish adds a version to every relay and nothing
ever removes one. Today the only way to remove them is the trash button per release on
Releases › "Releases on the first relay", one confirmation per version per click. That is 86
versions on each of two relays, and 91 % of the release bytes are versions no device can be offered any
more.

## 1. Current state

### 1.1 What is on the relays (measured 2026-11-07, read-only)

Both relays hold the same files (channel `stable` only). The sizes differ by a few bytes
because each relay's `latest.json` names that relay.

| Group (`stable/<app>/<platform>`) | Versions | Files | Bytes |
| --- | --- | --- | --- |
| customer/darwin-universal | 19 (0.5.0 … 0.5.24) | 39 | 321 MB |
| customer/windows-x86_64 | 24 (0.5.0 … 0.5.24) | 50 | 852 MB |
| technician/darwin-universal | 19 (0.5.0 … 0.5.24) | 77 | 796 MB |
| technician/windows-x86_64 | 24 (0.5.0 … 0.5.24) | 49 | 289 MB |
| **Total per relay** | **86** | **217** (incl. `releases.json`, `index.html`) | **2.26 GB** |

| Relay | Disk | `download/` | Rest of `/var/lib/tether-api` |
| --- | --- | --- | --- |
| Hetzner 46.224.17.114 | 38 GB, 6.5 GB used (18 %) | 2.26 GB | 0.5 MB (audit 455 KB) |
| Oracle tether-desk2 | 30 GB, 11 GB used (37 %) | 2.26 GB | < 10 KB |

What is protected today:

- `stable/releases.json` (signed index, serial 8): all four entries are **0.5.24**, rollout
  "Standard" started today (`rollout.start` 2026-11-07), not halted, no `minimumVersion`.
- Every `latest.json`: 0.5.24.
- `download/index.html` (download page) links the 0.5.24 files on both relays.
- `cdn-assets.json` on the admin PC: only 0.5.24 (the CDN was switched on with 0.5.24).

The rule in §2 (keep the published version and one before it) keeps 0.5.23 + 0.5.24 per group
and frees **2.05 GB per relay (190 of 217 files, 91 %)**; 204 MB stay.

Devices seen in the last 180 days (Hetzner 49, Oracle 3) run 0.3.0 … 0.5.24. That does not
matter for retention: an old device updates to the version the index or `latest.json` offers,
never to its own version or one in between (§2.2).

Disk is not tight (the `DiskAlmostFull` alert fires at 85 %, uploads are refused below 2 GiB
free). The cost is mostly clutter in Manage and in backups. At about 100 MB per round of four
builds, 30 GB would last a long time, but the Oracle disk is already at 37 % for other reasons.

### 1.2 Code

**tether-api** (`server/api/src/main.rs`)

- Layout `download/<channel>/<app>/<platform>/<file>` (`docs/api.md:178-196`), path checks in
  `release_path` (`main.rs:477-485`), `valid_channel` / `valid_segment` (`main.rs:464-475`).
- Admin routing: `admin()` (`main.rs:801-815`) handles `PUT`/`DELETE /api/admin/release/<channel>/<path>`
  before the generic table; every other admin route needs an entry in the `limit` match
  (`main.rs:816-827`, `LIMIT_SMALL` = 4 KiB, `main.rs:48`).
- `delete_release` (`main.rs:1155-1173`): removes **any** file under `download/`, with no check.
  It deletes the published artifact, `latest.json` or even `releases.json` (then it also forgets
  the index serial, `main.rs:1161-1163`). It bumps the release generation for `latest.json` and
  `releases.json` (`bump_generation`, `main.rs:1144-1153`) and removes empty folders.
- `list_releases` (`main.rs:1175-1194`): every file with `{channel, path, size, modified}`.
- `put_release` (`main.rs:1109-1132`): the signed index is verified (`release::Index::open`) and
  its serial must go up; `index_meta` (`main.rs:264-269`) reads serial and expiry back.
- `maintain()` (`main.rs:1220-1251`) runs every 60 s (`main.rs:1356-1359`) and already applies
  retention to devices (180 days), nonces, diagnostics (7 days) and the session audit log. It does
  not touch releases.
- No admin action log: `audit.rs` stores only device-posted session records (`conn`, `file`,
  `alarm`, `audit.rs:116-200`); admin writes leave nothing except the journal.
- `/q/<token>` custom apps serve the **current** `TetherQS-<v>.exe` from the channel's
  `customer/windows-x86_64/latest.json` (`docs/api.md:257`) and keep one prepared copy per app
  in `qcache/` outside `download/` (`qcache`, `qcache_drop`, `main.rs:1073-1107`).
- Metrics: `tether_api_data_bytes` = size of the data dir (`metrics.rs:143,152`,
  `docs/monitoring.md:124`); `DiskAlmostFull` at 85 % (`docs/monitoring.md:217`); the runbook
  already points at `download/` (`docs/monitoring.md:335`).

**Tether Manage** (`apps/manage`)

- `Releases.tsx:541-602`: "Releases on the first relay", one row per release
  (`groupReleases`, `lib/releases.ts:26-44`), a trash button per release (`removeGroup`,
  `Releases.tsx:402-410`) and per file (`remove`, `Releases.tsx:412-420`). Each is a
  `DELETE /api/admin/release/...` per file through `admin` → `App::call`, which sends writes to
  every relay (`main.rs:76-111`). Removing one mac technician release = 4 files × 2 relays = 8
  signed requests and one dialog.
- Bug found on the way: the file-name regex `NAME` (`lib/releases.ts:23`) does not match
  `TetherQS-<v>.exe.gz`, so the gzip copy shows as its own row without a version and without a
  delete button, labelled "latest.json (what devices get)" (`Releases.tsx:567`).
- `Overview` / `releases_overview` (`Releases.tsx:109`, `main.rs:1203-1244`): per group the
  highest uploaded version and each relay's published `latest.json` version.
- `release_files` (`main.rs:808-819`): the files of one release: artifact, `.sig`, `.gz`
  (customer Windows, `release::wants_packed`), mac technician `.dmg` + `.sig`.
- `release::NAMES`, `build_of`, `version_of` (`src-tauri/src/release.rs:36-71`): name ↔ version,
  Manage-only today.
- `publish_upload` (`main.rs:884-954`): files to every relay, then the CDN, then the signed index
  (`publish_index`, `main.rs:1001-1017`), then `latest.json`. Between the first and the last step
  the relays hold a version **newer** than the published one.
- `cdn_deploy` (`main.rs:964-985`) and `cdn::next_assets` (`cdn.rs:76-98`, `KEEP = 2`,
  `cdn.rs:27`): the CDN holds the current and previous version of every group. Each deploy
  uploads the whole asset set; a file Cloudflare no longer has is **re-read from the first relay**
  (`main.rs:978-981`: "the relays keep every release file"). Deleting a version that is still in
  `cdn-assets.json` therefore breaks the next CDN publish.
- `release_sync` (`main.rs:1246-1319`): copies a published release to relays that lack it, read
  from a relay that publishes it.
- `page_render` / `page_files` (`main.rs:1322-1366`): the download page links the files of the
  version in `latest.json` **at render time**. It is re-rendered by "Publish the latest builds"
  (`Releases.tsx:251-262`) and the Download page; after a single "Sign and publish" it can still
  link the previous version.

**Update docs**

- Halt does not roll back: devices that installed stay, recovery is **roll forward**
  (`docs/update-design.md:370-374`, `docs/updates.md:79-82`). Nothing in the client ever
  downloads a version below the published one (floor, Tauri's "only newer").
- A device downloading re-checks the index right before installing
  (`docs/update-design.md:371-372`); the file it downloads is the one the index names.
- `minimumVersion` / policy `update.minimum-version` (`docs/updates.md:94,108`) block sessions on
  versions below it; the fix for a blocked device is the **published** version.
- Policy pins a channel (`update.channel`, `crates/tether-trust/src/policy.rs:152`), never a
  version.

## 2. Retention rule

Per relay, per `<channel>/<app>/<platform>` folder (every channel, `test` and others included).
A **version** is the set of files `release_files` lists for it (artifact, `.sig`, `.gz`, `.dmg`,
`.dmg.sig`). A version is **kept** if any of these holds; the reason is shown in the plan:

| # | Keep | Why |
| --- | --- | --- |
| K1 | the version of this group in the channel's `releases.json` (verified with the key it names, as on startup: an index from a rotated-out key still protects), halted or not, at any rollout percent; an unreadable `releases.json` or `latest.json` keeps the whole folder | what index clients download now |
| K2 | the version in the folder's `latest.json` | legacy clients, `/q/` custom apps, `page_render` |
| K3 | every version **above** K1/K2 | a publish in progress (files are uploaded before the index), or a failed publish the admin wants to retry or `release_sync` |
| K4 | the newest `keep` versions below the published one (default **2**, 0-20; decided 2026-11-07) | a device that read the old index a second before a publish, manual reinstall, the CDN's "previous" |
| K5 | versions linked from `download/index.html` | a download page that was not re-rendered after a publish |
| K6 | versions the request names in `pin` | Manage adds every version in `cdn-assets.json` and the versions the admin unticked |
| K7 | files modified in the last 24 h | belt and braces against races with a parallel publish from another admin PC |

Everything else goes: whole versions only (never half a release). Files the name table does not
recognise (`latest.json`, `releases.json`, `index.html`, anything unknown) are never touched.

### 2.2 What does not need a rule

- **Rollback / halt**: there is no downgrade path, so the previous version is kept for margin
  (K4), not because halt needs it.
- **Devices mid-download**: they download the K1 version (or, during the publish switch, the
  K4 one). Both are kept. A 404 just makes the client retry at its next check.
- **minimum_version**: devices below it need the published version (K1/K2), not one at or above
  the minimum. Keeping everything ≥ minimum would keep nothing extra in practice and is dropped.
- **Pinned versions per policy**: do not exist (only channels). The pin list (K6) covers the
  admin's own wishes per run; a stored pin list is not proposed.
- **Custom apps**: use the K2 exe; their `qcache/` copy is outside `download/` and already
  replaced on change (`qcache_drop`). Out of scope.
- **Download page assets** (`index.html`): never deleted (unknown name); K5 keeps what it links.

## 3. Who executes it: options

| | A. Multi-select bulk delete in Manage | B. Signed `POST /api/admin/releases/prune` (dry run + execute), run from Manage | C. Automatic timer in tether-api |
| --- | --- | --- | --- |
| Server change | none | one endpoint, shared name table | B plus config and a timer |
| Requests | one `DELETE` per file per relay: 190 × 2 = **380 signed requests** today, each persisting `nonces.json` | **2 per relay** (plan, execute) | none |
| Guard against deleting the published version | client only; `DELETE` stays unguarded | **on the relay**, recomputed at execution | on the relay |
| Knows `cdn-assets.json` | yes | yes (Manage sends it as `pin`) | **no** (it lives on the admin PC) → can break the next CDN publish unless `cdn_deploy` tolerates missing files |
| Review before deleting | yes | yes (dry run = the exact plan) | no |
| Effort | ~1 day | ~2.5 days | +0.5-1 day on top of B |

**Recommendation: B**, with the plan reviewed in one sheet in Manage, plus an optional
"also clean up after each publish" switch in Manage (same endpoint, `keep` from settings, run at
the end of `publish_upload`) instead of a server timer. C is not recommended: the relay cannot
see the CDN asset list and the admin never sees what went.

The existing per-release trash button stays, but goes through the same guard (§4.3).

## 4. Design

### 4.1 Shared name table

Move `NAMES`, `build_of`, `version_of`, `dmg_name`, `wants_packed` and the file list of
`release_files` from `apps/manage/src-tauri/src/release.rs` into `crates/tether-trust/src/release.rs`
as `release::file_version(app, platform, file) -> Option<String>` (artifact, `.sig`, `.gz`, `.dmg`,
`.dmg.sig`) so the relay and Manage group files the same way. `lib/releases.ts:23` gets the `.gz`
suffix (fixes the stray row) and a test.

### 4.2 Endpoint

`POST /api/admin/releases/prune` (signed request, body ≤ 64 KiB, new `LIMIT_PRUNE`).

Plan (dry run):

```json
{"dry_run": true, "keep": 2, "pin": [{"channel":"stable","dir":"customer/windows-x86_64","version":"0.5.23"}]}
```

Answer:

```json
{"keep_older":2,"groups":[{"channel":"stable","dir":"customer/windows-x86_64","published":"0.5.24","index_serial":8,
  "keep":[{"version":"0.5.24","files":[…],"bytes":69797888,"why":["index","latest.json","download page"]},{"version":"0.5.23","files":[…],"bytes":…,"why":["previous"]}],
  "drop":[{"version":"0.5.22","files":["TetherQS-0.5.22.exe","TetherQS-0.5.22.exe.sig"],"bytes":34898944}]}],
 "drop_bytes":2054613901,"drop_files":190,"kept_bytes":204116694}
```

Execute:

```json
{"dry_run": false, "keep": 2, "pin": [...], "delete": [{"channel":"stable","dir":"customer/windows-x86_64","version":"0.5.22"}]}
```

- The relay **recomputes** the plan under the app lock and deletes only versions that are both in
  `delete` and in its own `drop` list. Anything else is answered per version as
  `{"version":…, "result":"kept", "why":[…]}` (protected now, e.g. a publish happened in between)
  or `"gone"` (already deleted: idempotent, safe to repeat). Answer: `deleted`, `kept`, `gone`,
  `failed` lists and `freed_bytes`.
- `keep` 0-20, default 2 when absent (400 otherwise; decided 2026-11-07); `delete` must be explicit (no "delete everything in the
  plan" shortcut), so an execute without a reviewed plan deletes nothing.
- Files are removed with the same code as `delete_release` (empty folders go, `bump_generation`
  is not needed: neither `latest.json` nor `releases.json` changes).
- One journal line per run (`release prune: 78 versions, 190 files, 2054613901 bytes deleted; 0
  kept, 0 gone, 0 failed (keep 2)`; the signing key is not named, `auth` does not return it), and a
  JSON record in `data/release-prune.jsonl` (last 100 records: executes and forced deletes; dry runs
  are not logged) with the deleted versions, so "what went when" survives log rotation.
  `GET /api/admin/status` gains `last_prune {time, files, bytes}`.
- Signed request as every admin call (`auth`, `main.rs:633-653`): replay-safe, admin key only.

### 4.3 Guard on the existing `DELETE`

`delete_release` refuses (`409 "published release: …"`) to delete a file of the K1 or K2 version,
`releases.json` or any `latest.json`, unless the query has `?force=1` (which Manage's per-release
and per-file buttons send only after a second, explicit warning, "Delete anyway"). Forced deletes
get a journal line and a `release-prune.jsonl` record. This closes the footgun in §1.2 for every
caller. (Built in `server/api/src/prune.rs`, `delete_guarded`.)

### 4.4 Manage

New command `release_prune(keep, pins, delete: Option<…>)` in `main.rs` next to `release_sync`:

1. Read `cdn-assets.json` (`main.rs:967`) and add its versions to `pin`.
2. Dry run on **every** relay (`app.servers()`), merge per channel/group/version: a version is
   offered for deletion only if every relay that has it would drop it (one relay's protection
   wins, so the relays end up consistent and nothing the other one still publishes is lost).
   If the index serials differ between relays, the sheet says so and offers "Sync first"
   (`release_sync`), because a lagging relay protects its older version (safe, but it keeps it).
3. Compute the plan again and keep of the selection only the versions it still offers
   (`still_offered`: the sheet's list is a request, not the rule). Execute with those on every
   relay, one `RelayResult` per relay (`fan_out`
   pattern, `main.rs:122-130`), then reload the list and the overview.

UI on the Releases page, header of "Releases on the first relay" (`Releases.tsx:539-549`):
button **Clean up old releases…** opens a sheet (same component family as the publish review):

- Top line: "Frees 2.05 GB on each of 2 relays (190 files, 78 versions). Keeps 8 versions."
- "Keep [2] older version(s) besides the published one" (number field, 0-20), re-runs the plan.
- Grouped by channel › app › platform: kept versions greyed with their reasons
  ("published", "in the release index, rollout 10 %", "on the CDN", "download page", "newer
  than published", "uploaded in the last 24 h"); versions to delete with a checkbox (ticked),
  version, files, size. A "select all / none" per group and for the whole sheet.
- Per relay column when the relays differ ("only on Hetzner").
- **Delete 78 versions** (destructive button, one confirmation). Results per relay, as the
  publish sheet shows them; the list refreshes.
- Settings (Releases, above "Releases on the first relay", `settings.prune {keep, auto}`): "Keep
  [2] older version(s)" and "Clean up after each publish" (off by default). When on,
  `publish_upload` runs the prune (execute with the merged plan's offer) after a successful publish
  and adds a "Cleanup" line to the results. A relay that does not answer the dry run stops the
  clean-up (nothing is deleted anywhere). Decided 2026-11-08: this stays so even though each relay
  only deletes what it would drop itself; the merged offer needs every relay's plan, and deleting
  on the reachable relays alone would leave them different. The plan of the relays that answered
  is still shown (`blocked` lists the silent ones, their versions `withheld`), and an execute
  returns one result per relay.

The mock (`lib/mock.ts`) gets a canned plan so the sheet works in the browser preview.

### 4.5 CDN

The CDN is never changed by the prune: its assets are a subset of what is kept (K6 pins every
`cdn-assets.json` version, and `next_assets` already keeps only two per group). Optional hardening
(0.25 day): in `cdn_deploy`, when an older asset is missing on the relay, drop it from the next
asset set instead of failing the publish (`main.rs:978-981`).

## 5. Metrics and alerts

- New on `/metrics`: `tether_api_release_bytes{channel}` and `tether_api_release_versions{channel}`
  (computed in the existing `dir_bytes` walk), `tether_api_release_prune_last_timestamp`,
  `tether_api_release_prune_freed_bytes_total`.
- Grafana "Releases" row: release bytes per relay next to data size.
- Built: `tether_api_release_versions{channel,app,platform}` (per folder rather than per channel),
  `tether_api_release_bytes{channel}`, `tether_api_release_prune_runs_total`,
  `tether_api_release_prune_freed_bytes_total`, `tether_api_release_prune_last_timestamp`; Grafana
  API › Releases: "Release files on disk" and "Versions in the largest release folder".
- Alert `ReleasesPileUp` (info): `tether_api_release_versions > 40` for 1 day, "Clean up old
  releases in Manage": **not built** (optional). `DiskAlmostFull` stays as is; its runbook
  (`docs/monitoring.md`) points at the new button.

## 6. Tests

- **Unit, tether-trust**: `file_version` for every name form, prerelease versions
  (`0.5.4-qa`), unknown files; plan function `prune_plan(files, index, latest, page, keep, pins,
  now)` as a pure function over a file list: K1-K7 each, halted entry, missing index (legacy
  channel with only `latest.json`), index and `latest.json` disagreeing, version newer than
  published, `keep = 1/3`, nothing to delete.
- **Unit, Manage**: merge of two relay plans (one relay protects → not offered), CDN pins from
  `cdn-assets.json`; `lib/releases.test.ts`: `.exe.gz` is grouped with its release.
- **Relay integration** (`server/api/tests/api.rs`, next to `releases()` at line 326): publish
  three versions with a signed index, dry run lists the oldest, execute deletes it, a second
  execute answers `gone`; execute naming the published version answers `kept`; unsigned or
  replayed request 401; `keep: 0` 400; `DELETE` of the published artifact 409 without `force`.
  Runs in `scripts/test-relay.sh`.
- **End to end** (`apps/desktop/e2e/update-e2e.mjs`): after the existing publish/halt/resume
  steps, run the prune through Manage with `keep = 1`, then check that the client still verifies
  and installs the published version, the old client (0.5.23) still updates, and the next
  publish with the CDN on still deploys.

## 7. Migration and rollout

- Relay first (endpoint, guard, metrics; old Manage keeps working: it does not call the endpoint,
  and its per-file delete of a published file now gets a 409 with a clear message), then Manage.
- Manage against a relay without the endpoint (404): the button says "Update the relays to
  0.5.25 first" and does nothing.
- First run on production: dry run, compare with §1.1 (expected 190 files, 2.05 GB per relay),
  then execute; `relay-ha.md` gets a line. Backups of `/var/lib/tether-api` shrink accordingly.

## 8. Effort

| Part | Days |
| --- | --- |
| Shared name table and plan function in tether-trust, with unit tests | 0.5 |
| tether-api endpoint, `DELETE` guard, prune log, metrics, integration tests | 0.75 |
| Manage command (merge, CDN pins), sheet, mock, `.gz` grouping fix | 1 |
| Auto-cleanup after publish (setting + call) | 0.25 |
| e2e, docs (`api.md`, `updates.md`, `monitoring.md`), Grafana panel, alert | 0.5 |
| **Total** | **~3** |

## 9. Decided 2026-11-07

| # | Question | Decision |
| --- | --- | --- |
| C1 | Executor | **B**: the signed relay command `POST /api/admin/releases/prune` (dry run returns the plan; execute with the explicit list; the relay recomputes and deletes only what is still unprotected; idempotent) **plus** the Tether Manage sheet "Clean up old releases…" (grouped by channel/app/platform, kept versions greyed with reason, checkbox per version, select all, totals, one confirmation, results per relay; a version is offered only if every relay that has it would drop it; Manage pins every version in its `cdn-assets.json`) |
| C2 | Older versions kept besides the published one | default **2**, configurable on Manage's Releases page, sent as `keep`; the relay accepts 0-20 and uses 2 when absent |
| C3 | Clean up automatically after each publish | possible, a switch in Manage, **off** by default (same prune with the configured `keep`, no prompt, result in the publish results) |
| C4 | Guard the existing `DELETE` | yes: `409` for the published artifact (in `latest.json` or the signed index), `latest.json` and `releases.json`, unless `?force=1`; Manage asks for an explicit confirmation before sending force |
| C5 | Keep files uploaded in the last 24 h (K7) | yes |
| C6 | Prune log on the relay | `release-prune.jsonl` (also forced deletes) in the data dir plus a journal line per action |

Also fixed: `lib/releases.ts` showed `TetherQS-<v>.exe.gz` as a row of its own; it now belongs to
its version.

### Notes for parallel work

- **Download page templates** (Manage › Download page): K5 keeps every version whose file path
  `/<channel>/<app>/<platform>/<file>` appears anywhere in `download/index.html`. Templates that
  keep linking release files with that path (relative or absolute, relay or CDN host) need nothing
  else; a template that links files some other way should still contain that path, or the linked
  version is protected only by K1/K2/K4.
- **Custom apps** (`/q/`): use the `latest.json` exe (K2), which is never deleted; `qcache/` is
  outside `download/` and untouched.

### Deploy

1. Relays (each): build with `scripts/build-relay.sh`, install the new `tether-api`, restart it.
   Old Manage keeps working (it never calls the endpoint; its per-file delete of a published file
   now gets the 409).
2. Tether Manage: the new build. Against a relay without the endpoint the sheet says "update its
   tether-api first" and deletes nothing.
3. First production run: open the sheet, compare with §1.1 (keep 2 also keeps 0.5.22, so about
   100 MB less than the 2.05 GB per relay of keep 1), then delete; add a line to `relay-ha.md`.

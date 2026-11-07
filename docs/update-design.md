# Update process: design for faster, safer updates

Status: **implemented** on branch `updimpl` (2026-11-07), not yet released or deployed. The owner's
decisions are in [Decisions](#decisions-needed) ("Decided 2026-11-07"); the status of each step is
in [§8](#8-plan-and-effort). D1 changed after the decision: no R2 (it needs billing details);
downloads go to **Cloudflare Workers Static Assets** instead (§5.6). Section 1 describes 0.5.23,
before this work.

Short version: a new release reaches online devices in **under 2 minutes** instead of up to
6 hours. Three things make that work:

- a **push hint** over the hbbs WebSocket that every device already holds;
- a **zero-cost fallback hint** in the heartbeat answer every device already gets every 15 minutes;
- a **signed release index** that fixes W9 and adds staged rollout, halt and expiry.

None of this needs a new server or a new long-lived connection. Moving downloads to a CDN is
optional: with a staged rollout and compressed TetherQS downloads, the relays can carry 10 000
devices.

## 1. Current state (verified in code, 0.5.23)

### 1.1 Clients

| What | Where | Behaviour |
| --- | --- | --- |
| Technician check loop | `apps/desktop/src-tauri/src/update.rs:194-209` (`start`) | First check 5 s after start, then waits `update.check-hours` (default 6, min 1) on a `Condvar`. `on_policy_change` (`:189-192`) wakes it early. No jitter. |
| Technician check | `update.rs:128-145` (`check`) | `GET /download/<channel>/technician/<target>/latest.json` through `policy::api` (first host that answers, 443 then 80, proxy then direct). 200 → parse, 404 → "no update". No `If-None-Match`. The result is cached in `update-state.json`, so every process role enforces it offline. |
| Technician install | `update.rs:147-186` (`install`) | **Fetches the manifest a second time** through `tauri-plugin-updater` (`endpoints` = every API host). `download_and_install` checks the minisign signature against `TETHER_UPDATER_PUB_KEY`, runs NSIS passively (UAC prompt for non-admins), and restarts. |
| Session blocking | `update.rs:82-107` | `minimumVersion` / `mandatory` come from the **unsigned** manifest and block outgoing and incoming sessions. |
| TetherQS check loop | `apps/quick/src-tauri/src/update.rs:372-397` (`spawn`) | At start, then every `update.check-hours`; after a failed check, again in 15 min. Not woken by policy changes. |
| TetherQS check | `quick/update.rs:244-321` | Tries every relay in turn; **the first one with a release wins**, not the highest version. A forced update installs after `update.restart-seconds` **even during a running session** (`:306-315`; no idle check). |
| TetherQS install | `quick/update.rs:323-370` | Downloads at most 200 MiB over https only, checks the minisign signature (`verify`, `:87-100`), swaps the exe and relaunches. The URL comes from the unsigned manifest. |
| Policy poll (both apps) | `apps/desktop/src-tauri/src/policy.rs:411-494`; `apps/quick/src-tauri/src/policy.rs:216-293` | Every `policy.refresh-minutes` (default **15**; 1 with `policy.fast-apply`): `GET /api/server-keys` and `GET /api/policy`, both with `If-None-Match: "<serial>"` → 304. Then `POST /api/heartbeat` (uuid, id, profile, **version**, os, policy serial), answered `204` (`server/api/src/main.rs:627-659`). Heartbeats go only to the first API host that answers. |
| How a policy change reaches clients | same | **Polling only**, every 15 min. If the bundle changed, the technician UI wakes its update checker (`policy.rs:484`). There is no push. |

How a policy change reaches clients: the client polls every `policy.refresh-minutes` (15 min), and
a newer bundle wakes the update check. A policy publish therefore reaches every device in ≤ 15 min
(≤ 1 min with `fast-apply`).

### 1.2 Servers

- Caddy serves `/download/*` from `/var/lib/tether-api` and sends `latest.json` with
  `Cache-Control: no-cache` (`server/caddy/Caddyfile:89-94`). Caddy has **no** limit on parallel
  downloads. The nginx front (`server/nginx/nginx.conf:66-71, 167-183`) caps downloads at
  32 × 512 KB/s per node; `latest.json` is never limited.
- tether-api writes the files through `PUT /api/admin/release/<channel>/<path>`
  (`main.rs:340`, `put_release` `:1026`). It stores signed documents with `ETag: "<serial>"` and
  answers 304 (`get_signed`, `:611-625`). This pattern already exists for policy and the server key
  list.
- Tether Manage signs each artifact with minisign, using the trusted comment
  `timestamp:<unix>\tfile:<name>\tversion:<version>` (`apps/manage/src-tauri/src/keys.rs:191`).
  It writes one `latest.json` per relay, each pointing at itself (`release.rs`, docs/management.md
  "Releases").
- hbbs keeps every registered WebSocket host in `TCP_PEERS` with a send channel (`peer_tx`)
  (`vendor/rustdesk-server/src/rendezvous_server.rs:470-503`), and pings it with an empty frame
  every 20 s (`:1436-1450`). It already pushes one thing unasked: `ConfigUpdate` when its serial is
  higher than the client's (`:492-499`). Clients do not resend `RegisterPeer` over TCP/WS once
  their key is confirmed; they ping every second (`vendor/rustdesk/src/rendezvous_mediator.rs:665-713`).
  On the technician app the mediator runs in the `--server` process (the service, if installed).
  The UI mirrors that process's options over IPC **every second** (`vendor/rustdesk/src/ui_interface.rs`,
  `check_connect_status_`).

### 1.3 Measured today (2026-11-07, against the live relays, read-only)

| | tether-desk | tether-desk2 |
| --- | --- | --- |
| TLS handshake (TLS 1.3, ECDSA P-256), server → client | 4 866 B | 4 871 B |
| `latest.json` body | 771 B | 772 B |
| `ETag` / 304 on `If-None-Match` | yes (`"dmp2gffrgv20lf"`), **304 = 215 B headers** | yes (other ETag: mtime based) |

So Caddy already answers 304s. Clients just never ask. `policy::api` builds a new `reqwest`
client for every request, so **each check pays a full TLS handshake**. That handshake is 85 % of
the bytes, which means a 304 saves only about 14 % per check.

Artifact sizes (`dist/`, 0.5.10-0.5.23): NSIS installer 12.0 MB, TetherQS.exe **34.9 MB**
(uncompressed PE), macOS `.app.tar.gz` 16.8 MB, TetherQS mac zip 13.1 MB. Compressing
TetherQS-0.5.11.exe (measured): gzip -9 → **14.6 MB** (42 %), xz -9 → **9.3 MB** (27 %).

### 1.4 Weaknesses in the current process

1. **Latency**: up to `check-hours` (6 h) plus restart time. TetherQS ignores policy changes.
2. **W9** (docs/encryption.md §11, SECURITY.md): the manifest is unsigned, which opens three attacks:
   - **Rollback**: a relay (or anyone who can write `download/`) can label an older *signed*
     binary with a higher `version`. The binary signature does not cover the version, because
     neither client checks the trusted comment.
   - **Blocking**: `minimumVersion: 99` stops all sessions on every device.
   - **Freeze**: hiding a release, e.g. a security fix.
3. **TOCTOU** in the technician app: the version you check is not the version you install.
   `install` fetches the manifest again, possibly from another relay.
4. **No staged rollout** other than the manual `test` channel, and **no halt**: once a bad
   `latest.json` is up, every device that checks installs it.
5. **Download herds**: a forced update or a policy change sends all devices within 15 min. At
   10 000 × 35 MB, docs/scaling.md computes 3.1 Gbit/s for 15 min, which saturates a 1 Gbit node
   that is also relaying sessions. Caddy (today's front) has no download cap.
6. **Sessions can be interrupted**: a forced TetherQS update restarts in the middle of a session.

## 2. How others do it (research, 2026)

| Product | Check interval | Push? | Staged rollout | Notes |
| --- | --- | --- | --- | --- |
| Chromium updater (Omaha 3.1) | 4.5 h base (rounded up to the hour); with 10 % chance 120 % of it; random start delay ≤ 1 min | no, client-driven | server-side gradual rollout; enterprise policy "fast / default / slow" join point | Spec: the interval should be **coprime with 24 h**, never fixed clock times. Server can send `X-Retry-After` (≤ 24 h). Differential updates: still "TODO" in the new updater. [functional spec](https://chromium.googlesource.com/chromium/src/+/main/docs/updater/functional_spec.md), [protocol 3.1](https://chromium.googlesource.com/chromium/src/+/main/docs/updater/protocol_3_1.md) |
| Firefox | background task every 7 h (`app.update.background.interval`) | update checks poll; **Remote Settings uses Megaphone**: a version *broadcast* over the existing Push WebSocket (autopush), after which the client fetches signed data | Balrog "throttled rollout" (for example 25 %, then up), background download pacing | Megaphone works like the hbbs hint proposed below: a tiny version number on an existing connection, and the content stays signed and fetched separately. [Balrog throttling](https://hearsum.ca/posts/streamlining-throttled-rollout-of-firefox-releases/), [BackgroundUpdates](https://firefox-source-docs.mozilla.org/toolkit/mozapps/update/docs/BackgroundUpdates.html), [Megaphone](https://github.com/mozilla-services/megaphone), [autopush-rs](https://github.com/mozilla-services/autopush-rs) |
| VS Code | 30 s after start, then **every 1 h**; immediately on resume | no | server-side | `abstractUpdateService.ts` |
| Electron `update-electron-app` | at start, then every 10 min (default) | no | – | [Electron docs](https://electronjs.org/docs/latest/tutorial/updates) |
| Sparkle (macOS) | default 1 day, **minimum 1 h** "to avoid overloading servers" | no | **phased rollout: 7 random groups, `phasedRolloutInterval` apart**; critical updates skip the phases | EdDSA-signed archives; optional **signed feed** (`SURequireSignedFeed`). [publishing](https://sparkle-project.org/documentation/publishing/), [customization](https://sparkle-project.org/documentation/customization/) |
| Tauri updater v2 | none prescribed | no | none built in | Static JSON or dynamic server (204 = no update). **Only artifacts are signed, not the manifest.** No delta updates. `Update` exposes `version`, `download_url`, `signature`, `raw_json`, `download()` → bytes, `install(bytes)`. [plugin docs](https://v2.tauri.app/plugin/updater/), [docs.rs](https://docs.rs/tauri-plugin-updater/latest/tauri_plugin_updater/struct.Update.html) |
| RustDesk upstream (vendored) | first check after 30 s, then **daily**, minimum 10 min; **30 min retry while a session is active** | no | none | `vendor/rustdesk/src/updater.rs:85-94`: upstream already defers installs during sessions |
| TeamViewer | **weekly** by default, admin policy sets frequency / auto-install | no | – | [support article](https://www.teamviewer.com/en/global/support/knowledge-base/teamviewer-remote/update/update-teamviewer/) |
| AnyDesk, Slack, Discord | no authoritative source found | | | not used in this design |
| TUF (The Update Framework) | – | – | – | Signed metadata with **expiry** (indefinite-freeze), **version numbers that only go up** (rollback), one consistent snapshot (mix-and-match), length/hash limits (endless data), protection against inflated versions (fast-forward). [security](https://theupdateframework.io/docs/security/) |

What this means for Tether: nobody polls below about 10 min, and the large vendors use 1-7 h. The
pattern built for "fast and cheap" is Megaphone's: a broadcast version number on a connection that
already exists, followed by a signed fetch. Staged rollouts with a stable random bucket per device,
plus a bypass for critical updates, are the norm (Sparkle, Balrog, Chromium).

## 3. Requirements

| # | Requirement | Target |
| --- | --- | --- |
| R1 | Detection latency | **95 % of online devices hold the new release index within 2 min** of publish, with the hbbs hint. Within 15 min when only the heartbeat hint works. Within `check-hours` when both fail. |
| R2 | Halt latency | 95 % of online devices stop a halted rollout within 2 min. A device that is downloading re-checks before installing. |
| R3 | No new servers, no new persistent connections | Steady-state extra load on a relay < 1 req/s at 10 000 devices. Release burst < 400 req/s for < 1 min. tether-api has been measured at 1 500 req/s (docs/scaling.md). |
| R4 | Downloads never starve relayed sessions | ≤ 130 Mbit/s per node from relay downloads (nginx cap), enforced by the rollout pace. Or move the downloads to a CDN. |
| R5 | Security | Metadata signed (fixes W9). Binary signed and **bound** to the signed metadata (sha256 + trusted comment). Rollback floor that only goes up. Freeze detection. A relay can no longer block sessions or downgrade. |
| R6 | Never interrupt a running session | Download in the background, install when idle. Mandatory blocks *new* sessions only. |
| R7 | Compatibility | Clients up to 0.5.x keep updating through `latest.json`. Works through TLS-inspecting proxies, proxies that strip `ETag`, and offline machines. |
| R8 | Scale | 10 000 devices / 3 000 sessions on today's nodes (docs/scaling.md). |

## 4. Options and load numbers

### 4.1 Assumptions

N = 10 000 online devices, one updater each. Every check is a new TLS connection, as today: 4.87 KB
server → client handshake (measured). Bodies: `latest.json` 0.77 KB, a signed release index about
3.3 KB (base64 payload, two apps × two platforms), 304 headers 0.22 KB. "Reuse" means a pooled
keep-alive connection with no handshake. A month is 30 days. Caddy CPU per small request is about
0.28 ms (docs/scaling.md: 1 500 req/s cost Caddy 0.42 core), so even 167 req/s is about 0.05 core.
**Bandwidth and handshakes matter more than CPU.**

### 4.2 Plain polling of the manifest (per relay that devices check)

| Interval | req/s | 200 each time | 304 (new TLS each time) | 304 on a reused connection |
| --- | --- | --- | --- | --- |
| 6 h (today) | 0.46 | 3 KB/s, 7 GB/mo | 2 KB/s, 6 GB/mo | 0.1 KB/s, 0.4 GB/mo |
| 1 h | 2.8 | 16 KB/s, 42 GB/mo | 14 KB/s, 37 GB/mo | 0.8 KB/s, 2 GB/mo |
| 15 min | 11 | 66 KB/s (0.5 Mbit/s), 170 GB/mo | 57 KB/s, 147 GB/mo | 3.3 KB/s, 9 GB/mo |
| 5 min | 33 | 197 KB/s (1.6 Mbit/s), 510 GB/mo | 170 KB/s, 441 GB/mo | 10 KB/s, 26 GB/mo |
| 1 min | 167 | 0.98 MB/s (7.9 Mbit/s), **2.5 TB/mo** | 0.85 MB/s, 2.2 TB/mo | 50 KB/s, 130 GB/mo |

For comparison, the existing policy cycle already costs 33 req/s, about 190 KB/s and 500 GB/month:
three requests per 15 min, each with a new TLS handshake.

Polling every 1-5 min is affordable on CPU but wasteful in bandwidth: 0.4-2.5 TB/month for a
manifest that changes a few times a week. It also still gives minutes of latency.

### 4.3 Options compared

| Option | Detection latency (p95) | Extra steady load (10 k) | Release burst | New infrastructure | Security | Verdict |
| --- | --- | --- | --- | --- | --- | --- |
| A. Shorter poll (e.g. 15 min) | 14 min | 11 req/s, 150-170 GB/mo | none (spread) | none | unchanged | simple, but slower than B and costs more |
| B. **Hint in the heartbeat answer** (204 → `200 {"releases":N}`) | 14 min (15-min cycle) | **0 extra requests**, +25 B per heartbeat ≈ 0.3 KB/s | ≤ 11 req/s index fetches over 15 min | none | hint unsigned, content signed | **free fallback, adopt** |
| C. **Push hint over the existing hbbs WebSocket** | **< 1 min** (jitter W = 30-60 s) | 0 (one 78 B frame per device per release ≈ 0.8 MB per node) | 10 k index fetches spread over W: **167 req/s for 60 s**, 82 MB (51 MB if most are 304) | hbbs + client patch | hint unsigned, content signed | **adopt as primary** |
| D. Separate SSE / long-poll / WebSocket channel | seconds | **+10 000 held connections per node**: Caddy ≈ 145 KB each (measured) = **+1.45 GB RAM** (nginx 41 KB = 0.4 GB) | as C | new endpoint | as C | reject: duplicates C, costs RAM the 1 GB Oracle node does not have |
| E. Manifest via CDN (Cloudflare caching `latest.json`) | as the poll interval | relay ≈ 0; CDN free | as A | **own domain** (~$10/yr) | unchanged; CDN is one more party that can serve stale data | only useful together with a signed index; does not fix latency |
| F. Binaries via CDN (decided: Workers Static Assets on `*.workers.dev`) | – | – | relay egress for releases → 0 | Cloudflare account (free plan, no domain, no billing) | binaries bound to the signed index; URL can be anywhere | **D1: adopted** |
| G. Web push / OS push (APNs, WNS) | seconds | – | – | vendor accounts, not reachable behind many corporate proxies | – | reject |

### 4.4 Download burst (the real bottleneck)

| Release | Total | Both relays at the nginx cap (2 × 16 MB/s) | One node at full 1 Gbit/s | Rollout pace 10 % of devices per hour |
| --- | --- | --- | --- | --- |
| Technician NSIS 12 MB × 10 k | 120 GB | 1.0 h | 16 min | 27 Mbit/s |
| TetherQS.exe 35 MB × 10 k | 350 GB | 3.0 h | 47 min (relaying stops) | 78 Mbit/s |
| TetherQS.exe gzip 14.6 MB × 10 k | 146 GB | 1.3 h | 19 min | 32 Mbit/s |

Fast detection only moves the herd earlier. **The rollout pace has to limit downloads**: below 10 %
per hour a release stays under 80 Mbit/s per relay, within the 130 Mbit/s budget. A CDN removes
the limit altogether. Each relay's provider includes plenty of traffic (Hetzner Cloud includes
TB-scale monthly traffic in the EU, and Oracle Always Free includes 10 TB per month; check the
current terms). A few releases a week are therefore a bandwidth-*rate* problem, not a volume
problem.

## 5. Recommended design

The design has six layers. Each one works without the ones after it.

```
 Tether Manage ──sign (admin key)──► download/<channel>/releases.json   (signed index, all relays [+ CDN mirror])
                └─sign (updater key)► artifacts (+ .sig, trusted comment binds file+version)
 tether-api: every write under download/ bumps the release generation G
   ├─ heartbeat answer   {"releases": G}                      (15-min cycle, layer B)
   └─ file release-generation ──► hbbs (stat every 5 s) ──► ReleaseHint{G} to every WS host (layer C)
 Client: hint changed → wait random 0..W s → GET releases.json (If-None-Match) → verify → rollout bucket
         → download (CDN or relay, sha256+minisign+trusted comment) → install when idle
 Fallback: full check every update.check-hours (jittered), at start, after policy change
```

### 5.1 Layer 0: bind binaries to versions (quick win, client-only)

Before installing, both apps parse the minisign signature's **trusted comment**, which the updater
key signs. They require `version:` to equal the offered version and `file:` to equal the URL's
file name. `minisign-verify` already exposes `trusted_comment()`. Tether Manage has always written
this comment (`keys.rs:191`).

In the technician app, after the plugin's `check()`, apply this to `update.signature` and
`update.version`. Then replace `download_and_install` with `download()` → sha256/trusted-comment
check → `install(bytes)`.

This closes the **rollback** half of W9 without any server change. It does not stop blocking or
freeze attacks.

### 5.2 Layer 1: the signed release index (fixes W9)

**File**: `download/<channel>/releases.json`, uploaded through the existing
`PUT /api/admin/release/<channel>/releases.json`. tether-api needs no new endpoint. Caddy and nginx
serve it statically, already with `ETag` / 304. Send it with `Cache-Control: no-cache`, like
`latest.json`.

**Envelope**: `tether_trust::envelope::Signed` (`{"payload","sig","key"}`, Ed25519) signed with the
**admin key**, the same key, the same rotation (`TETHER_ADMIN_PUB_KEY` lists several keys) and the
same code as policy and the server key list. Why the admin key and not the updater key:

- The admin key already controls `update.minimum-version` through policy, so signing release
  metadata with it adds no new authority.
- Code then needs **two** keys. Installing code needs the updater key (binary) **and** the admin
  key (metadata naming that binary). A leaked updater key alone can no longer push a binary to
  devices that require the index.
- Clients already trust it, so no new build variable is needed.

**Payload** (JSON, ≤ 64 KiB, read with `tether_trust::read_limited`):

```json
{
  "type": "tether-releases/1",
  "channel": "stable",
  "serial": 57,
  "issued": 1762527600,
  "expires": 1765119600,
  "releases": {
    "technician/windows-x86_64": {
      "version": "0.5.24",
      "file": "Tether_0.5.24_x64-setup.exe",
      "size": 11987549,
      "sha256": "9f2c…",
      "signature": "<base64 of the .sig file, as in latest.json>",
      "urls": [
        "https://dl.example.org/stable/technician/windows-x86_64/Tether_0.5.24_x64-setup.exe",
        "https://{relay}/download/stable/technician/windows-x86_64/Tether_0.5.24_x64-setup.exe"
      ],
      "notes": "…",
      "mandatory": false,
      "minimumVersion": "",
      "forced": false,
      "security": false,
      "rollout": { "start": 1762527600, "steps": [[0, 1], [3600, 10], [21600, 50], [86400, 100]] },
      "halted": false
    },
    "customer/windows-x86_64": { "…": "…" },
    "technician/darwin-universal": { "…": "…" },
    "customer/darwin-universal": { "…": "…" }
  }
}
```

- `{relay}` is replaced by the host the index came from. Manage signs **one** document that is
  valid on every relay, and "Publish to missing relays" just copies it.
- One document per channel holds every app and platform, which gives one consistent snapshot (no
  mix-and-match).
- `serial`: Manage uses `max(serial on any relay) + 1`. Clients keep a **floor** per channel
  (`releases-<channel>.serial`, written like `policy-serial` and `server-keys.serial` with
  `write_atomic`) and reject a lower serial. An equal serial means nothing changed.
- `expires` = issued + 30 days (**D3**).

**Client rules**:

1. Verify the envelope against `ADMIN_KEYS`. Check `type` and that `channel` is the requested one
   (a document from another channel is rejected). Apply the serial floor.
2. `mandatory`, `minimumVersion`, `forced`, `security`, the version and the URLs are taken **only
   from the signed index**. As soon as a device has accepted one signed index (floor > 0), it
   ignores the unsigned flags in `latest.json` for good. That fixes blocking via `minimumVersion`.
   Builds after the cut-over refuse unsigned metadata entirely (**D2**).
3. Download: try `urls` in order, https only. Read at most `size` (≤ 200 MiB, fixes endless data),
   then require `sha256`, a valid minisign signature, and trusted comment `version`/`file` equal
   to the entry.
4. **Expiry** (freeze detection), soft by default (**D3**):
   - When `now > expires`, keep using the entry. The binaries are still bound and the floor still
     holds.
   - Mark the state `stale`: show "Update information is out of date" in Help/About, send
     `update_stale: true` in the heartbeat (alert), and also ask the other relays and the CDN
     mirror.
   - A hard fail on expiry would turn "Manage not opened for 30 days" into an outage.
5. **Multiple sources**: when the hints from two relays differ, or the index is stale, fetch from
   the other relays and the CDN mirror as well. **The highest valid serial wins.** TetherQS changes
   from "first relay with a release wins" to the same rule. One compromised relay can then no
   longer freeze a device that reaches another source.

Remaining risk: an attacker who controls every source a device can reach can freeze it until
`expires` and then only causes a warning. A stolen admin key can block sessions, as it already can
through policy.

### 5.3 Layer 2: the release generation hint

tether-api keeps a **release generation** `G` (u64, persisted in `release-generation`). It
increments on every successful `PUT`/`DELETE` under `/api/admin/release/` that touches
`releases.json` or `latest.json`, and writes the value atomically to
`/var/lib/tether-api/release-generation` (mode 0644, readable by hbbs).

`G` is only a hint ("look again"). It is **unsigned on purpose**. A malicious relay can at most:

- trigger fetches, which the client limits to one hint-triggered fetch per host per 60 s;
- or withhold hints, which the fallback covers.

Each relay counts its own `G`, so clients compare `G` per host, for inequality only.

**B. Heartbeat answer (no extra requests).** `POST /api/heartbeat` answers `200` with
`{"releases": G}` instead of `204`. Old clients accept any 2xx: the technician app checks
`is_success()` (`policy.rs:467`) and TetherQS ignores the body (`quick/policy.rs:246-266`).
New clients compare `G` with the last value seen from that host and wake the update checker on a
change. Latency ≤ 15 min.

**C. hbbs push (primary, seconds).**

- Protocol: a new union member in `rendezvous.proto`, in **both** vendored `hbb_common` copies.
  The tag is high to avoid future upstream tags:
  ```proto
  message ReleaseHint {
    uint64 generation = 1;   // tether-api release generation G of this relay
    uint32 spread_secs = 2;  // client waits uniform(0, spread_secs) before fetching; clamped to 0..900
  }
  // in RendezvousMessage.union:
  ReleaseHint release_hint = 1001;
  ```
  Stock and old Tether clients ignore an unknown union member: `handle_resp` ends in `_ => {}`.
  hbbs sends it **only on WebSocket connections of registered hosts** (`/ws/id`), never on UDP.
- hbbs: a 5 s timer `stat`s `release-generation`. When the value changes, it sends `ReleaseHint`
  to every `TCP_PEERS` entry through its `peer_tx`; this is the same path signalling already uses,
  so there is no new lock pattern. It also sends the current `G` once right after a registration
  is accepted (next to the existing `ConfigUpdate` push, `rendezvous_server.rs:492-499`). After a
  reconnect storm a device therefore learns of any release it missed. Cost: about 78 B on the wire
  per device per release, **0.8 MB per node per release**.
- `spread_secs` comes from hbbs config (`RELEASE_HINT_SPREAD`, default 60). At 10 000 devices that
  gives 167 index requests/s for one minute, mostly 304s (§4.3). Raise it on small nodes.
- Client (`rendezvous_mediator.rs`, a thin hook calling a function in a Tether module):
  - store `"<host>=<G>"` in the option `tether-release-hint`.
  - On the technician app the UI already mirrors the server process's options every second.
    `update.rs` watches the mirrored value and wakes the checker after the random spread.
  - TetherQS runs the mediator in-process and wakes it directly.

**Fallback poll.** Keep the full check at start, after a policy change (now also in TetherQS) and
every `update.check-hours`. That interval gets +0-20 % jitter (Chromium) and a 60 s random start
delay. `If-None-Match` is sent per host, because each relay's ETags differ. Default stays 6 h: with
B and C it is only the last resort.

### 5.4 Layer 3: staged rollout, halt, security releases

- **Bucket**: `b = u16(SHA-256("tether-rollout" ‖ install_uuid ‖ app ‖ version)[0..2]) / 655.36`,
  which gives 0-99.99.
  - `install_uuid` already exists (`policy.rs`, heartbeat `uuid`), and TetherQS has its own.
  - Salting with the version reshuffles the cohort each release, so the same devices do not always
    go first. Pilots belong in the `test` channel, as today.
- **Rollout steps**: `steps = [[offset_s, percent], …]`, a step function over `now - start`. A
  device is eligible when `b < percent`. The ramp is signed once and runs while Manage is closed.
  - Preset "Standard": 1 % → 10 % after 1 h → 50 % after 6 h → 100 % after 24 h.
  - Preset "Fast": 10 % → 100 % after 1 h.
  - Preset "Immediately": [[0,100]].
  - At 10 % per hour, the TetherQS release in §4.4 needs 78 Mbit/s (32 with gzip).
- **Policy** `update.rollout` = `default` | `early` (treats b as 0) | `late` (waits for 100 %).
  This is the Chromium enterprise model, for admins who want their own machines first or last.
- **Halt**: Manage re-signs the index with `halted: true` (new serial). Hint, fetch, then:
  - eligible devices that have not installed stop, including a download in progress, because the
    client re-checks the index (conditional GET) right before `install`;
  - devices already on the new version are not downgraded.
  - Recovery = **roll forward** (publish x.y.z+1 with the fix or the old code). Tauri and the
    floor never install lower versions, and that is intended.
- **Security releases**: `security: true` skips the rollout steps (Sparkle's "critical"). With
  `minimumVersion` it blocks *new* sessions on older versions. The download herd is then limited
  only by the relay cap or the CDN, which is the main argument for **D1**.
- **Mandatory and forced** keep their current meaning, but now come only from signed data.

### 5.5 Layer 4: install when idle (R6)

- Download in the background as soon as the device is eligible. Install only when **no session is
  running** (incoming or outgoing, file transfer included). Check the same state that the
  connection manager and the sink already track. Upstream RustDesk does the same
  (`RETRY_INTERVAL` while a session is active).
- `forced`: the countdown of `update.restart-seconds` starts only when the device is idle. While a
  session runs, the card says "Will update after this session".
- Mandatory / minimum version block **new** sessions only. A running session is never closed by an
  update decision.
- TetherQS keeps the ID and the one-time password across the restart, as today.

### 5.6 Layer 5: offload downloads (optional, **D1**)

The index can list several URLs, so the binaries can live anywhere without trusting the host.

**Decided and built (2026-11-07): Cloudflare Workers Static Assets** on
`https://tether-dl.thomas-schend.workers.dev/<channel>/<app>/<platform>/<file>`. Free plan, no
domain and no billing details (R2 needs them; the owner declined). Requests answered by a static
asset are free and unlimited and do not count against the 100 000 Worker requests per day, as long
as the script is not run first (no `run_worker_first`; the script only answers misses with 404).
Limits: **25 MiB per file**, 20 000 files per version
([limits](https://developers.cloudflare.com/workers/platform/limits/),
[billing and limitations](https://developers.cloudflare.com/workers/static-assets/billing-and-limitations/)).
Tether Manage deploys a new Worker version on every publish through the assets upload API
(upload session with the manifest, buckets, script upload with the completion token), holding the
current and the previous release of each channel/app/platform; `server/cloudflare/worker/deploy.sh`
is the `wrangler deploy` fallback. Every update file fits under 25 MiB except the plain
TetherQS.exe (33.3 MiB): its index entry gets a `packed` gzip copy (`TetherQS-<v>.exe.gz`,
~14 MiB) whose own size and sha256 are checked before it is unpacked, then the unpacked exe like
any other; the plain exe (first downloads, old clients) stays on the relays. Manage refuses to
publish a CDN file over 25 MiB. The R2 row below is the original comparison.

| Option | Cost | Pros | Cons |
| --- | --- | --- | --- |
| **Cloudflare R2 + custom domain** (`dl.<domain>`) | domain ≈ $10/yr (Cloudflare Registrar at cost; check the live price). R2 free tier: 10 GB-month, 10 M reads, 1 M writes/month, **egress free** | No egress cost. Edge cache (max 512 MB per object on the free plan). A release is about 100 MB per version, so the last 5 versions fit in 1 GB. The signed index can be mirrored there too (`Cache-Control: max-age=60`) for freeze detection and relay-down. | Needs a domain and a Cloudflare account. `r2.dev` is rate-limited and "for development only", so it is not an option. Corporate proxies may block a new domain (clients fall back to the relay URL). |
| GitHub Releases (separate public repo) | free | No domain needed. Widely allowlisted. 2 GiB per file. | Binaries become public on github.com; they hold only public data (server names, public keys), which the download page already publishes. Manage needs a GitHub token. GitHub terms apply. |
| Bunny CDN | $0.01/GB EU/NA, $1/month minimum | Simple pull zone in front of the relays | Costs money per release (350 GB ≈ $3.50); the relays still serve cache misses |
| jsDelivr | free | – | **20 MB per-file limit** for GitHub sources, and TetherQS.exe is 35 MB. Not usable. |
| Workers | free 100 000 requests/day | – | Below the 10 k × 96 polls per day of a 15-min poll. Not needed: the files are static. |
| Stay on relays | 0 | Nothing new | Rollout pace is limited by 130 Mbit/s per node. Use **nginx** fronts (they have a cap), or add one to Caddy. |

Cheap win in any case (built): serve TetherQS.exe compressed. Caddy `file_server { precompressed gzip }`
with a `.exe.gz` that Manage uploads, and nginx `gzip_static on`. The TetherQS client sends
`Accept-Encoding: gzip`; `reqwest` with the `gzip` feature decompresses it transparently. The
signature still covers the plain exe. Result: 35 → 14.6 MB (measured). xz would be 9.3 MB, but
that needs a new dependency.

**Delta updates: not recommended now.**

- Tauri has no delta support, and NSIS and `.app.tar.gz` are already compressed, so a binary diff
  of them saves little.
- Only TetherQS (own updater, plain PE) could use a `zstd --patch-from` / bsdiff patch. That means
  a new dependency, a patch per (old, new) pair, and fallback logic, all to win perhaps 10 MB over
  gzip.
- Revisit only if the downloads still saturate after rollout and compression.

### 5.7 Failure modes

| Failure | Effect | Handling |
| --- | --- | --- |
| Relay 1 down | No heartbeat hint from it; no hbbs hint from it | hbbs hint from relay 2; index fetched from the next host (`policy::api` order); URLs list CDN and `{relay}` |
| hbbs (or tether-api → hbbs file) broken | No push | heartbeat hint (≤ 15 min); alert `ReleaseHintStale` |
| Both hints broken (old relay software) | – | fallback poll every `check-hours` |
| Cloudflare / CDN down or blocked by a proxy | Download fails on URL 1 | next URL (relay) |
| Manage not opened for 30 days | Index expires | Soft: warning + metric + more sources; Manage re-signs automatically when unlocked if < 15 days remain |
| Clock skew on a device | Rollout step early or late; false "stale" | Bounded effect only: no security decision depends on the client clock except the soft expiry. Optionally use the relay's `Date` header to warn about skew > 1 h. |
| Proxy strips `ETag` / `If-None-Match` | Always 200 | Correct, just 3 KB more |
| Proxy caches despite `no-cache` | Stale index | Serial floor and hint mismatch → retry with `Cache-Control: no-cache` request header and the other host |
| Proxy strips WebSocket frames | No hbbs push | Without the WebSocket there are no sessions either; heartbeat hint still works |
| Malicious relay sends a hint flood | Many fetches | ≤ 1 hint-triggered fetch per host per 60 s |
| Malicious relay serves an old index | – | Floor rejects it; a fresh install gets old *signed* metadata, which still only offers ≥ newer versions |
| Bad release | – | Halt (≤ 2 min, R2) → roll forward |
| 503 from the download cap | – | Retry with exponential backoff + jitter (30 s → 30 min), honouring `Retry-After` |
| Device offline for weeks | – | At start: index (floor holds), then rollout by elapsed time; an old ramp has reached 100 % |

### 5.8 Migration

| Client version | Reads | Gets |
| --- | --- | --- |
| ≤ 0.5.23 (today) | unsigned `latest.json` every `check-hours` | Keeps working: **Manage keeps writing `latest.json`** with the same contents. These clients get no rollout and no hint: they update at their next check, once, and then they are new clients. |
| Layer 0 build | `latest.json` + trusted-comment check | Rollback closed |
| Index build (Layer 1+) | `releases.json` if present, else `latest.json` until the first signed index is accepted | W9 fixed once floor > 0 |
| Strict build (after **D2**) | `releases.json` only | Unsigned metadata never trusted |

Order on the servers:

1. tether-api with generation + heartbeat answer. Old clients are unaffected.
2. Manage writes `releases.json` (and `latest.json`).
3. hbbs with `ReleaseHint`. Old clients ignore it.
4. Clients.

Every step can ship alone. The "rollout" for old clients is still "everyone at once". If that is a
concern for one specific release, Manage can delay the `latest.json` write (a manual second click).

### 5.9 What Tether Manage needs

- Sign and upload `releases.json` per channel. Serial = max over relays + 1.
- Upload order:
  1. artifacts (+ `.sig`, + `.exe.gz`) to all relays (and the CDN);
  2. `releases.json` to all relays (and the CDN mirror);
  3. legacy `latest.json` last.
- **Rollout control** in the Releases form: preset (Standard / Fast / Immediately / custom steps),
  start time, Security checkbox.
- **Halt / Resume** buttons per entry (re-sign with `halted`, new serial). They should show
  "Halting reaches devices in about 1 minute".
- **Expiry**: show days left. Re-sign automatically on unlock when < 15 days remain, and warn
  when < 7.
- **Adoption** per channel × app × platform. tether-api groups the device list by version
  (already in the heartbeat), plus the new heartbeat fields `release_serial` and `update_state`
  (`idle`, `waiting-rollout`, `downloading`, `waiting-idle`, `failed`, `stale`, `halted`).
- Optional (**D1**): CDN settings (R2 S3 endpoint, bucket, access key, stored encrypted in
  `keys.json` with the passphrase like the other secrets), or a GitHub repo + token.

## 6. Metrics and alerts (docs/monitoring.md)

New series:

| Source | Metric |
| --- | --- |
| tether-api | `tether_api_release_generation` (gauge); `tether_api_release_index_serial{channel}` and `tether_api_release_index_expires_timestamp{channel}` (parsed from the stored payload, no verification needed for a metric); `tether_api_devices_by_version{profile,version}` (active 24 h); `tether_api_devices_update_state{state}`; `tether_api_requests_total{endpoint="heartbeat",code="200"}` (exists, code changes) |
| hbbs | `hbbs_release_generation`, `hbbs_release_hints_sent_total`, `hbbs_release_hint_broadcast_seconds` (time to send to all peers) |
| Caddy / nginx | download bytes and 503s on `/download/` (the existing per-handler metrics; nginx stub/vts as in deploy/monitoring) |

Alerts (`deploy/monitoring/prometheus/rules/tether.yml`):

| Alert | Condition |
| --- | --- |
| `ReleaseGenerationMismatch` | `tether_api_release_generation != hbbs_release_generation` on a node for 2 min (hint path broken) |
| `ReleaseIndexMismatch` | `tether_api_release_index_serial` differs between relays for 10 min (publish incomplete) |
| `ReleaseIndexExpiringSoon` | expires − now < 7 days |
| `UpdateFailuresHigh` | `update_state="failed"` > 2 % of active devices for 30 min |
| `UpdateStale` | any `update_state="stale"` devices for 1 h |
| `RolloutStalled` | 24 h after a ramp reached 100 %, < 80 % of active devices of that app are on the version (they may be waiting for idle; informational) |
| `DownloadSaturation` | download egress > 70 % of the 130 Mbit/s budget for 15 min, or download 503s > 0 for 10 min |

Grafana: an "Updates" row with the version distribution over time (stacked), state counts, hint
broadcasts and download egress.

## 7. Test plan

- **Unit** (`crates/tether-trust`, new module `release`):
  - sign/verify;
  - reject: wrong key, wrong channel, wrong type, lower serial, oversized;
  - floor file handling;
  - expiry flag;
  - rollout step function and bucket distribution (100 000 synthetic uuids → each percent within
    ±0.3 %);
  - `{relay}` substitution, https-only;
  - trusted-comment parsing (Tauri `.sig` format, Manage's comment), plus a mismatch test (old
    binary with a higher version).
- **Client unit**:
  - the highest serial across hosts wins;
  - hint throttling;
  - idle gating (forced countdown starts only when idle);
  - unsigned `latest.json` flags ignored once floor > 0.
- **tether-api**:
  - G increments on release PUT/DELETE of `releases.json`/`latest.json` only, and persists across
    restart;
  - heartbeat returns `200 {"releases":G}`;
  - an old-client test asserts the technician and TetherQS heartbeat code paths accept it.
- **hbbs**:
  - a registered WS peer receives `ReleaseHint` after registration and after the file changes;
  - unregistered and UDP peers do not;
  - an old client binary ignores it (existing e2e).
  - Load: `tools/load/stack.sh` with 10 000 WS hosts. Bump G and measure broadcast time (target
    < 2 s), hbbs CPU and Caddy CPU, then the index fetch storm with `spread_secs` 60. Target: no
    503, p99 < 100 ms.
- **VM end-to-end** (the Hyper-V corporate-mode VM with a TLS-inspecting proxy, as in
  docs/updates.md):
  1. publish → device sees it in < 2 min;
  2. rollout 0 % → nothing; raise → device installs;
  3. halt during download → no install;
  4. a session running → install waits, then happens after disconnect;
  5. relay 1 stopped → update via relay 2;
  6. CDN URL blocked at the proxy → relay URL used;
  7. hbbs hint disabled → heartbeat hint within 15 min;
  8. TetherQS keeps ID/password across the update.
- **Security regression** (scripted against a local relay stack):
  - replay an older `releases.json` → rejected;
  - forged signature → rejected;
  - `latest.json` with `minimumVersion: 99` → ignored by index clients;
  - an older signed exe under a higher version → refused (trusted comment);
  - a sha256 mismatch → refused.
- **Clock**: device clock ±2 days → only a soft stale warning and a shifted ramp, never a block.

## 8. Plan and effort

| Step | Content | Effort (dev days) | Depends on | Status (2026-11-07, branch `updimpl`) |
| --- | --- | --- | --- | --- |
| 0 | Trusted-comment binding both apps; technician `download()` + `install(bytes)` instead of `download_and_install`; `If-None-Match` per host; jittered fallback interval; TetherQS woken by policy changes | 1 | – | **done** (own commit, ships alone); the technician app installs via `restore_update` + `install(bytes)`; TetherQS also takes the highest version over all relays |
| 1 | tether-api release generation + heartbeat `200 {"releases":G}`; client wakes on change (≤ 15 min latency for everyone on the new client) | 1 | – | **done** |
| 2 | Signed release index: `tether-trust::release`, Manage signing + upload order, client verify/floor/multi-source, unsigned flags ignored; docs (api.md, updates.md, encryption.md W9 → fixed, SECURITY.md) | 3-4 | 0 | **done**; tether-api verifies the index on upload and serves it with `ETag: "<serial>"` (index fetches are counted); D2 is the constant `REQUIRE_SIGNED_INDEX` |
| 3 | Rollout steps, `update.rollout` policy, halt, security flag, install when idle (both apps), heartbeat `update_state`; Manage rollout/halt UI and adoption view | 3 | 2 | **done**; adoption is in Grafana ("Devices by version", "Update states"); the Manage Devices page is unchanged |
| 4 | hbbs `ReleaseHint` (proto in both `hbb_common` copies, file watch, broadcast, send after registration), client hook + option mirror, `spread_secs`; load test | 2-3 | 1 | **done**, without the 10 000-host load test |
| 5 | Compressed TetherQS download (`precompressed gzip`, `gzip_static`, reqwest `gzip`) | 0.5 | – | **done**, plus the `packed` entry for the CDN |
| 6 | Metrics, alerts, Grafana row | 1 | 1-4 | **done** (not deployed) |
| 7 (D1) | CDN: Workers Static Assets on workers.dev (was: R2 + domain), Manage upload, index URL list | 1.5-2 | 2, decision | **done** in code; Worker not deployed, no Cloudflare resources created |
| | **Total** | **≈ 13-16** (without step 7: 11.5-14) | | |

Not built: an index mirror on the CDN (the relays stay the only source of the index, as decided),
the clock-skew warning from the relay's `Date` header, the `RolloutStalled` and
`DownloadSaturation` alerts, and an adoption view inside Tether Manage.

Suggested order: 0 → 1 → 5 (quick wins, about 2.5 days, latency already 6 h → 15 min, rollback
closed), then 2 → 3 (W9 fixed, safe rollouts), then 4 (seconds), 6, and 7 if decided.

## Decisions needed

**Decided 2026-11-07** (owner):

- **D1**: downloads offloaded to Cloudflare, served by a Worker on `<name>.workers.dev` (no
  domain on Cloudflare; `r2.dev` is not for production). The relays stay the source of truth for
  the signed release index and the push; the Worker only serves binaries; clients fall back to the
  relay URL. Every binary is verified against sha256 + size + minisign signature + signed
  trusted-comment version from the signed index, so the CDN is untrusted. **Revised the same day:**
  no R2 (enabling it needs billing details, which the owner refuses): Workers Static Assets on
  `tether-dl.thomas-schend.workers.dev`, free plan, 25 MiB per file, TetherQS.exe as a gzip copy
  (§5.6).
- **D2**: clients from this release verify the signed index whenever present; once a device has
  accepted one it never accepts an unsigned manifest again (the highest serial is persisted); from
  the next release the signed index is required (`tether_trust::release::REQUIRE_SIGNED_INDEX`).
- **D3**: expiry 30 days, warn only ("Update information is out of date" in Diagnostics/Settings),
  never refuse.
- **D4**: staged rollout default 1 % → 10 % → 50 % → 100 % over 24 h (signed ramp), stable
  per-device bucket per release, `security` skips the ramp, halt = re-signed index.
- **D5**: yes, build the hbbs push (`ReleaseHint` over the existing rendezvous connection: hbbs
  watches the file tether-api writes, broadcasts to registered hosts, sends the current value after
  registration, clients wait 0-60 s and then fetch the index), plus the heartbeat hint and the
  jittered fallback poll.
- **D6**: keep Caddy (nginx optional, as today); the staged rollout bounds download bandwidth.
- **D7**: yes: forced and mandatory updates never interrupt a running session (TetherQS and the
  technician app): download in the background, install when idle (no session, no file transfer).

The original questions:

| # | Question | Options | Recommendation |
| --- | --- | --- | --- |
| **D1** | Offload release downloads? | (a) stay on relays (rollout pace ≤ 10 %/h, nginx caps); (b) **buy a domain** (≈ $10/yr) and use Cloudflare R2 + custom domain (egress free); (c) a public GitHub repo for releases (free, binaries public on GitHub); (d) Bunny ($0.01/GB) | (a) is enough for normal releases at 10 k. Choose (b) if security releases must reach everyone within the hour; it also gives an independent index mirror. **Needs your decision: buying a domain / Cloudflare account.** |
| **D2** | When do clients *require* the signed index? | release N+1 after step 2, or later | Make the second release after step 2 strict, once both relays and Manage write `releases.json` |
| **D3** | Expiry length and what happens when it passes | soft warning (recommended) vs hard refuse; 30 days vs shorter | Soft, 30 days, Manage re-signs on unlock |
| **D4** | Default rollout preset | Standard (1/10/50/100 % over 24 h) / Fast / Immediately | Standard for `stable`, Immediately for `test` |
| **D5** | Build the hbbs push (step 4)? | stop at the heartbeat hint (≤ 15 min) or add push (< 1 min, halt in < 2 min) | Build it: 2-3 days, zero runtime cost, and the only way to meet R1/R2 |
| **D6** | Front end during releases | Caddy (no download cap) vs nginx (cap, recommended in scaling.md) | nginx, or a Caddy cap, before the first fast rollout to 10 k |
| **D7** | Forced TetherQS update during a session | today: restarts anyway; proposed: wait until idle | Wait until idle |

Not in scope, but the biggest remaining source of delay: per-machine NSIS installs need a UAC
prompt for non-admin users. Letting the installed Tether service (SYSTEM) run the already verified
installer would make technician updates fully silent. That needs its own design (it is a
privileged code path).

## Sources

- Chromium updater functional spec and Omaha protocol 3.1:
  <https://chromium.googlesource.com/chromium/src/+/main/docs/updater/functional_spec.md>,
  <https://chromium.googlesource.com/chromium/src/+/main/docs/updater/protocol_3_1.md>
- Firefox: <https://firefox-source-docs.mozilla.org/toolkit/mozapps/update/docs/BackgroundUpdates.html>,
  Balrog throttling <https://hearsum.ca/posts/streamlining-throttled-rollout-of-firefox-releases/>,
  Megaphone <https://github.com/mozilla-services/megaphone>, autopush-rs <https://github.com/mozilla-services/autopush-rs>
- VS Code: `src/vs/platform/update/electron-main/abstractUpdateService.ts` (microsoft/vscode, main)
- Electron updates: <https://electronjs.org/docs/latest/tutorial/updates>
- Sparkle: <https://sparkle-project.org/documentation/publishing/>, <https://sparkle-project.org/documentation/customization/>
- Tauri updater v2: <https://v2.tauri.app/plugin/updater/>, <https://docs.rs/tauri-plugin-updater/latest/tauri_plugin_updater/struct.Update.html>
- TeamViewer: <https://www.teamviewer.com/en/global/support/knowledge-base/teamviewer-remote/update/update-teamviewer/>
- TUF: <https://theupdateframework.io/docs/security/>
- Cloudflare R2 pricing and public buckets: <https://developers.cloudflare.com/r2/pricing/>,
  <https://developers.cloudflare.com/r2/buckets/public-buckets/>; Workers limits
  <https://developers.cloudflare.com/workers/platform/limits/>; Registrar <https://domains.cloudflare.com/>
- Bunny CDN pricing: <https://docs.bunny.net/cdn/pricing>
- jsDelivr limits (README copy): <https://github.com/ashitakaze/jsdelivr>

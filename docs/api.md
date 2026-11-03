# Tether relay API (`tether-api`) — wire contract

The contract between the technician app (`apps/desktop`), the customer module (`apps/quick`), the
management app (`apps/manage`) and the relay service (`server/api`). Shared types and crypto live
in `crates/tether-trust`; do not reimplement them.

Base URL: `https://<relay>` (production `https://tether-desk.duckdns.org`). Caddy routes:
- `/api/*` → `tether-api` on `127.0.0.1:21120`
- `/download/*` → static files in `/var/lib/tether-api/download/` (served by Caddy `file_server`;
  `latest.json` with `Cache-Control: no-cache`)
- `/` → `/var/lib/tether-api/download/index.html` (the download page)
- `/ws/id` and `/ws/relay` are unchanged.

Clients reach the API through the same route racer as the relay (443/80, proxy/direct), using the
system proxy. Everything is TLS; plain HTTP is never used.

## Keys

| Key | Holder | Public half lives in |
| --- | --- | --- |
| Admin key (Ed25519) | management app, encrypted at rest | clients: compile-time `TETHER_ADMIN_PUB_KEY` (base64); server: `ADMIN_PUB_KEY` in `/etc/tether/api.env` |
| Updater key (Tauri minisign) | management app, encrypted at rest | `tauri.conf.json` `plugins.updater.pubkey` (technician), compile-time `TETHER_UPDATER_PUB_KEY` (customer module) |

Several admin public keys may be trusted (comma-separated) to allow key rotation.

## Signed documents

`tether_trust::envelope::Signed` JSON: `{"payload": b64(bytes), "sig": b64(sig), "key": b64(pubkey)}`.
The payload of a policy document is the JSON of `tether_trust::policy::Bundle`.

## Public endpoints (no auth)

### `GET /api/policy`
Returns the stored signed policy document (`application/json`), or `404` if none was published.
Clients verify it with `envelope::open_json(raw, &[admin keys])`, parse with `Bundle::from_json`,
reject `serial` lower than the last accepted one (stored locally), then `policy::resolve`.
Response header `ETag: "<serial>"`; clients send `If-None-Match` and get `304` when unchanged.

### `POST /api/heartbeat`
Body (JSON, ≤ 2048 bytes, unknown fields rejected):
```json
{"uuid":"<install uuid, 8-64 [A-Za-z0-9-]>","id":"<Tether ID, ≤ 32>","profile":"technician|customer",
 "version":"0.3.0","os":"windows 10.0.26300","serial":12}
```
`204` on success. `429` when the same uuid posts more than once per 60 s or an IP more than 30 per
minute. The server stores `{…, "ip": <client ip from X-Forwarded-For>, "last_seen": unix}` per uuid,
at most 10 000 devices (oldest `last_seen` evicted), entries older than 180 days dropped.
Clients send it after each policy check.

### `POST /api/diag/<ticket>`
Upload a diagnostics zip for a ticket an admin opened. `<ticket>` is 8 characters `[A-HJ-NP-Z2-9]`
(no ambiguous letters). Body `application/zip`, ≤ 2 MiB. `204` if the ticket is open and
unused, else `404`. One upload per ticket. Stored 7 days.

## Admin endpoints (signed requests)

Every request carries `Tether-Sig: v1 ts=… nonce=… sig=…` (see `tether_trust::reqauth`; the path
includes the query string; body hash covers the exact bytes sent). The server verifies against
every trusted admin key, allows ±300 s clock skew, and rejects a nonce seen in the last 600 s.
Failure: `401` with a short reason. Bodies over the limits: `413`.

| Method & path | Body | Result |
| --- | --- | --- |
| `PUT /api/admin/policy` | signed policy document (≤ 256 KiB) | `204`. The server also verifies the signature, validates the bundle and requires `serial` > current (`409` otherwise). Stored atomically, previous versions kept in `policy-history/<serial>.json` (last 50). |
| `GET /api/admin/policy-history` | – | `[{"serial":…, "issued_at":…}]` newest first |
| `GET /api/admin/devices` | – | `[{uuid,id,profile,version,os,serial,ip,last_seen}]` |
| `POST /api/admin/diag-ticket` | `{"note":"…"}` (≤ 256 chars) | `{"ticket":"K7Q2M9XD","expires":unix}` (open 24 h) |
| `GET /api/admin/diag` | – | `[{ticket,note,created,uploaded,size}]` |
| `GET /api/admin/diag/<ticket>` | – | the zip (`application/zip`) or `404` |
| `PUT /api/admin/release/<channel>/<path>` | file bytes (≤ 200 MiB) | `204`. Writes `download/<channel>/<path>` atomically. `<channel>` `[a-z0-9-]{1,32}`; `<path>` segments `[A-Za-z0-9._-]`, no `..`. Upload artifacts first, `latest.json` last. |
| `DELETE /api/admin/release/<channel>/<path>` | – | `204` |
| `GET /api/admin/releases` | – | `[{channel,path,size,modified}]` |
| `PUT /api/admin/download-page` | HTML (≤ 512 KiB) | `204`, stored as `download/index.html` |
| `GET /api/admin/status` | – | `{version, disk_free, devices, policy_serial, uptime}` |

The server refuses uploads that would leave less than 2 GiB free on the disk (`507`).

## Download layout and update manifests

```
download/
  index.html                                  download page
  <channel>/technician/windows-x86_64/latest.json
  <channel>/technician/windows-x86_64/Tether_<v>_x64-setup.exe(.sig)
  <channel>/customer/windows-x86_64/latest.json
  <channel>/customer/windows-x86_64/TetherQS.exe(.sig)
  <channel>/technician/darwin-universal/…     (macOS, CI)
```
Channel default `stable` (policy feature `update.channel`). `latest.json` is the Tauri updater
format plus three optional fields:
```json
{"version":"0.3.1","notes":"…","pub_date":"2026-11-04T10:00:00Z",
 "platforms":{"windows-x86_64":{"signature":"<minisign sig, as Tauri writes it>","url":"https://…/Tether_0.3.1_x64-setup.exe"}},
 "mandatory":false,"minimumVersion":"","forced":false}
```
- `mandatory`: the update must be installed before the app can be used for sessions.
- `minimumVersion`: versions below it are blocked (same effect as policy `update.minimum-version`).
- `forced`: install without asking after `update.restart-seconds`.
Clients enforce the stricter of manifest and policy. Blocking is done in the backend (session
start, accepting incoming sessions), not only in the UI.

## Limits (1 GB relay VM)

`tether-api` is a single-threaded `tiny_http` service with `MemoryMax=64M`; request bodies are
streamed to disk for uploads; every JSON write is temp file + rename; no database.

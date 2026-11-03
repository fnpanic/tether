# TetherQS — customer quick-support module

TetherQS (`apps/quick`) is the small program a customer downloads and starts when they need help.
It is one portable `TetherQS.exe`: no installer, no admin rights, no service, no tray. It shows an
ID and a one-time password; the technician connects to it from the Tether technician app.

It runs the same RustDesk core as the technician app (`vendor/rustdesk`, Tauri bridge
`src/tauri_ui`) with a different, minimal UI. It only receives sessions
(`HARD_SETTINGS["conn-type"]="incoming"`, `disable-installation=Y`) and never connects out.

## How customers use it

1. Download `TetherQS.exe` (e.g. from `https://<relay>/download/stable/customer/windows-x86_64/TetherQS.exe`) and start it.
   Windows 10 21H2+ / 11 ship the WebView2 runtime it needs; if it is missing, TetherQS shows a
   message box with the download link instead of a window.
2. Read the ID and password to the technician (or copy them). The status pill shows
   *Connecting*, *Ready*, *Session active* or *Offline*.
3. When the technician connects, an incoming-request card appears: who is connecting and which
   permissions they get (toggles; locked ones are greyed out by policy). **Accept** or **Decline**.
   With `security.approve-mode=password` sessions start without the card; with `click` the
   password is not shown at all.
4. During the session the card shows the technician, elapsed time, indicators (Control, Files,
   REC) and a red **Disconnect** button. Permissions can be revoked at any time.
5. Closing the window ends support: the in-process host server stops with it.

**Help** (title bar `?`, policy `diagnostics.page`) shows version, ID, server, whether the server is
reachable (and latency), the proxy in use, admin-access state and the policy serial.

## Process model

`TetherQS.exe` with no arguments runs `core_main()` → `start_server(false)` in-process (host server
under the normal user), plus the connection manager's `_cm` IPC listener (`tauri_ui::cm::start`) in
the same process, so incoming requests show up in the one window and no `--cm` process is spawned.
If the core ever starts `TetherQS.exe --cm`, that process shows the same window. The core's other
roles (`--portable-service`, …) are handled by `core_main` unchanged.

Config, ID, logs: `%APPDATA%\Tether-QS` (`APP_NAME = "Tether-QS"`), separate from an installed
technician app. The webview stores its cache under `%LOCALAPPDATA%\net.tether.quick`.

Networking is the technician app's 443-only policy (copied `netpolicy.rs`): WebSocket/TLS to the
relay only, no UDP, system proxy (env, WinHTTP static/PAC/WPAD) detected and refreshed every
minute. Policy, heartbeat and update requests use the same proxy and the Windows trust store.

## Admin access (UAC)

A normal-user process cannot see or click UAC prompts or the secure desktop. **Allow admin
access** (policy `customer.elevate`) starts the core's elevated *portable service* (one UAC
prompt for the customer). During a session this happens in place (`elevate_portable`), so the
session continues. Without a session TetherQS restarts with `pre-elevate-service`; the ID and the
current one-time password are kept (handed over to the new process). The main process stays a
normal-user process either way.

The technician can also request elevation from inside a session (core `ElevationRequest`), which
starts the same portable service without a restart.

Limits: without admin access the technician sees a black/frozen screen while a UAC prompt is open
and cannot type into elevated windows. If the customer declines the UAC prompt, TetherQS keeps
running without admin access.

## Policy (profile `customer`)

At start the last verified policy (`%APPDATA%\Tether-QS\policy.json`) is applied before the core
starts. Then every `policy.refresh-minutes` (1 with `policy.fast-apply`): `GET /api/policy`,
verify with `tether_trust::envelope::open_json` against the compiled-in `TETHER_ADMIN_PUB_KEY`
(comma-separated for rotation), `Bundle::from_json`, reject a lower serial, cache, `resolve(…,
"customer", <install uuid>, None)`. Locked features with a core option become overrides, others
defaults; `FLOOR` keys are never touched. After each check `POST /api/heartbeat` with profile
`customer`. Release builds refuse to compile without the key; debug builds without it run on
defaults and make no policy/heartbeat requests.

| Key | Effect in TetherQS |
| --- | --- |
| `customer.title` | Window title / company name in the title bar |
| `customer.accent-color` | Accent colour (`#rrggbb`, invalid values fall back to the default) |
| `customer.welcome` | Text above the ID |
| `customer.close-after-session` | Quit 2 s after the last session ends |
| `customer.elevate` | Offer **Allow admin access** |
| `permission.*` | Default and lock of each permission toggle (`hidden` removes it); enforced by the core option |
| `security.approve-mode` | `click`: password hidden, accept only; `password`: no accept card; `both` |
| `security.verification-method`, `security.temporary-password-length` | Core options |
| `connect.incoming` | Off: incoming connections are closed by the backend |
| `recording.consent-text` | Fallback text of the recording consent dialog |
| `update.*` | See below |
| `policy.refresh-minutes`, `policy.fast-apply` | Policy check interval |
| `diagnostics.page` | Show the Help/diagnostics view |

## Updates (portable, no installer)

At start and every `update.check-hours` (if `update.enabled`): fetch
`https://<relay>/download/<update.channel>/customer/windows-x86_64/latest.json` (Tauri updater
format, docs/api.md). If the version is newer:

- The exe is downloaded (https only, ≤ 200 MiB) and its minisign signature is verified against the
  compiled-in `TETHER_UPDATER_PUB_KEY` (Tauri format: `signature` is base64 of the `.sig` file text;
  the key is base64 of the minisign public key file, a bare `RW…` key also works).
- It is written to `TetherQS.exe.new` next to the running exe; the running exe is renamed to
  `TetherQS.exe.old` and the new one takes its name; then TetherQS restarts (same ID and password)
  and deletes `.old` on the next start. If the folder is not writable the new exe goes to
  `%LOCALAPPDATA%\Tether-QS\TetherQS.exe` and is started from there.
- `mandatory` (with a newer version), `minimumVersion` or policy `update.minimum-version` above the
  running version make the update **required**: the backend closes incoming connections and the
  Accept button is disabled until it is installed.
- `forced` (or a required update with policy `update.force`) installs automatically after
  `update.restart-seconds`; otherwise the customer clicks **Update**.

Publishing: upload `TetherQS.exe` and `TetherQS.exe.sig` (signed with the updater key, e.g.
`tauri signer sign`), then `latest.json` with `platforms["windows-x86_64"] = {signature, url}`.

## Building

```sh
cd apps/quick && npm ci && npm run build
cd src-tauri
# TETHER_SERVER, TETHER_PUB_KEY, TETHER_ADMIN_PUB_KEY, TETHER_UPDATER_PUB_KEY are required for release
cargo build --release --features custom-protocol
# → <target>/release/TetherQS.exe (the only artifact; Tauri bundling is off)
```

## Known limits

- Recording consent works like in the technician app: the request arrives as
  `record_consent_request`, the customer's answer goes to the core via `cm_record_consent`
  (verified end to end: Deny → nothing recorded, Allow → playable file).
- A manifest `minimumVersion` is only seen while `update.enabled` is on; the policy key
  `update.minimum-version` is enforced either way.
- Starting TetherQS twice: the second instance finds the first one's IPC and exits.
- Over-the-shoulder UAC with a different admin account works for the portable service (it runs as
  SYSTEM), but the customer must know those credentials.

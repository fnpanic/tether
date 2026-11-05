# TetherQS — customer quick-support module

TetherQS (`apps/quick`) is the small program a customer downloads and starts when they need help.
It is one portable `TetherQS.exe`: no installer, no admin rights, no service, no tray. It shows an
ID and a one-time password; the technician connects to it from the Tether technician app.

It runs the same RustDesk core as the technician app (`vendor/rustdesk`, Tauri bridge
`src/tauri_ui`) with a different, minimal UI. It only receives sessions
(`HARD_SETTINGS["conn-type"]="incoming"`, `disable-installation=Y`) and never connects out.

## How customers use it

1. Download `TetherQS.exe` (from the download page `https://<relay>/`; releases are versioned, `…/download/stable/customer/windows-x86_64/TetherQS-<version>.exe`) and start it.
   Windows 10 21H2+ / 11 ship the WebView2 runtime it needs; if it is missing, TetherQS shows a
   message box with the download link instead of a window.
2. Read the ID and password to the technician (or copy them). The password is masked until the
   customer clicks the eye (as in the technician app); copy works either way. The status pill shows
   *Connecting*, *Ready*, *Session active* or *Offline*.
3. When the technician connects, an incoming-request card appears: who is connecting and which
   permissions they get (toggles; locked ones are greyed out by policy; audio is off by default).
   **Accept** or **Decline**. The card says that Accept lets the technician in without a password
   (in `both` mode typing the one-time password works too); after Accept a message confirms it.
   With `security.approve-mode=password` sessions start without the card; with `click` the
   password is not shown at all. For a request or session the window moves to the bottom-right
   corner of its screen, and back to the centre when the last session ends.
   When a session starts (after Accept, or right away when the technician used the password) the
   window minimizes to the taskbar (Dock on macOS) once: it is never hidden or moved to a tray, so
   the customer can always click it back. If they restore it, it stays open. A new request or a
   recording request restores it and brings it to the front. Tether's own host window (the
   connection manager of a Tether that is being connected to) behaves the same, and its main
   window minimizes with it. On Windows the Tether connection window has its own taskbar button
   (own AppUserModelID `<identifier>.cm`), so a click on it restores or minimizes it like
   TetherQS's single window; the main window keeps its own button and stays minimized. (Up to
   0.5.11 both windows shared one grouped button, whose click only showed thumbnails.) On macOS
   see [macos.md](macos.md#windows-and-the-dock).
4. During the session the card shows the technician, elapsed time, indicators (Control,
   Transferring files, REC) and a red **Disconnect** button. Permissions can be revoked at any
   time. A file transfer the technician opens from the session is part of it: no second request,
   no second card. The host admits it because it carries the accepted session's key and its
   random 128-bit companion secret (sent only inside the encrypted logins), and only while that
   session's **Transfer files** switch is on; switching Files off or Disconnect ends it. With
   Files off, a new file transfer is a normal request again.
   **Chat** (tab next to Permissions on the card): each message from the technician restores the
   window if it is minimized, switches to the Chat tab, shows a short notice and flashes the
   taskbar button (TetherQS does not take the focus). The customer can reply there.
   The one-time password changes as soon as it has let someone in, so within one session nothing
   asks again: the host lets the same technician window back in with the same session key and
   companion secret after a dropped connection or a reloaded window (whether the session was
   accepted by click or by password), while the session runs and for up to 2 minutes after its
   connection ended. **Disconnect** (or any other end by this computer) ends that at once; after
   it, or after the 2 minutes, the technician needs a new Accept or the new password, and their
   window says why. A remembered password that the host turns down is forgotten, and the
   technician is asked for the current one instead of being told it was wrong.
5. Closing the window ends support: the in-process host server stops with it.

**Help** (title bar `?`, policy `diagnostics.page`) shows version, ID, connection state, every
server it registers with (compiled-in and policy `servers.list`), the proxy in use, admin-access
state and the policy serial at once, also without a network. Whether each server is reachable (and
latency) is checked separately, at most 5 s each, with **Retry**; a broken network shows its error
there instead of hiding the page.

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
(comma-separated for rotation), `Bundle::from_json`, reject a lower serial, cache, `resolve_with(…,
"customer", <install uuid>, <custom app layer>, None)` (see Custom apps). Locked features with a core option become overrides, others
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

At start and every `update.check-hours` (if `update.enabled`; 15 minutes after a failed check),
and when the customer clicks **Help → Check for updates**: fetch
`https://<relay>/download/<update.channel>/customer/windows-x86_64/latest.json` (macOS:
`…/customer/darwin-universal/latest.json`, a zipped `.app`, see [updates.md](updates.md)) (Tauri
updater format, docs/api.md) from each server in turn; the first one with a release wins, an
unreachable one or one without a release (`404`) passes on to the next. Help shows the result of
the last check ("… is up to date", "Version … is available", or "Check failed: <server>: <reason>").
A TetherQS that was already running when a release was published sees it only at its next check:
use **Check for updates** or restart it. If the version is newer:

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
  `update.restart-seconds`; otherwise the customer clicks **Update**. The button shows the download
  in percent (the relays serve a few hundred KB/s, so the 35 MB exe can take a minute or two).

Publishing: upload `TetherQS.exe` and `TetherQS.exe.sig` (signed with the updater key, e.g.
`tauri signer sign`), then `latest.json` with `platforms["windows-x86_64"] = {signature, url}`.

## Custom apps (branded downloads)

Tether Manage can create a **custom app**: a name, logo (PNG, at most 96 KiB), accent colour,
welcome text and customer settings, signed with the admin key (`tether_trust::custom::CustomProfile`)
and stored on every relay together with a private, expiring download link
`https://<relay>/q/<token>` (256-bit random token, at most 366 days; wire contract in
[api.md](api.md#custom-apps-branded-tetherqs-downloads)). The relay streams the channel's current
`TetherQS-<v>.exe` with the signed profile appended (`<signed JSON><u32 LE length>TQSPROF1`) as
`<Name>-Support.exe`. Unknown, revoked and expired links all answer `404`; 30 requests per minute
per IP; the admin sees download counts. The exe itself stays the same signed build: no per-customer
compilation.

In TetherQS (Windows):
- At start it reads the tail of its own exe, verifies the profile against the compiled-in
  `TETHER_ADMIN_PUB_KEY` and validates it. A missing, damaged or untrusted trailer is logged and
  TetherQS runs as plain TetherQS.
- Name, accent and welcome replace `customer.title`, `customer.accent-color` and
  `customer.welcome` (window title and title bar); the logo is shown in the title bar; the Help view
  lists the custom app name.
- The settings layer is applied with `policy::resolve_with`: defaults < policy profile < **custom
  app** < group < machine. Only customer-profile features are allowed in it.
- Self-update downloads and verifies the plain exe as usual, then writes it with this instance's
  trailer appended unchanged, so the customer keeps the branded app.

Revoking or letting the link expire stops new downloads only; copies already downloaded keep
working (the profile has no expiry). Test helper without Tether Manage:
`cargo run --example custom_app -- <keyfile> <TetherQS.exe> <out.exe>` in `crates/tether-trust`
(prints the public key for `TETHER_ADMIN_PUB_KEY`, writes a signed test profile).

## Building

```sh
cd apps/quick && npm ci && npm run build
cd src-tauri
# TETHER_SERVER, TETHER_PUB_KEY, TETHER_ADMIN_PUB_KEY, TETHER_UPDATER_PUB_KEY are required for release
cargo build --release --features custom-protocol
# → <target>/release/TetherQS.exe (the only artifact; Tauri bundling is off)
```

macOS: `bash scripts/build-macos.sh` builds a universal `TetherQS.app` (bundling switched on for
that build only) and zips it as `dist/macos/TetherQS-<version>-universal.zip`; see
[macos.md](macos.md).

## Known limits

- Recording consent works like in the technician app: the request arrives as
  `record_consent_request`, the customer's answer goes to the core via `cm_record_consent`
  (verified end to end: Deny → nothing recorded, Allow → playable file).
- A manifest `minimumVersion` is only seen while `update.enabled` is on; the policy key
  `update.minimum-version` is enforced either way.
- Starting TetherQS twice: the second instance finds the first one's IPC and exits.
- Over-the-shoulder UAC with a different admin account works for the portable service (it runs as
  SYSTEM), but the customer must know those credentials.

# TetherQS — customer quick-support module

TetherQS (`apps/quick`) is the small program a customer downloads and starts when they need help.
It is one portable `TetherQS.exe`: no installer, no admin rights, no service, no tray. It shows an
ID and a one-time password; the technician connects to it from the Tether technician app.

It runs the same RustDesk core as the technician app (`vendor/rustdesk`, Tauri bridge
`src/tauri_ui`) with a different, minimal UI. It only receives sessions
(`HARD_SETTINGS["conn-type"]="incoming"`, `disable-installation=Y`) and never connects out.
It refuses a camera view and a remote terminal (`enable-camera` and `enable-terminal` forced to
`N` in `security.rs`, as in the technician app).

## How customers use it

1. Download `TetherQS.exe` (from the download page `https://<relay>/`; releases are versioned, `…/download/stable/customer/windows-x86_64/TetherQS-<version>.exe`) and start it.
   Windows 10 21H2+ / 11 ship the WebView2 runtime it needs; if it is missing, TetherQS shows a
   message box with the download link instead of a window.
2. Read the ID and password to the technician (or copy them). The password is masked until the
   customer clicks the eye (as in the technician app); copy works either way. The status pill shows
   *Connecting*, *Ready*, *Session active* or *Offline*.
3. When the technician connects, an incoming-request card appears: who is connecting and which
   permissions they get (toggles; locked ones are greyed out by policy; audio is off by default).
   The card names the ID, says that the name is chosen by the other side and is not verified, and
   says what is asked for (see and control this screen, transfer files, the camera, a command
   line, or the network through this computer). **Accept** or **Decline**; Accept is never
   pre-selected, so a key typed as the card appears accepts nothing. A request that arrives during
   a session is shown above the running sessions' cards (their **Disconnect** stays reachable), and
   an open Help view closes for a request or a session start.
   The card says that Accept lets the technician in without a password
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
   TetherQS's single window. (Up to 0.5.11 both windows shared one grouped button, whose click
   only showed thumbnails.) After 0.5.17 the main window is also taken off the taskbar for the
   session (minimized and hidden), so the connection window's is the only Tether button; starting
   Tether again (pinned taskbar icon, Start menu) toggles the connection window too, never the
   main one (the `--cm` process passes its window handle along with `--minimize-main`). When the
   last session ends (`--host-session-ended`) the main window is back on the taskbar, still
   minimized; the tray's **Open Tether** opens it at any time. (In 0.5.12 to 0.5.17 a click on
   the main window's own button restored it during the session.) On macOS
   see [macos.md](macos.md#windows-and-the-dock).
4. During the session the card shows the technician, elapsed time, indicators (Control,
   Transferring files, REC) and a red **Disconnect** button. Permissions can be revoked at any
   time; each switch shows what that connection really has. With two or more technicians connected, each has its own card at full height and the
   window scrolls between them; each card's switches change only that connection. A file transfer the technician opens from the session is part of it: no second request,
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
   connection ended. The returning connection keeps the permissions the customer last set, and
   the old connection, if it still runs, is closed: a session never runs on two connections.
   **Disconnect** (or any other end by this computer) ends that at once, and so does the
   technician closing the session; after either, or after the 2 minutes, the technician needs a
   new Accept or the new password, and their window says why. A technician app released before
   this change reports a reloaded session window to the host as a closed session, so it is asked
   again after a reload (a dropped connection still comes back). A remembered password that the host turns down is forgotten, and the
   technician is asked for the current one instead of being told it was wrong.
5. Closing the window ends support: the in-process host server stops with it.

**Help** (title bar `?`, policy `diagnostics.page`) shows version, ID, connection state, every
server it registers with (compiled-in and policy `servers.list`), the proxy in use, admin-access
state and the policy serial at once, also without a network. Whether each server is reachable (and
latency) is checked separately, at most 5 s each, with **Retry**; a broken network shows its error
there instead of hiding the page. **Send to support** below it (policy `diagnostics.upload`) takes
the 8-character ticket code from Tether Manage and uploads the diagnostics bundle (TetherQS's logs
and settings, no passwords; [troubleshooting.md](troubleshooting.md#send-to-support)).

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

## ID

TetherQS derives its ID from the MAC address like the technician app, but salted (`ID_SALT`
"TetherQS" in `apps/quick/src-tauri/src/main.rs`, hbb_common `Config::auto_id_from_mac`): a 9-digit
ID that is never the technician app's. Up to 0.5.28 both apps on one PC got the **same ID** (and the
same machine UUID, but different keys); hbbs accepts that and lets the last registration win, so a
technician dialling that ID reached either app at random, with a password that did not match.

From 0.5.29 a new TetherQS starts with the salted ID. An existing one keeps its stored ID unless
the technician app on the same machine stores the same one: then it moves once to its own ID at start
(`Config::separate_auto_id`, log line "id … is the technician app's on this machine, moved to …") and
keeps it; key pair and password stay. "The technician app's ID" is the `id` in
`%APPDATA%\Tether\config\Tether.toml` of the same user or in the installed service's
`C:\Windows\ServiceProfiles\LocalService\AppData\Roaming\Tether\config\Tether.toml` (macOS
`~/Library/Preferences/app.tether.Tether/Tether.toml`, the root service's under `/var/root`); a file
TetherQS cannot read counts as absent. Not on a relaunch with hand-over (update, admin access): that
restart keeps the ID the customer already gave out, and the next plain start moves it. The technician
app's ID never changes (customers and Manage know it). The old ID's record on the relays still holds
TetherQS's key after the move; hbbs asks the technician app's live connection to register again (see
[corporate-networks.md](corporate-networks.md)), so it is reachable with its own key within about a
minute.

## Admin access (UAC)

A normal-user process cannot see or click UAC prompts or the secure desktop. **Allow admin
access** (policy `customer.elevate`) starts the core's elevated *portable service* (one UAC
prompt for the customer). During a session this happens in place (`elevate_portable`), so the
session continues. Without a session TetherQS restarts with `pre-elevate-service`; the ID and the
current one-time password are kept (handed over to the new process). The main process stays a
normal-user process either way.

The technician can also request it from inside a session: **Remote actions → Request admin
access** (core `ElevationRequest`, `session_elevate`; shown for a TetherQS host on Windows,
disabled once admin access is on). It starts the same portable service without a restart; the
customer confirms one UAC prompt and can be asked again after a No.

Policy `customer.elevate` off covers both ways (from TetherQS 0.5.28): the customer's button is
hidden, and the host refuses a technician's request before any UAC prompt opens (core option
`allow-elevate` = `N`, `connection.rs` `handle_elevation_request`). The technician gets "Admin access
could not be requested: it is turned off by policy on that computer." and the session continues.
TetherQS 0.5.27 and older still start the prompt on request. Test: `apps/desktop/e2e/vm-elevpol-e2e.mjs`
(VM, both cases).

Limits: without admin access the technician sees a black/frozen screen while a UAC prompt is open
and cannot type into elevated windows. If the customer declines the UAC prompt, TetherQS keeps
running without admin access.

What the technician sees (viewer, `apps/desktop/src/lib/elevation.ts`): while a UAC prompt is open
on a host without admin access, a notice at the top of the picture ("An admin prompt (UAC) is
open…"); the session stays connected and usable (it is not an error). When the prompt closes, the
notice goes and a toast says how it ended: "Admin access is on…" after Yes to Tether's own
request, "Admin access was declined…" when that request brought no admin access within 5 s (No,
Esc, or the prompt closed), and "…was answered or closed" for any other prompt (a program or a
protected folder; the host cannot learn that answer, consent.exe runs as SYSTEM). An unanswered
prompt is closed by Windows after about 2 minutes (122 s measured on Windows 11 24H2), which
counts as No; the session keeps running meanwhile. An elevated window in
front gets its own notice ("…runs as administrator…") until it is no longer in front. With admin
access on, the technician sees the secure desktop and no notices. Tested with
`apps/desktop/e2e/vm-uac-e2e.mjs`.

## Policy (profile `customer`)

At start the last verified policy (`%APPDATA%\Tether-QS\policy.json`) is applied before the core
starts. Then every `policy.refresh-minutes` (1 with `policy.fast-apply`): `GET /api/policy`,
verify with `tether_trust::envelope::open_json` against the compiled-in `TETHER_ADMIN_PUB_KEY`
(comma-separated for rotation), `Bundle::from_json`, reject a lower serial, cache, `resolve_with(…,
"customer", <install uuid>, <custom app layer>, None)` (see Custom apps). The highest serial ever
accepted is kept in `policy-serial` beside the cache, so an older signed policy does not come back
when `policy.json` is missing or was replaced. A rule with a feature this TetherQS does not know or a value it finds invalid is
skipped and logged (`policy <serial>: rules not applied: …`); the rest applies. Locked features with a core option become overrides, others
defaults; `FLOOR` keys are never touched. A permission the new policy locks off is switched off for
running connections and for connections that resume, and a running file-transfer connection is closed. After each check `POST /api/heartbeat` with profile
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
| `connect.chat` | Off: no Chat tab, the technician's messages are not shown and do not restore the window, replies are refused |
| `recording.consent-text` | Text of the recording consent dialog; applied as soon as the policy arrives |
| `update.*` | See below |
| `policy.refresh-minutes`, `policy.fast-apply` | Policy check interval |
| `diagnostics.page` | Show the Help/diagnostics view |

## Updates (portable, no installer)

At start (within a minute) and every `update.check-hours` (15 minutes after a failed check), and
when the customer clicks **Help → Check for updates**, TetherQS reads the signed release index of
its channel from every server: `https://<relay>/download/<update.channel>/releases-v2.json` and
`…/releases.json` ([updates.md](updates.md), [update-design.md](update-design.md)). Each is
verified against the compiled-in admin key; the highest serial over all servers wins and becomes
the floor (`releases-<channel>.serial`), below which no index is accepted again. When no server
has a valid one, the cached index is used. Its entry `customer/windows-x86_64` (macOS:
`customer/darwin-universal`, a zipped `.app`) describes the release. The unsigned
`…/customer/<platform>/latest.json` is not read by a build with an admin key
(`REQUIRE_SIGNED_INDEX` in `crates/tether-trust/src/release.rs`). The check also runs with
`update.enabled` off: then nothing is offered or downloaded ("Your organisation installs updates
for you."), but a minimum version still blocks. Help shows the result of
the last check ("… is up to date", "Version … is available", or "Check failed: <server>: <reason>").
A TetherQS that was already running when a release was published sees it only at its next check:
use **Check for updates** or restart it. If the version is newer and the staged rollout has
reached this device:

- The exe is downloaded in the background (https only, ≤ 200 MiB) and its minisign signature is
  verified against the compiled-in `TETHER_UPDATER_PUB_KEY` (Tauri format: `signature` is base64 of
  the `.sig` file text; the key is base64 of the minisign public key file, a bare `RW…` key also
  works). It waits as `update-<version>.staged` and counts as staged only when it verifies again:
  a broken or foreign file is downloaded anew.
- It is written to `TetherQS.exe.new` next to the running exe; the running exe is renamed to
  `TetherQS.exe.old` and the new one takes its name; then TetherQS restarts (same ID and password; see [ID](#id) for the one-time move from 0.5.29 on PCs that also run Tether)
  and deletes `.old` on the next start. If the folder is not writable the new exe goes to
  `%LOCALAPPDATA%\Tether-QS\TetherQS.exe` and is started from there.
- `mandatory` (with a newer version), `minimumVersion` or policy `update.minimum-version` above the
  running version make the update **required**: the backend closes incoming connections and the
  Accept button is disabled until it is installed. A device below a minimum version (the
  release's or the policy's) is offered the release whatever its place in the staged rollout or
  `update.rollout = late`; a halted release is never offered.
- `forced` (or a required update with policy `update.force`) installs automatically after
  `update.restart-seconds`, counted only while no session runs; otherwise the customer clicks
  **Update**. TetherQS never replaces its exe while a session runs: a forced update waits for the
  session to end again, and **Update** answers "The update restarts TetherQS: end the session
  first." The button shows the download
  in percent (the relays serve a few hundred KB/s, so the 35 MB exe can take a minute or two).

Publishing: Tether Manage › Releases ([updates.md](updates.md#release-procedure)); it signs the
exe with the updater key and writes the signed release index. A `latest.json` alone reaches no
TetherQS built with an admin key.

## Custom apps (branded downloads)

Tether Manage can create a **custom app**: a name, logo (PNG, at most 96 KiB), accent colour,
welcome text and customer settings, signed with the admin key (`tether_trust::custom::CustomProfile`)
and stored on every relay together with a private, expiring download link
`https://<relay>/q/<token>` (256-bit random token, at most 366 days; wire contract in
[api.md](api.md#custom-apps-branded-tetherqs-downloads)). The relay serves the channel's current
`TetherQS-<v>.exe` with the signed profile appended (`<signed JSON><u32 LE length>TQSPROF1`) as
`<Name>-Support.exe`, and to a Mac browser the TetherQS zip as `<Name>-Support.zip` holding
`<Name>-Support [<mac id>].app` (`/q/<token>/mac` and `/q/<token>/windows` choose explicitly). Unknown, revoked and expired links all answer `404`; 30 requests per minute
per IP; the admin sees download counts. The exe itself stays the same build: no per-customer compilation. If
the release exe is Authenticode-signed (`scripts/sign-windows.ps1`, see
[code-signing.md](code-signing.md#custom-tetherqs-apps)), the trailer goes at the end of its certificate
table (`tether_trust::custom::attach`: zero padding, then the trailer, and both table sizes in the
header grow), so the custom exe keeps the valid `Tether Code Signing` signature. Appended outside
the table it would show as NotSigned.

In TetherQS (Windows):
- At start it reads the tail of its own exe, verifies the profile against the compiled-in
  `TETHER_ADMIN_PUB_KEY` and validates it. A missing, damaged or untrusted trailer is logged and
  TetherQS runs as plain TetherQS.
- Name, accent and welcome replace `customer.title`, `customer.accent-color` and
  `customer.welcome` (window title and title bar); the logo is shown in the title bar; the Help view
  lists the custom app name.
- The settings layer is applied with `policy::resolve_with`: defaults < policy profile <
  **custom app** < group < machine. Its setting is ignored for every feature the profile locks
  (`off`, `locked`, `hidden`), so it cannot unlock what the policy locks (the signed profile cannot
  be revoked). The attached profile is opened leniently (`custom::open_app`): one setting this
  TetherQS does not know or finds invalid is skipped, not the whole custom app. Only customer-profile features are allowed in it.
- Self-update downloads and verifies the plain exe as usual, then writes it with this instance's
  profile attached unchanged (`custom::attach`, inside the signature like the relay does), so the
  customer keeps the branded, signed app (from TetherQS 0.5.30; 0.5.29 and earlier append it, so
  their updated copy works but shows as unsigned).

In TetherQS (macOS) the signed `.app` cannot carry a trailer, so:
- At start it reads the mac id from its own bundle folder name (`… [<16 hex>].app`; also under App
  Translocation, which keeps the name). No id: plain TetherQS, no request.
- With an id it uses `custom-<mac id>.json` from its config folder
  (`~/Library/Preferences/app.tether.Tether-QS/`), else fetches `https://<relay>/q/m/<mac id>` from
  each compiled-in relay in turn (10 s each, first start only) and caches it. The profile is
  verified and applied exactly as on Windows; a failure is logged ("custom app profile ignored")
  and TetherQS runs as plain TetherQS.
- Self-update replaces the bundle under the same folder name (also in the fallback folder), and the
  cache stays: still branded. Renaming the app in Finder removes the id (plain TetherQS).
- `TetherQS.app/Contents/MacOS/TetherQS --print-custom-profile` prints what was found and exits
  (0 = custom app).

Revoking or letting the link expire stops new downloads only; copies already downloaded keep
working (the profile has no expiry). On macOS a copy that was downloaded but never started before
a **revoke** or **New link** starts as plain TetherQS (its profile is fetched at the first start);
expiry alone does not affect it. Test helper without Tether Manage:
`cargo run --example custom_app -- <keyfile> <TetherQS.exe> <out.exe>` in `crates/tether-trust`
(prints the public key for `TETHER_ADMIN_PUB_KEY`, writes a signed test profile;
`--profile <out.json>` instead of the two files writes the profile alone, as `/q/m/` serves it).

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
- A release's `minimumVersion` and the policy key `update.minimum-version` block sessions also
  while `update.enabled` is off (the check still runs; nothing is offered or downloaded).
- Starting TetherQS twice: the second instance finds the first one's IPC and exits.
- Branding: text on a fill in the brand accent colour is black or white, whichever contrasts more
  (`onAccent` in `apps/quick/src/color.ts`). Only fills get a contrasting text colour: a dark
  brand colour is hard to read in the dark theme where the accent is used as text or icon colour
  (links, the Help icon).
- A connection that resumes after a network drop does not get back a permission that policy
  locked off in the meantime (the same in the technician app).
- Over-the-shoulder UAC with a different admin account works for the portable service (it runs as
  SYSTEM), but the customer must know those credentials.

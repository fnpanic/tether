# macOS build

`scripts/build-macos.sh` builds both apps on a Mac as **unsigned universal** binaries (one build runs
on Apple silicon and Intel, macOS 11 or newer). Builds are ad-hoc signed (`codesign -s -`), which
Apple silicon requires, but not signed with an Apple Developer ID and not notarized.

## Prerequisites

The script checks and installs what it can. You need:

- A Mac (Apple silicon or Intel) with about 25 GB free disk space and an internet connection.
- **Xcode Command Line Tools**: `xcode-select --install` (the script opens the dialog if missing).
- **Homebrew** (https://brew.sh). The script installs `llvm cmake ninja pkg-config node` with it.

Everything else is installed by the script under `.tools/` in the repository (nothing needs sudo):

| Tool | Why |
| --- | --- |
| Rust (rustup) + targets `aarch64-apple-darwin`, `x86_64-apple-darwin` | both halves of the universal binary |
| NASM 2.16.03, built from source | x86_64 assembly in aom/libvpx/ffmpeg; Homebrew's NASM 3.x does not work (same as RustDesk's CI) |
| vcpkg at the commit pinned in `vendor/rustdesk/vcpkg.json` (`.tools/vcpkg`, or your `VCPKG_ROOT`) | static libvpx, libyuv, opus, aom, libjpeg-turbo, ffmpeg for `arm64-osx` and `x64-osx`, with deployment target 11.0 |
| `scripts/bootstrap.sh` | pinned RustDesk sources in `vendor/` with `patches/*.patch` |

## Build

```bash
export TETHER_SERVER=tether-desk.duckdns.org
export TETHER_PUB_KEY=fe8A7xTX72RJlJypn8aBxG3NZHHxygCLPQqCWxbeev8=
export TETHER_ADMIN_PUB_KEY=LdVr9FeDLNbgO6T8PPHddPsicLPr5Hkly1MXqmaLwKI=
export TETHER_UPDATER_PUB_KEY=dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDFBRDBDM0ZCRENBM0I0RUEKUldUcXRLUGMrOFBRR3ZGdVZuSXp2QlBUTGRmUVl1bkVDWjk4Ymh6c0w2YnJBK0tVWm82YXZteFAK
bash scripts/build-macos.sh
```

These are the production values (relay `tether-desk.duckdns.org`). The script stops with this
list if one is missing; release builds refuse to compile without them. The first run takes long
(vcpkg builds ffmpeg and the codecs twice, one per architecture); later runs reuse everything.

Optional variables:

| Variable | Default | Effect |
| --- | --- | --- |
| `OUT` | `dist/macos` | Artifact folder |
| `VCPKG_ROOT` | `.tools/vcpkg` | Use an existing vcpkg |
| `TAURI_SIGNING_PRIVATE_KEY` (+ `_PASSWORD`) | – | Also write `.sig` files. Not needed: Tether Manage signs on publish. |
| `APPLE_SIGNING_IDENTITY` | `-` (ad-hoc) | A Developer ID identity from the keychain, for signed builds |
| `MACOSX_DEPLOYMENT_TARGET` | `11.0` | Oldest supported macOS |

## Artifacts

In `dist/macos/` (the script prints the paths at the end):

| File | What |
| --- | --- |
| `Tether_<v>_universal.dmg` | Technician app, for manual install (drag to Applications) |
| `Tether_<v>_universal.app.tar.gz` | Technician app, update archive for the built-in updater |
| `TetherQS-<v>-universal.zip` | TetherQS, portable `.app` (no installation), also its update archive |

The bundles themselves are in `apps/desktop/src-tauri/target/universal-apple-darwin/release/bundle/`
and `apps/quick/src-tauri/target/universal-apple-darwin/release/bundle/macos/`.

CI (`.github/workflows/desktop.yml`, job `macos`) runs the same script and uploads `dist/macos/*`.

## First start (Gatekeeper)

Unsigned apps downloaded from the internet are blocked with "cannot be opened because Apple cannot
check it for malicious software". Either:

- **Right-click (or Control-click) the app → Open → Open.** Needed once. On macOS 15 and later:
  try to open it once, then **System Settings → Privacy & Security → Open Anyway**.
  macOS then asks for an **administrator password**: that is macOS confirming the change to its
  security settings, not Tether asking for admin rights. Neither app needs admin rights to run.
- Or remove the quarantine flag in Terminal (no password, no Settings detour):
  ```bash
  xattr -dr com.apple.quarantine /Applications/Tether.app
  xattr -dr com.apple.quarantine ~/Downloads/TetherQS.app
  ```

Unzip TetherQS and move it out of Downloads (e.g. to the Desktop) before the first start: an app
started straight from a quarantined download runs from a read-only random location (App
Translocation), and its self-update then installs into
`~/Library/Application Support/Tether-QS/` instead of replacing the app.

## Permissions

To be controlled remotely, macOS needs the user's consent in **System Settings → Privacy &
Security**. macOS asks for Screen Recording the first time the screen is shared; enable the others
(and any prompt that was dismissed) there by hand:

| Permission | Needed for | Without it |
| --- | --- | --- |
| **Screen Recording** (Screen & System Audio Recording) | Sharing the screen | The technician waits for the first image forever (no picture is sent) |
| **Accessibility** | Remote mouse and keyboard | View only; input is ignored |
| Input Monitoring (only if listed) | Keyboard in some apps | Some keys do not arrive |
| Microphone (optional) | Sending audio | No audio |

After enabling Screen Recording macOS asks to quit and reopen the app; do that, then reconnect.

macOS ties these permissions to the app's code signature. Ad-hoc signed builds get a new signature
on every build: **after an update macOS asks again, although System Settings still shows the old
entry as allowed** (that entry belongs to the previous build). Fix on the Mac, once per app:

```sh
tccutil reset ScreenCapture app.tether.desktop    # Tether
tccutil reset ScreenCapture net.tether.quick      # TetherQS
tccutil reset Accessibility app.tether.desktop
tccutil reset Accessibility net.tether.quick
```

then quit and reopen the app and allow it once more (or remove the entries with − in System
Settings and add the apps again).

**Stable signature (from 0.5.3, free):** the Mac build runner signs both release apps with one
self-signed code-signing certificate (`Tether Code Signing`, SHA-1 `BCF53642…FB33`, see
[code-signing.md](code-signing.md#status-the-two-certificates)). CI does the same only when the
repository secrets `MACOS_SIGNING_P12` (base64 of that .p12) and `MACOS_SIGNING_PASSWORD` exist;
its builds are not released. The designated requirement then names that certificate
instead of one build's hash, so the permissions survive updates (one last re-grant when moving from
an ad-hoc build). Gatekeeper still needs **Open Anyway** once (no Apple notarization), as before.
The smoke test prints each app's signature and fails when the certificate was expected but not
used. Without the secrets CI signs ad-hoc as before. The key lives only on the admin's PC
(`C:\vms\macsign`, sent per job to the build runner) and in the admin's own backup, not in the
repository; the public certificate is `certs/tether-codesign-macos.cer`. Renewing it means one more
re-grant on every Mac. A Developer ID identity (`APPLE_SIGNING_IDENTITY` secret) takes precedence.

What the apps do about it (0.5.4):

- **On the Mac:** Tether (home page) and TetherQS show a warning while Screen Recording or
  Accessibility is missing, with **Fix Screen Recording** / **Fix Accessibility** buttons. Fix
  runs the `tccutil reset` above for this app only (no admin needed), so a switch that is on but
  belongs to an older build is cleared; macOS then asks afresh and opens the System Settings page.
  Switch the app on and choose **Quit & Reopen**.
- **When a session starts** on a Mac without these permissions, the Mac asks once per app run (not
  on every connection) and tells the technician. The technician's window shows "The Mac does not share its screen yet" with the
  steps above; with only Accessibility missing, a message says that input is ignored.
- Without that message (an older Mac app), the technician's window explains the likely cause
  after 8 s without a picture.
- **Accessibility "on" but clicks and keys ignored (fixed 2026-11-08):** macOS can report the
  Accessibility switch as granted (`AXIsProcessTrusted`) while it still refuses to let the app
  post events, e.g. a grant made for an earlier build or one switched on after the app started.
  The pointer then moves (tooltips appear) but clicks and keys do nothing, and up to 0.5.29 no
  warning appeared. Tether and TetherQS now also ask `CGPreflightPostEventAccess`; when it is
  refused they show the Accessibility warning (Mac) and the "input is ignored" message
  (technician). Fix: Fix Accessibility, switch the app on, then quit and reopen it. Don't press
  Fix again while the switch is on and the app has not been reopened: Fix clears the entry.

## Remote input and App Nap

On a Mac host, remote mouse and keyboard input is injected on Tether's main thread. Tether
minimizes its windows when a session starts; macOS would then put it into App Nap and throttle
that thread, so input stops arriving after a few seconds. From 0.5.13 Tether opts out of App Nap
(`NSProcessInfo beginActivity`, user-initiated and latency-critical; idle system sleep stays
allowed). The protection of Tether's own windows drops keys only while the active Tether also
has a window on screen, not merely because Tether is the active app.

Verified 2026-11-05 on an Apple Silicon Mac: before the fix, input died about 20 s after the
session started and every key was dropped; with 0.5.13 keys pass and input still arrives after
100 s.

### Drags: CGEvent and HID

Mouse input is posted as CGEvents (`libs/enigo/src/macos/macos_impl.rs`), with one exception:
**left-button drags** go through IOKit's `IOHIDPostEvent` (`hid_post.rs`). The Screenshot app's
selection (Cmd+Shift+5) grows only by the deltas of drags that arrive at the HID level; it draws
no rectangle for CGEvent drags, whatever their fields, while Cmd+Shift+4 and ordinary apps
take either. Details of the path, checked on macOS 15 (2026-11-10):

- Right before a left press, one HID move to the press point (button still up): the pointer got
  there by CGEvent moves, which the HID side does not see, and the first drag's delta must start
  from the press point. Without it the selection stopped at about a third of the drag.
- Each drag carries its step from the previous HID position as `dx`/`dy`, and the absolute
  position (`kIOHIDSetCursorPosition`).
- Presses and releases stay CGEvents: a HID click reached TextEdit but not the system's permission
  dialogs ("… would like to control this computer"), which a supporter must be able to click.
- If IOHIDSystem cannot be opened, drags use CGEvent as before (logged once).

E2E through Tether, from the PC against the build Mac (`tools/macbuild` jobs drive the Mac side):

```sh
TOOLBAR=1 node e2e/mac-drag-e2e.mjs     # Screenshot app: the rectangle reaches the drag's end
node e2e/mac-drag-e2e.mjs               # Cmd+Shift+4: macOS saves the dragged region
OVER_TETHER=1 node e2e/mac-drag-e2e.mjs # the same, starting on Tether's own window
node e2e/mac-click-e2e.mjs              # TextEdit: double-click selects a word, a drag selects text
```

They click and drag on the Mac's screen: run them while nobody uses the Mac (a window that comes
to the front ends a screenshot selection).

## Files through the clipboard

Files copied in Finder (⌘C) can be pasted in Explorer on the Windows side (Ctrl+V), and files copied in
Explorer can be pasted in Finder (⌘V), with the Mac as host or as viewer. The macOS builds enable the
core's `unix-file-copy-paste` feature for this (`[target.'cfg(target_os = "macos")'.dependencies]` in
both apps' `Cargo.toml`; Windows uses its own clipboard code and needs no feature). Mac ↔ Mac and Linux
are not covered here.

How the Mac receives files (`vendor/rustdesk/libs/clipboard/src/platform/unix/macos`): the pasteboard gets
an empty placeholder file `/tmp/.rustdesk_<uuid>`; when Finder pastes it somewhere in the home folder,
an fsevent watcher notices the copy, removes it and fetches the real files from the other side into
that folder. So a paste works only in the home folder (Desktop, Documents, Downloads, …), not on other
volumes. The placeholder stays until it is pasted, the other side copies something new, or the
session ends; the fsevent watch on the home folder runs only as long: from the moment the pasteboard
hands out the placeholder until the paste or the session's end (the core stopped both after 30 quiet
seconds, so a later ⌘V found nothing or went unnoticed; patched). Outside a session, or once the
files are pasted, no watch runs. One gap: copying something else on the Mac instead of pasting leaves
the watch on until the session ends (the core is not told about local pasteboard changes).

Same requirements as the clipboard itself (permission "clipboard", on by default) plus file transfer
allowed on the host side. Test: `node e2e/mac-fileclip-e2e.mjs` (build Mac + Windows VM: Mac viewer ↔
TetherQS in the VM, the technician app in the VM ↔ Mac host; the viewer must not run on a hidden desktop,
where Explorer never asks for the file contents).

## Windows and the Dock

Tether shows **one** icon in the Dock. Session windows (screen, file transfer, camera, terminal),
also those opened from `tether://` links, open in the main process, so they share its icon and
are listed in its Window menu.

The connection window on the controlled Mac (who is connected, chat, disconnect) runs in its own
process (`--cm`, started by the local server when someone connects). From 0.5.9 that process has
no Dock icon and no menu bar of its own when the main Tether runs (activation policy
*Accessory*): its window floats on top. A click on the Tether Dock icon is passed to it (the main
process signals it through `$TMPDIR/<identifier>-cm.sock` and gets one byte back):

- **during a session** it toggles only the connection window, like a single-window app: shown →
  minimized, minimized → back in front, as often as you like. The main window stays as it is:
  the `--cm` process answers before it toggles (up to 0.5.17 it answered after the restore
  animation, the main process stopped waiting after 500 ms and restored the main window too), and
  the main process does not open the main window while a host session is active (it learns that
  from `--minimize-main` and `--host-session-ended` on its single-instance socket).
- **without a session** the connection window is shown, and the main window opens if no Tether
  window is shown (as up to 0.5.11).

It does not appear in Cmd-Tab.

Exceptions, which keep their own Dock icon as before:

- The connection window when the main Tether does not run (its server was started some other
  way) or when the socket cannot be created: then it is the only way to reach the window.
- `tether --connect <id>` from Terminal while Tether does not run opens its session in a process
  of its own. While Tether runs, the request (with a `--password-env`/`--password-stdin`
  password) goes to the running Tether through the single-instance socket and the second process
  exits, as on Windows; only a socket owned by the same user gets it (`/tmp` is shared by all
  users). `tether://` links always go to the running Tether (macOS delivers them to it). The
  forwarding is built on the Mac but not yet tried there by hand (Windows: keyrot-e2e).

TetherQS runs its connection manager in its single process: one icon.

When a session starts, the connection window and the main window minimize (to the right side of
the Dock, never hidden; the `--cm` process tells the main process through the single-instance
socket `/tmp/<identifier with _>_si.sock`); whenever the connection window is minimized again during the session (yellow button, Dock
click), the main window minimizes with it (the `--cm` process checks its window
every 300 ms, macOS has no minimize event); a chat message from the technician, a new request or
a click on the Tether Dock icon brings the connection window back. Verified 2026-11-05 on an Apple Silicon Mac with 0.5.9: one Tether icon with a
screen and a file transfer session, the window stays minimized when the second session starts,
a chat message restores it on the Chat tab, a Dock click restores it.

## Retina displays

A Mac host streams a Retina display at its logical size (a 3456×2234 MacBook Pro screen as
1728×1117): a quarter of the pixels, so a key frame is ~4× smaller and the picture stays live
on slow relay paths (measured 0.3–0.6 Mbit/s on a congested evening). macOS scales the capture;
the Mac's own resolution does not change. To stream the full pixel size, set the host option
`tether-full-retina` to `Y`. On congestion a Mac host may lower its bitrate to ~0.3 Mbit/s
(other hosts keep the ~1 Mbit/s floor).

## Custom apps

A [custom app](management.md#custom-apps) link `https://<relay>/q/<token>` gives a Mac browser
`<Name>-Support.zip`, which unpacks to `<Name>-Support [<mac id>].app` (`/q/<token>/mac` forces it).
The app inside is the unchanged, signed TetherQS release: the relay only renames the bundle folder
in the zip headers (`server/api/src/maczip.rs`), because anything added inside the bundle (a
trailer on the Mach-O as on Windows, a file in `Resources/`) would break the code signature, and
the relay has no signing key. At the first start TetherQS reads the mac id from its folder name,
fetches the signed profile from `https://<relay>/q/m/<mac id>`, verifies it against the
compiled-in admin keys and caches it as `custom-<mac id>.json` in
`~/Library/Preferences/app.tether.Tether-QS/` ([quick-support.md](quick-support.md#custom-apps-branded-downloads)).
Self-updates keep the folder name (the new bundle is swapped in under the same path, or under the
same name in `~/Library/Application Support/Tether-QS/`) and the cache, so the app stays branded.

Check an app: `"<Name>-Support [<mac id>].app/Contents/MacOS/TetherQS" --print-custom-profile`
prints the custom app (or why it runs as plain TetherQS) and exits, 0 = custom app.

Limits: renaming the app in Finder drops the id (plain TetherQS); a copy that was never started
before the app was revoked or got a new link starts plain; the first start needs a relay.

### Why the folder name (checked on the Mac, 2026-11-07, macOS 15.7.4, arm64)

Through the build runner, with the 0.5.24 release zip and a TetherQS build of this change (signed
with "Tether Code Signing", a test admin key added for the run only); no install, own `HOME`:

| Check | Result |
| --- | --- |
| `codesign --verify --deep --strict` after `mv TetherQS.app "ACME GmbH-Support [0123456789abcdef].app"` | `valid on disk`, `satisfies its Designated Requirement` (DR: identifier `net.tether.quick` + certificate root; no path) |
| Zip renamed by tether-api's `maczip::rename_app` (byte-identical to an independent Python rewrite), quarantined (`0083;<time>;Safari;<UUID>`), unpacked with `ditto -x -k` and with Info-ZIP `unzip` | Same files as the `mv` copy; codesign valid in both |
| `spctl -a -vv -t exec`, original vs renamed, quarantined or not | Identical verdict: `rejected, origin=Tether Code Signing` (self-signed, not notarized: users approve once with Open Anyway, docs above). A notarized build would keep its ticket too: the ticket is bound to the code directory hash, not to the path. |
| `--print-custom-profile`, renamed copy with a cached profile | `custom app example-1 (Example Support), accent "#c8102e", 1 settings …`, exit 0 |
| Same, no cache | Fetched `https://<relay>/q/m/0123456789abcdef` from the compiled-in relay: `404` (endpoint not deployed yet) → `plain TetherQS: custom app …: could not fetch the profile: … 404`, nothing cached, exit 1 |
| Plain `TetherQS.app` with the same cache | `plain TetherQS (no custom app)`: no lookup without an id |
| Quarantined bundle exec'd from a shell | Killed by Gatekeeper (SIGKILL), for the renamed **and** the plain `TetherQS.app` alike: needs the user's Open Anyway, unrelated to the rename |
| `codesign` after the runs | Still valid (TetherQS writes nothing into its bundle) |

Rejected alternatives:
- **Extended attribute on the bundle** (`ditto -c -k --sequesterRsrc`): codesign still passes, but
  the attribute was gone after `ditto -x -k` and after `unzip`. Too fragile.
- **Signed build per custom app** (profile in `Contents/Resources`): robust, but needs the Mac and
  the signing key for every app and every release: about 5 minutes per incremental build job plus
  a re-sign and upload per app and update, and the relays would host one zip per app and version.
- **Download URL from the quarantine database** (`~/Library/Preferences/com.apple.LaunchServices.QuarantineEventsV2`):
  readable without permissions, but the newest entries on the test Mac (Edge) had no URL at all,
  other unpackers do not record one, and the URL would contain the secret token.

**App Translocation: verified by hand (2026-11-07).** A custom link opened in Safari on the Mac, the app
started from Downloads (so macOS runs it from a random read-only translocation path), showed the custom
app's name and branding. Translocation keeps the bundle name
(`/private/var/folders/…/AppTranslocation/<UUID>/d/<Name> [<id>].app`), which is all TetherQS needs. The
runner cannot test this itself: `SecTranslocateCreateSecureDirectoryForURL` is refused in its sandbox and
a GUI start needs Open Anyway at the unlocked Mac.

## Publish with Tether Manage

1. Copy `dist/macos/` to the PC with Tether Manage (keep the `.dmg` next to the `.app.tar.gz`).
2. Tether Manage → unlock → **Releases**:
   - **Choose…** `Tether_<v>_universal.app.tar.gz`: App, Platform (macOS) and Version are filled in.
     Tether Manage signs it and uploads it, its `.sig`, the `.dmg` (+ `.sig`) and last
     `latest.json` to `download/<channel>/technician/darwin-universal/`. The manifest lists the
     build as `darwin-universal`, `darwin-aarch64` and `darwin-x86_64`.
   - **Choose…** `TetherQS-<v>-universal.zip`: same, to `download/<channel>/customer/darwin-universal/`.
3. **Download page → Publish** again: it then offers the macOS downloads, and shows them first to
   Mac visitors.

Details: [updates.md](updates.md), [management.md](management.md#releases).

## Not verified without a Mac

CI (`scripts/smoke-macos.sh`, on GitHub's macOS runner) checks after every build that both apps
open a window and show no admin prompt. Everything else was written and reviewed on Windows only;
check it on a real Mac:
the vcpkg x86_64 cross build on Apple silicon (and arm64 on Intel), the Tauri universal bundle and
dmg (the dmg step drives Finder via AppleScript and may ask for permission to control Finder),
ad-hoc signing with the entitlements, the RustDesk core's macOS screen capture and input in
Tether's Tauri shell, the permission prompts, and both update paths.

### Checked on a Mac with 0.5.20 (2026-11-06)

- Host session, Tether Dock icon clicked four times (one click at a time, the icon's position
  re-read from a screenshot each time, since a minimized window adds a Dock thumbnail and shifts
  the icons): the connection window was shown, minimized, shown, minimized; the main window never
  appeared.
- PC technician → Mac: Ctrl+N arrived as ⌘N (a new Terminal window opened).
- Accept with one click could not be checked from a session, by design: the host drops remote
  clicks on Tether's own windows (`own_ui_guard`), so the request ignored both remote clicks. It
  needs a person at the Mac: make another app active, then click Accept once.

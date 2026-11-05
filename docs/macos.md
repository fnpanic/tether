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

**Stable signature (from 0.5.3, free):** CI signs both apps with one self-signed code-signing
certificate (`Tether Code Signing`) when the repository secrets `MACOS_SIGNING_P12` (base64 of the
.p12) and `MACOS_SIGNING_PASSWORD` exist. The designated requirement then names that certificate
instead of one build's hash, so the permissions survive updates (one last re-grant when moving from
an ad-hoc build). Gatekeeper still needs **Open Anyway** once (no Apple notarization), as before.
The smoke test prints each app's signature and fails when the certificate was expected but not
used. Without the secrets CI signs ad-hoc as before. The certificate and its key live only in the
secrets and in the admin's own backup (not in the repository); renewing it means one more re-grant
on every Mac. A Developer ID identity (`APPLE_SIGNING_IDENTITY` secret) takes precedence.

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
  minimized, minimized → back in front, as often as you like. The main window stays as it is.
- **without a session** the connection window is shown, and the main window opens if no Tether
  window is shown (as up to 0.5.11).

It does not appear in Cmd-Tab.

Exceptions, which keep their own Dock icon as before:

- The connection window when the main Tether does not run (its server was started some other
  way) or when the socket cannot be created: then it is the only way to reach the window.
- `tether --connect <id>` from Terminal opens its session in a separate process (Windows hands it
  to the running Tether; macOS does not yet).

TetherQS runs its connection manager in its single process: one icon.

When a session starts, the connection window and the main window minimize (to the right side of
the Dock, never hidden; the `--cm` process tells the main process through the single-instance
socket `/tmp/<identifier with _>_si.sock`); a chat message from the technician, a new request or
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

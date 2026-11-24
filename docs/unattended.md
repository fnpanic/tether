# Unattended access (Windows service, macOS launchd)

Windows first; the Mac is described under [macOS](#macos).

First verified end to end on 2026-11-07 with **Tether 0.5.23**, which needed three workarounds (bugs below).
Re-verified the same day **without any workaround** on the `svcfix` build (0.5.23 + the fixes listed under
[Bugs fixed](#bugs-fixed)), release-like (production keys and relays), in the Hyper-V test VM `tether-bench`
(Windows 11 LTSC, local admin `bench`, corporate mode: all outbound traffic blocked except the TLS-inspecting
proxy 172.30.240.1:8080, whose CA the machine trusts). Technician: the same build on the host, on a hidden
desktop, driven through its devtools (`apps/desktop/e2e/unattended-drive.mjs`). Relays: production
(tether-desk / tether-desk2).

## Result (svcfix build, no workarounds)

| Step | Result | Notes |
|---|---|---|
| 1. Fresh install (NSIS `/S`) | PASS | 5 s. `InstallLocation` = `C:\Program Files\Tether` (unquoted). Settings shows *Install as service* (no service yet). |
| 2. Set the permanent password, *Accepted passwords* = Permanent, **then** *Install as service* | PASS | Click 17:15:39 → service RUNNING (AUTO_START, LocalSystem), `--service` in session 0 and `--server` in the user session at 17:15:40, Tether reopened at 17:15:41 with *Install as service* gone. The service's config has the same password and verification method (`import config from …` in the import-config log). |
| 3. Service online behind the proxy | PASS | Machine proxy set the usual way for services (`netsh winhttp set proxy 172.30.240.1:8080`), SYSTEM's IE settings left empty: `using system proxy http://172.30.240.1:8080` at the next 60 s refresh, registered on both relays through the inspecting proxy (TLS verified) 9 s later. |
| 4. Connect with the permanent password | PASS | Production relays via the proxy, session up ~9 s after starting the viewer. |
| 5. Sign out, sign in again through Tether | PASS | Win+X → U → I: logon screen visible through Tether within 30 s; password typed with `session_type_text` + Enter: desktop within 20 s. |
| 6. Reboot (auto-logon off) → logon screen → sign in through Tether | PASS | *Restart remote computer* 17:20:03; boot 17:20:10, service 17:20:14, **registered via the proxy 17:20:15** (nobody signed in), and the **viewer reconnected by itself** at 17:20:16, 13 s after the restart command, showing the logon screen. Signed in through Tether. |
| 7. Install → **then** set the password | PASS | New password set in Settings while the service runs: stored by the service (17:29:12), connected with it. |
| 8. `tether.exe --install-service` failure | PASS | With the old quoted value put back: exit code 1, no service (0.5.23: exit code 0). |
| 9. Update 0.5.23 → svcfix | PASS | The 0.5.23 install has `InstallLocation = "C:\Program Files\Tether"`; installing the svcfix build over it (`/S`, the same install section the in-app updater runs) rewrites it unquoted; *Install as service* then works from Settings (service RUNNING, settings imported). |

Screenshots (host, `C:\vms\svcfix\`):

| File | Shows |
|---|---|
| `a01-settings-before.png`, `a02-pw-set.png` | Fresh install: *Install as service* offered; permanent password saved |
| `a03-after-install.png` | Tether reopened after *Install* (service running) |
| `a04-settings-service-running.png`, `a05-pw-changed-service.png` | Service running: row gone; password changed while the service runs |
| `b01-perm-connected.png`, `b02-pw2-connected.png` | Connected with the password set before / after installing the service |
| `c05-console-text-vs-keys.png` | PowerShell: `session_type_text` (`typedtext`) does not arrive, key events (`q`, `w`, `x`, `z`) do |
| `d01-signout-menu.png` → `d02-after-signout.png` → `d03-signed-in.png` | Sign out through Tether → logon screen → signed in through Tether |
| `e01-viewer-during-reboot.png` → `e02-signed-in-after-reboot.png` | Viewer back by itself on the logon screen after the reboot → signed in |
| `f01-after-update-settings.png` | After the update from 0.5.23: *Install as service* offered (no service yet) |

0.5.23 run (with workarounds): screenshots in `C:\vms\svc\` (`u10-after-install-click.png` = the old failure).

## Verified flow

1. Install Tether (NSIS, `/S` works; 5 s).
2. Start Tether, Settings → *Unattended access*: set the permanent password (and the accepted passwords /
   two-factor options), before or after the next step; both work.
3. *Install as service* → *Install*. Admin rights are needed (UAC prompt; with UAC at default it appears on the
   local secure desktop, so a technician connected to the user-mode app or to TetherQS without *Allow admin
   access* cannot answer it). The service is created and started, the user's settings are copied into it, and
   Tether reopens (its host role moves into the service). An error or a cancelled UAC prompt is shown under the
   button.
4. Behind a proxy-only network the service needs the machine proxy: `netsh winhttp set proxy <proxy:port>` (or
   `netsh winhttp import proxy source=ie`, or the equivalent GPO), as for any Windows service. WPAD also works.
5. Connect with the ID and the permanent password. The service shows the logon screen, UAC prompts and the
   desktop of whoever is signed in; signing out/in and rebooting keep the device reachable.

## Bugs fixed

### Bug 1: *Install as service* failed on every NSIS install

Root cause: the NSIS installer (`apps/desktop/src-tauri/windows/installer.nsi`, from the Tauri template) wrote
the uninstall key's `InstallLocation` **with quotes** (`"C:\Program Files\Tether"`). The core derives the install
path and exe from it (`get_install_info_with_subkey`, `vendor/rustdesk/src/platform/windows.rs`) and its
cmd-injection check `validate_install_value` rejects `"`: Settings → *Install* failed with a native box
"RustDesk Output — Installation failed: Installer path or name contains characters unsafe for cmd.exe!", and
`tether.exe --install-service` logged the same error and exited 0. The same value made `is_installed()` false,
which was the only reason the Settings row showed (`!status.installed`). The button also called the core's
full self-install (`install_me`), which on an NSIS install would first run the core's uninstall script
(deleting the install folder) and then copy the exe onto itself.

Fix:

- `installer.nsi` writes `InstallLocation` unquoted (nothing else reads it: the installer uses `MANUPRODUCTKEY`).
  Existing installs get the unquoted value with their next update, which runs the same install section (step 9).
- *Install* calls the core's `install_service()` (the `--install-service` path: UAC, import the user's settings,
  `sc create` + start with rollback), never `install_me`; NSIS keeps owning files, shortcuts, uninstaller and
  updates. On success the core reopens Tether; errors and cancel are shown under the button.
- `status.installed` means "the service runs" on Windows (`api::is_service_installed`), so the row appears exactly
  while unattended access is off.
- `--install-service` exits 1 on failure; the native message box is titled "Tether Output".

`is_installed()` is now true for every NSIS install of Tether, as on an upstream install with the service
stopped (no portable-service elevation from the full Tether app, which has no UI for it anyway; TetherQS has
its own app name and is unaffected).

### Bug 2: the service ignored the machine proxy

Root cause: the service (`--service` and its `--server`, both LocalSystem) detects the proxy with
`WinHttpGetIEProxyConfigForCurrentUser` (`apps/desktop/src-tauri/src/netpolicy.rs`), i.e. SYSTEM's own IE
settings, which are empty; the machine WinHTTP proxy was never read. Behind a proxy-only network the service was
never online.

Fix: when running as LocalSystem and the IE/PAC/WPAD lookup finds nothing, `netpolicy` uses the machine WinHTTP
proxy (`WinHttpGetDefaultProxyConfiguration`). Apps in the user's session are unchanged. TLS to the inspected
relays still verifies against the machine-trusted proxy CA (step 3).

Not done: falling back to the signed-in user's IE proxy. It would not help at the logon screen (nobody signed
in), so a proxy-only network needs the machine proxy anyway.

### Bug 3: a permanent password set before installing the service was lost

Root cause: on install the core copies the user's config into the service's with `--import-config`
(`vendor/rustdesk/src/core_main.rs`), but only if the user's config is **older than the exe**
(`get_modified_time(path) < get_exe_time()`, made for its own copy-then-import install). With NSIS the exe dates
from the Tether install, so a password set after installing Tether was never imported. Setting it while the
service runs always worked (it goes to the service over IPC).

Fix: that condition is dropped; the import still only happens when the user's config is newer than the
service's. The password (`Tether.toml`) and the options in `Tether2.toml` (accepted passwords, and the 2FA
secret, which was not exercised in the VM) reach the service (steps 2 and 7).

### Also fixed: no reconnect after *Restart remote computer*

The viewer stayed at "Restarting remote device / Connection in progress" although the device was back after
10 s: the core ends the session with a `restarting` message box, which the reconnect policy
(`apps/desktop/src/lib/reconnect.ts`) classified as fatal. It is now retryable, so the usual reconnect series
runs (step 6: back 13 s after the restart command).

## Policy in the service

Added after the runs above (not part of them; unit tests in `apps/desktop/src-tauri/src/policy.rs`).

- The service's `--server` process checks the policy on the usual interval (`policy::start`) and
  refuses **new** incoming sessions while policy `connect.incoming` is off or policy
  `update.minimum-version` is above its version, also at the logon screen: it sets the core option
  `tether-refuse-incoming` (`policy::apply_incoming`), and the core answers the viewer in
  `Connection::on_open`, before any password or Accept, with "This device does not accept incoming
  sessions (set by its organisation)." or "This device must be updated before it accepts
  sessions." Sessions that already run are not ended.
- The service has no release state of its own. A mandatory release or a release's minimum version
  is enforced by the app in the user's session and its connection window (which closes the new
  connection), so not at the logon screen.
- Group policy: the service runs as another account with its own config folder and device id.
  The app of the signed-in user publishes its device id to the service (core option
  `tether-install-uuid`, only from the session the service serves); from its next policy check the
  service resolves the policy with that id, so the group Tether Manage assigned to the device
  applies. Before a user has opened Tether, only the profile and the machine policy apply.
- A policy that locks a permission off takes it from running sessions and from sessions that
  resume, where the connection window runs (`sink::enforce_permission_locks` and `revoke_locked`,
  in the `--cm` process). A running file-transfer connection is closed when file transfer is
  locked off.

## Open

- `session_type_text` does not reach **console windows** (PowerShell/cmd, conhost): it injects characters as
  `KEYEVENTF_UNICODE` (VK_PACKET), which conhost ignores; GUI fields (the logon password field included) accept
  them, and key events (keys typed one by one, Enter) reach consoles too (`c05-console-text-vs-keys.png`).
  A fix would map characters to virtual keys of the foreground window's layout (`VkKeyScanEx`) when the target
  is a console; not small enough to do here. Workaround: type the keys, or paste through the shared clipboard.
- The Tether window (and its native message boxes) cannot be clicked by the remote technician by design
  (`own_ui_guard`), so only the person at the PC (or an admin tool) can enable unattended access.
- Test harness only: a Tether window started with `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=…`
  and a connection manager (`--cm`) without it cannot run side by side in one user session (same WebView2
  user-data folder, different browser arguments): whichever starts second aborts with 0xc0000409. Seen in both
  directions (CM crash loop in the 0.5.23 run; the devtools window failing to start while a session was open in
  this run). A CM that aborts on a WebView2 init error is fragile; worth a graceful error.
- TetherQS and the Tether service on the same machine report the **same ID** (490209544 here) and both register
  it on the relays; which one a technician reaches is a race. Don't run both on one PC (the test VM is restored
  without the service for this reason). Fixed in TetherQS 0.5.29: it has its own ID ([quick-support.md](quick-support.md#id)).

## Reproduce

Elevated helpers (through the broker, `C:\vms\broker\run-elevated.ps1 <script>`): `vm-autologon.ps1` (auto-logon
bench), `vm-svc-setup.ps1 -Installer <setup.exe>` (copy + install, start Tether with devtools on 9232 → port
9235), `vm-svc-proxy.ps1 -WinHttp set|reset`, `vm-svc-cli.ps1 -Install|-Uninstall|-StopUi`,
`vm-svc-unquote.ps1 -Requote` (old quoted value), `vm-svc-reg.ps1`, `vm-svc-noautologon.ps1`, `vm-svc-log.ps1`
(service state + logs), `vm-svcfix-cfg.ps1` (InstallLocation, WinHTTP proxy, which config holds which password
as a hash prefix, import-config log, proxy log lines), `vm-svc-cleanup.ps1` (uninstall + undo),
`vm-qs-setup.ps1 -Qs <TetherQS.exe>` (back to customer + TetherQS).

VM app (host, `C:\vms\svcfix\vmapp.cjs`, password from `C:\vms\payload\svc-perm.json` or `PWFILE`):
`node vmapp.cjs setpw install state shot:<png>`.

Technician side (`VIEWER` = the build under test, `PROF` = a separate profile):

```sh
node apps/desktop/e2e/unattended-drive.mjs start 490209544 C:/vms/payload/svc-perm.json
node apps/desktop/e2e/unattended-drive.mjs combo:MetaLeft+KeyX wait:1500 combo:KeyU wait:1000 combo:KeyI   # sign out
node apps/desktop/e2e/unattended-drive.mjs pwfile:C:/vms/payload/bench.json,enter wait:20000 shot:desktop.png
node apps/desktop/e2e/unattended-drive.mjs restart
```

Logs in the VM: service/server `C:\Windows\ServiceProfiles\LocalService\AppData\Roaming\Tether\log\server\`,
settings import `…\log\import-config\`, installer CLI `C:\Users\<user>\AppData\Roaming\Tether\log\install-service\`,
user app and CM `C:\Users\<user>\AppData\Roaming\Tether\log\` (`cm\`).

## Instance builds

An instance build (`TETHER_INSTANCE=north`,
[several-organisations.md](several-organisations.md#two-organisations-on-one-pc)) installs its own
service beside the default build's. Every name on this page follows the product name
`Tether-north`, because the core takes them from the app name: the Windows service `Tether-north`
(`sc create Tether-north`, `vendor/rustdesk/src/platform/windows.rs`), its settings and logs under
`…\LocalService\AppData\Roaming\Tether-north\` and `%APPDATA%\Tether-north\`, and on macOS
`/Library/LaunchDaemons/app.tether.Tether-north_service.plist`,
`/Library/LaunchAgents/app.tether.Tether-north_server.plist` and
`~/Library/Preferences/app.tether.Tether-north/` (`get_full_name`, `vendor/rustdesk/src/common.rs`),
running `/Applications/Tether-north.app/Contents/MacOS/tether-north`. The installer's hooks use the
product name too (`apps/desktop/src-tauri/windows/hooks.nsh`), so uninstalling one build leaves the
other's service alone. None of this was tried on a real machine: no instance build has been
installed yet.

## macOS

From 0.6.3. Settings → Unattended access → *Install as service* → *Install* (shown while the
service is not installed and policy `security.unattended` allows it), or from Terminal
`/Applications/Tether.app/Contents/MacOS/tether --install-service` (exit code 1 when cancelled or
failed). Tether must run from `/Applications`.

What it installs, after the macOS administrator prompt ("Tether wants to install daemon and
agent"):

| File | Runs | As | When |
|---|---|---|---|
| `/Library/LaunchDaemons/app.tether.Tether_service.plist` | `tether --service` | root | from boot. No host: it keeps root's copy of the settings (ID, key, passwords) in step with the user's, over a local socket. |
| `/Library/LaunchAgents/app.tether.Tether_server.plist` | `tether --server` | the signed-in user | in every user session (the host there, also with Tether closed and behind the lock screen) |
| the same file | `tether --server` | root | at the login window (nobody signed in) |

The user's settings (`~/Library/Preferences/app.tether.Tether/Tether.toml`, `Tether2.toml`) are
copied to `/var/root/Library/Preferences/app.tether.Tether/` at install, so the login window
answers under the same ID and permanent password; later changes reach root through the daemon.
Tether then quits, the agent takes the host role and Tether opens again a few seconds later as a
window onto that host. *Install as service* is gone from Settings while both files exist.
With `--install-service` a running Tether is closed by the agent and opened again.

While the agent runs, opening Tether (Dock, Finder, `open`) is answered by the agent, which starts
the window as its child process. The agent's launchd file therefore has `AbandonProcessGroup`:
without it launchd closes the window whenever the agent restarts.

- **Permissions** (Screen Recording, Accessibility) belong to the app's signature, so the agent
  uses what Tether was given; nothing new to allow.
- **No menu bar icon** from the background host, as on Windows; the connection window appears
  when someone connects. Started by the agent it has its own Dock icon (it cannot borrow the main
  window's, [macos.md](macos.md#windows-and-the-dock)).
- **Updates:** the updater replaces `/Applications/Tether.app` as before. The agent and the
  daemon check every 10 s whether their binary on disk is another file and then exit as soon as
  no session runs; launchd starts them from the new app.
- **Remove:** `/Applications/Tether.app/Contents/MacOS/tether --uninstall-service` (administrator
  prompt; unloads the daemon, deletes both files, ends the agent). An open Tether window notices
  after 15 s that nobody hosts and hosts itself again, as before the install; with Tether closed
  the Mac is unreachable until Tether is opened. Do this before moving Tether to the Trash:
  otherwise launchd keeps trying to start the missing program.
- **Policy** at the login window: root's `--server` has its own configuration folder, as the
  Windows service has ([Policy in the service](#policy-in-the-service)).

Limits:

- **FileVault:** after a restart the Mac asks for the disk password before macOS and the network
  start. Nothing can connect until someone has unlocked the disk at the Mac (or the restart was
  made with `sudo fdesetup authrestart`). After signing out, and behind the lock screen, the Mac
  stays reachable.
- The root daemon and the login-window host run the program in `/Applications/Tether.app`, which
  a user of the `admin` group can replace without a password. Same as upstream RustDesk and other
  drag-installed apps with a root helper; see [SECURITY.md](../SECURITY.md).
- Installing needs a person at the Mac: a technician connected through Tether cannot click
  Tether's own window, and macOS does not accept remote input in its administrator prompt.

How it differs from the core (RustDesk) it is built on: the core's install script names its
launchd files `com.carriez.<app>_…` and points them at `<app>.app/Contents/MacOS/service` and
`…/MacOS/<app>`; Tether looks for `app.tether.Tether_…` and has one binary `tether`. The names
and paths are corrected where the scripts are read (`correct_app_name`,
`vendor/rustdesk/src/platform/macos.rs`), the root folder is created before the copy, the
daemon does not run the core's own auto-update, and the install waits for the result
(`install_daemon`) instead of running in the background.

### Checked on the build Mac (2026-11-23)

MacBook Pro (Apple silicon), macOS 27.2.1, FileVault on, user in the `admin` group, behind a
corporate proxy; arm64 test builds of 0.6.3 signed with "Tether Code Signing"; technician: a
Windows viewer on a hidden desktop (`C:ms
ight\mac56.mjs`), commands on the Mac through a
shell. The administrator prompts were answered by the person at the Mac.

| Step | Result | Notes |
|---|---|---|
| `tether --install-service` with Tether open | PASS | Prompt answered after 71 s, exit code 0. Both files written (`root:wheel`, names and paths as in the table above), `tether --service` running as root, agent running as the user (first start refused: the open Tether still held the socket; second start hosts), Tether reopened as the agent's child. |
| Settings reach the agent | PASS | Agent log `sync config from root`; same ID as before, registered on both relays through the proxy 1 s after its start. |
| Session with the permanent password | PASS | Direct (LAN), picture 1728×1116; a pointer move to 60 % / 40 % of the screen arrived at 0.600 / 0.399; connection window started by the agent. File transfer (a build runner download) too. |
| App replaced on disk | PASS | Binary touched during a session: the agent kept running; 44 s after the viewer closed (the core holds an ended connection that long) it exited with code 1 and launchd started it again; the daemon restarted within 25 s. The Tether window stayed open. |
| Real replacement (build runner swap, service installed) | PASS | Daemon restarted itself 8 s after the swap, agent and window back on the new build, online again. |
| `tether --uninstall-service` with Tether open | PASS | Prompt answered, exit code 0; both files and both launchd jobs gone; the open window logged `no host for 15 s: hosting from the main window's process` and was registered on both relays 1 s later; a session and a file transfer worked afterwards. |

Before the real install the agent role was tried without administrator rights (the same agent
file in `~/Library/LaunchAgents`): host under launchd, session with picture and input, restart
after a touched binary. That run showed the window being closed with the agent, hence
`AbandonProcessGroup`.

Not checked:

- **The login window.** Nobody was signed out: root's `--server` there, its picture and input,
  and policy in that role are as the core provides them, unverified with Tether.
- **The *Install* button in Settings.** It calls the same function as `--install-service`; what
  differs (Tether quits by itself before the agent is loaded) was not clicked through.
- A restart of the Mac, fast user switching, an Intel Mac, the universal build in this role, a
  cancelled prompt, and an update through the in-app updater (the swap above replaces the bundle
  the same way).

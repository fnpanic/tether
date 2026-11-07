# Unattended access (Windows service)

Verified end to end on 2026-11-07 with **Tether 0.5.23** (installer `dist/windows/Tether_0.5.23_x64-setup.exe`)
in the Hyper-V test VM `tether-bench` (Windows 11 LTSC, local admin `bench`, corporate mode: all outbound
traffic blocked except the TLS-inspecting proxy 172.30.240.1:8080). Technician: the 0.5.23 viewer on the
host, on a hidden desktop, driven through its devtools (`apps/desktop/e2e/unattended-drive.mjs`). Relays:
production (tether-desk / tether-desk2).

## Result

| Step | Result | Notes |
|---|---|---|
| 1. Install as service | **FAIL in 0.5.23** (passes with a workaround) | Settings → *Install* and `tether.exe --install-service` both refuse: see [Bug 1](#bug-1-install-as-service-fails-on-every-nsis-install). With the workaround: `sc query Tether` RUNNING, START_TYPE AUTO_START, LocalSystem, `--service` in session 0 + `--server` in the user session. |
| 2. Connect with the permanent password | PASS (after [Bug 2](#bug-2-the-service-ignores-the-machine-proxy) workaround and setting the password again, [Bug 3](#bug-3-a-permanent-password-set-before-installing-the-service-is-lost)) | Relay, via the proxy. |
| 3. Sign out, sign in again through Tether | PASS | Start → user → *Sign out*: the session follows to the Windows logon screen (~40 s). Password typed with `session_type_text`, Enter as key event: desktop back in ~20 s. |
| 4. Reboot (auto-logon off) → service back → logon screen → sign in through Tether | PASS | *Restart remote computer* (`session_restart_remote`) at 16:19:40; boot 16:19:46, service 16:19:49, **online (registered via proxy) 16:19:50 = 10 s after the restart command**. New connection with the permanent password shows the logon screen; sign-in through Tether works. |

Screenshots (host, `C:\vms\svc\`):

| File | Shows |
|---|---|
| `u10-after-install-click.png` | Settings → *Install*: native box "RustDesk Output — Installation failed: Installer path or name contains characters unsafe for cmd.exe!" |
| `u13-settings-pw-service.png` | Permanent password saved while the service runs |
| `v02-perm-connected.png` | Connected with the permanent password (service) |
| `w09-click-signout.png` → `w10-after-signout.png` | Sign out through Tether → logon screen seen through Tether |
| `x01-typed.png` → `x03-desktop.png` | Password typed through Tether → desktop |
| `y01-console-after-reboot.png` | VM console after the reboot: logon screen (auto-logon off) |
| `y03-logon-after-reboot.png` → `y04-desktop-after-reboot.png` | After reboot: logon screen through Tether → signed in through Tether |

## Verified flow

1. Install Tether (NSIS, `/S` works; 6 s).
2. Start Tether, Settings → *Unattended access* → *Install*. Admin rights are needed: with UAC at
   default the prompt appears on the local secure desktop; a technician connected to the user-mode app
   (or to TetherQS without *Allow admin access*) cannot see or answer it ("Please wait for confirmation of UAC").
3. **After** the service runs, Settings → *Permanent password* → *Set password* (it is then stored by the service).
4. Connect with the ID and the permanent password. The service shows the logon screen, UAC prompts and the
   desktop of whoever is signed in; signing out/in and rebooting keep the device reachable.

## Bugs found

### Bug 1: *Install as service* fails on every NSIS install

The Tauri NSIS installer writes the uninstall key's `InstallLocation` **with quotes**:
`HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Tether` → `InstallLocation = "C:\Program Files\Tether"`
(also on the host PC). The core derives the install path and exe from it (`get_install_info_with_subkey`,
`vendor/rustdesk/src/platform/windows.rs`), and 0.5.23's cmd-injection hardening
`validate_install_value` (`platform/windows/installer_shell.rs`) rejects `"`:

- UI (*Install*, `install_me`): log `ERROR [src\ui_interface.rs:111] Installation failed: Installer path or name contains characters unsafe for cmd.exe` and a native message box titled **"RustDesk Output"** (branding leak) in the user's session.
- CLI `tether.exe --install-service`: log `Failed to prepare service installation: Installer path or name contains characters unsafe for cmd.exe`, **exit code 0**, no service.

Second effect of the same value: `is_installed()` checks `"C:\Program Files\Tether"\Tether.exe`, which does not
exist, so the app believes it is not installed. That is the only reason the *Install as service* row is shown
at all: Settings shows it when `!status.installed`, and `installed` means "tether.exe at InstallLocation", not
"service installed". With a correct (unquoted) InstallLocation the row disappears and the UI offers no way to
install the service.

Fix direction: trim quotes from `InstallLocation` when reading it (and/or write it unquoted in the NSIS
hooks); base the Settings row on whether the service exists; show install errors in the app instead of a
native "RustDesk Output" box; non-zero exit code for a failed `--install-service`.

Test workaround used here (VM only, `C:\vms\vm-svc-unquote.ps1 -Unquote`): strip the quotes, then
`tether.exe --install-service` (1 s, service RUNNING / AUTO_START).

### Bug 2: the service ignores the machine proxy

The service (`--server` as LocalSystem) detects the proxy with `WinHttpGetIEProxyConfigForCurrentUser`
(`apps/desktop/src-tauri/src/netpolicy.rs`), i.e. SYSTEM's own IE settings, which are empty. It does not
fall back to the machine WinHTTP proxy (`netsh winhttp set proxy`, what admins set for services):
verified, with WinHTTP set for 2 minutes the service kept logging `proxy: false` and
`rendezvous mediator error ... An attempt was made to access a socket in a way forbidden`, and the viewer
said "The remote device is offline". Behind a proxy-only network the service is therefore never online.
Worked once SYSTEM's IE proxy was set (`HKU\S-1-5-18\...\Internet Settings`, `C:\vms\vm-svc-proxy.ps1 -SystemIe set`):
`using system proxy http://172.30.240.1:8080` within the 60 s refresh, registered right after.
Fix direction: in `platform_proxy`, also use `WinHttpGetDefaultProxyConfiguration`; for the service, the
signed-in user's IE/PAC settings are a reasonable further fallback.

### Bug 3: a permanent password set before installing the service is lost

Set in the user-mode app ("Password saved."), then service installed: connecting with it asks for the
one-time password. Setting it again while the service runs works. Either carry the password over on
install or tell the user to set it after installing.

### Other observations

- The Tether window (and its native message boxes) cannot be clicked by the remote technician by design
  (`own_ui_guard`: "The Tether window on the remote computer can only be used by the person at that
  computer"). So the technician cannot enable unattended access remotely; the person at the PC (or an
  admin tool) must. Step 1 here was driven through the VM app's devtools.
- `session_type_text` works in GUI fields, the Windows logon password field included (no per-key fallback
  needed), but **not in console windows** (PowerShell/conhost): nothing arrives; single key events (Enter) do.
- After *Restart remote computer* the viewer stayed at "Restarting remote device / Connection in progress"
  for minutes although the device was online again after 10 s; a new connection worked at once.
- The service-started connection manager (`Tether.exe --cm`) crashed in a loop (0xc0000409, hundreds of WER
  events, viewer reconnecting every ~2 s) while a Tether UI started with
  `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=...` ran in the same user session (test
  harness only: same WebView2 user-data folder, different browser arguments). It stopped as soon as that UI
  exited. A CM that aborts on a WebView2 init error is fragile; worth a graceful error.
- TetherQS and the Tether service on the same machine report the **same ID** (490209544 here) and both
  register it on the relays; which one a technician reaches is a race. Don't run both on one PC (the test VM
  was restored without the service for this reason).

## Reproduce

Elevated helpers (through the broker, `C:\vms\broker\run-elevated.ps1 <script>`): `vm-autologon.ps1`
(auto-logon bench), `vm-svc-setup.ps1` (copy + install, start Tether with devtools on 9232 → port 9235),
`vm-svc-unquote.ps1`, `vm-svc-cli.ps1 -Install|-Start|-StopUi`, `vm-svc-proxy.ps1`, `vm-svc-noautologon.ps1`,
`vm-svc-log.ps1` (service state + logs), `vm-svc-events.ps1` (crash events), `vm-svc-cleanup.ps1`
(uninstall + undo), `vm-qs-setup.ps1` (back to customer + TetherQS).

Technician side:

```sh
node apps/desktop/e2e/unattended-drive.mjs start 490209544 C:/vms/payload/svc-perm.json
node apps/desktop/e2e/unattended-drive.mjs click:0.406,0.977 wait:1500 shot:start.png   # Start menu; then user tile, Sign out
node apps/desktop/e2e/unattended-drive.mjs pwfile:C:/vms/payload/bench.json,enter wait:20000 shot:desktop.png
node apps/desktop/e2e/unattended-drive.mjs restart
```

Logs in the VM: service/server `C:\Windows\ServiceProfiles\LocalService\AppData\Roaming\Tether\log\server\`,
installer CLI `C:\Users\<user>\AppData\Roaming\Tether\log\install-service\`, user app and CM
`C:\Users\<user>\AppData\Roaming\Tether\log\` (`cm\`).

# Troubleshooting (support staff)

Start with the user's **Diagnostics** page, then a diagnostics bundle sent with a ticket code.
Network background: [corporate-networks.md](corporate-networks.md),
[connection-resilience.md](connection-resilience.md).

## Diagnostics page

Technician app: **Settings → Diagnostics** (policy `diagnostics.page`). TetherQS has a smaller
**Help** view (title bar `?`) with version, ID, server, reachability and latency, proxy,
admin-access state and policy serial.

| Field | Meaning |
| --- | --- |
| Version | Installed app version |
| ID | This device's Tether ID |
| System | OS name and build |
| Policy | Applied policy serial, the group (if assigned) and the layers that contributed: `defaults`, `profile`, `group:<name>`, `machine` (GPO/registry). Serial 0 means no policy was ever accepted. |
| Server | Relay host compiled into the build |
| Proxy | Proxy Tether uses, without credentials. "none, direct connection" means no proxy was found in `HTTPS_PROXY`/`ALL_PROXY`, the WinINet settings, PAC or WPAD. |
| DNS | Addresses the relay name resolves to and the lookup time, or the error |
| Connection routes | Connection attempts to the relay made by this process, one row per route |
| Recent errors and warnings | The last 40 `ERROR`/`WARN` lines from the newest log files |

**Connection routes** columns:

| Column | Meaning |
| --- | --- |
| Route | `wss://<relay>/ws/id` (port 443) or `wss://<relay>:80/ws/id` (port 80 fallback). Green: connected; red: failed, with the error below. |
| Via | **Proxy** (through the detected proxy) or **Direct** |
| Time | Time until the TLS and WebSocket handshake completed (or failed). It covers several round trips, so it is an upper bound for the relay latency; a working route is usually tens to a few hundred ms. |

**Test connection** connects every route now (443 and 80, each through the proxy and direct,
8 s timeout each), shows the result in the same table and repeats the DNS lookup. What to read
from it:

| Result | Meaning |
| --- | --- |
| Proxy routes OK, direct fail | Normal in a corporate network |
| All fail with a certificate error (`UnknownIssuer`, `unknown ca`, `invalid peer certificate`) | A TLS-inspecting proxy whose root CA is not trusted by Windows |
| All fail with a timeout or `connection refused`, DNS OK | Firewall or proxy blocks the relay host |
| DNS failed | No name resolution for the relay; nothing else can work |
| Port 80 OK, 443 fails | Port 443 to the relay is blocked; Tether keeps working over 80 |

## Verbose logging

The **Verbose logging (1 hour)** toggle (policy `diagnostics.verbose-logging`) writes trace-level
logs for one hour. It **takes effect when a Tether process next starts**, because the log level is
set at process start:

1. Turn it on.
2. Quit Tether from the tray icon and start it again. For the background service of an
   unattended host, restart it (`sc.exe stop Tether`, `sc.exe start Tether`, elevated) or reboot.
3. Reproduce the problem within the hour, then export or send the diagnostics.

It switches off by itself after the hour (at the next start), or with the toggle.

## Log locations

| App / role | Folder |
| --- | --- |
| Technician app, main window | `%APPDATA%\Tether\log\` |
| Technician app, other roles | `%APPDATA%\Tether\log\<role>\`, e.g. `server` (host side), `cm` (incoming-connection window), `connect` (session started from the command line), `service`, `portable-service` |
| Technician app, development profile `TETHER_PROFILE=x` | `%APPDATA%\Tether-x\log\…` |
| TetherQS | `%APPDATA%\Tether-QS\log\` (and `<role>` subfolders) |
| macOS technician app | `~/Library/Logs/Tether/` |

Logs rotate daily or at 16 MiB. **Open log folder** on the Diagnostics page opens the folder of
the current user. Processes running as another account (the Windows service runs as SYSTEM) write
to that account's profile and are not part of the export.

## Export diagnostics

**Export diagnostics** writes `tether-diagnostics-<unix time>.zip` to the user's Downloads folder.
`tether --diagnostics <out.zip>` writes the same archive ([cli.md](cli.md)); so does
`TetherQS --diagnostics <out.zip>` for TetherQS (its own logs and config; on Windows the exe has no
console, check the file).

| Entry | Content |
| --- | --- |
| `diagnostics.json` | Everything the Diagnostics page shows, plus the update state (`update`), the verbose-logging expiry and the log folder |
| `logs/<role>/*.log` | Log files of every role changed in the last 24 h: the newest 1 MiB of each, 12 MiB in total |
| `config/*.toml` | The config files, redacted |

Redaction in the config files: the value of every key whose name contains `password`, or a part
(split at `-`, `_`, `.`) equal to `key`, `salt`, `token`, `secret`, `pin`, `otp`, `2fa`, `tfa`,
`trusted`, `pk` or `sk`, becomes `<redacted>`. That covers the permanent password and its salt,
key pairs, the server key, PINs, 2FA secrets, the recording upload token and proxy credentials.
The proxy is shown without user name and password. Device ID, options and the proxy host stay.

**Not redacted:** the log files themselves. They can contain device IDs, peer IDs, IP addresses,
host names and file names. Tell the user before they send a bundle outside the organisation.

## Send to support

1. Support creates a ticket in Tether Manage → **Diagnostics** ([management.md](management.md#diagnostics-tickets))
   and reads the 8-character code to the user.
2. The user enters it (policy `diagnostics.upload`; input is uppercased, spaces dropped) and
   clicks **Send to support** / **Send**:
   - technician app (Windows, macOS): **Settings → Diagnostics → Send to support**;
   - TetherQS (Windows, macOS): **Help** (title bar `?`, policy `diagnostics.page`) → **Send to support**.
3. Support refreshes the ticket list and clicks **Open**, and **Close**s the ticket when done.

The bundle is the same as the export. If it is larger than 2 MiB, only `diagnostics.json` is sent
(no logs, no config); ask for an exported file in that case. Codes are valid for 24 hours and one
upload; the server keeps uploads for 7 days, closed (archived) tickets 90 days.

TetherQS bundle: `diagnostics.json` (`app: "TetherQS"`, version, ID, servers, proxy without
credentials, admin access, policy serial and group, custom app name, OS, the last 40 errors and
warnings, the log folder), `logs/` (TetherQS's own logs of the last 24 h: Windows
`%APPDATA%\Tether-QS\log`, macOS `~/Library/Logs/Tether-QS`) and `config/*.toml` redacted as above
(other files of the config folder, e.g. the hand-over file with the one-time password, are never
included). The elevated helper (admin access) logs as SYSTEM and is not part of it.

The app sends to its relays one after the other: a relay that does not know the code (`404`) or is
busy (`503`, 4 uploads at once) does not end the attempt; one that has the code but used, expired
or closed does. Without the window: `tether --diag-upload <code>` / `TetherQS --diag-upload <code>`
(same steps, prints `sent` or the message below).

| Message | Cause |
| --- | --- |
| "This code is not valid, has expired or was already used." | Typo, expired (24 h), already used, or the ticket was closed. Create a new ticket. |
| "Could not reach the support server. Try again later; …" | No relay took the code, at least one could not be reached (no connection, timeout, or a `5xx` other than `503` from the proxy in front of it) and the others did not know the code: the code may be open on the unreachable one. Try again later (up to 0.5.26 this said "not valid"). |
| "The server is busy. Try again in a minute." | Every relay is at its upload limit |
| "Cannot reach the server: …" | The device is offline; use **Export diagnostics** and another channel. |
| "Your organisation does not allow sending diagnostics." | Policy `diagnostics.upload` is off |

Up to 0.5.26 the apps sent only to the relay they had used last (after an update check: the last
relay of the list) and stopped at its answer, and Tether Manage listed the tickets of its first
relay only: with two relays an upload went to relay 2 and stayed "Waiting" in Manage, or a code
one relay did not know was "not valid". TetherQS had no upload at all. Fixed in the next release
of the apps and Tether Manage.

Verified 2026-11-03 (Hyper-V VM in corporate mode, TLS-inspecting proxy): ticket created in
Tether Manage, upload and open in about 7 s, config secrets redacted.

## Command line

```powershell
tether --diagnostics C:\Temp\diag.zip    # archive without the UI
tether --check-update                    # exit 6: update required
tether --policy-refresh                  # exit 3 offline, 4 signature rejected
```

Exit codes: [cli.md](cli.md#exit-codes).

## Common problems

| Symptom | Likely cause | Fix |
| --- | --- | --- |
| Status *Offline*, Proxy "none", all direct routes time out | The network only allows traffic through a proxy that Tether did not find (no WinINet/PAC/WPAD setting for this user, e.g. a service account) | Configure the system proxy (WinINet/PAC/WPAD) for the user, or set `HTTPS_PROXY` (e.g. `http://proxy:8080`). Tether re-detects the proxy every 60 s. Allow `https://<relay>` (TCP 443, WebSocket) on the proxy. |
| Offline, proxy routes fail with `407` | The proxy requires authentication | Exempt the relay host from proxy authentication. Tether has no setting for proxy credentials in its UI. |
| Every route fails with a certificate error | TLS inspection with a company root CA that is not in the Windows trust store | Deploy the root CA to `LocalMachine\Root` (GPO), or exempt the relay host from inspection. Tether never accepts unknown certificates. |
| Works for minutes, then drops and reconnects | Proxy or firewall closes long-lived WebSockets | Allow long-lived connections to the relay host; hosts send a heartbeat every 20 s |
| Session shows "Reconnecting… (attempt n)" repeatedly, then an error with Retry | Host lost its network, went to sleep, or restarted; after a restart it may not have re-registered yet ("could not be end-to-end encrypted") | Wait for the host to come back online, then **Retry**. The reconnect window is `connect.reconnect-minutes` (default 10). Check the host's own Diagnostics page. |
| "Update required", sessions refused, CLI exit 6 | A mandatory release or a minimum version (release or policy) is above the installed version | Install the update from the card. If it says "none is available yet", the admin raised the minimum without publishing a release in the device's channel. If it says "Your organisation installs updates for you", `update.enabled` is off: deploy the new version with software distribution. See [updates.md](updates.md). |
| Incoming sessions to a host close immediately | The host needs a required update (its connection manager closes every incoming connection), or `connect.incoming` is off | Update the host; check its policy |
| Setting is greyed out, or "Managed by your organisation" / `locked:<feature>` | The policy locks the feature (state off, locked or hidden), at profile, group or machine level | Diagnostics → Policy shows the serial, group and layers. Change it in Tether Manage and publish, or in the GPO if the source list contains `machine` (the machine layer wins). |
| Policy change not applied | The device has not checked yet (`policy.refresh-minutes`, default 15), is offline, or rejects the signature | Run `tether --policy-refresh`. Exit 4: the client was built with a different admin key and must be reinstalled. Tether Manage → Devices shows the serial each device runs. |
| No record button in the session | Policy `recording.mode` is `off` | Change the policy if recording is wanted |
| "Recording declined by the other party" | The remote user chose not to allow it | Expected. Tether never records without consent. |
| Record button stays "Waiting for the remote user to allow recording" | Nobody is at the host (unattended access) to answer, or the consent dialog is hidden behind other windows | Ask the remote user to answer. An unattended host cannot consent, so it cannot be recorded. A reconnect asks again. |
| Cannot find a recording | | Main window → camera button (**Recordings**, next to Settings) lists them; **Open folder** or Settings → Recording shows the folder in use. A file appears once the recording stops (a few seconds later). With `recording.keep-local = N` the local file is deleted after upload. |
| "Upload failed" on the Recording saved card | Share or HTTPS target unreachable, or token rejected | The file stays local and in the queue ("Waiting for upload" in the list) and is retried at the next session or app start. The error is in the log. |
| Technician sees a black or frozen screen when a UAC prompt opens on a TetherQS customer PC; elevated windows ignore input | TetherQS runs as a normal user and cannot see the secure desktop | Ask the customer to click **Allow admin access** (policy `customer.elevate`), or use **Remote actions → Request admin access**, and have the customer confirm the one UAC prompt (a No can be followed by another request). During a session it elevates in place and the session continues; outside a session TetherQS restarts with the same ID and password. The Help view shows "Admin access: On" afterwards. |
| "Allow admin access" does nothing useful | The customer has no admin rights and does not know admin credentials, or declined the UAC prompt | TetherQS keeps running without admin rights. Someone with admin credentials must confirm the prompt (over-the-shoulder credentials work). |
| TetherQS does not offer a release that was just published | It checks at start and every `update.check-hours` (default 6); a running TetherQS has not checked since | **Help → Check for updates** (or restart TetherQS). Help shows the last result; "Check failed: …" names each server and why it was not reached |
| TetherQS update fails with "signature does not match" | The exe on the server does not match `latest.json` (upload in progress, or signed with another key) | Wait for the release upload to finish; if it persists, republish the release from Tether Manage |
| Update fails with "The signature was created with a different key than the one provided" | The installed build carries another update-signing key than the one Tether Manage signs releases with. 0.5.5 and the first 0.5.6 builds were built with the key from before the 2026-11-04 key reset (key id `43DFAF08CF9D9DD2`; releases are signed with `1AD0C3FBDCA3B4EA`) | Install 0.5.6 (or later) once by hand, Tether and TetherQS alike; from then on updates install by themselves. Builds must take `TETHER_UPDATER_PUB_KEY` from the current key file (see [macos.md](macos.md)); the key is compiled in, so changing it later needs another manual install. |
| Toggles for verbose logging or Send to support are missing | Policy `diagnostics.verbose-logging` / `diagnostics.upload` is off | Use **Export diagnostics**, or change the policy |
| Clicks and keys do nothing in the Tether window (session card, TetherQS, connection window, Tether settings) on the remote computer; the technician sees "The Tether window on the remote computer can only be used by the person at that computer." | Intended since 0.5.6: remote input never reaches Tether's own windows on the host, so a technician cannot change their own permissions or approve prompts | Ask the person at that computer to click. Leave the window with **Alt+Tab** (or Win / Cmd+Tab) or click another window; the pointer can still move across it. File transfer also refuses Tether's own config and install folders. See SECURITY.md, "Tether's own windows are for the local user only". |
| Keys in a session partly work (letters act like shortcuts, a modifier seems stuck); Alt+Space or the Win key opens something on the technician's PC | Before the next version: a key released while the session window did not have the focus (local Alt+Tab, Start menu, PowerToys Run) stayed pressed on the remote computer | Fixed: held keys are released when the window loses the focus, and system keys go to the remote computer (Windows technician app). See [keyboard.md](keyboard.md). If the session says "View only: … turned off control", the person at the remote computer has to turn **Control mouse and keyboard** back on. |
| Two devices on the same network, but the session badge shows **Relay** (not **Direct (LAN)**) | The host does not listen for direct connections: older version (before 0.5.5); policy `network.direct` is `off`; Windows: the installed Tether lacks its firewall rule `Tether` (reinstall), TetherQS has no admin-created rule `Tether QS`; macOS: the application firewall is on; another program uses TCP 21118 / UDP 21119; or the two are in different subnets / client isolation (guest Wi-Fi) blocks broadcasts | Viewer log: `LAN lookup: <id> at <ip>` (found) and `used to establish LAN connection` (won). Host log: `direct listener allowed: true`, `Direct server listening on 0.0.0.0:21118`, `lan discovery listener started`. For TetherQS on Windows create `New-NetFirewallRule -DisplayName 'Tether QS' -Direction Inbound -Program <path to TetherQS.exe> -Protocol TCP -LocalPort 21118 -RemoteAddress LocalSubnet -Action Allow` plus the same with `-Protocol UDP -LocalPort 21119`. Without such a rule (TetherQS without admin rights) the host cannot receive the UDP 21119 lookup or TCP 21118 at all; the LAN path then comes from WebRTC instead: both sides send ICE checks to each other's private (host) addresses, and the host's own outbound checks open its stateful firewall for the answers, so no rule and no prompt are needed. Viewer log `WebRTC selected pair: (local) udp host … <-> (remote) udp host … (lan: true)`; behind a proxy also `WebRTC LAN-only (proxy configured)`. Needs `network.direct = auto` on both sides and both on this version (before it, a proxy on either side ruled WebRTC out). |
| Same network, badge **P2P** although the PCs are next to each other | Before this version a WebRTC pair between two private addresses was labelled P2P | Badge now says **Direct (LAN)** when the selected pair's addresses are both private (RFC 1918, link-local, ULA); a pair via a public (STUN) address stays **P2P**, a TURN pair **Relay (UDP)**. |

## Connected, but "Waiting for the first image…" (Mac)

The Mac has not allowed Tether (or TetherQS) to record its screen. This always happens after an
update of the unsigned Mac app: macOS ties the permission to the app's signature, and the switch
can still look on. On the Mac: **System Settings → Privacy & Security → Screen & System Audio
Recording**, remove the app with −, add it again, quit and reopen it, connect again. Also check
**Accessibility** (mouse and keyboard). Since 0.5.3 both apps show a warning and the technician is
told what is missing; since 0.5.4 its **Fix** button clears the old entry itself (use it when the
switch is already on but the warning stays), then switch the app on and choose Quit & Reopen. See [macos.md](macos.md#permissions).

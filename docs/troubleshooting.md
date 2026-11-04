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
`tether --diagnostics <out.zip>` writes the same archive ([cli.md](cli.md)).

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
2. The user enters it on the Diagnostics page (policy `diagnostics.upload`) and clicks
   **Send to support**. Input is uppercased; only letters and digits are kept.
3. Support refreshes the ticket list and clicks **Open**.

The bundle is the same as the export. If it is larger than 2 MiB, only `diagnostics.json` is sent
(no logs, no config); ask for an exported file in that case. Codes are valid for 24 hours and one
upload; the server keeps uploads for 7 days.

| Message | Cause |
| --- | --- |
| "This code is not valid or was already used." | Typo, expired (24 h) or already used. Create a new ticket. |
| "Cannot reach the server: …" | The device is offline; use **Export diagnostics** and another channel. |
| "Your organisation does not allow sending diagnostics." | Policy `diagnostics.upload` is off |

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
| Technician sees a black or frozen screen when a UAC prompt opens on a TetherQS customer PC; elevated windows ignore input | TetherQS runs as a normal user and cannot see the secure desktop | Ask the customer to click **Allow admin access** (policy `customer.elevate`) and confirm the one UAC prompt. During a session it elevates in place and the session continues; outside a session TetherQS restarts with the same ID and password. The Help view shows "Admin access: On" afterwards. |
| "Allow admin access" does nothing useful | The customer has no admin rights and does not know admin credentials, or declined the UAC prompt | TetherQS keeps running without admin rights. Someone with admin credentials must confirm the prompt (over-the-shoulder credentials work). |
| TetherQS does not offer a release that was just published | It checks at start and every `update.check-hours` (default 6); a running TetherQS has not checked since | **Help → Check for updates** (or restart TetherQS). Help shows the last result; "Check failed: …" names each server and why it was not reached |
| TetherQS update fails with "signature does not match" | The exe on the server does not match `latest.json` (upload in progress, or signed with another key) | Wait for the release upload to finish; if it persists, republish the release from Tether Manage |
| Toggles for verbose logging or Send to support are missing | Policy `diagnostics.verbose-logging` / `diagnostics.upload` is off | Use **Export diagnostics**, or change the policy |

## Connected, but "Waiting for the first image…" (Mac)

The Mac has not allowed Tether (or TetherQS) to record its screen. This always happens after an
update of the unsigned Mac app: macOS ties the permission to the app's signature, and the switch
can still look on. On the Mac: **System Settings → Privacy & Security → Screen & System Audio
Recording**, remove the app with −, add it again, quit and reopen it, connect again. Also check
**Accessibility** (mouse and keyboard). Since 0.5.3 both apps show a warning and the technician is
told what is missing; since 0.5.4 its **Fix** button clears the old entry itself (use it when the
switch is already on but the warning stays), then switch the app on and choose Quit & Reopen. See [macos.md](macos.md#permissions).

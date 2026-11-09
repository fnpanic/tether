# Privacy policy

Tether and TetherQS send no data to the Tether developers or to third parties. They talk only to the
relay servers chosen by the organization that gave you the app (built into the app or set by that
organization's signed policy). That organization runs those servers and is responsible for the data on them.

What goes to those relay servers:

| Data | Why | Stored |
| --- | --- | --- |
| Device ID, IP address, app version, online status | Connecting technician and device | while online |
| Screen, input, clipboard, files and audio of a session | The remote session itself; encrypted between the two devices, the relay only forwards it | not stored |
| Session audit record: time, device, technician name, IP address, duration, names of transferred files, security alarms | Accountability of remote access (shown in Tether Manage) | daily files, kept for the retention the organization sets |
| Update checks: app, channel, version | Offering signed updates | access logs of the relay |

Session recordings are made only when recording is switched on and are saved on the technician's
computer. They are uploaded only if the organization's signed policy sets an upload target
(`recording.upload-target`), and then only to that target.

If WebView2 is missing, the installer downloads it from Microsoft (go.microsoft.com), under Microsoft's
privacy statement.

Opting out: audit records and update checks are part of the organization's policy; ask the
organization that provides the app. Uninstalling the app (Settings → Apps, or the uninstaller in the
install folder) stops all transfers.

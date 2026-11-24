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
| Health report, only on machines where the organization installed a monitoring agent: machine name, names and versions of installed products, the state, title, value and hint of each check, agent version, time of the last report | Showing the health of supported machines to technicians | the last full report and 90 days of state changes; removed when the device is removed or after 180 days without a report |
| Customer reference, only on machines where the organization installed a monitoring agent and its administrator put one in the enrolment token: a short label such as a customer number (set by the administrator, never by the machine) | Grouping supported machines by customer in Tether | with the device's enrolment: removed when the device is removed or after 180 days without a report |

Health reports come from a separate monitoring agent, not from Tether or TetherQS, and only if the
organization installed it. They go to the same relay servers, are not end-to-end encrypted, and can be
read by the relay's operator and by every member of the organization's technician team in Tether.
What a report contains is decided by the agent; the format is described in [docs/health.md](docs/health.md).
When a technician links a monitoring agent to a host in Tether, the relay stores that technician's
name (as in the team roster) and the time with the device's enrolment; every team member sees it,
and it is removed with the device.

Session recordings are made only when recording is switched on and are saved on the technician's
computer. They are uploaded only if the organization's signed policy sets an upload target
(`recording.upload-target`), and then only to that target.

If WebView2 is missing, the installer downloads it from Microsoft (go.microsoft.com), under Microsoft's
privacy statement.

Opting out: audit records and update checks are part of the organization's policy; ask the
organization that provides the app. Uninstalling the app (Settings → Apps, or the uninstaller in the
install folder) stops all transfers.

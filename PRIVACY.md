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
| Staff access record (team audit log), about the organization's technicians and other support staff, not about the supported machine's users: the staff member's name, user name and directory id, the time, the device and customer concerned, the search term of a software search, the staff member's IP address, the kind of access and its result; never what was shown or returned (no report or list contents, no search hits) | Accountability of staff access to supported machines' data (shown in Tether Manage › Team activity) | daily files on each relay, 90 days or 2 GB by default (the organization sets both) |
| Software list and hardware identity, only on machines where the organization installed a monitoring agent that collects them, and only if the relay's operator switched the storage on: names, versions and publishers of the installed programs and of the operating system, and the machine's manufacturer, model, serial number, memory and processor | Showing a machine's installed software to technicians, searching the machines for an installed program (only names, versions and publishers, where the relay's operator switched the search on), and the organization's own inventory exports | one document per machine; removed when the device is removed or after 180 days without a report (the organization can also switch the collection off, which deletes it) |

Health reports come from a separate monitoring agent, not from Tether or TetherQS, and only if the
organization installed it. On Windows, Tether and TetherQS can install that agent for the
organization (from version 0.7.0), but only in these ways: the organization's signed policy tells
an installed Tether that runs as a Windows service to install it; a technician in a remote session
with control of the computer asks for it, where the organization's policy allows that; or the person
at the computer clicks **Install…** on Tether's card. Without Tether's service nothing is installed
until the person at the computer has said yes in Tether's own window, which says who asks, that the
program stays on the computer after the session and what it sends to which servers, and has
approved the Windows administrator prompt. With Tether's service nobody at the computer is asked;
during a session Tether's connection window says when the technician installed or removed it, and
Tether's Settings say when and why Tether installed it. Tether installs only the organization's
signed agent. The person at the computer can remove it at any time in Windows *Settings › Apps ›
Tether Monitor* (or with `tether-monitor uninstall`); Tether then never installs it again by
policy. A computer's administrator can refuse it for good with the machine policy
`{"health.agent":{"state":"locked","value":"manual"}}` in `HKLM\SOFTWARE\Policies\Tether` (value
`PolicyJson`, also by Group Policy). Details: [docs/health.md](docs/health.md#installing-the-agent-through-tether). They go to the same relay servers, are not end-to-end encrypted, and can be
read by the relay's operator and by the organization's support staff whose role allows it (see below).
What a report contains is decided by the agent; the format is described in [docs/health.md](docs/health.md).
The software list is stored and read in the same way ([docs/health.md](docs/health.md#the-software-list)): it is not
end-to-end encrypted, and the relay's operator and the support staff whose role includes software lists can read it.
If the relay's operator also switched the software search on, the relay reads the name, version and
publisher of each installed program from every list and keeps them in a file next to the list (never
the hardware identity; the file is removed with the list), so that technicians can search all
machines for an installed program. Only team members whom the organization's signed team document
allows to read software lists can search, and only over the machines of the customers it assigns
them. Each search is recorded in the team's audit log with the searched text, the technician, the
IP address and, for an answered search, the number of machines found (never which ones)
([docs/health.md](docs/health.md#software-search)).
An organization may also run its own alerting service that reads the relays as a team member does;
it may keep a copy of each software list and hardware identity and write them into export files on
the organization's own systems. Those copies and files, and the systems they are passed on to, are
covered by that organization's privacy statement.
When a technician links a monitoring agent to a host in Tether, the relay stores that technician's
name (as in the team roster) and the time with the device's enrolment; every team member sees it,
and it is removed with the device.

**Who at the support organization sees a machine's data, and how that is recorded.** The
organization's support staff see health reports and software lists through Tether only if they are
in its team: signed in with the organization's account and given a role in its directory (for
example Microsoft Entra ID), or holding a key the organization added. The organization's signed
team document decides what each role may read, and it can limit staff to the machines of certain
customers (by the customer reference above); without such a limit, staff with a role see every
machine. Every access to a machine's data is recorded on the relay that served it: who (name, user
name, directory id), when, which device and customer, the search term of a software search, and the
staff member's IP address, never what was shown. The records are kept for the retention the
organization sets (90 days by default) and can be read only by the organization's administrators
and the staff it gives a Tether Manage role, not by technicians. If a relay cannot write the log (disk
full, overload), it still serves the accesses; the records it missed are counted and the operator can
see that. A customer can ask the support organization which of its staff accessed its machines. Details:
[docs/api.md](docs/api.md#team-audit-log), [docs/sign-in.md](docs/sign-in.md).

Session recordings are made only when recording is switched on and are saved on the technician's
computer. They are uploaded only if the organization's signed policy sets an upload target
(`recording.upload-target`), and then only to that target.

If WebView2 is missing, the installer downloads it from Microsoft (go.microsoft.com), under Microsoft's
privacy statement.

Opting out: audit records and update checks are part of the organization's policy; ask the
organization that provides the app. Uninstalling the app (Settings → Apps, or the uninstaller in the
install folder) stops all transfers.

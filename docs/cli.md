# Command line and `tether://` links

The technician app (`tether.exe`, installed to `C:\Program Files\Tether`) accepts the commands
below. Source of truth: `apps/desktop/src-tauri/src/cli.rs` (`tether --help` prints the same
list). Arguments Tether does not know are passed to the RustDesk core unchanged (`--server`,
`--cm`, `--service`, `--install-service`, …); they are internal and not covered here.

## Commands

| Command | What it does | Output |
| --- | --- | --- |
| *(none)* | Start Tether (main window and tray) | – |
| `--connect <id> [password]` | Open a session window to `<id>` | – |
| `tether://connect/<id>` | Same as `--connect <id>`, without a password | – |
| `--get-id` | Print this device's Tether ID | the ID |
| `--version` | Print the version | e.g. `0.3.1` |
| `--check-update` | Check the update channel now | `update 0.3.1 available (installed 0.3.0)` or `0.3.1 is up to date` |
| `--policy-refresh` | Fetch the organisation policy now and apply it | `policy serial 12 (defaults, profile, group:pilot)` |
| `--diagnostics <out.zip>` | Write a diagnostics archive ([troubleshooting.md](troubleshooting.md#export-diagnostics)) | `wrote <out.zip>` |
| `--help`, `-h`, `/?` | Show the help | the help text |

`--get-id`, `--version`, `--check-update`, `--policy-refresh` and `--help` take no further
arguments. Output goes to the console the command was started from (stdout; errors to stderr).

### Options for `--connect`

| Option | Effect |
| --- | --- |
| `--password-stdin` | Read the password from the first line of standard input |
| `--password-env <VAR>` | Read the password from environment variable `VAR` |
| `<password>` (positional) or `--password <pw>` | Password on the command line (see the security note) |
| `--file-transfer` | Open file transfer instead of the remote screen |
| `--view-only` | Do not send mouse or keyboard input |
| `--relay` | Always use the relay. Accepted for compatibility; Tether always uses the relay. |
| `--fullscreen` | Open the session window in full screen |

Rules:

- `<id>` is the device ID: letters, digits and `_ - . @ :`, at most 64 characters. Spaces are
  removed, so `--connect "123 456 789"` works.
- Give the password only once (one of positional, `--password`, `--password-stdin`,
  `--password-env`). At most 128 characters.
- Without a password, the session window asks for it (or the remote side accepts by click).
- `--file-transfer <id> [password]` (connection type as the command, RustDesk style) is also
  accepted.

`--connect` checks the policy (`connect.outgoing`) and the update state first, then opens the
session window in a new process. The command does not wait for the session: once the window is
open, the result of the connection is shown there, not in the exit code.

## Exit codes

| Code | Meaning | Typical cause |
| --- | --- | --- |
| 0 | OK | |
| 1 | Error | Anything else, e.g. the policy server answered with an unexpected status, the zip could not be written |
| 2 | Bad arguments | Unknown option, missing ID, invalid ID, password given twice, `--password-env` variable not set, password too long |
| 3 | Offline | The relay could not be reached on any route (443/80, proxy/direct) |
| 4 | Policy signature rejected | The served policy is not signed by a trusted admin key (client built with other keys) |
| 5 | Not allowed by policy | e.g. `connect.outgoing` is off: `locked:connect.outgoing: …` |
| 6 | Update required | This version may not be used for sessions until it is updated (`--check-update`, `--connect`) |

`--check-update` prints the result line and still exits 6 when the installed version is blocked.

## `tether://` links

The installer registers the `tether` URL scheme. The only form is:

```
tether://connect/<id>
tether://connect/<id>/
```

Nothing else is accepted: no query string, no password, no other actions, no extra command-line
arguments. A link never carries a password, so a link in a ticket or e-mail cannot give anyone
access by itself; the technician still enters the password or the remote user accepts.

On Windows, opening a link starts `tether.exe tether://connect/<id>` and opens a session window.
On macOS the running app receives the link. A link that fails the policy check is refused and
logged (`link refused: …`).

## Calling Tether from other applications

### PowerShell

```powershell
$tether = "C:\Program Files\Tether\tether.exe"

# Read a value. Start-Process -Wait is needed: tether.exe is a GUI program, so PowerShell would
# not wait for it or set $LASTEXITCODE when it is called with &.
$p = Start-Process $tether -ArgumentList '--check-update' -NoNewWindow -Wait -PassThru
if ($p.ExitCode -eq 6) { Write-Warning "Tether must be updated before sessions work" }

# Connect with a password from a secret store, without putting it on the command line.
$env:TETHER_PW = (Get-Secret -Name 'host-123456789' -AsPlainText)
Start-Process $tether -ArgumentList '--connect','123456789','--password-env','TETHER_PW'
Remove-Item Env:TETHER_PW

# Or through stdin.
Get-Secret -Name 'host-123456789' -AsPlainText | & $tether --connect 123456789 --password-stdin

# Collect diagnostics into a known place.
Start-Process $tether -ArgumentList '--diagnostics',"$env:TEMP\tether-diag.zip" -NoNewWindow -Wait
```

The child process inherits the environment, so `--password-env` sees the variable set just
before. Clear it afterwards.

### cmd / batch

```bat
set TETHER="C:\Program Files\Tether\tether.exe"
start "" /wait %TETHER% --policy-refresh
if errorlevel 3 echo Tether is offline or the policy was rejected (code %errorlevel%)

start "" %TETHER% --connect 123456789 --view-only
```

### Ticket system or intranet link

```html
<a href="tether://connect/123456789">Connect to 123 456 789 with Tether</a>
```

A template such as `tether://connect/{{device.tether_id}}` works in most ticket systems. The
browser asks once whether to open Tether. Do not add a password to the link; it would be rejected.

## Security note: passwords on the command line

A positional password or `--password <pw>` is visible to every process on the machine that can
read command lines (Task Manager, `Get-CimInstance Win32_Process`, EDR and audit logs, shell
history). Use it only for tests.

| Method | Visible in the process list | Visible in shell history |
| --- | --- | --- |
| `--connect <id> <password>` | yes | yes |
| `--password-env <VAR>` | no (only the variable name) | depends on how `VAR` was set |
| `--password-stdin` | no | no, if piped from a secret store |

Prefer `--password-stdin` or `--password-env`, and fill them from a secret store. Better still,
connect without a password and let the remote user accept.

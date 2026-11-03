# Updates

Both apps update themselves from the relay's `download/` folder. Every update is signed with the
updater key held by Tether Manage; clients verify the signature against the public key compiled
into them (`TETHER_UPDATER_PUB_KEY`) and refuse anything else. Manifest format and server layout:
[api.md](api.md#download-layout-and-update-manifests).

## Endpoint layout

```
https://<relay>/download/<channel>/technician/windows-x86_64/latest.json
https://<relay>/download/<channel>/technician/windows-x86_64/Tether_<version>_x64-setup.exe(.sig)
https://<relay>/download/<channel>/customer/windows-x86_64/latest.json
https://<relay>/download/<channel>/customer/windows-x86_64/TetherQS.exe(.sig)
```

`<channel>` comes from policy `update.channel` (default `stable`; anything that is not 1–32
lowercase letters, digits or dashes falls back to `stable` in the technician app). A missing
`latest.json` (`404`) means "no update".

## Policy keys

| Key | Default | Effect |
| --- | --- | --- |
| `update.enabled` | `Y` | Check and install updates. `N`: the app does not install updates itself; the organisation deploys them. |
| `update.channel` | `stable` | Channel folder to read |
| `update.check-hours` | `6` | Check interval (minimum 1 hour) |
| `update.minimum-version` | – | Versions below it cannot be used for sessions |
| `update.force` | `N` | Install *required* updates automatically |
| `update.restart-seconds` | `60` | Countdown before a forced update installs and restarts |

## Update modes

Set per release in Tether Manage (manifest fields) or by policy. Clients apply the stricter of
manifest and policy.

| Mode | How it is set | User experience | Sessions on the old version |
| --- | --- | --- | --- |
| Optional | Release without flags | Card "Update available" with **Install and restart** / **Later** | Allowed |
| Mandatory | Release `mandatory` | Modal "Update required", no Later | Blocked until installed |
| Minimum version | Release `minimumVersion` or policy `update.minimum-version` | As mandatory. If no newer version is published: "Your organisation requires a newer version … none is available yet." | Blocked, even with `update.enabled = N` |
| Forced | Release `forced`, or a required update with policy `update.force = Y` | "Tether will update", countdown of `update.restart-seconds`, then install and restart | Blocked only if also required |

With `update.enabled = N` a `mandatory` release does not block (the user cannot install it), but a
minimum version does: the app shows "Your organisation installs updates for you".

### Blocking is enforced in the backend

The update state is cached in the config folder (`update-state.json`), so every process role
enforces it without network access:

- **Outgoing sessions** (UI, `--connect`, `tether://` links) fail with
  `locked:update-required: Install the required Tether update to start sessions.` The CLI exits
  with code 6.
- **Incoming sessions**: the connection manager closes every incoming connection while the update
  is required.
- TetherQS closes incoming connections in the backend and disables **Accept**.

## Technician app (installer)

1. The main window process checks 5 s after start, every `update.check-hours`, and after every
   policy change. The request uses the API route racer (443, then 80; proxy, then direct).
   `tether --check-update` runs the same check from the command line.
2. **Install and restart** (or the forced countdown) hands over to the Tauri updater: it reads the
   same `latest.json` on port 443 through the detected proxy, downloads the NSIS installer,
   verifies its minisign signature against `TETHER_UPDATER_PUB_KEY` and runs it in passive mode
   (progress window, no questions).
3. The installer (per machine, `C:\Program Files\Tether`) stops the `Tether` background service
   if it is installed, overwrites the files, starts the service again, and the updater restarts
   the app. A per-machine install needs administrator rights, so on a PC where the user is not an
   administrator Windows shows a UAC prompt.

Verified 2026-11-03: 0.3.0 → 0.3.1 auto-update on a Hyper-V VM in corporate mode, through a
TLS-inspecting proxy.

## TetherQS (portable self-update)

TetherQS has no installer. At start and every `update.check-hours` (only if `update.enabled`) it
reads `…/customer/windows-x86_64/latest.json`. If the version is newer:

1. Download the exe (HTTPS only, at most 200 MiB).
2. Verify its minisign signature (`platforms["windows-x86_64"].signature`, base64 of the `.sig`
   text) against the compiled-in `TETHER_UPDATER_PUB_KEY`. A mismatch stops the update.
3. Write it as `TetherQS.exe.new` next to the running exe, rename the running exe to
   `TetherQS.exe.old` (Windows allows renaming, not overwriting, a running exe), and rename
   `.new` to `TetherQS.exe`. If the second rename fails, the old exe is put back.
4. Restart. The new process keeps the ID and the one-time password (hand-over file, valid 60 s),
   so a technician can reconnect with the same credentials. `.old` and `.new` are deleted at the
   next start.

If the folder of the exe is not writable, the new exe goes to the fallback folder
`%LOCALAPPDATA%\Tether-QS\TetherQS.exe` and is started from there. The original file stays old: if
the customer starts it again, it updates again.

Mandatory, minimum version and forced work as in the table above. No admin rights are needed.

Verified 2026-11-03: 0.3.0 → 0.3.1 self-update as a standard user in 47 s (VM in corporate mode,
TLS-inspecting proxy).

## Signing keys

| Key | Private half | Public half |
| --- | --- | --- |
| Updater key (minisign) | Tether Manage, `keys.json`, encrypted with the passphrase | `TETHER_UPDATER_PUB_KEY` at build time, both apps |

Release builds refuse to compile without the key. A client built with another key rejects every
update; such clients must be reinstalled by hand. Keep the key backup ([management.md](management.md#first-run-keys-and-passphrase)).

Tether Manage signs exactly like `tauri signer sign`: the `.sig` and the manifest `signature` are
base64 of the minisign signature file, with the trusted comment
`timestamp:<unix>\tfile:<name>\tversion:<version>`.

## Release procedure

1. Set the version in `apps/desktop/src-tauri/tauri.conf.json` / `Cargo.toml` (technician) or
   `apps/quick/src-tauri/Cargo.toml` (TetherQS).
2. Build with the four release variables (`TETHER_SERVER`, `TETHER_PUB_KEY`,
   `TETHER_ADMIN_PUB_KEY`, `TETHER_UPDATER_PUB_KEY`), see [deployment.md](deployment.md):
   technician `npx tauri build --bundles nsis` → `Tether_<version>_x64-setup.exe`; TetherQS
   `npx tauri build` → `TetherQS.exe`.
3. Test the build on one machine (install, connect, `tether --version`).
4. Tether Manage → unlock → **Releases**:
   1. **Choose…** the file. For a technician installer the version is filled in from the name;
      for TetherQS type it.
   2. Channel: `test` first (see below), later `stable`.
   3. Notes, and Mandatory / Minimum version / Forced only if needed.
   4. **Sign and publish**. Tether Manage uploads the file, then `.sig`, then `latest.json`.
5. Check **Files on the server** for the three files.
6. On a test device: `tether --check-update` should print `update <version> available`.
7. Watch **Devices**: the Version column changes as devices update (each sends a heartbeat after
   its next policy check).
8. For the technician installer, republish the **Download page** so its link points to the new
   file.

Do not publish a version lower than the one already in a channel: clients only install newer
versions, so nothing would happen.

## Staged rollout with a `test` channel

1. **Policy → New group** `pilot` (profile `technician`, and a second one for `customer` if
   needed). Set `update.channel` to **Locked** with value `test`.
2. **Devices**: assign the pilot machines to the group. **Publish** the policy.
3. Publish the release to channel `test`. Optionally use a pre-release version such as
   `0.3.1-test`: it sorts below `0.3.1`, so pilot devices later move to the final `0.3.1` on
   `stable` by themselves.
4. When the pilots are fine, publish the release to `stable`. To move pilots back, remove the
   group assignment (or set the channel to `stable`) and publish the policy. Switching channel
   never downgrades: a device on a higher version waits until `stable` catches up.

Test-channel builds are not linked from the download page, which always uses `stable`.

# Tether Manage — administration app

Tether Manage (`apps/manage`) is the desktop app an administrator uses to run a Tether deployment.
It holds the two signing keys and talks to the relay API (`tether-api`, see [api.md](api.md)) with
signed requests. Nothing in it is needed by technicians or customers.

| Screen | What it does |
| --- | --- |
| Status | Relay API version, published policy serial, device count, free disk, uptime |
| Policy | Edit, preview and publish the signed organisation policy; history and restore |
| Devices | Every app that sent a heartbeat in the last 180 days; group assignment |
| Releases | Sign and publish technician installers and `TetherQS.exe` to an update channel |
| Download page | Generate and publish the page at `https://<relay>/` |
| Diagnostics | Open support tickets and read the diagnostics bundles users send |
| Keys and server | Public keys to deploy, relay address, key backup |

## Data folder

Keys, settings, the policy draft and local copies of published policies live in
`%APPDATA%\Tether-Manage` (shown on the Keys screen):

| File | Content |
| --- | --- |
| `keys.json` | Admin key and updater key, encrypted with the passphrase |
| `settings.json` | Relay address (default `https://tether-desk.duckdns.org`), download page texts |
| `policy/draft.json` | The policy being edited |
| `policy/published/<serial>.json` | Every policy this PC published |

Set `TETHER_MANAGE_DIR` to use a different folder: a separate vault for tests or for several
deployments on one PC.

```powershell
$env:TETHER_MANAGE_DIR = "D:\vaults\customer-b"
& "<path to Tether Manage.exe>"
```

## First run: keys and passphrase

On first start Tether Manage offers **Create keys** or **Import a backup instead**.

**Create keys** generates:

| Key | Type | Signs |
| --- | --- | --- |
| Admin key | Ed25519 | Policy documents and every admin API request |
| Updater key | minisign (Tauri updater format) | Technician installers and `TetherQS.exe` |

Both are sealed in `keys.json` with your passphrase (argon2id, 64 MiB, 3 passes, then
XChaCha20-Poly1305). The passphrase must be at least 8 characters. There is no recovery.

After creating the keys, immediately:

1. **Keys and server → Export backup…** writes `tether-manage-keys.tmkeys`. It is the sealed key
   file itself, still encrypted with the current passphrase.
2. Store the backup and the passphrase in two separate safe places (for example the backup on
   offline media, the passphrase in the company password vault).

> **Losing the passphrase and the backup means rebuilding every client.** The admin and updater
> public keys are compiled into the technician app and TetherQS. Without the private keys you can
> no longer publish policies or updates that installed clients accept. New keys only work in newly
> built and newly deployed clients.

On later starts, **Unlock** asks for the passphrase. The keys stay in memory until the app closes.

**Import a backup** (first run, or **Keys and server → Import backup…**) needs the backup file and
the passphrase it was exported with. It replaces the keys on this PC. Use it to work from a second
PC or to restore after a reinstall. Importing a backup of *different* keys locks you out of
existing clients until their public keys are deployed.

## Keys and server

The screen shows the public halves only. Private keys never leave the PC.

| Public key | Where it goes |
| --- | --- |
| Admin public key | Build environment `TETHER_ADMIN_PUB_KEY` for both apps, and on the relay `ADMIN_PUB_KEY=<key>` in `/etc/tether/api.env` (then `sudo systemctl restart tether-api`). Both accept several keys, comma-separated, for a rotation. Without it the server answers the admin endpoints with `503`. |
| Update signing public key | Build environment `TETHER_UPDATER_PUB_KEY` for the technician app and for TetherQS. |

Release builds refuse to compile without these values; see the build commands in
[deployment.md](deployment.md).

**Relay address** must start with `https://`. The Status screen shows "Connected, admin key
accepted" when the server trusts this PC's admin key.

## Policy

The policy is one signed document for all devices (`tether_trust::policy::Bundle`). Every
feature is listed in [policy.md](policy.md).

### Layers

Later wins:

1. built-in defaults
2. **profile**: `technician` (the technician app) or `customer` (TetherQS)
3. **group** overrides, for devices assigned to the group
4. machine policy on the device (`HKLM\SOFTWARE\Policies\Tether` `PolicyJson`, set by GPO/MDM)

### Feature states

| State | Effect on the device |
| --- | --- |
| Default | Not set in this layer; the layer below decides |
| On | Enabled, the user may change it; the value is the default |
| Off | Disabled and locked |
| Locked | Locked to the value (bool features: on). Text, number and choice features need a value. |
| Hidden | Disabled, locked and not shown |

A later layer set to **On** unlocks a feature an earlier layer locked.

Values are checked when editing: bool `Y`/`N`, choices from the list, numbers as whole numbers,
text up to 1024 characters. Unknown features or values make the whole draft "Not publishable"; a
typo never silently does nothing. The EN/DE switch only changes the labels.

### Groups and device assignment

1. **Policy → New group**: a name (1–64 characters) and the profile it targets.
2. Set the group's overrides.
3. **Devices**: pick the group in the device's *Group* column. Only groups of the device's profile
   are offered. Devices are identified by their install uuid (shown under the ID), so an
   assignment survives ID changes but not a fresh install.
4. Publish. Devices pick up assignments only from a published policy.

Deleting a group also removes its assignments.

### Preview

The right column shows the effective policy a device gets from the *draft*: pick "Any device
without a group" or a specific device. Locked features have a lock icon, hidden ones are struck
through. The machine policy layer is not part of the preview.

### Publish

**Publish** validates the draft, signs it with the admin key and uploads it
(`PUT /api/admin/policy`). Each publish gets a new serial: the higher of the server's current
serial and the highest serial published from this PC, plus one. Devices reject a serial lower than
the last one they accepted.

Devices fetch the policy every `policy.refresh-minutes` (default 15, or every minute with
`policy.fast-apply`). On a device, `tether --policy-refresh` fetches it immediately
([cli.md](cli.md)). The Devices screen shows the serial each device runs; an older one is
highlighted.

### History and restore

**History** lists every version the server keeps (the last 50) merged with the copies on this PC
("on server" / "local only"). **Restore** publishes the content of an old version again as a new
serial, replacing unpublished edits.

Versions published from another PC can be restored too: Tether Manage downloads the signed
document from the server (`GET /api/admin/policy-history/<serial>`) and only uses it if its
signature verifies with this PC's admin key. Both PCs must therefore use the same keys (import
the backup).

On a new PC with no draft, the editor starts from the policy the server serves, again only if this
admin key signed it.

## Devices

The list is built from heartbeats (`POST /api/heartbeat`). The technician app sends one after each
policy check from its main window process; TetherQS sends one after each policy check. Builds
without an admin key send none.

| Column | Meaning |
| --- | --- |
| ID / uuid | Tether ID and the install uuid (one per config folder) |
| Profile | `technician` or `customer` |
| Version | App version |
| OS | Operating system and build |
| Policy | Policy serial the device applied; highlighted if older than the published one |
| Last seen | Time of the last heartbeat |
| IP | Address the server saw (behind a proxy: the proxy's public address) |
| Group | Assignment in the draft |

The server keeps at most 10 000 devices and drops entries older than 180 days.

## Releases

**Sign and publish** signs a file with the updater key and uploads it to a channel.

| Field | Meaning |
| --- | --- |
| File | Windows: `Tether_<version>_x64-setup.exe` (technician, NSIS) or `TetherQS-<version>.exe` (customer). macOS: `Tether_<version>_universal.app.tar.gz` (technician; the `Tether_<version>_universal.dmg` from the same folder is uploaded with it) or `TetherQS-<version>-universal.zip` (customer). App, platform and version are filled in from the file name. |
| App | Technician app or customer module |
| Platform | Windows (`windows-x86_64`) or macOS (`darwin-universal`, Apple silicon and Intel) |
| Version | Must equal the version built into the file (`0.3.1`, `0.3.1-test`). A wrong version makes clients offer the update again after installing it. |
| Channel | 1–32 lowercase letters, digits or dashes. Devices use the channel from policy `update.channel` (default `stable`). |
| Release notes | Shown in the update card |
| Mandatory | Sessions are blocked on older versions until this one is installed |
| Minimum version | Versions below it are blocked, whether or not they can update |
| Forced | Installed without asking after `update.restart-seconds` |

Upload order, so that clients never see a manifest pointing to a missing file:

1. the artifact, to `download/<channel>/<app>/<platform>/<file>`
2. `<file>.sig` (macOS technician: then the `.dmg` and its `.sig`)
3. `latest.json`, last. The current `latest.json` is read first; entries of other platforms for the
   same version are kept.

macOS builds: [macos.md](macos.md).

**Files on the server** lists everything under `download/`, with delete. The server refuses uploads
that would leave less than 2 GiB free.

How clients handle these flags: [updates.md](updates.md).

## Download page

Generates a self-contained page (no external resources, light and dark) and publishes it as
`https://<relay>/`. Settings: title, introduction (empty line = new paragraph), and which apps to
offer. The page has a Windows and a macOS section, each with the offered apps (technician
installer / disk image, TetherQS). A small inline script detects the visitor's OS
(`navigator.userAgentData.platform`, else the user agent), shows that section first and marks it
"Your computer"; the **Windows / macOS** switch shows the other. Without JavaScript both sections
are shown and the switch jumps to them. An app without a stable release for an OS shows "Not
available yet". The links point to the current **stable** release; **Publish** fails only if no
offered app has a stable release on any OS. Republish the page after every release (file names
contain the version) and after changing the texts. The preview in Tether Manage shows the page
without its script (both sections).

## Diagnostics tickets

1. **Create ticket** with a note (who and what, up to 256 characters).
2. Read the 8-character code to the user. Codes avoid ambiguous characters (no I, O, 0, 1). It is
   valid for 24 hours and for one upload.
3. The user enters it under **Settings → Diagnostics → Send to support**.
4. The ticket shows the upload size and time. **Open** lists the files and shows
   `diagnostics.json`; **Save zip…** stores the bundle.

Uploads are kept on the server for 7 days. What the bundle contains: [troubleshooting.md](troubleshooting.md#export-diagnostics).

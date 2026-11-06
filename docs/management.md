# Tether Manage — administration app

Tether Manage (`apps/manage`) is the desktop app an administrator uses to run a Tether deployment.
It holds the two signing keys and talks to the relay API (`tether-api`, see [api.md](api.md)) with
signed requests. Nothing in it is needed by technicians or customers.

| Screen | What it does |
| --- | --- |
| Status and relays | The relay list (every publish goes to all of them) with per-relay health; live policy vs draft |
| Policy | Edit the draft, compare it with the live policy on every relay, history |
| Devices | Every app that sent a heartbeat to any relay in the last 180 days; filters; group assignment |
| Releases | What each relay publishes vs the latest upload; sign and publish installers and `TetherQS.exe` |
| Download page | Generate and publish the page at `https://<relay>/` |
| Custom apps | Branded TetherQS downloads (name, logo, colour, settings) behind private, expiring links |
| Diagnostics | Open support tickets and read the diagnostics bundles users send |
| Keys | Public keys to deploy, server key rotation, key backup |

A bar at the top of every screen shows the policy state, for example
**Draft has 3 changes · Live: serial 4 on 1/2 relays**, with one **Publish…** button (see
[Publish](#publish)), **Bring 1 relay up to date** when a relay serves an older policy, and a warning
badge when the policy names a relay that is not in the relay list.

### Publishing pattern

Everything that goes to the relays (policy, releases, the download page, custom apps) is published
the same way:

1. **Review.** A sheet lists what will change and every configured relay, each checked with
   `GET /api/admin/status`. If a relay does not answer, Publish stays disabled until you tick
   "Publish to the others anyway".
2. **Publish** to every relay.
3. **Result per relay:** ✓ with what is live now, or the error. A relay that failed can be brought
   up to date later (policy: the bar's **Bring … up to date**; releases: **Publish to missing
   relays**).

## Data folder

Keys, settings, the policy draft and local copies of published policies live in
`%APPDATA%\Tether-Manage` (shown on the Keys screen):

| File | Content |
| --- | --- |
| `keys.json` | Admin key, updater key and server keys generated for a rotation, encrypted with the passphrase |
| `server-keys/rotation.json`, `server-keys/published/<serial>.json` | Progress of a server key rotation; every key list this PC published |
| `settings.json` | Relay list (default `https://tether-desk.duckdns.org`), download page texts |
| `policy/draft.json` | The policy being edited |
| `policy/published/<serial>.json` | Every policy this PC published |
| `custom/<id>.json` | Custom apps created on this PC: profile (logo, colour, settings), token, expiry |

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

`keys.json` has a format version (`v`). A vault that also holds server keys is written as format 2
(Tether Manage 0.5.5 and later), which older versions refuse to open instead of opening it and
dropping the server keys when they save it again. A vault without server keys stays format 1.
Fields a newer version adds are kept unchanged when this version saves the vault; a vault of a
newer format than this version knows can be unlocked and used, but this version refuses to save
it ("written by a newer version of Tether Manage … update Tether Manage first").

After creating the keys, immediately:

1. **Keys → Export backup…** writes `tether-manage-keys.tmkeys`. It is the sealed key
   file itself, still encrypted with the current passphrase.
2. Store the backup and the passphrase in two separate safe places (for example the backup on
   offline media, the passphrase in the company password vault).

> **Losing the passphrase and the backup means rebuilding every client.** The admin and updater
> public keys are compiled into the technician app and TetherQS. Without the private keys you can
> no longer publish policies or updates that installed clients accept. New keys only work in newly
> built and newly deployed clients.

On later starts, **Unlock** asks for the passphrase. The keys stay in memory until the app closes.

**Import a backup** (first run, or **Keys → Import backup…**) needs the backup file and
the passphrase it was exported with. It replaces the keys on this PC. Use it to work from a second
PC or to restore after a reinstall. Importing a backup of *different* keys locks you out of
existing clients until their public keys are deployed.

## Keys

The screen shows the public halves only. Private keys never leave the PC.

| Public key | Where it goes |
| --- | --- |
| Admin public key | Build environment `TETHER_ADMIN_PUB_KEY` for both apps, and on the relay `ADMIN_PUB_KEY=<key>` in `/etc/tether/api.env` (then `sudo systemctl restart tether-api`). Both accept several keys, comma-separated, for a rotation. Without it the server answers the admin endpoints with `503`. |
| Update signing public key | Build environment `TETHER_UPDATER_PUB_KEY` for the technician app and for TetherQS. |

Release builds refuse to compile without these values; see the build commands in
[deployment.md](deployment.md).

### Server key

hbbs and hbbr sign device identities with the server key (`/var/lib/tether/id_ed25519` on every
relay). Installed apps trust the key they were built with (`TETHER_PUB_KEY`) until a **server key
list** signed with the admin key replaces it, so the key can be replaced without reinstalling
anything. Design, threat model and failure cases: [key-rotation.md](key-rotation.md).

**Key list on the relays** shows per relay the serial and keys of the list it serves
(`GET /api/server-keys`, verified with this admin key), which key devices present (the first),
an expiry if set, and **Behind** for a relay with an older list. "No list published" means devices
trust only their built-in key.

**Rotate the server key** is a checklist; each step opens when the previous one is done:

1. **Generate the new key.** Enter the current server public key (prefilled from a published list)
   and the passphrase. The new key pair is sealed into `keys.json` at once. Export the backup again.
2. **Announce it to the devices.** **Publish key list…** (review sheet, every relay) publishes
   *current + new*: devices keep presenting the current key and start to trust the new one.
   Optionally set a date from which devices stop trusting the old key on their own.
3. **Switch the relays.** A counter shows how many devices active in the last 24 h report the list
   (heartbeat `server_keys_serial`, refreshed every 30 s; versions before key rotation never do).
   When enough do: **Save key files…** writes `id_ed25519` and `id_ed25519.pub` of the new key into
   a folder; the commands below install them on every relay with `server/rotate-key.sh switch`
   (Helm: the `helm upgrade` line). `id_ed25519` is the server's **private** key: it is created
   so that only the current Windows user can open it (a protected ACL with that user's SID only,
   nothing inherited; 0600 on macOS/Linux), and a warning with **Delete key files** stays in this
   step and the next one until you delete it (the button removes only `id_ed25519` and
   `id_ed25519.pub` from that folder). **Every relay is switched** opens only when every relay's
   hbbs reports that it signs with the new key (`https://<relay>/rendezvous/pubkey`, see
   [key-rotation.md](key-rotation.md#checking-the-switch)); the list shows which relay still signs
   with the old key or cannot tell (hbbs or Caddyfile without the route: update them first), and
   **Check again** reloads it. Build new installers with the new `TETHER_PUB_KEY` from now on.
4. **Retire the old key on the devices.** **Publish list without the old key…**.
5. **Stop accepting the old key on the relays**, when the counter is high enough again:
   `rotate-key.sh retire` per relay (Helm: `key.accept=""`), then **Every relay is done**.

Devices that miss a step heal on their next policy check, because the list comes over HTTPS from
the relay API. Until then they get "Server key mismatch …" when connecting.

## Status and relays

**Relays** is the most important setting: Tether Manage publishes to exactly these relays
([relay-ha.md](relay-ha.md)). Add a relay by host name (`https://` is added), remove one, then
**Save relay list**. The first relay is used for reads and for the links of custom apps. Every
address must start with `https://`.

Each relay row shows its health from `GET /api/admin/status` ("Admin key accepted" or the error),
relay API version, the policy serial it serves (**Live** or **Behind**), device count, free disk
(highlighted below 2 GiB, where uploads are refused) and uptime.

If the policy's `servers.list` names a relay that is not in the list, a warning says so (also in
the top bar and the publish sheet), with **Add** for that host: devices may use that relay, but no
publish reaches it, so its devices keep old policies and releases.

## Policy

The policy is one signed document for all devices (`tether_trust::policy::Bundle`). Every
feature is listed in [policy.md](policy.md).

### Layers

Later wins:

1. built-in defaults
2. **profile**: `technician` (the technician app) or `customer` (TetherQS)
3. **custom app** settings, for TetherQS downloaded through a [custom app](#custom-apps) link
4. **group** overrides, for devices assigned to the group
5. machine policy on the device (`HKLM\SOFTWARE\Policies\Tether` `PolicyJson`, set by GPO/MDM)

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

### Editor

**Policy → Draft editor** edits one layer at a time (technician profile, customer profile, or a
group). Every feature has a state control; the selected state is a filled segment (**Default**
quietly outlined, any other state filled in the accent colour). A feature set in this layer has an
accent bar on the left and the badge **Changed from default**; one whose rule differs from the
live policy also has **Not published**. The **All / Changed only / Not published** switch filters
the list. The EN/DE switch only changes the labels.

The right column, **Effective draft**, shows what a device gets from the draft: pick "Any device
without a group" or a specific device. Locked features have a lock icon, hidden ones are struck
through. The machine policy layer is not part of the preview.

### Live policy

**Policy → Live policy** is read-only and shows what devices get now:

- every relay's public `GET /api/policy`, verified with this app's admin key: serial, issue time,
  **Verified** or why not (for example "Signed by a different admin key": after new keys, devices
  built with the new key reject the old document), **Live** / **Behind**;
- **Draft compared with live**: the changes Publish would make, in plain words
  (`Technician profile · Allow clipboard sync  off → on`);
- **Effective live values** per profile for a device without a group (`policy::resolve`), with the
  source of each value (**Default** or **Profile**), lock/hidden icons, and the draft's value where
  it differs.

### Publish

**Publish…** (top bar) opens the review sheet: the list of changes, the relays with a reachability
check, and a warning if the policy names a relay that is not in the relay list. **Publish** then:

1. validates the draft and signs it with the admin key;
2. gives it a new serial: the highest serial of every policy a relay serves that verifies with this
   app's admin key and every serial published from this PC, plus one (devices reject a serial lower
   than the last one they accepted). What a relay merely reports (`/api/admin/status`
   `policy_serial`) or serves unsigned or signed by another key does not count, so a malicious
   relay cannot push the serial to its maximum and end publishing for good. A serial that would
   overflow is refused with an error;
3. uploads it to every relay (`PUT /api/admin/policy`) and reads each relay's `GET /api/policy`
   back: a relay shows ✓ only when it serves the new serial.

After publishing, the sheet counts how many devices active in the last 24 h already report the new
serial, refreshed every 30 s for 10 minutes. Devices fetch the policy every
`policy.refresh-minutes` (default 15), or every minute with `policy.fast-apply`: for a rollout you
want to watch, turn Fast apply on, publish, and turn it off again later. On a device,
`tether --policy-refresh` fetches it immediately ([cli.md](cli.md)).

If a relay was down or missed a publish, the top bar shows **Bring N relay(s) up to date**: it
copies the newest signed document any relay serves (verified with this key) to the relays that
are behind, unchanged.

### History

**Policy → History** lists every version the server keeps (the last 50) merged with the copies on
this PC ("on the relay" / "only on this PC"); the live one is marked. **Load into draft** replaces
the draft with an old version; nothing is published until you press **Publish**.

Versions published from another PC can be loaded too: Tether Manage downloads the signed document
from the server (`GET /api/admin/policy-history/<serial>`) and only uses it if its signature
verifies with this PC's admin key. Both PCs must therefore use the same keys (import the backup).

On a new PC with no draft, the editor starts from the policy the server serves, again only if this
admin key signed it.

## Devices

The list is built from heartbeats (`POST /api/heartbeat`) and read from **every** relay, merged by
install uuid (newest `last_seen` wins), because a device heartbeats to whichever relay it uses. A
relay that does not answer is named above the list. The technician app sends a heartbeat after each
policy check from its main window process; TetherQS after each policy check. Builds without an
admin key send none.

Filters (combined): search (ID, uuid, IP, version, OS, group), **All / Active today** (last
heartbeat since 00:00 local time on this PC), OS family (from the OS string: Windows, macOS, …),
app version, profile, group (or "No group"). The line above the table counts shown devices, active
today, and devices not yet on the live policy.

| Column | Meaning |
| --- | --- |
| ID / uuid | Tether ID and the install uuid (one per config folder) |
| Profile | `technician` or `customer` |
| Version | App version |
| OS | Operating system and build |
| Policy | Policy serial the device applied: **Current** = the live serial, **Stale** = older |
| Last seen | Time of the last heartbeat |
| IP | Address the server saw (behind a proxy: the proxy's public address) |
| Group | Assignment in the draft (publish it from the top bar) |

The server keeps at most 10 000 devices and drops entries older than 180 days.

## Sessions

The session audit log ([api.md](api.md), "session audit log"): what the hosts reported about
incoming connections, read from **every** relay (`GET /api/admin/audit`, newest 5000 per relay
and range) and merged. A host posts to the first relay of its build only. Records are
deduplicated by their nonce (else host + connection + action + time), so a record two relays have
is listed once. A relay that does not answer, or runs a tether-api without the audit log (404), is
named above the list.

Filters: **24 h / 7 days / 30 days / 90 days** (read from the relays again) and a search over host
ID, technician ID and name, IPs and path (spaces in IDs ignored). **Export CSV** saves the rows
shown (UTC times; cells a spreadsheet would run as a formula are prefixed with `'`).

| Column | Meaning |
| --- | --- |
| Time | When the relay received the record (server clock); hover: the relay that logged it |
| Host | Tether ID of the computer that was connected to, and its OS if it heartbeats |
| Technician | Name and Tether ID the viewer presented (filled in on every row of the connection from its login record) |
| IP | Technician address as the host saw it (through a relay: the relay's); hover: the address the host posted from |
| Action | Connected (connection opened), Session started (logged in; a badge names the kind unless remote control), Files sent to / received from the technician with folder or file and count (hover: up to 10 names), Disconnected, Alarm (e.g. blocked by the IP whitelist) |
| Duration | On Disconnected: time since the connection opened |

The records are posted without a device key: anyone who can reach a relay can add rows. Read
them as a log of what hosts reported, not as proof (SECURITY.md). Retention on the relays: 90 days
or 50 MB (`AUDIT_RETENTION_DAYS`, `AUDIT_MAX_MB` in `/etc/tether/api.env`).

## Releases

**Publish the latest builds** is the usual way to release: **Choose…** the build folder once
(e.g. `dist`, searched two levels deep, so `dist/windows`, `dist/quick` and `dist/macos` are
found; the folder is remembered). For each app and platform it shows the newest build in the
folder and the highest version the relays publish on the channel. **Publish N new builds…**
signs every newer build, uploads it to every relay (not mandatory, not forced, no minimum
version) and, on `stable`, then rebuilds the download page with its saved title and text. A
macOS technician build without its `.dmg` next to it is skipped and marked. Use the form below
for mandatory, forced or minimum-version releases.

While it publishes, the sheet shows the build being signed or uploaded, the relay (**Relay 1 of
2 · host**) and the file, with a progress bar (MB sent and percent), and below it every build ×
relay that has finished, failed ones with their error. The same progress shows for **Sign and
publish** and **Publish to missing relays**. The result lists per relay what it now publishes,
or every error there.

**What devices get** lists every channel × app × platform with:

- **Latest**: the highest version uploaded to any relay (from `GET /api/admin/releases`);
- per relay, what its public `latest.json` publishes: **Published · latest**, **Older · 0.3.1**
  (devices on that relay get the older version), or **Missing on <relay>**.

**Publish to missing relays** copies the release from a relay that publishes it (artifact, `.dmg`,
signatures; nothing is signed again) to the others, and writes each one its own `latest.json`
pointing at itself. If no relay publishes the latest version yet, publish it with the form.

**Sign and publish a new release** signs a file with the updater key and, after the review sheet,
uploads it to every relay. One failing relay does not stop the others; the result shows each one.

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
   same version are kept. Each relay's manifest points at that relay's own files.

macOS builds: [macos.md](macos.md).

**Releases on the first relay** lists what is under `download/`, one line per release (channel,
app, platform, version, number of files, size). Open a line for its files: installer or archive,
`.sig`, and for the macOS technician app the `.dmg` with its `.sig`. Each folder's `latest.json`
has its own line. **Delete release** removes all of a release's files on every relay after a
warning (devices that still get that version from `latest.json` can no longer download it); a
single file can be deleted from the opened line. The server refuses uploads that would leave
less than 2 GiB free.

How clients handle these flags: [updates.md](updates.md).

## Download page

Generates a self-contained page (no external resources, light and dark) and publishes it as
`https://<relay>/`. Settings: title, introduction (empty line = new paragraph), and which apps to
offer. The page has a Windows and a macOS section, each with one card per offered app: one
**Download** button for the file a person needs (Windows installer, macOS disk image, TetherQS),
all buttons at the same height. Signatures and updater archives are not on the page (they are for
the updater; see Releases).
A small inline script detects the visitor's OS
(`navigator.userAgentData.platform`, else the user agent), shows that section first and marks it
"Your computer"; the **Windows / macOS** switch shows the other. Without JavaScript both sections
are shown and the switch jumps to them. An app without a stable release for an OS shows "Not
available yet". The links point to the current **stable** release; **Publish** fails only if no
offered app has a stable release on any OS. Republish the page after every release (file names
contain the version) and after changing the texts. **Publish…** opens the review sheet and
uploads the page to every relay; its links are relative, so each relay serves its own files
(publish releases to every relay first). The preview in Tether Manage shows the page without its
script (both sections); its links and the switch do nothing there, and **Preview** always redraws it.

## Custom apps

A custom app is a TetherQS download for one customer: their name, logo, accent colour, welcome text
and customer-module settings, signed with the admin key and appended to `TetherQS.exe` by the relay
when the private link is used ([api.md](api.md#custom-apps-branded-tetherqs-downloads)). Windows
only for now.

**New custom app**:

| Field | Meaning |
| --- | --- |
| Name | Window title / company name, and the download's file name (`<Name>-Support.exe`). 1–64 characters. |
| Logo | PNG, JPEG, SVG or WebP; scaled in the app to at most 256 × 256 px and 96 KiB (PNG). Empty: Tether logo. |
| Accent colour | `#rrggbb`. Empty: the policy's `customer.accent-color`. |
| Welcome text | Up to 400 characters. Empty: the policy's `customer.welcome`. |
| Update channel | Which TetherQS build the link serves (default `stable`). |
| Link valid for | 1–366 days, default 30. |
| Settings | Customer-module features with the same editor as the policy. Order on the device: defaults < customer profile < custom app < group < machine policy. |

**Review… → Create** gives the app a random id, a new 256-bit link token (`custom::new_token`),
signs the profile and stores it on every relay with the same token, so the link
`https://<first relay>/q/<token>` works on any relay. A copy (with logo and settings) is kept in
`custom/<id>.json` on this PC.

**The link is private:** anyone who has it can download the app until it expires. It is not listed
anywhere and cannot be guessed; unknown, expired and revoked links get the same "not found". Send
it only to the customer it is for.

The list shows logo, name, link (copy), expiry, downloads and status. **Edit** changes the app and
keeps the link (tick "Issue a new link" to replace it), **+30 days** extends the link, **New link**
replaces the token (the old link stops working at once), **Revoke** deletes it on every relay.
Downloaded apps keep working after a revoke. Apps created on another PC can be revoked but not
edited (their settings are only on that PC).

## Diagnostics tickets

1. **Create ticket** with a note (who and what, up to 256 characters).
2. Read the 8-character code to the user. Codes avoid ambiguous characters (no I, O, 0, 1). It is
   valid for 24 hours and for one upload.
3. The user enters it under **Settings → Diagnostics → Send to support**.
4. The ticket shows the upload size and time. **Open** lists the files and shows
   `diagnostics.json`; **Save zip…** stores the bundle.

Uploads are kept on the server for 7 days. What the bundle contains: [troubleshooting.md](troubleshooting.md#export-diagnostics).

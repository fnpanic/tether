# Tether Manage — administration app

Tether Manage (`apps/manage`) is the desktop app an administrator uses to run a Tether deployment.
It holds the two signing keys and talks to the relay API (`tether-api`, see [api.md](api.md)) with
signed requests. Each signature names the relay it is for (`Tether-Sig: v2 host=…`, see
[api.md](api.md)), and Tether Manage follows no HTTP redirect: a relay that answers with one gets
the error "The server redirects to another address, which this app does not follow. Enter the
server's own https:// address." Nothing in it is needed by technicians or customers.

**Update order (request signatures).** A relay accepts a v2 signature only for its own name, and
no app falls back to v1 by itself. So:

1. First every relay: the new tether-api with `RELAY_HOST` set to the host name(s) entered for it
   under Status and relays.
2. Then Tether Manage and the apps.
3. Last `TETHER_SIG_V1=off` on every relay.

A new Tether Manage in front of an old relay says "This relay runs an older tether-api that does
not know this app's request signatures. Update the relay first."; with a missing or different
`RELAY_HOST` it says "The relay does not know its own host name. Set RELAY_HOST on the relay to the
name entered under Status and relays." or "The relay knows itself under another host name. Enter
the relay's own name (its RELAY_HOST) under Status and relays." (`api.rs::explain`).

| Screen | What it does |
| --- | --- |
| Status and relays | The relay list (every publish goes to all of them) with per-relay health; live policy vs draft |
| Policy | Edit the draft, compare it with the live policy on every relay, history |
| Devices | Every app that sent a heartbeat to any relay in the last 180 days; filters; group assignment |
| Team | Technicians who may read the device list and shared tags with their own key |
| Releases | What each relay publishes vs the latest upload; sign and publish installers and `TetherQS.exe` |
| Download page | Generate and publish the page at `https://<relay>/` |
| Custom apps | Branded TetherQS downloads (name, logo, colour, settings) behind private, expiring links |
| Diagnostics | Open support tickets and read the diagnostics bundles users send |
| Keys | Public keys to deploy, server key rotation, key backup |

A bar at the top of every screen shows the policy state, for example
**Draft has 3 changes · Live: serial 4 on 1/2 relays**, with one **Publish…** button (see
[Publish](#publish)), **Bring 1 relay up to date** when a relay serves an older policy, and a warning
badge when the policy names a relay that is not in the relay list. The live part is read from
every relay at unlock (10 s limit each). When no relay could be read it says **Live: could not
check (host: reason; …)**, never "nothing published yet"; while the last check failed or a relay
did not answer it, the check is asked again every 30 s until every relay answers (the refresh
button next to it asks at once). The Live policy tab and Status and relays say the same.

### Publishing pattern

Everything that goes to the relays (policy, releases, the download page, custom apps) is published
the same way:

1. **Review.** A sheet lists what will change and every configured relay, each checked with
   `GET /api/admin/status`. If a relay does not answer, Publish stays disabled until you tick
   "Publish to the others anyway".
2. **Publish** to every relay. With "Publish to the others anyway" ticked, the relays that did
   not answer the check are left out (nothing is sent to them, not even a read) and their result
   says **skipped (not reachable)**; this holds for policy, server keys, releases (both sheets and
   Publish to missing relays), the download page, custom apps and the team roster.
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
| `releases/<channel>.serial`, `releases/<channel>.live` | Per channel: the last release index serial this PC signed, and the last one a relay accepted from this PC (see "A relay that may hold a newer index" under [Keys](#keys)) |
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
The vault, like the other files Tether Manage keeps in the data folder, is written to a temporary
file, synced to disk and then renamed, so a crash or power loss leaves the old file or the new
one, never an empty one.
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

Build Tether Manage with the same `TETHER_ADMIN_PUB_KEY` as the apps. Each release publish (and
Halt, Resume, ramp) signs a new release index based on the newest index on the relays; Manage builds
on an index signed by its own key or by any key in that list. After an **admin key rotation** the
relays still serve the index signed with the old key: with the old key listed, the first publish
with the new key keeps every release in it and takes a serial above it. If a relay serves an index
that no listed key verifies, Manage refuses to publish ("… no trusted admin key verifies …")
instead of starting a new index without those releases.

**Releases on several relays.** The files go to every relay first. The index points devices at the
relay it came from, so it must never name files that relay lacks: before a new index (publish,
Halt, Resume, ramp, re-sign) goes to a relay, Manage checks (`HEAD`) that the relay serves every
artifact and gzip copy the index lists. A relay whose upload failed, or that misses a file of an
earlier release, keeps its previous index and is reported as failed with the missing files
("… use Publish to missing relays"). A relay that does not answer the read or the check is
reported as **not reachable** instead ("not reachable (…); its release index was not updated. Do
this again once it answers."): it may well have the files. **Publish to missing relays** copies the release and the newest
index to it, together with every file of other releases that index names and the relay lacks;
later publishes reach it again.

**A relay that may hold a newer index.** Manage remembers per channel the serial of the last
release index a relay accepted from this PC (`releases/<channel>.live`). The check starts with the
first successful publish after updating Tether Manage: until that file exists nothing is refused.
When the newest index it can read is older than that, and a relay did
not answer or was left out, that relay may still hold the newer index: a new index built on the
older one would drop its releases and halts. Publish, Resume, Everyone now and re-sign are then
refused: "Not published: a relay that did not answer or was left out may hold a newer release index
(serial N; the newest one read has serial M). Building on the older one would drop its releases and
halts. Do this again when that relay answers; if it is gone for good, take it out of the server list
under Status and relays. Halt still works." With every relay of the list read, the publish goes ahead on the newest index found.
**Halt is never refused:** the halted index goes to the relays that answer, and the result of each
relay that did not answer adds "It may hold a newer release index (serial N; the halt was built on
the newest one read, serial M) in which this release is not halted. Halt it again when this relay
answers, and check that no release is missing under Rollouts."
The re-sign at unlock leaves such a channel out and still re-signs the other channels.

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

1. **Generate the new key.** Enter the current server public key and the passphrase. The field is
   prefilled with the key the relays' hbbs sign with when they tell and agree (after a rotation
   cancelled past the switch that is its new key, not the first key of the list); when no relay
   tells, with the first key of the published list. A key other than the one the relays sign with
   is refused ("Not the key the relays sign with: … Start from that key."), and so is a start while
   the relays sign with different keys ("The relays sign with different keys: … Bring every relay to
   one key before a new rotation."). The new key pair is sealed into `keys.json` at once. Export the
   backup again.
2. **Announce it to the devices.** **Publish key list…** (review sheet, every relay) publishes
   *current + new*: devices keep presenting the current key and start to trust the new one.
   Optionally set a date from which devices stop trusting the old key on their own; the sheet
   shows it ("trusted until …"). Every publish of a key list first asks each relay which key its
   hbbs signs with and is refused when the list lacks one of them ("Not published: the list lacks
   the key these relays sign with now, and devices would stop trusting them: …"). When no relay
   tells its key, the list is published and the result says that it could not be checked.
3. **Switch the relays.** A counter shows how many devices active in the last 24 h report the list
   (heartbeat `server_keys_serial`, refreshed every 30 s; versions before key rotation never do).
   When enough do: **Save key files…** writes `id_ed25519` and `id_ed25519.pub` of the new key into
   a folder; the commands below install them on every relay with `server/rotate-key.sh switch`
   (Helm: the `helm upgrade` line). `id_ed25519` is the server's **private** key: it is created
   so that only the current Windows user can open it (a protected ACL with that user's SID only,
   nothing inherited; 0600 on macOS/Linux). A folder that already holds another `id_ed25519` pair
   (for example an SSH key) is refused: "… already holds another key pair named id_ed25519. Pick an
   empty folder." A warning with **Delete key files** stays in this
   step and the next one until you delete it (the button removes only `id_ed25519` and
   `id_ed25519.pub` from that folder, and only when the `.pub` there is a server key of this vault;
   otherwise "Nothing deleted: the id_ed25519 files in … are not a server key saved from Tether
   Manage."). **Every relay is switched** opens only when every relay's
   hbbs reports that it signs with the new key (`https://<relay>/rendezvous/pubkey`, see
   [key-rotation.md](key-rotation.md#checking-the-switch)); the list shows which relay still signs
   with the old key or cannot tell (hbbs or Caddyfile without the route: update them first), and
   **Check again** reloads it. Build new installers with the new `TETHER_PUB_KEY` from now on.
4. **Retire the old key on the devices.** **Publish list without the old key…**.
5. **Stop accepting the old key on the relays**, when the counter is high enough again:
   `rotate-key.sh retire` per relay (Helm: `key.accept=""`), then **Every relay is done**.

Devices that miss a step heal on their next policy check, because the list comes over HTTPS from
the relay API. Until then they get "Server key mismatch …" when connecting.

**Cancel rotation** (below the steps) forgets the rotation; the generated key stays in the key
file. When a relay already signs with the new key, it asks first: cancelling does not switch that
relay back, and the next rotation then starts from that key.

## Status and relays

**Relays** is the most important setting: Tether Manage publishes to exactly these relays
([relay-ha.md](relay-ha.md)). Add a relay by host name (`https://` is added), remove one, then
**Save relay list**. The first relay is used for reads and for the links of custom apps. Every
address must be `https://host` with an optional port and nothing else (no path, user name, query
or fragment; `server_ok`), and must be the relay's own address: requests are signed for that host
name and redirects are not followed.

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

One exception: a custom app's setting is ignored for every feature the profile locks (Off, Locked
or Hidden). Its signed settings have no serial or expiry and cannot be revoked on a downloaded
app, so a custom app can add restrictions and defaults but never unlock what the published policy
locks.

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
text up to 1024 characters; `update.minimum-version` must be empty or a version and
`update.channel` empty or a channel name. Unknown features or values make the whole draft "Not
publishable"; a typo never silently does nothing. The EN/DE switch only changes the labels.

The apps are less strict: one rule never makes an app ignore the policy. An app skips a rule with
a feature it does not know or a value it finds invalid, logs it (technician app: `policy rules not
applied: …`; TetherQS: `policy <serial>: rules not applied: …`) and applies the rest; a rule with a
state or field from a newer Tether Manage is skipped without a log line. An unparsable
`update.minimum-version` or `update.channel` is such a skipped rule (the channel falls back to
`stable`); Tether Manage and the relay refuse to publish one.

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
quietly outlined, any other state filled in the accent colour). A feature set in this layer has a
bar on the left and the badge **Changed from default**; one whose rule differs from the
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

The server keeps at most 50 000 devices and drops entries older than 180 days.

## Sessions

The session audit log ([api.md](api.md), "session audit log"): what the hosts reported about
incoming connections, read from **every** relay (`GET /api/admin/audit`, newest 5000 per relay
and range) and merged. A relay answers at most about 8 MiB of records and marks a cut answer with
the header `X-Audit-Truncated: 1`; the Sessions page then says "<relay> sent only the newest
records (answer cut by size); pick a shorter range". A host posts to the first relay of its build only. Records are
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

## Team

Technicians who do not hold the admin key can still see the organisation's devices and share
device tags: each technician app has its own key, and the **team roster** on the relays lists the
keys allowed in ([api.md](api.md), "Team (technician) endpoints"; security: SECURITY.md, "Team
access").

The page loads the roster from every relay (`GET /api/admin/team`), shows the first relay's and
names any relay whose roster differs or that does not answer.

| Column | Meaning |
| --- | --- |
| Name | Shown to the team as the author of tag changes |
| Fingerprint | First 16 hex digits of the SHA-256 of the key, e.g. `630D-CD29-66C4-3366` |
| Can edit shared tags | May replace the shared tag document (`write`) |
| Can read session history | May read the session audit log of one device at a time (`audit`) |

Unpublished changes to the roster are kept while Tether Manage is open, also while another page is
shown ("Not published yet. The changes are kept until you publish or reload; closing Tether Manage
discards them."). **Reload from the relays** asks before it discards them.

**Add technician**: the technician copies the public key from their app and sends it; paste it
with a name. The page checks that it is a 32-byte base64 key and shows its fingerprint: compare it
with the fingerprint the technician reads out from their app (on the phone, not through the channel
the key came by). Adding, removing and changing flags only change the local draft; **Publish…**
sends the roster to every relay (review sheet, result per relay, as for everything else). A
technician removed from the roster is locked out at once on every relay that got the new roster; a
relay that missed the publish still lets them in, so publish again until every relay has it. An
empty roster turns team access off.

**Signed team document (relays of 0.7).** A relay of 0.7 also takes an admin-signed team document
in place of this plain roster: per member also software lists (`sbom`), the agent menu (`agent`),
the customers whose devices the member sees (`customers`, e.g. `47*`), service entries, and the
sign-in settings. Once a relay has accepted one it refuses the plain roster this page publishes
today (`409`), also after a restart. Publishing the signed document from Tether Manage comes with
its sign-in release; until then this page and the plain roster work as described above. Wire format
and rules: [api.md](api.md), "The signed team document".

## Releases

**Publish the latest builds** is the usual way to release: **Choose…** the build folder once
(e.g. `dist`, searched two levels deep, so `dist/windows`, `dist/quick` and `dist/macos` are
found; the folder is remembered). For each app and platform it shows the newest build in the
folder and the highest version the relays publish on the channel. **Publish N new builds…**
signs every newer build, uploads it to every relay (not mandatory, not forced, no minimum
version) and, on `stable`, then rebuilds the download page with its saved title and text. Its
review sheet sets the **Rollout** and **Security release** for all of them, as the form below
does (see "Rollout" there). A macOS technician build without its `.dmg` next to it is skipped
and marked. Use the form below for mandatory, forced or minimum-version releases.

While it publishes, the progress replaces the review (notes, rollout and the relay list can no
longer change, so they are hidden; Cancel stays at the bottom). It shows the build being signed, then one line per relay (**Relay 1 of
2 · host**, the file, a progress bar with MB sent and percent, "waiting for the relay to confirm"
once a file is sent, "files uploaded, waiting for the other relays" when it has them all): the relays get the files **at the same time**, so a slow or silent relay
does not hold up the others. Below that, every build × relay that has finished, failed ones with
their error as soon as they fail. The same progress shows for **Sign and publish** and **Publish
to missing relays**. The result lists per relay what it now publishes, or every error there.

**What the result shows.** Per relay, what actually happened, in order: files uploaded, then the
signed release index written (from here the release is **live** on that relay: devices from
0.5.24 on update from the index), then the legacy `latest.json`. A relay whose index was written
but whose `latest.json` write failed is **live with a warning** (a check mark, "Live (release
index)", and in orange "release index written, so it is live here; legacy latest.json not
updated: <error> Older apps (before 0.5.24) do not see this release yet; run Publish to missing
relays later"); it is never reported as not
published, and when every relay ended like that the publish is a success with warnings (the
result screen opens, not an error on the review screen). The summary line is "Published on every
relay", "Live on every relay. Some need attention", "Not every relay has it" or, after a Cancel,
what is live (below). For **Publish the latest builds** each relay row names every build that
went live there and, one line per build, every build that did not with the reason (failed,
skipped because the relay did not answer, cancelled); the summary names where each build is live.
A build that fails on every relay is split per relay: each row shows only its own relay's error.
The toast matches: "Released X to channel" only when every relay has it, otherwise "... on 1 of 2
relays only", "... some relays need attention", "Too late to cancel: X is live on 2 of 2 relays"
or "Cancelled after <build>".

**Cancel** stays available while publishing; only the button stops it (Escape and a click beside
the sheet do nothing while it publishes: a click to bring the window to the front once cancelled a
whole run). Until the first relay starts writing
the signed release index, it stops the publish: the uploads and copies in progress end at once,
and the steps after them (CDN, index read and check, index write) are not started. Nothing is
published (no release index, no `latest.json`; files that already went out stay unused until the
next publish or a clean-up), the result says "Cancelled. Nothing was published", and the remaining
builds are not started (also when Cancel is pressed just as the next build starts: all builds of
one press of Publish share a run id). Once an index write has started, the publish is not rolled
back (an index cannot be taken back: relays only take a higher serial, and a "rollback" would
itself be a new publish): it finishes the index and `latest.json` on the relays it reached, every
step bounded as below, then shows **"Cancel came too late"** with a **Cancel** line "Too late to
cancel: <release> is live on <relays>" (also when `latest.json` then fails or stalls: the index
made it live). To stop devices installing it, **Halt** it under Rollouts. The automatic clean-up
does not run after a Cancel, also when Cancel is pressed while it reads the relays: its
**Cleanup** line says "skipped: Cancel was pressed, so no old versions were deleted. The release
stays live". In **Publish the latest builds**, Cancel between two builds keeps the builds already
live and says so: "Cancelled after <build>; <build> is live on every relay. The builds after it
were not published." Publish to missing relays works the same way per relay:
a relay whose index write had not started gets "Cancelled. Not published on this relay"; one that
was already writing finishes and is named in the "Too late to cancel" line.

**A relay that stops answering during a publish** fails on its own, while the others go on:

| Limit | Value | When |
| --- | --- | --- |
| First answer of a read or a small write | 10 s | everything except the file transfer itself |
| Upload stalls | 30 s | the relay takes no data for 30 s while a file goes out |
| Answer after the upload | 30 s | after the last byte the relay checks the signature over the whole file and stores it; a 200 MB release took 1.0 s on a local relay (`apps/desktop/e2e/hang-e2e.mjs`), so 30 s leaves room for slow disks |
| Download (copy for Publish to missing relays, CDN re-read) | 10 s to the first byte, then 30 s without data | the next relay that publishes the file is tried; a relay that failed once is not asked again for the other files of that copy (a 4-file mac release with a stalling first relay: one 30 s stall, not four) |
| Whole transfer | 30 min | a slow link: about 1 Mbit/s for 200 MB |
| Cloudflare API (CDN) | 30 s to the answer, uploads as above | |

With one silent relay a publish therefore ends after about 40 s (30 s answer limit plus 10 s for
reading the release index), with that relay's error and the others published. **Publish the latest
builds** leaves a relay that did not answer for one build (also when the build failed on every
relay) out of the next builds and the download page, listed there as "skipped (not reachable)", so
several builds cost those 40 s once; a left-out relay gets no request at all, also not for the
custom apps a custom download-page template shows. Ticking "Publish to the others anyway" avoids the
wait completely.

**What devices get** lists every channel × app × platform with:

- **Latest**: the highest version uploaded to any relay (from `GET /api/admin/releases`);
- per relay, what its public `latest.json` publishes: **Published · latest**, **Older · 0.3.1**
  (devices on that relay get the older version), or **Missing on <relay>**.

**Publish to missing relays** copies the release from a relay that publishes it (artifact, `.dmg`,
signatures; nothing is signed again) to the others, and writes each one its own `latest.json`
pointing at itself. A relay whose release index is older also gets the newest signed index, but
only after it has every file that index names: the files of other releases it lacks (for example
everything published while it was down) are copied to it first (with their signatures), from the
relays with the newest index. If one of them cannot be copied, that relay gets neither the index
nor `latest.json` and its result says which file was missing: an index never points devices at a
file their relay lacks. If no relay publishes the latest version yet, publish it with the form; if
relays did not answer, the error names each of them ("not reachable (…)") instead of "No relay
publishes …".

When relays were left out of a publish ("Publish to the others anyway"), they get no request at
all: not the publish, not the download page that follows it, and not **Clean up after each
publish**, which is skipped for that publish ("relays were left out …, so nothing was deleted
anywhere"), as with a relay that does not answer.

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
| Minimum version | Versions below it are blocked, whether or not they can update. It cannot be above the release's own version ("The minimum version … is above this release (…)"): no device could reach it. |
| Forced | Installed without asking after `update.restart-seconds` |
| Rollout | How fast devices get it, signed into the release index (each device has a stable bucket 0–100 per release and installs once the ramp passes it). **Standard**: 1 % at once → 10 % after 1 h → 50 % after 6 h → 100 % after 24 h. **Fast**: 10 % → 100 % after 1 h. **Immediately**: everyone at once. **Custom**: the share at once (1–100 %), then the hours from publishing until 10 %, 50 % and 100 % (fractions allowed, e.g. 0.5; at most 720 h = 30 days; each step later than the one before; steps the first share already reaches are left out). The last choice is remembered for both forms (`settings.rollout`). **Everyone now** under "Rollout" in the release index list ends a ramp early. |
| Security release | Skips the rollout: every device gets it at once |

Upload order, so that clients never see a manifest pointing to a missing file:

1. the artifact, to `download/<channel>/<app>/<platform>/<file>`
2. `<file>.sig` (macOS technician: then the `.dmg` and its `.sig`)
3. `latest.json`, last. The current `latest.json` is read first; entries of other platforms for the
   same version are kept. Each relay's manifest points at that relay's own files.

macOS builds: [macos.md](macos.md).

**Releases on the first relay** lists what is under `download/`, one line per release (channel,
app, platform, version, number of files, size). Open a line for its files: installer or archive,
`.sig`, the TetherQS `.exe.gz`, and for the macOS technician app the `.dmg` with its `.sig`. Each
folder's `latest.json` has its own line. **Delete release** removes all of a release's files on
every relay after a warning; a single file can be deleted from the opened line. The relays refuse
to delete the published release (the version in the release index or `latest.json`),
`latest.json` and `releases.json`: Tether Manage then asks a second time (**Delete anyway**) and
only then sends the forced delete. The server refuses uploads that would leave less than 2 GiB
free.

**Rollouts** lists the releases of one channel's signed index with **Halt**, **Resume** and the
ramp controls (**Everyone now** asks "Offer … to everyone now?"). A control acts on the channel
of the table it is in, also when the channel field was changed since. When the new index did not
reach every relay, a sheet "… not on every relay" lists each relay's result: devices that ask a
relay marked failed still get its old release index, so do it again once that relay answers.
While a relay that may hold a newer index does not answer, Resume and Everyone now are refused;
Halt always goes out to the relays that answer (see "A relay that may hold a newer index").

### Cleaning up old releases

Every publish adds a version to every relay; nothing removes them by itself. Above the list:

- **Keep [2] older version(s) besides the published one** (0-20, saved in the settings).
- **Clean up after each publish** (off by default): after a successful publish Tether Manage runs
  the clean-up below with that number, without asking, and adds a **Cleanup** line to the
  publish results ("Cleanup: 3 old versions deleted, 104.7 MB freed per relay").
- **Clean up old releases…** opens the review sheet. Every relay computes its own plan
  ([api.md](api.md#release-clean-up-post-apiadminreleasesprune)); the sheet merges them by channel ›
  app › platform. A version is offered (ticked) only when **every** relay that has it would delete
  it; kept versions are greyed with the reason: in the release index (also a halted rollout),
  published (`latest.json`), newer than published, kept as an older version, download page, on the
  CDN (every version in this PC's `cdn-assets.json` is kept, the next CDN deploy needs it), or
  uploaded in the last 24 h. A version only one relay protects says on which. The top line shows
  what the ticked versions free per relay; **Select all / none** for the sheet and per folder.
  **Delete N versions** asks once. Tether Manage then computes the plan again and sends only the
  versions that plan still offers, and each relay checks again and deletes only what it would
  still delete (a version published in between is kept). The results show per relay what was
  deleted, kept or already gone. If the relays hold different release indexes the sheet says so:
  a lagging relay keeps its older version, so publish to the missing relays first.

**A relay that does not answer** (10 s) blocks the clean-up on every relay: the sheet names it
with its error, shows what the relays that answered would delete ("Would be deleted once every
relay answers") and offers nothing. Reason: a version is deleted only when every relay that has it
would drop it, and a silent relay's state is unknown; deleting on the others alone would leave
the relays different with no way to check. Run it again once the relay answers, or remove the
relay from the list. A delete sent anyway returns one result per relay ("not reachable" for the
silent one, "nothing deleted: … does not answer" for the others); **Clean up after each publish**
adds "Cleanup: skipped: … does not answer, so nothing was deleted anywhere".

Only whole versions are deleted; `latest.json`, `releases.json`, the download page and unknown
files are never touched. A relay without the clean-up (tether-api before it) answers "update its
tether-api first". Each relay logs every run in `/var/lib/tether-api/release-prune.jsonl`.

How clients handle these flags: [updates.md](updates.md).

## Download page

**Design: Standard (built-in) | Custom HTML.** Standard (the default) is the generated page described
here. Custom HTML publishes your own template with `{{tether.…}}` placeholders for links, versions,
sizes, checksums and notes: see [download-page.md](download-page.md).

Standard generates a self-contained page (no external resources, light and dark) and publishes it as
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
(publish releases to every relay first). When only some relays took it, the message says so
("Download page published on 1 of 2 relays only"). The file names come from the relays' unsigned
`latest.json`: a release is linked only when the name there is exactly the name Tether Manage
publishes for that app, platform and version. The preview in Tether Manage shows the page without its
script (both sections); its links and the switch do nothing there, and **Preview** always redraws it.

## Custom apps

A custom app is a TetherQS download for one customer: their name, logo, accent colour, welcome text
and customer-module settings, signed with the admin key and appended to `TetherQS.exe` by the relay
when the private link is used ([api.md](api.md#custom-apps-branded-tetherqs-downloads)). The same
link serves macOS: a Mac browser gets the TetherQS zip with the app named
`<Name>-Support [<mac id>].app`, which fetches its signed profile at the first start
([macos.md](macos.md#custom-apps)). Nothing to do for the admin beyond publishing a macOS TetherQS
release in the app's channel; `<link>/mac` and `<link>/windows` choose a platform explicitly.

**New custom app**:

| Field | Meaning |
| --- | --- |
| Name | Window title / company name, and the download's file name (`<Name>-Support.exe`, `<Name>-Support.zip`). 1–64 characters. |
| Logo | PNG, JPEG, SVG or WebP; scaled in the app to at most 256 × 256 px and 96 KiB (PNG). Empty: Tether logo. |
| Accent colour | `#rrggbb`. Empty: the policy's `customer.accent-color`. |
| Welcome text | Up to 400 characters. Empty: the policy's `customer.welcome`. |
| Update channel | Which TetherQS build the link serves (default `stable`). |
| Link valid for | 1–366 days, default 30. |
| Settings | Customer-module features with the same editor as the policy. Order on the device: defaults < customer profile < custom app < group < machine policy. A custom app's setting is ignored for a feature the profile locks, so a custom app cannot switch on what the policy locks. |

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
Downloaded apps keep working after a revoke (on macOS: once started; a copy never started before
the revoke or a new link starts as plain TetherQS). Apps created on another PC can be revoked but not
edited (their settings are only on that PC).

## Diagnostics tickets

1. **Create ticket** with a note (who and what, up to 256 characters). Manage picks the code and
   opens it on every relay of the server setting at once. A relay that does not answer within 10 s
   is named in the error ("Saved on some relays, but not on: …"); the code is in the list and works
   on the other relays.
2. Read the 8-character code to the user. Codes avoid ambiguous characters (no I, O, 0, 1). It is
   valid for 24 hours and for one upload.
3. The user enters it in the technician app under **Settings → Diagnostics → Send to support**, or
   in TetherQS under **Help** (title bar `?`) **→ Send to support**.
4. The ticket shows the upload size and time, whichever relay the app sent it to (the list merges
   all relays, asked at once with a 10 s limit; a relay that does not answer is named above the
   table). The folder button next to an upload (**Open the upload**) downloads it from the relay
   that has it (it must start within 10 s and finish within 60 s), lists the files and shows
   `diagnostics.json`; **Save zip…** stores the bundle.
5. **Close** when the case is done: no more uploads with that code, and the ticket moves to
   **Archived** (note, dates, size, **Open**), kept 90 days with its upload. **Delete** (asks
   first) removes a ticket and its upload from every relay for good, active or archived.
   Close and Delete act on every relay; when one fails, the result of each relay is shown.

**One upload per code, per relay.** Each relay takes one upload per code. The apps stop at the
first relay that takes the upload, and a relay that has the code but used, expired or closed ends
the attempt. A second upload is still possible: Manage opens the ticket on every relay, so when an
app reaches relay 1 first after another upload went to relay 2 (where relay 1 never saw it), relay
1 takes it too. Manage then lists both ("2 uploads, one per relay"), each with its own button,
and Close and Delete act on both. The relays do not tell each other about uploads, so this race
stays; it needs the same code used twice within its 24 h.

**A relay that does not answer** costs at most 10 s in every view and small write (ticket,
policy, team, server keys, download page, release index, clean-up): Manage asks all relays at once,
gives up on one that does not start answering within 10 s (a small write must finish within 30 s),
and shows each relay's result. Release file uploads and copies have their own limits (see
Releases: 30 s without progress, 30 s for the answer after an upload). The policy bar keeps the last live serial after a failed check, marked "earlier check;
checking again failed: …".

Active tickets: uploads are kept 7 days, a code without upload expires after 24 h ("Expired, no
upload") and leaves the list after 7 days. States and retention: [api.md](api.md#post-apidiagticket).
What the bundle contains: [troubleshooting.md](troubleshooting.md#export-diagnostics).

# Running Tether for several organisations

A service provider supports several organisations whose technicians must not see each other's
machines. Tether has no tenants inside a relay: for that separation, run one deployment per
organisation.

## One deployment per organisation

A deployment is everything [getting-started.md](getting-started.md) sets up:

| Part | Why it cannot be shared |
|---|---|
| Relay (hbbs, hbbr, tether-api) | Everything below is stored once per relay, for all callers. |
| Admin key | Signs policies, release indexes, admin requests (publishing the team roster is one) and enrolment tokens. Every key in a relay's `ADMIN_PUB_KEY` is an admin of all of it; a second organisation's key in that list is a second admin of the first organisation. |
| Updater key | Signs the update files. An update signed with it is accepted by every app built with its public key. |
| Team roster | One per relay. Every member can read the device list, the shared tags and the health of every device on that relay. |
| Builds | The relay's name and the public keys are compiled into the apps, so each organisation gets its own builds. |

Several relays for one organisation ([relay-ha.md](relay-ha.md)) stay one deployment: they share
its keys and roster. Moving an organisation to new keys later is a key rotation
([key-rotation.md](key-rotation.md)).

## Why there are no tenants inside a relay

The roster, the device list, the shared tags, the session audit log, the policy, the release
indexes and the health store are each one per relay. None of them has a field that says which
organisation a record belongs to, and no route filters by one. Separating organisations inside a
relay would mean adding that to each of them and to every check; Tether does not do that.

## The customer reference

Inside one deployment, an administrator can group the devices that report their health by a
customer reference (1 to 32 characters `A-Z a-z 0-9 . _ -`, for example `4711`), set by the
enrolment token ([health.md](health.md#customer-reference)). The host list shows it in a Customer
column and finds it with `customer:4711`. It is a label for sorting and searching, not a boundary:
every team member still sees every device, and it applies only to devices with a monitoring agent.

## Two organisations on one PC

A technician who works for two organisations needs both organisations' technician apps, since
each is built for its own relay and keys. Two default builds cannot be installed side by side:
they have the same product name, so the second setup updates or replaces the first. For this case
an organisation's technician app can be built as an **instance**: the build value
`TETHER_INSTANCE` (1 to 16 characters `a-z 0-9`, for example `north`) gives it its own names,
so it installs beside the default build or beside another instance. Without the value (empty or
unset) the build is the default one, with exactly the names it always had.

What an instance `north` is called, next to the default build:

| | Default build | `TETHER_INSTANCE=north` |
|---|---|---|
| Product name, app name | `Tether` | `Tether-north` |
| Windows program | `C:\Program Files\Tether\tether.exe` | `C:\Program Files\Tether-north\tether-north.exe` |
| macOS app | `/Applications/Tether.app` | `/Applications/Tether-north.app` |
| Bundle identifier | `app.tether.desktop` | `app.tether.desktop.north` |
| Link scheme ([cli.md](cli.md#tether-links)) | `tether://` | `tether-north://` |
| Settings and logs (Windows) | `%APPDATA%\Tether` | `%APPDATA%\Tether-north` |
| Windows service ([unattended.md](unattended.md)) | `Tether` | `Tether-north` |
| Windows firewall rule | `Tether` | `Tether-north` |
| Machine policy ([policy.md](policy.md#layers-and-validation)) | `HKLM\SOFTWARE\Policies\Tether`, macOS `net.tether.app` | `HKLM\SOFTWARE\Policies\Tether-north`, macOS `net.tether.app.north` |
| Default recording folder | `Videos\Tether` (macOS `Movies/Tether`) | `Videos\Tether-north` (macOS `Movies/Tether-north`) |

Each instance also has its own device ID: the ID is derived from the PC's network address, and an
instance build mixes its name into it (`apps/desktop/src-tauri/src/main.rs`,
`instance_id_salt`), so the two apps on one PC do not register the same ID. Its own settings
folder means its own passwords, its own technician key for the team roster
([management.md](management.md#team)) and its own policy cache.

The value applies to the technician app (`apps/desktop`) only. TetherQS and Tether Manage do not
read it; one Tether Manage serves several organisations through `TETHER_MANAGE_DIR`
([management.md](management.md#data-folder)).

### Building an instance

The build needs `TETHER_INSTANCE` **and** the matching Tauri configuration from
`scripts/instance-config.sh`, always together. The variable names what the program calls itself
(`apps/desktop/src-tauri/build.rs`); the configuration names the product, the program file, the
identifier and the link scheme. A build with only the variable produces a program that calls
itself `Tether-north` inside an installer named `Tether`, which nothing checks. The `desktop`
workflow and `scripts/build-macos.sh` pass both; a build by hand must too:

```bash
export TETHER_INSTANCE=north     # with the four values of getting-started.md
cd apps/desktop
npx tauri build --bundles nsis --config "$(bash ../../scripts/instance-config.sh "$TETHER_INSTANCE")"
# -> src-tauri/target/release/bundle/nsis/Tether-north_<version>_x64-setup.exe
```

- A value outside `a-z 0-9`, or longer than 16 characters, stops the build with
  `TETHER_INSTANCE must be empty or 1 to 16 characters a-z, 0-9`; `instance-config.sh` refuses
  it the same way.
- CI: set `TETHER_INSTANCE` as a repository **variable** (Settings → Secrets and variables →
  Actions → Variables; it is not a secret) next to the four secrets. The workflow also runs
  `scripts/check-hooks.sh`, which fails when the installer hooks name `Tether` literally (an
  instance's installer would otherwise stop the default build's service or delete its data).
- macOS: `TETHER_INSTANCE=north bash scripts/build-macos.sh` builds `Tether-north.app`.
- **The published file names stay `Tether_<version>_x64-setup.exe`,
  `Tether_<version>_universal.app.tar.gz` and `Tether_<version>_universal.dmg`**, because the
  release index accepts only those. The workflow and `build-macos.sh` rename the files; after a
  build by hand, rename `Tether-north_<version>_x64-setup.exe` to `Tether_<version>_x64-setup.exe`
  before publishing it. The macOS archive keeps `Tether-north.app` inside.

### Limits

- **Only one of the two gets direct connections.** Both apps listen on TCP 21118 and answer the
  LAN lookup on UDP 21119 (`vendor/rustdesk/src/rendezvous_mediator.rs`, `get_direct_port`;
  `vendor/rustdesk/src/lan.rs`, `get_broadcast_port`). The one started first holds the ports;
  sessions to the other go through the relay ([policy.md](policy.md#direct-connections-networkdirect)).
- **`tether://` links open only the default build.** An instance answers `tether-north://` links
  only, and refuses a `tether://` link given to it. Links in a ticket system must use the scheme of
  the organisation's build.
- **An existing install cannot be turned into an instance.** The instance is a fresh install with
  a new ID, new settings and a new technician key; the default build stays installed until it is
  uninstalled. Whoever saved the old ID for this PC needs the new one.
- **Each organisation publishes only its own build to its own relays.** The updater installs what
  the relay's release index offers under the shared file names. A default build published to an
  instance's relay would install a second app (`Tether`) on Windows and replace the instance app
  on macOS; the same holds the other way round.
- **Some texts still say Tether:** the `--help` text (`Usage: tether`, `tether://connect/<id>`)
  and the user agent of its HTTP requests.
- **macOS permissions** (Screen Recording, Accessibility) are granted per bundle identifier: each
  instance needs its own, as a new app does.
- CI skips `scripts/smoke-macos.sh` for an instance build (it checks `Tether.app` only).

### What was tried

- The unit tests of the technician app, built once without and once with `TETHER_INSTANCE=north`
  (the default names are asserted in the first build), and the build refusing `North`, `a b`,
  17 characters and `../x`.
- One Windows installer with `TETHER_INSTANCE=north`, built and inspected, **not installed**:
  `Tether-north_0.6.3_x64-setup.exe`, product name `Tether-north`, program `tether-north.exe`,
  identifier `app.tether.desktop.north`, link scheme `tether-north` in its registry entries.

Not tried: two builds installed side by side on one PC, the service and the firewall rule of an
instance on a real machine, updating an instance build, and the macOS build of an instance.

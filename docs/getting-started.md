# Getting started: your own Tether deployment

From nothing to a first session. You need a Linux server with a public DNS name and TCP 443 (and
80) open, and a Windows PC to build on. Plan an afternoon: most of it is the first build.

Only want to see the interface? Open the [browser demo](https://fnpanic.github.io/tether/demo/)
(sample data, connects to nothing).

> This page puts the steps from the other documents in order. The single steps are what the
> maintainer's deployment uses; the whole path on a fresh server and a fresh PC has not been
> walked by someone new yet. If a step stops you, please say so in the
> [discussions](https://github.com/fnpanic/tether/discussions).

## The four values

Every app is built with four values compiled in. They decide which relay it talks to and whose
signatures it accepts.

| Value | What it is | Where it comes from |
| --- | --- | --- |
| `TETHER_SERVER` | Your relay's DNS name | You choose it |
| `TETHER_ADMIN_PUB_KEY` | Admin public key (signs policies and admin requests) | Tether Manage, step 2 |
| `TETHER_UPDATER_PUB_KEY` | Update signing public key | Tether Manage, step 2 |
| `TETHER_PUB_KEY` | The relay's server public key | The relay, step 3 |

A fifth value is optional: `TETHER_INSTANCE` (for example `north`) builds a technician app that
installs beside another organisation's on the same PC. It needs a `--config` from
`scripts/instance-config.sh` as well; see
[several-organisations.md](several-organisations.md#two-organisations-on-one-pc). Leave it unset
for a single organisation.

## 1. Get the sources and the build tools

Windows: Rust (MSVC), Node 22+, LLVM (`LIBCLANG_PATH`), VS Build Tools, vcpkg (`VCPKG_ROOT`).
Run in Git Bash:

```bash
git clone https://github.com/fnpanic/tether && cd tether
scripts/bootstrap.sh
cd vendor/rustdesk && VCPKG_DEFAULT_HOST_TRIPLET=x64-windows-static \
  $VCPKG_ROOT/vcpkg install --triplet x64-windows-static --x-install-root=$VCPKG_ROOT/installed
cd ../..
```

## 2. Build Tether Manage and create your keys

Tether Manage builds without the four values.

```bash
cd apps/manage && npm ci && npx tauri build
```

Start it, choose **Create keys** and set a passphrase. Then **Keys → Export backup…** and store
the backup and the passphrase in two separate places: without them you cannot publish policies or
updates to installed apps any more ([management.md](management.md#first-run-keys-and-passphrase)).
The **Keys** page shows the admin public key and the update signing public key.

## 3. Install the relay

A relay is three services (`hbbs` for IDs, `hbbr` for relayed sessions, `tether-api` for
policies, updates and the device list) behind an HTTPS front end on port 443. `server/install.sh`
sets all of it up with systemd, including the certificate (Caddy).

Build the three binaries on any Linux machine with Rust and `musl-tools`
(`rustup target add x86_64-unknown-linux-musl`). The `server` workflow does the same and offers
the result as the artifact `tether-server-linux-x86_64`.

```bash
git clone https://github.com/fnpanic/tether && cd tether
scripts/bootstrap.sh
bash scripts/build-relay.sh /tmp/relay            # -> /tmp/relay/bin/{hbbs,hbbr,tether-api}
```

Then, on the server, in a copy of the repository's `server/` folder and the binaries:

```bash
sudo ADMIN_PUB_KEY=<admin public key from step 2> ./install.sh relay.example.com /tmp/relay/bin
sudo cat /var/lib/tether/id_ed25519.pub           # TETHER_PUB_KEY
```

The relay must know the admin public key, or it refuses what Tether Manage sends. Details,
several relays and Kubernetes: [deployment.md](deployment.md), [relay-ha.md](relay-ha.md),
[kubernetes.md](kubernetes.md).

`server/docker-compose.yml` starts only `hbbs` and `hbbr`, without `tether-api` and the front
end on 443. That is enough for stock RustDesk clients, not for a Tether deployment.

In Tether Manage, add the relay under **Status and relays** and save the list. Its row should
say "Admin key accepted".

## 4. Build the apps

```bash
export TETHER_SERVER=relay.example.com
export TETHER_PUB_KEY=<from step 3>
export TETHER_ADMIN_PUB_KEY=<from step 2>
export TETHER_UPDATER_PUB_KEY=<from step 2>
cd apps/desktop && npm ci && npx tauri build --bundles nsis   # Tether_<version>_x64-setup.exe
cd ../quick && npm ci && npx tauri build                       # TetherQS.exe (portable)
```

macOS: `bash scripts/build-macos.sh` on a Mac ([macos.md](macos.md)).

Without your own build machine: fork the repository, store the four values as Actions secrets
with the same names and run the `desktop` workflow.

## 5. First session

Install Tether on two PCs (or, to test on one PC, start the same Tether a second time with the
development profile `TETHER_PROFILE=b`, which gives it a second ID and settings folder; this is not
an instance build for a second organisation,
[several-organisations.md](several-organisations.md#two-organisations-on-one-pc)). Both
should show **Ready**. Enter the other device's ID, then its one-time password or accept the
request on its screen.

If it stays offline: **Settings → Diagnostics → Test connection**, then
[troubleshooting.md](troubleshooting.md).

## 6. Hand it to your users

- Sign the Windows files, or Windows and browsers will warn about them
  ([code-signing.md](code-signing.md)).
- Publish the builds and a download page with Tether Manage ([updates.md](updates.md),
  [management.md](management.md#download-page)).
- Set a policy before many devices are out ([policy.md](policy.md)).

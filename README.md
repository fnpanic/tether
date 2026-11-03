# Tether

Fast, private, self-hosted remote desktop for Windows and macOS, with a Linux relay.

- **Desktop app:** Tauri 2 + React. One binary is both viewer and host.
- **Core:** a lightly patched fork of the RustDesk core (capture, codecs, input, NAT traversal,
  end-to-end encryption), linked as a library behind a `tauri` feature.
- **Streaming:** FluxRT, a clean-room low-latency profile with hardware encoding on the host and
  WebCodecs hardware decoding in the viewer. See [docs/fluxrt-spec.md](docs/fluxrt-spec.md).
- **Relay:** `hbbs` (ID/rendezvous) and `hbbr` (relay) on Linux via systemd or Docker.
- **Security:** end-to-end encryption is mandatory, with no plaintext fallback. The server key is
  pinned. 2FA, strong passwords and brute-force limits are built in. See [SECURITY.md](SECURITY.md).
- **Design:** [DESIGN.md](DESIGN.md).
- **Networks:** only TCP 443 (port 80 as fallback), works behind proxies and TLS inspection. See
  [docs/corporate-networks.md](docs/corporate-networks.md) and
  [docs/connection-resilience.md](docs/connection-resilience.md).
- **Management:** Tether Manage (`apps/manage`) signs policies and releases; the relay API
  serves them. Customer module TetherQS (`apps/quick`): portable, no admin rights. See
  [docs/management.md](docs/management.md), [docs/policy.md](docs/policy.md),
  [docs/updates.md](docs/updates.md), [docs/quick-support.md](docs/quick-support.md),
  [docs/recording.md](docs/recording.md), [docs/cli.md](docs/cli.md),
  [docs/troubleshooting.md](docs/troubleshooting.md), [docs/api.md](docs/api.md).
- **Changes:** [CHANGELOG.md](CHANGELOG.md).

## Layout

```
apps/desktop/            Tauri app (src/ = React UI, src-tauri/ = Rust shell)
patches/                 Our changes to the pinned upstream sources
scripts/bootstrap.sh     Fetch vendor/ at pinned tags and apply patches
server/                  Relay deployment: Dockerfile, compose, systemd units, install.sh
docs/                    FluxRT spec, benchmarks
```

## Build

Prerequisites:
- Windows: Rust (MSVC), Node 22+, LLVM (`LIBCLANG_PATH`), VS Build Tools.
- macOS: Xcode command-line tools.
- Both: vcpkg (`VCPKG_ROOT`).

```bash
scripts/bootstrap.sh
cd vendor/rustdesk && VCPKG_DEFAULT_HOST_TRIPLET=x64-windows-static \
  $VCPKG_ROOT/vcpkg install --triplet x64-windows-static --x-install-root=$VCPKG_ROOT/installed
cd ../../apps/desktop && npm ci
TETHER_SERVER=relay.example.com TETHER_PUB_KEY=<key from install.sh> npx tauri build
```

On macOS use the triplets `arm64-osx` / `x64-osx`. Release builds refuse to compile without
`TETHER_SERVER` and `TETHER_PUB_KEY`.

For development, `npm run dev` serves the UI against an in-browser mock backend. To run a real host
and viewer side by side on one machine, start a second instance with `TETHER_PROFILE=b`.

## Deploy the relay

```bash
# Docker (host networking)
RELAY_HOST=relay.example.com docker compose -f server/docker-compose.yml up -d
# or systemd with prebuilt binaries
sudo server/install.sh relay.example.com ./bin
```

Tether clients use only TCP 443: put `server/caddy/Caddyfile` (`tether-https.service`) in front. Stock RustDesk clients additionally need TCP 21115–21117 and UDP 21116. See [docs/corporate-networks.md](docs/corporate-networks.md).

## License

AGPL-3.0, inherited from RustDesk. You must offer the complete corresponding source, including
`patches/`, to everyone you distribute Tether to.

# Policy features

Generated from `tether_trust::policy::FEATURES` by `TETHER_WRITE_POLICY_DOC=1 cargo test policy_doc` in `apps/desktop/src-tauri`; a test fails when a feature is missing here.

States per layer: `on` (user may change, `value` is the default), `off` (locked off), `locked` (locked to `value`), `hidden` (locked off and not shown). Layers, later wins: defaults < profile < custom app < group < machine policy; a custom app's setting is ignored for a feature the profile locks (`HKLM\SOFTWARE\Policies\Tether` REG_SZ `PolicyJson`, or `/Library/Managed Preferences/net.tether.app.plist` key `PolicyJson`, both a `Layer` JSON such as `{"connect.outgoing":{"state":"off"}}`). Core options in `FLOOR` (TLS, encryption, 443-only networking, no silent recording) can never be set by policy.

| Feature | Area | Description | Default | Profiles | Core option |
| --- | --- | --- | --- | --- | --- |
| `permission.control` | permissions | Allow mouse and keyboard control | `Y` | technician, customer | `enable-keyboard` |
| `permission.clipboard` | permissions | Allow clipboard sync | `Y` | technician, customer | `enable-clipboard` |
| `permission.file-transfer` | permissions | Allow file transfer | `Y` | technician, customer | `enable-file-transfer` |
| `permission.audio` | permissions | Share audio | `N` | technician, customer | `enable-audio` |
| `permission.restart` | permissions | Allow remote restart | `Y` | technician, customer | `enable-remote-restart` |
| `permission.tunnel` | permissions | Allow TCP tunnels | `N` | technician, customer | `enable-tunnel` |
| `permission.block-input` | permissions | Allow blocking local input | `Y` | technician, customer | `enable-block-input` |
| `security.approve-mode` | security | How incoming sessions are approved (both, click, password) | `both` | technician, customer | `approve-mode` |
| `security.verification-method` | security | Accepted passwords (both, use-temporary-password, use-permanent-password) | `both` | technician, customer | `verification-method` |
| `security.temporary-password-length` | security | One-time password length (6, 8, 10) | `10` | technician, customer | `temporary-password-length` |
| `security.permanent-password` | security | Allow setting a permanent password | `Y` | technician | – |
| `security.unattended` | security | Allow unattended access (background service) | `Y` | technician | – |
| `connect.outgoing` | connections | Allow connecting to other devices | `Y` | technician | – |
| `connect.incoming` | connections | Allow incoming sessions | `Y` | technician, customer | – |
| `connect.chat` | connections | Chat in sessions | `Y` | technician, customer | – |
| `connect.reconnect-minutes` | connections | Keep reconnecting after a connection loss (minutes) | `10` | technician, customer | – |
| `settings.server` | connections | Allow changing the relay server | `N` | technician | – |
| `servers.list` | connections | Additional relay servers (host names or wss:// URLs, comma-separated) | – | technician, customer | – |
| `network.direct` | connections | Direct connections (same network) beside the relay (auto, off) | `auto` | technician, customer | – |
| `video.quality` | video | Default image quality (best, balanced, low) | `balanced` | technician | `image-quality` |
| `video.codec` | video | Preferred codec (auto, h264, h265, av1, vp9, vp8) | `auto` | technician | `codec-preference` |
| `recording.mode` | recording | Session recording (off, on-request, always) | `on-request` | technician | – |
| `recording.folder` | recording | Recording folder | – | technician | – |
| `recording.upload` | recording | Upload recordings to (none, unc, https-put) | `none` | technician | – |
| `recording.upload-target` | recording | Upload target (UNC path or HTTPS URL) | – | technician | – |
| `recording.upload-token` | recording | Upload bearer token (HTTPS) | – | technician | – |
| `recording.keep-local` | recording | Keep local copy after upload | `Y` | technician | – |
| `recording.max-days` | recording | Delete local recordings after (days, 0 = never) | `0` | technician | – |
| `recording.consent-text` | recording | Consent question shown to the other party | `The technician wants to record this session. Allow?` | technician, customer | – |
| `update.enabled` | updates | Automatic updates | `Y` | technician, customer | – |
| `update.channel` | updates | Update channel | `stable` | technician, customer | – |
| `update.check-hours` | updates | Check for updates every (hours) | `6` | technician, customer | – |
| `update.minimum-version` | updates | Minimum version (older versions cannot connect) | – | technician, customer | – |
| `update.force` | updates | Install required updates automatically | `N` | technician, customer | – |
| `update.rollout` | updates | Staged rollouts: early joins the first wave, late waits until a release reaches everyone (early, default, late) | `default` | technician, customer | – |
| `update.restart-seconds` | updates | Countdown before a forced update restarts the app (s) | `60` | technician, customer | – |
| `policy.refresh-minutes` | policy | Check for policy changes every (minutes) | `15` | technician, customer | – |
| `policy.fast-apply` | policy | Fast apply (check every minute) | `N` | technician, customer | – |
| `ui.advanced` | interface | Advanced mode: host inventory with search and tags as the home view (off, available, forced) | `available` | technician | – |
| `diagnostics.page` | diagnostics | Diagnostics page | `Y` | technician, customer | – |
| `diagnostics.upload` | diagnostics | Send diagnostics to support | `Y` | technician, customer | – |
| `diagnostics.verbose-logging` | diagnostics | Allow verbose logging | `Y` | technician, customer | – |
| `customer.title` | customer | Window title / company name | `Tether Support` | customer | – |
| `customer.accent-color` | customer | Accent colour (#rrggbb) | `#0071e3` | customer | – |
| `customer.welcome` | customer | Welcome text | `Give this ID and password to your support technician.` | customer | – |
| `customer.close-after-session` | customer | Quit when the session ends | `N` | customer | – |
| `customer.elevate` | customer | Admin access for UAC prompts (Elevate button, technician requests) | `Y` | customer | `allow-elevate` |

## Layers and validation

- **Custom app** is the settings layer signed into a custom TetherQS download ([quick-support.md](quick-support.md#custom-apps-branded-downloads)). It is applied after the profile and before the group and the machine policy. Its setting is ignored for every feature the policy profile locks (`off`, `locked` or `hidden`), so a custom app can add restrictions and defaults but never unlock what the policy locks: the signed profile has no serial and no expiry and cannot be revoked. Its name, accent colour and welcome text still replace `customer.title`, `customer.accent-color` and `customer.welcome`.
- **Tether Manage and the relay validate strictly** (`Bundle::validate`): an unknown feature, a bad value, an unknown profile or group make the whole bundle invalid.
- **In the apps one rule never fails the policy** (`Bundle::from_json`): a rule with an unknown feature or a bad value is left out and the rest of the policy applies. The log names it: `policy rules not applied: profile: unknown feature "…"` (TetherQS: `policy <serial>: rules not applied: …`). A rule with a state or a field this app version does not know (from a newer Tether Manage) is left out without a log line. Still refused as a whole: broken JSON, an unknown field at the top level or in a group, a device assigned to a group that does not exist. TetherQS opens the profile attached to a custom app the same way (`custom::open_app`).
- **Machine policy and custom app are checked rule by rule**: one invalid rule is left out and logged the same way; the other rules of that layer apply.
- `update.minimum-version` must be empty or a version (`0.5.35`, `1.0`, `0.5.0-test.1`), `update.channel` empty or a channel name (lower case, e.g. `stable`, `beta-2`). Tether Manage and the relay refuse to publish anything else (`Bundle::validate`). An app that meets such a value in a published policy skips that rule: the minimum is then empty and the channel falls back to `stable`. A minimum version with spaces around it still blocks.
- **A permission locked off is taken from running sessions too**, and from sessions that resume, in both apps (technician app: in the connection window's process, `sink::enforce_permission_locks` and `revoke_locked`). A running file-transfer connection is closed when file transfer is locked off. The switch in the window can then only be turned off.
- **`recording.upload-token` is not a secret**: it is part of the policy bundle, and `GET /api/policy` needs no authentication, so anyone who can reach the relay can read it. Give it write-only rights ([recording.md](recording.md#storage-upload-and-retention)).
- TetherQS keeps the highest policy serial it accepted in `policy-serial` beside its cache, like the technician app: an older signed policy is refused also when `policy.json` is missing or was replaced.

## Direct connections (`network.direct`)

`auto` (default): when a session starts, the viewer also asks the local network for the host (one UDP broadcast lookup on port 21119, answered only by a host with direct connections on, and only with its listener port) and races a direct TCP connection to it (port 21118) against the relay. The first path that completes the end-to-end key exchange wins; the host must prove the key the rendezvous server signed for its ID, and applies the same password / Accept / permissions as on the relay. When no host answers within 300 ms, or on corporate networks where the lookup cannot leave the PC, the session uses the relay exactly as before (no added delay). The session window shows the path: `Direct (LAN)` (TCP on the LAN, or WebRTC between two private addresses), `Direct (IPv6)`, `P2P` (NAT hole punching across networks), `Relay (UDP)` (TURN) or `Relay`.

Across networks (`auto`, no proxy on either side) the viewer also sends a WebRTC offer through the rendezvous WebSocket; ICE tries global IPv6 and NAT hole punching (STUN on UDP 21116 of our own relays, never a public STUN server). A ready relay waits at most 0.4 s for that attempt. A side with a proxy offers/answers LAN-only: no STUN/TURN server, only private addresses, remote candidates on public addresses dropped, so nothing bypasses the proxy towards the internet; two PCs on the same LAN still get **Direct (LAN)**, everything else uses the relay ([corporate-networks.md](corporate-networks.md#lan-paths-behind-a-proxy)). Details: [relay-ha.md](relay-ha.md#ipv6-and-nat-hole-punching-webrtcice).

A host listens only where that raises no firewall prompt: on Windows the installed Tether gets an inbound rule `Tether` (local subnet only) from its installer; TetherQS listens only when an admin created an inbound rule named `Tether QS` for it; on macOS only while the application firewall is off. `off`: never look up, never listen, no WebRTC offer (viewer) or answer (host), always relay.

# Policy features

Generated from `tether_trust::policy::FEATURES` by `TETHER_WRITE_POLICY_DOC=1 cargo test policy_doc` in `apps/desktop/src-tauri`; a test fails when a feature is missing here.

States per layer: `on` (user may change, `value` is the default), `off` (locked off), `locked` (locked to `value`), `hidden` (locked off and not shown). Layers, later wins: defaults < profile < group < machine policy (`HKLM\SOFTWARE\Policies\Tether` REG_SZ `PolicyJson`, or `/Library/Managed Preferences/net.tether.app.plist` key `PolicyJson`, both a `Layer` JSON such as `{"connect.outgoing":{"state":"off"}}`). Core options in `FLOOR` (TLS, encryption, 443-only networking, no silent recording) can never be set by policy.

| Feature | Area | Description | Default | Profiles | Core option |
| --- | --- | --- | --- | --- | --- |
| `permission.control` | permissions | Allow mouse and keyboard control | `Y` | technician, customer | `enable-keyboard` |
| `permission.clipboard` | permissions | Allow clipboard sync | `Y` | technician, customer | `enable-clipboard` |
| `permission.file-transfer` | permissions | Allow file transfer | `Y` | technician, customer | `enable-file-transfer` |
| `permission.audio` | permissions | Share audio | `Y` | technician, customer | `enable-audio` |
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
| `update.restart-seconds` | updates | Countdown before a forced update restarts the app (s) | `60` | technician, customer | – |
| `policy.refresh-minutes` | policy | Check for policy changes every (minutes) | `15` | technician, customer | – |
| `policy.fast-apply` | policy | Fast apply (check every minute) | `N` | technician, customer | – |
| `diagnostics.page` | diagnostics | Diagnostics page | `Y` | technician, customer | – |
| `diagnostics.upload` | diagnostics | Send diagnostics to support | `Y` | technician, customer | – |
| `diagnostics.verbose-logging` | diagnostics | Allow verbose logging | `Y` | technician, customer | – |
| `customer.title` | customer | Window title / company name | `Tether Support` | customer | – |
| `customer.accent-color` | customer | Accent colour (#rrggbb) | `#0071e3` | customer | – |
| `customer.welcome` | customer | Welcome text | `Give this ID and password to your support technician.` | customer | – |
| `customer.close-after-session` | customer | Quit when the session ends | `N` | customer | – |
| `customer.elevate` | customer | Offer "Elevate" (admin access for UAC prompts) | `Y` | customer | – |

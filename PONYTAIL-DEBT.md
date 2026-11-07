# Shortcut debt ledger

Every deliberate shortcut in the code carries a `shortcut:` comment naming its limit (ceiling) and
when to revisit it (upgrade). This ledger collects them so a deferral does not quietly become
permanent. Generated 2026-11-07 with
`git grep -nE '(#|//|/[*]) ?(shortcut|ponytail):'`; core and relay paths are in `vendor/`, which is
generated from `patches/*.patch`.

`no-trigger` = the comment names no upgrade trigger (these rot silently).
`trigger reached` = the named trigger has already happened.

## apps/desktop

- `apps/desktop/src-tauri/src/app.rs:227`: forwarding `--connect` to the already running instance.
  ceiling: Windows only; elsewhere `--connect` runs in its own process. upgrade: `no-trigger`.
- `apps/desktop/src-tauri/src/netpolicy.rs:217`: corporate proxy on macOS. ceiling: static HTTPS
  proxy only, no PAC. upgrade: when a customer needs PAC (CFNetwork).
- `apps/desktop/src-tauri/src/policy.rs:336`: machine policy on macOS read via `defaults`, no plist
  crate. ceiling: fine for one string key. upgrade: when more keys are read.

## apps/quick

- `apps/quick/src-tauri/src/custom.rs:109`: custom-app profile cache (macOS). ceiling: a failed cache
  write is ignored, the profile is fetched again at the next start. upgrade: `no-trigger`.
- `apps/quick/src-tauri/src/netpolicy.rs:212`: corporate proxy on macOS. ceiling: static HTTPS proxy
  only, no PAC. upgrade: when a customer needs PAC (CFNetwork).

## vendor/rustdesk (patches/rustdesk.patch)

- `vendor/rustdesk/src/server/own_ui_guard.rs:427`: macOS part of the guard that keeps Tether's own
  window from remote input. ceiling: "compile-unverified on macOS". upgrade: "the next Mac build".
  `trigger reached`: 0.5.20 to 0.5.25 were built for macOS.
- `vendor/rustdesk/src/tauri_ui/mod.rs:153`: a file transfer is matched to the first remote session
  to the same peer. ceiling: two sessions to one peer at once are rare. upgrade: implied (when two at
  once to one peer happen).

## vendor/rustdesk/libs/hbb_common (patches/hbb_common.patch)

- `vendor/rustdesk/libs/hbb_common/src/webrtc.rs:1234`: relay-vs-WebRTC race state is process-wide.
  ceiling: two sessions starting together share it. upgrade: thread per-session state through the
  race.

## vendor/rustdesk-server (patches/rustdesk-server.patch)

- `vendor/rustdesk-server/src/metrics.rs:348`: CPU metric. ceiling: assumes USER_HZ = 100 (Linux
  x86_64 and arm64). upgrade: use `sysconf` if that changes.
- `vendor/rustdesk-server/src/rendezvous_server.rs:1796`: relay choice by load. ceiling: load is
  sampled every 3 s, so a burst inside one interval goes to the same relay. upgrade: count handed-out
  sessions locally if bursts matter (docs/scaling.md, several relays).

## tools/macbuild

- `tools/macbuild/mac/runner.py:592`: killing a job's process group. ceiling: the pgid could be
  reused after a reboot; harmless (own uid only). upgrade: `no-trigger`.

11 markers, 3 with no trigger, 1 with its trigger reached.

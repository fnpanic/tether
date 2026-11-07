# Shortcut debt ledger

Every deliberate shortcut in the code carries a `shortcut:` comment naming its limit (ceiling) and
when to revisit it (upgrade). This ledger collects them so a deferral does not quietly become
permanent. Generated 2026-11-07 with
`git grep -nE '(#|//|/[*]) ?(shortcut|ponytail):'`; core and relay paths are in `vendor/`, which is
generated from `patches/*.patch`.

Reviewed 2026-11-07 (branch `debt`): of the 11 markers, 5 were fixed, 2 removed (no longer a
shortcut) and 4 kept, each now with a trigger that can be observed.

## Open (4)

- `apps/desktop/src-tauri/src/netpolicy.rs:217` and `apps/quick/src-tauri/src/netpolicy.rs:212`:
  corporate proxy on macOS. ceiling: static HTTPS proxy from `scutil --proxy` only; a network that
  only offers PAC/WPAD goes direct. upgrade: when a customer's Macs show
  `ProxyAutoConfigEnable : 1` in `scutil --proxy` (no customer has PAC today). KEPT. The two files
  differ in the firewall rule name, how the target URL is built and the Windows service's machine
  proxy (desktop only), and they live in two crates without a shared one that both could use for
  this, so they stay two files.
- `apps/desktop/src-tauri/src/policy.rs:336`: machine policy on macOS read via `defaults`, no plist
  crate. ceiling: one string key (`PolicyJson`, the only one read). upgrade: when a second managed
  key or a non-string value is read. KEPT.
- `vendor/rustdesk-server/src/rendezvous_server.rs:1796` (patches/rustdesk-server.patch): relay
  choice by load. ceiling: load is sampled every 3 s, so a burst inside one interval goes to the same
  relay. Moot today: one primary relay, the others are backups, and hbbs always picks the primary.
  upgrade: once two or more primaries run (docs/scaling.md's 8-node plan) and a burst can reach a
  relay's headroom; then count handed-out sessions locally. KEPT.

## Done

- `vendor/rustdesk/libs/hbb_common/src/webrtc.rs:1234`: relay-vs-WebRTC race state was
  process-wide. FIXED. Failure: sessions started together (Advanced mode, or screen + file
  transfer) shared "the peer offered a TURN candidate in the last 5 s", so a session whose peer
  offered none still waited 800 ms more for WebRTC before taking the relay. The time is now kept
  per WebRTC stream and the race asks its own stream (`race_transports_prefer_webrtc(...,
  turn_offered)`). Tests: `test_remote_relay_candidate_is_per_stream`,
  `turn_candidate_extends_the_hold_for_this_session_only`; keyrot-e2e starts screen and file
  transfer at once in one viewer.
- `apps/desktop/src-tauri/src/app.rs:227`: `--connect` forwarded to the running instance on
  Windows only. FIXED. On macOS, `tether --connect <id>` while Tether ran started a second process
  (second Dock icon, its own session list). It now goes through the single-instance plugin's socket,
  which `tell_main` already used, and only to a socket the same user owns (`/tmp` is shared and the
  request can carry a password). Linux has no forwarding (no Linux build).
- `vendor/rustdesk-server/src/metrics.rs:348`: CPU metric assumed USER_HZ = 100. FIXED:
  `sysconf(_SC_CLK_TCK)` once, 100 if it fails (`clock_ticks_from_sysconf_or_100`).
- `tools/macbuild/mac/runner.py:592`: killing a job's process group after a runner restart could
  hit a reused pid. FIXED: only if the group leader started within 5 s of the job's `started_at`
  (`started_near`, `ps -o lstart`); tests `test_recover_spares_a_reused_pid`, `test_started_near`.
  Takes effect on the Mac with the next runner update job.
- `apps/quick/src-tauri/src/custom.rs:109`: a failed custom-app profile cache write was ignored.
  FIXED: logged as a warning (path and error); the profile is still fetched again at the next start.
- `vendor/rustdesk/src/server/own_ui_guard.rs:427`: "compile-unverified on macOS until the next Mac
  build". REMOVED: the macOS part has been built since 0.5.13 and checked on a Mac (docs/macos.md:
  keys dropped only while Tether has a window on screen, 0.5.13; remote clicks on Tether's own
  windows ignored, 0.5.20).
- `vendor/rustdesk/src/tauri_ui/mod.rs:153`: a file transfer opened without a source session takes
  the login of "the first" remote session to the same peer. REMOVED: there is never more than one;
  `open_session_with` (apps/desktop app.rs) keeps one window per peer and connection type, and
  view-only is an option of that one session. A file transfer opened from a session uses that
  session's key (`login_from`).

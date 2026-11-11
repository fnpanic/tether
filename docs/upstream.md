# Upstream RustDesk: pin, updates and the pull request review

Tether links the RustDesk core as a library. `scripts/bootstrap.sh` fetches it at a pinned tag
(checked against a pinned commit) into `vendor/rustdesk` and applies `patches/rustdesk.patch` and
`patches/hbb_common.patch`. This page records what the pin is, how to move it, and which upstream
pull requests were looked at and why they were taken or left out.

## Current pin

| | Tag | Commit | Since |
| --- | --- | --- | --- |
| rustdesk | 1.5.3 | `95cca3f661f635b3cc85c04935fe8d96d8f432ef` | 2026-11-11 (before: 1.5.1, `23e5bb47`) |
| hbb_common (submodule) | – | `fae47e024dd83cef6aaecde0aa7d11731df11649` | unchanged between 1.5.1 and 1.5.3 |
| rustdesk-server | 1.1.16 | `73523b31cfd25d77dee862e6fc9f5e1fb5e485ef` | unchanged |

On top of 1.5.3 the patch carries three upstream changes that are not in the tag:

| Upstream PR | State on 2026-11-11 | What it is |
| --- | --- | --- |
| 16954 | merged after the tag | `default-net` 0.14 → patched 0.22 (`rustdesk-org/netdev`, branch `rustdesk-0.22`) |
| 16240 | open, by the maintainer | file sizes declared in the Windows clipboard file list |
| 16974 | open, by the maintainer | `sodium_init()` at the start of `global_init` (`src/common.rs` part only) |

Drop each of them from the patch when a later pin contains it.

## Moving the pin

Do it in a separate worktree with its own `vendor/`, so a running build or uncommitted vendor work
in the main checkout is not disturbed.

1. Clone the new tag into `vendor/rustdesk` (as `bootstrap.sh` does) and fetch the old pinned
   commit into the same clone (`git fetch --depth 1 origin <old commit>`).
2. Check out the old commit, apply `patches/rustdesk.patch`, commit it locally, check out the new
   tag and `git cherry-pick -n` that commit. Git merges three-way; resolve the conflicts. From
   1.5.1 to 1.5.3 there were three, all "keep both sides": `Cargo.toml` (a new profile section
   beside our `[patch]`), `src/port_forward.rs` (new return type of `connect_and_login`, our
   refusal of the raw tunnel inside), `src/server.rs` (a new test module beside ours).
3. If the `hbb_common` submodule moved, do the same there with `patches/hbb_common.patch`.
4. `cargo check` in `apps/desktop/src-tauri` and `apps/quick/src-tauri`: this shows what the new
   core changed under our glue code (`src/tauri_ui`, the apps) and updates both `Cargo.lock`
   files. 1.5.3 needed one change: `LoginConfigHandler::initialize` takes a login method
   (upstream's "request approval"); Tether passes `""`, the password login.
5. Compare upstream's `[patch.crates-io]` and git dependencies in `vendor/rustdesk/Cargo.toml`
   with the apps' `Cargo.toml` (patches only apply at the workspace root).
6. `git reset` in `vendor/rustdesk` (the cherry-pick leaves everything staged, and
   `make-patches.sh` diffs the working tree against the index), then `scripts/make-patches.sh`.
   Set tag and commit in `scripts/bootstrap.sh`.
7. Tests: `cargo test` in `apps/desktop/src-tauri`, the local end-to-end tests
   (`apps/desktop/e2e`: `disconnect-e2e`, `connect-once-e2e`, `cm-window-e2e`, `keyboard-e2e`),
   and after a change in `src/platform/windows*` or `src/ipc*` the service checks of
   [unattended.md](unattended.md) (install as service, uninstall, update over an installed
   version).
8. After the merge every checkout has to recreate `vendor/rustdesk`: `bootstrap.sh` refuses a
   tree at the old commit. Move the old directory away first; changes that are not in `patches/`
   are lost with it.

A trap in the VM checks: while the app's main window runs with a WebView2 debugging port
(`vm-svc-setup.ps1` starts it that way), the connection window of an incoming session cannot
create its WebView2 (`0x8007139F`: same profile folder, other browser arguments) and the session
ends with "The remote side ended the session". Stop that window (`vm-svc-cli.ps1 -StopUi`)
before connecting. And `vm-svc-setup.ps1` recreates the "Test CDP tech" firewall rule for the old
subnet: run `vm-perf-exec.ps1 -Script fwsubnet.ps1` after it.

What was run for 1.5.3 is listed in the CHANGELOG entry "Upstream RustDesk 1.5.3".

## Review of upstream pull requests (2026-11-11)

All 89 open pull requests and the 42 merged since the 1.5.1 pin were looked at. About 45 touch
only code Tether does not build (Flutter, Sciter, mobile, Linux, HarmonyOS, CI, translations) and
were set aside from their file lists. The rest were read against our tree.

### Taken

| PR | What a Tether user gets |
| --- | --- |
| 16954 | macOS: no crash at connect on Macs where `default-net` 0.14 misread the interface list (reported with about 150 `utun` interfaces). The call runs on every viewer connect with `network.direct = auto`, and release builds abort on a panic. |
| 16858 | A host only dials a relay address it offered itself (remembered for 10 minutes) or its configured one, and relay uuids are checked. Before, anyone who could reach hbbs could make a host connect out to an address of their choice. |
| 16889, 16871, 16908 | Windows "Install as service" and `--uninstall-service` no longer fail where F-Secure DeepGuard blocks the named-pipe handoff or where `C:\Windows\Temp` is a junction; the staging directory is guarded. |
| 16865 | Windows: uninstalling removes the service also when the configuration sync fails first (before: an orphan service could stay). |
| 16755, 16885, 16913 | Windows: the main and service pipes no longer let any local user create further pipe instances, and a client checks who serves the pipe (SYSTEM, or the same user, session and executable). Matters on shared PCs and terminal servers. |
| 16695 | Windows clipboard: the owner marker is written first, so a failed format write is not echoed back by the peer (Excel lost its copy state). |
| 16240 | Pasting many files through the clipboard to a Windows receiver: sizes travel in the file list instead of one blocking request per file. |
| 16974 | libsodium is initialised before first use. |

Side effects of the pin worth knowing:

- **Windows pipes and versions.** A 1.5.3 client refuses a main or service pipe with the old
  access list, so a new app does not talk to a service of an older version that is still
  running. The installer stops the service before it replaces files, so this only shows when
  an old and a new `tether.exe` are run side by side by hand.
- **Request approval** (16784, 16851) is in the core but not offered in Tether's UI; sessions
  log in with the password as before.
- 16778 (trusted devices for direct-IP connections) was already in 1.5.1.

### Maybe later

| PR | What it is | Why not now |
| --- | --- | --- |
| 16917 (and its larger draft 16975) | Host option: type text as real key presses, so it reaches VM consoles and elevated console windows; the gap noted in [unattended.md](unattended.md) | Off by default, no human review yet, reportedly not read in the portable service, and console windows can report a stale layout |
| 16255 | Hardware encoder told the real frame rate instead of 30 | Our bitrate adaptation and benchmarks are tuned against today's behaviour: measure first |
| 16612 | macOS host: key-up for Unicode text, against repeated characters in a Parallels guest | Maintainer asked for changes (the key-up must not carry the string) |
| 16566 | macOS host: click count 1 on single clicks (Telegram menus) | Maintainer sees a regression risk; the fix would be two conditions by hand |
| 14930 | Recording to a slow share freezes the picture | The upstream patch is for the native viewer; our `record_passthrough` writes synchronously too and would need its own writer thread |
| 16961 | mfx-dispatch patch vendored | Only helps cold vcpkg builds when GitHub rate-limits |

### Left out

- **Code Tether does not run:** UDP registration sealing (16902), direct-IP encryption (16503),
  port forward over KCP (16880), upstream's key grab loop (16481, 16388, 16173), the native
  viewer's decoders (16914, 16252), upstream's updater and macOS service (16335, 16921, 14705,
  16879), heartbeat strategies (16696), the IP whitelist (16692).
- **Solved differently in Tether:** modifier swap (14681), proxy fallback (16141), security
  hardening (14674), online status (16017), reconnect timing (16946, 16564, 16445).
- **Not ready upstream:** 13944 (never reproduced), 16952 (draft, can discard a new image),
  16499 (needs an unmerged hwcodec change).

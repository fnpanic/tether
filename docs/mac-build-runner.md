# Mac build runner (builds while the Mac is locked)

A small job runner on the build Mac (user `thomasschend`, Apple silicon, macOS 15.7) that runs
**signed** build jobs sent from the PC over Tether file transfer. Nothing is typed on the Mac, the
GUI is never used, the screen can stay locked, and nothing needs sudo. Several senders (Tether,
MirrorMagic, other projects or Claude sessions) share it, each in its own namespace.

- Mac side: [`tools/macbuild/mac/runner.py`](../tools/macbuild/mac/runner.py) (user LaunchAgent
  `local.build-runner`), installer [`tools/macbuild/mac/install.sh`](../tools/macbuild/mac/install.sh).
- PC side: [`tools/macbuild/macbuild.py`](../tools/macbuild/macbuild.py) (Python 3, stdlib only,
  `ssh-keygen` and the Tether file-transfer helpers `C:\vms\night\put.sh` / `get.sh`).
- Tether's own Mac build as a job: [`tools/macbuild/jobs/tether/`](../tools/macbuild/jobs/tether/).
- Tests: [`tools/macbuild/test_macbuild.py`](../tools/macbuild/test_macbuild.py).

## Why this works while the screen is locked

| Question | Answer | Evidence |
| --- | --- | --- |
| Do processes keep running when the screen locks? | Yes. Locking only covers the session; the login session, its LaunchAgents (`gui/<uid>` domain) and all apps keep running. They stop on **logout**, **sleep** or reboot (until the user logs in again). | The MirrorMagic runner (same LaunchAgent pattern, `~/mirrormagic-build`) runs builds without a Terminal; GitHub's and GitLab's self-hosted macOS runners are user LaunchAgents with the same limits ([Pants: macOS runners](https://pantsbuild.org/dev/docs/contributions/releases/github-actions-macos-arm64-runners), [Scaleway guide](https://scaleway.com/en/docs/tutorials/install-github-actions-runner-mac)). |
| Does Tether file transfer work while locked? | Yes: it runs inside the Tether host app, which keeps running behind the lock screen. Uploads are written as `<name>.download` and renamed when complete (`vendor/rustdesk/libs/base/src/fs.rs`), so the runner never sees half a file. | Code. File transfer is part of the host's session handling, not of the screen; e2e steps 3 and 10 confirm it on the Mac. |
| Typing into Terminal while locked? | No: input goes to the lock screen. That is why the runner exists. | |
| `codesign` with the login keychain? | Avoided. The login keychain is unlocked at login and usually stays so while locked, but nothing should depend on it. The Tether job imports the p12 into a **temporary keychain** that it creates and unlocks itself (`security create-keychain/unlock-keychain -p <random>`), and `set-key-partition-list -S apple-tool:,apple:` lets codesign use the key without a prompt. Both work without a GUI; `errSecInternalComponent` is what a locked keychain gives. | [Apple forums 735270](https://developer.apple.com/forums/thread/735270), [712005](https://developer.apple.com/forums/thread/712005), [685967](https://developer.apple.com/forums/thread/685967); same commands as the existing `run.sh`. |
| `security unlock-keychain` of the login keychain? | Not used: it needs the user's password, and storing it would be a much bigger secret than the p12. | |
| Notarization? | Not used by Tether (self-signed). `xcrun notarytool` runs headless with an App Store Connect API key (`--key/--key-id/--issuer`) passed as a job secret; a `--keychain-profile` lives in the login keychain (works only while that keychain is unlocked). | |
| cargo / Tauri / Xcode CLI builds? | Pure command-line tools: unaffected by the lock. **Exception:** Tauri's dmg step runs an AppleScript to Finder (window layout) that needs Automation consent and fails without someone to click. With `CI=true` it passes `--skip-jenkins` and skips it (the dmg then has no custom layout). The Tether job sets `CI=true`. | [tauri-bundler dmg/mod.rs](https://docs.rs/crate/tauri-bundler/2.9.2/source/src/bundle/macos/dmg/mod.rs), [tauri#3055](https://github.com/tauri-apps/tauri/issues/3055), [tauri#11799](https://github.com/tauri-apps/tauri/pull/11799); docs/macos.md "Not verified without a Mac". |
| Launching / replacing a running app? | `open` works from a LaunchAgent. Replacing `/Applications/Tether.app` can hit macOS **App Management** (Ventura+: changing another developer's app bundle needs that permission for the process doing it; a prompt nobody can answer while locked). AppleScript `quit app` would need **Automation** consent. And the running Tether *is* the remote access, so a failed swap while nobody is at the Mac is the worst case. Therefore the swap is **deferred until the screen is unlocked**, uses SIGTERM instead of AppleScript, and renames instead of deleting, so a refusal changes nothing. | [mjtsai: Ventura App Management](https://mjtsai.com/blog/2022/11/07/ventura-app-management/), [Jamf thread](https://community.jamf.com/t5/jamf-pro/ventura-quot-app-management-quot/m-p/304969). |
| Is the screen locked? | `ioreg -n Root -d1 -a` → `IOConsoleUsers[uid].CGSSessionScreenIsLocked` (present only while locked), `kCGSSessionOnConsoleKey` (false after fast user switching). No sudo. Unknown counts as locked. | [stephen-fox gist](https://gist.github.com/stephen-fox/ec873f9012e203e6a947320a0aa21f6a), [brunerd gist](https://gist.github.com/brunerd/5ba75737b2093e88eb30aa614584a21b), [Jamf thread](https://community.jamf.com/t5/jamf-pro/run-policy-only-when-the-user-session-is-not-locked/m-p/167660). Checked by e2e step 3. |
| Sleep | A sleeping Mac runs nothing (and Tether is unreachable). While a job runs the runner holds `caffeinate -i -s -w <pid>` (no idle sleep; `-s` no system sleep on AC power); the display may sleep. Without sudo, `pmset` can only be read (`pmset -g assertions` shows the assertion); lid-closed sleep without an external display cannot be prevented. Between jobs the Mac's own energy settings apply: keep it on power, lid open, "Prevent automatic sleeping when the display is off" on (System Settings → Battery/Energy, a user setting). | `man caffeinate`, `man pmset`. |
| App Nap / throttling | App Nap applies to apps, not to launchd jobs. A LaunchAgent without `ProcessType` gets light CPU/IO limits; the runner sets `ProcessType = Interactive` (no limits). | `man launchd.plist`; [GitLab runner #29089](https://gitlab.com/gitlab-org/gitlab-runner/-/issues/29089) (slow builds under launchd until `Interactive`). |
| No sudo | LaunchAgent in `~/Library/LaunchAgents`, `launchctl bootstrap gui/<uid>`, `/usr/bin/python3` (Command Line Tools 3.9.6, present), `/usr/bin/ssh-keygen` (OpenSSH 9.x on macOS 15), `sandbox-exec`, `caffeinate`, `ioreg`: all user-level. macOS shows "Background Items Added" once; turning the item off in System Settings → General → Login Items stops the runner. | |
| TCC | The runner works in `~/build-runner`, which is not a protected folder (Desktop/Documents/Downloads would prompt). | |

## Architecture

```
PC (any Claude session)                        Mac (user thomasschend, LaunchAgent local.build-runner)
macbuild.py submit                              ~/build-runner/
  manifest.json + work/ + secrets/  -> bundle.tgz   inbox/<user>/<id>.job       <- put.sh (Tether file transfer)
  ssh-keygen -Y sign (ed25519)      -> bundle.sig   users/<user>/jobs/<id>/     status.json job.log artifacts/ work/ secrets/
  tar(bundle.tgz, bundle.sig)       =  <id>.job     users/<user>/cache/         persistent between jobs (incremental builds)
  put.sh -> inbox/<user>/<id>.job                   users/<user>/rejected/      <file>.json: why a file was refused
macbuild.py status/log/wait/fetch <- get.sh         keys/allowed_signers        who may send (per user, per namespace)
                                                    config.json state/ runner.py runner.log
```

The runner polls every 3 s (a long-running process: launchd `QueueDirectories`, which MirrorMagic
uses, cannot watch per-user subfolders). Per pass it:

1. **Ingests** each `inbox/<user>/<id>.job|.cancel` of a registered user (ignoring `*.download`):
   the file must be a tar of exactly `bundle.tgz` + `bundle.sig`; the signature must verify with
   `ssh-keygen -Y verify -I <user> -n macbuild-job` (`macbuild-admin` for `admin`); only then is the
   bundle read: only `manifest.json`, `work/**`, `secrets/**`, regular files and folders, no `..`.
   The manifest must name the same user and id as the path, be inside its validity window
   (`created` ≤ now + 5 min, `expires` > now, at most 48 h apart) and its id must be unused
   (replay list in `state/seen-<user>.json`, kept until expiry). The inbox file is deleted either way;
   refusals are written to `users/<user>/rejected/<file>.json`.
2. **Schedules** fairly: at most `max_parallel` jobs (default **1**), at most one per user; the
   waiting user whose last job started longest ago goes first, FIFO within a user.
3. **Runs** `cmd` in `work/` in its own process group (session), stdout+stderr → `job.log`, under
   `caffeinate`, and (default) under `sandbox-exec` (see below). Environment: a clean `PATH`
   (Homebrew first), `HOME`, `JOB_ID`, `JOB_USER`, `JOB_DIR`, `JOB_WORK`, `JOB_SECRETS`, `JOB_CACHE`,
   `JOB_ARTIFACTS`, `JOB_ON_UNLOCK`, `JOB_RUNNER`, `JOB_PYTHON`, plus the manifest's `env`.
4. **Supervises**: timeout (manifest `timeout`, capped at 6 h) → SIGTERM to the group, SIGKILL after
   30 s; disk quota of `users/<user>` (default 60 GB) before the start and every 10 min; cancel files.
5. **Finishes**: kills what is left of the process group, deletes `secrets/`, copies `artifacts`
   globs (relative to `work/`; folders such as `.app` are zipped with `ditto`) to `artifacts/`,
   writes `status.json`: `state` (`queued`, `running`, `ok`, `failed`, `cancelled`), `exit_code`,
   `reason`, `queued_at`, `started_at`, `ended_at`, `artifacts`, `on_unlock`.
6. **Deferred step**: a successful job that wrote `$JOB_ON_UNLOCK` (`on-unlock.sh`) gets
   `on_unlock: pending`; the runner starts it once the user's session is unlocked (output
   `on-unlock.log`, 15 min limit, then `ok` / `failed: exit N`). A cancel for the job id drops it.
7. **Housekeeping**: finished jobs and refusal notes older than 7 days are deleted. After a runner
   restart, jobs that were running are marked `failed: runner restarted` and their secrets deleted;
   their process group is killed only if its leader (`ps -o lstart`) started within 5 s of the
   job, so a pid reused after a reboot or wrap is left alone.
   When `runner.py` was replaced (update job) the runner exits as soon as nothing runs, and launchd
   (`KeepAlive`) starts the new version.

Settings live in `~/build-runner/config.json` (`max_parallel`, `retention_days`, `max_timeout`,
`default_quota_gb`, `users.<name>.quota_gb`, `users.<name>.sandbox`, …; defaults in `runner.py`).

## Security model

A job is arbitrary code running as `thomasschend`. The design goal: **only registered senders can
run code, a sender cannot act as another, a captured job cannot be run twice or late, and secrets
live on the Mac only while their job runs.**

| Threat | Defence |
| --- | --- |
| Someone who can drop files into the inbox (another Tether technician with file-transfer access to this Mac, a mistake, a stray tool) | Nothing runs without a valid ed25519 signature by a key in `keys/allowed_signers` for the inbox's user. Unsigned, malformed, oversized (1 GB) and unknown-user files are deleted/ignored. |
| Sender A submits as B, or moves A's signed job into B's inbox | Verification uses `-I <inbox user>`; the manifest's `user`/`id` must match the path. |
| User key used for admin actions | Admin keys are listed with `namespaces="macbuild-admin"`, user keys with `namespaces="macbuild-job"`; admin jobs must be signed in the admin namespace. |
| Replay of an old job file, or a captured file run days later | Unique id per job, remembered until the manifest expires; jobs expire (default 24 h, at most 48 h). |
| Path tricks in the payload | Only regular files/folders below `work/` and `secrets/`, no absolute paths, no `..`, no links (checked before extraction). |
| Secrets (signing p12, API keys) | Sent per job inside the signed bundle (`--secret`), stored in the job's `secrets/` (0700), deleted when the job ends, whatever the outcome; on runner restart; and on cancel. The client deletes its local `.job` after upload. With `max_parallel = 1` no other job runs while they exist. Transport: Tether's encrypted session. |
| One job reading another user's files | `sandbox-exec` profile per job: everything allowed except reading/writing other users' `users/<u>` and `inbox/<u>`, and writing `keys/`, `state/`, `runner.py`, `config.json`, `~/Library/LaunchAgents`. |
| Runaway job | Timeout, process-group kill, disk quota, one job at a time. |

**Limits (be honest about them):**

- All jobs run as the same macOS user. The sandbox stops accidents and careless scripts, not a
  deliberately malicious authorised sender: a job can start processes outside its sandbox through
  launchd or LaunchServices (`launchctl submit`, `open`), read the user's other files, or persist
  via shell profiles. **Register only senders you would let type in your Terminal.**
- All Claude sessions on the PC run as the same Windows user and can read each other's keys under
  `%USERPROFILE%\.macbuild` (including the admin key). Per-user keys separate namespaces, quotas and
  accidents, not mutually hostile PC users.
- Anyone with Tether file-transfer access to the Mac can already overwrite any file in the home
  folder (including `allowed_signers` and `runner.py`); the signature protects the runner's entry
  point, not the Mac against Tether access. Tether access is the outer trust boundary.
- A job's `env` and command line are visible to `ps` of the same user while it runs; pass secrets
  as files. Overwriting deleted files is pointless on APFS/SSD; secrets are unlinked.
- `max_parallel > 1` lets jobs of two users run side by side: their secrets then coexist (still
  separated by the sandbox).

## Install (once, at the Mac, unlocked)

The runner is installed by one command in Terminal on the Mac. Prepare it from the PC:

```bash
# PC (Git Bash): keys for the admin and the senders, if not there yet (%USERPROFILE%\.macbuild\<user>)
python tools/macbuild/macbuild.py keygen --user admin
python tools/macbuild/macbuild.py keygen --user tether
# setup bundle: runner, installer, one <user>.pub per sender (admin.pub is required)
S=$(mktemp -d); cp tools/macbuild/mac/runner.py tools/macbuild/mac/install.sh "$S"/
for u in admin tether mirrormagic; do cp ~/.macbuild/$u/id_ed25519.pub "$S/$u.pub"; done
tar --owner=0 --group=0 -czf /c/vms/macbuild-setup/build-runner-setup.tgz -C "$S" .
bash /c/vms/night/put.sh 'C:\vms\macbuild-setup\build-runner-setup.tgz' /Users/thomasschend/build-runner-setup.tgz
```

Then, **at the Mac** (unlocked), in Terminal:

```bash
mkdir -p ~/build-runner-setup && tar -xzf ~/build-runner-setup.tgz -C ~/build-runner-setup && bash ~/build-runner-setup/install.sh
```

It copies the runner to `~/build-runner`, registers every `<user>.pub` (`admin` without sandbox),
writes `~/Library/LaunchAgents/local.build-runner.plist` and starts it (`launchctl bootstrap
gui/$(id -u)`); the output ends with `state = running` and `build runner installed: … (users: admin
mirrormagic tether)`. macOS reports "Background Items Added". Afterwards updates arrive as admin
jobs (`macbuild.py update-runner`); only a change of the plist needs the command again.

Uninstall: `launchctl bootout gui/$(id -u)/local.build-runner; rm ~/Library/LaunchAgents/local.build-runner.plist; rm -rf ~/build-runner`.

## Using it from another Claude session

Run in Git Bash from any checkout of this repository (the client only needs Python 3 and
`ssh-keygen`; Git for Windows has both). Keep transfers short: the client takes a PC-wide lock and
waits while another Tether file-transfer viewer runs, but each `put`/`get` still opens a fresh
Tether session (10–20 s), so poll every minute or two, not every second.

1. **Key** (once per project): `python tools/macbuild/macbuild.py keygen --user <name>` (name: a–z,
   0–9, `-`). Prints the public key; the private key stays in `%USERPROFILE%\.macbuild\<name>\`.
2. **Register** (once, by whoever holds the admin key; tell the user if that is not you):
   `python tools/macbuild/macbuild.py register --user <name> --pub ~/.macbuild/<name>/id_ed25519.pub [--quota-gb 80] [--no-sandbox]`.
   This is an admin job; `wait --user admin <id>` shows when it ran. `--replace` drops the user's
   older keys (rotation). Running it again changes quota/sandbox.
3. **Submit**: put everything the job needs in one folder (scripts, a source `.tgz`; no symlinks:
   pack trees with links into a tarball), then
   ```bash
   python tools/macbuild/macbuild.py submit --user <name> --work <folder> \
     --secret 'C:\path\cert.p12=cert.p12' --artifact 'out/*.dmg' --timeout 7200 -- bash build.sh
   ```
   prints the job id. The command runs in `work/` (= your folder). Read secrets from
   `$JOB_SECRETS/<name>`, keep caches in `$JOB_CACHE`, write results where `--artifact` globs find
   them (relative to `work/`). Write `$JOB_ON_UNLOCK` (a bash script) for a step that must wait
   until someone unlocks the Mac. Use interpreters (`bash x.sh`), not `./x.sh`, files lose their
   exec bit on Windows.
4. **Poll**: `python tools/macbuild/macbuild.py wait --user <name> <id> --interval 120` (prints the
   state each poll; at the end the log tail and `status.json`; exit code 0 = ok), or once with
   `status`, `log --tail 50`, `log --file on-unlock.log`. `state: rejected` comes with the reason.
5. **Fetch**: `python tools/macbuild/macbuild.py fetch --user <name> <id> <artifact-name> --out <file>`
   (names are in `status.json` → `artifacts`).
6. **Cancel**: `python tools/macbuild/macbuild.py cancel --user <name> <id>` (queued, running, or a
   pending on-unlock step).

Example, smallest job:

```bash
python tools/macbuild/macbuild.py submit --user tether -- sh -c 'sw_vers; "$JOB_PYTHON" "$JOB_RUNNER" lockstate'
python tools/macbuild/macbuild.py wait --user tether <id> --interval 30
```

MirrorMagic's existing `build-on-mac.sh` fits as is: `--work` with `build-on-mac.sh` and `src.tgz`,
secrets `cs.p12`/`p12.pass`, `-- bash build-on-mac.sh` after pointing its `B=` at `$JOB_CACHE` and
its p12 paths at `$JOB_SECRETS` (user `mirrormagic` is registered at install).

## Tether's Mac build

```bash
bash tools/macbuild/jobs/tether/submit.sh            # HEAD of this checkout; --no-swap: build only
python tools/macbuild/macbuild.py wait --user tether <id> --interval 300
```

`submit.sh` builds both UIs on the PC under `buildlock.sh` (npm is blocked on the Mac), packs
`git archive HEAD` + `prebuilt/` as `src.tgz`, and sends `build.sh` + `swap.sh` with `keys.env`
(`C:\vms\mac59\keys.env`) and `cs.p12`/`p12.pass` (`C:\vms\macsign`) as secrets. On the Mac
[`build.sh`](../tools/macbuild/jobs/tether/build.sh) is the former `~/tether-build/run.sh` with
`$JOB_CACHE` as its folder (incremental: `src/`, `.tools/`, Cargo's `target/` stay; the first build
is a cold one): temporary keychain, codesign probe, `scripts/build-macos.sh` with `CI=true`,
designated-requirement check against `/Applications/Tether.app`, TetherQS check, keychain deleted,
artifacts `out/*` (dmg, updater `.app.tar.gz`, TetherQS zip). It copies the app into `work/` and
requests the swap through `$JOB_ON_UNLOCK`; [`swap.sh`](../tools/macbuild/jobs/tether/swap.sh) runs
after the next unlock: re-checks the signature and requirement, stages the new app in
`/Applications/.Tether.app.new`, renames the old one away (if macOS refuses, nothing changed and
the log says so), stops the old process (SIGTERM, no AppleScript), opens the new one, waits for its
rendezvous start and 30 s more, else puts the old one back. If the run was killed, the next run
deletes the stale temporary keychain first.

## End-to-end test (on the real Mac, after installing)

Run from Git Bash in the repository on the PC; `M=python tools/macbuild/macbuild.py`. Each
step lists what must be seen.

1. **Runner up** (at the Mac, right after the install command): output shows `state = running`;
   `tail ~/build-runner/runner.log` shows `runner started`.
2. **Hello, unlocked**: `$M submit --user tether -- sh -c 'echo hello; id -un; sw_vers -productVersion; "$JOB_PYTHON" "$JOB_RUNNER" lockstate; pmset -g assertions | grep -i caffeinate'`
   then `$M wait --user tether <id> --interval 20` → `ok`, log: `hello`, `thomasschend`, `15.7.4`,
   `unlocked`, a `caffeinate` assertion line.
3. **Locked**: lock the Mac (Ctrl-Cmd-Q) and leave it. Repeat step 2 → `ok`, log says `locked`.
   (If it says `unknown`, lock detection does not work on this macOS: deferred swaps would never
   run; report it.)
4. **Signature refused**: `$M submit --user tether --pack-only -- true` prints a path; flip a byte
   (`python -c "import sys;p=sys.argv[1];b=bytearray(open(p,'rb').read());b[600]^=1;open(p,'wb').write(b)" <path>`),
   upload it: `bash /c/vms/night/put.sh '<path as C:\…>' /Users/thomasschend/build-runner/inbox/tether/<id>.job`;
   `$M status --user tether <id>` → `rejected`, reason `signature not valid` or `bad bundle`.
5. **Replay refused**: pack another job with `--pack-only`, copy the `.job` somewhere, upload it
   (runs, `ok`), upload the copy again under the same name → `status` stays `ok`; the runner log
   / `users/tether/rejected/<id>.job.json` says `replay` (check with
   `bash /c/vms/night/get.sh /Users/thomasschend/build-runner/users/tether/rejected/<id>.job.json 'C:\vms\x\r.json'`).
6. **Wrong sender**: `MACBUILD_DIR` unchanged, pack as `mirrormagic` (`$M submit --user mirrormagic --pack-only -- true`)
   and upload the file into `inbox/tether/` → rejected for tether (`signature not valid for tether`).
7. **Sandbox**: `$M submit --user tether -- sh -c 'ls ~/build-runner/users/mirrormagic; touch ~/build-runner/keys/x; echo done'`
   → log shows `Operation not permitted` twice, then `done`.
8. **Timeout and cancel**: `$M submit --user tether --timeout 30 -- sleep 600` → `failed`, reason
   `timeout`, about 30–60 s after start. `$M submit --user tether -- sleep 600`, then
   `$M cancel --user tether <id>` → `cancelled`.
9. **Fairness**: submit three `sleep 60` jobs as `tether`, then one as `mirrormagic` → the
   mirrormagic job starts second (compare `started_at`).
10. **Tether build while locked** (the main test): lock the Mac, `bash tools/macbuild/jobs/tether/submit.sh`,
    `$M wait --user tether <id> --interval 300` → `ok` (cold first build ~1–2 h), log contains
    `codesign test ok`, `requirement new: … = old`, `temporary keychain deleted`,
    `BUILD OK; Tether.app will be replaced when the screen is unlocked`; status `on_unlock: pending`.
    Secrets gone: `$M submit --user tether -- sh -c 'ls -la "$JOB_CACHE/sign"; ls ../../*/secrets 2>&1; security list-keychains -d user'`
    → no `cs.p12`, no `tmp.keychain-db`, only this job's own (empty) `secrets` folder, search list without the temp keychain.
    The Mac's Tether stayed reachable the whole time (the file transfers worked).
11. **Swap on unlock**: unlock the Mac. Within seconds `$M log --user tether <id> --file on-unlock.log`
    shows `SWAP OK: new build running`, status `on_unlock: ok`; Tether reconnects (same
    permissions, same signature). If it says `cannot rename … App Management`: allow `python3` (or
    the "bash" entry) in System Settings → Privacy & Security → App Management and cancel/resubmit.
12. **Artifacts**: `$M fetch --user tether <id> Tether_<version>_aarch64.dmg --out 'C:\vms\x\Tether.dmg'`.
13. **Update path**: `$M update-runner`, `$M wait --user admin <id>` → `ok`; `runner.log` shows
    `runner.py changed: restarting` and a new `runner started`.
14. **Retention** (optional): set `retention_days` to 0 in `config.json` with an admin job; old jobs vanish within 10 min.

## Troubleshooting

- `status` says `unknown`: the job was not picked up. The runner is not running (Login Items
  switched off, logged out, Mac asleep) or the upload went elsewhere. `runner.log` and
  `launchctl print gui/$(id -u)/local.build-runner` on the Mac tell.
- `failed`, reason `sandbox-exec …` or tools failing with `sandbox_apply: Operation not permitted`
  (a tool that sandboxes itself, e.g. SwiftPM, Homebrew post-install): re-register the user with
  `--no-sandbox`.
- Uploads: give the full target file path; existing files are never overwritten (the transfer
  waits for a prompt), so every job has a new id; the client downloads into fresh folders.

## Tests

```bash
wsl -d Ubuntu-22.04 --cd "$(pwd)" -- python3 -m unittest discover -s tools/macbuild -v
```

POSIX (process groups) and `ssh-keygen` are needed, so they run in WSL (or on a Mac/Linux); the
macOS-only tools are not called (sandbox off, lock state stubbed). They cover manifest validation,
signatures (tampered, wrong key, moved between inboxes, admin namespace), replay, expiry, unsafe
bundle entries, partial uploads, fair scheduling with one job at a time, timeouts killing the whole
process group, cancel, disk quota, retention, secret deletion, artifacts, the on-unlock step,
restart recovery, registration through an admin job, and the CLI end to end with a fake transfer.

## Checked on the Mac (2026-11-07)

Installed with the one-time command; then from the PC, Tether file transfer only:
hello job while the screen was locked (`locked`, caffeinate assertion), tampered job rejected
(`signature not valid … incorrect signature`), job filed in another user's inbox rejected,
sandbox (`Operation not permitted` for another user's folder and `keys/`), timeout (failed after
33 s, `timeout`), cancel of a running job (`cancelled`). Tether 0.5.22 for macOS was then built as a
job (job 20261107T123043Z-f9ccb9560f99bd58, about 11 minutes): `BUILD OK`, temporary keychain
deleted, app swapped (`on_unlock: ok`), artifacts fetched with `macbuild.py fetch`.
Not covered yet: replay (e2e step 5), fairness across users (step 9). The runner does not run
while the Mac sleeps (lid closed, no external display); a 12 MB job upload failed twice while
the Mac was asleep or offline and succeeded once it was awake.

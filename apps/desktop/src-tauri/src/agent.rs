//! The health monitoring agent, installed, updated and removed by Tether's Windows service
//! (policy features `health.agent` and `health.enrol-grant`, docs/policy.md).
//!
//! Who acts: only the `--server` process running as LocalSystem on Windows ([`tick`] after each
//! policy check, [`request`] for a technician in a session). A user-mode `--server`, the UI, macOS
//! and TetherQS ignore the features. `manual` (the default) does nothing at all: no file, no
//! request, no process.
//!
//! What runs: the `monitor/windows-x86_64` artifact of an admin-signed, unexpired, not halted
//! `monitor.json` at or above this machine's serial floor (`monitor-<channel>.serial` beside the
//! state), verified (size, SHA-256, minisign with the compiled-in updater key, trusted comment
//! with file and version) into `health-agent\` (only SYSTEM and Administrators can write; owner and
//! reparse point checked), opened with a handle that denies writing and deleting, verified again
//! through that handle and run while the handle is held, so the bytes verified are the bytes
//! executed. Besides it only the installed agent in its program folder (`identity`, `enrol`,
//! `uninstall`, `--version`). Every argument list is a constant; the grant and the link proof go
//! over stdin only and are never stored or logged.
//!
//! State `%ProgramData%\<app>\health-agent.json` ([`State`]): the practice's removal sticks
//! (`removed-locally`, from the agent's `RemovedAt` registry value or its service vanishing), an
//! agent installed by hand (service, no state) is only touched by `remove` and a technician's
//! remove, failures back off 1 h, 6 h, 24 h, re-enrolment at most once a day, the 48 h floor timer:
//! all persisted, so a restart changes nothing.
#![cfg_attr(not(windows), allow(dead_code))]

use std::{
    cmp::Ordering,
    path::{Path, PathBuf},
    time::Duration,
};

use hbb_common::log;
use serde::{Deserialize, Serialize};
use tether_trust::{
    release::{self, Candidate},
    status,
    version::{below_minimum, compare},
    VerifyingKey,
};

pub use crate::agent_elevated::{Action, AgentAnswer};
use crate::agent_elevated::{active, download_time, parse_identity, tidy, STAGED_EXE};

const STATE_FILE: &str = "health-agent.json";
/// What Settings shows on this computer, readable by its users (the state folder is not): only
/// [`public_answer`]'s fields.
const PUBLIC_FILE: &str = "health-agent-status.json";
const DOWNLOAD_DIR: &str = "health-agent";
/// Agents before this have no `uninstall --by-tether` (installed by hand only).
const FIRST_TETHER_AWARE: &str = "0.2.0";
const BACKOFF: [u64; 3] = [3600, 6 * 3600, 86_400];
const FLOOR_GRACE: u64 = 48 * 3600;
const INDEX_EVERY: u64 = 6 * 3600;
const ENROL_EVERY: u64 = 86_400;
const SHORT: Duration = Duration::from_secs(60);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(180);
const UPGRADE_TIMEOUT: Duration = Duration::from_secs(600);
const NEEDS_SERVICE: &str = "needs Tether's service";


// ----------------------------------------------------------------------------- state

/// `health-agent.json`. Never holds the grant or the proof (only the grant's id).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct State {
    /// `installed`, `removed-locally`, `removed-by-tether`, `failed`.
    pub state: String,
    /// `policy` or `session`.
    pub by: String,
    pub version: String,
    pub at: u64,
    pub grant_id: String,
    pub failures: u32,
    pub next_try: u64,
    /// The agent Tether installed is there (its service vanishing is a local removal).
    pub ours: bool,
    /// Why the policy does not install: `removed-locally` (the practice) or `session` (a
    /// technician removed it); cleared by a successful install.
    pub hold: String,
    pub detail: String,
    /// Last enrolment (once a day for `not-enrolled`).
    pub enrol_at: u64,
    /// Last minimum-version check (index read at most every 6 hours).
    pub checked_at: u64,
    /// Since when the installed agent is below the index's `minimumVersion` (0: not below).
    pub below_since: u64,
    /// Saved before the agent's `install` runs, cleared with its outcome: set after a restart means
    /// Tether stopped while it ran ([`Manager::adopt`]).
    pub pending_install: Option<Pending>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Pending {
    pub at: u64,
    /// `policy` or `session`.
    pub by: String,
}

/// A pending install older than this (the install's own limit and a margin) did not finish.
const PENDING_MAX: u64 = INSTALL_TIMEOUT.as_secs() + 60;
/// After this (past [`PENDING_MAX`]) a mark no longer adopts an agent that appeared.
const ADOPT_GRACE: u64 = 86_400;
/// The answer while an install Tether started before it stopped may still run.
const MAY_STILL_RUN: &str = "an install Tether started before it stopped may still be running; ask again in a few minutes";

// ----------------------------------------------------------------------------- the machine

/// A finished process.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Ran {
    pub code: i32,
    pub out: String,
    pub err: String,
}

/// Everything the manager does to the machine and the network: the process runner, the SCM and
/// the agent's files and registry value, the folders, HTTP. Real on Windows, fakes in the tests.
pub(crate) trait Sys {
    /// This process is Tether's service `--server` running as LocalSystem ([`service_gate`]).
    fn service_process(&self) -> bool;
    /// This host's ID and the 32-byte seed of the key this process registers with the rendezvous
    /// server (the link proof is signed with it).
    fn own_identity(&self) -> Option<(String, [u8; 32])>;
    /// The agent's service exists (in any state).
    fn service_exists(&self) -> Result<bool, String>;
    /// Installed supervisor (`--version`) and worker (`current.json`) versions.
    fn versions(&self) -> (Option<String>, Option<String>);
    /// The agent's `not-enrolled` file is there.
    fn not_enrolled(&self) -> bool;
    /// The practice's removal (`RemovedAt`): `Some(time)`, `Some(0)` when unparseable.
    fn removed_at(&self) -> Option<u64>;
    /// Create `dir` for SYSTEM and Administrators only, or refuse it (junction, link, owner).
    fn secure_dir(&self, dir: &Path) -> Result<(), String>;
    /// Read-only check of the state folder (no create, no ACL change): absent is fine; a junction
    /// or link, or an owner other than SYSTEM or Administrators, is refused with a fixed sentence
    /// and at most the owner's SID.
    fn check_dir(&self, dir: &Path) -> Result<(), String>;
    /// `path` is absent, or a plain file owned by SYSTEM or Administrators.
    fn file_trusted(&self, path: &Path) -> bool;
    /// `(relay base, raw)` of `download/<channel>/monitor.json` from every relay that answered.
    fn fetch_index(&self, channel: &str) -> Vec<(String, Vec<u8>)>;
    /// The candidate's verified bytes, every source together within `within`.
    fn download(&self, c: &Candidate, key: &str, within: Duration) -> Result<Vec<u8>, String>;
    /// The file's bytes, read through a handle that denies writing and deleting it while the
    /// returned guard lives (the file can still be executed).
    fn open_locked(&self, path: &Path) -> Result<(Vec<u8>, Box<dyn std::any::Any>), String>;
    /// Start `exe` directly (no shell) with these arguments, `stdin` as one line, and wait.
    fn run(&self, exe: &Path, args: &[&str], stdin: Option<&str>, timeout: Duration) -> Result<Ran, String>;
    /// The installed agent's program.
    fn installed_exe(&self) -> PathBuf;
    /// Unix seconds now (the pending mark is stamped when the install child starts, after the
    /// download that may take minutes).
    fn clock(&self) -> u64 {
        now()
    }
    /// Writes `data` to `path` (in the state folder) readable by this computer's users, writable
    /// only by SYSTEM and Administrators.
    fn write_public(&self, path: &Path, data: &[u8]) -> Result<(), String>;
}

/// The public copy of the state: whether Tether's agent is there, by whom, when; nothing else.
fn public_answer(st: &State) -> AgentAnswer {
    let state = match st.state.as_str() {
        _ if st.ours => "installed",
        "" => "not-installed",
        s => s,
    };
    AgentAnswer { state: state.into(), version: st.version.clone(), installed_by: st.by.clone(), detail: String::new(), at: st.at }
}

/// Settings' line from the public copy: only a known state and the three plain fields, no detail.
fn read_public(raw: &[u8]) -> Option<AgentAnswer> {
    let a: AgentAnswer = serde_json::from_slice(raw).ok().filter(|_| raw.len() <= 4096)?;
    let ok = |s: &str, n: usize| s.len() <= n && s.chars().all(|c| c.is_ascii_alphanumeric() || ".-+".contains(c));
    (matches!(a.state.as_str(), "installed" | "not-installed" | "removed-locally" | "removed-by-tether" | "failed") && ok(&a.version, 32) && matches!(a.installed_by.as_str(), "" | "policy" | "session")).then(|| AgentAnswer { detail: String::new(), ..a })
}

/// What the policy and the build say (no Debug: it holds the grant).
#[derive(Clone, Default)]
pub(crate) struct Inputs {
    pub mode: String,
    pub grant: String,
    pub channel: String,
    pub rollout: String,
    pub uuid: String,
    pub admin: Vec<VerifyingKey>,
    pub updater_key: String,
    /// The state folder.
    pub dir: PathBuf,
}


/// The one place that decides whether the manager acts here: Tether's `--server` process (not the
/// connection manager or the UI, which run the policy loop too) running as LocalSystem.
fn service_gate(first_arg: Option<&str>, local_system: bool) -> bool {
    first_arg == Some("--server") && local_system
}



fn tidied(a: AgentAnswer) -> AgentAnswer {
    AgentAnswer { detail: tidy(&a.detail), ..a }
}

/// The action on the agent running in this process, by name. Set by [`Running::start`], cleared
/// when the [`Running`] guard drops, also when the action panics.
static RUNNING: std::sync::Mutex<Option<&'static str>> = std::sync::Mutex::new(None);

fn running() -> std::sync::MutexGuard<'static, Option<&'static str>> {
    RUNNING.lock().unwrap_or_else(|e| e.into_inner())
}

struct Running;

/// The slot's name while a policy check runs no agent process.
const TICK: &str = "checking the policy for the agent";

/// `status` while an action runs: `busy` naming it, except while only a policy check holds the
/// slot (then the saved state answers).
fn busy_now(slot: Option<&'static str>) -> Option<AgentAnswer> {
    slot.filter(|w| *w != TICK).map(busy)
}

/// The policy check runs the agent now (`status` answers `busy` naming it).
fn child(what: &'static str) {
    let mut r = running();
    if *r == Some(TICK) {
        *r = Some(what);
    }
}

impl Running {
    /// `Err(name)`: that action runs.
    fn start(what: &'static str) -> Result<Self, &'static str> {
        let mut r = running();
        match *r {
            Some(other) => Err(other),
            None => {
                *r = Some(what);
                Ok(Running)
            }
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        *running() = None;
    }
}

fn busy(what: &str) -> AgentAnswer {
    AgentAnswer::only("busy", &format!("Tether is {what}; ask for the status later"))
}

/// Runs `work` as the one action `what`, on this thread to its end (the core answers `running` to
/// the session when it takes longer, and still gets this outcome for the audit). Meanwhile every
/// other action answers `busy` naming this one.
fn exclusive(what: &'static str, work: impl FnOnce() -> AgentAnswer) -> AgentAnswer {
    match Running::start(what) {
        Ok(_slot) => work(),
        Err(other) => busy(other),
    }
}

fn line_of(r: &Ran) -> String {
    let pick = |s: &str| s.lines().map(str::trim).filter(|l| !l.is_empty()).last().map(str::to_owned);
    let l = if r.code != 0 { pick(&r.err).or_else(|| pick(&r.out)) } else { pick(&r.out).or_else(|| pick(&r.err)) };
    tidy(&l.unwrap_or_else(|| format!("exit code {}", r.code)))
}


pub(crate) struct Manager<'a> {
    pub sys: &'a dyn Sys,
    pub p: Inputs,
}

impl Manager<'_> {
    fn load(&self) -> Option<State> {
        let path = self.p.dir.join(STATE_FILE);
        self.sys.file_trusted(&path).then(|| std::fs::read(&path).ok()).flatten().and_then(|b| serde_json::from_slice(&b).ok())
    }

    /// Files in the (secured) state folder that someone other than SYSTEM or Administrators owns
    /// (put there before Tether reset the folder's ACL; their owner may still change their ACL)
    /// are deleted, so they count as absent and the next write replaces them.
    fn sweep(&self) -> Result<(), String> {
        let ch = &self.p.channel;
        for name in [STATE_FILE.to_owned(), PUBLIC_FILE.to_owned(), format!("monitor-{ch}.json"), format!("monitor-{ch}.serial")] {
            let path = self.p.dir.join(&name);
            if !self.sys.file_trusted(&path) {
                log::warn!("health agent: {name} is not owned by SYSTEM or Administrators: ignored and deleted");
                std::fs::remove_file(&path).map_err(|e| format!("{name}: {e}"))?;
            }
        }
        Ok(())
    }

    fn save(&self, old: &Option<State>, st: &State) -> Result<(), String> {
        if old.as_ref() == Some(st) || (old.is_none() && *st == State::default()) {
            return Ok(());
        }
        let raw = serde_json::to_vec(st).map_err(|e| e.to_string())?;
        crate::policy::write_atomic(&self.p.dir.join(STATE_FILE), &raw).map_err(|e| format!("{STATE_FILE}: {e}"))?;
        // The copy this computer's users may read (Settings): no grant, proof, path or agent line.
        let public = serde_json::to_vec(&public_answer(st)).map_err(|e| e.to_string())?;
        if let Err(e) = self.sys.write_public(&self.p.dir.join(PUBLIC_FILE), &public) {
            log::warn!("health agent: {PUBLIC_FILE}: {e}");
        }
        Ok(())
    }

    fn fail(st: &mut State, why: String, now: u64) {
        log::warn!("health agent: {why}");
        st.state = "failed".into();
        st.detail = why;
        st.at = now;
        st.failures += 1;
        st.next_try = now + BACKOFF[(st.failures as usize - 1).min(BACKOFF.len() - 1)];
    }

    fn succeeded(st: &mut State, state: &str, by: &str, detail: String, now: u64) {
        log::info!("health agent: {state} by {by}: {detail}");
        st.state = state.into();
        st.by = by.into();
        st.detail = detail;
        st.at = now;
        st.failures = 0;
        st.next_try = 0;
    }

    /// The practice removed the agent: its `RemovedAt` value, or the service Tether installed is gone.
    fn notice_removal(&self, st: &mut State, svc: bool, now: u64) {
        let at = match self.sys.removed_at() {
            Some(t) if st.hold != "removed-locally" => if t == 0 { now } else { t },
            Some(_) => return,
            None if st.ours && !svc => now,
            None => return,
        };
        log::warn!("health agent: removed on this computer; Tether does not install it again by policy");
        *st = State { state: "removed-locally".into(), at, hold: "removed-locally".into(), detail: "removed on this computer".into(), version: st.version.clone(), by: st.by.clone(), grant_id: st.grant_id.clone(), ..Default::default() };
    }

    /// An install under way when Tether stopped: the agent's service there means it finished (the
    /// agent is Tether's, installed by whoever asked); none after [`PENDING_MAX`] means it did not.
    fn adopt(st: &mut State, svc: bool, now: u64) {
        let Some(p) = st.pending_install.take() else { return };
        // A mark from long ago claims nothing: the agent there may be one the practice installed since.
        if now >= p.at + PENDING_MAX + ADOPT_GRACE {
            log::info!("health agent: an old install mark was dropped");
        } else if svc {
            Self::succeeded(st, "installed", &p.by, "installed while Tether stopped".into(), now);
            st.ours = true;
            st.hold.clear();
        } else if now >= p.at + PENDING_MAX {
            Self::fail(st, "the install was interrupted (Tether stopped while it ran)".into(), now);
        } else {
            st.pending_install = Some(p); // the install may still be running
        }
    }

    /// One policy check (only modes other than `manual` get here).
    pub fn tick(&self, now: u64) -> Result<(), String> {
        if !active(&self.p.mode) {
            // Only a stale install mark goes (it would otherwise wait for the mode to come back).
            if self.sys.service_process() && self.sys.check_dir(&self.p.dir).is_ok() {
                if let Some(old) = self.load().filter(|s| s.pending_install.as_ref().is_some_and(|p| now >= p.at + PENDING_MAX)) {
                    self.save(&Some(old.clone()), &State { pending_install: None, ..old })?;
                }
            }
            return Ok(());
        }
        if !self.sys.service_process() {
            return Err(NEEDS_SERVICE.into());
        }
        self.sys.secure_dir(&self.p.dir)?;
        self.sweep()?;
        let svc = self.sys.service_exists()?;
        let mut st = self.load().unwrap_or_default();
        Self::adopt(&mut st, svc, now);
        self.notice_removal(&mut st, svc, now);
        if now >= st.next_try && st.pending_install.is_none() {
            match self.p.mode.as_str() {
                "remove" if svc => match self.remove() {
                    Ok(l) => {
                        Self::succeeded(&mut st, "removed-by-tether", "policy", l, now);
                        st.ours = false;
                        st.hold.clear();
                    }
                    Err(e) => Self::fail(&mut st, e, now),
                },
                "install" if !svc && st.hold.is_empty() => {
                    if let Err(e) = self.install(&mut st, "policy", now, true) {
                        Self::fail(&mut st, e, now);
                    }
                }
                "install" | "on-request" if svc && st.ours => self.maintain(&mut st, now),
                // An agent installed by hand, a removal that sticks, nothing to do.
                _ => {}
            }
        }
        // Against the file as it is now (an install saves its pending mark in between).
        self.save(&self.load(), &st)
    }

    /// Re-enrolment (mode `install`) and the minimum version, for the agent Tether installed.
    fn maintain(&self, st: &mut State, now: u64) {
        if self.p.mode == "install" {
            let grant_changed = self.grant(now).is_ok_and(|(_, id)| id != st.grant_id);
            if grant_changed || (self.sys.not_enrolled() && now >= st.enrol_at + ENROL_EVERY) {
                st.enrol_at = now;
                if let Ok((_, id)) = self.grant(now) {
                    st.grant_id = id; // once per grant, whatever the outcome
                }
                let by = st.by.clone();
                match self.enrol(now) {
                    Ok(l) => Self::succeeded(st, "installed", &by, l, now),
                    Err(e) => Self::fail(st, e, now),
                }
                return;
            }
        }
        if now < st.checked_at + INDEX_EVERY {
            return;
        }
        st.checked_at = now;
        let (sup, worker) = self.sys.versions();
        let c = match self.candidate(now, false) {
            Ok(Some(c)) => c,
            Ok(None) => return,
            Err(e) => {
                log::info!("health agent: minimum version check: {e}");
                st.detail = e;
                return;
            }
        };
        let below = [&sup, &worker].iter().any(|v| v.as_deref().is_some_and(|v| below_minimum(v, &c.minimum_version)));
        if !below {
            st.below_since = 0;
            return;
        }
        if st.below_since == 0 {
            st.below_since = now;
        }
        if now < st.below_since + FLOOR_GRACE {
            return;
        }
        let higher = |v: &Option<String>| v.as_deref().is_none_or(|v| compare(&c.version, v) == Some(Ordering::Greater));
        if !higher(&sup) || !higher(&worker) {
            st.detail = format!("below the minimum version, and the index offers no newer version than {}", sup.or(worker).unwrap_or_default());
            return;
        }
        let by = st.by.clone();
        child("upgrading the agent");
        match self.staged(&c, |exe| self.sys.run(exe, &["upgrade"], None, UPGRADE_TIMEOUT)) {
            Ok(r) if r.code == 0 => {
                Self::succeeded(st, "installed", &by, line_of(&r), now);
                st.version = c.version.clone();
                st.below_since = 0;
            }
            Ok(r) => Self::fail(st, line_of(&r), now),
            Err(e) => Self::fail(st, e, now),
        }
    }

    /// The policy's grant, verified with the admin keys: `(token, id)`.
    fn grant(&self, now: u64) -> Result<(String, String), String> {
        crate::agent_elevated::grant(&self.p.grant, &self.p.admin, now)
    }

    /// The agent `monitor.json` offers now. `Ok(None)`: the rollout has not reached this machine
    /// (only when `rollout`, i.e. by policy).
    fn candidate(&self, now: u64, rollout: bool) -> Result<Option<Candidate>, String> {
        if self.p.admin.is_empty() || self.p.updater_key.trim().is_empty() {
            return Err("this build has no update keys".into());
        }
        let ch = &self.p.channel;
        let cache = self.p.dir.join(format!("monitor-{ch}.json"));
        let (c, _) = crate::agent_elevated::candidate(&self.sys.fetch_index(ch), &cache, &self.p.admin, ch, now)?;
        if rollout && !c.eligible(release::bucket(&self.p.uuid, crate::agent_elevated::APP, &c.version), now, &self.p.rollout) {
            log::info!("health agent: the rollout of {} has not reached this computer yet", c.version);
            return Ok(None);
        }
        Ok(Some(c))
    }

    /// Download `c` into the protected folder, lock it, verify it again through the lock, run
    /// `f` with its path while the lock is held, then delete it.
    fn staged<T>(&self, c: &Candidate, f: impl FnOnce(&Path) -> Result<T, String>) -> Result<T, String> {
        let key = self.p.updater_key.as_str();
        let dl = self.p.dir.join(DOWNLOAD_DIR);
        self.sys.secure_dir(&dl)?;
        let bytes = self.sys.download(c, key, download_time(c.size))?;
        c.verify(&bytes, key)?;
        let exe = dl.join(STAGED_EXE);
        let err = |e: std::io::Error| format!("{}: {e}", exe.display());
        match std::fs::remove_file(&exe) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(err(e)),
            _ => {}
        }
        let written = std::fs::OpenOptions::new().write(true).create_new(true).open(&exe).and_then(|mut f| std::io::Write::write_all(&mut f, &bytes).and_then(|()| f.sync_all()));
        let r = written.map_err(err).and_then(|()| {
            let (back, lock) = self.sys.open_locked(&exe)?;
            if !self.sys.file_trusted(&exe) {
                return Err(format!("the downloaded agent ({STAGED_EXE}) has an owner that is not SYSTEM or Administrators: refused"));
            }
            c.verify(&back, key).map_err(|e| format!("the downloaded agent changed before it ran: {e}"))?;
            let r = f(&exe);
            drop(lock);
            r
        });
        let _ = std::fs::remove_file(&exe);
        r
    }

    /// `identity` of `exe`, the link statement signed, the line for `--from-tether -`.
    fn from_tether_line(&self, exe: &Path, token: &str) -> Result<String, String> {
        let (id, seed) = self.sys.own_identity().ok_or("this computer has no Tether ID yet")?;
        if !status::valid_tether_id(&id) {
            return Err("this computer's Tether ID is not one a relay accepts".into());
        }
        let r = self.sys.run(exe, &["identity"], None, SHORT)?;
        if r.code != 0 {
            return Err(line_of(&r));
        }
        let i = parse_identity(&r.out)?;
        let proof = status::sign_agent_link(&seed, &id, &i.uuid, &i.pubkey);
        Ok(serde_json::json!({ "token": token, "tether_id": id, "proof": proof, "channel": self.p.channel }).to_string())
    }

    fn check_channel(&self) -> Result<(), String> {
        match self.p.channel.as_str() {
            "stable" | "test" => Ok(()),
            c => Err(format!("the agent has no channel {c:?} (stable or test)")),
        }
    }

    /// Download, `identity`, sign, `install --from-tether -`. `rollout`: by policy.
    fn install(&self, st: &mut State, by: &str, now: u64, rollout: bool) -> Result<(), String> {
        self.check_channel()?;
        let (token, grant_id) = self.grant(now)?;
        let Some(c) = self.candidate(now, rollout)? else { return Ok(()) };
        child("installing the agent");
        let r = self.staged(&c, |exe| {
            let line = self.from_tether_line(exe, &token)?;
            // When the child starts: PENDING_MAX covers only its own limit, not the download before it.
            st.pending_install = Some(Pending { at: self.sys.clock().max(now), by: by.into() });
            self.save(&None, st)?;
            self.sys.run(exe, &["install", "--from-tether", "-"], Some(&line), INSTALL_TIMEOUT)
        });
        st.pending_install = None;
        let r = r?;
        let mut detail = line_of(&r);
        if r.code != 0 {
            // A failure after the agent made its service (its worker did not come up in time): that
            // agent is Tether's all the same, so re-enrolment, the version floor and removal apply.
            if !self.sys.service_exists().unwrap_or(false) {
                return Err(detail);
            }
            log::warn!("health agent: the install failed after its service was made: {detail}");
            detail = format!("installed, but the agent's install reported a failure: {detail}");
        }
        Self::succeeded(st, "installed", by, detail, now);
        st.version = c.version.clone();
        st.grant_id = grant_id;
        st.ours = true;
        st.hold.clear();
        st.enrol_at = now;
        st.checked_at = now;
        st.below_since = 0;
        Ok(())
    }

    /// The installed agent's `identity`, sign, `enrol --from-tether -`.
    fn enrol(&self, now: u64) -> Result<String, String> {
        self.check_channel()?;
        let (token, _) = self.grant(now)?;
        let exe = self.sys.installed_exe();
        child("enrolling the agent");
        let line = self.from_tether_line(&exe, &token)?;
        let r = self.sys.run(&exe, &["enrol", "--from-tether", "-"], Some(&line), INSTALL_TIMEOUT)?;
        match r.code {
            0 => Ok(line_of(&r)),
            _ => Err(line_of(&r)),
        }
    }

    /// The installed agent's `uninstall --by-tether` (plain `uninstall` for agents before it).
    fn remove(&self) -> Result<String, String> {
        child("removing the agent");
        let (sup, _) = self.sys.versions();
        let args: &[&str] = match sup.as_deref() {
            Some(v) if below_minimum(v, FIRST_TETHER_AWARE) => &["uninstall"],
            _ => &["uninstall", "--by-tether"],
        };
        let r = self.sys.run(&self.sys.installed_exe(), args, None, INSTALL_TIMEOUT)?;
        match r.code {
            0 => Ok(line_of(&r)),
            _ => Err(line_of(&r)),
        }
    }

    /// What the host answers to `status` (reads only).
    pub fn status(&self, now: u64) -> AgentAnswer {
        tidied(self.status_of(now))
    }

    fn status_of(&self, now: u64) -> AgentAnswer {
        if !self.sys.service_process() {
            return AgentAnswer::only("needs-service", NEEDS_SERVICE);
        }
        if !active(&self.p.mode) {
            return AgentAnswer::only("not-allowed", "not allowed by this computer's policy");
        }
        // A folder a user created first (and owns) holds whatever that user wrote: not read.
        if let Err(e) = self.sys.check_dir(&self.p.dir) {
            return AgentAnswer::only("failed", &e);
        }
        let svc = match self.sys.service_exists() {
            Ok(s) => s,
            Err(e) => return AgentAnswer::only("failed", &e),
        };
        let mut st = self.load().unwrap_or_default();
        Self::adopt(&mut st, svc, now); // read only: the next tick or request saves it
        let answer = |state: &str| AgentAnswer { state: state.into(), version: st.version.clone(), installed_by: st.by.clone(), detail: st.detail.clone(), at: st.at };
        if svc {
            let (sup, worker) = self.sys.versions();
            let version = sup.or(worker).unwrap_or_default();
            return match st.ours {
                true => AgentAnswer { version, ..answer("installed") },
                false => AgentAnswer { state: "installed-by-hand".into(), version, ..Default::default() },
            };
        }
        if let Some(t) = self.sys.removed_at().filter(|_| st.hold != "removed-locally") {
            return AgentAnswer { state: "removed-locally".into(), at: if t == 0 { now } else { t }, detail: "removed on this computer".into(), ..Default::default() };
        }
        match st.state.as_str() {
            _ if st.ours || st.hold == "removed-locally" => answer("removed-locally"),
            "failed" | "removed-by-tether" => answer(&st.state),
            _ => AgentAnswer::only("not-installed", ""),
        }
    }

    /// A technician's request in a session: no rollout, no backoff; refused in `manual`.
    pub fn request(&self, action: Action, now: u64) -> AgentAnswer {
        tidied(self.request_of(action, now))
    }

    fn request_of(&self, action: Action, now: u64) -> AgentAnswer {
        if action == Action::Status {
            return self.status_of(now);
        }
        if !self.sys.service_process() {
            return AgentAnswer::only("needs-service", NEEDS_SERVICE);
        }
        if !active(&self.p.mode) {
            return AgentAnswer::only("not-allowed", "not allowed by this computer's policy");
        }
        if let Err(e) = self.sys.check_dir(&self.p.dir) {
            return AgentAnswer::only("failed", &e);
        }
        let r = (|| {
            self.sys.secure_dir(&self.p.dir)?;
            self.sweep()?;
            let svc = self.sys.service_exists()?;
            let mut st = self.load().unwrap_or_default();
            Self::adopt(&mut st, svc, now);
            self.notice_removal(&mut st, svc, now);
            if action == Action::Install && !svc && st.pending_install.is_some() && self.p.mode != "remove" {
                // That install may well succeed: not a failure, and the saved state stays as it is.
                return Ok(None);
            }
            let done = match action {
                Action::Install if self.p.mode == "remove" => Err("not allowed: this computer's policy removes the agent".to_owned()),
                Action::Install if svc => Err("the agent is already installed".to_owned()),
                Action::Install => self.install(&mut st, "session", now, false).map(|()| st.detail.clone()),
                Action::Remove if !svc => Err("the agent is not installed".to_owned()),
                Action::Remove => self.remove().map(|l| {
                    Self::succeeded(&mut st, "removed-by-tether", "session", l.clone(), now);
                    st.ours = false;
                    // A technician's removal sticks against an install by policy.
                    st.hold = "session".into();
                    l
                }),
                Action::Status => Ok(String::new()),
            };
            if let Err(e) = &done {
                if matches!(action, Action::Install) && !svc && self.p.mode != "remove" {
                    // The failed install is recorded (no backoff for the technician, but shown).
                    st.state = "failed".into();
                    st.detail = e.clone();
                    st.at = now;
                }
            }
            self.save(&self.load(), &st)?;
            done.map(Some)
        })();
        match r {
            Ok(None) => AgentAnswer::only("busy", MAY_STILL_RUN),
            Ok(Some(line)) => AgentAnswer { detail: line, ..self.status_of(now) },
            Err(e) => {
                log::warn!("health agent request: {e}");
                AgentAnswer { detail: e, ..self.status_of(now) }
            }
        }
    }
}

// ----------------------------------------------------------------------------- entry points

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[cfg(windows)]
fn inputs(eff: &tether_trust::policy::Effective) -> Inputs {
    let channel = eff.get("update.channel");
    Inputs {
        mode: eff.get("health.agent").to_owned(),
        grant: eff.get("health.enrol-grant").to_owned(),
        channel: if release::valid_channel(channel) { channel.to_owned() } else { "stable".into() },
        rollout: eff.get("update.rollout").to_owned(),
        uuid: crate::policy::install_uuid(),
        admin: crate::policy::admin_keys(),
        updater_key: crate::update::UPDATER_KEY.unwrap_or_default().to_owned(),
        dir: win::program_data().join(crate::APP_NAME),
    }
}

/// After each policy check in the `--server` process. Inert unless the policy asks for something
/// and this process is Tether's LocalSystem service.
#[cfg(windows)]
pub fn tick(eff: &tether_trust::policy::Effective, now: u64) {
    // Inactive: only to drop a stale install mark, so only when a state file is there at all.
    if !active(eff.get("health.agent")) && !win::program_data().join(crate::APP_NAME).join(STATE_FILE).is_file() {
        return;
    }
    if !win::Real.service_process() {
        return;
    }
    let Ok(_running) = Running::start(TICK) else { return };
    match (Manager { sys: &win::Real, p: inputs(eff) }).tick(now) {
        Err(e) if e == NEEDS_SERVICE => log::debug!("health agent: {e}"),
        Err(e) => log::info!("health agent: {e}"),
        Ok(()) => {}
    }
}

/// No Tether service runs on this computer: this process hosts for the user, and the agent goes
/// through the elevation prompt (agent_elevated.rs) instead of the manager.
#[cfg(windows)]
fn without_service() -> bool {
    !win::Real.service_process() && !librustdesk::platform::is_self_service_running()
}

/// The elevated path for this process: its own identity key (the user's `Tether.toml`), its policy.
#[cfg(windows)]
fn elevated(eff: &tether_trust::policy::Effective) -> crate::agent_elevated::Host<'static> {
    let i = inputs(eff);
    let relays = crate::policy::relay_bases().into_iter().map(|(host, _)| host).collect();
    use crate::agent_elevated::{machine_manual, win::machine_policy};
    // The technician profile names no support organisation (`customer.title` is TetherQS's): none is shown.
    let machine_off = machine_manual(machine_policy(crate::policy::MACHINE_POLICY_KEY).as_deref());
    let p = crate::agent_elevated::Inputs { mode: i.mode, grant: i.grant, channel: i.channel, admin: i.admin, updater_key: i.updater_key, elevate: true, identity: win::Real.own_identity(), org: crate::APP_NAME.to_owned(), support_org: String::new(), relays, machine_off };
    crate::agent_elevated::Host { m: &win::UserMachine, p }
}

/// A technician's request in a session, run to its end with its real outcome (the core answers the
/// session `running` meanwhile when it takes longer, see [`exclusive`]). `viewer`: the name their
/// program gave (the elevated path's dialog shows it as not verified).
#[cfg(windows)]
pub fn request(action: Action, viewer: &str) -> AgentAnswer {
    let what = match action {
        Action::Status => return status(),
        Action::Install => "installing the agent",
        Action::Remove => "removing the agent",
    };
    if without_service() {
        return elevated(&crate::policy::effective()).request(action, Some(viewer), now());
    }
    let m = Manager { sys: &win::Real, p: inputs(&crate::policy::effective()) };
    if !win::Real.service_process() {
        return m.request(action, now());
    }
    exclusive(what, || m.request(action, now()))
}

/// What the host answers to `status`: `busy` while an action runs ([`busy_now`]).
#[cfg(windows)]
pub fn status() -> AgentAnswer {
    if without_service() {
        return elevated(&crate::policy::effective()).status(now());
    }
    let slot = *running();
    if let Some(b) = busy_now(slot) {
        if win::Real.service_process() {
            return b;
        }
    }
    (Manager { sys: &win::Real, p: inputs(&crate::policy::effective()) }).status(now())
}

/// Settings on this computer (the UI process, never the service): whether the agent's service is
/// there and whether Tether installed it, when and why: from the service's public copy, else this
/// user's own record of the prompt path. Never a secret.
#[cfg(windows)]
pub fn local_status() -> AgentAnswer {
    if !crate::agent_elevated::win::service_exists().unwrap_or(false) {
        return AgentAnswer::only("not-installed", "");
    }
    let public = win::program_data().join(crate::APP_NAME).join(PUBLIC_FILE);
    let by_service = win::file_trusted(&public).then(|| std::fs::read(&public).ok()).flatten().and_then(|b| read_public(&b)).filter(|a| a.state == "installed");
    by_service.unwrap_or_else(|| match elevated(&crate::policy::effective()).status(now()) {
        a if a.state == "installed" => AgentAnswer { detail: String::new(), ..a },
        _ => AgentAnswer::only("installed-by-hand", ""),
    })
}

/// The home view's card (spec 7.2): the policy asks to install, Tether runs without its service,
/// the agent is not there and was not removed on this computer.
#[cfg(windows)]
pub fn offer() -> bool {
    let eff = crate::policy::effective();
    eff.get("health.agent") == "install" && without_service() && matches!(elevated(&eff).status(now()).state.as_str(), "not-installed" | "failed" | "removed-by-tether")
}

/// The card's Install…: the person at this computer asked (the same dialog and prompt).
#[cfg(windows)]
pub fn install_here() -> AgentAnswer {
    if !without_service() {
        return AgentAnswer::only("needs-service", "Tether's service installs the agent on this computer");
    }
    elevated(&crate::policy::effective()).request(Action::Install, None, now())
}

#[cfg(not(windows))]
pub fn tick(_eff: &tether_trust::policy::Effective, _now: u64) {}

#[cfg(not(windows))]
pub fn request(_action: Action, _viewer: &str) -> AgentAnswer {
    AgentAnswer::only("not-supported", "the health monitoring agent is for Windows")
}

#[cfg(not(windows))]
pub fn status() -> AgentAnswer {
    request(Action::Status, "")
}

#[cfg(not(windows))]
pub fn local_status() -> AgentAnswer {
    status()
}

#[cfg(not(windows))]
pub fn offer() -> bool {
    false
}

#[cfg(not(windows))]
pub fn install_here() -> AgentAnswer {
    status()
}

// ----------------------------------------------------------------------------- Windows

#[cfg(windows)]
mod win {
    use super::*;
    use crate::agent_elevated::AGENT_FOLDER;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::{GetLastError, LocalFree, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW, SetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{GetSecurityDescriptorDacl, ACL, DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES};
    use windows_sys::Win32::Storage::FileSystem::{CreateDirectoryW, FILE_ATTRIBUTE_REPARSE_POINT};

    /// Protected: SYSTEM and Administrators full control, inherited by files and folders, nothing else.
    const SDDL: &str = "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    pub fn program_data() -> PathBuf {
        std::env::var_os("ProgramData").map_or_else(|| PathBuf::from(r"C:\ProgramData"), PathBuf::from)
    }


    /// SYSTEM or the Administrators group.
    pub(super) fn trusted_owner(sid: &str) -> bool {
        matches!(sid, "S-1-5-18" | "S-1-5-32-544")
    }

    fn is_reparse(p: &Path) -> std::io::Result<bool> {
        use std::os::windows::fs::MetadataExt;
        Ok(std::fs::symlink_metadata(p)?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
    }

    /// Owner of `p` as a SID string.
    pub(super) fn owner_sid(p: &Path) -> Result<String, String> {
        let w = wide(p.as_os_str());
        unsafe {
            let (mut owner, mut sd): (PSID, PSECURITY_DESCRIPTOR) = (null_mut(), null_mut());
            let r = GetNamedSecurityInfoW(w.as_ptr(), SE_FILE_OBJECT, OWNER_SECURITY_INFORMATION, &mut owner, null_mut(), null_mut(), null_mut(), &mut sd);
            if r != 0 {
                return Err(format!("{}: {}", p.display(), std::io::Error::from_raw_os_error(r as i32)));
            }
            let mut s = null_mut();
            let ok = ConvertSidToStringSidW(owner, &mut s);
            LocalFree(sd);
            if ok == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let len = (0..).take_while(|&i| *s.add(i) != 0).count();
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(s, len));
            LocalFree(s.cast());
            Ok(sid)
        }
    }

    /// Create `dir` with [`SDDL`]; refuse a junction or link and an owner other than SYSTEM or
    /// Administrators (a standard user can create folders in `%ProgramData%`); then set the DACL
    /// again (an administrator's folder may have had other entries).
    pub(super) fn secure_dir(dir: &Path) -> Result<(), String> {
        let w = wide(dir.as_os_str());
        let sddl = wide(std::ffi::OsStr::new(SDDL));
        unsafe {
            let mut sd: PSECURITY_DESCRIPTOR = null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, null_mut()) == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let r = (|| {
                let sa = SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd, bInheritHandle: 0 };
                if CreateDirectoryW(w.as_ptr(), &sa) == 0 && GetLastError() != ERROR_ALREADY_EXISTS {
                    return Err(format!("{}: {}", dir.display(), std::io::Error::last_os_error()));
                }
                if is_reparse(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
                    return Err(format!("{} is a junction or link: refused", dir.display()));
                }
                if !dir.is_dir() {
                    return Err(format!("{} is not a folder: refused", dir.display()));
                }
                let owner = owner_sid(dir)?;
                if !trusted_owner(&owner) {
                    return Err(format!("{} is owned by {owner}, not SYSTEM or Administrators: refused", dir.display()));
                }
                let (mut present, mut defaulted, mut dacl): (i32, i32, *mut ACL) = (0, 0, null_mut());
                if GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted) == 0 {
                    return Err(std::io::Error::last_os_error().to_string());
                }
                let r = SetNamedSecurityInfoW(w.as_ptr() as *mut u16, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, null_mut(), null_mut(), dacl, null_mut());
                if r != 0 {
                    return Err(format!("{}: {}", dir.display(), std::io::Error::from_raw_os_error(r as i32)));
                }
                Ok(())
            })();
            LocalFree(sd);
            r
        }
    }

    /// [`Sys::check_dir`]: reads only.
    pub(super) fn check_dir(dir: &Path) -> Result<(), String> {
        const NAME: &str = "Tether's state folder";
        match is_reparse(dir) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(format!("{NAME} cannot be read")),
            Ok(true) => return Err(format!("{NAME} is a junction or link: refused")),
            Ok(false) if !dir.is_dir() => return Err(format!("{NAME} is not a folder: refused")),
            Ok(false) => {}
        }
        let owner = owner_sid(dir).map_err(|_| format!("the owner of {NAME} cannot be read"))?;
        if !trusted_owner(&owner) {
            return Err(format!("{NAME} is owned by {owner}, not SYSTEM or Administrators: refused"));
        }
        Ok(())
    }

    /// [`Sys::file_trusted`].
    pub(super) fn file_trusted(p: &Path) -> bool {
        use std::os::windows::fs::MetadataExt;
        match std::fs::symlink_metadata(p) {
            Err(e) => e.kind() == std::io::ErrorKind::NotFound,
            Ok(m) => m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0 && owner_sid(p).is_ok_and(|s| trusted_owner(&s)),
        }
    }

    pub(super) use crate::agent_elevated::win::open_locked;

    /// What is kept of each of stdout and stderr: the last bytes, cut to whole lines.
    const TAIL: usize = 4096;
    /// After the child ended or was killed, how long its output may still take (a grandchild
    /// holding the pipes would otherwise keep the manager busy for good).
    const DRAIN_AFTER: Duration = Duration::from_secs(3);

    /// CreateProcess on `exe` with a fixed argument list (never a shell), no window; the last
    /// [`TAIL`] bytes of stdout and stderr kept; killed after `timeout`.
    pub(super) fn run(exe: &Path, args: &[&str], stdin: Option<&str>, timeout: Duration) -> Result<Ran, String> {
        use std::io::{Read, Write};
        use std::os::windows::process::CommandExt;
        use std::process::Stdio;
        let mut cmd = std::process::Command::new(exe);
        cmd.args(args).creation_flags(CREATE_NO_WINDOW).stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
        if let Some(d) = exe.parent() {
            cmd.current_dir(d);
        }
        let mut child = cmd.spawn().map_err(|e| format!("{}: {e}", exe.display()))?;
        type Tail = std::sync::Arc<std::sync::Mutex<Vec<u8>>>;
        fn drain(r: Option<impl Read + Send + 'static>) -> (Tail, std::thread::JoinHandle<()>) {
            let tail = Tail::default();
            let t = tail.clone();
            let h = std::thread::spawn(move || {
                let Some(mut r) = r else { return };
                let mut buf = [0u8; 8192];
                while let Ok(n @ 1..) = r.read(&mut buf) {
                    let mut v = t.lock().unwrap_or_else(|e| e.into_inner());
                    v.extend_from_slice(&buf[..n]);
                    if v.len() > TAIL {
                        let cut = v.len() - TAIL;
                        v.drain(..cut);
                    }
                }
            });
            (tail, h)
        }
        fn text(t: &Tail) -> String {
            let v = t.lock().unwrap_or_else(|e| e.into_inner()).clone();
            // A cut tail starts in the middle of a line: drop that part.
            let v = if v.len() >= TAIL { v.iter().position(|&b| b == b'\n').map_or(&v[..], |i| &v[i + 1..]).to_vec() } else { v };
            String::from_utf8_lossy(&v).into_owned()
        }
        let ((out, out_h), (err, err_h)) = (drain(child.stdout.take()), drain(child.stderr.take()));
        let finish = |out_h: std::thread::JoinHandle<()>, err_h: std::thread::JoinHandle<()>| {
            // Past this the drains are abandoned (they end when the last holder of the pipe does).
            let end = std::time::Instant::now() + DRAIN_AFTER;
            while !(out_h.is_finished() && err_h.is_finished()) && std::time::Instant::now() < end {
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        if let (Some(line), Some(mut i)) = (stdin, child.stdin.take()) {
            let _ = i.write_all(format!("{line}\n").as_bytes());
        }
        let deadline = std::time::Instant::now() + timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break s,
                Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(Duration::from_millis(200)),
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("{} {} did not finish within {} s", exe.display(), args.first().unwrap_or(&""), timeout.as_secs()));
                }
                Err(e) => return Err(e.to_string()),
            }
        };
        finish(out_h, err_h);
        Ok(Ran { code: status.code().unwrap_or(-1), out: text(&out), err: text(&err) })
    }

    /// The elevated path's machine for this process (a Tether without its service).
    pub struct UserMachine;

    impl crate::agent_elevated::Machine for UserMachine {
        fn user_dir(&self) -> PathBuf {
            crate::agent_elevated::win::user_dir()
        }
        fn agent_service(&self) -> Result<bool, String> {
            crate::agent_elevated::win::service_exists()
        }
        fn removed_at(&self) -> Option<u64> {
            crate::agent_elevated::win::removed_at()
        }
        fn installed_exe(&self) -> Result<PathBuf, String> {
            crate::agent_elevated::win::installed_exe()
        }
        fn service_image(&self) -> Result<String, String> {
            crate::agent_elevated::win::service_image()
        }
        fn installed_version(&self) -> Option<String> {
            crate::agent_elevated::win::installed_version()
        }
        fn uac(&self) -> crate::agent_elevated::Uac {
            crate::agent_elevated::win::uac()
        }
        fn owner_window(&self) -> usize {
            crate::app::front_main_window()
        }
        fn fetch_index(&self, channel: &str) -> Vec<(String, Vec<u8>)> {
            Real.fetch_index(channel)
        }
        fn download(&self, c: &Candidate, key: &str) -> Result<Vec<u8>, String> {
            crate::update::download_within(c, key, Some(download_time(c.size)))
        }
        fn prepare_stage(&self, dir: &Path) -> Result<(), String> {
            crate::agent_elevated::win::prepare_stage(dir)
        }
        fn open_locked(&self, path: &Path) -> Result<(Vec<u8>, Box<dyn std::any::Any>), String> {
            open_locked(path)
        }
        fn pipe_name(&self) -> Result<String, String> {
            crate::agent_elevated::win::pipe_name()
        }
        fn create_pipe(&self, name: &str) -> Result<Box<dyn crate::agent_elevated::Pipe>, String> {
            crate::agent_elevated::win::create_pipe(name)
        }
        fn launch(&self, exe: &Path, args: &[&str], owner: usize) -> Result<Box<dyn crate::agent_elevated::Child>, crate::agent_elevated::Launch> {
            crate::agent_elevated::win::launch(exe, args, owner)
        }
        fn consent(&self, d: &crate::agent_elevated::Dialog) -> crate::agent_elevated::Consent {
            crate::agent_elevated::ask_window(d.clone(), crate::app::show_agent_consent, crate::agent_elevated::CONSENT_WAIT)
        }
    }

    /// The public copy's access list: SYSTEM and Administrators full control, Users read.
    const PUBLIC_SDDL: &str = "D:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FR;;;BU)";

    pub(super) fn write_public(path: &Path, data: &[u8]) -> Result<(), String> {
        crate::policy::write_atomic(&path.to_path_buf(), data).map_err(|e| e.to_string())?;
        let w = wide(path.as_os_str());
        let sddl = wide(std::ffi::OsStr::new(PUBLIC_SDDL));
        unsafe {
            let mut sd: PSECURITY_DESCRIPTOR = null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, null_mut()) == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let (mut present, mut defaulted, mut dacl): (i32, i32, *mut ACL) = (0, 0, null_mut());
            let r = if GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted) == 0 {
                Err(std::io::Error::last_os_error().to_string())
            } else {
                match SetNamedSecurityInfoW(w.as_ptr() as *mut u16, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, null_mut(), null_mut(), dacl, null_mut()) {
                    0 => Ok(()),
                    e => Err(std::io::Error::from_raw_os_error(e as i32).to_string()),
                }
            };
            LocalFree(sd);
            r
        }
    }

    pub struct Real;

    impl Sys for Real {
        fn service_process(&self) -> bool {
            service_gate(std::env::args().nth(1).as_deref(), librustdesk::platform::is_root())
        }

        fn own_identity(&self) -> Option<(String, [u8; 32])> {
            // The key pair this process registers with the rendezvous server (its own config).
            crate::agent_elevated::own_identity()
        }

        fn service_exists(&self) -> Result<bool, String> {
            crate::agent_elevated::win::service_exists()
        }

        fn versions(&self) -> (Option<String>, Option<String>) {
            let exe = self.installed_exe();
            let sup = exe
                .is_file()
                .then(|| run(&exe, &["--version"], None, Duration::from_secs(30)).ok())
                .flatten()
                .filter(|r| r.code == 0)
                .and_then(|r| r.out.lines().next().and_then(|l| l.split_whitespace().nth(1)).map(str::to_owned))
                .filter(|v| release::valid_version(v));
            let worker = std::fs::read(program_data().join(AGENT_FOLDER).join("current.json"))
                .ok()
                .filter(|b| b.len() <= 64 << 10)
                .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                .and_then(|v| v["current"].as_str().map(str::to_owned))
                .filter(|v| release::valid_version(v));
            (sup, worker)
        }

        fn not_enrolled(&self) -> bool {
            program_data().join(AGENT_FOLDER).join("not-enrolled").is_file()
        }

        fn removed_at(&self) -> Option<u64> {
            crate::agent_elevated::win::removed_at()
        }

        fn secure_dir(&self, dir: &Path) -> Result<(), String> {
            secure_dir(dir)
        }

        fn check_dir(&self, dir: &Path) -> Result<(), String> {
            check_dir(dir)
        }

        fn file_trusted(&self, path: &Path) -> bool {
            file_trusted(path)
        }

        fn fetch_index(&self, channel: &str) -> Vec<(String, Vec<u8>)> {
            let path = format!("/download/{channel}/{}", release::MONITOR_INDEX_FILE);
            crate::policy::relay_bases()
                .into_iter()
                .filter_map(|(host, base)| match crate::update::fetch_doc(&host, &path, release::MAX_INDEX) {
                    Ok(Some(raw)) => Some((base, raw)),
                    Ok(None) => None,
                    Err(e) => {
                        log::info!("health agent: {host}{path}: {e}");
                        None
                    }
                })
                .collect()
        }

        fn download(&self, c: &Candidate, key: &str, within: Duration) -> Result<Vec<u8>, String> {
            crate::update::download_within(c, key, Some(within))
        }

        fn open_locked(&self, path: &Path) -> Result<(Vec<u8>, Box<dyn std::any::Any>), String> {
            open_locked(path)
        }

        fn run(&self, exe: &Path, args: &[&str], stdin: Option<&str>, timeout: Duration) -> Result<Ran, String> {
            run(exe, args, stdin, timeout)
        }

        fn installed_exe(&self) -> PathBuf {
            crate::agent_elevated::win::installed_exe().unwrap_or_default()
        }

        fn write_public(&self, path: &Path, data: &[u8]) -> Result<(), String> {
            write_public(path, data)
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use crate::agent_elevated::AGENT_EXE;
    use tether_trust::{envelope, release::{Entry, Index, Rollout}, SigningKey};

    const NOW: u64 = 1_900_000_000;
    const ID: &str = "123456789";
    const SEED: [u8; 32] = [7; 32];

    struct Keys {
        admin: SigningKey,
        kp: minisign::KeyPair,
        public: String,
    }

    fn b64(b: &[u8]) -> String {
        use hbb_common::base64::Engine;
        hbb_common::base64::engine::general_purpose::STANDARD.encode(b)
    }

    fn keys() -> Keys {
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let public = b64(kp.pk.to_box().unwrap().to_string().as_bytes());
        Keys { admin: tether_trust::generate_key(), kp, public }
    }

    struct Pub {
        version: &'static str,
        data: Vec<u8>,
        file: Option<String>,
        signed_file: Option<String>,
        sha: Option<String>,
        serial: u64,
        expires: u64,
        halted: bool,
        minimum: &'static str,
        rollout: Vec<(u64, u8)>,
        /// The minisign signature made with a key other than the updater key.
        foreign_sig: bool,
    }

    fn agent(version: &'static str) -> Pub {
        Pub { version, data: format!("agent {version}").into_bytes(), file: None, signed_file: None, sha: None, serial: 5, expires: NOW + 30 * 86_400, halted: false, minimum: "", rollout: vec![(0, 100)], foreign_sig: false }
    }

    fn doc(k: &Keys, p: &Pub) -> Vec<u8> {
        let file = p.file.clone().unwrap_or_else(|| format!("tether-monitor-{}.exe", p.version));
        let trusted = format!("timestamp:1\tfile:{}\tversion:{}", p.signed_file.clone().unwrap_or_else(|| file.clone()), p.version);
        let other = p.foreign_sig.then(|| minisign::KeyPair::generate_unencrypted_keypair().unwrap());
        let kp = other.as_ref().unwrap_or(&k.kp);
        let sig = minisign::sign(Some(&kp.pk), &kp.sk, std::io::Cursor::new(&p.data), Some(&trusted), Some("u")).unwrap();
        let e = Entry {
            version: p.version.into(),
            file,
            size: p.data.len() as u64,
            sha256: p.sha.clone().unwrap_or_else(|| release::sha256_hex(&p.data)),
            signature: b64(sig.to_string().as_bytes()),
            urls: vec!["https://{relay}/download/stable/monitor/x.exe".into()],
            minimum_version: p.minimum.into(),
            halted: p.halted,
            rollout: Rollout { start: NOW - 10, steps: p.rollout.clone() },
            ..Default::default()
        };
        let ix = Index { kind: release::INDEX_TYPE.into(), channel: "stable".into(), serial: p.serial, issued: NOW - 86_400 * 40, expires: p.expires, releases: [(format!("monitor/{}", release::WINDOWS), e)].into() };
        serde_json::to_vec(&envelope::sign(&serde_json::to_vec(&ix).unwrap(), &k.admin)).unwrap()
    }

    fn grant(k: &Keys, proof: bool) -> (String, String) {
        status::make_token("policy:test", NOW + 86_400 * 365, None, None, proof, &k.admin).unwrap()
    }

    const UUID: &str = "agent-uuid-0001";
    fn agent_pubkey() -> String {
        tether_trust::key_to_b64(&SigningKey::from_bytes(&[3; 32]).verifying_key())
    }

    #[derive(Default)]
    struct Fake {
        user: bool,
        svc: Cell<bool>,
        versions: RefCell<(Option<String>, Option<String>)>,
        not_enrolled: Cell<bool>,
        removed_at: Cell<Option<u64>>,
        refuse_dir: RefCell<Option<String>>,
        docs: RefCell<Vec<Vec<u8>>>,
        artifact: RefCell<Vec<u8>>,
        fetches: Cell<u32>,
        runs: RefCell<Vec<(String, Vec<String>, Option<String>)>>,
        codes: RefCell<HashMap<&'static str, (i32, String)>>,
        tamper: Cell<bool>,
        /// Files a standard user owns: `(path, bytes)` (a rewrite by Tether is owned by it).
        planted: RefCell<Vec<(PathBuf, Vec<u8>)>>,
        /// `open_locked` finds the staged file owned by a user.
        plant_staged: Cell<bool>,
        /// What `identity` prints instead of the good line; this host's ID instead of [`ID`].
        identity: RefCell<Option<String>>,
        own_id: RefCell<Option<String>>,
        within: RefCell<Vec<Duration>>,
        /// The clock (0: the time the test passes in); the download moves it on by `download_takes`.
        clock: Cell<u64>,
        download_takes: Cell<u64>,
        /// A failing `install` has made the agent's service first.
        svc_on_failure: Cell<bool>,
    }

    impl Fake {
        fn plant(&self, p: &Path, bytes: &[u8]) {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, bytes).unwrap();
            self.planted.borrow_mut().push((p.to_owned(), bytes.to_owned()));
        }
    }

    impl Sys for Fake {
        fn service_process(&self) -> bool {
            !self.user
        }
        fn own_identity(&self) -> Option<(String, [u8; 32])> {
            Some((self.own_id.borrow().clone().unwrap_or_else(|| ID.into()), SEED))
        }
        fn service_exists(&self) -> Result<bool, String> {
            Ok(self.svc.get())
        }
        fn versions(&self) -> (Option<String>, Option<String>) {
            self.versions.borrow().clone()
        }
        fn not_enrolled(&self) -> bool {
            self.not_enrolled.get()
        }
        fn removed_at(&self) -> Option<u64> {
            self.removed_at.get()
        }
        fn secure_dir(&self, dir: &Path) -> Result<(), String> {
            match self.refuse_dir.borrow().clone() {
                Some(e) => Err(e),
                None => std::fs::create_dir_all(dir).map_err(|e| e.to_string()),
            }
        }
        fn check_dir(&self, _dir: &Path) -> Result<(), String> {
            self.refuse_dir.borrow().clone().map_or(Ok(()), Err)
        }
        fn file_trusted(&self, path: &Path) -> bool {
            let now = std::fs::read(path).ok();
            !self.planted.borrow().iter().any(|(p, b)| p == path && now.as_ref() == Some(b))
        }
        fn fetch_index(&self, _channel: &str) -> Vec<(String, Vec<u8>)> {
            self.fetches.set(self.fetches.get() + 1);
            self.docs.borrow().iter().map(|d| ("https://relay.test".to_owned(), d.clone())).collect()
        }
        fn download(&self, _c: &Candidate, _key: &str, within: Duration) -> Result<Vec<u8>, String> {
            self.within.borrow_mut().push(within);
            if self.download_takes.get() > 0 {
                self.clock.set(NOW + self.download_takes.get());
            }
            Ok(self.artifact.borrow().clone())
        }
        fn clock(&self) -> u64 {
            self.clock.get()
        }
        fn write_public(&self, path: &Path, data: &[u8]) -> Result<(), String> {
            std::fs::write(path, data).map_err(|e| e.to_string())
        }
        fn open_locked(&self, path: &Path) -> Result<(Vec<u8>, Box<dyn std::any::Any>), String> {
            if self.tamper.get() {
                std::fs::write(path, b"planted").unwrap();
            }
            if self.plant_staged.get() {
                self.planted.borrow_mut().push((path.to_owned(), std::fs::read(path).unwrap()));
            }
            Ok((std::fs::read(path).map_err(|e| e.to_string())?, Box::new(())))
        }
        fn run(&self, exe: &Path, args: &[&str], stdin: Option<&str>, _timeout: Duration) -> Result<Ran, String> {
            let name = exe.file_name().unwrap().to_string_lossy().into_owned();
            if name == STAGED_EXE {
                assert!(exe.exists(), "the staged file is there while it runs");
            }
            self.runs.borrow_mut().push((name, args.iter().map(|a| a.to_string()).collect(), stdin.map(str::to_owned)));
            let (code, line) = self.codes.borrow().get(args[0]).cloned().unwrap_or((0, "ok".into()));
            // Tether stops while the child runs; -99: the child finishes, -98: it does not.
            if code == -99 || code == -98 {
                self.svc.set(code == -99);
                panic!("Tether stopped");
            }
            let out = match args[0] {
                "identity" if code == 0 => self.identity.borrow().clone().unwrap_or_else(|| format!("{{\"uuid\":\"{UUID}\",\"pubkey\":\"{}\"}}\n", agent_pubkey())),
                _ => format!("{line}\n"),
            };
            if code != 0 && args[0] == "install" && self.svc_on_failure.get() {
                self.svc.set(true);
            }
            if code == 0 {
                match args[0] {
                    "install" => self.svc.set(true),
                    "uninstall" => self.svc.set(false),
                    _ => {}
                }
            }
            Ok(Ran { code, err: if code != 0 { format!("{line}\n") } else { String::new() }, out })
        }
        fn installed_exe(&self) -> PathBuf {
            PathBuf::from(r"C:\Program Files\Tether Monitor\tether-monitor.exe")
        }
    }

    struct World {
        k: Keys,
        fake: Fake,
        p: Inputs,
        token: String,
        grant_id: String,
        _dir: TempDir,
    }

    struct TempDir(PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn world(name: &str, mode: &str) -> World {
        let k = keys();
        let (token, grant_id) = grant(&k, true);
        let dir = std::env::temp_dir().join(format!("tether-agent-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = Inputs { mode: mode.into(), grant: token.clone(), channel: "stable".into(), rollout: String::new(), uuid: "install-uuid-1".into(), admin: vec![k.admin.verifying_key()], updater_key: k.public.clone(), dir: dir.clone() };
        let fake = Fake::default();
        let a = agent("0.2.0");
        *fake.docs.borrow_mut() = vec![doc(&k, &a)];
        *fake.artifact.borrow_mut() = a.data;
        World { k, fake, p, token, grant_id, _dir: TempDir(dir) }
    }

    impl World {
        fn m(&self) -> Manager<'_> {
            Manager { sys: &self.fake, p: self.p.clone() }
        }
        fn tick(&self, now: u64) -> Result<(), String> {
            self.m().tick(now)
        }
        fn state(&self) -> Option<State> {
            self.m().load()
        }
        fn runs(&self) -> Vec<Vec<String>> {
            self.fake.runs.borrow().iter().map(|r| r.1.clone()).collect()
        }
        fn publish(&self, p: Pub) {
            *self.fake.docs.borrow_mut() = vec![doc(&self.k, &p)];
            *self.fake.artifact.borrow_mut() = p.data;
        }
        fn files(&self) -> Vec<PathBuf> {
            std::fs::read_dir(&self.p.dir).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default()
        }
    }

    #[test]
    fn manual_does_nothing_in_every_state() {
        for (svc, removed, state) in [(false, None, false), (true, None, false), (true, Some(5), true), (false, None, true)] {
            let w = world("manual", "manual");
            w.fake.svc.set(svc);
            w.fake.removed_at.set(removed);
            if state {
                std::fs::create_dir_all(&w.p.dir).unwrap();
                std::fs::write(w.p.dir.join(STATE_FILE), br#"{"state":"installed","ours":true}"#).unwrap();
            }
            let before = w.files();
            w.tick(NOW).unwrap();
            assert_eq!(w.fake.fetches.get(), 0);
            assert!(w.runs().is_empty());
            assert_eq!(w.files(), before, "no file written");
            // a session request is refused too
            for a in [Action::Install, Action::Remove] {
                assert_eq!(w.m().request(a, NOW).state, "not-allowed");
            }
            assert!(w.runs().is_empty() && w.files() == before);
        }
        // an unknown mode is manual
        let w = world("unknownmode", "yes");
        w.tick(NOW).unwrap();
        assert!(w.runs().is_empty() && w.files().is_empty());
    }

    #[test]
    fn install_by_policy_downloads_verifies_links_and_installs() {
        let w = world("install", "install");
        w.tick(NOW).unwrap();
        let runs = w.fake.runs.borrow().clone();
        assert_eq!(runs.len(), 2, "{runs:?}");
        assert_eq!((runs[0].0.as_str(), runs[0].1.clone(), runs[0].2.clone()), (STAGED_EXE, vec!["identity".to_owned()], None));
        assert_eq!((runs[1].0.as_str(), runs[1].1.clone()), (STAGED_EXE, vec!["install".to_owned(), "--from-tether".into(), "-".into()]));
        let line: serde_json::Value = serde_json::from_str(runs[1].2.as_deref().unwrap()).unwrap();
        assert_eq!(line.as_object().unwrap().len(), 4);
        assert_eq!((line["token"].as_str(), line["tether_id"].as_str(), line["channel"].as_str()), (Some(w.token.as_str()), Some(ID), Some("stable")));
        let pk = tether_trust::key_to_b64(&SigningKey::from_bytes(&SEED).verifying_key());
        status::verify_agent_link(&pk, line["proof"].as_str().unwrap(), ID, UUID, &agent_pubkey()).unwrap();
        let st = w.state().unwrap();
        assert_eq!((st.state.as_str(), st.by.as_str(), st.version.as_str(), st.ours, st.grant_id.as_str()), ("installed", "policy", "0.2.0", true, w.grant_id.as_str()));
        assert!(!w.p.dir.join(DOWNLOAD_DIR).join(STAGED_EXE).exists(), "staged file removed");
        // neither grant nor proof in any file
        for f in w.files().into_iter().filter(|f| f.is_file()) {
            let t = String::from_utf8_lossy(&std::fs::read(&f).unwrap()).into_owned();
            assert!(!t.contains(&w.token) && !t.contains(line["proof"].as_str().unwrap()), "{}", f.display());
        }
        let a = w.m().status(NOW);
        assert_eq!((a.state.as_str(), a.installed_by.as_str()), ("installed", "policy"));
        // the next tick does nothing more
        w.tick(NOW + 60).unwrap();
        assert_eq!(w.runs().len(), 2);
    }

    #[test]
    fn a_failed_install_backs_off_1h_6h_24h_with_the_agents_line() {
        let w = world("backoff", "install");
        w.fake.codes.borrow_mut().insert("install", (1, "enrolment failed: relay.test: 400 unknown field `tether_proof`".into()));
        let mut t = NOW;
        for wait in [3600, 6 * 3600, 86_400, 86_400] {
            w.tick(t).unwrap();
            let st = w.state().unwrap();
            assert_eq!(st.state, "failed");
            assert!(st.detail.contains("400 unknown field"), "{}", st.detail);
            assert_eq!(st.next_try, t + wait);
            let n = w.runs().len();
            w.tick(t + wait - 1).unwrap();
            assert_eq!(w.runs().len(), n, "nothing before the backoff ends");
            t += wait;
        }
        assert_eq!(w.m().status(NOW).state, "failed");
        // a technician in a session ignores the backoff
        w.fake.codes.borrow_mut().clear();
        let a = w.m().request(Action::Install, t - 10);
        assert_eq!((a.state.as_str(), a.installed_by.as_str()), ("installed", "session"));
    }

    #[test]
    fn local_removal_sticks() {
        // the service Tether installed vanished
        let w = world("vanished", "install");
        w.tick(NOW).unwrap();
        w.fake.svc.set(false);
        w.tick(NOW + 100).unwrap();
        let st = w.state().unwrap();
        assert_eq!((st.state.as_str(), st.ours, st.at), ("removed-locally", false, NOW + 100));
        let n = w.runs().len();
        for t in [NOW + 200, NOW + 86_400 * 30] {
            w.tick(t).unwrap();
        }
        assert_eq!(w.runs().len(), n, "never installed again by policy");
        assert_eq!(w.m().status(NOW).state, "removed-locally");
        // RemovedAt: a hand-installed agent removed by the practice, and an unparseable value
        for (v, at) in [(Some(1_800_000_000), 1_800_000_000), (Some(0), NOW)] {
            let w = world("marker", "install");
            w.fake.removed_at.set(v);
            w.tick(NOW).unwrap();
            assert!(w.runs().is_empty());
            assert_eq!(w.fake.fetches.get(), 0);
            let st = w.state().unwrap();
            assert_eq!((st.state.as_str(), st.at), ("removed-locally", at));
        }
        // a technician can install again (the agent's install deletes RemovedAt)
        let w = world("again", "install");
        w.fake.removed_at.set(Some(5));
        w.tick(NOW).unwrap();
        assert_eq!(w.m().request(Action::Install, NOW + 1).state, "installed");
        w.fake.removed_at.set(None);
        w.tick(NOW + 2).unwrap();
        assert_eq!(w.state().unwrap().hold, "");
    }

    #[test]
    fn remove_runs_uninstall_by_tether_on_any_agent() {
        for (state, sup, args) in [(true, "0.2.0", vec!["uninstall", "--by-tether"]), (false, "0.2.1", vec!["uninstall", "--by-tether"]), (false, "0.1.9", vec!["uninstall"])] {
            let w = world("remove", "remove");
            w.fake.svc.set(true);
            *w.fake.versions.borrow_mut() = (Some(sup.into()), Some(sup.into()));
            if state {
                std::fs::create_dir_all(&w.p.dir).unwrap();
                std::fs::write(w.p.dir.join(STATE_FILE), br#"{"state":"installed","ours":true}"#).unwrap();
            }
            w.tick(NOW).unwrap();
            assert_eq!(w.fake.runs.borrow()[0].0, AGENT_EXE);
            assert_eq!(w.runs(), [args]);
            assert_eq!(w.fake.fetches.get(), 0);
            assert_eq!(w.state().unwrap().state, "removed-by-tether");
            // removed by policy: an install policy later installs again
            let mut p = w.p.clone();
            p.mode = "install".into();
            Manager { sys: &w.fake, p }.tick(NOW + 10).unwrap();
            assert_eq!(w.state().unwrap().state, "installed");
        }
    }

    #[test]
    fn an_install_that_fails_after_making_its_service_is_tethers_and_maintained() {
        const WHY: &str = "the worker did not come up within 60 s";
        let w = world("failsvc", "install");
        w.fake.svc_on_failure.set(true);
        w.fake.codes.borrow_mut().insert("install", (1, WHY.into()));
        w.tick(NOW).unwrap();
        let st = w.state().unwrap();
        assert_eq!((st.state.as_str(), st.by.as_str(), st.version.as_str(), st.ours, st.next_try), ("installed", "policy", "0.2.0", true, 0), "{st:?}");
        assert!(st.detail.contains(WHY), "{}", st.detail);
        let a = w.m().status(NOW + 1);
        assert_eq!((a.state.as_str(), a.installed_by.as_str()), ("installed", "policy"));
        // maintained: re-enrolment when the relays forgot it
        w.fake.not_enrolled.set(true);
        w.tick(NOW + ENROL_EVERY).unwrap();
        assert_eq!(w.runs().last().unwrap()[0], "enrol", "{:?}", w.runs());
        // and removed by policy
        let m = Manager { sys: &w.fake, p: Inputs { mode: "remove".into(), ..w.p.clone() } };
        m.tick(NOW + ENROL_EVERY + 1).unwrap();
        assert_eq!((w.state().unwrap().state.as_str(), w.runs().last().unwrap()[0].as_str()), ("removed-by-tether", "uninstall"));
        // a technician's install the same way: installed by the session, with the agent's line
        let w = world("failsvcsession", "on-request");
        w.fake.svc_on_failure.set(true);
        w.fake.codes.borrow_mut().insert("install", (1, WHY.into()));
        let a = w.m().request(Action::Install, NOW);
        assert_eq!((a.state.as_str(), a.installed_by.as_str()), ("installed", "session"), "{a:?}");
        assert!(a.detail.contains(WHY), "{}", a.detail);
        assert!(w.state().unwrap().ours);
        // no service made: failed, as before
        let w = world("failnosvc", "on-request");
        w.fake.codes.borrow_mut().insert("install", (1, WHY.into()));
        let a = w.m().request(Action::Install, NOW);
        assert_eq!(a.state, "failed");
        assert!(!w.state().unwrap().ours);
    }

    #[test]
    fn a_hand_installed_agent_is_untouched_except_by_remove() {
        for mode in ["install", "on-request"] {
            let w = world("byhand", mode);
            w.fake.svc.set(true);
            w.fake.not_enrolled.set(true);
            *w.fake.versions.borrow_mut() = (Some("0.1.0".into()), Some("0.1.0".into()));
            w.publish(Pub { minimum: "0.2.0", ..agent("0.2.0") });
            for t in [NOW, NOW + 3 * 86_400] {
                w.tick(t).unwrap();
            }
            assert!(w.runs().is_empty() && w.fake.fetches.get() == 0, "{mode}");
            assert!(w.state().is_none(), "no state written");
            assert_eq!(w.m().status(NOW).state, "installed-by-hand");
            assert_eq!(w.m().request(Action::Install, NOW).detail, "the agent is already installed");
            // a technician's explicit remove does reach it
            assert_eq!(w.m().request(Action::Remove, NOW).state, "removed-by-tether");
            assert_eq!(w.runs(), [vec!["uninstall"]]);
        }
    }

    #[test]
    fn a_technicians_removal_sticks_against_install_by_policy() {
        let w = world("sessionremove", "install");
        w.tick(NOW).unwrap();
        assert_eq!(w.m().request(Action::Remove, NOW + 5).state, "removed-by-tether");
        let n = w.runs().len();
        w.tick(NOW + 86_400).unwrap();
        assert_eq!(w.runs().len(), n);
        assert_eq!(w.m().request(Action::Install, NOW + 86_401).state, "installed");
    }

    #[test]
    fn the_minimum_version_upgrades_after_48_hours_and_never_down() {
        let w = world("floor", "install");
        w.tick(NOW).unwrap();
        *w.fake.versions.borrow_mut() = (Some("0.2.0".into()), Some("0.2.0".into()));
        w.publish(Pub { minimum: "0.3.0", serial: 6, ..agent("0.3.0") });
        let upgrades = || w.runs().iter().filter(|r| r[0] == "upgrade").count();
        let t0 = NOW + INDEX_EVERY;
        w.tick(t0).unwrap();
        assert_eq!(w.state().unwrap().below_since, t0);
        w.tick(t0 + INDEX_EVERY * 7).unwrap(); // 42 h
        assert_eq!(upgrades(), 0, "not before 48 h");
        w.tick(t0 + FLOOR_GRACE).unwrap();
        assert_eq!(upgrades(), 1);
        assert_eq!(w.fake.runs.borrow().last().unwrap().0, STAGED_EXE);
        let st = w.state().unwrap();
        assert_eq!((st.version.as_str(), st.below_since), ("0.3.0", 0));
        // a minimum above what the index offers: no upgrade to the same or a lower version
        let w = world("floordown", "on-request");
        w.m().request(Action::Install, NOW);
        *w.fake.versions.borrow_mut() = (Some("0.2.0".into()), Some("0.2.0".into()));
        w.publish(Pub { minimum: "0.9.0", serial: 6, ..agent("0.2.0") });
        for h in [6, 60, 120] {
            w.tick(NOW + h * 3600).unwrap();
        }
        assert!(!w.runs().iter().any(|r| r[0] == "upgrade"));
        // a refused upgrade (pending, bad version) is failed with backoff
        let w = world("floorrefused", "install");
        w.tick(NOW).unwrap();
        *w.fake.versions.borrow_mut() = (Some("0.2.0".into()), Some("0.2.0".into()));
        w.publish(Pub { minimum: "0.3.0", serial: 6, ..agent("0.3.0") });
        w.fake.codes.borrow_mut().insert("upgrade", (1, "a version is pending; nothing was changed".into()));
        w.tick(NOW + INDEX_EVERY).unwrap();
        w.tick(NOW + INDEX_EVERY + FLOOR_GRACE).unwrap();
        let st = w.state().unwrap();
        assert_eq!((st.state.as_str(), st.detail.as_str()), ("failed", "a version is pending; nothing was changed"));
        assert_eq!(st.next_try, NOW + INDEX_EVERY + FLOOR_GRACE + 3600);
    }

    #[test]
    fn not_enrolled_re_enrols_at_most_once_a_day_and_a_new_grant_once() {
        let w = world("reenrol", "install");
        w.tick(NOW).unwrap();
        w.fake.not_enrolled.set(true);
        let enrols = || w.fake.runs.borrow().iter().filter(|r| r.1[0] == "enrol").cloned().collect::<Vec<_>>();
        w.tick(NOW + 100).unwrap();
        assert!(enrols().is_empty(), "the install enrolled it less than a day ago");
        w.tick(NOW + ENROL_EVERY).unwrap();
        let e = enrols();
        assert_eq!(e.len(), 1);
        assert_eq!((e[0].0.as_str(), e[0].1.clone()), (AGENT_EXE, vec!["enrol".to_owned(), "--from-tether".into(), "-".into()]));
        assert!(e[0].2.as_deref().unwrap().contains(&w.token));
        w.tick(NOW + ENROL_EVERY + 3600).unwrap();
        assert_eq!(enrols().len(), 1, "once a day");
        // not in on-request
        let mut p = w.p.clone();
        p.mode = "on-request".into();
        Manager { sys: &w.fake, p: p.clone() }.tick(NOW + 3 * ENROL_EVERY).unwrap();
        assert_eq!(enrols().len(), 1);
        // a new grant: once
        w.fake.not_enrolled.set(false);
        let (token2, id2) = grant(&w.k, true);
        let mut p = w.p.clone();
        p.grant = token2.clone();
        let m = Manager { sys: &w.fake, p };
        m.tick(NOW + 4 * ENROL_EVERY).unwrap();
        m.tick(NOW + 4 * ENROL_EVERY + 60).unwrap();
        assert_eq!(enrols().len(), 2);
        assert!(enrols()[1].2.as_deref().unwrap().contains(&token2));
        assert_eq!(w.state().unwrap().grant_id, id2);
    }

    #[test]
    fn nothing_runs_from_a_bad_index_or_file_and_the_state_names_why() {
        let k_other = keys();
        let cases: Vec<(&str, Box<dyn Fn(&World)>)> = vec![
            ("expired", Box::new(|w: &World| w.publish(Pub { expires: NOW - 1, ..agent("0.2.0") }))),
            ("halted", Box::new(|w: &World| w.publish(Pub { halted: true, ..agent("0.2.0") }))),
            ("older than the accepted", Box::new(|w: &World| {
                std::fs::create_dir_all(&w.p.dir).unwrap();
                std::fs::write(w.p.dir.join("monitor-stable.serial"), format!("9 {}", tether_trust::key_to_b64(&w.k.admin.verifying_key()))).unwrap();
            })),
            ("untrusted key", Box::new(move |w: &World| *w.fake.docs.borrow_mut() = vec![doc(&k_other, &agent("0.2.0"))])),
            ("signed as file", Box::new(|w: &World| w.publish(Pub { signed_file: Some("other.exe".into()), ..agent("0.2.0") }))),
            ("unexpected file", Box::new(|w: &World| w.publish(Pub { file: Some("tether-monitor-0.2.0.exe.sig".into()), signed_file: Some("tether-monitor-0.2.0.exe.sig".into()), ..agent("0.2.0") }))),
            ("sha256", Box::new(|w: &World| w.publish(Pub { sha: Some("0".repeat(64)), ..agent("0.2.0") }))),
            // index, size and SHA-256 fine; the minisign signature made with another key
            ("signature", Box::new(|w: &World| w.publish(Pub { foreign_sig: true, ..agent("0.2.0") }))),
            ("no relay answered", Box::new(|w: &World| w.fake.docs.borrow_mut().clear())),
            ("changed before it ran", Box::new(|w: &World| w.fake.tamper.set(true))),
            ("junction", Box::new(|w: &World| *w.fake.refuse_dir.borrow_mut() = Some("health-agent is a junction or link: refused".into()))),
            ("not one for Tether", Box::new(|_: &World| {})),
        ];
        for (why, setup) in cases {
            let mut w = world("bad", "install");
            if why == "not one for Tether" {
                w.p.grant = grant(&w.k, false).0;
            }
            setup(&w);
            if why == "junction" {
                // the state folder itself refused: the tick stops before anything
                assert!(w.tick(NOW).unwrap_err().contains("junction"));
                assert!(w.runs().is_empty());
                continue;
            }
            w.tick(NOW).unwrap();
            assert!(w.runs().is_empty(), "{why}: {:?}", w.runs());
            let st = w.state().unwrap();
            assert_eq!(st.state, "failed", "{why}");
            assert!(st.detail.contains(why), "{why}: {}", st.detail);
            // nor from a session
            let a = w.m().request(Action::Install, NOW + 1);
            assert!(w.runs().is_empty(), "{why}");
            assert!(a.detail.contains(why), "{why}: {}", a.detail);
        }
    }

    #[test]
    fn a_planted_download_folder_is_refused() {
        let w = world("planted", "install");
        // the state folder is fine, the download folder is not
        struct DlRefused<'a>(&'a Fake);
        impl Sys for DlRefused<'_> {
            fn service_process(&self) -> bool { true }
            fn check_dir(&self, d: &Path) -> Result<(), String> { self.0.check_dir(d) }
            fn file_trusted(&self, p: &Path) -> bool { self.0.file_trusted(p) }
            fn own_identity(&self) -> Option<(String, [u8; 32])> { self.0.own_identity() }
            fn service_exists(&self) -> Result<bool, String> { self.0.service_exists() }
            fn versions(&self) -> (Option<String>, Option<String>) { self.0.versions() }
            fn not_enrolled(&self) -> bool { false }
            fn removed_at(&self) -> Option<u64> { None }
            fn secure_dir(&self, dir: &Path) -> Result<(), String> {
                if dir.ends_with(DOWNLOAD_DIR) { Err("health-agent is owned by S-1-5-21-1, not SYSTEM or Administrators: refused".into()) } else { self.0.secure_dir(dir) }
            }
            fn fetch_index(&self, c: &str) -> Vec<(String, Vec<u8>)> { self.0.fetch_index(c) }
            fn download(&self, c: &Candidate, k: &str, w: Duration) -> Result<Vec<u8>, String> { self.0.download(c, k, w) }
            fn open_locked(&self, p: &Path) -> Result<(Vec<u8>, Box<dyn std::any::Any>), String> { self.0.open_locked(p) }
            fn run(&self, e: &Path, a: &[&str], s: Option<&str>, t: Duration) -> Result<Ran, String> { self.0.run(e, a, s, t) }
            fn installed_exe(&self) -> PathBuf { self.0.installed_exe() }
            fn write_public(&self, p: &Path, d: &[u8]) -> Result<(), String> { self.0.write_public(p, d) }
        }
        let sys = DlRefused(&w.fake);
        Manager { sys: &sys, p: w.p.clone() }.tick(NOW).unwrap();
        assert!(w.runs().is_empty());
        assert!(w.state().unwrap().detail.contains("not SYSTEM or Administrators"));
    }

    #[test]
    fn the_rollout_bucket_applies_to_tick_not_to_request() {
        let w = world("rollout", "install");
        w.publish(Pub { rollout: vec![(0, 0)], ..agent("0.2.0") });
        w.tick(NOW).unwrap();
        assert!(w.runs().is_empty());
        assert!(w.state().is_none(), "waiting is not a failure");
        assert_eq!(w.m().request(Action::Install, NOW).state, "installed");
        assert_eq!(w.state().unwrap().by, "session");
    }

    #[test]
    fn requests_follow_the_policy() {
        // remove from a session in on-request and install
        for mode in ["on-request", "install"] {
            let w = world("reqremove", mode);
            assert_eq!(w.m().request(Action::Install, NOW).state, "installed");
            assert_eq!(w.m().request(Action::Remove, NOW + 1).state, "removed-by-tether");
            assert_eq!(w.runs().last().unwrap(), &vec!["uninstall".to_owned(), "--by-tether".into()]);
        }
        // the policy removes: no install from a session
        let w = world("reqpolicyremove", "remove");
        let a = w.m().request(Action::Install, NOW);
        assert!(w.runs().is_empty());
        assert!(a.detail.contains("removes"), "{}", a.detail);
        // on-request installs nothing by itself
        let w = world("onrequest", "on-request");
        w.tick(NOW).unwrap();
        assert!(w.runs().is_empty() && w.fake.fetches.get() == 0);
        assert_eq!(Action::parse("status"), Some(Action::Status));
        assert_eq!(Action::parse("rm -rf"), None);
    }

    #[test]
    fn no_policy_check_starts_anything_outside_the_service_in_any_mode() {
        // The elevated path (agent_elevated.rs) is reachable only from a session request or the
        // local click: a policy check outside the service runs, fetches and asks nothing.
        for mode in ["manual", "on-request", "install", "remove"] {
            let mut w = world("tick-user", mode);
            w.fake.user = true;
            let _ = w.tick(NOW);
            assert!(w.runs().is_empty() && w.fake.fetches.get() == 0 && w.files().is_empty(), "{mode}");
        }
    }

    #[test]
    fn the_public_copy_says_only_whether_by_whom_and_when() {
        let w = world("public", "install");
        w.tick(NOW).unwrap();
        let raw = std::fs::read(w.p.dir.join(PUBLIC_FILE)).unwrap();
        let text = String::from_utf8_lossy(&raw).into_owned();
        assert!(!text.contains(&w.token) && !text.contains("ok") && !text.contains(&w.grant_id), "{text}");
        assert_eq!(read_public(&raw), Some(AgentAnswer { state: "installed".into(), version: "0.2.0".into(), installed_by: "policy".into(), detail: String::new(), at: NOW }));
        // a copy with anything else in it is not shown
        assert_eq!(read_public(br#"{"state":"installed","version":"0.2.0 <b>","installed_by":"policy","detail":"x","at":1}"#), None);
        assert_eq!(read_public(br#"{"state":"pwned","version":"","installed_by":"","detail":"","at":1}"#), None);
        assert_eq!(read_public(br#"{"state":"installed","version":"","installed_by":"policy","detail":"call 0900","at":1}"#).unwrap().detail, "");
        // a technician's removal is public too
        assert_eq!(w.m().request(Action::Remove, NOW + 10).state, "removed-by-tether");
        assert_eq!(read_public(&std::fs::read(w.p.dir.join(PUBLIC_FILE)).unwrap()).unwrap().state, "removed-by-tether");
    }

    #[test]
    fn not_local_system_needs_the_service() {
        let mut w = world("user", "install");
        w.fake.user = true;
        assert_eq!(w.tick(NOW).unwrap_err(), NEEDS_SERVICE);
        for a in [Action::Status, Action::Install, Action::Remove] {
            assert_eq!(w.m().request(a, NOW).state, "needs-service");
        }
        assert!(w.runs().is_empty() && w.files().is_empty() && w.fake.fetches.get() == 0);
    }

    #[test]
    fn a_refused_state_folder_shows_only_the_reason() {
        let w = world("refusedstatus", "install");
        std::fs::create_dir_all(&w.p.dir).unwrap();
        std::fs::write(w.p.dir.join(STATE_FILE), br#"{"state":"removed-locally","hold":"removed-locally","detail":"call 0900 123"}"#).unwrap();
        *w.fake.refuse_dir.borrow_mut() = Some("Tether's state folder is owned by S-1-5-21-9, not SYSTEM or Administrators: refused".into());
        let a = w.m().status(NOW);
        assert_eq!((a.state.as_str(), a.detail.as_str()), ("failed", "Tether's state folder is owned by S-1-5-21-9, not SYSTEM or Administrators: refused"));
        // a request hears the same at once, before anything happens
        for act in [Action::Install, Action::Remove] {
            assert_eq!(w.m().request(act, NOW), a);
        }
        assert!(w.runs().is_empty() && w.fake.fetches.get() == 0);
    }

    #[test]
    fn files_a_user_planted_in_the_state_folder_are_ignored_and_replaced() {
        let w = world("plantedfiles", "install");
        let key = tether_trust::key_to_b64(&w.k.admin.verifying_key());
        let state = w.p.dir.join(STATE_FILE);
        w.fake.plant(&state, br#"{"state":"removed-locally","hold":"removed-locally","detail":"planted"}"#);
        w.fake.plant(&w.p.dir.join("monitor-stable.serial"), format!("9 {key}").as_bytes());
        assert_eq!(w.m().status(NOW).state, "not-installed", "the planted state is not read");
        w.tick(NOW).unwrap();
        assert_eq!(w.state().unwrap().state, "installed", "neither the planted hold nor the planted floor counted");
        assert!(w.fake.file_trusted(&state), "replaced by Tether's own file");
        // a staged file a user owns is not run
        let w = world("plantedstaged", "install");
        w.fake.plant_staged.set(true);
        w.tick(NOW).unwrap();
        assert!(w.runs().is_empty(), "{:?}", w.runs());
        let st = w.state().unwrap();
        assert_eq!(st.state, "failed");
        assert!(st.detail.contains("not SYSTEM or Administrators"), "{}", st.detail);
    }

    #[test]
    fn only_a_canonical_identity_and_a_valid_id_are_signed() {
        let pk = agent_pubkey();
        let good = format!("{{\"uuid\":\"{UUID}\",\"pubkey\":\"{pk}\"}}");
        for (out, id) in [
            (format!("{{\"uuid\":\"{UUID}\",\"pubkey\":\"{pk} \"}}"), ID),
            (format!("{good}\n{good}"), ID),
            (format!("{{\"uuid\":{}\"{UUID}\",\"pubkey\":\"{pk}\"}}", " ".repeat(600)), ID),
            (good.clone(), "bad id"),
            (good.clone(), ""),
        ] {
            let w = world("identity", "install");
            *w.fake.identity.borrow_mut() = Some(out.clone());
            *w.fake.own_id.borrow_mut() = Some(id.into());
            w.tick(NOW).unwrap();
            assert!(!w.runs().iter().any(|r| r[0] == "install"), "{out:?} {id:?}: {:?}", w.runs());
            assert_eq!(w.state().unwrap().state, "failed", "{out:?} {id:?}");
        }
    }

    #[test]
    fn an_unparseable_removal_time_is_now_in_the_status() {
        let w = world("removedzero", "install");
        w.fake.removed_at.set(Some(0));
        let a = w.m().status(NOW);
        assert_eq!((a.state.as_str(), a.at), ("removed-locally", NOW));
    }

    #[test]
    fn only_the_services_server_process_manages_the_agent() {
        assert!(service_gate(Some("--server"), true));
        for (arg, system) in [(Some("--server"), false), (Some("--cm"), true), (None, true), (Some("--service"), true), (Some("--tray"), true)] {
            assert!(!service_gate(arg, system), "{arg:?} {system}");
        }
    }

    #[test]
    fn the_download_has_a_total_deadline() {
        let w = world("deadline", "install");
        w.tick(NOW).unwrap();
        assert_eq!(*w.fake.within.borrow(), [Duration::from_secs(120)]);
        assert_eq!(download_time(5 << 20), Duration::from_secs(200));
        assert_eq!(download_time(release::MAX_ARTIFACT), Duration::from_secs(600));
        // the deadline covers every source: none is tried once it has passed
        let c = Candidate { urls: vec!["https://127.0.0.1:9/a.exe".into(), "https://127.0.0.1:9/b.exe".into()], ..Default::default() };
        let e = crate::update::download_within(&c, "k", Some(Duration::ZERO)).unwrap_err();
        assert!(e.contains("did not finish within 0 s"), "{e}");
    }

    #[test]
    fn the_answer_holds_no_grant_proof_or_profile_path() {
        let w = world("tidy", "on-request");
        let line = format!("\x1b[31mfailed\x07 with {} at C:\\Users\\Jane Doe\\AppData\\x.log {}", w.token, "x ".repeat(400));
        w.fake.codes.borrow_mut().insert("install", (1, line));
        let a = w.m().request(Action::Install, NOW);
        assert_eq!(a.state, "failed");
        for d in [&a.detail, &w.m().status(NOW).detail, &w.state().unwrap().detail] {
            assert!(d.chars().count() <= 300 && !d.chars().any(char::is_control), "{d:?}");
            assert!(d.contains("failed") && !d.contains(&w.token[..40]) && !d.contains("Jane") && !d.to_lowercase().contains("users"), "{d:?}");
        }
        assert_eq!(tidy(&format!("proof {} sha256 {}", "A+/".repeat(30), "0".repeat(64))), format!("proof [removed] sha256 {}", "0".repeat(64)));
        assert_eq!(tidy(&"ä".repeat(400)).chars().count(), 300);
    }

    /// The tests that hold the process-wide running slot, one at a time.
    static SLOT: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn wait_free() {
        let t = std::time::Instant::now();
        while running().is_some() && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_long_action_runs_to_its_end_and_the_next_one_hears_busy() {
        let _one = SLOT.lock().unwrap_or_else(|e| e.into_inner());
        wait_free();
        // The real outcome, however long it takes (the core answers the session `running` meanwhile).
        let long = std::thread::spawn(|| {
            exclusive("installing the agent", || {
                std::thread::sleep(Duration::from_millis(400));
                AgentAnswer::only("installed", "done")
            })
        });
        let t = std::time::Instant::now();
        while running().is_none() && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
        let b = exclusive("removing the agent", || AgentAnswer::only("removed-by-tether", ""));
        assert_eq!((b.state.as_str(), b.detail.as_str()), ("busy", "Tether is installing the agent; ask for the status later"));
        assert_eq!(busy_now(*running()).map(|a| a.detail), Some("Tether is installing the agent; ask for the status later".into()), "status meanwhile");
        assert_eq!(long.join().unwrap(), AgentAnswer::only("installed", "done"));
        assert_eq!(exclusive("removing the agent", || AgentAnswer::only("removed-by-tether", "")).state, "removed-by-tether");
        // a panic inside does not leave it busy
        assert!(std::panic::catch_unwind(|| exclusive("installing the agent", || panic!("boom"))).is_err());
        assert!(running().is_none());
        assert!(Running::start("checking").is_ok());
    }

    #[test]
    fn status_during_a_policy_check_answers_from_the_saved_state_until_it_runs_the_agent() {
        let _one = SLOT.lock().unwrap_or_else(|e| e.into_inner());
        wait_free();
        let slot = Running::start(TICK).unwrap();
        assert_eq!(busy_now(*running()), None, "only checking: the saved state answers");
        child("installing the agent");
        assert_eq!(busy_now(*running()).map(|a| a.state), Some("busy".into()));
        assert_eq!(*running(), Some("installing the agent"));
        drop(slot);
        // a session's action is never renamed by the policy's
        let s = Running::start("removing the agent").unwrap();
        child("upgrading the agent");
        assert_eq!(*running(), Some("removing the agent"));
        drop(s);
        assert_eq!(busy_now(None), None);
    }

    #[test]
    fn an_install_tether_stopped_in_is_adopted_once_the_service_is_there() {
        let w = world("pending", "install");
        w.fake.codes.borrow_mut().insert("install", (-99, String::new()));
        let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| w.m().request(Action::Install, NOW)));
        assert!(stopped.is_err());
        let st = w.state().unwrap();
        assert_eq!(st.pending_install, Some(Pending { at: NOW, by: "session".into() }), "saved before the run");
        assert!(!st.ours);
        assert_eq!(w.m().status(NOW + 30).installed_by, "session", "status reads it the same way");
        w.fake.codes.borrow_mut().clear();
        w.tick(NOW + 60).unwrap();
        let st = w.state().unwrap();
        assert_eq!((st.state.as_str(), st.by.as_str(), st.ours, st.pending_install.is_none()), ("installed", "session", true, true));
        // Tether's agent now: it is enrolled with the policy's grant (not left as one installed by hand)
        assert!(w.runs().iter().any(|r| r[0] == "enrol"), "{:?}", w.runs());
        assert_eq!(st.grant_id, w.grant_id);
        assert_eq!(w.m().status(NOW + 70).installed_by, "session");
        // by policy, adopted through a session request too
        let w = world("pending-policy", "install");
        w.fake.codes.borrow_mut().insert("install", (-99, String::new()));
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| w.tick(NOW))).is_err());
        assert_eq!(w.state().unwrap().pending_install.map(|p| p.by), Some("policy".into()));
        w.fake.codes.borrow_mut().clear();
        assert_eq!(w.m().request(Action::Remove, NOW + 10).state, "removed-by-tether");
        assert_eq!(w.state().unwrap().pending_install, None);
    }

    #[test]
    fn an_install_that_never_finished_is_recorded_as_interrupted() {
        let w = world("interrupted", "install");
        w.fake.codes.borrow_mut().insert("install", (-98, String::new()));
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| w.tick(NOW))).is_err());
        w.fake.codes.borrow_mut().clear();
        let n = w.runs().len();
        // it may still be running: no second install, by policy or in a session
        w.tick(NOW + 10).unwrap();
        assert_eq!(w.runs().len(), n);
        assert!(w.m().request(Action::Install, NOW + 20).detail.contains("may still be running"));
        assert_eq!(w.runs().len(), n);
        assert!(w.state().unwrap().pending_install.is_some());
        // past the install's own limit: failed, interrupted, the field cleared
        w.tick(NOW + PENDING_MAX).unwrap();
        let st = w.state().unwrap();
        assert_eq!((st.state.as_str(), st.pending_install.is_none()), ("failed", true));
        assert!(st.detail.contains("interrupted"), "{}", st.detail);
        assert_eq!(w.runs().len(), n, "backs off before the next try");
        assert_eq!(w.m().status(NOW + PENDING_MAX).state, "failed");
    }

    #[test]
    fn the_pending_mark_is_stamped_when_the_install_child_starts() {
        let w = world("pending-late", "install");
        w.fake.download_takes.set(600);
        w.fake.codes.borrow_mut().insert("install", (-98, String::new()));
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| w.tick(NOW))).is_err());
        assert_eq!(w.state().unwrap().pending_install.map(|p| p.at), Some(NOW + 600));
        w.fake.codes.borrow_mut().clear();
        // 4 minutes after the child started, less than PENDING_MAX after it: still running, not interrupted
        let n = w.runs().len();
        w.tick(NOW + 600 + 200).unwrap();
        let st = w.state().unwrap();
        assert!(st.pending_install.is_some() && st.state != "failed", "{st:?}");
        assert_eq!(w.runs().len(), n, "no second installer beside it");
    }

    #[test]
    fn an_old_mark_claims_nothing_and_an_inactive_mode_drops_a_stale_one() {
        let w = world("pending-old", "install");
        std::fs::create_dir_all(&w.p.dir).unwrap();
        std::fs::write(w.p.dir.join(STATE_FILE), br#"{"pending_install":{"at":1900000000,"by":"session"}}"#).unwrap();
        // the practice installed an agent by hand weeks later
        w.fake.svc.set(true);
        w.tick(NOW + PENDING_MAX + ADOPT_GRACE + 3600).unwrap();
        let st = w.state().unwrap_or_default();
        assert!(!st.ours && st.pending_install.is_none(), "{st:?}");
        assert_eq!(w.m().status(NOW + PENDING_MAX + ADOPT_GRACE + 3600).state, "installed-by-hand");
        // a young mark still adopts
        let w = world("pending-young", "install");
        std::fs::create_dir_all(&w.p.dir).unwrap();
        std::fs::write(w.p.dir.join(STATE_FILE), br#"{"pending_install":{"at":1900000000,"by":"session"}}"#).unwrap();
        w.fake.svc.set(true);
        w.tick(NOW + 600).unwrap();
        assert!(w.state().unwrap().ours);
        // manual: a stale mark is dropped, nothing else happens
        let w = world("pending-manual", "manual");
        std::fs::create_dir_all(&w.p.dir).unwrap();
        std::fs::write(w.p.dir.join(STATE_FILE), br#"{"pending_install":{"at":1900000000,"by":"session"}}"#).unwrap();
        w.tick(NOW + PENDING_MAX).unwrap();
        assert_eq!(w.state().unwrap().pending_install, None);
        assert!(w.runs().is_empty() && w.fake.fetches.get() == 0);
    }

    #[test]
    fn an_install_that_may_still_run_answers_busy_and_records_no_failure() {
        let w = world("pending-busy", "install");
        w.fake.codes.borrow_mut().insert("install", (-98, String::new()));
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| w.m().request(Action::Install, NOW))).is_err());
        w.fake.codes.borrow_mut().clear();
        let before = w.state().unwrap();
        let a = w.m().request(Action::Install, NOW + 20);
        assert_eq!((a.state.as_str(), a.detail.as_str()), ("busy", MAY_STILL_RUN));
        assert_eq!(w.state().unwrap(), before, "saved state untouched");
        assert_ne!(w.m().status(NOW + 30).state, "failed");
    }

    #[cfg(windows)]
    #[test]
    fn run_keeps_the_last_output_and_does_not_wait_for_a_grandchild() {
        let cmd = Path::new(r"C:\Windows\System32\cmd.exe");
        let r = win::run(cmd, &["/c", "for /L %i in (1,1,3000) do @echo line %i"], None, Duration::from_secs(60)).unwrap();
        assert!(r.out.len() <= 4096, "{}", r.out.len());
        assert_eq!(r.out.lines().last(), Some("line 3000"));
        // a grandchild keeps the pipes open for 30 s after cmd exits
        let t = std::time::Instant::now();
        let r = win::run(cmd, &["/c", "start /b ping -n 30 127.0.0.1 & echo done"], None, Duration::from_secs(60)).unwrap();
        assert!(t.elapsed() < Duration::from_secs(15), "{:?}", t.elapsed());
        assert!(r.out.contains("done"), "{r:?}");
    }

    #[cfg(not(windows))]
    #[test]
    fn the_features_are_ignored_off_windows() {
        let bundle = r#"{"serial":1,"issued_at":0,"profiles":{"technician":{"health.agent":{"state":"locked","value":"install"}}}}"#;
        let b = tether_trust::policy::Bundle::from_json(bundle.as_bytes()).unwrap();
        let eff = tether_trust::policy::resolve(Some(&b), crate::policy::PROFILE, "u", None);
        assert_eq!(eff.get("health.agent"), "install");
        tick(&eff, NOW);
        assert_eq!(request(Action::Install).state, "not-supported");
        assert_eq!(status().state, "not-supported");
    }

    #[cfg(windows)]
    #[test]
    fn a_locked_file_runs_but_cannot_be_changed_and_junctions_are_refused() {
        let dir = std::env::temp_dir().join(format!("tether-agent-win-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let _cleanup = TempDir(dir.clone());
        let exe = dir.join("probe.exe");
        std::fs::copy(r"C:\Windows\System32\where.exe", &exe).unwrap();
        let (bytes, lock) = win::open_locked(&exe).unwrap();
        assert_eq!(bytes, std::fs::read(&exe).unwrap());
        assert!(std::fs::OpenOptions::new().write(true).open(&exe).is_err(), "no writing while locked");
        assert!(std::fs::remove_file(&exe).is_err(), "no deleting while locked");
        assert!(std::fs::rename(&exe, dir.join("other.exe")).is_err(), "no renaming while locked");
        let r = win::run(&exe, &["/?"], None, Duration::from_secs(30)).unwrap();
        assert_eq!(r.code, 0, "{r:?}");
        drop(lock);
        std::fs::remove_file(&exe).unwrap();
        // a junction in place of the folder
        let target = dir.join("target");
        std::fs::create_dir(&target).unwrap();
        let link = dir.join("junction");
        let ok = std::process::Command::new("cmd").args(["/c", "mklink", "/J"]).arg(&link).arg(&target).output().unwrap().status.success();
        assert!(ok);
        assert!(win::secure_dir(&link).unwrap_err().contains("junction"));
        assert!(win::trusted_owner("S-1-5-18") && win::trusted_owner("S-1-5-32-544"));
        assert!(!win::trusted_owner("S-1-5-32-545") && !win::trusted_owner("S-1-5-21-1-2-3-1001"));
        assert!(win::owner_sid(&target).unwrap().starts_with("S-1-"));
    }
}

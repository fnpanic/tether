//! The health monitoring agent installed and removed through a Windows elevation prompt, by a Tether
//! without its LocalSystem service and by TetherQS (spec 11). One file for both apps: the desktop has
//! it as `crate::agent_elevated`, TetherQS includes it with `#[path]` (as `diag_bundle.rs`). Also the
//! pieces the service's manager (`agent.rs`) shares: the answer, the index check, `identity`'s line.
//!
//! Only an explicit action starts it: a technician's request in a session ([`Host::request`] with
//! the viewer's name) or the local person's click (no name). Never a policy check, in any mode.
//! First the pre-dialog in this program's own window (remote input never reaches Tether's windows,
//! `own_ui_guard`); "Don't install", closing it or no answer within [`CONSENT_WAIT`] ends it with
//! nothing downloaded or run. Then: the `monitor/windows-x86_64` entry of an admin-signed,
//! unexpired, not halted `monitor.json` at or above the floor, downloaded and verified (size, SHA-256,
//! minisign with the compiled-in updater key, trusted comment) into `%LOCALAPPDATA%\<app>\health-agent\`
//! under a fixed name, opened with a handle that denies writing and deleting, verified again through
//! it, and held until the elevated process has exited. That file alone is started with `runas`,
//! directly by this process (the agent checks the pipe's server is its parent), with the fixed
//! arguments `install --from-tether pipe:<name>`. The pipe (`tether-agent-<32 hex from the OS random
//! source>`, first and only instance, this user and Administrators only) gives the token, the proof
//! and the raw signed index only to the process this code launched, after `identity` arrived on it.
//! The elevated agent verifies its own copy against that index before it creates its service.
//! Removal: the installed agent's `uninstall --by-tether` (fixed path in the program folder), after
//! the same kind of dialog.
//!
//! The pre-dialog is not a boundary against a technician who controls the desktop (the On-Screen
//! Keyboard or any program run as the user can operate it); the Windows prompt on the secure desktop
//! is. Where Windows would not prompt (this process elevated, UAC off, administrators elevated
//! without asking, TetherQS's elevated service), a technician's install or removal is refused and
//! only the person at the computer can start it ([`Uac::silent`]).
#![cfg_attr(not(windows), allow(dead_code))]

use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, AtomicU8, Ordering},
        mpsc, Mutex, MutexGuard, TryLockError,
    },
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tether_trust::{
    release::{self, Candidate},
    status, VerifyingKey,
};

/// The agent's Windows service.
pub const SERVICE: &str = "TetherMonitor";
/// The agent's folder name under `%ProgramFiles%` and `%ProgramData%`.
pub const AGENT_FOLDER: &str = "Tether Monitor";
pub const AGENT_EXE: &str = "tether-monitor.exe";
/// App and release key in `monitor.json`.
pub const APP: &str = "monitor";
/// The verified download is always staged under this name: no file name from the network is run.
pub const STAGED_EXE: &str = "health-agent.exe";
/// The per-user folder's staging subfolder and state file (no secret in it).
pub const STAGE_DIR: &str = "health-agent";
pub const USER_STATE: &str = "health-agent.json";

/// The pre-dialog waits this long for the person at the computer (spec 11.2).
pub const CONSENT_WAIT: Duration = Duration::from_secs(120);
/// After the prompt was approved: the agent copies itself, then connects.
pub const CONNECT_WAIT: Duration = Duration::from_secs(60);
/// Reading `identity` and writing the line.
pub const EXCHANGE_WAIT: Duration = Duration::from_secs(30);
/// The elevated agent's whole run after the prompt (install: enrolment and the service start).
pub const EXIT_WAIT: Duration = Duration::from_secs(240);
/// `identity`'s line is small.
pub const IDENTITY_MAX: usize = 512;
/// The agent's result line (`{"ok":…,"line":…}`, its line at most 300 characters).
pub const RESULT_MAX: usize = 2048;
/// After a "no" (Don't install, a declined prompt, no answer) a technician's next ask waits this long.
pub const ASK_AGAIN_AFTER: u64 = 600;
/// A pending record younger than this may still be running: the prompt (Windows closes it after
/// about 2 minutes), the agent's connect, the exchange and its run, and a margin.
pub const PENDING_MAX: u64 = 120 + CONNECT_WAIT.as_secs() + EXCHANGE_WAIT.as_secs() + EXIT_WAIT.as_secs() + 60;
/// Past [`PENDING_MAX`] by this, a record claims nothing (an agent there now may be the practice's).
pub const ADOPT_GRACE: u64 = 86_400;
/// The first agent with `uninstall --by-tether` (and the Apps entry that names its version).
pub const BY_TETHER_SINCE: &str = "0.2.0";

/// The whole download, every source together: 2 minutes plus a second per 64 KiB of the signed
/// size, at most 10 minutes (the dialog was answered already; nobody should wait longer).
pub fn download_time(size: u64) -> Duration {
    Duration::from_secs((120 + size / 65_536).min(600))
}

// ----------------------------------------------------------------------------- shared interface

/// A technician's request in a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Status,
    Install,
    Remove,
}

impl Action {
    /// `status`, `install`, `remove`; anything else is None.
    #[allow(dead_code)] // the wire uses the core's enum
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "status" => Some(Self::Status),
            "install" => Some(Self::Install),
            "remove" => Some(Self::Remove),
            _ => None,
        }
    }
}

/// What the host answers. `state`: `not-installed`, `installed`, `installed-by-hand`,
/// `removed-locally`, `removed-by-tether`, `failed`, `not-allowed` (policy `manual`, or an install
/// while the policy removes), `needs-service` (not Tether's LocalSystem service), `not-supported`
/// (not Windows), `busy` (another action runs). `installed_by`: `policy`, `session` or empty;
/// `detail`: the agent's last line or the reason; `at`: unix seconds of the last change (0: none).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentAnswer {
    pub state: String,
    pub version: String,
    pub installed_by: String,
    pub detail: String,
    #[serde(default)]
    pub at: u64,
}

impl AgentAnswer {
    pub fn only(state: &str, detail: &str) -> Self {
        Self { state: state.into(), detail: detail.into(), ..Default::default() }
    }
}

pub fn active(mode: &str) -> bool {
    matches!(mode, "on-request" | "install" | "remove")
}

/// What goes to a remote technician: control characters out; a path under a user profile cut
/// from its word on; anything that could be a grant or a proof (a run of more than 64 base64
/// characters) replaced; at most 300 characters.
pub fn tidy(detail: &str) -> String {
    let mut s: String = detail.chars().filter(|c| c.is_control() || !invisible(*c)).map(|c| if c.is_control() { ' ' } else { c }).collect();
    let low = s.to_ascii_lowercase();
    if let Some(i) = ["\\users\\", "/users/"].iter().filter_map(|p| low.find(p)).min() {
        let start = s[..i].char_indices().rev().find(|(_, c)| c.is_whitespace()).map_or(0, |(w, c)| w + c.len_utf8());
        s.truncate(start);
        s.push_str("[path removed]");
    }
    let b64 = |c: char| c.is_ascii_alphanumeric() || "+/=_-.".contains(c);
    let (mut out, mut run) = (String::with_capacity(s.len()), String::new());
    let flush = |run: &mut String, out: &mut String| {
        out.push_str(if run.len() > 64 { "[removed]" } else { run });
        run.clear();
    };
    for c in s.chars() {
        if b64(c) {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out.trim().chars().take(300).collect()
}

/// `identity`'s line.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub uuid: String,
    pub pubkey: String,
}

pub fn parse_identity(out: &str) -> Result<Identity, String> {
    let bad = || "the agent's identity is not what Tether expects".to_owned();
    // One line of at most 512 bytes, the key exactly in its canonical encoding (it is signed as given).
    let mut lines = out.lines().map(str::trim).filter(|l| !l.is_empty());
    let line = lines.next().filter(|l| l.len() <= IDENTITY_MAX && lines.next().is_none()).ok_or_else(bad)?;
    let i: Identity = serde_json::from_str(line).map_err(|_| bad())?;
    if !status::valid_uuid(&i.uuid) || tether_trust::key_from_b64(&i.pubkey).map(|k| tether_trust::key_to_b64(&k)).ok().as_deref() != Some(i.pubkey.as_str()) {
        return Err(bad());
    }
    Ok(i)
}

/// The agent `monitor.json` offers: admin-signed for `channel`, at or above the floor kept in
/// `cache` (`.serial` beside it), not expired, not halted, the expected file. Also the raw signed
/// document it came from (the elevated agent verifies its copy against exactly these bytes).
pub fn candidate(docs: &[(String, Vec<u8>)], cache: &Path, admin: &[VerifyingKey], channel: &str, now: u64) -> Result<(Candidate, Vec<u8>), String> {
    let (base, ix) = release::choose_for(cache, docs, admin, channel, release::MONITOR_APPS)?.ok_or("no relay answered with the agent's index")?;
    if ix.stale(now) {
        return Err("the agent's index has expired".into());
    }
    let e = ix.releases.get(&format!("{APP}/{}", release::WINDOWS)).ok_or("the agent's index has no agent for Windows")?;
    if e.halted {
        return Err(format!("the agent {} is halted", e.version));
    }
    if !e.file.ends_with(".exe") || release::file_version(APP, release::WINDOWS, &e.file).as_deref() != Some(e.version.as_str()) {
        return Err("the agent's index names an unexpected file".into());
    }
    let raw = docs
        .iter()
        .find(|(b, raw)| *b == base && release::Index::open_for(raw, admin, channel, release::MONITOR_APPS).is_ok_and(|i| i.serial == ix.serial))
        .map(|(_, raw)| raw.clone())
        .ok_or("the agent's index came from no relay")?;
    Ok((Candidate::from_entry(e, &base, &ix), raw))
}

/// This process's Tether ID and the 32-byte seed of the key it registers with the rendezvous
/// server: its own config (the user's `Tether.toml`, TetherQS's `Tether-QS.toml`, the service's).
pub fn own_identity() -> Option<(String, [u8; 32])> {
    let id = hbb_common::config::Config::get_id();
    let (sk, _) = hbb_common::config::Config::get_key_pair();
    let seed: [u8; 32] = sk.get(..32)?.try_into().ok()?;
    (!id.is_empty() && sk.len() == 64).then_some((id, seed))
}

/// The policy's grant, verified with the admin keys and made for Tether's proof: `(token, id)`.
pub fn grant(token: &str, admin: &[VerifyingKey], now: u64) -> Result<(String, String), String> {
    let g = token.trim();
    if g.is_empty() {
        return Err("the policy has no enrolment grant".into());
    }
    let p = status::verify_token(g, admin, now).map_err(|e| format!("the policy's enrolment grant: {e}"))?;
    if !p.tether_proof {
        return Err("the policy's enrolment grant is not one for Tether".into());
    }
    Ok((g.to_owned(), p.id))
}

// ----------------------------------------------------------------------------- the pre-dialog

/// What the dialog asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Install,
    Remove,
}

/// Who asks, for what. `viewer`: the name the technician's Tether gave (None: the person at this
/// computer clicked); `org`: the product name; `support_org`: the support organisation's name from
/// the signed policy or custom app (empty: none known); `relays`: host names it reports to.
#[derive(Debug, Clone)]
pub struct Ask {
    pub kind: Kind,
    pub viewer: Option<String>,
    pub org: String,
    pub support_org: String,
    pub relays: Vec<String>,
    /// Windows will show its prompt (false only for the local person's own click: [`Uac::silent`]).
    pub prompt: bool,
}

/// The pre-dialog as the window shows it: plain text only.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Dialog {
    pub id: u64,
    pub title: String,
    pub paragraphs: Vec<String>,
    pub yes: String,
    pub no: String,
}

/// Control, bidi, line-separator and zero-width characters: they can reorder or hide what is shown.
/// The front ends drop the same set (`plainText` in AgentHere.tsx and TetherQS's App.tsx).
pub fn invisible(c: char) -> bool {
    c.is_control() || matches!(c as u32, 0x061c | 0x200b..=0x200f | 0x2028..=0x202e | 0x2060..=0x2064 | 0x2066..=0x2069 | 0xfeff)
}

/// A name from the other side: no [`invisible`] characters, at most 64 characters.
fn shown_name(s: &str) -> String {
    let s: String = s.chars().filter(|c| !invisible(*c)).take(64).collect();
    s.trim().to_owned()
}

/// The texts of spec 11.2 (install) and 11.6 (remove).
pub fn dialog(id: u64, a: &Ask) -> Dialog {
    let org = shown_name(&a.org);
    let support = if org.is_empty() { "your organisation's support".to_owned() } else { format!("support of {org}") };
    let session = a.viewer.is_some();
    let who = if session {
        format!("A technician ({support})")
    } else if org.is_empty() {
        "Your organisation".to_owned()
    } else {
        format!("Your organisation ({org})")
    };
    // On its own line, never inside a sentence it could rewrite.
    let named = a.viewer.as_deref().map(shown_name).filter(|n| !n.is_empty()).map(|n| format!("Name given by the technician (not verified): {n}"));
    let relays: Vec<String> = a.relays.iter().map(|r| shown_name(r)).filter(|r| !r.is_empty()).collect();
    let to = if relays.is_empty() { "your organisation's servers".to_owned() } else { relays.join(", ") };
    let next = if a.prompt { "Windows will ask for administrator approval next." } else { "No Windows prompt will follow: this computer gives administrator rights without asking." };
    let stays = if session { "stays on this computer after this session" } else { "stays on this computer" };
    let (title, ask, yes, no) = match a.kind {
        Kind::Install => ("Install the health monitoring program?", format!("{who} asks to install Tether Monitor."), "Install", "Don't install"),
        Kind::Remove => ("Remove the health monitoring program?", format!("{who} asks to remove Tether Monitor from this computer. It stops sending information."), "Remove", "Don't remove"),
    };
    let org_line = Some(shown_name(&a.support_org)).filter(|n| !n.is_empty()).map(|n| format!("Your support organisation: {n}"));
    let mut paragraphs: Vec<String> = std::iter::once(ask).chain(org_line).chain(named).collect();
    if a.kind == Kind::Install {
        paragraphs.push(format!("It runs in the background as a Windows service, {stays}, and sends information about the computer's state and installed software to {to}."));
        paragraphs.push("What it sends is listed in Tether's privacy policy (PRIVACY.md in Tether's documentation, rows “Health report” and “Software list”).".into());
        paragraphs.push("You can remove it at any time in Settings › Apps › Tether Monitor.".into());
    }
    paragraphs.push(next.into());
    Dialog { id, title: title.into(), paragraphs, yes: yes.into(), no: no.into() }
}

/// How the pre-dialog ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Consent {
    Yes,
    No,
    NoAnswer,
    /// No window could show it (a process without this program's window).
    NoWindow,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
/// The dialog waiting for an answer (one at a time) and where its answer goes.
static PENDING: Mutex<Option<(Dialog, mpsc::Sender<bool>)>> = Mutex::new(None);

fn pending_slot() -> std::sync::MutexGuard<'static, Option<(Dialog, mpsc::Sender<bool>)>> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Puts `d` up for the window, calls `show` (bring the window up, tell it), and waits at most `wait`
/// for [`answer`]. The window reads it with [`pending`] (also after a reload).
pub fn ask_window(d: Dialog, show: impl FnOnce() -> bool, wait: Duration) -> Consent {
    let (tx, rx) = mpsc::channel();
    *pending_slot() = Some((d.clone(), tx));
    let r = if !show() {
        Consent::NoWindow
    } else {
        match rx.recv_timeout(wait) {
            Ok(true) => Consent::Yes,
            Ok(false) => Consent::No,
            Err(_) => Consent::NoAnswer,
        }
    };
    let mut p = pending_slot();
    if p.as_ref().is_some_and(|(q, _)| q.id == d.id) {
        *p = None;
    }
    r
}

/// The dialog the window should show now.
pub fn pending() -> Option<Dialog> {
    pending_slot().as_ref().map(|(d, _)| d.clone())
}

/// The window's answer to dialog `id`; false when that dialog is no longer waiting.
pub fn answer(id: u64, yes: bool) -> bool {
    let mut p = pending_slot();
    match p.take() {
        Some((d, tx)) if d.id == id => tx.send(yes).is_ok(),
        other => {
            *p = other;
            false
        }
    }
}

// ----------------------------------------------------------------------------- the machine

/// The elevated process this code started.
pub trait Child {
    fn pid(&self) -> u32;
    /// The process runs the image of the staged file held by `locked` (not another file put at its path).
    fn runs(&self, locked: &dyn std::any::Any) -> bool;
    /// Its exit code once it has exited, waiting at most `within`.
    fn wait(&mut self, within: Duration) -> Option<u32>;
}

/// The server end of the pipe.
pub trait Pipe {
    /// Waits for the client (at most `within`, and not after `child` exited); its process ID.
    fn accept(&mut self, child: &mut dyn Child, within: Duration) -> Result<u32, String>;
    /// One line of at most `max` bytes (without its end).
    fn read_line(&mut self, max: usize, within: Duration) -> Result<String, String>;
    fn write_all(&mut self, bytes: &[u8], within: Duration) -> Result<(), String>;
}

/// How `runas` failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    /// `ERROR_CANCELLED`: the prompt was declined or closed, or no administrator's credentials.
    Declined,
    Failed(String),
}

/// What decides whether Windows shows its prompt for `runas` here.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Uac {
    /// This process's token is elevated (`TokenElevation`).
    pub elevated: bool,
    /// `EnableLUA` = 0: UAC is off.
    pub lua_off: bool,
    /// `ConsentPromptBehaviorAdmin` = 0: administrators are elevated without asking.
    pub admin_silent: bool,
    /// This user is a member of Administrators (split token, or elevated).
    pub admin: bool,
    /// TetherQS's elevated (portable) service runs.
    pub elevated_service: bool,
}

impl Uac {
    /// No prompt would appear: the pre-dialog would be the only consent.
    pub fn silent(&self) -> bool {
        self.elevated || self.lua_off || (self.admin_silent && self.admin) || self.elevated_service
    }
}

/// Everything the elevated path does to the machine, the network and the window. Real on Windows,
/// fakes in the tests.
pub trait Machine {
    /// `%LOCALAPPDATA%\<app>` (the state file and the staging folder live here).
    fn user_dir(&self) -> PathBuf;
    /// The agent's Windows service exists.
    fn agent_service(&self) -> Result<bool, String>;
    /// The practice's removal (`HKLM\SOFTWARE\Tether Monitor` `RemovedAt`; `Some(0)` unparseable).
    fn removed_at(&self) -> Option<u64>;
    /// `<FOLDERID_ProgramFilesX64>\Tether Monitor\tether-monitor.exe` (never from the environment).
    fn installed_exe(&self) -> Result<PathBuf, String>;
    /// The agent service's configured command line (`QueryServiceConfigW`).
    fn service_image(&self) -> Result<String, String>;
    /// The installed agent's version from its Apps entry (`DisplayVersion`; none before 0.2.0).
    fn installed_version(&self) -> Option<String>;
    /// Whether Windows would prompt.
    fn uac(&self) -> Uac;
    /// Unix seconds now (the pending record is stamped at the launch, after the download).
    fn clock(&self) -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
    }
    /// Brings this program's window to the front and returns its handle (0: none) to own the prompt.
    fn owner_window(&self) -> usize;
    /// `(relay base, raw)` of `download/<channel>/monitor.json` from every relay that answered.
    fn fetch_index(&self, channel: &str) -> Vec<(String, Vec<u8>)>;
    /// The candidate's bytes, verified (size, SHA-256, minisign, trusted comment).
    fn download(&self, c: &Candidate, key: &str) -> Result<Vec<u8>, String>;
    /// Creates the staging folder; refuses a junction or link in its place.
    fn prepare_stage(&self, dir: &Path) -> Result<(), String>;
    /// The file's bytes through a handle that denies writing and deleting it while the guard lives.
    fn open_locked(&self, path: &Path) -> Result<(Vec<u8>, Box<dyn std::any::Any>), String>;
    /// `tether-agent-<128 bits of the OS random source as 32 lowercase hex digits>`.
    fn pipe_name(&self) -> Result<String, String>;
    /// The first and only instance of `\\.\pipe\<name>`, for this user and Administrators only.
    fn create_pipe(&self, name: &str) -> Result<Box<dyn Pipe>, String>;
    /// `ShellExecuteExW` verb `runas`, directly by this process, with these fixed arguments, the
    /// prompt owned by window `owner` (so it comes to the front instead of flashing in the taskbar).
    fn launch(&self, exe: &Path, args: &[&str], owner: usize) -> Result<Box<dyn Child>, Launch>;
    /// The pre-dialog in this program's own window ([`ask_window`]).
    fn consent(&self, d: &Dialog) -> Consent;
}

/// The name the agent accepts (`tether-agent-` and 32 lowercase hex digits).
pub fn valid_pipe_name(n: &str) -> bool {
    n.strip_prefix("tether-agent-").is_some_and(|h| h.len() == 32 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

/// `\\?\C:\x` and `C:\x` are the same path; case does not count.
pub fn same_file_path(a: &str, b: &str) -> bool {
    let n = |s: &str| s.trim_start_matches(r"\\?\").to_lowercase();
    n(a) == n(b)
}

/// The program of a service's command line: the quoted first part, or up to the first space.
pub fn service_exe(cmd: &str) -> &str {
    let c = cmd.trim();
    match c.strip_prefix('"') {
        Some(r) => r.split('"').next().unwrap_or(""),
        None => c.split(' ').next().unwrap_or(""),
    }
}

/// The agent's result line on the pipe (the agent's usage documents it): whether it installed, and
/// its last line, cleaned again here and with the token and proof taken out should they be in it.
pub fn agent_result(raw: &str, secrets: &[&str]) -> Option<(bool, String)> {
    #[derive(Deserialize)]
    struct R {
        ok: bool,
        line: String,
    }
    let r: R = serde_json::from_str(raw.trim()).ok()?;
    let line = secrets.iter().filter(|s| !s.is_empty()).fold(r.line, |l, s| l.replace(s, "[removed]"));
    Some((r.ok, tidy(&line)))
}

// ----------------------------------------------------------------------------- the host

/// What the policy and this program say (no Debug: it holds the grant).
#[derive(Clone, Default)]
pub struct Inputs {
    pub mode: String,
    pub grant: String,
    pub channel: String,
    pub admin: Vec<VerifyingKey>,
    pub updater_key: String,
    /// TetherQS: `customer.elevate` (an installed Tether: true).
    pub elevate: bool,
    /// This program's Tether ID and the 32-byte seed of the key it registers with hbbs.
    pub identity: Option<(String, [u8; 32])>,
    /// The product name; `support_org`: see [`Ask`].
    pub org: String,
    pub support_org: String,
    pub relays: Vec<String>,
    /// The machine policy (GPO) sets `health.agent` to `manual` ([`machine_manual`]).
    pub machine_off: bool,
}

/// Whether a machine policy `PolicyJson` (a policy layer) sets `health.agent` to `manual`. Only that
/// one rule is read; anything unreadable is no rule (as the desktop's machine layer ignores it).
pub fn machine_manual(raw: Option<&str>) -> bool {
    use tether_trust::policy::{resolve, Layer, Rule, PROFILE_CUSTOMER};
    let rule = raw.and_then(|r| serde_json::from_str::<serde_json::Value>(r).ok()).and_then(|v| v.get("health.agent").cloned()).and_then(|r| serde_json::from_value::<Rule>(r).ok());
    // resolved like any machine rule (`off` means manual too); absent: the default, not the machine's word
    rule.is_some_and(|r| resolve(None, PROFILE_CUSTOMER, "", Some(&Layer::from([("health.agent".to_owned(), r)]))).get("health.agent") == "manual")
}

/// `health-agent.json` in the per-user folder: what this program did (never the grant or the proof).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UserState {
    /// `installed`, `removed-by-tether`, `failed`.
    pub state: String,
    /// `policy` (the person here clicked the card) or `session`.
    pub by: String,
    pub version: String,
    pub at: u64,
    pub detail: String,
    /// The Tether ID the agent was linked to.
    pub tether_id: String,
    /// Written before the launch, cleared when the outcome is known ([`Host::settle`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending: Option<Pending>,
}

/// An elevated agent this program started (it may outlive the wait, or this program).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pending {
    pub at: u64,
    /// `install` or `remove`.
    pub action: String,
    pub by: String,
    pub version: String,
    pub tether_id: String,
}

/// One elevated action at a time in this process; [`DOING`] says which (1 install, 2 remove).
static RUNNING: Mutex<()> = Mutex::new(());
static DOING: AtomicU8 = AtomicU8::new(0);
/// When the person here last said no (or did not answer): unix seconds, 0 none.
static DECLINED_AT: AtomicU64 = AtomicU64::new(0);

/// The action lock, None while an action runs.
fn lock() -> Option<MutexGuard<'static, ()>> {
    match RUNNING.try_lock() {
        Ok(g) => Some(g),
        Err(TryLockError::Poisoned(p)) => Some(p.into_inner()),
        Err(TryLockError::WouldBlock) => None,
    }
}

fn busy() -> AgentAnswer {
    let what = if DOING.load(Ordering::Relaxed) == 2 { "removing the agent" } else { "installing the agent" };
    AgentAnswer::only("busy", &format!("Tether is {what} (asking the person at this computer, or Windows' prompt is open); ask for the status later"))
}

const NOT_ALLOWED: &str = "not allowed by this computer's policy";
const MACHINE_OFF: &str = "This computer's administrator has switched this off (machine policy).";
const NO_ELEVATE: &str = "not allowed: this computer's policy does not allow administrator prompts";
const DECLINED: &str = "Administrator approval was not given (declined, or no administrator's name and password).";
const MAY_STILL_RUN: &str = "the agent Tether started may still be running; ask for the status again in a few minutes";

pub struct Host<'a> {
    pub m: &'a dyn Machine,
    pub p: Inputs,
}

/// A refusal before anything ran, or how the action ended.
enum Ended {
    /// Nothing changed (nothing is recorded); the sentence for the answer.
    Kept(String),
    Done(UserState),
    /// The agent did not end within the wait: the pending record stays.
    Running,
}

impl Host<'_> {
    fn state_path(&self) -> PathBuf {
        self.m.user_dir().join(USER_STATE)
    }

    fn load(&self) -> UserState {
        std::fs::read(self.state_path()).ok().filter(|b| b.len() <= 16 << 10).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn save(&self, st: &UserState) {
        let p = self.state_path();
        if *st == UserState::default() {
            // nothing to remember (a pending record that served): no file
            let _ = std::fs::remove_file(&p);
            return;
        }
        let r = std::fs::create_dir_all(self.m.user_dir()).and_then(|()| std::fs::write(&p, serde_json::to_vec(st).unwrap_or_default()));
        if let Err(e) = r {
            hbb_common::log::warn!("health agent: {}: {e}", p.display());
        }
    }

    fn own_id(&self) -> Option<&str> {
        self.p.identity.as_ref().map(|(id, _)| id.as_str())
    }

    /// `not-allowed` with its reason, before any dialog.
    fn refused(&self) -> Option<AgentAnswer> {
        if self.p.machine_off {
            return Some(AgentAnswer::only("not-allowed", MACHINE_OFF));
        }
        if !active(&self.p.mode) {
            return Some(AgentAnswer::only("not-allowed", NOT_ALLOWED));
        }
        (!self.p.elevate).then(|| AgentAnswer::only("not-allowed", NO_ELEVATE))
    }

    /// What is true on this computer: `busy` while an action of this process runs; otherwise a
    /// pending record is settled first.
    pub fn status(&self, now: u64) -> AgentAnswer {
        if let Some(r) = self.refused() {
            return r;
        }
        let Some(_free) = lock() else { return busy() };
        self.settle(now);
        self.state_now(now)
    }

    /// With the action lock held (no elevated child of this process is being waited for): the
    /// staged file goes (a running image cannot be deleted, so a still running agent keeps it), and a
    /// pending record is settled as the service path's `adopt` does: the agent's service there
    /// (install) or gone (remove) means it finished, installed or removed by whoever asked; a record
    /// older than [`PENDING_MAX`] otherwise, or older than that and [`ADOPT_GRACE`], is dropped.
    fn settle(&self, now: u64) {
        let _ = std::fs::remove_file(self.m.user_dir().join(STAGE_DIR).join(STAGED_EXE));
        let mut st = self.load();
        let Some(p) = st.pending.take() else { return };
        let Ok(svc) = self.m.agent_service() else { return };
        if now >= p.at + PENDING_MAX + ADOPT_GRACE {
            hbb_common::log::info!("health agent: an old pending record was dropped");
        } else if p.action == "install" && svc {
            st = UserState { state: "installed".into(), by: p.by, version: p.version, at: now, detail: "installed while Tether was not waiting for it".into(), tether_id: p.tether_id, pending: None };
        } else if p.action == "remove" && !svc {
            st = UserState { state: "removed-by-tether".into(), by: p.by, at: now, detail: "removed while Tether was not waiting for it".into(), pending: None, ..st };
        } else if now < p.at + PENDING_MAX {
            return; // it may still be running
        }
        self.save(&st);
    }

    fn state_now(&self, now: u64) -> AgentAnswer {
        let svc = match self.m.agent_service() {
            Ok(s) => s,
            Err(e) => return AgentAnswer::only("failed", &tidy(&e)),
        };
        let st = self.load();
        let ours = |state: &str| AgentAnswer { state: state.into(), version: st.version.clone(), installed_by: st.by.clone(), detail: String::new(), at: st.at };
        if svc {
            if st.state != "installed" {
                return AgentAnswer::only("installed-by-hand", "");
            }
            let other = (!st.tether_id.is_empty() && Some(st.tether_id.as_str()) != self.own_id()).then(|| format!("linked to another ID ({})", st.tether_id));
            return AgentAnswer { detail: other.unwrap_or_default(), ..ours("installed") };
        }
        if let Some(t) = self.m.removed_at() {
            return AgentAnswer { state: "removed-locally".into(), at: if t == 0 { now } else { t }, detail: "removed on this computer".into(), ..Default::default() };
        }
        match st.state.as_str() {
            // installed by this program and its service gone: removed here
            "installed" => AgentAnswer { detail: "removed on this computer".into(), ..ours("removed-locally") },
            "failed" | "removed-by-tether" => AgentAnswer { detail: tidy(&st.detail), ..ours(&st.state) },
            _ => AgentAnswer::only("not-installed", ""),
        }
    }

    /// A technician's request (`viewer`: the name their program gave) or the local person's click
    /// (`viewer` None). A technician's install or removal is refused where Windows would not prompt,
    /// and for [`ASK_AGAIN_AFTER`] after the person here said no.
    pub fn request(&self, action: Action, viewer: Option<&str>, now: u64) -> AgentAnswer {
        if action == Action::Status {
            return self.status(now);
        }
        if let Some(r) = self.refused() {
            return r;
        }
        let uac = self.m.uac();
        if viewer.is_some() && uac.silent() {
            let what = if action == Action::Install { "install" } else { "remove" };
            return AgentAnswer::only("not-allowed", &format!("Windows would not ask for confirmation on this computer; the person at the computer can {what} it there."));
        }
        let since = DECLINED_AT.load(Ordering::Relaxed);
        if viewer.is_some() && since != 0 && now < since + ASK_AGAIN_AFTER {
            let mins = (since + ASK_AGAIN_AFTER - now).div_ceil(60);
            return AgentAnswer::only("busy", &format!("The person at the computer said no a moment ago; ask again in {mins} minute{}.", if mins == 1 { "" } else { "s" }));
        }
        let Some(_one) = lock() else { return busy() };
        DOING.store(if action == Action::Install { 1 } else { 2 }, Ordering::Relaxed);
        self.settle(now);
        if self.load().pending.is_some() {
            return AgentAnswer::only("busy", MAY_STILL_RUN);
        }
        let r = match action {
            Action::Install => self.install(viewer, !uac.silent(), now),
            _ => self.remove(viewer, !uac.silent(), now),
        };
        if !matches!(r, Ok(Ended::Running)) {
            // the outcome is known: the record has served
            let mut st = self.load();
            if st.pending.take().is_some() {
                self.save(&st);
            }
        }
        match r {
            Ok(Ended::Done(st)) => {
                self.save(&st);
                AgentAnswer { detail: tidy(&st.detail), ..self.state_now(now) }
            }
            Ok(Ended::Kept(why)) => AgentAnswer { detail: why, ..self.state_now(now) },
            Ok(Ended::Running) => AgentAnswer::only("busy", MAY_STILL_RUN),
            Err(e) => {
                hbb_common::log::warn!("health agent (prompt): {e}");
                let st = UserState { state: "failed".into(), detail: tidy(&e), at: now, ..self.load() };
                self.save(&st);
                AgentAnswer { detail: st.detail.clone(), ..self.state_now(now) }
            }
        }
    }

    fn ask(&self, kind: Kind, viewer: Option<&str>, prompt: bool, now: u64) -> Result<(), String> {
        let d = dialog(next_id(), &Ask { kind, viewer: viewer.map(str::to_owned), org: self.p.org.clone(), support_org: self.p.support_org.clone(), relays: self.p.relays.clone(), prompt });
        let why = match self.m.consent(&d) {
            Consent::Yes => return Ok(()),
            Consent::No => "The person at the computer declined.",
            Consent::NoAnswer => "The person at the computer did not answer.",
            Consent::NoWindow => return Err("Tether's window could not be shown on that computer; nothing was done.".into()),
        };
        DECLINED_AT.store(now, Ordering::Relaxed);
        Err(why.into())
    }

    fn mark(&self, action: &str, by: &str, version: &str, tether_id: &str) {
        let p = Pending { at: self.m.clock(), action: action.into(), by: by.into(), version: version.into(), tether_id: tether_id.into() };
        self.save(&UserState { pending: Some(p), ..self.load() });
    }

    fn install(&self, viewer: Option<&str>, prompt: bool, now: u64) -> Result<Ended, String> {
        if self.p.mode == "remove" {
            return Ok(Ended::Kept("not allowed: this computer's policy removes the agent".into()));
        }
        // Everything that can be checked without the person comes first: no dialog for nothing.
        let (id, seed) = self.p.identity.clone().ok_or("this computer has no Tether ID yet")?;
        if !status::valid_tether_id(&id) {
            return Err("this computer's Tether ID is not one a relay accepts".into());
        }
        let (token, _) = grant(&self.p.grant, &self.p.admin, now)?;
        if !matches!(self.p.channel.as_str(), "stable" | "test") {
            return Err(format!("the agent has no channel {:?} (stable or test)", self.p.channel));
        }
        if self.p.admin.is_empty() || self.p.updater_key.trim().is_empty() {
            return Err("this build has no update keys".into());
        }
        if self.m.agent_service()? {
            return Ok(Ended::Kept("the agent is already installed".into()));
        }
        if let Err(why) = self.ask(Kind::Install, viewer, prompt, now) {
            return Ok(Ended::Kept(why));
        }
        let by = if viewer.is_some() { "session" } else { "policy" };
        let dir = self.m.user_dir().join(STAGE_DIR);
        self.m.prepare_stage(&dir)?;
        let cache = dir.join(format!("monitor-{}.json", self.p.channel));
        let (c, index) = candidate(&self.m.fetch_index(&self.p.channel), &cache, &self.p.admin, &self.p.channel, now)?;
        let key = self.p.updater_key.as_str();
        let bytes = self.m.download(&c, key)?;
        c.verify(&bytes, key)?;
        let exe = dir.join(STAGED_EXE);
        let err = |e: std::io::Error| format!("{}: {e}", exe.display());
        match std::fs::remove_file(&exe) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(err(e)),
            _ => {}
        }
        let written = std::fs::OpenOptions::new().write(true).create_new(true).open(&exe).and_then(|mut f| std::io::Write::write_all(&mut f, &bytes).and_then(|()| f.sync_all()));
        let r = written.map_err(err).and_then(|()| {
            let (back, lock) = self.m.open_locked(&exe)?;
            // A same-user process may have changed the file before the lock: then this refuses.
            c.verify(&back, key).map_err(|e| format!("the downloaded agent changed before it ran: {e}"))?;
            let index = String::from_utf8(index).map_err(|_| "the agent's index is not text".to_owned())?;
            let name = self.m.pipe_name()?;
            if !valid_pipe_name(&name) {
                return Err("the pipe name is not one the agent accepts".into());
            }
            let mut pipe = self.m.create_pipe(&name)?;
            let arg = format!("pipe:{name}");
            let owner = self.m.owner_window();
            self.mark("install", by, &c.version, &id);
            let mut child = match self.m.launch(&exe, &["install", "--from-tether", &arg], owner) {
                Ok(c) => c,
                Err(Launch::Declined) => return Ok(None),
                Err(Launch::Failed(e)) => return Err(e),
            };
            let sent = (|| {
                if !child.runs(&*lock) {
                    return Err("the program Windows started is not the verified agent: nothing was sent to it".to_owned());
                }
                let client = pipe.accept(&mut *child, CONNECT_WAIT)?;
                if client != child.pid() {
                    return Err("another program connected to Tether's pipe: nothing was sent".into());
                }
                let i = parse_identity(&pipe.read_line(IDENTITY_MAX, EXCHANGE_WAIT)?)?;
                let proof = status::sign_agent_link(&seed, &id, &i.uuid, &i.pubkey);
                let line = serde_json::json!({ "token": token, "tether_id": id, "proof": proof, "channel": self.p.channel, "index": index }).to_string();
                pipe.write_all(format!("{line}\n").as_bytes(), EXCHANGE_WAIT).map(|()| proof)
            })();
            // The agent's one result line when it ends (an older agent closes the pipe and sends none).
            let start = std::time::Instant::now();
            let result = sent.as_ref().ok().and_then(|proof| pipe.read_line(RESULT_MAX, EXIT_WAIT).ok().and_then(|l| agent_result(&l, &[&token, proof])));
            // Our end goes: an agent still waiting for the line gets end of file and refuses.
            drop(pipe);
            let code = child.wait(EXIT_WAIT.saturating_sub(start.elapsed()));
            drop(lock);
            sent?;
            Ok(Some((code, result)))
        });
        let _ = std::fs::remove_file(&exe);
        match r? {
            None => {
                DECLINED_AT.store(now, Ordering::Relaxed);
                Ok(Ended::Kept(DECLINED.into()))
            }
            Some((None, _)) => Ok(Ended::Running),
            // Exit 1 after a service whose start failed: installed all the same (the agent's line says why).
            Some((Some(code), result)) if code == 0 || self.m.agent_service()? => {
                let said = result.map(|(_, l)| l).filter(|l| !l.is_empty());
                let detail = match said {
                    Some(l) => format!("Installed {}, linked to {id}. The agent: {l}", c.version),
                    None => format!("Installed {}, linked to {id}; first report within five minutes.", c.version),
                };
                Ok(Ended::Done(UserState { state: "installed".into(), by: by.into(), version: c.version.clone(), at: now, detail, tether_id: id, pending: None }))
            }
            Some((Some(code), result)) => Err(match result.map(|(_, l)| l).filter(|l| !l.is_empty()) {
                Some(l) => format!("the agent refused: {l}"),
                None => format!("the agent refused (exit code {code}); nothing was installed as the agent"),
            }),
        }
    }

    fn remove(&self, viewer: Option<&str>, prompt: bool, now: u64) -> Result<Ended, String> {
        if !self.m.agent_service()? {
            return Ok(Ended::Kept("the agent is not installed".into()));
        }
        let old = self.m.installed_version().is_none_or(|v| tether_trust::version::compare(&v, BY_TETHER_SINCE).is_none_or(|o| o.is_lt()));
        if old {
            return Ok(Ended::Kept(format!("This version of the agent (older than {BY_TETHER_SINCE}) cannot be removed through Tether: remove it on the computer under Settings › Apps › Tether Monitor, or with \"tether-monitor uninstall\" in an administrator's command prompt. No prompt was started.")));
        }
        // The installed exe in the administrators-only program folder (no staging needed), and only
        // when it is the program the agent's service runs.
        let exe = self.m.installed_exe()?;
        let image = self.m.service_image()?;
        if !same_file_path(service_exe(&image), &exe.to_string_lossy()) {
            return Ok(Ended::Kept("The agent's service runs another program than the one in Tether Monitor's program folder: nothing was started. Remove it on the computer under Settings › Apps.".into()));
        }
        if let Err(why) = self.ask(Kind::Remove, viewer, prompt, now) {
            return Ok(Ended::Kept(why));
        }
        let by = if viewer.is_some() { "session" } else { "policy" };
        let owner = self.m.owner_window();
        self.mark("remove", by, "", "");
        let mut child = match self.m.launch(&exe, &["uninstall", "--by-tether"], owner) {
            Ok(c) => c,
            Err(Launch::Declined) => {
                DECLINED_AT.store(now, Ordering::Relaxed);
                return Ok(Ended::Kept(DECLINED.into()));
            }
            Err(Launch::Failed(e)) => return Err(e),
        };
        match child.wait(EXIT_WAIT) {
            Some(0) => Ok(Ended::Done(UserState { state: "removed-by-tether".into(), by: by.into(), at: now, detail: "Removed: the agent's service, folders and enrolment are gone.".into(), pending: None, ..self.load() })),
            Some(code) => Err(format!("the agent's uninstall failed (exit code {code})")),
            None => Ok(Ended::Running),
        }
    }
}

// ----------------------------------------------------------------------------- Windows

#[cfg(windows)]
pub mod win {
    use super::*;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, LocalFree, ERROR_CANCELLED, ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, ERROR_SERVICE_DOES_NOT_EXIST, GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
    use windows_sys::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
    use windows_sys::Win32::Security::{GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER};
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, GetFinalPathNameByHandleW, ReadFile, WriteFile, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ, OPEN_EXISTING, PIPE_ACCESS_DUPLEX};
    use windows_sys::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT};
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_ANY, RRF_SUBKEY_WOW6464KEY};
    use windows_sys::Win32::System::Services::{CloseServiceHandle, OpenSCManagerW, OpenServiceW, SC_MANAGER_CONNECT, SERVICE_QUERY_STATUS};
    use windows_sys::Win32::System::Threading::{CreateEventW, GetCurrentProcess, GetExitCodeProcess, GetProcessId, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION};
    use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
    use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    /// `HKLM\<key>` `RemovedAt`: written by the practice's `uninstall` (admin only), deleted by an install.
    const REMOVED_KEY: &str = r"SOFTWARE\Tether Monitor";
    const REMOVED_VALUE: &str = "RemovedAt";

    pub fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    /// The 64-bit Program Files folder from the known-folder API (HKLM): `%ProgramFiles%` is the
    /// user's to override for the next logon.
    pub fn program_files() -> Result<PathBuf, String> {
        use windows_sys::Win32::System::Com::CoTaskMemFree;
        use windows_sys::Win32::UI::Shell::{SHGetKnownFolderPath, FOLDERID_ProgramFilesX64, KF_FLAG_DEFAULT};
        unsafe {
            let mut p: windows_sys::core::PWSTR = null_mut();
            let hr = SHGetKnownFolderPath(&FOLDERID_ProgramFilesX64, KF_FLAG_DEFAULT as u32, null_mut(), &mut p);
            let r = (hr >= 0 && !p.is_null()).then(|| {
                let n = (0..).take_while(|&i| *p.add(i) != 0).count();
                PathBuf::from(String::from_utf16_lossy(std::slice::from_raw_parts(p, n)))
            });
            CoTaskMemFree(p.cast());
            r.ok_or_else(|| format!("the Program Files folder could not be found (0x{hr:08x})"))
        }
    }

    pub fn installed_exe() -> Result<PathBuf, String> {
        Ok(program_files()?.join(AGENT_FOLDER).join(AGENT_EXE))
    }

    /// The agent service's configured command line.
    pub fn service_image() -> Result<String, String> {
        use windows_sys::Win32::System::Services::{QueryServiceConfigW, QUERY_SERVICE_CONFIGW, SERVICE_QUERY_CONFIG};
        let name = wide(std::ffi::OsStr::new(SERVICE));
        let err = || format!("the {SERVICE} service: {}", std::io::Error::last_os_error());
        unsafe {
            let m = OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT);
            if m.is_null() {
                return Err(format!("service manager: {}", std::io::Error::last_os_error()));
            }
            let s = OpenServiceW(m, name.as_ptr(), SERVICE_QUERY_CONFIG);
            let r = if s.is_null() {
                Err(err())
            } else {
                // 8 KiB, aligned for the struct: the documented maximum of a configuration.
                let mut buf = vec![0u64; 1024];
                let mut need = 0u32;
                let q = buf.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW;
                let r = if QueryServiceConfigW(s, q, (buf.len() * 8) as u32, &mut need) == 0 || (*q).lpBinaryPathName.is_null() {
                    Err(err())
                } else {
                    let p = (*q).lpBinaryPathName;
                    let n = (0..).take_while(|&i| *p.add(i) != 0).count();
                    Ok(String::from_utf16_lossy(std::slice::from_raw_parts(p, n)))
                };
                CloseServiceHandle(s);
                r
            };
            CloseServiceHandle(m);
            r
        }
    }

    /// A string value under HKLM (64-bit view), at most 255 characters.
    fn hklm_text(key: &str, value: &str) -> Option<String> {
        use windows_sys::Win32::System::Registry::RRF_RT_REG_SZ;
        let (key, name) = (wide(std::ffi::OsStr::new(key)), wide(std::ffi::OsStr::new(value)));
        let mut buf = [0u16; 256];
        let mut size = std::mem::size_of_val(&buf) as u32;
        let r = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY, null_mut(), buf.as_mut_ptr().cast(), &mut size) };
        (r == 0).then(|| String::from_utf16_lossy(&buf[..(size as usize / 2).min(buf.len())]).trim_end_matches('\0').trim().to_owned())
    }

    /// `PolicyJson` (REG_SZ) under `HKLM\<key>`, 64-bit view, read only, at most 1 MiB.
    pub fn machine_policy(key: &str) -> Option<String> {
        use windows_sys::Win32::System::Registry::RRF_RT_REG_SZ;
        let (key, name) = (wide(std::ffi::OsStr::new(key)), wide(std::ffi::OsStr::new("PolicyJson")));
        let get = |buf: *mut std::ffi::c_void, size: &mut u32| unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY, null_mut(), buf, size) };
        let mut size = 0u32;
        if get(null_mut(), &mut size) != 0 || size == 0 || size > 1 << 20 {
            return None;
        }
        let mut buf = vec![0u16; size as usize / 2];
        if get(buf.as_mut_ptr().cast(), &mut size) != 0 {
            return None;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]))
    }

    fn hklm_dword(key: &str, value: &str) -> Option<u32> {
        use windows_sys::Win32::System::Registry::RRF_RT_REG_DWORD;
        let (key, name) = (wide(std::ffi::OsStr::new(key)), wide(std::ffi::OsStr::new(value)));
        let (mut v, mut size) = (0u32, 4u32);
        let r = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), name.as_ptr(), RRF_RT_REG_DWORD | RRF_SUBKEY_WOW6464KEY, null_mut(), (&mut v as *mut u32).cast(), &mut size) };
        (r == 0).then_some(v)
    }

    /// `DisplayVersion` of the agent's Apps entry (written by install from 0.2.0 on).
    pub fn installed_version() -> Option<String> {
        hklm_text(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\TetherMonitor", "DisplayVersion").filter(|v| release::valid_version(v))
    }

    /// This process's token and the UAC settings (`HKLM\…\Policies\System`; absent values are the
    /// defaults: UAC on, administrators asked).
    pub fn uac() -> Uac {
        use windows_sys::Win32::Security::{TokenElevation, TokenElevationType, TokenElevationTypeLimited, TOKEN_ELEVATION};
        const POLICY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System";
        let (mut elevated, mut kind) = (false, 0i32);
        unsafe {
            let mut tok: HANDLE = null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok) != 0 {
                let mut e: TOKEN_ELEVATION = std::mem::zeroed();
                let mut len = 0u32;
                elevated = GetTokenInformation(tok, TokenElevation, (&mut e as *mut TOKEN_ELEVATION).cast(), 4, &mut len) != 0 && e.TokenIsElevated != 0;
                if GetTokenInformation(tok, TokenElevationType, (&mut kind as *mut i32).cast(), 4, &mut len) == 0 {
                    kind = 0;
                }
                CloseHandle(tok);
            }
        }
        Uac { elevated, lua_off: hklm_dword(POLICY, "EnableLUA") == Some(0), admin_silent: hklm_dword(POLICY, "ConsentPromptBehaviorAdmin") == Some(0), admin: elevated || kind == TokenElevationTypeLimited, elevated_service: false }
    }

    /// `%LOCALAPPDATA%\<app name of this process>`.
    pub fn user_dir() -> PathBuf {
        let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        base.join(hbb_common::config::APP_NAME.read().unwrap_or_else(|e| e.into_inner()).as_str())
    }

    fn is_reparse(p: &Path) -> std::io::Result<bool> {
        use std::os::windows::fs::MetadataExt;
        Ok(std::fs::symlink_metadata(p)?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
    }

    pub fn service_exists() -> Result<bool, String> {
        let name = wide(std::ffi::OsStr::new(SERVICE));
        unsafe {
            let m = OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT);
            if m.is_null() {
                return Err(format!("service manager: {}", std::io::Error::last_os_error()));
            }
            let s = OpenServiceW(m, name.as_ptr(), SERVICE_QUERY_STATUS);
            let r = if !s.is_null() {
                CloseServiceHandle(s);
                Ok(true)
            } else if GetLastError() == ERROR_SERVICE_DOES_NOT_EXIST {
                Ok(false)
            } else {
                Err(format!("the {SERVICE} service: {}", std::io::Error::last_os_error()))
            };
            CloseServiceHandle(m);
            r
        }
    }

    pub fn removed_at() -> Option<u64> {
        let (key, name) = (wide(std::ffi::OsStr::new(REMOVED_KEY)), wide(std::ffi::OsStr::new(REMOVED_VALUE)));
        let mut buf = [0u16; 64];
        let mut size = std::mem::size_of_val(&buf) as u32;
        let mut kind = 0u32;
        let r = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), name.as_ptr(), RRF_RT_ANY | RRF_SUBKEY_WOW6464KEY, &mut kind, buf.as_mut_ptr().cast(), &mut size) };
        if r == ERROR_FILE_NOT_FOUND {
            return None;
        }
        // Anything there but a readable number still counts as removed (the practice's choice wins).
        const REG_SZ: u32 = 1;
        let text = (r == 0 && kind == REG_SZ).then(|| {
            let n = (size as usize / 2).min(buf.len());
            String::from_utf16_lossy(&buf[..n]).trim_end_matches('\0').trim().to_owned()
        });
        Some(text.and_then(|t| t.parse().ok()).unwrap_or(0))
    }

    /// Read through a handle that shares reading only: nobody can write, rename or delete the
    /// file while the guard (the `File`) lives; the loader's own open (read and execute) is still allowed.
    pub fn open_locked(path: &Path) -> Result<(Vec<u8>, Box<dyn std::any::Any>), String> {
        use std::io::Read;
        use std::os::windows::fs::MetadataExt;
        use std::os::windows::io::FromRawHandle;
        let err = |e: std::io::Error| format!("{}: {e}", path.display());
        let w = wide(path.as_os_str());
        let h = unsafe { CreateFileW(w.as_ptr(), GENERIC_READ, FILE_SHARE_READ, null(), OPEN_EXISTING, FILE_FLAG_OPEN_REPARSE_POINT, null_mut()) };
        if h == INVALID_HANDLE_VALUE {
            return Err(err(std::io::Error::last_os_error()));
        }
        let f = unsafe { std::fs::File::from_raw_handle(h) };
        let md = f.metadata().map_err(err)?;
        if md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 || !md.is_file() || md.len() > release::MAX_ARTIFACT {
            return Err(format!("{} is not a plain file of at most {} bytes: refused", path.display(), release::MAX_ARTIFACT));
        }
        let mut bytes = Vec::with_capacity(md.len() as usize);
        (&f).take(release::MAX_ARTIFACT + 1).read_to_end(&mut bytes).map_err(err)?;
        Ok((bytes, Box::new(f)))
    }

    /// The staging folder in the user's own profile: created, a junction or link refused.
    pub fn prepare_stage(dir: &Path) -> Result<(), String> {
        for d in [dir.parent().unwrap_or(dir), dir] {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
            if is_reparse(d).map_err(|e| format!("{}: {e}", d.display()))? {
                return Err(format!("{} is a junction or link: refused", d.display()));
            }
        }
        Ok(())
    }

    pub fn pipe_name() -> Result<String, String> {
        let r = hbb_common::sodiumoxide::randombytes::randombytes(16);
        if r.len() != 16 {
            return Err("no random bytes".into());
        }
        Ok(format!("tether-agent-{}", r.iter().map(|b| format!("{b:02x}")).collect::<String>()))
    }

    /// This process's user as a SID string.
    fn user_sid() -> Result<String, String> {
        unsafe {
            let mut tok: HANDLE = null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok) == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let mut buf = vec![0u8; 256];
            let mut len = 0u32;
            let ok = GetTokenInformation(tok, TokenUser, buf.as_mut_ptr().cast(), buf.len() as u32, &mut len);
            CloseHandle(tok);
            if ok == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let tu = &*(buf.as_ptr() as *const TOKEN_USER);
            let mut s = null_mut();
            if ConvertSidToStringSidW(tu.User.Sid, &mut s) == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let n = (0..).take_while(|&i| *s.add(i) != 0).count();
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(s, n));
            LocalFree(s.cast());
            Ok(sid)
        }
    }

    /// The access list of the pipe: this user and Administrators, nobody else.
    pub fn pipe_sddl() -> Result<String, String> {
        Ok(format!("D:P(A;;GA;;;{})(A;;GA;;;BA)", user_sid()?))
    }

    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }
    unsafe impl Send for Handle {}

    pub struct ServerPipe {
        h: Handle,
        ev: Handle,
        buf: Vec<u8>,
    }

    impl ServerPipe {
        /// One overlapped operation (`start` begins it with the OVERLAPPED), waited for at most
        /// `within` in short steps (`stop` ends it early), cancelled otherwise.
        fn wait_io(&self, ov: &mut OVERLAPPED, started: bool, within: Duration, stop: &mut dyn FnMut() -> bool) -> Result<u32, String> {
            if !started {
                let e = unsafe { GetLastError() };
                if e != ERROR_IO_PENDING {
                    return Err(std::io::Error::from_raw_os_error(e as i32).to_string());
                }
            }
            let end = std::time::Instant::now() + within;
            loop {
                if unsafe { WaitForSingleObject(self.ev.0, 100) } == WAIT_OBJECT_0 {
                    let mut n = 0u32;
                    if unsafe { GetOverlappedResult(self.h.0, ov, &mut n, 0) } == 0 {
                        return Err(std::io::Error::last_os_error().to_string());
                    }
                    return Ok(n);
                }
                if std::time::Instant::now() >= end || stop() {
                    unsafe {
                        CancelIoEx(self.h.0, ov);
                        let mut n = 0u32;
                        GetOverlappedResult(self.h.0, ov, &mut n, 1);
                    }
                    return Err("Tether's pipe: no answer in time".into());
                }
            }
        }

        fn ov(&self) -> OVERLAPPED {
            let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
            ov.hEvent = self.ev.0;
            ov
        }
    }

    impl Pipe for ServerPipe {
        fn accept(&mut self, child: &mut dyn Child, within: Duration) -> Result<u32, String> {
            let mut ov = self.ov();
            let ok = unsafe { ConnectNamedPipe(self.h.0, &mut ov) } != 0;
            // The client may have connected between the launch and this call.
            if !ok && unsafe { GetLastError() } != ERROR_PIPE_CONNECTED {
                self.wait_io(&mut ov, false, within, &mut || child.wait(Duration::ZERO).is_some()).map_err(|e| format!("the agent did not connect: {e}"))?;
            }
            let mut pid = 0u32;
            if unsafe { GetNamedPipeClientProcessId(self.h.0, &mut pid) } == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            Ok(pid)
        }

        fn read_line(&mut self, max: usize, within: Duration) -> Result<String, String> {
            let end = std::time::Instant::now() + within;
            loop {
                if let Some(i) = self.buf.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = self.buf.drain(..=i).collect();
                    return String::from_utf8(line).map(|s| s.trim_end().to_owned()).map_err(|_| "the agent's line is not text".into());
                }
                if self.buf.len() > max {
                    return Err("the agent's line is too long".into());
                }
                let mut chunk = [0u8; 1024];
                let mut ov = self.ov();
                let mut n = 0u32;
                let ok = unsafe { ReadFile(self.h.0, chunk.as_mut_ptr(), chunk.len() as u32, &mut n, &mut ov) } != 0;
                let left = end.saturating_duration_since(std::time::Instant::now());
                let n = if ok { n } else { self.wait_io(&mut ov, false, left, &mut || false)? };
                if n == 0 {
                    return Err("the agent closed the pipe".into());
                }
                self.buf.extend_from_slice(&chunk[..n as usize]);
            }
        }

        fn write_all(&mut self, bytes: &[u8], within: Duration) -> Result<(), String> {
            let end = std::time::Instant::now() + within;
            let mut at = 0;
            while at < bytes.len() {
                let part = &bytes[at..(at + 64 * 1024).min(bytes.len())];
                let mut ov = self.ov();
                let mut n = 0u32;
                let ok = unsafe { WriteFile(self.h.0, part.as_ptr(), part.len() as u32, &mut n, &mut ov) } != 0;
                let left = end.saturating_duration_since(std::time::Instant::now());
                at += if ok { n } else { self.wait_io(&mut ov, false, left, &mut || false)? } as usize;
            }
            Ok(())
        }
    }

    /// The first and only instance (`FILE_FLAG_FIRST_PIPE_INSTANCE`: a pipe of that name made by
    /// anyone before fails this), local clients only, the access list of [`pipe_sddl`].
    pub fn create_pipe(name: &str) -> Result<Box<dyn Pipe>, String> {
        let sddl = wide(std::ffi::OsStr::new(&pipe_sddl()?));
        let path = wide(std::ffi::OsStr::new(&format!(r"\\.\pipe\{name}")));
        unsafe {
            let mut sd: PSECURITY_DESCRIPTOR = null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, null_mut()) == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd, bInheritHandle: 0 };
            let h = CreateNamedPipeW(path.as_ptr(), PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE | FILE_FLAG_OVERLAPPED, PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS, 1, 64 * 1024, 4096, 0, &sa);
            let e = std::io::Error::last_os_error();
            LocalFree(sd);
            if h == INVALID_HANDLE_VALUE {
                return Err(format!("Tether's pipe: {e}"));
            }
            let ev = CreateEventW(null(), 1, 0, null());
            if ev.is_null() {
                CloseHandle(h);
                return Err(std::io::Error::last_os_error().to_string());
            }
            Ok(Box::new(ServerPipe { h: Handle(h), ev: Handle(ev), buf: Vec::new() }))
        }
    }

    pub struct Elevated {
        h: Handle,
        pid: u32,
    }

    fn final_path(h: HANDLE) -> Option<String> {
        let mut buf = vec![0u16; 1024];
        let n = unsafe { GetFinalPathNameByHandleW(h, buf.as_mut_ptr(), buf.len() as u32, 0) } as usize;
        (n > 0 && n < buf.len()).then(|| String::from_utf16_lossy(&buf[..n]))
    }

    fn image_path(h: HANDLE) -> Option<String> {
        let mut buf = vec![0u16; 1024];
        let mut n = buf.len() as u32;
        (unsafe { QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut n) } != 0).then(|| String::from_utf16_lossy(&buf[..n as usize]))
    }

    impl Child for Elevated {
        fn pid(&self) -> u32 {
            self.pid
        }

        fn runs(&self, locked: &dyn std::any::Any) -> bool {
            use std::os::windows::io::AsRawHandle;
            let Some(f) = locked.downcast_ref::<std::fs::File>() else { return false };
            let Some(want) = final_path(f.as_raw_handle()) else { return false };
            let got = image_path(self.h.0).or_else(|| unsafe {
                let p = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, self.pid);
                if p.is_null() {
                    return None;
                }
                let r = image_path(p);
                CloseHandle(p);
                r
            });
            got.is_some_and(|g| same_file_path(&g, &want))
        }

        fn wait(&mut self, within: Duration) -> Option<u32> {
            if unsafe { WaitForSingleObject(self.h.0, within.as_millis().min(u32::MAX as u128 - 1) as u32) } != WAIT_OBJECT_0 {
                return None;
            }
            let mut code = 0u32;
            (unsafe { GetExitCodeProcess(self.h.0, &mut code) } != 0).then_some(code)
        }
    }

    /// `ShellExecuteExW` verb `runas` on `exe` with `args` (no spaces in any of them), no window,
    /// the prompt owned by window `owner` (0: none); blocks while the prompt is open.
    pub fn launch(exe: &Path, args: &[&str], owner: usize) -> Result<Box<dyn Child>, Launch> {
        if args.iter().any(|a| a.is_empty() || a.contains([' ', '"', '\t'])) {
            return Err(Launch::Failed("an argument with a space".into()));
        }
        let (verb, file, params) = (wide(std::ffi::OsStr::new("runas")), wide(exe.as_os_str()), wide(std::ffi::OsStr::new(&args.join(" "))));
        let dir = exe.parent().map(|d| wide(d.as_os_str()));
        let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
        info.hwnd = owner as windows_sys::Win32::Foundation::HWND;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params.as_ptr();
        info.lpDirectory = dir.as_ref().map_or(null(), |d| d.as_ptr());
        info.nShow = SW_HIDE;
        if unsafe { ShellExecuteExW(&mut info) } == 0 {
            let e = unsafe { GetLastError() };
            return Err(if e == ERROR_CANCELLED { Launch::Declined } else { Launch::Failed(format!("Windows could not start the agent with administrator rights: {}", std::io::Error::from_raw_os_error(e as i32))) });
        }
        if info.hProcess.is_null() {
            return Err(Launch::Failed("Windows started the agent without a process handle".into()));
        }
        let pid = unsafe { GetProcessId(info.hProcess) };
        Ok(Box::new(Elevated { h: Handle(info.hProcess), pid }))
    }
}

// ----------------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use tether_trust::{envelope, release::{Entry, Index, Rollout}, SigningKey};

    const NOW: u64 = 1_900_000_000;
    const ID: &str = "123456789";
    const SEED: [u8; 32] = [9; 32];
    const UUID: &str = "agent-uuid-0001";
    const NAME: &str = "tether-agent-0123456789abcdef0123456789abcdef";

    fn b64(b: &[u8]) -> String {
        use hbb_common::base64::Engine;
        hbb_common::base64::engine::general_purpose::STANDARD.encode(b)
    }

    fn agent_pubkey() -> String {
        tether_trust::key_to_b64(&SigningKey::from_bytes(&[3; 32]).verifying_key())
    }

    struct Keys {
        admin: SigningKey,
        kp: minisign::KeyPair,
        public: String,
    }

    fn keys() -> Keys {
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let public = b64(kp.pk.to_box().unwrap().to_string().as_bytes());
        Keys { admin: tether_trust::generate_key(), kp, public }
    }

    fn index(k: &Keys, data: &[u8], version: &str) -> Vec<u8> {
        let file = format!("tether-monitor-{version}.exe");
        let trusted = format!("timestamp:1\tfile:{file}\tversion:{version}");
        let sig = minisign::sign(Some(&k.kp.pk), &k.kp.sk, std::io::Cursor::new(data), Some(&trusted), Some("u")).unwrap();
        let e = Entry { version: version.into(), file, size: data.len() as u64, sha256: release::sha256_hex(data), signature: b64(sig.to_string().as_bytes()), urls: vec!["https://{relay}/download/stable/monitor/x.exe".into()], rollout: Rollout { start: NOW - 10, steps: vec![(0, 100)] }, ..Default::default() };
        let ix = Index { kind: release::INDEX_TYPE.into(), channel: "stable".into(), serial: 5, issued: NOW - 86_400, expires: NOW + 30 * 86_400, releases: [(format!("monitor/{}", release::WINDOWS), e)].into() };
        serde_json::to_vec(&envelope::sign(&serde_json::to_vec(&ix).unwrap(), &k.admin)).unwrap()
    }

    /// What the fake agent does after the launch.
    #[derive(Clone, Copy, PartialEq, Debug)]
    enum Agent {
        /// Connects, sends identity, reads the line, exits with this code.
        Runs(u32),
        /// Another process connects to the pipe.
        Stranger,
        /// Windows started another image than the locked file.
        Swapped,
        /// Still running when the wait ends.
        Hangs,
        /// Made the service, could not start it, exits 1.
        Unstarted,
    }

    struct FakeChild {
        pid: u32,
        agent: Agent,
        alive: std::rc::Rc<Cell<bool>>,
        /// On a successful exit: an install made the service (only when it got the line), an uninstall removed it.
        svc: std::rc::Rc<Cell<bool>>,
        install: bool,
        sent: std::rc::Rc<RefCell<Vec<u8>>>,
    }

    impl Child for FakeChild {
        fn pid(&self) -> u32 {
            self.pid
        }
        fn runs(&self, _locked: &dyn std::any::Any) -> bool {
            self.agent != Agent::Swapped
        }
        fn wait(&mut self, _within: Duration) -> Option<u32> {
            match self.agent {
                Agent::Hangs => return None,
                Agent::Runs(0) => self.svc.set(self.install && !self.sent.borrow().is_empty()),
                Agent::Unstarted => self.svc.set(true),
                _ => {}
            }
            self.alive.set(false);
            Some(match self.agent {
                Agent::Runs(c) => c,
                _ => 1,
            })
        }
    }

    struct FakePipe {
        client: u32,
        /// The second connection attempt on a pipe that allows one instance.
        connections: std::rc::Rc<Cell<u32>>,
        /// What the agent writes: `identity`, then its result line (none from an older agent).
        lines: std::collections::VecDeque<String>,
        sent: std::rc::Rc<RefCell<Vec<u8>>>,
    }

    impl Pipe for FakePipe {
        fn accept(&mut self, _child: &mut dyn Child, _within: Duration) -> Result<u32, String> {
            self.connections.set(self.connections.get() + 1);
            Ok(self.client)
        }
        fn read_line(&mut self, max: usize, _within: Duration) -> Result<String, String> {
            let l = self.lines.pop_front().ok_or("the agent closed the pipe")?;
            if l.len() > max {
                return Err("the agent's line is too long".into());
            }
            Ok(l)
        }
        fn write_all(&mut self, bytes: &[u8], _within: Duration) -> Result<(), String> {
            self.sent.borrow_mut().extend_from_slice(bytes);
            Ok(())
        }
    }

    struct Fake {
        dir: PathBuf,
        svc: std::rc::Rc<Cell<bool>>,
        removed: Cell<Option<u64>>,
        docs: RefCell<Vec<(String, Vec<u8>)>>,
        artifact: Vec<u8>,
        consent: Cell<Consent>,
        dialogs: RefCell<Vec<Dialog>>,
        launch: RefCell<Option<Launch>>,
        agent: Cell<Agent>,
        launched: RefCell<Vec<(PathBuf, Vec<String>)>>,
        fetches: Cell<u32>,
        tamper: Cell<bool>,
        /// The staged file was there (and locked) when the launch came.
        staged_at_launch: Cell<bool>,
        sent: std::rc::Rc<RefCell<Vec<u8>>>,
        connections: std::rc::Rc<Cell<u32>>,
        alive: std::rc::Rc<Cell<bool>>,
        identity: RefCell<String>,
        result: RefCell<Option<String>>,
        uac: Cell<Uac>,
        image: RefCell<String>,
        version: RefCell<Option<String>>,
        clock: Cell<u64>,
        fronted: Cell<bool>,
        /// The owner window the launch got, and whether the window had come to the front before it.
        owner_at_launch: Cell<Option<(usize, bool)>>,
    }

    impl Machine for Fake {
        fn user_dir(&self) -> PathBuf {
            self.dir.clone()
        }
        fn agent_service(&self) -> Result<bool, String> {
            Ok(self.svc.get())
        }
        fn removed_at(&self) -> Option<u64> {
            self.removed.get()
        }
        fn installed_exe(&self) -> Result<PathBuf, String> {
            Ok(PathBuf::from(r"C:\Program Files\Tether Monitor\tether-monitor.exe"))
        }
        fn service_image(&self) -> Result<String, String> {
            Ok(self.image.borrow().clone())
        }
        fn installed_version(&self) -> Option<String> {
            self.version.borrow().clone()
        }
        fn uac(&self) -> Uac {
            self.uac.get()
        }
        fn clock(&self) -> u64 {
            self.clock.get()
        }
        fn owner_window(&self) -> usize {
            self.fronted.set(true);
            0x4711
        }
        fn fetch_index(&self, _channel: &str) -> Vec<(String, Vec<u8>)> {
            self.fetches.set(self.fetches.get() + 1);
            self.docs.borrow().clone()
        }
        fn download(&self, _c: &Candidate, _key: &str) -> Result<Vec<u8>, String> {
            Ok(self.artifact.clone())
        }
        fn prepare_stage(&self, dir: &Path) -> Result<(), String> {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())
        }
        fn open_locked(&self, path: &Path) -> Result<(Vec<u8>, Box<dyn std::any::Any>), String> {
            if self.tamper.get() {
                std::fs::write(path, b"planted by a process of the same user").unwrap();
            }
            Ok((std::fs::read(path).map_err(|e| e.to_string())?, Box::new(())))
        }
        fn pipe_name(&self) -> Result<String, String> {
            Ok(NAME.into())
        }
        fn create_pipe(&self, name: &str) -> Result<Box<dyn Pipe>, String> {
            assert_eq!(name, NAME);
            let client = match self.agent.get() {
                Agent::Stranger => 4242,
                _ => 77,
            };
            let lines = std::iter::once(self.identity.borrow().clone()).chain(self.result.borrow().clone()).collect();
            Ok(Box::new(FakePipe { client, connections: self.connections.clone(), lines, sent: self.sent.clone() }))
        }
        fn launch(&self, exe: &Path, args: &[&str], owner: usize) -> Result<Box<dyn Child>, Launch> {
            self.staged_at_launch.set(exe.exists());
            self.owner_at_launch.set(Some((owner, self.fronted.get())));
            self.launched.borrow_mut().push((exe.to_owned(), args.iter().map(|a| a.to_string()).collect()));
            if let Some(l) = self.launch.borrow().clone() {
                return Err(l);
            }
            self.alive.set(true);
            Ok(Box::new(FakeChild { pid: 77, agent: self.agent.get(), alive: self.alive.clone(), svc: self.svc.clone(), install: args[0] == "install", sent: self.sent.clone() }))
        }
        fn consent(&self, d: &Dialog) -> Consent {
            self.dialogs.borrow_mut().push(d.clone());
            self.consent.get()
        }
    }

    struct World {
        k: Keys,
        fake: Fake,
        p: Inputs,
        token: String,
        /// One at a time: the action lock and the pending dialog are process-wide.
        _one: std::sync::MutexGuard<'static, ()>,
    }

    static ONE: Mutex<()> = Mutex::new(());

    impl Drop for World {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.fake.dir);
        }
    }

    fn world(name: &str, mode: &str) -> World {
        let one = ONE.lock().unwrap_or_else(|e| e.into_inner());
        let k = keys();
        let (token, _) = status::make_token("policy:test", NOW + 86_400 * 365, None, None, true, &k.admin).unwrap();
        let dir = std::env::temp_dir().join(format!("tether-elevated-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let artifact = b"agent 0.2.0".to_vec();
        let docs = vec![("https://relay.test".to_owned(), index(&k, &artifact, "0.2.0"))];
        let fake = Fake {
            dir,
            svc: Default::default(),
            removed: Cell::new(None),
            docs: RefCell::new(docs),
            artifact,
            consent: Cell::new(Consent::Yes),
            dialogs: RefCell::default(),
            launch: RefCell::new(None),
            agent: Cell::new(Agent::Runs(0)),
            launched: RefCell::default(),
            fetches: Cell::new(0),
            tamper: Cell::new(false),
            staged_at_launch: Cell::new(false),
            sent: Default::default(),
            connections: Default::default(),
            alive: Default::default(),
            identity: RefCell::new(format!("{{\"uuid\":\"{UUID}\",\"pubkey\":\"{}\"}}", agent_pubkey())),
            result: RefCell::new(None),
            uac: Cell::new(Uac::default()),
            image: RefCell::new(r#""C:\Program Files\Tether Monitor\tether-monitor.exe" service"#.into()),
            version: RefCell::new(Some("0.2.0".into())),
            clock: Cell::new(NOW),
            fronted: Cell::new(false),
            owner_at_launch: Cell::new(None),
        };
        DECLINED_AT.store(0, Ordering::Relaxed);
        let p = Inputs { mode: mode.into(), grant: token.clone(), channel: "stable".into(), admin: vec![k.admin.verifying_key()], updater_key: k.public.clone(), elevate: true, identity: Some((ID.into(), SEED)), org: "Example Support".into(), relays: vec!["relay.test".into()], ..Default::default() };
        World { k, fake, p, token, _one: one }
    }

    impl World {
        fn host(&self) -> Host<'_> {
            Host { m: &self.fake, p: self.p.clone() }
        }
        fn install(&self) -> AgentAnswer {
            self.host().request(Action::Install, Some("Sam at the help desk"), NOW)
        }
        fn sent(&self) -> Option<serde_json::Value> {
            let s = String::from_utf8(self.fake.sent.borrow().clone()).unwrap();
            (!s.is_empty()).then(|| serde_json::from_str(s.trim_end()).unwrap())
        }
        /// Nothing left: no staged file, no process still running.
        fn clean(&self) -> bool {
            !self.fake.dir.join(STAGE_DIR).join(STAGED_EXE).exists() && !self.fake.alive.get()
        }
    }

    #[test]
    fn an_install_sends_the_raw_index_and_the_proof_for_the_key_received() {
        let w = world("install", "on-request");
        let a = w.install();
        assert_eq!(a.state, "installed", "{a:?}");
        assert!(a.detail.contains("linked to 123456789"), "{}", a.detail);
        // only the staged file, with the fixed arguments, while it was there and locked
        let l = w.fake.launched.borrow().clone();
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].0, w.fake.dir.join(STAGE_DIR).join(STAGED_EXE));
        assert_eq!(l[0].1, vec!["install".to_owned(), "--from-tether".into(), format!("pipe:{NAME}")]);
        assert!(w.fake.staged_at_launch.get());
        let line = w.sent().unwrap();
        assert_eq!(line.as_object().unwrap().len(), 5);
        assert_eq!((line["token"].as_str(), line["tether_id"].as_str(), line["channel"].as_str()), (Some(w.token.as_str()), Some(ID), Some("stable")));
        assert_eq!(line["index"].as_str().unwrap().as_bytes(), &w.fake.docs.borrow()[0].1[..], "the raw signed bytes");
        let pk = tether_trust::key_to_b64(&SigningKey::from_bytes(&SEED).verifying_key());
        status::verify_agent_link(&pk, line["proof"].as_str().unwrap(), ID, UUID, &agent_pubkey()).unwrap();
        assert!(w.clean());
        // the per-user state names no secret
        let st = std::fs::read_to_string(w.fake.dir.join(USER_STATE)).unwrap();
        assert!(!st.contains(&w.token) && !st.contains(line["proof"].as_str().unwrap()));
        let s = w.host().status(NOW + 1);
        assert_eq!((s.state.as_str(), s.installed_by.as_str(), s.version.as_str(), s.detail.as_str()), ("installed", "session", "0.2.0", ""));
        // the dialog came first and said who asks, unverified
        let d = &w.fake.dialogs.borrow()[0];
        assert_eq!(d.title, "Install the health monitoring program?");
        assert_eq!(d.paragraphs[0], "A technician (support of Example Support) asks to install Tether Monitor.");
        assert_eq!(d.paragraphs[1], "Name given by the technician (not verified): Sam at the help desk");
        assert!(d.paragraphs[2].contains("stays on this computer after this session") && d.paragraphs[2].contains("relay.test"));
        assert!(d.paragraphs[3].contains("privacy policy") && d.paragraphs[3].contains("PRIVACY.md"), "{d:?}");
        assert!(d.paragraphs.iter().any(|p| p.contains("Settings › Apps › Tether Monitor")));
        assert_eq!(d.paragraphs.last().unwrap(), "Windows will ask for administrator approval next.");
        assert_eq!((d.yes.as_str(), d.no.as_str()), ("Install", "Don't install"));
        // the prompt is owned by this program's window, brought to the front first
        assert_eq!(w.fake.owner_at_launch.get(), Some((0x4711, true)));
    }

    #[test]
    fn the_proof_is_signed_with_this_programs_own_key() {
        let mut w = world("ownkey", "install");
        let other = [5u8; 32];
        w.p.identity = Some(("987654321".into(), other));
        assert_eq!(w.install().state, "installed");
        let line = w.sent().unwrap();
        assert_eq!(line["tether_id"].as_str(), Some("987654321"));
        let pk = tether_trust::key_to_b64(&SigningKey::from_bytes(&other).verifying_key());
        status::verify_agent_link(&pk, line["proof"].as_str().unwrap(), "987654321", UUID, &agent_pubkey()).unwrap();
        let wrong = tether_trust::key_to_b64(&SigningKey::from_bytes(&SEED).verifying_key());
        assert!(status::verify_agent_link(&wrong, line["proof"].as_str().unwrap(), "987654321", UUID, &agent_pubkey()).is_err());
    }

    #[test]
    fn declined_unanswered_and_dont_install_leave_nothing() {
        for (consent, launch, says) in [
            (Consent::No, None, "declined"),
            (Consent::NoAnswer, None, "did not answer"),
            (Consent::NoWindow, None, "could not be shown"),
            (Consent::Yes, Some(Launch::Declined), "Administrator approval was not given"),
        ] {
            let w = world("declined", "install");
            w.fake.consent.set(consent);
            *w.fake.launch.borrow_mut() = launch.clone();
            let a = w.install();
            assert_eq!(a.state, "not-installed", "{consent:?}");
            assert!(a.detail.contains(says), "{}", a.detail);
            assert!(w.sent().is_none() && w.clean());
            if launch.is_none() {
                assert_eq!(w.fake.fetches.get(), 0, "nothing downloaded before the answer");
                assert!(w.fake.launched.borrow().is_empty() && !w.fake.dir.join(STAGE_DIR).exists());
            }
            assert!(!w.fake.dir.join(USER_STATE).exists(), "no failure recorded");
        }
    }

    #[test]
    fn the_agent_refusing_or_a_stranger_on_the_pipe() {
        let w = world("refused", "install");
        w.fake.agent.set(Agent::Runs(1));
        let a = w.install();
        assert_eq!(a.state, "failed");
        assert!(a.detail.contains("exit code 1"), "{}", a.detail);
        assert!(w.clean());
        // another process of this user connects first: nothing is sent
        drop(w);
        let w = world("stranger", "install");
        w.fake.agent.set(Agent::Stranger);
        let a = w.install();
        assert!(a.state == "failed" && a.detail.contains("another program"), "{a:?}");
        assert!(w.sent().is_none() && w.clean());
        // Windows started another file than the verified one (swapped at its path): nothing sent
        drop(w);
        let w = world("swapped", "install");
        w.fake.agent.set(Agent::Swapped);
        let a = w.install();
        assert!(a.state == "failed" && a.detail.contains("not the verified agent"), "{a:?}");
        assert!(w.sent().is_none() && w.clean());
        assert_eq!(w.fake.connections.get(), 0, "not even accepted");
        // identity not as expected (another key form, too long): nothing sent
        drop(w);
        let w = world("identity", "install");
        *w.fake.identity.borrow_mut() = "x".repeat(600);
        assert_eq!(w.install().state, "failed");
        assert!(w.sent().is_none() && w.clean());
    }

    #[test]
    fn the_staged_file_is_verified_again_through_the_lock_before_the_launch() {
        let w = world("tamper", "install");
        w.fake.tamper.set(true);
        let a = w.install();
        assert!(a.state == "failed" && a.detail.contains("changed before it ran"), "{a:?}");
        assert!(w.fake.launched.borrow().is_empty() && w.clean());
        // a bad index: nothing staged or started
        drop(w);
        let w = world("badindex", "install");
        let other = keys();
        *w.fake.docs.borrow_mut() = vec![("https://relay.test".into(), index(&other, &w.fake.artifact, "0.2.0"))];
        assert_eq!(w.install().state, "failed");
        assert!(w.fake.launched.borrow().is_empty() && w.clean());
        let _ = &w.k;
    }

    #[test]
    fn the_policy_and_customer_elevate_refuse_before_any_dialog() {
        for (mode, elevate) in [("manual", true), ("yes", true), ("install", false), ("on-request", false)] {
            let mut w = world("refuse", mode);
            w.p.elevate = elevate;
            for action in [Action::Install, Action::Remove, Action::Status] {
                assert_eq!(w.host().request(action, Some("x"), NOW).state, "not-allowed", "{mode} {elevate}");
            }
            assert!(w.fake.dialogs.borrow().is_empty() && w.fake.fetches.get() == 0 && w.fake.launched.borrow().is_empty());
        }
        // remove mode: no install
        let w = world("removemode", "remove");
        assert_eq!(w.install().state, "not-installed");
        assert!(w.fake.dialogs.borrow().is_empty());
    }

    #[test]
    fn remove_asks_then_runs_the_installed_uninstall_by_tether() {
        let w = world("remove", "on-request");
        w.fake.svc.set(true);
        w.fake.consent.set(Consent::No);
        let a = w.host().request(Action::Remove, Some("Sam"), NOW);
        assert!(a.state == "installed-by-hand" && a.detail.contains("declined"), "{a:?}");
        assert!(w.fake.launched.borrow().is_empty());
        w.fake.consent.set(Consent::Yes);
        // a technician asks again only after the wait
        let a = w.host().request(Action::Remove, Some("Sam"), NOW + ASK_AGAIN_AFTER);
        assert_eq!(a.state, "removed-by-tether", "{a:?}");
        assert_eq!(w.fake.launched.borrow()[0], (w.fake.installed_exe().unwrap(), vec!["uninstall".to_owned(), "--by-tether".into()]));
        assert!(w.fake.dialogs.borrow()[1].title.starts_with("Remove"));
        assert_eq!(w.fake.owner_at_launch.get(), Some((0x4711, true)));
    }

    #[test]
    fn status_without_the_service_says_what_is_true_here() {
        let w = world("status", "install");
        assert_eq!(w.host().status(NOW).state, "not-installed");
        w.fake.svc.set(true);
        assert_eq!(w.host().status(NOW).state, "installed-by-hand");
        w.fake.svc.set(false);
        w.fake.removed.set(Some(NOW - 5));
        let s = w.host().status(NOW);
        assert_eq!((s.state.as_str(), s.at), ("removed-locally", NOW - 5));
    }

    #[test]
    fn an_agent_linked_to_another_id_says_so() {
        let mut w = world("otherid", "install");
        assert_eq!(w.install().state, "installed");
        // the same program, now with another ID (another profile, a new TetherQS ID)
        w.p.identity = Some(("555666777".into(), SEED));
        let s = w.host().status(NOW + 5);
        assert_eq!((s.state.as_str(), s.detail.as_str()), ("installed", "linked to another ID (123456789)"));
    }

    #[test]
    fn the_window_answers_only_the_dialog_it_was_shown() {
        let d = dialog(next_id(), &Ask { kind: Kind::Install, viewer: None, org: "".into(), support_org: "".into(), relays: vec![], prompt: true });
        assert_eq!(d.paragraphs[0], "Your organisation asks to install Tether Monitor.");
        let id = d.id;
        let t = std::thread::spawn(move || ask_window(d, || true, Duration::from_secs(5)));
        let start = std::time::Instant::now();
        while pending().is_none() && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(pending().map(|p| p.id), Some(id));
        assert!(!answer(id + 1000, true), "another dialog's answer");
        assert!(answer(id, false));
        assert_eq!(t.join().unwrap(), Consent::No);
        assert!(pending().is_none());
        // no answer in time
        let d = dialog(next_id(), &Ask { kind: Kind::Remove, viewer: Some("\u{202e}evil\u{0007}".into()), org: "Org".into(), support_org: "".into(), relays: vec![], prompt: true });
        assert_eq!(d.paragraphs[1], "Name given by the technician (not verified): evil", "{d:?}");
        assert_eq!(ask_window(d, || true, Duration::from_millis(50)), Consent::NoAnswer);
        assert!(pending().is_none());
    }

    #[test]
    fn a_technician_is_refused_where_windows_would_not_prompt() {
        let cases = [
            Uac { elevated: true, admin: true, ..Default::default() },
            Uac { lua_off: true, ..Default::default() },
            Uac { admin_silent: true, admin: true, ..Default::default() },
            Uac { elevated_service: true, ..Default::default() },
        ];
        for uac in cases {
            assert!(uac.silent(), "{uac:?}");
            let w = world("silent", "on-request");
            w.fake.uac.set(uac);
            let a = w.install();
            assert_eq!((a.state.as_str(), a.detail.as_str()), ("not-allowed", "Windows would not ask for confirmation on this computer; the person at the computer can install it there."), "{uac:?}");
            w.fake.svc.set(true);
            let a = w.host().request(Action::Remove, Some("Sam"), NOW);
            assert!(a.state == "not-allowed" && a.detail.contains("can remove it there"), "{a:?}");
            assert!(w.fake.dialogs.borrow().is_empty() && w.fake.fetches.get() == 0 && w.fake.launched.borrow().is_empty(), "nothing shown or started");
            // the person here can: their dialog says that no Windows prompt follows
            w.fake.svc.set(false);
            assert_eq!(w.host().request(Action::Install, None, NOW).state, "installed", "{uac:?}");
            let d = &w.fake.dialogs.borrow()[0];
            assert_eq!(d.paragraphs.last().unwrap(), "No Windows prompt will follow: this computer gives administrator rights without asking.");
            assert!(!d.paragraphs.iter().any(|p| p.contains("after this session")), "{d:?}");
        }
        // administrators elevated without asking: a standard user is still asked for credentials
        assert!(!Uac { admin_silent: true, ..Default::default() }.silent());
        let w = world("prompting", "on-request");
        w.fake.uac.set(Uac { admin_silent: true, ..Default::default() });
        assert_eq!(w.install().state, "installed");
    }

    #[test]
    fn a_no_holds_a_technician_off_for_ten_minutes_not_the_person_here() {
        for (consent, launch) in [(Consent::No, None), (Consent::NoAnswer, None), (Consent::Yes, Some(Launch::Declined))] {
            let w = world("again", "install");
            w.fake.consent.set(consent);
            *w.fake.launch.borrow_mut() = launch;
            assert_eq!(w.install().state, "not-installed");
            w.fake.consent.set(Consent::Yes);
            *w.fake.launch.borrow_mut() = None;
            let a = w.host().request(Action::Install, Some("Sam"), NOW + 60);
            assert_eq!((a.state.as_str(), a.detail.as_str()), ("busy", "The person at the computer said no a moment ago; ask again in 9 minutes."), "{consent:?}");
            assert_eq!(w.host().request(Action::Install, Some("Sam"), NOW + ASK_AGAIN_AFTER - 30).detail, "The person at the computer said no a moment ago; ask again in 1 minute.");
            assert_eq!(w.fake.dialogs.borrow().len(), 1, "no second dialog");
            // the person's own click is not held off
            assert_eq!(w.host().request(Action::Install, None, NOW + 60).state, "installed");
        }
        // a window that could not be shown is no "no"
        let w = world("nowindow", "install");
        w.fake.consent.set(Consent::NoWindow);
        w.install();
        w.fake.consent.set(Consent::Yes);
        assert_eq!(w.host().request(Action::Install, Some("Sam"), NOW + 1).state, "installed");
    }

    #[test]
    fn status_is_busy_while_the_dialog_or_the_prompt_is_open() {
        let w = world("busy", "install");
        let held = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        DOING.store(1, Ordering::Relaxed);
        let a = w.host().status(NOW);
        assert!(a.state == "busy" && a.detail.contains("installing the agent"), "{a:?}");
        DOING.store(2, Ordering::Relaxed);
        assert!(w.host().status(NOW).detail.contains("removing the agent"));
        assert_eq!(w.install().state, "busy");
        drop(held);
        assert_eq!(w.host().status(NOW).state, "not-installed");
    }

    #[test]
    fn the_support_organisation_stands_on_its_own_line_when_known() {
        let ask = |org: &str, viewer: Option<&str>| dialog(1, &Ask { kind: Kind::Install, viewer: viewer.map(Into::into), org: "TetherQS".into(), support_org: org.into(), relays: vec![], prompt: true });
        let d = ask("Example Practice IT\u{202e}\u{0007}", Some("Sam"));
        assert_eq!(d.paragraphs[0], "A technician (support of TetherQS) asks to install Tether Monitor.");
        assert_eq!(d.paragraphs[1], "Your support organisation: Example Practice IT");
        assert_eq!(d.paragraphs[2], "Name given by the technician (not verified): Sam");
        assert_eq!(ask(&"y".repeat(500), None).paragraphs[1], format!("Your support organisation: {}", "y".repeat(64)));
        // none known (the desktop's policy has none): no line, the product name stays in the sentence
        for none in ["", " \u{2060} "] {
            assert!(!ask(none, Some("Sam")).paragraphs.iter().any(|p| p.contains("support organisation")));
        }
    }

    #[test]
    fn a_machine_policy_manual_refuses_everything_before_any_dialog() {
        for (raw, manual) in [
            (r#"{"health.agent":{"state":"locked","value":"manual"}}"#, true),
            (r#"{"health.agent":{"state":"on","value":"manual"}}"#, true),
            (r#"{"health.agent":{"state":"off"}}"#, true),
            (r#"{"health.agent":{"state":"locked","value":"install"}}"#, false),
            // other rules, even unknown or broken ones, do not matter; only this one is read
            (r#"{"connect.chat":{"state":"off"},"x.unknown":{"bad":1},"health.agent":{"state":"locked","value":"manual"}}"#, true),
            (r#"{"connect.chat":{"state":"off"}}"#, false),
            (r#"{"health.agent":{"state":"locked","value":"manual","extra":1}}"#, false),
            ("not json", false),
            ("", false),
        ] {
            assert_eq!(machine_manual(Some(raw)), manual, "{raw}");
        }
        assert!(!machine_manual(None));
        let mut w = world("machineoff", "install");
        w.p.machine_off = true;
        for (action, viewer) in [(Action::Install, Some("x")), (Action::Remove, Some("x")), (Action::Status, Some("x")), (Action::Install, None)] {
            let a = w.host().request(action, viewer, NOW);
            assert_eq!((a.state.as_str(), a.detail.as_str()), ("not-allowed", "This computer's administrator has switched this off (machine policy)."), "{action:?} {viewer:?}");
        }
        assert!(w.fake.dialogs.borrow().is_empty() && w.fake.fetches.get() == 0 && w.fake.launched.borrow().is_empty());
    }

    #[test]
    fn the_name_stands_on_its_own_line_cleaned_and_bounded() {
        let ask = |viewer: &str| dialog(1, &Ask { kind: Kind::Install, viewer: Some(viewer.into()), org: "Org\u{2066}".into(), support_org: "".into(), relays: vec!["relay\u{2028}.test".into()], prompt: true });
        let d = ask("Sam\u{061c}\u{2028}\u{2029}\u{2060}\u{2061}\u{2064}\u{200b} says: approve");
        assert_eq!(d.paragraphs[1], "Name given by the technician (not verified): Sam says: approve");
        assert!(d.paragraphs[0].starts_with("A technician (support of Org)") && !d.paragraphs[0].contains("Sam"), "{d:?}");
        assert!(d.paragraphs[2].contains("relay.test"), "{d:?}");
        let long = ask(&"x".repeat(500));
        assert_eq!(long.paragraphs[1].chars().filter(|c| *c == 'x').count(), 64);
        // a name of only hidden characters: no name line
        assert!(!ask("\u{2060}\u{202e}").paragraphs.iter().any(|p| p.starts_with("Name given")));
        // the person here: no technician, no "after this session"
        let d = dialog(2, &Ask { kind: Kind::Install, viewer: None, org: "Org".into(), support_org: "".into(), relays: vec![], prompt: true });
        assert_eq!(d.paragraphs[0], "Your organisation (Org) asks to install Tether Monitor.");
        assert!(d.paragraphs[1].contains("stays on this computer,") && !d.paragraphs.iter().any(|p| p.contains("after this session")), "{d:?}");
        assert_eq!(tidy("a\u{2028}b\u{061c}c\u{2063}d"), "abcd");
    }

    #[test]
    fn the_agents_result_line_is_the_outcome_and_the_detail() {
        let w = world("result", "install");
        *w.fake.result.borrow_mut() = Some(r#"{"ok":true,"line":"installed, service running, first check cycle done"}"#.into());
        let a = w.install();
        assert_eq!(a.state, "installed");
        assert_eq!(a.detail, "Installed 0.2.0, linked to 123456789. The agent: installed, service running, first check cycle done");
        // refused: its line, never the token or the proof even if it carried them
        drop(w);
        let w = world("resultno", "install");
        w.fake.agent.set(Agent::Runs(1));
        let proof = status::sign_agent_link(&SEED, ID, UUID, &agent_pubkey());
        *w.fake.result.borrow_mut() = Some(serde_json::json!({ "ok": false, "line": format!("enrolment refused for {} with {proof}\u{0007}", w.token) }).to_string());
        let a = w.install();
        assert_eq!(a.state, "failed");
        assert!(a.detail.starts_with("the agent refused: enrolment refused for"), "{}", a.detail);
        assert!(!a.detail.contains(&w.token) && !a.detail.contains(&proof) && !a.detail.contains('\u{0007}'), "{}", a.detail);
        let st = std::fs::read_to_string(w.fake.dir.join(USER_STATE)).unwrap();
        assert!(!st.contains(&w.token) && !st.contains(&proof));
        // the service was made but did not start (exit 1): installed all the same, with its line
        drop(w);
        let w = world("unstarted", "install");
        w.fake.agent.set(Agent::Unstarted);
        *w.fake.result.borrow_mut() = Some(r#"{"ok":false,"line":"installed, but the service did not start: access denied"}"#.into());
        let a = w.install();
        assert!(a.state == "installed" && a.detail.contains("did not start"), "{a:?}");
        // an older agent sends nothing, or something else: the exit code as before
        drop(w);
        let w = world("noline", "install");
        w.fake.agent.set(Agent::Runs(3));
        *w.fake.result.borrow_mut() = Some("not json".into());
        assert!(w.install().detail.contains("exit code 3"));
        assert_eq!(agent_result(r#"{"ok":true}"#, &[]), None);
        // a secret too short for tidy's rule is taken out by name
        assert_eq!(agent_result(r#"{"ok":false,"line":"refused tok-123 here"}"#, &["tok-123", ""]), Some((false, "refused [removed] here".into())));
    }

    #[test]
    fn an_agent_still_running_after_the_wait_is_settled_later() {
        let w = world("pending", "install");
        w.fake.agent.set(Agent::Hangs);
        let a = w.install();
        assert!(a.state == "busy" && a.detail.contains("may still be running"), "{a:?}");
        let st = w.host().load();
        assert_eq!((st.state.as_str(), st.pending.as_ref().map(|p| (p.action.as_str(), p.by.as_str(), p.at))), ("", Some(("install", "session", NOW))), "no failure, the record kept");
        // a second ask meanwhile: busy, nothing started
        assert_eq!(w.host().request(Action::Install, None, NOW + 10).state, "busy");
        assert_eq!(w.fake.launched.borrow().len(), 1);
        assert_eq!(w.host().status(NOW + 10).state, "not-installed");
        assert!(w.host().load().pending.is_some(), "young: kept");
        // the agent finished meanwhile (or after Tether stopped): installed by the session, linked
        w.fake.svc.set(true);
        let s = w.host().status(NOW + 100);
        assert_eq!((s.state.as_str(), s.installed_by.as_str(), s.version.as_str(), s.detail.as_str()), ("installed", "session", "0.2.0", ""));
        assert!(w.host().load().pending.is_none() && !w.fake.dir.join(STAGE_DIR).join(STAGED_EXE).exists());
        drop(w);
        // a record from long ago claims nothing; one past the limits without a service is dropped
        for (svc, later, want) in [(true, PENDING_MAX + ADOPT_GRACE, "installed-by-hand"), (false, PENDING_MAX, "not-installed")] {
            let w = world("oldpending", "install");
            w.host().mark("install", "policy", "0.2.0", ID);
            w.fake.svc.set(svc);
            assert_eq!(w.host().status(NOW + later).state, want);
            assert!(w.host().load().pending.is_none());
        }
        // a removal that ended while Tether was not waiting
        let w = world("pendingremove", "install");
        w.host().mark("remove", "session", "", "");
        assert_eq!(w.host().status(NOW + 5).state, "removed-by-tether");
        // a staged file left behind goes when nothing of ours runs
        std::fs::create_dir_all(w.fake.dir.join(STAGE_DIR)).unwrap();
        std::fs::write(w.fake.dir.join(STAGE_DIR).join(STAGED_EXE), b"x").unwrap();
        w.host().status(NOW + 6);
        assert!(!w.fake.dir.join(STAGE_DIR).join(STAGED_EXE).exists());
    }

    #[test]
    fn removal_needs_a_by_tether_agent_and_the_services_own_program() {
        for v in [None, Some("0.1.9")] {
            let w = world("oldagent", "install");
            w.fake.svc.set(true);
            *w.fake.version.borrow_mut() = v.map(str::to_owned);
            let a = w.host().request(Action::Remove, Some("Sam"), NOW);
            assert!(a.detail.contains("older than 0.2.0") && a.detail.contains("Settings › Apps") && a.detail.contains("No prompt was started"), "{a:?}");
            assert!(w.fake.dialogs.borrow().is_empty() && w.fake.launched.borrow().is_empty());
        }
        for image in [r#""C:\Users\x\AppData\evil.exe" service"#, r"C:\Program Files\Tether Monitor\tether-monitor.exe service", ""] {
            let w = world("image", "install");
            w.fake.svc.set(true);
            *w.fake.image.borrow_mut() = image.into();
            let a = w.host().request(Action::Remove, Some("Sam"), NOW);
            assert!(a.detail.contains("runs another program") && a.detail.contains("nothing was started"), "{image}: {a:?}");
            assert!(w.fake.dialogs.borrow().is_empty() && w.fake.launched.borrow().is_empty());
        }
        assert_eq!(service_exe(r#""C:\Program Files\Tether Monitor\tether-monitor.exe" service"#), r"C:\Program Files\Tether Monitor\tether-monitor.exe");
        assert_eq!(service_exe(r"C:\x\a.exe service"), r"C:\x\a.exe");
        assert!(same_file_path(r"\\?\C:\Program Files\X.exe", r"c:\program files\x.exe"));
    }

    /// The program folder comes from the known-folder API, not from `%ProgramFiles%`.
    #[cfg(windows)]
    #[test]
    fn the_program_folder_does_not_follow_the_environment() {
        let real = win::program_files().unwrap();
        let old = std::env::var_os("ProgramFiles");
        std::env::set_var("ProgramFiles", std::env::temp_dir());
        let seen = win::installed_exe();
        match old {
            Some(o) => std::env::set_var("ProgramFiles", o),
            None => std::env::remove_var("ProgramFiles"),
        }
        assert_eq!(seen.unwrap(), real.join(AGENT_FOLDER).join(AGENT_EXE));
        assert!(!real.starts_with(std::env::temp_dir()));
        // no Tether Monitor service on this machine: its configuration cannot be read
        assert!(win::service_image().is_err() || win::service_exists().unwrap_or(false));
    }

    #[test]
    fn pipe_names_are_what_the_agent_accepts() {
        assert!(valid_pipe_name(NAME));
        for bad in ["tether-agent-0A0A0A0A0A0A0A0A0A0A0A0A0A0A0A0A", "tether-agent-00", "other-0123456789abcdef0123456789abcdef"] {
            assert!(!valid_pipe_name(bad), "{bad}");
        }
        #[cfg(windows)]
        {
            let a = win::pipe_name().unwrap();
            assert!(valid_pipe_name(&a) && a != win::pipe_name().unwrap());
        }
    }

    /// The real pipe: one instance only, a second server or a second client is refused, the line
    /// framing and the client's process ID (this process, as client here).
    #[cfg(windows)]
    #[test]
    fn the_real_pipe_takes_one_connection_and_refuses_a_second() {
        use std::io::{BufRead, Write};
        let name = win::pipe_name().unwrap();
        let mut server = win::create_pipe(&name).unwrap();
        assert!(win::create_pipe(&name).is_err(), "a second instance");
        assert!(win::pipe_sddl().unwrap().starts_with("D:P(A;;GA;;;S-1-5-"));
        let path = format!(r"\\.\pipe\{name}");
        let client = std::fs::OpenOptions::new().read(true).write(true).open(&path).unwrap();
        assert!(std::fs::OpenOptions::new().read(true).write(true).open(&path).is_err(), "a second client");
        struct Me;
        impl Child for Me {
            fn pid(&self) -> u32 { std::process::id() }
            fn runs(&self, _: &dyn std::any::Any) -> bool { true }
            fn wait(&mut self, _: Duration) -> Option<u32> { None }
        }
        assert_eq!(server.accept(&mut Me, Duration::from_secs(5)).unwrap(), std::process::id());
        let mut w = client.try_clone().unwrap();
        w.write_all(b"{\"uuid\":\"u\"}\n").unwrap();
        assert_eq!(server.read_line(IDENTITY_MAX, Duration::from_secs(5)).unwrap(), "{\"uuid\":\"u\"}");
        let big = format!("{}\n", "i".repeat(300_000));
        let reader = std::thread::spawn(move || {
            let mut l = String::new();
            std::io::BufReader::new(client).read_line(&mut l).unwrap();
            l
        });
        server.write_all(big.as_bytes(), Duration::from_secs(10)).unwrap();
        assert_eq!(reader.join().unwrap(), big);
        // a line that never ends is bounded
        drop(w);
        assert!(server.read_line(IDENTITY_MAX, Duration::from_millis(300)).is_err());
    }
}

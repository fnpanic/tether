//! Portable self-update (no installer): `download/<channel>/customer/<platform>/latest.json`,
//! minisign-verified exe (Windows) or zipped `.app` (macOS), swapped in next to the running one,
//! then relaunched. Every relay is asked, the highest version wins; the signed trusted comment
//! must name the offered file and version (`tether_trust::release`), so an older signed build
//! cannot be offered under a newer version.
//!
//! Signed release index (docs/update-design.md): `download/<channel>/releases.json` from every
//! relay, verified against the admin key, the highest serial wins and raises the floor
//! (`releases-<channel>.serial`). Once a floor exists, `latest.json` is never used again. The
//! index carries the rollout ramp (stable bucket per device and release), halt and security
//! flags, and size + sha256 of the file (a gzip copy for the CDN). Checks run at start, every
//! `update.check-hours` (jittered), after a policy change, and after a release hint (hbbs push
//! or heartbeat answer, both only "look again"). A forced update waits until no session runs.

use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::{Condvar, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[cfg(test)]
use hbb_common::base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::{Deserialize, Serialize};
use tether_trust::{
    release::{self, Candidate},
    version,
};

use crate::policy;

const PUBKEY: Option<&str> = option_env!("TETHER_UPDATER_PUB_KEY");
#[cfg(not(target_os = "macos"))]
const PLATFORM: &str = "windows-x86_64";
#[cfg(target_os = "macos")]
const PLATFORM: &str = "darwin-universal";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_EXE: usize = 200 * 1024 * 1024;

#[derive(Deserialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub platforms: HashMap<String, Platform>,
    #[serde(default)]
    pub mandatory: bool,
    #[serde(default)]
    pub minimum_version: String,
    #[serde(default)]
    pub forced: bool,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Platform {
    pub signature: String,
    pub url: String,
}

#[derive(Serialize, Clone, Default)]
pub struct State {
    /// Newer version on the server.
    pub available: Option<String>,
    pub notes: String,
    /// Sessions are blocked until the update is installed.
    pub required: bool,
    /// Unix seconds when a forced update installs itself.
    pub forced_at: Option<u64>,
    pub installing: bool,
    pub error: Option<String>,
    /// Result of the last check, for the help page.
    pub last_check: Option<String>,
    /// Download progress while installing: `[bytes, total]` (total 0 = unknown).
    pub progress: Option<[u64; 2]>,
    /// Serial of the signed release index in use (0: none, legacy latest.json).
    pub serial: u64,
    /// The index is past its expiry: "update information is out of date" (D3, warn only).
    pub stale: bool,
    /// Heartbeat `update_state`: idle, waiting-rollout, downloading, staged, waiting-idle, failed,
    /// stale, halted.
    pub phase: String,
    /// A forced update waits for the running session to end.
    pub waiting_idle: bool,
}

static STATE: Mutex<State> = Mutex::new(State {
    available: None,
    notes: String::new(),
    required: false,
    forced_at: None,
    installing: false,
    error: None,
    last_check: None,
    progress: None,
    serial: 0,
    stale: false,
    phase: String::new(),
    waiting_idle: false,
});
/// The newer release this device may install (eligible, not halted).
static MANIFEST: Mutex<Option<Candidate>> = Mutex::new(None);
/// One forced-update waiter at a time.
static FORCED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn state() -> State {
    STATE.lock().unwrap().clone()
}

/// Accepting incoming sessions is blocked (enforced in `sink`, not only in the UI).
pub fn blocked() -> bool {
    STATE.lock().unwrap().required || version::below_minimum(VERSION, policy::current().get("update.minimum-version"))
}

/// `(newer, required)`: the stricter of manifest and policy wins (docs/api.md). A halted release
/// is not offered; a mandatory one requires only devices its rollout reached (`eligible`).
pub fn evaluate(current: &str, m: &Candidate, policy_min: &str, eligible: bool) -> (bool, bool) {
    let newer = !m.halted && version::compare(&m.version, current) == Some(std::cmp::Ordering::Greater);
    let required = (newer && eligible && m.mandatory) || version::below_minimum(current, &m.minimum_version) || version::below_minimum(current, policy_min);
    (newer, required)
}

/// Heartbeat fields: `(update_state, release serial, version in flight)`.
pub fn heartbeat_fields() -> (String, u64, String) {
    let s = STATE.lock().unwrap();
    (s.phase.clone(), s.serial, s.available.clone().unwrap_or_default())
}

// ----------------------------------------------------------------------------- hints

static HINTS: Mutex<Option<release::Hints>> = Mutex::new(None);

/// A relay's release generation changed (hbbs `ReleaseHint`, heartbeat `{"releases": G}`): check
/// after a random 0..spread s, at most once a minute per relay.
pub fn hint(host: &str, generation: u64, spread: u32) {
    let fire = HINTS.lock().unwrap().get_or_insert_with(Default::default).update(host, generation, now());
    if fire {
        hbb_common::log::info!("release hint from {host}: generation {generation}");
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(release::uniform_secs(spread.min(release::MAX_SPREAD_SECS) as u64)));
            wake();
        });
    }
}

/// The mediator writes hbbs hints into an option (rendezvous_mediator.rs); poll it.
fn watch_hints() {
    std::thread::spawn(|| loop {
        let raw = hbb_common::config::Config::get_option(release::HINT_OPTION);
        for (host, (g, spread)) in release::parse_hints(&raw) {
            hint(&host, g, spread);
        }
        std::thread::sleep(Duration::from_secs(2));
    });
}

static WAKE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

/// Check now (policy changed, release hint).
pub fn wake() {
    *WAKE.0.lock().unwrap() = true;
    WAKE.1.notify_all();
}

impl Manifest {
    pub fn candidate(&self) -> Option<Candidate> {
        let p = self.platforms.get(PLATFORM)?;
        Some(Candidate::legacy(&self.version, &self.notes, &p.url, &p.signature, self.mandatory, &self.minimum_version, self.forced))
    }
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Remove a file or a whole `.app` folder.
fn remove_any(p: &Path) -> io::Result<()> {
    if p.is_dir() {
        std::fs::remove_dir_all(p)
    } else {
        std::fs::remove_file(p)
    }
}

/// What an update replaces: the exe, or on macOS the `.app` bundle around
/// `TetherQS.app/Contents/MacOS/TetherQS`.
fn unit(exe: &Path) -> io::Result<PathBuf> {
    if cfg!(target_os = "macos") {
        let app = exe.ancestors().nth(3).filter(|a| a.extension().is_some_and(|e| e == "app"));
        return app.map(Path::to_path_buf).ok_or_else(|| io::Error::other("TetherQS is not running from its .app"));
    }
    Ok(exe.to_path_buf())
}

/// The executable inside a (new) unit, for the relaunch.
fn exe_in(unit: &Path, exe: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        unit.join("Contents/MacOS").join(exe.file_name().unwrap_or("TetherQS".as_ref()))
    } else {
        unit.to_path_buf()
    }
}

/// Swap a downloaded `new` file (or `.app`) in for the running one; Windows allows renaming a
/// running exe, not overwriting it, and macOS allows renaming a running bundle. Rolls back if the
/// second rename fails.
pub fn swap(exe: &Path, new: &Path) -> io::Result<()> {
    let old = with_suffix(exe, ".old");
    let _ = remove_any(&old);
    std::fs::rename(exe, &old)?;
    if let Err(e) = std::fs::rename(new, exe) {
        let _ = std::fs::rename(&old, exe);
        return Err(e);
    }
    Ok(())
}

/// Write the verified update (`write`: plain file or unpacked `.app`) next to the running `unit`,
/// or into `fallback_dir` if that folder is not writable. Returns the unit to start.
pub fn stage(unit: &Path, bytes: &[u8], fallback_dir: &Path, write: impl Fn(&Path, &[u8]) -> io::Result<()>) -> io::Result<PathBuf> {
    let new = with_suffix(unit, ".new");
    let _ = remove_any(&new);
    if write(&new, bytes).is_ok() {
        swap(unit, &new)?;
        return Ok(unit.to_path_buf());
    }
    let _ = remove_any(&new);
    std::fs::create_dir_all(fallback_dir)?;
    let target = fallback_dir.join(unit.file_name().unwrap_or("TetherQS.exe".as_ref()));
    let tmp = with_suffix(&target, ".new");
    let _ = remove_any(&tmp);
    write(&tmp, bytes)?;
    if target.exists() {
        swap(&target, &tmp)?;
    } else {
        std::fs::rename(&tmp, &target)?;
    }
    Ok(target)
}

/// Leftovers of the previous update; the old exe is no longer running once we are.
pub fn cleanup_old() {
    if let Some(u) = std::env::current_exe().ok().and_then(|e| unit(&e).ok()) {
        let _ = remove_any(&with_suffix(&u, ".old"));
        let _ = remove_any(&with_suffix(&u, ".new"));
    }
}

fn fallback_dir() -> PathBuf {
    let base = if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    };
    base.unwrap_or_else(std::env::temp_dir).join(crate::APP_NAME)
}

/// macOS update artifact: a zip (`ditto -c -k --keepParent TetherQS.app`) holding one `.app`.
/// Unpacked with the system `ditto`, which keeps the bundle's symlinks, modes and signature.
fn unpack_app(dest: &Path, zip: &[u8]) -> io::Result<()> {
    let work = with_suffix(dest, ".unpack");
    let _ = remove_any(&work);
    std::fs::create_dir_all(&work)?;
    let r = (|| {
        let z = work.join("update.zip");
        std::fs::write(&z, zip)?;
        let out = work.join("x");
        if !std::process::Command::new("/usr/bin/ditto").args(["-x", "-k"]).arg(&z).arg(&out).status()?.success() {
            return Err(io::Error::other("ditto could not unpack the update"));
        }
        let mut apps = std::fs::read_dir(&out)?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "app"));
        let app = match (apps.next(), apps.next()) {
            (Some(a), None) if a.join("Contents/MacOS").is_dir() => a,
            _ => return Err(io::Error::other("the update does not contain exactly one .app")),
        };
        std::fs::rename(app, dest)
    })();
    let _ = remove_any(&work);
    r
}

/// The verified plain exe with this instance's signed custom-app profile attached unchanged (inside
/// the Authenticode table, as the relay does it), so a branded TetherQS stays branded and signed.
fn write_exe(p: &Path, plain: &[u8], doc: &[u8]) -> io::Result<()> {
    use io::Write;
    let mut f = std::fs::File::create(p)?;
    if doc.is_empty() {
        f.write_all(plain)?;
    } else {
        f.write_all(&tether_trust::custom::attach(plain, doc))?;
    }
    f.sync_all()
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Policy `update.channel`, or `stable` if it breaks the rule of the technician app and the relay:
/// `[a-z0-9-]{1,32}`.
fn channel(raw: &str) -> &str {
    if (1..=32).contains(&raw.len()) && raw.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-') {
        raw
    } else {
        "stable"
    }
}

fn manifest_url(base: &str, channel: &str) -> String {
    format!("{base}/download/{channel}/customer/{PLATFORM}/latest.json")
}

/// Every server is asked; the highest version wins (one stale relay cannot hold devices back).
/// An unreachable server, or one without a release on the channel (`Ok(None)`, HTTP 404), does
/// not count. `Ok(None)`: no server has a release.
fn highest_manifest(urls: &[String], get: impl Fn(&str) -> Result<Option<Manifest>, String>) -> Result<Option<Manifest>, String> {
    let mut errors = Vec::new();
    let mut answered = false;
    let mut best: Option<Manifest> = None;
    for u in urls {
        match get(u) {
            Ok(Some(m)) => {
                answered = true;
                if best.as_ref().is_none_or(|b| version::compare(&m.version, &b.version) == Some(std::cmp::Ordering::Greater)) {
                    best = Some(m);
                }
            }
            Ok(None) => answered = true,
            Err(e) => errors.push(format!("{}: {e}", u.split('/').nth(2).unwrap_or(u))),
        }
    }
    if answered {
        Ok(best)
    } else if errors.is_empty() {
        Err("no server".into())
    } else {
        Err(errors.join("; "))
    }
}

/// Per URL: ETag and manifest of the last 200 (each relay has its own ETags).
static ETAGS: Mutex<Option<HashMap<String, (String, Manifest)>>> = Mutex::new(None);

fn get_manifest(client: &reqwest::blocking::Client, url: &str) -> Result<Option<Manifest>, String> {
    let etag = ETAGS.lock().unwrap().as_ref().and_then(|m| m.get(url)).map(|e| e.0.clone());
    let mut rq = client.get(url);
    if let Some(t) = &etag {
        rq = rq.header("If-None-Match", t);
    }
    let resp = rq.send().map_err(|e| crate::commands::with_causes(&e))?;
    match resp.status().as_u16() {
        304 => Ok(ETAGS.lock().unwrap().as_ref().and_then(|m| m.get(url)).map(|e| e.1.clone())),
        404 => Ok(None),
        200 => {
            let tag = resp.headers().get("ETag").and_then(|v| v.to_str().ok()).map(str::to_owned);
            let raw = tether_trust::read_limited(resp, 64 * 1024).map_err(|e| format!("update manifest: {e}"))?;
            let m: Manifest = serde_json::from_slice(&raw).map_err(|e| format!("invalid update manifest: {e}"))?;
            if let Some(t) = tag {
                ETAGS.lock().unwrap().get_or_insert_with(HashMap::new).insert(url.to_owned(), (t, m.clone()));
            }
            Ok(Some(m))
        }
        s => Err(format!("HTTP {s}")),
    }
}

/// One check against every server (highest version wins). Returns the text for
/// "Check for updates"; the same text (or the error) is kept in the state for the help page.
pub fn check() -> Result<String, String> {
    let r = check_inner();
    STATE.lock().unwrap().last_check = Some(match &r {
        Ok(t) => t.clone(),
        Err(e) => format!("Check failed: {e}"),
    });
    r
}

fn check_inner() -> Result<String, String> {
    let eff = policy::current();
    if PUBKEY.is_none() {
        return Err("updates are not configured in this build".into());
    }
    // Off: still checked, so a release minimum version blocks as in the technician app; nothing is
    // offered, downloaded or required by `mandatory` (the organisation deploys updates).
    let enabled = eff.on("update.enabled");
    let channel = channel(eff.get("update.channel"));
    let client = policy::http()?;
    let bases = policy::base_urls();
    let Some(m) = find_release(&client, &bases, channel)? else {
        *MANIFEST.lock().unwrap() = None;
        release::prune_staged(&fallback_dir(), None);
        let mut s = STATE.lock().unwrap();
        s.available = None;
        s.required = false;
        s.phase = "idle".into();
        return Ok(format!("TetherQS {VERSION} is up to date (no release on channel \"{channel}\")."));
    };
    let t = now();
    let bucket = release::bucket(&policy::device_uuid(), "customer", &m.version);
    let eligible = m.eligible(bucket, t, eff.get("update.rollout"));
    let (newer, required) = evaluate(VERSION, &m, eff.get("update.minimum-version"), enabled && eligible);
    let offer = enabled && newer && eligible;
    let auto = offer && (m.forced || (required && eff.on("update.force")));
    // Superseded or installed downloads (the running exe is the new one after a swap).
    release::prune_staged(&fallback_dir(), staged_path(&m).filter(|_| offer).as_deref());
    {
        let mut s = STATE.lock().unwrap();
        s.available = offer.then(|| m.version.clone());
        s.notes = m.notes.clone();
        s.required = required;
        s.serial = m.serial;
        s.stale = m.stale(t);
        s.phase = match () {
            _ if !enabled => "idle",
            _ if m.halted && version::compare(&m.version, VERSION) == Some(std::cmp::Ordering::Greater) => "halted",
            _ if newer && !eligible => "waiting-rollout",
            _ if s.stale => "stale",
            _ if offer && staged_path(&m).is_some_and(|p| p.exists()) => "staged",
            _ => "idle",
        }
        .into();
    }
    let text = match () {
        _ if !enabled => "Your organisation installs updates for you.".to_owned(),
        _ if offer => format!("Version {} is available (this is {VERSION}).", m.version),
        _ if newer => format!("TetherQS {VERSION} is up to date; {} is rolling out ({} % of devices so far).", m.version, m.rollout.percent(t)),
        _ => format!("TetherQS {VERSION} is up to date."),
    };
    *MANIFEST.lock().unwrap() = offer.then_some(m.clone());
    if offer {
        stage_in_background(m);
    }
    if auto {
        let secs: u64 = eff.get("update.restart-seconds").parse().unwrap_or(60);
        forced_when_idle(secs);
    }
    Ok(text)
}

/// Per index URL: ETag and document of the last 200 answer.
static INDEX_ETAGS: Mutex<Option<HashMap<String, (String, Vec<u8>)>>> = Mutex::new(None);

fn index_path(channel: &str) -> PathBuf {
    hbb_common::config::Config::path(format!("releases-{channel}.json"))
}

/// The release index `name` of one relay (`Ok(None)`: none published there).
fn get_index(client: &reqwest::blocking::Client, base: &str, channel: &str, name: &str) -> Result<Option<Vec<u8>>, String> {
    let url = format!("{base}/download/{channel}/{name}");
    let etag = INDEX_ETAGS.lock().unwrap().as_ref().and_then(|m| m.get(&url)).map(|e| e.0.clone());
    let mut rq = client.get(&url);
    if let Some(t) = &etag {
        rq = rq.header("If-None-Match", t);
    }
    let resp = rq.send().map_err(|e| crate::commands::with_causes(&e))?;
    match resp.status().as_u16() {
        304 => Ok(INDEX_ETAGS.lock().unwrap().as_ref().and_then(|m| m.get(&url)).map(|e| e.1.clone())),
        404 => Ok(None),
        200 => {
            let tag = resp.headers().get("ETag").and_then(|v| v.to_str().ok()).map(str::to_owned);
            let raw = tether_trust::read_limited(resp, release::MAX_INDEX).map_err(|e| format!("release index: {e}"))?;
            if let Some(t) = tag {
                INDEX_ETAGS.lock().unwrap().get_or_insert_with(HashMap::new).insert(url.clone(), (t, raw.clone()));
            }
            Ok(Some(raw))
        }
        s => Err(format!("HTTP {s}")),
    }
}

/// The release to consider: from the signed index when any relay has one (or this device ever
/// accepted one: then never from `latest.json` again), else the highest legacy manifest.
fn find_release(client: &reqwest::blocking::Client, bases: &[String], channel: &str) -> Result<Option<Candidate>, String> {
    let keys = policy::trusted_keys();
    let path = index_path(channel);
    let mut docs = Vec::new();
    let mut errors = Vec::new();
    // releases-v2.json first (xz copies; `choose` keeps the first of equal serials), then
    // releases.json; the highest serial over both wins, so a stale or missing v2 does no harm.
    for name in [release::INDEX_V2_FILE, release::INDEX_FILE] {
        for b in bases {
            match get_index(client, b, channel, name) {
                Ok(Some(raw)) => docs.push((b.clone(), raw)),
                Ok(None) => {}
                // Reachability is judged by releases.json, which every relay has.
                Err(e) if name == release::INDEX_FILE => errors.push(format!("{}: {e}", b.trim_start_matches("https://"))),
                Err(e) => hbb_common::log::info!("{name}: {e}"),
            }
        }
    }
    let chosen = if keys.is_empty() { Ok(None) } else { release::choose(&path, &docs, &keys, channel) };
    let from_index = |base: &str, ix: &release::Index| ix.releases.get(&format!("customer/{PLATFORM}")).map(|e| Candidate::from_entry(e, base, ix));
    match chosen {
        Ok(Some((base, ix))) => return Ok(from_index(&base, &ix)),
        Err(e) => hbb_common::log::warn!("release index: {e}"),
        Ok(None) => {}
    }
    if !keys.is_empty() && (release::floor(&path, &keys) > 0 || release::REQUIRE_SIGNED_INDEX) {
        // Signed metadata only: the cached index (the relays were unreachable or served old ones).
        let cached = release::load(&path, &keys, channel);
        return match (cached, bases.first()) {
            (Some(ix), Some(b)) => Ok(from_index(b, &ix)),
            _ if !errors.is_empty() => Err(errors.join("; ")),
            _ => Ok(None),
        };
    }
    let urls: Vec<String> = bases.iter().map(|b| manifest_url(b, channel)).collect();
    Ok(highest_manifest(&urls, |u| get_manifest(client, u))?.and_then(|m| m.candidate()))
}

/// Where a verified download of `c` waits for its install.
fn staged_path(c: &Candidate) -> Option<PathBuf> {
    release::valid_version(&c.version).then(|| fallback_dir().join(format!("update-{}.staged", c.version)))
}

/// Download and verify in the background as soon as the device may install it (D7: the install
/// itself waits for idle). Only one download at a time.
fn stage_in_background(c: Candidate) {
    static BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let Some(path) = staged_path(&c) else { return };
    if path.exists() || BUSY.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        STATE.lock().unwrap().phase = "downloading".into();
        let r = PUBKEY.ok_or("no updater key".to_owned()).and_then(|k| download(&c, k)).and_then(|b| {
            tether_trust::write_atomic(&path, &b).map_err(|e| e.to_string())
        });
        let mut s = STATE.lock().unwrap();
        match r {
            Ok(()) => s.phase = "staged".into(),
            Err(e) => {
                hbb_common::log::warn!("update download: {e}");
                s.phase = "failed".into();
            }
        }
        drop(s);
        BUSY.store(false, std::sync::atomic::Ordering::SeqCst);
    });
}

/// The idle gate (D7): no incoming session and no file transfer.
fn busy() -> bool {
    crate::sink::has_session()
}

/// Forced update: the countdown of `update.restart-seconds` starts only while no session runs and
/// starts over when one begins; then install (which re-checks the index first).
fn forced_when_idle(secs: u64) {
    if FORCED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        let r = forced_loop(secs, busy, || STATE.lock().unwrap().waiting_idle = busy(), |at| STATE.lock().unwrap().forced_at = at, || std::thread::sleep(Duration::from_secs(1)), install);
        if let Err(e) = r {
            hbb_common::log::error!("forced update failed: {e}");
        }
        FORCED.store(false, std::sync::atomic::Ordering::SeqCst);
    });
}

/// The countdown logic of [`forced_when_idle`], with the clock and the install injected (tests).
fn forced_loop(
    secs: u64,
    busy: impl Fn() -> bool,
    mark: impl Fn(),
    set_at: impl Fn(Option<u64>),
    tick: impl Fn(),
    install: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    'wait: loop {
        mark();
        if busy() {
            set_at(None);
            tick();
            continue;
        }
        set_at(Some(now() + secs));
        for _ in 0..secs {
            tick();
            if busy() {
                continue 'wait;
            }
        }
        set_at(None);
        return install();
    }
}

/// Download, verify, swap and relaunch. Only returns on failure.
pub fn install() -> Result<(), String> {
    let r = (|| {
        let key = PUBKEY.ok_or("no updater key in this build")?;
        // Re-check right before installing: a halted rollout stops here (R2).
        if let Err(e) = check_inner() {
            hbb_common::log::warn!("update re-check: {e}");
        }
        let c = MANIFEST.lock().unwrap().clone().ok_or("no update available (it may have been halted)")?;
        STATE.lock().unwrap().installing = true;
        let staged = staged_path(&c).and_then(|p| std::fs::read(p).ok()).filter(|b| c.verify(b, key).is_ok());
        let bytes = match staged {
            Some(b) => b,
            None => download(&c, key)?,
        };
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let replace = |e: io::Error| format!("could not replace the program: {e}");
        let u = unit(&exe).map_err(replace)?;
        let staged = if cfg!(target_os = "macos") {
            stage(&u, &bytes, &fallback_dir(), unpack_app)
        } else {
            stage(&u, &bytes, &fallback_dir(), |p, b| write_exe(p, b, crate::custom::doc()))
        };
        let target = exe_in(&staged.map_err(replace)?, &exe);
        if let Some(p) = staged_path(&c) {
            let _ = std::fs::remove_file(p);
        }
        hbb_common::log::info!("updated to {}, restarting {}", c.version, target.display());
        crate::relaunch(&target, false)
    })();
    let mut s = STATE.lock().unwrap();
    s.installing = false;
    s.progress = None;
    s.error = r.as_ref().err().cloned();
    r
}

/// Each source in turn (the packed copy first, https only), then the verified bytes.
fn download(c: &Candidate, key: &str) -> Result<Vec<u8>, String> {
    let mut last = String::from("no download URL");
    for (url, packed) in c.sources() {
        let url = &url;
        match download_one(url, c, packed).and_then(|b| c.accept(b, packed, key, unpack)) {
            Ok(b) => return Ok(b),
            Err(e) => {
                hbb_common::log::warn!("update download {url}: {e}");
                last = e;
            }
        }
    }
    Err(last)
}

/// A packed copy (`release::PACKED_ENCODINGS`) → the artifact, at most `max` bytes. Its size and
/// sha256 were checked against the signed index first, so only an admin-signed file gets here.
fn unpack(encoding: &str, data: &[u8], max: u64) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    match encoding {
        "gzip" => {
            std::io::Read::read_to_end(&mut std::io::Read::take(flate2::read::GzDecoder::new(data), max + 1), &mut out).map_err(|e| format!("bad gzip copy: {e}"))?;
        }
        "xz" => lzma_rs::xz_decompress(&mut std::io::BufReader::new(data), &mut out).map_err(|e| format!("bad xz copy: {e}"))?,
        e => return Err(format!("unknown packed encoding {e}")),
    }
    if out.len() as u64 > max {
        return Err(format!("the {encoding} copy unpacks larger than announced"));
    }
    Ok(out)
}

fn download_one(url: &str, c: &Candidate, packed: bool) -> Result<Vec<u8>, String> {
    if !url.starts_with("https://") && policy::dev_apis().is_none() {
        return Err("update URL must be https".to_owned());
    }
    let mut resp = policy::http()?
        .get(url)
        .timeout(Duration::from_secs(1800))
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("download failed: {}", crate::commands::with_causes(&e)))?;
    // Relays serve a few hundred KB/s: show progress, the download can take minutes. A gzip
    // transfer (relay `precompressed`) is decoded by reqwest; its Content-Length is the packed size.
    let limit = c.limit(packed).min(MAX_EXE as u64);
    let total = if c.signed() { limit } else { resp.content_length().unwrap_or(0) };
    let mut bytes = Vec::with_capacity((total as usize).min(MAX_EXE));
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = io::Read::read(&mut resp, &mut buf).map_err(|e| format!("download failed: {e}"))?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..n]);
        if bytes.len() as u64 > limit {
            return Err("update too large".into());
        }
        STATE.lock().unwrap().progress = Some([bytes.len() as u64, total.max(bytes.len() as u64)]);
    }
    Ok(bytes)
}

/// Check at start (after a random 0-60 s), every `update.check-hours` (+0-20 % jitter), after a
/// policy change ([`wake`]), and 15 minutes after a failed check.
pub fn spawn() {
    std::thread::spawn(|| {
        cleanup_old();
        if PUBKEY.is_none() {
            hbb_common::log::warn!("no TETHER_UPDATER_PUB_KEY: self-update disabled");
            return;
        }
        watch_hints();
        std::thread::sleep(Duration::from_secs(release::uniform_secs(60)));
        loop {
            let eff = policy::current();
            let hours: u64 = eff.get("update.check-hours").parse().unwrap_or(6);
            let wait = match check() {
                Ok(t) => {
                    hbb_common::log::info!("update check: {t}");
                    release::jitter(Duration::from_secs(hours.max(1) * 3600), 20)
                }
                // No network yet or every server down: try again soon, not in `check-hours`.
                Err(e) => {
                    hbb_common::log::warn!("update check: {e}");
                    release::jitter(Duration::from_secs(15 * 60), 20)
                }
            };
            let woken = WAKE.0.lock().unwrap();
            let (mut woken, _) = WAKE.1.wait_timeout_while(woken, wait, |w| !*w).unwrap();
            *woken = false;
        }
    });
}

#[cfg(test)]
mod tests {

    #[test]
    fn unpack_gzip_and_xz_with_limit() {
        let plain: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let mut xz = Vec::new();
        lzma_rs::xz_compress(&mut std::io::BufReader::new(&plain[..]), &mut xz).unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut gz, &plain).unwrap();
        let gz = gz.finish().unwrap();
        let n = plain.len() as u64;
        assert_eq!(unpack("xz", &xz, n).unwrap(), plain);
        assert_eq!(unpack("gzip", &gz, n).unwrap(), plain);
        assert!(unpack("xz", &xz, n - 1).unwrap_err().contains("larger than announced"));
        assert!(unpack("gzip", &gz, n - 1).is_err());
        assert!(unpack("xz", &gz, n).unwrap_err().contains("bad xz copy"));
        assert!(unpack("zstd", &xz, n).unwrap_err().contains("unknown packed encoding"));
    }
    use super::*;
    use std::io::Cursor;

    fn manifest(v: &str, mandatory: bool, min: &str) -> Manifest {
        Manifest { version: v.into(), mandatory, minimum_version: min.into(), ..Default::default() }
    }

    fn cand(v: &str, mandatory: bool, min: &str) -> Candidate {
        Candidate { version: v.into(), mandatory, minimum_version: min.into(), ..Default::default() }
    }

    #[test]
    fn version_rules() {
        assert_eq!(evaluate("0.3.0", &cand("0.3.1", false, ""), "", true), (true, false));
        assert_eq!(evaluate("0.3.0", &cand("0.3.1", true, ""), "", true), (true, true));
        assert_eq!(evaluate("0.3.1", &cand("0.3.1", true, ""), "", true), (false, false));
        assert_eq!(evaluate("0.3.0", &cand("0.3.2", false, "0.3.1"), "", true), (true, true));
        assert_eq!(evaluate("0.3.0", &cand("0.3.0", false, ""), "0.3.1", true), (false, true));
        assert_eq!(evaluate("0.3.1-test", &cand("0.3.1", false, ""), "", true), (true, false));
        // Not reached by the rollout yet: mandatory does not block; a halted release is not offered.
        assert_eq!(evaluate("0.3.0", &cand("0.3.1", true, ""), "", false), (true, false));
        assert_eq!(evaluate("0.3.0", &Candidate { halted: true, ..cand("0.3.1", true, "") }, "", true), (false, false));
    }

    /// D7: the countdown starts only while idle, starts over when a session begins, and the
    /// install runs only after a full idle countdown.
    #[test]
    fn forced_update_waits_for_idle() {
        use std::cell::{Cell, RefCell};
        // Session for ticks 0-2, idle 3-4, session at 5, then idle.
        let t = Cell::new(0u32);
        let sessions = [true, true, true, false, false, true, false, false, false, false, false];
        let busy = || sessions.get(t.get() as usize).copied().unwrap_or(false);
        let log = RefCell::new(Vec::new());
        let installed_at = Cell::new(None);
        let r = forced_loop(
            3,
            &busy,
            || {},
            |at| log.borrow_mut().push(at.is_some()),
            || t.set(t.get() + 1),
            || {
                installed_at.set(Some(t.get()));
                assert!(!busy(), "installed during a session");
                Ok(())
            },
        );
        assert!(r.is_ok());
        // Countdown 3 ticks from tick 3 is cut at tick 5; from 6 it runs through 9.
        assert_eq!(installed_at.get(), Some(9));
        assert!(log.borrow().contains(&false) && log.borrow().contains(&true));
    }

    #[test]
    fn manifest_urls_and_channel() {
        assert_eq!(channel("beta-2"), "beta-2");
        assert_eq!(channel(""), "stable");
        assert_eq!(channel("Beta"), "stable");
        assert_eq!(channel("../x"), "stable");
        assert_eq!(channel(&"a".repeat(33)), "stable");
        let platform = if cfg!(target_os = "macos") { "darwin-universal" } else { "windows-x86_64" };
        assert_eq!(manifest_url("https://r1", "stable"), format!("https://r1/download/stable/customer/{platform}/latest.json"));
    }

    #[test]
    fn highest_release_over_servers_wins() {
        let urls = ["https://r1/x".to_owned(), "https://r2/x".to_owned()];
        let got = |r1: Result<Option<&str>, &str>, r2: Result<Option<&str>, &str>| {
            highest_manifest(&urls, |u| {
                let r = if u.starts_with("https://r1") { r1 } else { r2 };
                r.map(|o| o.map(|v| manifest(v, false, ""))).map_err(str::to_owned)
            })
            .map(|m| m.map(|m| m.version))
        };
        assert_eq!(got(Ok(Some("1.0.0")), Ok(Some("2.0.0"))), Ok(Some("2.0.0".into())));
        assert_eq!(got(Ok(Some("2.0.0")), Ok(Some("1.0.0"))), Ok(Some("2.0.0".into())));
        assert_eq!(got(Err("down"), Ok(Some("2.0.0"))), Ok(Some("2.0.0".into())));
        assert_eq!(got(Ok(None), Ok(Some("2.0.0"))), Ok(Some("2.0.0".into())));
        assert_eq!(got(Ok(None), Err("down")), Ok(None));
        assert_eq!(got(Err("refused"), Err("timeout")), Err("r1: refused; r2: timeout".into()));
        assert_eq!(highest_manifest(&[], |_| Ok(None)).unwrap_err(), "no server");
    }

    /// Against the production relays (read-only): `cargo test -- --ignored prod_release`.
    #[test]
    #[ignore]
    fn prod_release() {
        let client = policy::http().unwrap();
        for base in policy::base_urls() {
            let m = get_manifest(&client, &manifest_url(&base, "stable")).unwrap().expect("no release");
            let p = &m.platforms[PLATFORM];
            let bytes = client.get(&p.url).timeout(Duration::from_secs(600)).send().unwrap().bytes().unwrap();
            m.candidate().unwrap().verify(&bytes, PUBKEY.unwrap()).unwrap();
            println!("{base}: {} {} bytes, signature ok, newer than {VERSION}: {:?}", m.version, bytes.len(), evaluate(VERSION, &m.candidate().unwrap(), "", true).0);
        }
    }

    #[test]
    fn minisign_tauri_format_and_trusted_comment() {
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let data = b"TetherQS.exe bytes";
        let trusted = "timestamp:1\tfile:TetherQS-0.5.24.exe\tversion:0.5.24";
        let sig_text = minisign::sign(Some(&kp.pk), &kp.sk, Cursor::new(&data[..]), Some(trusted), None).unwrap().to_string();
        let sig = B64.encode(&sig_text);
        let pk_tauri = B64.encode(kp.pk.to_box().unwrap().to_string());
        let c = |v: &str| Candidate::legacy(v, "", &format!("https://r/download/stable/customer/windows-x86_64/TetherQS-{v}.exe"), &sig, false, "", false);
        assert!(c("0.5.24").verify(data, &pk_tauri).is_ok());
        assert!(c("0.5.24").verify(data, &kp.pk.to_base64()).is_ok());
        assert!(c("0.5.24").verify(b"tampered", &pk_tauri).is_err());
        // The same signed build offered as a newer version: refused (W9 rollback).
        assert!(c("0.5.30").verify(data, &pk_tauri).is_err());
        let other = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        assert!(c("0.5.24").verify(data, &B64.encode(other.pk.to_box().unwrap().to_string())).is_err());
        let mut bad = c("0.5.24");
        bad.signature = "not base64!".into();
        assert!(bad.verify(data, &pk_tauri).is_err());
    }

    #[test]
    fn swap_and_fallback() {
        let dir = std::env::temp_dir().join(format!("tetherqs-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("TetherQS.exe");
        std::fs::write(&exe, b"v1").unwrap();
        let fb = dir.join("fallback");
        let write = |p: &Path, b: &[u8]| std::fs::write(p, b);
        assert_eq!(stage(&exe, b"v2", &fb, write).unwrap(), exe);
        assert_eq!(std::fs::read(&exe).unwrap(), b"v2");
        assert_eq!(std::fs::read(with_suffix(&exe, ".old")).unwrap(), b"v1");
        assert!(!with_suffix(&exe, ".new").exists());
        // A failed swap rolls back.
        assert!(swap(&exe, &dir.join("missing.new")).is_err());
        assert_eq!(std::fs::read(&exe).unwrap(), b"v2");
        // Unwritable folder (here: a path below a file) falls back to the per-user folder.
        let blocked = dir.join("TetherQS.exe.old").join("TetherQS.exe");
        let target = stage(&blocked, b"v3", &fb, write).unwrap();
        assert_eq!(target, fb.join("TetherQS.exe"));
        assert_eq!(std::fs::read(&target).unwrap(), b"v3");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn update_keeps_custom_trailer() {
        let dir = std::env::temp_dir().join(format!("tetherqs-trailer-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("ACME-Support.exe");
        let doc = br#"{"payload":"x","sig":"y","key":"z"}"#;
        let v1 = tether_trust::custom::attach(b"MZ v1", doc);
        std::fs::write(&exe, &v1).unwrap();
        stage(&exe, b"MZ v2", &dir.join("fb"), |p, b| write_exe(p, b, doc)).unwrap();
        let got = std::fs::read(&exe).unwrap();
        let (plain, doc) = tether_trust::custom::split(&got);
        assert_eq!(plain, b"MZ v2");
        assert_eq!(doc, tether_trust::custom::split(&v1).1);
        // Plain TetherQS stays plain.
        stage(&exe, b"MZ v3", &dir.join("fb"), |p, b| write_exe(p, b, &[])).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"MZ v3");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The macOS path: the unit is a folder, a stale `.old` folder is replaced.
    #[test]
    fn swap_app_folders() {
        let dir = std::env::temp_dir().join(format!("tetherqs-app-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let app = dir.join("TetherQS.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::create_dir_all(dir.join("TetherQS.app.old/stale")).unwrap();
        let write = |p: &Path, b: &[u8]| {
            std::fs::create_dir_all(p.join("Contents/MacOS"))?;
            std::fs::write(p.join("Contents/MacOS/TetherQS"), b)
        };
        assert_eq!(stage(&app, b"v2", &dir.join("fb"), write).unwrap(), app);
        assert_eq!(std::fs::read(app.join("Contents/MacOS/TetherQS")).unwrap(), b"v2");
        assert!(dir.join("TetherQS.app.old/Contents/MacOS").is_dir() && !dir.join("TetherQS.app.old/stale").exists());
        assert!(!dir.join("TetherQS.app.new").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

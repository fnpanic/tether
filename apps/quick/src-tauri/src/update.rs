//! Portable self-update (no installer): `download/<channel>/customer/<platform>/latest.json`,
//! minisign-verified exe (Windows) or zipped `.app` (macOS), swapped in next to the running one,
//! then relaunched. Every relay is asked, the highest version wins; the signed trusted comment
//! must name the offered file and version (`tether_trust::release`), so an older signed build
//! cannot be offered under a newer version.

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
}

static STATE: Mutex<State> = Mutex::new(State { available: None, notes: String::new(), required: false, forced_at: None, installing: false, error: None, last_check: None, progress: None });
static MANIFEST: Mutex<Option<Manifest>> = Mutex::new(None);

pub fn state() -> State {
    STATE.lock().unwrap().clone()
}

/// Accepting incoming sessions is blocked (enforced in `sink`, not only in the UI).
pub fn blocked() -> bool {
    STATE.lock().unwrap().required || version::below_minimum(VERSION, policy::current().get("update.minimum-version"))
}

/// `(newer, required)`: the stricter of manifest and policy wins (docs/api.md).
pub fn evaluate(current: &str, m: &Manifest, policy_min: &str) -> (bool, bool) {
    let newer = version::compare(&m.version, current) == Some(std::cmp::Ordering::Greater);
    let required = (newer && m.mandatory) || version::below_minimum(current, &m.minimum_version) || version::below_minimum(current, policy_min);
    (newer, required)
}

static WAKE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

/// Check now (policy changed, release hint).
pub fn wake() {
    *WAKE.0.lock().unwrap() = true;
    WAKE.1.notify_all();
}

impl Manifest {
    fn candidate(&self) -> Option<Candidate> {
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

/// The verified plain exe plus this instance's custom-app trailer (unchanged), so a branded
/// TetherQS stays branded after updating.
fn write_exe(p: &Path, plain: &[u8], trailer: &[u8]) -> io::Result<()> {
    use io::Write;
    let mut f = std::fs::File::create(p)?;
    f.write_all(plain)?;
    f.write_all(trailer)?;
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
    if !eff.on("update.enabled") {
        return Ok("Your organisation installs updates for you.".into());
    }
    let channel = channel(eff.get("update.channel"));
    let client = policy::http()?;
    let urls: Vec<String> = policy::base_urls().iter().map(|b| manifest_url(b, channel)).collect();
    let Some(m) = highest_manifest(&urls, |u| get_manifest(&client, u))? else {
        *MANIFEST.lock().unwrap() = None;
        STATE.lock().unwrap().available = None;
        return Ok(format!("TetherQS {VERSION} is up to date (no release on channel \"{channel}\")."));
    };
    let (newer, required) = evaluate(VERSION, &m, eff.get("update.minimum-version"));
    let auto = newer && (m.forced || (required && eff.on("update.force")));
    {
        let mut s = STATE.lock().unwrap();
        s.available = newer.then(|| m.version.clone());
        s.notes = m.notes.clone();
        s.required = required;
        if auto && s.forced_at.is_none() {
            let secs: u64 = eff.get("update.restart-seconds").parse().unwrap_or(60);
            s.forced_at = Some(now() + secs);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(secs));
                if let Err(e) = install() {
                    hbb_common::log::error!("forced update failed: {e}");
                }
            });
        }
    }
    let text = if newer { format!("Version {} is available (this is {VERSION}).", m.version) } else { format!("TetherQS {VERSION} is up to date.") };
    *MANIFEST.lock().unwrap() = newer.then_some(m);
    Ok(text)
}

/// Download, verify, swap and relaunch. Only returns on failure.
pub fn install() -> Result<(), String> {
    let r = (|| {
        let m = MANIFEST.lock().unwrap().clone().ok_or("no update available")?;
        let c = m.candidate().ok_or("no build for this platform")?;
        STATE.lock().unwrap().installing = true;
        let bytes = download(&c, PUBKEY.ok_or("no updater key in this build")?)?;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let replace = |e: io::Error| format!("could not replace the program: {e}");
        let u = unit(&exe).map_err(replace)?;
        let staged = if cfg!(target_os = "macos") {
            stage(&u, &bytes, &fallback_dir(), unpack_app)
        } else {
            stage(&u, &bytes, &fallback_dir(), |p, b| write_exe(p, b, crate::custom::trailer()))
        };
        let target = exe_in(&staged.map_err(replace)?, &exe);
        hbb_common::log::info!("updated to {}, restarting {}", m.version, target.display());
        crate::relaunch(&target, false)
    })();
    let mut s = STATE.lock().unwrap();
    s.installing = false;
    s.progress = None;
    s.error = r.as_ref().err().cloned();
    r
}

/// Each URL in turn (https only), then the verified bytes.
fn download(c: &Candidate, key: &str) -> Result<Vec<u8>, String> {
    let mut last = String::from("no download URL");
    for url in &c.urls {
        match download_one(url, c).and_then(|b| c.verify(&b, key).map(|_| b)) {
            Ok(b) => return Ok(b),
            Err(e) => {
                hbb_common::log::warn!("update download {url}: {e}");
                last = e;
            }
        }
    }
    Err(last)
}

fn download_one(url: &str, c: &Candidate) -> Result<Vec<u8>, String> {
    if !url.starts_with("https://") {
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
    let total = if c.size > 0 { c.size } else { resp.content_length().unwrap_or(0) };
    let mut bytes = Vec::with_capacity((total as usize).min(MAX_EXE));
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = io::Read::read(&mut resp, &mut buf).map_err(|e| format!("download failed: {e}"))?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..n]);
        if bytes.len() > MAX_EXE || (c.size > 0 && bytes.len() as u64 > c.size) {
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
    use super::*;
    use std::io::Cursor;

    fn manifest(v: &str, mandatory: bool, min: &str) -> Manifest {
        Manifest { version: v.into(), mandatory, minimum_version: min.into(), ..Default::default() }
    }

    #[test]
    fn version_rules() {
        assert_eq!(evaluate("0.3.0", &manifest("0.3.1", false, ""), ""), (true, false));
        assert_eq!(evaluate("0.3.0", &manifest("0.3.1", true, ""), ""), (true, true));
        assert_eq!(evaluate("0.3.1", &manifest("0.3.1", true, ""), ""), (false, false));
        assert_eq!(evaluate("0.3.0", &manifest("0.3.2", false, "0.3.1"), ""), (true, true));
        assert_eq!(evaluate("0.3.0", &manifest("0.3.0", false, ""), "0.3.1"), (false, true));
        assert_eq!(evaluate("0.3.1-test", &manifest("0.3.1", false, ""), ""), (true, false));
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
            println!("{base}: {} {} bytes, signature ok, newer than {VERSION}: {:?}", m.version, bytes.len(), evaluate(VERSION, &m, "").0);
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
        let trailer = tether_trust::custom::trailer(br#"{"payload":"x","sig":"y","key":"z"}"#);
        let mut v1 = b"MZ v1".to_vec();
        v1.extend(&trailer);
        std::fs::write(&exe, &v1).unwrap();
        stage(&exe, b"MZ v2", &dir.join("fb"), |p, b| write_exe(p, b, &trailer)).unwrap();
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

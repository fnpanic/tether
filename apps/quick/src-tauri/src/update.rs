//! Portable self-update (no installer): `download/<channel>/customer/windows-x86_64/latest.json`,
//! minisign-verified exe, swapped in next to the running one, then relaunched.

use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use hbb_common::base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::{Deserialize, Serialize};
use tether_trust::version;

use crate::policy;

const PUBKEY: Option<&str> = option_env!("TETHER_UPDATER_PUB_KEY");
const PLATFORM: &str = "windows-x86_64";
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
}

static STATE: Mutex<State> = Mutex::new(State { available: None, notes: String::new(), required: false, forced_at: None, installing: false, error: None });
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

/// Tauri updater format: `signature` is base64 of the minisign signature file; the public key is
/// base64 of the minisign public key file (a bare `RW…` key line is accepted too).
pub fn verify(data: &[u8], signature: &str, pubkey: &str) -> Result<(), String> {
    let pk = B64
        .decode(pubkey.trim())
        .ok()
        .and_then(|raw| String::from_utf8(raw).ok())
        .filter(|t| t.contains("untrusted comment"))
        .map(|t| minisign_verify::PublicKey::decode(&t))
        .unwrap_or_else(|| minisign_verify::PublicKey::from_base64(pubkey.trim()))
        .map_err(|e| format!("bad updater key: {e}"))?;
    let text = B64.decode(signature.trim()).map_err(|e| format!("bad signature encoding: {e}"))?;
    let text = String::from_utf8(text).map_err(|_| "bad signature encoding".to_owned())?;
    let sig = minisign_verify::Signature::decode(&text).map_err(|e| format!("bad signature: {e}"))?;
    pk.verify(data, &sig, true).map_err(|e| format!("signature does not match: {e}"))
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Swap a downloaded `new` file in for the (running) `exe`; Windows allows renaming a running
/// exe, not overwriting it. Rolls back if the second rename fails.
pub fn swap(exe: &Path, new: &Path) -> io::Result<()> {
    let old = with_suffix(exe, ".old");
    let _ = std::fs::remove_file(&old);
    std::fs::rename(exe, &old)?;
    if let Err(e) = std::fs::rename(new, exe) {
        let _ = std::fs::rename(&old, exe);
        return Err(e);
    }
    Ok(())
}

/// Write the verified exe next to the running one, or into `fallback_dir` if that folder is not
/// writable. Returns the exe to start.
pub fn stage(exe: &Path, bytes: &[u8], fallback_dir: &Path) -> io::Result<PathBuf> {
    let new = with_suffix(exe, ".new");
    if std::fs::write(&new, bytes).is_ok() {
        swap(exe, &new)?;
        return Ok(exe.to_path_buf());
    }
    std::fs::create_dir_all(fallback_dir)?;
    let target = fallback_dir.join(exe.file_name().unwrap_or("TetherQS.exe".as_ref()));
    let tmp = with_suffix(&target, ".new");
    std::fs::write(&tmp, bytes)?;
    if target.exists() {
        swap(&target, &tmp)?;
    } else {
        std::fs::rename(&tmp, &target)?;
    }
    Ok(target)
}

/// Leftovers of the previous update; the old exe is no longer running once we are.
pub fn cleanup_old() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(with_suffix(&exe, ".old"));
        let _ = std::fs::remove_file(with_suffix(&exe, ".new"));
    }
}

fn fallback_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join(crate::APP_NAME)
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn check() -> Result<(), String> {
    let eff = policy::current();
    let base = policy::base_url().ok_or("no server")?;
    let channel = eff.get("update.channel");
    // Same rule as the technician app and the relay: [a-z0-9-]{1,32}.
    let channel = if (1..=32).contains(&channel.len()) && channel.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-') { channel } else { "stable" };
    let resp = policy::http()?
        .get(format!("{base}/download/{channel}/customer/{PLATFORM}/latest.json"))
        .send()
        .map_err(|e| e.to_string())?;
    if resp.status().as_u16() == 404 {
        return Ok(());
    }
    let m: Manifest = resp.error_for_status().map_err(|e| e.to_string())?.json().map_err(|e| e.to_string())?;
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
    *MANIFEST.lock().unwrap() = newer.then_some(m);
    Ok(())
}

/// Download, verify, swap and relaunch. Only returns on failure.
pub fn install() -> Result<(), String> {
    let r = (|| {
        let m = MANIFEST.lock().unwrap().clone().ok_or("no update available")?;
        let p = m.platforms.get(PLATFORM).ok_or("no build for this platform")?;
        if !p.url.starts_with("https://") {
            return Err("update URL must be https".to_owned());
        }
        STATE.lock().unwrap().installing = true;
        let bytes = policy::http()?
            .get(&p.url)
            .timeout(Duration::from_secs(600))
            .send()
            .and_then(|r| r.error_for_status())
            .and_then(|r| r.bytes())
            .map_err(|e| format!("download failed: {e}"))?;
        if bytes.len() > MAX_EXE {
            return Err("update too large".into());
        }
        verify(&bytes, &p.signature, PUBKEY.ok_or("no updater key in this build")?)?;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let target = stage(&exe, &bytes, &fallback_dir()).map_err(|e| format!("could not replace the program: {e}"))?;
        hbb_common::log::info!("updated to {}, restarting {}", m.version, target.display());
        crate::relaunch(&target, false)
    })();
    let mut s = STATE.lock().unwrap();
    s.installing = false;
    s.error = r.as_ref().err().cloned();
    r
}

/// Check at start and every `update.check-hours` while `update.enabled`.
pub fn spawn() {
    std::thread::spawn(|| {
        cleanup_old();
        if PUBKEY.is_none() {
            hbb_common::log::warn!("no TETHER_UPDATER_PUB_KEY: self-update disabled");
            return;
        }
        loop {
            let eff = policy::current();
            if eff.on("update.enabled") {
                if let Err(e) = check() {
                    hbb_common::log::warn!("update check: {e}");
                }
            }
            let hours: u64 = eff.get("update.check-hours").parse().unwrap_or(6);
            std::thread::sleep(Duration::from_secs(hours.max(1) * 3600));
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
    fn minisign_tauri_format() {
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let data = b"TetherQS.exe bytes";
        let sig_text = minisign::sign(Some(&kp.pk), &kp.sk, Cursor::new(&data[..]), None, None).unwrap().to_string();
        let sig = B64.encode(&sig_text);
        let pk_file = kp.pk.to_box().unwrap().to_string();
        let pk_tauri = B64.encode(&pk_file);
        let pk_bare = kp.pk.to_base64();
        assert!(verify(data, &sig, &pk_tauri).is_ok());
        assert!(verify(data, &sig, &pk_bare).is_ok());
        assert!(verify(b"tampered", &sig, &pk_tauri).is_err());
        let other = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        assert!(verify(data, &sig, &B64.encode(other.pk.to_box().unwrap().to_string())).is_err());
        assert!(verify(data, "not base64!", &pk_tauri).is_err());
    }

    #[test]
    fn swap_and_fallback() {
        let dir = std::env::temp_dir().join(format!("tetherqs-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("TetherQS.exe");
        std::fs::write(&exe, b"v1").unwrap();
        let fb = dir.join("fallback");
        assert_eq!(stage(&exe, b"v2", &fb).unwrap(), exe);
        assert_eq!(std::fs::read(&exe).unwrap(), b"v2");
        assert_eq!(std::fs::read(with_suffix(&exe, ".old")).unwrap(), b"v1");
        assert!(!with_suffix(&exe, ".new").exists());
        // A failed swap rolls back.
        assert!(swap(&exe, &dir.join("missing.new")).is_err());
        assert_eq!(std::fs::read(&exe).unwrap(), b"v2");
        // Unwritable folder (here: a path below a file) falls back to the per-user folder.
        let blocked = dir.join("TetherQS.exe.old").join("TetherQS.exe");
        let target = stage(&blocked, b"v3", &fb).unwrap();
        assert_eq!(target, fb.join("TetherQS.exe"));
        assert_eq!(std::fs::read(&target).unwrap(), b"v3");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

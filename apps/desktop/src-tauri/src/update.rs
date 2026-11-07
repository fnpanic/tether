//! Updates (docs/api.md "Download layout and update manifests", docs/updates.md).
//!
//! The manifest `https://<relay>/download/<channel>/technician/<target>/latest.json` is read here
//! (custom fields `mandatory`, `minimumVersion`, `forced`) and cached in the config dir, so every
//! process role can enforce it without network: below the policy or manifest minimum version, or
//! with a pending mandatory update, sessions are refused (`blocked`). Every relay is asked (with
//! `If-None-Match`), the highest version wins. The download is verified here (minisign,
//! `TETHER_UPDATER_PUB_KEY`, and the signed trusted comment must name the offered file and
//! version, `tether_trust::release`), and exactly those bytes go to `tauri-plugin-updater`'s
//! installer: the version checked is the version installed.

use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashMap},
    sync::{Condvar, LazyLock, Mutex},
    time::Duration,
};

use hbb_common::{config::Config, log};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::AppHandle;
use tauri_plugin_updater::UpdaterExt;
use tether_trust::{
    release::{self, Candidate},
    version::{below_minimum, compare},
};

use crate::policy;

/// Minisign public key (as `tauri signer generate` prints it); release builds require it.
const UPDATER_KEY: Option<&str> = option_env!("TETHER_UPDATER_PUB_KEY");
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub mandatory: bool,
    #[serde(default)]
    pub minimum_version: String,
    #[serde(default)]
    pub forced: bool,
    /// `{"windows-x86_64": {"url", "signature"}}` as in the Tauri manifest.
    #[serde(default)]
    pub platforms: BTreeMap<String, Platform>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Platform {
    pub url: String,
    pub signature: String,
}

impl Manifest {
    /// The installable release for this platform, if the manifest lists one.
    fn candidate(&self) -> Option<Candidate> {
        let p = self.platforms.get(&target())?;
        Some(Candidate::legacy(&self.version, &self.notes, &p.url, &p.signature, self.mandatory, &self.minimum_version, self.forced))
    }
}

/// Per relay: ETag and manifest of its last 200 (the relays' ETags differ).
static ETAGS: LazyLock<Mutex<HashMap<String, (String, Option<Manifest>)>>> = LazyLock::new(Default::default);

static LAST: LazyLock<Mutex<Option<Manifest>>> = LazyLock::new(|| {
    let cached = std::fs::read(Config::path("update-state.json")).ok();
    Mutex::new(cached.and_then(|raw| serde_json::from_slice(&raw).ok()))
});
static WAKE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

/// Updater `{{target}}` and manifest platform key: `windows-x86_64`, `darwin-universal`, …
pub fn target() -> String {
    if cfg!(target_os = "macos") {
        return "darwin-universal".into();
    }
    let os = if cfg!(windows) { "windows" } else { std::env::consts::OS };
    let arch = match std::env::consts::ARCH {
        "x86" => "i686",
        a => a,
    };
    format!("{os}-{arch}")
}

fn channel() -> String {
    let c = policy::get("update.channel");
    let ok = (1..=32).contains(&c.len()) && c.chars().all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-');
    if ok {
        c
    } else {
        "stable".into()
    }
}

fn manifest_path(target: &str) -> String {
    format!("/download/{}/technician/{target}/latest.json", channel())
}

// ----------------------------------------------------------------------------- enforcement

fn newer(m: &Manifest) -> bool {
    compare(&m.version, VERSION) == Some(Ordering::Greater)
}

/// Why sessions are blocked, if they are.
fn reason_for(m: Option<&Manifest>, policy_minimum: &str, updates_enabled: bool) -> Option<&'static str> {
    if below_minimum(VERSION, policy_minimum) || m.is_some_and(|m| below_minimum(VERSION, &m.minimum_version)) {
        return Some("below-minimum");
    }
    // A mandatory update can only be required when updates may be installed.
    (updates_enabled && m.is_some_and(|m| m.mandatory && newer(m))).then_some("mandatory")
}

fn reason() -> Option<&'static str> {
    let m = LAST.lock().unwrap().clone();
    reason_for(m.as_ref(), &policy::get("update.minimum-version"), policy::on("update.enabled"))
}

/// True while this version may not be used for sessions (outgoing or incoming).
pub fn blocked() -> bool {
    reason().is_some()
}

/// Session guard: `Err("locked:update-required: …")` while [`blocked`].
pub fn guard() -> Result<(), String> {
    if blocked() {
        Err("locked:update-required: Install the required Tether update to start sessions.".into())
    } else {
        Ok(())
    }
}

/// State for the UI (`update` event, `update_status`).
pub fn status() -> Value {
    let m = LAST.lock().unwrap().clone().filter(newer);
    let reason = reason();
    let enabled = policy::on("update.enabled");
    json!({
        "current": VERSION,
        "version": m.as_ref().map(|m| m.version.clone()),
        "notes": m.as_ref().map(|m| m.notes.clone()).unwrap_or_default(),
        "mandatory": reason.is_some(),
        "forced": enabled && m.as_ref().is_some_and(|m| m.forced || (reason.is_some() && policy::on("update.force"))),
        "blocked": reason,
        "enabled": enabled,
        "restartSeconds": policy::get("update.restart-seconds").parse::<u64>().unwrap_or(60),
    })
}

// ----------------------------------------------------------------------------- check / install

/// The manifest of one relay; 304 answers come from the remembered copy.
fn manifest_from(host: &str, path: &str) -> Result<Option<Manifest>, String> {
    let etag = ETAGS.lock().unwrap().get(host).map(|e| e.0.clone());
    let r = policy::api_via(&[host.to_owned()], |c, base| {
        let rq = c.get(format!("{base}{path}"));
        match &etag {
            Some(t) => rq.header("If-None-Match", t),
            None => rq,
        }
    })?;
    match r.status().as_u16() {
        304 => Ok(ETAGS.lock().unwrap().get(host).and_then(|e| e.1.clone())),
        404 => Ok(None),
        200 => {
            let tag = r.headers().get("ETag").and_then(|v| v.to_str().ok()).map(str::to_owned);
            let raw = tether_trust::read_limited(r, 64 * 1024).map_err(|e| format!("update manifest: {e}"))?;
            let m: Manifest = serde_json::from_slice(&raw).map_err(|e| format!("invalid update manifest: {e}"))?;
            if let Some(t) = tag {
                ETAGS.lock().unwrap().insert(host.to_owned(), (t, Some(m.clone())));
            }
            Ok(Some(m))
        }
        s => Err(format!("update server answered {s}")),
    }
}

/// Highest version over every relay's answer; an error only when no relay answered.
fn highest(answers: Vec<Result<Option<Manifest>, String>>) -> Result<Option<Manifest>, String> {
    let mut best: Option<Manifest> = None;
    let mut errors = Vec::new();
    let mut answered = false;
    for a in answers {
        match a {
            Ok(m) => {
                answered = true;
                if let Some(m) = m.filter(|m| best.as_ref().is_none_or(|b| compare(&m.version, &b.version) == Some(Ordering::Greater))) {
                    best = Some(m);
                }
            }
            Err(e) => errors.push(e),
        }
    }
    if answered {
        Ok(best)
    } else {
        Err(errors.join("; "))
    }
}

/// Ask every relay for the manifest (blocking). `Err("offline: …")` when none can be reached.
pub fn check() -> Result<Value, String> {
    let path = manifest_path(&target());
    let answers = policy::relay_bases().iter().map(|(host, _)| manifest_from(host, &path)).collect();
    let m = highest(answers)?;
    let raw = serde_json::to_vec(&m).unwrap_or_default();
    if let Err(e) = policy::write_atomic(&Config::path("update-state.json"), &raw) {
        log::warn!("cannot cache update state: {e}");
    }
    *LAST.lock().unwrap() = m;
    let s = status();
    crate::app::emit("update", s.clone());
    Ok(s)
}

/// Download the candidate (each URL in turn) and return the verified bytes.
fn download(c: &Candidate, key: &str) -> Result<Vec<u8>, String> {
    let mut last = String::from("no download URL");
    for url in &c.urls {
        match download_one(url, c).and_then(|b| c.verify(&b, key).map(|_| b)) {
            Ok(b) => return Ok(b),
            Err(e) => {
                log::warn!("update download {url}: {e}");
                last = e;
            }
        }
    }
    Err(last)
}

fn download_one(url: &str, c: &Candidate) -> Result<Vec<u8>, String> {
    let mut r = policy::fetch(url, Duration::from_secs(1800))?;
    let total = if c.size > 0 { c.size } else { r.content_length().unwrap_or(0) };
    let mut bytes = Vec::with_capacity(total.min(release::MAX_ARTIFACT) as usize);
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = std::io::Read::read(&mut r, &mut buf).map_err(|e| format!("download failed: {e}"))?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..n]);
        if bytes.len() as u64 > release::MAX_ARTIFACT || (c.size > 0 && bytes.len() as u64 > c.size) {
            return Err("the download is larger than announced".into());
        }
        crate::app::emit("update-progress", json!({ "done": bytes.len(), "total": total }));
    }
    Ok(bytes)
}

/// Download, verify and install the update, then restart. Progress: `update-progress` events.
pub async fn install(app: AppHandle) -> Result<(), String> {
    if !policy::on("update.enabled") {
        return Err("locked:update.enabled: Your organisation installs updates for you.".into());
    }
    let key = UPDATER_KEY.filter(|k| !k.is_empty()).ok_or("Updates are not configured in this build.")?;
    let c = LAST.lock().unwrap().clone().filter(newer).and_then(|m| m.candidate()).ok_or("No update is available.")?;
    let bytes = tauri::async_runtime::spawn_blocking({
        let c = c.clone();
        move || download(&c, key)
    })
    .await
    .map_err(|e| e.to_string())??;
    install_bytes(&app, key, &c, &bytes)?;
    app.restart();
}

/// Hand verified bytes to the plugin's installer (NSIS on Windows, the .app on macOS). The plugin
/// checks the signature and the signed version once more.
fn install_bytes(app: &AppHandle, key: &str, c: &Candidate, bytes: &[u8]) -> Result<(), String> {
    let mut b = app.updater_builder().pubkey(key).target(target());
    if let Some(p) = policy::proxy_url().and_then(|p| p.parse().ok()) {
        b = b.proxy(p);
    }
    let raw = json!({ "version": c.version, "notes": c.notes, "platforms": { target(): { "url": c.urls.first(), "signature": c.signature } } });
    let update = b.build().map_err(|e| e.to_string())?.restore_update(raw).map_err(|e| e.to_string())?.ok_or("No update is available.")?;
    update.install(bytes).map_err(|e| e.to_string())
}

/// Ask the checker to run now (policy changed).
pub fn on_policy_change() {
    *WAKE.0.lock().unwrap() = true;
    WAKE.1.notify_all();
}

/// Main UI: check on start (after a random 5-60 s), every `update.check-hours` (+0-20 % jitter,
/// so devices never poll in step), and after each policy change.
pub fn start() {
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(5 + release::uniform_secs(55)));
        loop {
            if let Err(e) = check() {
                log::info!("update check: {e}");
            }
            let hours = policy::get("update.check-hours").parse::<u64>().unwrap_or(6).max(1);
            let woken = WAKE.0.lock().unwrap();
            let wait = release::jitter(Duration::from_secs(hours * 3600), 20);
            let (mut woken, _) = WAKE.1.wait_timeout_while(woken, wait, |w| !*w).unwrap();
            *woken = false;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(version: &str, mandatory: bool, minimum: &str) -> Manifest {
        Manifest { version: version.into(), mandatory, minimum_version: minimum.into(), ..Default::default() }
    }

    #[test]
    fn blocking_rules() {
        assert_eq!(reason_for(None, "", true), None);
        assert_eq!(reason_for(None, "999.0.0", true), Some("below-minimum"));
        // Policy minimum still blocks with updates disabled.
        assert_eq!(reason_for(None, "999.0.0", false), Some("below-minimum"));
        assert_eq!(reason_for(Some(&m("999.0.0", false, "999.0.0")), "", true), Some("below-minimum"));
        assert_eq!(reason_for(Some(&m("999.0.0", true, "")), "", true), Some("mandatory"));
        assert_eq!(reason_for(Some(&m("999.0.0", true, "")), "", false), None);
        assert_eq!(reason_for(Some(&m("999.0.0", false, "")), "", true), None);
        // A mandatory release that is not newer does not block.
        assert_eq!(reason_for(Some(&m("0.0.1", true, "")), "", true), None);
    }

    #[test]
    fn manifest_custom_fields_parse() {
        let raw = r#"{"version":"0.3.1","notes":"n","pub_date":"2026-11-04T10:00:00Z","platforms":{},"mandatory":true,"minimumVersion":"0.3.0","forced":true}"#;
        let m: Manifest = serde_json::from_str(raw).unwrap();
        assert!(m.mandatory && m.forced);
        assert_eq!(m.minimum_version, "0.3.0");
        assert_eq!(serde_json::from_str::<Manifest>(r#"{"version":"1"}"#).unwrap().minimum_version, "");
    }

    #[test]
    fn highest_version_over_relays_wins() {
        let v = |x: &str| Ok(Some(m(x, false, "")));
        let got = |a: Vec<Result<Option<Manifest>, String>>| highest(a).map(|m| m.map(|m| m.version));
        assert_eq!(got(vec![v("0.5.23"), v("0.5.24")]), Ok(Some("0.5.24".into())));
        assert_eq!(got(vec![v("0.5.24"), Err("down".into())]), Ok(Some("0.5.24".into())));
        assert_eq!(got(vec![Ok(None), v("0.5.2")]), Ok(Some("0.5.2".into())));
        assert_eq!(got(vec![Ok(None), Err("down".into())]), Ok(None));
        assert_eq!(got(vec![Err("a".into()), Err("b".into())]), Err("a; b".into()));
    }

    #[test]
    fn manifest_platform_becomes_a_bound_candidate() {
        let raw = format!(r#"{{"version":"0.5.24","platforms":{{"{}":{{"url":"https://h/download/stable/technician/x/Tether_0.5.24_x64-setup.exe","signature":"S"}}}}}}"#, target());
        let c = serde_json::from_str::<Manifest>(&raw).unwrap().candidate().unwrap();
        assert_eq!((c.file.as_str(), c.version.as_str(), c.serial), ("Tether_0.5.24_x64-setup.exe", "0.5.24", 0));
        assert!(serde_json::from_str::<Manifest>(r#"{"version":"1"}"#).unwrap().candidate().is_none());
    }

    #[test]
    fn endpoint_layout_matches_the_contract() {
        assert_eq!(manifest_path("windows-x86_64"), "/download/stable/technician/windows-x86_64/latest.json");
        #[cfg(all(windows, target_arch = "x86_64"))]
        assert_eq!(target(), "windows-x86_64");
    }
}

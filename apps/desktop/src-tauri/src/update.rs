//! Updates (docs/updates.md, docs/update-design.md, wire format in docs/api.md).
//!
//! Source of truth is the signed release index `https://<relay>/download/<channel>/releases.json`
//! (admin-key envelope): every relay is asked (`If-None-Match`), the highest valid serial wins and
//! raises the floor (`releases-<channel>.serial`). Once a device accepted one, the unsigned
//! `latest.json` is never used again; before that (relays without an index) the highest
//! `latest.json` is. The result is cached in the config dir (`update-state.json`), so every
//! process role enforces it without network: below the policy or release minimum version, or with
//! a pending mandatory update that reached this device, sessions are refused (`blocked`).
//!
//! The rollout ramp (stable bucket per install and release, policy `update.rollout`), halt and
//! security flags come from the signed entry. An eligible update is downloaded in the background
//! and verified (size, sha256, minisign, signed trusted comment: file and version), staged in the
//! config dir, and those exact bytes go to `tauri-plugin-updater`'s installer. A forced update
//! installs only while no session runs (D7). Checks: at start (random 5-60 s), every
//! `update.check-hours` (+0-20 %), after a policy change, and after a release hint (hbbs push via
//! the mirrored option `tether-release-hint`, or the heartbeat answer).

use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashMap},
    path::PathBuf,
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

/// Legacy `latest.json` (clients up to 0.5.23 read only this).
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

/// What the last check found, cached for every process role (`update-state.json`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Found {
    pub release: Option<Candidate>,
    /// The rollout reached this device (computed at check time; legacy releases always).
    pub eligible: bool,
    /// The signed index is past its expiry (D3: warn only).
    pub stale: bool,
}

/// Per relay: ETag and body of its last 200 answer, for `latest.json` and `releases.json`.
static ETAGS: LazyLock<Mutex<HashMap<String, (String, Vec<u8>)>>> = LazyLock::new(Default::default);

static LAST: LazyLock<Mutex<Found>> = LazyLock::new(|| {
    let cached = std::fs::read(Config::path("update-state.json")).ok();
    Mutex::new(cached.and_then(|raw| serde_json::from_slice(&raw).ok()).unwrap_or_default())
});
static WAKE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());
/// Background download: `idle`, `downloading`, `staged`, `failed`.
static PHASE: Mutex<&'static str> = Mutex::new("idle");
static HINTS: Mutex<Option<release::Hints>> = Mutex::new(None);

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
    if release::valid_channel(&c) {
        c
    } else {
        "stable".into()
    }
}

fn manifest_path(target: &str) -> String {
    format!("/download/{}/technician/{target}/latest.json", channel())
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

// ----------------------------------------------------------------------------- enforcement

fn newer(c: &Candidate) -> bool {
    !c.halted && compare(&c.version, VERSION) == Some(Ordering::Greater)
}

/// Why sessions are blocked, if they are. A mandatory release blocks only devices its rollout
/// reached (the others cannot install it yet); minimum versions always block.
fn reason_for(f: &Found, policy_minimum: &str, updates_enabled: bool) -> Option<&'static str> {
    let c = f.release.as_ref();
    if below_minimum(VERSION, policy_minimum) || c.is_some_and(|c| below_minimum(VERSION, &c.minimum_version)) {
        return Some("below-minimum");
    }
    // A mandatory update can only be required when updates may be installed.
    (updates_enabled && f.eligible && c.is_some_and(|c| c.mandatory && newer(c))).then_some("mandatory")
}

fn reason() -> Option<&'static str> {
    let f = LAST.lock().unwrap().clone();
    reason_for(&f, &policy::get("update.minimum-version"), policy::on("update.enabled"))
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

/// The release this device may install now.
fn offer() -> Option<Candidate> {
    let f = LAST.lock().unwrap().clone();
    f.release.filter(|c| f.eligible && newer(c))
}

/// Heartbeat `update_state` (docs/api.md): idle, waiting-rollout, downloading, staged,
/// waiting-idle, failed, stale, halted.
pub fn phase() -> String {
    let f = LAST.lock().unwrap().clone();
    let p = *PHASE.lock().unwrap();
    match f.release.as_ref() {
        Some(c) if c.halted && compare(&c.version, VERSION) == Some(Ordering::Greater) => "halted",
        Some(c) if newer(c) && !f.eligible => "waiting-rollout",
        Some(c) if newer(c) && p == "staged" && c.forced && crate::app::busy() => "waiting-idle",
        Some(c) if newer(c) && p != "idle" => p,
        _ if f.stale => "stale",
        _ => "idle",
    }
    .into()
}

/// Heartbeat fields: `(update_state, release serial, version in flight)`.
pub fn heartbeat_fields() -> (String, u64, String) {
    let f = LAST.lock().unwrap().clone();
    let v = f.release.as_ref().filter(|c| newer(c)).map(|c| c.version.clone()).unwrap_or_default();
    (phase(), f.release.as_ref().map_or(0, |c| c.serial), v)
}

/// State for the UI (`update` event, `update_status`).
pub fn status() -> Value {
    let f = LAST.lock().unwrap().clone();
    let m = offer();
    let reason = reason();
    let enabled = policy::on("update.enabled");
    let pending = f.release.as_ref().filter(|c| newer(c) && !f.eligible);
    json!({
        "current": VERSION,
        "version": m.as_ref().map(|m| m.version.clone()),
        "notes": m.as_ref().map(|m| m.notes.clone()).unwrap_or_default(),
        "mandatory": reason.is_some(),
        "forced": enabled && m.as_ref().is_some_and(|m| m.forced || (reason.is_some() && policy::on("update.force"))),
        "blocked": reason,
        "enabled": enabled,
        "restartSeconds": policy::get("update.restart-seconds").parse::<u64>().unwrap_or(60),
        // A session runs: a forced update waits for it (D7).
        "busy": crate::app::busy(),
        "phase": phase(),
        "stale": f.stale,
        "serial": f.release.as_ref().map_or(0, |c| c.serial),
        "rollout": pending.map(|c| json!({ "version": c.version, "percent": c.rollout.percent(now()) })),
    })
}

// ----------------------------------------------------------------------------- check

/// One relay's document at `path`; 304 answers come from the remembered copy. `Ok(None)`: 404.
fn fetch_doc(host: &str, path: &str, limit: usize) -> Result<Option<Vec<u8>>, String> {
    let key = format!("{host}{path}");
    let etag = ETAGS.lock().unwrap().get(&key).map(|e| e.0.clone());
    let r = policy::api_via(&[host.to_owned()], |c, base| {
        let rq = c.get(format!("{base}{path}"));
        match &etag {
            Some(t) => rq.header("If-None-Match", t),
            None => rq,
        }
    })?;
    match r.status().as_u16() {
        304 => Ok(ETAGS.lock().unwrap().get(&key).map(|e| e.1.clone())),
        404 => Ok(None),
        200 => {
            let tag = r.headers().get("ETag").and_then(|v| v.to_str().ok()).map(str::to_owned);
            let raw = tether_trust::read_limited(r, limit).map_err(|e| format!("{path}: {e}"))?;
            if let Some(t) = tag {
                ETAGS.lock().unwrap().insert(key, (t, raw.clone()));
            }
            Ok(Some(raw))
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

fn index_cache(channel: &str) -> PathBuf {
    Config::path(format!("releases-{channel}.json"))
}

/// The release to consider: signed index first (highest serial over the relays, never below the
/// floor), else (no floor yet, D2) the highest legacy manifest.
fn find_release(relays: &[(String, String)], channel: &str, keys: &[tether_trust::VerifyingKey]) -> Result<Option<Candidate>, String> {
    let cache = index_cache(channel);
    let ipath = format!("/download/{channel}/releases.json");
    let mut docs = Vec::new();
    let mut errors = Vec::new();
    for (host, base) in relays {
        match fetch_doc(host, &ipath, release::MAX_INDEX) {
            Ok(Some(raw)) => docs.push((base.clone(), raw)),
            Ok(None) => {}
            Err(e) => errors.push(e),
        }
    }
    let key = format!("technician/{}", target());
    let pick = |base: &str, ix: &release::Index| ix.releases.get(&key).map(|e| Candidate::from_entry(e, base, ix));
    if !keys.is_empty() {
        match release::choose(&cache, &docs, keys, channel) {
            Ok(Some((base, ix))) => return Ok(pick(&base, &ix)),
            Ok(None) => {}
            Err(e) => log::warn!("release index: {e}"),
        }
        if release::floor(&cache, keys) > 0 || release::REQUIRE_SIGNED_INDEX {
            return match (release::load(&cache, keys, channel), relays.first()) {
                (Some(ix), Some((_, base))) => Ok(pick(base, &ix)),
                _ if errors.len() == relays.len() && !relays.is_empty() => Err(errors.join("; ")),
                _ => Ok(None),
            };
        }
    }
    let mpath = manifest_path(&target());
    let answers = relays
        .iter()
        .map(|(host, _)| fetch_doc(host, &mpath, 64 * 1024).and_then(|r| r.map(|b| serde_json::from_slice::<Manifest>(&b).map_err(|e| format!("invalid update manifest: {e}"))).transpose()))
        .collect();
    Ok(highest(answers)?.and_then(|m| m.candidate()))
}

/// Ask every relay (blocking). `Err("offline: …")` when none can be reached.
pub fn check() -> Result<Value, String> {
    let found = match find_release(&policy::relay_bases(), &channel(), &policy::admin_keys())? {
        Some(c) => {
            let t = now();
            let bucket = release::bucket(&policy::install_uuid(), "technician", &c.version);
            Found { eligible: c.eligible(bucket, t, &policy::get("update.rollout")), stale: c.stale(t), release: Some(c) }
        }
        None => Found::default(),
    };
    let raw = serde_json::to_vec(&found).unwrap_or_default();
    if let Err(e) = policy::write_atomic(&Config::path("update-state.json"), &raw) {
        log::warn!("cannot cache update state: {e}");
    }
    *LAST.lock().unwrap() = found;
    if let Some(c) = offer() {
        stage_in_background(c);
    }
    let s = status();
    crate::app::emit("update", s.clone());
    Ok(s)
}

// ----------------------------------------------------------------------------- download / install

fn gunzip(data: &[u8], max: u64) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut std::io::Read::take(flate2::read::GzDecoder::new(data), max + 1), &mut out).map_err(|e| format!("bad gzip copy: {e}"))?;
    if out.len() as u64 > max {
        return Err("the gzip copy unpacks larger than announced".into());
    }
    Ok(out)
}

/// Every source in turn (the gzip copy first), then the verified bytes.
fn download(c: &Candidate, key: &str) -> Result<Vec<u8>, String> {
    let mut last = String::from("no download URL");
    for (url, packed) in c.sources() {
        match download_one(&url, c, packed).and_then(|b| c.accept(b, packed, key, gunzip)) {
            Ok(b) => return Ok(b),
            Err(e) => {
                log::warn!("update download {url}: {e}");
                last = e;
            }
        }
    }
    Err(last)
}

fn download_one(url: &str, c: &Candidate, packed: bool) -> Result<Vec<u8>, String> {
    let mut r = policy::fetch(url, Duration::from_secs(1800))?;
    let limit = c.limit(packed);
    let total = if c.signed() { limit } else { r.content_length().unwrap_or(0) };
    let mut bytes = Vec::with_capacity(total.min(release::MAX_ARTIFACT) as usize);
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = std::io::Read::read(&mut r, &mut buf).map_err(|e| format!("download failed: {e}"))?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..n]);
        if bytes.len() as u64 > limit {
            return Err("the download is larger than announced".into());
        }
        crate::app::emit("update-progress", json!({ "done": bytes.len(), "total": total }));
    }
    Ok(bytes)
}

/// The verified download of `c`, waiting for its install (config dir).
fn staged_path(c: &Candidate) -> Option<PathBuf> {
    release::valid_version(&c.version).then(|| Config::path(format!("update-{}.staged", c.version)))
}

fn staged(c: &Candidate, key: &str) -> Option<Vec<u8>> {
    // Read back and verified again: the config dir is not trusted more than the network.
    std::fs::read(staged_path(c)?).ok().filter(|b| c.verify(b, key).is_ok())
}

/// Download and verify in the background as soon as the device may install it; one at a time.
fn stage_in_background(c: Candidate) {
    static BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let (Some(key), Some(path)) = (UPDATER_KEY.filter(|k| !k.is_empty()), staged_path(&c)) else { return };
    if !policy::on("update.enabled") || BUSY.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        if staged(&c, key).is_some() {
            *PHASE.lock().unwrap() = "staged";
        } else {
            *PHASE.lock().unwrap() = "downloading";
            let r = download(&c, key).and_then(|b| policy::write_atomic(&path, &b).map_err(|e| e.to_string()));
            *PHASE.lock().unwrap() = match r {
                Ok(()) => {
                    log::info!("update {} downloaded and verified", c.version);
                    "staged"
                }
                Err(e) => {
                    log::warn!("update download: {e}");
                    "failed"
                }
            };
        }
        BUSY.store(false, std::sync::atomic::Ordering::SeqCst);
        crate::app::emit("update", status());
    });
}

/// Install the update and restart. `auto` (the forced countdown) is refused while a session runs
/// (D7); a click by the user is not. Right before installing the relays are asked again, so a
/// halted rollout stops here. Progress: `update-progress` events.
pub async fn install(app: AppHandle, auto: bool) -> Result<(), String> {
    if !policy::on("update.enabled") {
        return Err("locked:update.enabled: Your organisation installs updates for you.".into());
    }
    if auto && crate::app::busy() {
        return Err("busy: The update installs when the session ends.".into());
    }
    let key = UPDATER_KEY.filter(|k| !k.is_empty()).ok_or("Updates are not configured in this build.")?;
    let c = tauri::async_runtime::spawn_blocking(move || {
        if let Err(e) = check() {
            log::warn!("update re-check: {e}");
        }
        offer().ok_or_else(|| "No update is available (the release may have been halted).".to_owned())
    })
    .await
    .map_err(|e| e.to_string())??;
    let bytes = tauri::async_runtime::spawn_blocking({
        let c = c.clone();
        move || staged(&c, key).map_or_else(|| download(&c, key), Ok)
    })
    .await
    .map_err(|e| e.to_string())??;
    if auto && crate::app::busy() {
        return Err("busy: The update installs when the session ends.".into());
    }
    install_bytes(&app, key, &c, &bytes)?;
    if let Some(p) = staged_path(&c) {
        let _ = std::fs::remove_file(p);
    }
    app.restart();
}

/// Hand verified bytes to the plugin's installer (NSIS on Windows, the .app on macOS). The plugin
/// checks the signature and the signed version once more.
fn install_bytes(app: &AppHandle, key: &str, c: &Candidate, bytes: &[u8]) -> Result<(), String> {
    // `build` insists on endpoints; `restore_update` never fetches them.
    let endpoints = policy::relay_bases().iter().filter_map(|(_, base)| format!("{base}{}", manifest_path("{{target}}")).parse().ok()).collect();
    let mut b = app.updater_builder().pubkey(key).target(target()).endpoints(endpoints).map_err(|e| e.to_string())?;
    if let Some(p) = policy::proxy_url().and_then(|p| p.parse().ok()) {
        b = b.proxy(p);
    }
    let url = c.urls.first().or(c.packed.as_ref().and_then(|p| p.urls.first())).ok_or("no download URL")?;
    let raw = json!({ "version": c.version, "notes": c.notes, "platforms": { target(): { "url": url, "signature": c.signature } } });
    let update = b.build().map_err(|e| e.to_string())?.restore_update(raw).map_err(|e| e.to_string())?.ok_or("No update is available.")?;
    update.install(bytes).map_err(|e| e.to_string())
}

// ----------------------------------------------------------------------------- triggers

/// Ask the checker to run now (policy changed, release hint).
pub fn on_policy_change() {
    *WAKE.0.lock().unwrap() = true;
    WAKE.1.notify_all();
}

/// A relay's release generation changed (hbbs push, heartbeat answer): check after a random
/// 0..spread s, at most once a minute per relay. The value is unsigned: it only triggers a fetch.
pub fn hint(host: &str, generation: u64, spread: u32) {
    let fire = HINTS.lock().unwrap().get_or_insert_with(Default::default).update(host, generation, now());
    if fire {
        log::info!("release hint from {host}: generation {generation}");
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(release::uniform_secs(spread.min(release::MAX_SPREAD_SECS) as u64)));
            on_policy_change();
        });
    }
}

/// Main UI: check on start (after a random 5-60 s), every `update.check-hours` (+0-20 % jitter,
/// so devices never poll in step), after each policy change and each release hint.
pub fn start() {
    // hbbs hints arrive in the `--server` process (rendezvous_mediator.rs) and reach this one
    // through the option mirror (`start_status_sync`).
    std::thread::spawn(|| loop {
        let raw = librustdesk::ui::api::get_option(release::HINT_OPTION.to_owned());
        for (host, (g, spread)) in release::parse_hints(&raw) {
            hint(&host, g, spread);
        }
        std::thread::sleep(Duration::from_secs(2));
    });
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

    fn found(version: &str, mandatory: bool, minimum: &str, eligible: bool) -> Found {
        Found { release: Some(Candidate { version: version.into(), mandatory, minimum_version: minimum.into(), ..Default::default() }), eligible, stale: false }
    }

    #[test]
    fn blocking_rules() {
        let none = Found::default();
        assert_eq!(reason_for(&none, "", true), None);
        assert_eq!(reason_for(&none, "999.0.0", true), Some("below-minimum"));
        // Policy minimum still blocks with updates disabled.
        assert_eq!(reason_for(&none, "999.0.0", false), Some("below-minimum"));
        assert_eq!(reason_for(&found("999.0.0", false, "999.0.0", true), "", true), Some("below-minimum"));
        assert_eq!(reason_for(&found("999.0.0", true, "", true), "", true), Some("mandatory"));
        assert_eq!(reason_for(&found("999.0.0", true, "", true), "", false), None);
        assert_eq!(reason_for(&found("999.0.0", false, "", true), "", true), None);
        // A mandatory release that is not newer does not block.
        assert_eq!(reason_for(&found("0.0.1", true, "", true), "", true), None);
        // Not reached by the rollout yet, or halted: a mandatory release does not block.
        assert_eq!(reason_for(&found("999.0.0", true, "", false), "", true), None);
        let mut halted = found("999.0.0", true, "", true);
        halted.release.as_mut().unwrap().halted = true;
        assert_eq!(reason_for(&halted, "", true), None);
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

    /// The cache written by 0.5.23 (a bare Manifest) does not parse as `Found`: start empty.
    #[test]
    fn old_cache_is_ignored() {
        let old = br#"{"version":"0.5.24","notes":"","mandatory":true,"minimumVersion":"","forced":false}"#;
        let f: Found = serde_json::from_slice(old).unwrap_or_default();
        assert!(f.release.is_none() && !f.eligible);
        let back: Found = serde_json::from_slice(&serde_json::to_vec(&found("1.0.0", true, "", true)).unwrap()).unwrap();
        assert_eq!(back, found("1.0.0", true, "", true));
    }

    #[test]
    fn endpoint_layout_matches_the_contract() {
        assert_eq!(manifest_path("windows-x86_64"), "/download/stable/technician/windows-x86_64/latest.json");
        #[cfg(all(windows, target_arch = "x86_64"))]
        assert_eq!(target(), "windows-x86_64");
    }
}

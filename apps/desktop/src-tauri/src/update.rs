//! Updates (docs/api.md "Download layout and update manifests").
//!
//! The manifest `https://<relay>/download/<channel>/technician/<target>/latest.json` is read here
//! (custom fields `mandatory`, `minimumVersion`, `forced`) and cached in the config dir, so every
//! process role can enforce it without network: below the policy or manifest minimum version, or
//! with a pending mandatory update, sessions are refused (`blocked`). Download, signature check
//! (minisign, `TETHER_UPDATER_PUB_KEY`) and install are done by `tauri-plugin-updater`.

use std::{
    cmp::Ordering,
    sync::{Condvar, LazyLock, Mutex},
    time::Duration,
};

use hbb_common::{config::Config, log};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::AppHandle;
use tauri_plugin_updater::UpdaterExt;
use tether_trust::version::{below_minimum, compare};

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
}

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

/// Fetch the manifest (blocking). `Err("offline: …")` when the server cannot be reached.
pub fn check() -> Result<Value, String> {
    let path = manifest_path(&target());
    let r = policy::api(|c, base| c.get(format!("{base}{path}")))?;
    let m = match r.status().as_u16() {
        404 => None,
        200 => Some(r.json::<Manifest>().map_err(|e| format!("invalid update manifest: {e}"))?),
        s => return Err(format!("update server answered {s}")),
    };
    let raw = serde_json::to_vec(&m).unwrap_or_default();
    if let Err(e) = policy::write_atomic(&Config::path("update-state.json"), &raw) {
        log::warn!("cannot cache update state: {e}");
    }
    *LAST.lock().unwrap() = m;
    let s = status();
    crate::app::emit("update", s.clone());
    Ok(s)
}

/// Download, verify and install the update, then restart. Progress: `update-progress` events.
pub async fn install(app: AppHandle) -> Result<(), String> {
    if !policy::on("update.enabled") {
        return Err("locked:update.enabled: Your organisation installs updates for you.".into());
    }
    let key = UPDATER_KEY.filter(|k| !k.is_empty()).ok_or("Updates are not configured in this build.")?;
    // Every server serves the same signed releases; the updater tries them in order.
    let mut endpoints = Vec::new();
    for host in policy::api_hosts() {
        endpoints.push(format!("https://{host}{}", manifest_path("{{target}}")).parse().map_err(|e| format!("{e}"))?);
    }
    let mut b = app
        .updater_builder()
        .pubkey(key)
        .target(target())
        .endpoints(endpoints)
        .map_err(|e| e.to_string())?;
    if let Some(p) = policy::proxy_url().and_then(|p| p.parse().ok()) {
        b = b.proxy(p);
    }
    let update = b
        .build()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| format!("offline: {e}"))?
        .ok_or("No update is available.")?;
    let mut done = 0u64;
    update
        .download_and_install(
            |chunk, total| {
                done += chunk as u64;
                crate::app::emit("update-progress", json!({ "done": done, "total": total }));
            },
            || {},
        )
        .await
        .map_err(|e| e.to_string())?;
    app.restart();
}

/// Ask the checker to run now (policy changed).
pub fn on_policy_change() {
    *WAKE.0.lock().unwrap() = true;
    WAKE.1.notify_all();
}

/// Main UI: check on start, every `update.check-hours`, and after each policy change.
pub fn start() {
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(5));
        loop {
            if let Err(e) = check() {
                log::info!("update check: {e}");
            }
            let hours = policy::get("update.check-hours").parse::<u64>().unwrap_or(6).max(1);
            let woken = WAKE.0.lock().unwrap();
            let (mut woken, _) = WAKE.1.wait_timeout_while(woken, Duration::from_secs(hours * 3600), |w| !*w).unwrap();
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
    fn endpoint_layout_matches_the_contract() {
        assert_eq!(manifest_path("windows-x86_64"), "/download/stable/technician/windows-x86_64/latest.json");
        #[cfg(all(windows, target_arch = "x86_64"))]
        assert_eq!(target(), "windows-x86_64");
    }
}

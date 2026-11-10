//! IPC commands for the single window. Everything the UI may do is checked here, not in the page.

use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

use librustdesk::ui::{api, cm};
use serde::Serialize;
use serde_json::Value;

use crate::{policy, sink, update};

type R<T> = Result<T, String>;

/// Core permission name → policy feature.
const PERMS: &[(&str, &str, &str)] = &[
    ("keyboard", "permission.control", "Control mouse and keyboard"),
    ("clipboard", "permission.clipboard", "Use the clipboard"),
    ("file", "permission.file-transfer", "Transfer files"),
    ("audio", "permission.audio", "Hear this computer's sound"),
    ("restart", "permission.restart", "Restart this computer"),
    ("block_input", "permission.block-input", "Block my mouse and keyboard"),
];

#[derive(Serialize)]
struct Perm {
    name: &'static str,
    label: &'static str,
    /// Allowed by policy at all (the client payload says whether it is currently granted).
    allowed: bool,
    locked: bool,
}

#[derive(Serialize)]
struct Branding {
    title: String,
    accent: String,
    welcome: String,
    elevate: bool,
    approve_mode: String,
    consent_text: String,
    diagnostics: bool,
    /// "Send to support" in the Help view (policy `diagnostics.upload`).
    diagnostics_upload: bool,
    /// Policy `connect.chat`.
    chat: bool,
    perms: Vec<Perm>,
    /// Custom app name (docs/api.md "Custom apps"), None for plain TetherQS.
    custom_app: Option<String>,
    /// `data:` URL of the custom app's logo, empty = none.
    logo: String,
}

#[derive(Serialize)]
pub struct State {
    id: String,
    password: String,
    /// -1 offline, 0 connecting, 1 ready
    status: i32,
    version: &'static str,
    elevated: bool,
    branding: Branding,
    update: update::State,
    blocked: bool,
    clients: Vec<Value>,
    /// macOS only: what the Mac allows TetherQS (None elsewhere).
    mac: Option<MacPerms>,
}

#[derive(Serialize)]
pub struct MacPerms {
    /// Screen Recording: without it the technician gets no picture.
    screen: bool,
    /// Accessibility: without it mouse and keyboard are ignored.
    accessibility: bool,
}

fn mac_perms() -> Option<MacPerms> {
    cfg!(target_os = "macos").then(|| MacPerms { screen: api::is_can_screen_recording(false), accessibility: api::is_process_trusted(false) })
}

/// Clear a stale grant, let macOS ask, open the matching System Settings page.
#[tauri::command]
fn mac_fix(app: tauri::AppHandle, which: String) -> R<()> {
    api::mac_fix(&app.config().identifier, &which)
}

fn branding() -> Branding {
    let e = policy::current();
    let accent = e.get("customer.accent-color");
    let valid = accent.len() == 7 && accent.starts_with('#') && accent[1..].chars().all(|c| c.is_ascii_hexdigit());
    Branding {
        title: e.get("customer.title").to_owned(),
        accent: if valid { accent.to_owned() } else { "#0071e3".into() },
        welcome: e.get("customer.welcome").to_owned(),
        // UAC elevation (portable service) exists on Windows only.
        elevate: cfg!(windows) && e.on("customer.elevate"),
        approve_mode: e.get("security.approve-mode").to_owned(),
        consent_text: e.get("recording.consent-text").to_owned(),
        diagnostics: e.on("diagnostics.page"),
        diagnostics_upload: e.on("diagnostics.upload"),
        chat: e.on("connect.chat"),
        perms: PERMS
            .iter()
            .filter(|(_, f, _)| !e.hidden.contains(*f))
            .map(|(name, f, label)| Perm { name, label, allowed: e.on(f), locked: e.is_locked(f) })
            .collect(),
        custom_app: crate::custom::profile().map(|p| p.name.clone()),
        logo: crate::custom::logo_url(),
    }
}

/// This process is elevated, or the core's portable service (elevated helper) is running.
fn elevated() -> bool {
    static CACHE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    let mut c = CACHE.lock().unwrap();
    if let Some((t, v)) = *c {
        if t.elapsed() < Duration::from_secs(3) {
            return v;
        }
    }
    let v = compute_elevated();
    *c = Some((Instant::now(), v));
    v
}

#[cfg(windows)]
fn compute_elevated() -> bool {
    use librustdesk::platform::is_elevated;
    if is_elevated(None).unwrap_or(false) {
        return true;
    }
    let Some(name) = std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase())) else {
        return false;
    };
    let mut sys = hbb_common::sysinfo::System::new();
    sys.refresh_processes();
    let me = std::process::id() as usize;
    // The helper runs as SYSTEM: its token cannot even be opened from here, which also means elevated.
    sys.processes().iter().any(|(pid, p)| {
        usize::from(*pid) != me && p.name().to_lowercase() == name && is_elevated(Some(usize::from(*pid) as u32)).unwrap_or(true)
    })
}

#[cfg(not(windows))]
fn compute_elevated() -> bool {
    false
}

#[tauri::command]
fn state() -> State {
    let s = api::get_connect_status();
    State {
        id: if s.id.is_empty() { api::get_id() } else { s.id },
        password: api::temporary_password(),
        status: s.status_num,
        version: update::VERSION,
        elevated: elevated(),
        branding: branding(),
        update: update::state(),
        blocked: update::blocked(),
        clients: sink::clients(),
        mac: mac_perms(),
    }
}

#[tauri::command]
fn refresh_password() {
    api::update_temporary_password();
}

#[tauri::command]
fn cm_authorize(id: i32) -> R<()> {
    if update::blocked() {
        return Err("Update TetherQS before accepting sessions.".into());
    }
    cm::authorize(id);
    sink::authorized(id);
    Ok(())
}

#[tauri::command]
fn cm_close(id: i32) {
    cm::close(id);
}

/// Reply to the technician's chat (shown in their session window's chat panel).
#[tauri::command]
fn cm_chat(id: i32, text: String) -> R<()> {
    if !policy::current().on("connect.chat") {
        return Err("Chat is turned off by your administrator.".into());
    }
    if text.len() > 4096 {
        return Err("Message too long.".into());
    }
    cm::send_chat(id, text);
    Ok(())
}

#[tauri::command]
fn cm_permission(id: i32, name: String, enabled: bool) -> R<()> {
    let (_, feature, _) = PERMS.iter().find(|(n, _, _)| *n == name).ok_or("unknown permission")?;
    if enabled && denied(&policy::current(), feature) {
        return Err("Your administrator does not allow this.".into());
    }
    cm::switch_permission(id, name.clone(), enabled);
    sink::set_field(id, &name, enabled);
    Ok(())
}

/// Policy keeps this permission off: locked (or hidden) and off.
fn denied(e: &tether_trust::policy::Effective, feature: &str) -> bool {
    (e.is_locked(feature) || e.hidden.contains(feature)) && !e.on(feature)
}

/// `(connection, permission)`: what the connections hold although policy `e` denies it.
pub(crate) fn to_revoke(e: &tether_trust::policy::Effective, clients: &[Value]) -> Vec<(i32, &'static str)> {
    let off: Vec<&'static str> = PERMS.iter().filter(|(_, f, _)| denied(e, f)).map(|(n, _, _)| *n).collect();
    let mut v = Vec::new();
    for c in clients {
        let Some(id) = c["id"].as_i64() else { continue };
        v.extend(off.iter().filter(|n| c[**n] == true).map(|n| (id as i32, *n)));
    }
    v
}

/// A policy applied during a session takes what it denies from the running connections too (the
/// switch in the window only greys out).
pub fn revoke_denied(e: &tether_trust::policy::Effective) {
    revoke(e, &sink::clients());
}

/// Take from `clients` (as the core announced them) what policy `e` denies.
pub fn revoke(e: &tether_trust::policy::Effective, clients: &[Value]) {
    for (id, name) in to_revoke(e, clients) {
        hbb_common::log::info!("policy: {name} switched off for connection {id}");
        cm::switch_permission(id, name.to_owned(), false);
        sink::set_field(id, name, false);
        // A running file transfer does not check the switch: end it.
        if name == "file" && clients.iter().any(|c| c["id"] == id && c["is_file_transfer"] == true) {
            cm::close(id);
        }
    }
}

/// Answer to `record_consent_request`: Allow grants this connection's recording permission,
/// Deny keeps it off (the viewer reports "declined").
#[tauri::command]
#[allow(non_snake_case)]
fn cm_record_consent(connId: i32, allow: bool) -> R<()> {
    hbb_common::log::info!("record consent for {connId}: {allow}");
    librustdesk::ui::cm::record_consent(connId, allow);
    Ok(())
}

/// Admin rights for UAC prompts. During a session: the core's portable service elevates in place
/// (one UAC prompt, the session continues). Otherwise: restart elevated, keeping ID and password.
#[tauri::command]
fn elevate() -> R<()> {
    if !policy::current().on("customer.elevate") {
        return Err("Your administrator does not allow this.".into());
    }
    if elevated() {
        return Ok(());
    }
    let active = crate::sink::clients().into_iter().find(|c| c["authorized"] == true).and_then(|c| c["id"].as_i64());
    if let Some(id) = active {
        if librustdesk::ui::cm::can_elevate() {
            librustdesk::ui::cm::elevate_portable(id as i32);
            return Ok(());
        }
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    crate::relaunch(&exe, true)
}

/// "Check for updates": the result text, or why the check failed.
#[tauri::command]
async fn update_check() -> R<String> {
    tauri::async_runtime::spawn_blocking(update::check).await.map_err(|e| e.to_string())?
}

#[tauri::command]
fn update_install() -> R<()> {
    // The update restarts TetherQS, which would end the session.
    if sink::has_session() {
        return Err("The update restarts TetherQS: end the session first.".into());
    }
    std::thread::spawn(|| {
        if let Err(e) = update::install() {
            hbb_common::log::error!("update failed: {e}");
        }
    });
    Ok(())
}

/// Everything known locally: shown at once, also without a network.
#[derive(Serialize)]
pub(crate) struct Diagnostics {
    version: &'static str,
    id: String,
    /// Every server this app registers with (compiled-in, then policy `servers.list`).
    servers: Vec<String>,
    proxy: String,
    elevated: bool,
    policy_serial: u64,
    policy_group: Option<String>,
}

#[tauri::command]
pub(crate) fn diagnostics() -> Diagnostics {
    let e = policy::current();
    Diagnostics {
        version: update::VERSION,
        id: api::get_id(),
        servers: policy::base_urls(),
        proxy: hbb_common::config::DEFAULT_SETTINGS.read().unwrap().get("proxy-url").map(|p| without_credentials(p)).unwrap_or_else(|| "none (direct)".into()),
        elevated: elevated(),
        policy_serial: e.serial,
        policy_group: e.group,
    }
}

/// A proxy URL as shown (Help view, diagnostics bundle): no user name or password.
pub(crate) fn without_credentials(url: &str) -> String {
    // Not strippable (`user:pass@host` parses as scheme `user`): never shown as it is.
    let strip = |mut u: reqwest::Url| (u.set_password(None).is_ok() && u.set_username("").is_ok()).then(|| u.to_string());
    reqwest::Url::parse(url).ok().and_then(strip).unwrap_or_else(|| "(not a URL)".into())
}

/// Help view › Send to support: the diagnostics bundle for a ticket code.
#[tauri::command]
async fn diag_upload(ticket: String) -> R<()> {
    tauri::async_runtime::spawn_blocking(move || crate::diag::upload(&ticket)).await.map_err(|e| e.to_string())?
}

#[derive(Serialize)]
struct Probe {
    reachable: bool,
    ms: Option<u128>,
    error: Option<String>,
}

/// reqwest's message alone is "error sending request for url (…)": add the causes (refused, DNS, TLS, timeout).
pub(crate) fn with_causes(e: &dyn std::error::Error) -> String {
    let mut s = e.to_string();
    let mut src = e.source();
    while let Some(c) = src {
        s.push_str(": ");
        s.push_str(&c.to_string());
        src = c.source();
    }
    s
}

/// The network check, separate from `diagnostics` so a dead network never hides the local facts.
#[tauri::command]
async fn diagnostics_probe(server: String) -> Probe {
    const LIMIT: Duration = Duration::from_secs(5);
    let t = Instant::now();
    let job = tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        if !policy::base_urls().contains(&server) {
            return Err("not a server of this app".into());
        }
        policy::http()?.get(format!("{server}/api/policy")).timeout(LIMIT).send().map(|_| ()).map_err(|e| with_causes(&e))
    });
    // Proxy detection (WPAD/PAC) can hang before the request starts: bound the whole check.
    let r = match hbb_common::tokio::time::timeout(LIMIT + Duration::from_secs(1), job).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err(format!("no answer within {} s", LIMIT.as_secs())),
    };
    Probe { reachable: r.is_ok(), ms: r.is_ok().then(|| t.elapsed().as_millis()), error: r.err() }
}

pub fn handler() -> impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        state,
        refresh_password,
        cm_authorize,
        cm_close,
        cm_chat,
        cm_permission,
        cm_record_consent,
        elevate,
        update_check,
        update_install,
        diagnostics,
        diagnostics_probe,
        diag_upload,
        mac_fix,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tether_trust::policy::{resolve, Bundle, PROFILE_CUSTOMER};

    #[test]
    fn proxy_url_without_credentials() {
        assert_eq!(without_credentials("http://alice:s3cret@proxy.corp:8080"), "http://proxy.corp:8080/");
        assert_eq!(without_credentials("alice:s3cret@proxy"), "(not a URL)");
    }

    #[test]
    fn a_policy_lock_takes_the_permission_from_running_connections() {
        let b = Bundle::from_json(br#"{"serial":1,"issued_at":0,"profiles":{"customer":{"permission.clipboard":{"state":"off"},"permission.file-transfer":{"state":"locked"}}}}"#).unwrap();
        let e = resolve(Some(&b), PROFILE_CUSTOMER, "dev", None);
        let clients = [
            json!({ "id": 1, "keyboard": true, "clipboard": true, "file": true, "audio": true }),
            json!({ "id": 2, "keyboard": true, "clipboard": false }),
        ];
        // Only what is locked off and held: not the unlocked audio, not the locked-on file.
        assert_eq!(to_revoke(&e, &clients), vec![(1, "clipboard")]);
        assert!(to_revoke(&resolve(None, PROFILE_CUSTOMER, "dev", None), &clients).is_empty());
    }
}

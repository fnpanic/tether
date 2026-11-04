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

/// Let macOS ask, then open the matching System Settings page (the switch may be on but stale
/// after an update: the user removes and re-adds the app there).
#[tauri::command]
fn mac_fix(which: String) -> R<()> {
    let pane = match which.as_str() {
        "screen" => {
            api::is_can_screen_recording(true);
            "Privacy_ScreenCapture"
        }
        "accessibility" => {
            api::is_process_trusted(true);
            "Privacy_Accessibility"
        }
        _ => return Err("unknown permission".into()),
    };
    if cfg!(target_os = "macos") {
        std::process::Command::new("open")
            .arg(format!("x-apple.systempreferences:com.apple.preference.security?{pane}"))
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
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
    sink::set_field(id, "authorized", true);
    Ok(())
}

#[tauri::command]
fn cm_close(id: i32) {
    cm::close(id);
}

#[tauri::command]
fn cm_permission(id: i32, name: String, enabled: bool) -> R<()> {
    let (_, feature, _) = PERMS.iter().find(|(n, _, _)| *n == name).ok_or("unknown permission")?;
    let e = policy::current();
    if enabled && (e.is_locked(feature) || e.hidden.contains(*feature)) && !e.on(feature) {
        return Err("Your administrator does not allow this.".into());
    }
    cm::switch_permission(id, name.clone(), enabled);
    sink::set_field(id, &name, enabled);
    Ok(())
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

#[tauri::command]
fn update_install() {
    std::thread::spawn(|| {
        if let Err(e) = update::install() {
            hbb_common::log::error!("update failed: {e}");
        }
    });
}

/// Everything known locally: shown at once, also without a network.
#[derive(Serialize)]
struct Diagnostics {
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
fn diagnostics() -> Diagnostics {
    let e = policy::current();
    Diagnostics {
        version: update::VERSION,
        id: api::get_id(),
        servers: policy::base_urls(),
        proxy: hbb_common::config::DEFAULT_SETTINGS.read().unwrap().get("proxy-url").cloned().unwrap_or_else(|| "none (direct)".into()),
        elevated: elevated(),
        policy_serial: e.serial,
        policy_group: e.group,
    }
}

#[derive(Serialize)]
struct Probe {
    reachable: bool,
    ms: Option<u128>,
    error: Option<String>,
}

/// reqwest's message alone is "error sending request for url (…)": add the causes (refused, DNS, TLS, timeout).
fn with_causes(e: &dyn std::error::Error) -> String {
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
        cm_permission,
        cm_record_consent,
        elevate,
        update_install,
        diagnostics,
        diagnostics_probe,
        mac_fix,
    ]
}

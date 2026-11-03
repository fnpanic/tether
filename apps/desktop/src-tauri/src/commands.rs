//! IPC commands exposed to the webview. Thin wrappers over `librustdesk::ui`.

use librustdesk::ui::{self as core, api, cm};
use serde::Serialize;
use serde_json::Value;
use tauri::{
    ipc::{Channel, InvokeResponseBody},
    AppHandle,
};

use crate::{app, fluxrt, security, sink};

type R<T> = Result<T, String>;

fn with_session<T>(key: &str, f: impl FnOnce(&core::TauriSession) -> T) -> R<T> {
    core::session(key)
        .map(|s| f(&s))
        .ok_or_else(|| "session not found".to_owned())
}

// ----------------------------------------------------------------------------- main window

#[derive(Serialize)]
pub struct Status {
    id: String,
    password: String,
    /// -1 not ready, 0 connecting, 1 ready
    status: i32,
    key_confirmed: bool,
    installed: bool,
    version: String,
}

#[tauri::command]
fn status() -> Status {
    let s = api::get_connect_status();
    Status {
        id: if s.id.is_empty() { api::get_id() } else { s.id },
        password: api::temporary_password(),
        status: s.status_num,
        key_confirmed: s.key_confirmed,
        installed: api::is_installed(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

#[tauri::command]
fn refresh_password() {
    api::update_temporary_password();
}

#[tauri::command]
fn set_permanent_password(password: String) -> R<()> {
    crate::policy::require("security.permanent-password", "Your organisation does not allow a permanent password.")?;
    security::check_password_strength(&password)?;
    if api::set_permanent_password(password) {
        Ok(())
    } else {
        Err("The password could not be saved.".to_owned())
    }
}

#[tauri::command]
fn get_option(key: String) -> R<String> {
    security::check_option(&key)?;
    Ok(api::get_option(key))
}

#[tauri::command]
fn set_option(key: String, value: String) -> R<()> {
    security::check_option(&key)?;
    security::check_option_write(&key)?;
    if value.len() > 512 {
        return Err("value too long".to_owned());
    }
    api::set_option(key, value);
    Ok(())
}

// ----------------------------------------------------------------------------- 2FA (TOTP)

#[tauri::command]
fn tfa_enabled() -> bool {
    api::has_valid_2fa()
}

/// Starts enrolment; returns the `otpauth://` URI to show as a QR code.
#[tauri::command]
fn tfa_begin() -> String {
    api::generate2fa()
}

#[tauri::command]
fn tfa_confirm(code: String) -> R<()> {
    if code.len() != 6 || !code.chars().all(|c| c.is_ascii_digit()) {
        return Err("Enter the 6-digit code.".to_owned());
    }
    if api::verify2fa(code) {
        Ok(())
    } else {
        Err("That code is not valid. Check the time on your phone and try again.".to_owned())
    }
}

#[tauri::command]
fn tfa_disable() {
    api::disable_2fa();
}

#[tauri::command]
fn recent_peers() -> Vec<Value> {
    api::recent_peers(24)
}

#[tauri::command]
fn remove_peer(id: String) -> R<()> {
    security::check_peer_id(&id)?;
    api::remove_peer(&id);
    Ok(())
}

#[tauri::command]
fn install_service() -> R<()> {
    crate::policy::require("security.unattended", "Your organisation does not allow unattended access.")?;
    std::thread::spawn(api::install_service);
    Ok(())
}

/// Async: creating a window from a synchronous command deadlocks on Windows (WebView2).
#[tauri::command]
async fn open_session(app: AppHandle, peer: String, conn: Option<String>) -> R<String> {
    let peer = peer.trim().replace(' ', "");
    security::check_peer_id(&peer)?;
    crate::policy::check_outgoing()?;
    let conn = match conn.as_deref() {
        None | Some("default") => "default",
        Some("file-transfer") => "file-transfer",
        Some(_) => return Err("unsupported connection type".to_owned()),
    };
    app::open_session(&app, &peer, conn).map_err(|e| e.to_string())
}

// ----------------------------------------------------------------------------- session

#[tauri::command]
fn session_start(
    key: String,
    peer: String,
    password: Option<String>,
    conn: Option<String>,
    force_relay: Option<bool>,
    webcodecs: Vec<String>,
    video: Channel<InvokeResponseBody>,
) -> R<()> {
    security::check_peer_id(&peer)?;
    if !key.chars().all(|c| c.is_ascii_hexdigit()) || key.len() != 32 {
        return Err("invalid session key".to_owned());
    }
    if password.as_ref().is_some_and(|p| p.len() > 128) {
        return Err("password too long".to_owned());
    }
    let mut password = password;
    if let Some(s) = sink::get() {
        s.attach_video(&key, video);
        if password.is_none() {
            password = s.take_pending_password(&key);
        }
    }
    core::session_start(
        &key,
        &peer,
        password.as_deref().unwrap_or(""),
        conn.as_deref().unwrap_or("default"),
        force_relay.unwrap_or(false),
    );
    core::session_set_webcodecs(&key, &webcodecs);
    Ok(())
}

#[tauri::command]
fn session_close(key: String) {
    if let Some(s) = sink::get() {
        s.detach_video(&key);
    }
    core::session_close(&key);
}

#[tauri::command]
fn session_reconnect(key: String, force_relay: bool) -> R<()> {
    with_session(&key, |s| s.reconnect(force_relay))
}

#[tauri::command]
fn session_login(key: String, password: String, remember: bool) -> R<()> {
    with_session(&key, |s| s.login(String::new(), String::new(), password, remember))
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn session_mouse(
    key: String,
    mask: i32,
    x: i32,
    y: i32,
    alt: bool,
    ctrl: bool,
    shift: bool,
    command: bool,
) -> R<()> {
    with_session(&key, |s| s.send_mouse(mask, x, y, alt, ctrl, shift, command))
}

/// `code` is a DOM `KeyboardEvent.code` (layout independent physical key).
#[tauri::command]
fn session_key(
    key: String,
    code: String,
    down: bool,
    lock_modes: i32,
    character: Option<String>,
    mode: Option<String>,
) -> R<()> {
    let usb_hid = rdev::usb_hid_keycode_from_key(rdev::chrome_key_from_code(&code)).unwrap_or(0);
    if usb_hid == 0 {
        return Ok(());
    }
    with_session(&key, |s| {
        s.handle_flutter_key_event(
            hbb_common::SessionID::nil(),
            mode.as_deref().unwrap_or("map"),
            character.as_deref().unwrap_or(""),
            usb_hid as i32,
            lock_modes,
            down,
        )
    })
}

#[tauri::command]
fn session_type_text(key: String, text: String) -> R<()> {
    with_session(&key, |s| s.input_string(&text))
}

#[tauri::command]
fn session_ctrl_alt_del(key: String) -> R<()> {
    with_session(&key, |s| s.ctrl_alt_del())
}

#[tauri::command]
fn session_lock_screen(key: String) -> R<()> {
    with_session(&key, |s| s.lock_screen())
}

#[tauri::command]
fn session_switch_display(key: String, display: i32) -> R<()> {
    with_session(&key, |s| s.switch_display(display))
}

#[tauri::command]
fn session_refresh(key: String, display: i32) -> R<()> {
    with_session(&key, |s| s.refresh_video(display))
}

/// `best` | `balanced` | `low` (speed)
#[tauri::command]
fn session_quality(key: String, quality: String) -> R<()> {
    with_session(&key, |s| {
        s.save_image_quality(quality);
        // The quality message resets the host's fps ceiling; restore FluxRT's.
        s.set_custom_fps(fluxrt::session_fps());
    })
}

/// Applies FluxRT streaming parameters once the peer is connected.
#[tauri::command]
fn session_apply_fluxrt(key: String) -> R<()> {
    with_session(&key, |s| s.set_custom_fps(fluxrt::session_fps()))
}

#[tauri::command]
fn session_fps(key: String, fps: i32) -> R<()> {
    with_session(&key, |s| s.set_custom_fps(fps))
}

#[tauri::command]
fn session_toggle(key: String, name: String) -> R<bool> {
    with_session(&key, |s| {
        s.toggle_option(name.clone());
        s.get_toggle_option(name)
    })
}

#[tauri::command]
fn session_get_toggle(key: String, name: String) -> R<bool> {
    with_session(&key, |s| s.get_toggle_option(name))
}

#[tauri::command]
fn session_chat(key: String, text: String) -> R<()> {
    if text.len() > 4096 {
        return Err("message too long".to_owned());
    }
    with_session(&key, |s| s.send_chat(text))
}

#[tauri::command]
fn session_restart_remote(key: String) -> R<()> {
    with_session(&key, |s| s.restart_remote_device())
}

#[tauri::command]
fn session_webcodecs_failed(key: String, codec: u8, display: i32) {
    core::session_webcodecs_failed(&key, codec, display);
}

#[tauri::command]
fn session_codec(key: String, codec: String) -> R<()> {
    with_session(&key, |s| {
        s.set_option("codec-preference".to_owned(), codec);
        s.update_supported_decodings();
    })
}

// ----------------------------------------------------------------------------- files

#[tauri::command]
fn fs_local_dir(path: String, hidden: bool) -> R<Value> {
    core::files::read_local_dir(&path, hidden)
}

#[tauri::command]
fn fs_home_dir() -> String {
    core::files::home_dir()
}

#[tauri::command]
fn fs_remote_dir(key: String, path: String, hidden: bool) -> R<()> {
    with_session(&key, |s| core::files::read_remote_dir(s, path, hidden))
}

#[tauri::command]
fn fs_transfer(
    key: String,
    id: i32,
    path: String,
    to: String,
    file_num: i32,
    hidden: bool,
    is_remote: bool,
) -> R<()> {
    with_session(&key, |s| {
        core::files::send_files(s, id, path, to, file_num, hidden, is_remote)
    })
}

#[tauri::command]
fn fs_cancel(key: String, id: i32) -> R<()> {
    with_session(&key, |s| core::files::cancel_job(s, id))
}

#[tauri::command]
fn fs_create_dir(key: String, id: i32, path: String, is_remote: bool) -> R<()> {
    with_session(&key, |s| core::files::create_dir(s, id, path, is_remote))
}

#[tauri::command]
fn fs_remove(key: String, id: i32, path: String, is_remote: bool, is_dir: bool) -> R<()> {
    with_session(&key, |s| core::files::remove(s, id, path, is_remote, is_dir))
}

#[tauri::command]
fn fs_override(key: String, id: i32, file_num: i32, need_override: bool, remember: bool, is_upload: bool) -> R<()> {
    with_session(&key, |s| {
        core::files::confirm_override(s, id, file_num, need_override, remember, is_upload)
    })
}

// ----------------------------------------------------------------------------- connection manager

#[tauri::command]
fn cm_clients() -> Vec<Value> {
    sink::get().map(|s| s.cm_clients()).unwrap_or_default()
}

#[tauri::command]
fn cm_authorize(id: i32) {
    cm::authorize(id);
}

#[tauri::command]
fn cm_close(id: i32) {
    cm::close(id);
}

#[tauri::command]
fn cm_permission(id: i32, name: String, enabled: bool) {
    cm::switch_permission(id, name, enabled);
}

#[tauri::command]
fn cm_chat(id: i32, text: String) {
    cm::send_chat(id, text);
}

// --- policy/update/diagnostics/cli ---

#[tauri::command]
fn policy_state() -> Value {
    crate::policy::state_json()
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> R<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn policy_refresh() -> R<Value> {
    blocking(crate::policy::refresh).await??;
    Ok(crate::policy::state_json())
}

#[tauri::command]
fn update_status() -> Value {
    crate::update::status()
}

#[tauri::command]
async fn update_check() -> R<Value> {
    blocking(crate::update::check).await?
}

#[tauri::command]
async fn update_install(app: AppHandle) -> R<()> {
    crate::update::install(app).await
}

#[tauri::command]
async fn diag_info() -> R<Value> {
    blocking(crate::diag::info).await
}

#[tauri::command]
async fn diag_test() -> Value {
    crate::diag::test_connection().await
}

#[tauri::command]
fn diag_verbose(on: bool) -> R<u64> {
    crate::diag::set_verbose(on)
}

#[tauri::command]
fn diag_open_logs() -> R<()> {
    crate::diag::open_log_folder()
}

/// Writes the diagnostics zip to the Downloads folder and returns its path.
#[tauri::command]
async fn diag_export() -> R<String> {
    blocking(crate::diag::export_to_downloads).await?
}

#[tauri::command]
async fn diag_upload(ticket: String) -> R<()> {
    blocking(move || crate::diag::upload(&ticket)).await?
}

// --- end policy/update/diagnostics/cli ---

pub fn handler() -> impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        status,
        refresh_password,
        set_permanent_password,
        get_option,
        set_option,
        tfa_enabled,
        tfa_begin,
        tfa_confirm,
        tfa_disable,
        recent_peers,
        remove_peer,
        install_service,
        open_session,
        session_start,
        session_close,
        session_reconnect,
        session_login,
        session_mouse,
        session_key,
        session_type_text,
        session_ctrl_alt_del,
        session_lock_screen,
        session_switch_display,
        session_refresh,
        session_quality,
        session_fps,
        session_apply_fluxrt,
        session_toggle,
        session_get_toggle,
        session_chat,
        session_restart_remote,
        session_webcodecs_failed,
        session_codec,
        fs_local_dir,
        fs_home_dir,
        fs_remote_dir,
        fs_transfer,
        fs_cancel,
        fs_create_dir,
        fs_remove,
        fs_override,
        cm_clients,
        cm_authorize,
        cm_close,
        cm_permission,
        cm_chat,
        // --- policy/update/diagnostics/cli ---
        policy_state,
        policy_refresh,
        update_status,
        update_check,
        update_install,
        diag_info,
        diag_test,
        diag_verbose,
        diag_open_logs,
        diag_export,
        diag_upload,
        // --- reconnect/recording ---
        session_policy_get,
        session_record,
        recording_finished,
        cm_record_consent_init,
        cm_record_consent,
        // --- end reconnect/recording ---
    ]
}

// --- reconnect/recording ---
use crate::{recording, session_policy};

/// Policy value for the webview (`connect.reconnect-minutes`, `recording.*`).
#[tauri::command]
fn session_policy_get(id: String) -> String {
    session_policy::get(&id)
}

/// Start/stop recording. Starting without the host's permission asks the host user first.
#[tauri::command]
fn session_record(key: String, start: bool) -> R<()> {
    if start {
        if session_policy::get("recording.mode") == "off" {
            return Err("Recording is disabled by policy.".to_owned());
        }
        recording::apply_folder();
        recording::started();
    }
    with_session(&key, |s| s.record_screen(start))
}

/// A recording stopped or a session window opened: queue closed recordings, retry uploads,
/// prune old files.
#[tauri::command]
fn recording_finished() {
    std::thread::spawn(|| {
        // The recorder finalises its file when its video thread drops it.
        std::thread::sleep(std::time::Duration::from_secs(3));
        recording::after_session();
    });
}

/// Host side: load the consent question from policy (call once when the CM window opens).
#[tauri::command]
fn cm_record_consent_init() {
    cm::set_record_consent_text(session_policy::get("recording.consent-text"));
}

/// Host side: answer to a `record_consent_request` event.
#[tauri::command]
fn cm_record_consent(conn_id: i32, allow: bool) {
    cm::record_consent(conn_id, allow);
}
// --- end reconnect/recording ---

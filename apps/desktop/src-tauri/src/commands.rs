//! IPC commands exposed to the webview. Thin wrappers over `librustdesk::ui`.

use librustdesk::ui::{self as core, api, cm};
use serde::Serialize;
use serde_json::Value;
use tauri::{
    ipc::{Channel, InvokeResponseBody},
    AppHandle,
};

use crate::{app, fluxrt, keys, security, sink};

type R<T> = Result<T, String>;

fn with_session<T>(key: &str, f: impl FnOnce(&core::TauriSession) -> T) -> R<T> {
    core::session(key)
        .map(|s| f(&s))
        .ok_or_else(|| "Not connected to the remote device.".to_owned())
}

/// Commands that change this machine's security posture answer only the main window: a session
/// or connection window shows content from the other side.
fn main_only(w: &tauri::WebviewWindow) -> R<()> {
    main_label(w.label())
}

fn main_label(label: &str) -> R<()> {
    if label == "main" {
        Ok(())
    } else {
        Err("not available in this window".to_owned())
    }
}

const CHAT_MAX: usize = 4096;

/// Chat text: bounded, and only while policy `connect.chat` allows chat.
fn check_chat(text: &str) -> R<()> {
    if text.len() > CHAT_MAX {
        return Err("message too long".to_owned());
    }
    crate::policy::require("connect.chat", "Your organisation does not allow chat.")
}

/// Session types the windows may start.
fn conn_type(conn: Option<&str>) -> R<&'static str> {
    match conn {
        None | Some("default") => Ok("default"),
        Some("file-transfer") => Ok("file-transfer"),
        Some(_) => Err("unsupported connection type".to_owned()),
    }
}

// ----------------------------------------------------------------------------- main window

#[derive(Serialize)]
pub struct Status {
    id: String,
    password: String,
    /// -1 not ready, 0 connecting, 1 ready
    status: i32,
    key_confirmed: bool,
    /// Unattended access is on (Windows: the service runs; macOS: the launchd jobs are installed).
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
        installed: api::is_service_installed(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

#[tauri::command]
fn refresh_password() {
    api::update_temporary_password();
}

#[tauri::command]
fn set_permanent_password(window: tauri::WebviewWindow, password: String) -> R<()> {
    main_only(&window)?;
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
fn set_option(window: tauri::WebviewWindow, key: String, value: String) -> R<()> {
    main_only(&window)?;
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
fn tfa_begin(window: tauri::WebviewWindow) -> R<String> {
    main_only(&window)?;
    Ok(api::generate2fa())
}

#[tauri::command]
fn tfa_confirm(window: tauri::WebviewWindow, code: String) -> R<()> {
    main_only(&window)?;
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
fn tfa_disable(window: tauri::WebviewWindow) -> R<()> {
    main_only(&window)?;
    api::disable_2fa();
    Ok(())
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
async fn install_service(window: tauri::WebviewWindow) -> R<()> {
    main_only(&window)?;
    crate::policy::require("security.unattended", "Your organisation does not allow unattended access.")?;
    #[cfg(target_os = "macos")]
    if !api::is_installed() {
        return Err("Move Tether into the Applications folder first, then start it from there.".to_owned());
    }
    // Windows: on success the core restarts Tether, so this only returns on failure or cancel.
    if blocking(api::install_service).await? {
        // macOS: the launchd agent takes over as host once this process is gone, then Tether reopens.
        #[cfg(target_os = "macos")]
        tauri::Manager::app_handle(&window).exit(0);
        Ok(())
    } else {
        Err("The service was not installed (cancelled, or see the log).".to_owned())
    }
}

/// macOS only (None elsewhere): Screen Recording (else the technician gets no picture) and
/// Accessibility (else remote mouse and keyboard are ignored).
#[tauri::command]
fn mac_permissions() -> Option<serde_json::Value> {
    cfg!(target_os = "macos").then(|| serde_json::json!({ "screen": api::is_can_screen_recording(false), "accessibility": api::is_process_trusted(false) }))
}

/// Clear a stale grant, let macOS ask, open the matching System Settings page.
#[tauri::command]
fn mac_fix(app: tauri::AppHandle, which: String) -> R<()> {
    api::mac_fix(&app.config().identifier, &which)
}

/// Async: creating a window from a synchronous command deadlocks on Windows (WebView2).
/// `from`: a live session to the same peer whose login the new one reuses (no second password).
#[tauri::command]
async fn open_session(app: AppHandle, peer: String, conn: Option<String>, from: Option<String>, view_only: Option<bool>) -> R<String> {
    let peer = peer.trim().replace(' ', "");
    security::check_peer_id(&peer)?;
    security::check_not_self(&peer)?;
    crate::policy::check_outgoing()?;
    let conn = conn_type(conn.as_deref())?;
    if let Some(f) = &from {
        if core::session(f).is_none() {
            return Err("The session has ended; connect again.".to_owned());
        }
    }
    let c = crate::cli::Connect { peer: peer.clone(), conn: conn.to_owned(), view_only: view_only.unwrap_or(false), ..Default::default() };
    let key = app::open_session_with(&app, &c).map_err(|e| e.to_string())?;
    if let (Some(f), Some(s)) = (from, sink::get()) {
        s.set_pending_login(&key, f);
    }
    Ok(key)
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
    security::check_not_self(&peer)?;
    if !key.chars().all(|c| c.is_ascii_hexdigit()) || key.len() != 32 {
        return Err("invalid session key".to_owned());
    }
    if password.as_ref().is_some_and(|p| p.len() > 128) {
        return Err("password too long".to_owned());
    }
    // The same checks as `open_session`: a window cannot start what the main window could not.
    let conn = conn_type(conn.as_deref())?;
    crate::policy::check_outgoing()?;
    if webcodecs.len() > 8 || webcodecs.iter().any(|f| f.len() > 8) {
        return Err("invalid codec list".to_owned());
    }
    let mut password = password;
    let mut login_from = None;
    if let Some(s) = sink::get() {
        s.attach_video(&key, video);
        if password.is_none() {
            password = s.take_pending_password(&key);
        }
        login_from = s.take_pending_login(&key);
    }
    core::session_start(
        &key,
        &peer,
        password.as_deref().unwrap_or(""),
        conn,
        force_relay.unwrap_or(false),
        login_from.as_deref(),
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
    if password.len() > 128 {
        return Err("password too long".to_owned());
    }
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

/// `code` is a DOM `KeyboardEvent.code` (layout independent physical key). Goes through the
/// PC/Mac shortcut mapping (`keys.rs`).
#[tauri::command]
fn session_key(key: String, code: String, down: bool, lock_modes: i32, character: Option<String>) -> R<()> {
    // One typed character (a few code points at most).
    if character.as_ref().is_some_and(|c| c.len() > 32) {
        return Err("invalid character".to_owned());
    }
    let k = rdev::chrome_key_from_code(&code);
    if rdev::usb_hid_keycode_from_key(k).unwrap_or(0) == 0 {
        return Ok(());
    }
    with_session(&key, |_| keys::webview_input(&key, k, down, character.as_deref().unwrap_or(""), lock_modes))
}

/// Keyboard settings of a session window (toolbar); every argument is optional. `pause` is set
/// while a text field of the window has the focus (the system-key hook lets keys through).
#[tauri::command]
fn session_keyboard(
    key: String,
    shortcuts: Option<bool>,
    system_keys: Option<bool>,
    alt_tab_remote: Option<bool>,
    pause: Option<bool>,
) -> R<keys::Settings> {
    with_session(&key, |_| keys::settings(&key, shortcuts, system_keys, alt_tab_remote, pause))
}

/// Key-ups for every key still held on the remote side.
#[tauri::command]
fn session_release_keys(key: String) {
    keys::release(&key);
}

/// Cmd+Option+Esc (Force Quit Applications) on a Mac host.
#[tauri::command]
fn session_mac_force_quit(key: String) -> R<()> {
    with_session(&key, |_| keys::mac_force_quit(&key))
}

#[tauri::command]
fn session_type_text(key: String, text: String) -> R<()> {
    if text.len() > 4096 {
        return Err("text too long".to_owned());
    }
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

/// "Request admin access": a TetherQS host starts its elevated helper once its user confirms UAC.
#[tauri::command]
fn session_elevate(key: String) -> R<()> {
    with_session(&key, |s| s.elevate_direct())
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
    check_chat(&text)?;
    with_session(&key, |s| s.send_chat(text))
}

#[tauri::command]
fn session_restart_remote(key: String) -> R<()> {
    with_session(&key, |s| s.restart_remote_device())
}

/// The webview drew a natively decoded picture (see `TauriSink::video`).
#[tauri::command]
fn session_frame_drawn(key: String) {
    if let Some(s) = sink::get() {
        s.rgba_drawn(&key);
    }
}

#[tauri::command]
fn session_webcodecs_failed(key: String, codec: u8, display: i32) -> R<()> {
    // The core shifts a u8 mask by it.
    if codec >= 8 {
        return Err("unknown codec".to_owned());
    }
    core::session_webcodecs_failed(&key, codec, display);
    Ok(())
}

#[tauri::command]
fn session_codec(key: String, codec: String) -> R<()> {
    hbb_common::log::info!("codec preference {codec} requested by the viewer");
    with_session(&key, |s| {
        s.set_option("codec-preference".to_owned(), codec);
        s.update_supported_decodings();
    })
}

/// A message the main window shows once as a toast (app::notify), e.g. a refused `--connect` to the own ID.
#[tauri::command]
fn take_notice() -> Option<String> {
    app::take_notice()
}

/// Debug builds only: `TETHER_DEV_VIDEO_STALL` (`webcodecs` or `native`) makes the viewer draw
/// nothing, like a silently failing decoder, to test its watchdog (lib/video.ts). Release: "".
#[tauri::command]
fn dev_video_stall() -> String {
    if cfg!(debug_assertions) {
        std::env::var("TETHER_DEV_VIDEO_STALL").unwrap_or_default()
    } else {
        String::new()
    }
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

/// A folder delete: `fs_remove` lists its files (`folder_files` with this id), the window removes
/// them, then this removes the then empty folder tree.
#[tauri::command]
fn fs_remove_empty_dirs(key: String, id: i32, path: String, is_remote: bool) -> R<()> {
    with_session(&key, |s| core::files::remove_empty_dirs(s, id, path, is_remote))
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
    if let Some(s) = sink::get() {
        s.cm_authorized(id);
    }
}

#[tauri::command]
fn cm_close(id: i32) {
    cm::close(id);
}

/// Core permission name → policy feature (None: no policy feature, the host user decides).
pub const PERMS: &[(&str, Option<&str>)] = &[
    ("keyboard", Some("permission.control")),
    ("clipboard", Some("permission.clipboard")),
    ("file", Some("permission.file-transfer")),
    ("audio", Some("permission.audio")),
    ("restart", Some("permission.restart")),
    ("block_input", Some("permission.block-input")),
    ("recording", None),
];

/// Policy keeps this feature off (locked or hidden).
fn locked_off(e: &tether_trust::policy::Effective, feature: &str) -> bool {
    (e.is_locked(feature) || e.hidden.contains(feature)) && !e.on(feature)
}

/// Permissions policy keeps off: never granted here, and taken from running connections (sink.rs).
pub fn perms_locked_off(e: &tether_trust::policy::Effective) -> Vec<&'static str> {
    PERMS.iter().filter(|(_, f)| f.is_some_and(|f| locked_off(e, f))).map(|(n, _)| *n).collect()
}

fn check_perm(e: &tether_trust::policy::Effective, name: &str, enabled: bool) -> R<()> {
    if !PERMS.iter().any(|(n, _)| *n == name) {
        return Err("unknown permission".to_owned());
    }
    if enabled && perms_locked_off(e).contains(&name) {
        return Err("locked: Your organisation does not allow this.".to_owned());
    }
    Ok(())
}

#[tauri::command]
fn cm_permission(id: i32, name: String, enabled: bool) -> R<()> {
    check_perm(&crate::policy::effective(), &name, enabled)?;
    cm::switch_permission(id, name.clone(), enabled);
    if let Some(s) = sink::get() {
        s.cm_set_field(id, &name, enabled);
    }
    Ok(())
}

#[tauri::command]
fn cm_chat(id: i32, text: String) -> R<()> {
    check_chat(&text)?;
    cm::send_chat(id, text);
    Ok(())
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
async fn update_install(app: AppHandle, auto: Option<bool>) -> R<()> {
    // `auto`: the forced countdown, refused while a session runs (D7).
    crate::update::install(app, auto.unwrap_or(false)).await
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
        mac_permissions,
        mac_fix,
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
        session_keyboard,
        session_release_keys,
        session_mac_force_quit,
        session_type_text,
        session_ctrl_alt_del,
        session_lock_screen,
        session_elevate,
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
        session_frame_drawn,
        session_codec,
        dev_video_stall,
        take_notice,
        fs_local_dir,
        fs_home_dir,
        fs_remote_dir,
        fs_transfer,
        fs_cancel,
        fs_create_dir,
        fs_remove,
        fs_remove_empty_dirs,
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
        recordings_list,
        recordings_open,
        recordings_delete,
        recordings_pick_folder,
        recordings_reset_folder,
        cm_record_consent_init,
        cm_record_consent,
        // --- end reconnect/recording ---
        hosts_peers,
        hosts_store_load,
        hosts_store_save,
        hosts_online,
        main_window_fit,
        sessions_open,
        session_focus_window,
        hosts_export,
        hosts_import,
        team_identity,
        team_me,
        team_devices,
        team_tags_get,
        team_tags_put,
        team_audit,
    ]
}

// --- advanced mode (docs/advanced.md) ---
use crate::{hosts, team};

#[tauri::command]
fn hosts_peers() -> Vec<Value> {
    hosts::peers()
}

#[tauri::command]
async fn hosts_store_load() -> R<String> {
    blocking(hosts::load_store).await?
}

#[tauri::command]
async fn hosts_store_save(window: tauri::WebviewWindow, doc: String) -> R<()> {
    main_only(&window)?;
    blocking(move || hosts::save_store(&doc)).await?
}

#[tauri::command]
async fn hosts_online(ids: Vec<String>) -> R<Vec<String>> {
    hosts::online(ids).await
}

#[tauri::command]
fn main_window_fit(app: AppHandle, advanced: bool) -> R<()> {
    app::fit_main(&app, advanced).map_err(|e| e.to_string())
}

#[tauri::command]
fn sessions_open() -> Vec<Value> {
    app::open_sessions().into_iter().map(|(key, peer, conn)| serde_json::json!({ "key": key, "peer": peer, "conn": conn })).collect()
}

#[tauri::command]
fn session_focus_window(app: AppHandle, key: String) -> bool {
    app::focus_session(&app, &key)
}

/// Save an export where the user picks (native dialog); false when cancelled.
#[tauri::command]
async fn hosts_export(app: AppHandle, name: String, content: String) -> R<bool> {
    use tauri_plugin_dialog::DialogExt;
    let ext = hosts::export_ext(&name)?;
    if content.len() > hosts::STORE_MAX * 2 {
        return Err("The export is too large.".into());
    }
    let picked = blocking(move || app.dialog().file().set_title("Export hosts").set_file_name(&name).add_filter(ext.to_uppercase(), &[ext]).blocking_save_file()).await?;
    let Some(p) = picked.and_then(|p| p.into_path().ok()) else { return Ok(false) };
    blocking(move || std::fs::write(p, content).map(|_| true).map_err(|e| format!("The file could not be written: {e}"))).await?
}

/// Read a file the user picks (native dialog) for import; None when cancelled.
#[tauri::command]
async fn hosts_import(app: AppHandle) -> R<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let picked = blocking(move || app.dialog().file().set_title("Import hosts").add_filter("Hosts (JSON, CSV)", &["json", "csv", "txt"]).blocking_pick_file()).await?;
    let Some(p) = picked.and_then(|p| p.into_path().ok()) else { return Ok(None) };
    blocking(move || hosts::read_import(&p).map(Some)).await?
}

#[tauri::command]
async fn team_identity() -> R<Value> {
    blocking(team::identity).await?
}

#[tauri::command]
async fn team_me() -> R<Value> {
    blocking(team::me).await?
}

#[tauri::command]
async fn team_devices() -> R<Value> {
    blocking(team::devices).await?
}

#[tauri::command]
async fn team_tags_get() -> R<Value> {
    blocking(team::tags_get).await?
}

#[tauri::command]
async fn team_tags_put(window: tauri::WebviewWindow, rev: u64, doc: String) -> R<Value> {
    main_only(&window)?;
    blocking(move || team::tags_put(rev, &doc)).await?
}

#[tauri::command]
async fn team_audit(device: String, since: u64) -> R<Value> {
    blocking(move || team::audit(&device, since)).await?
}
// --- end advanced mode ---

// --- reconnect/recording ---
use crate::recording;

/// Policy values the webview reads (Session.tsx). Anything else, like `recording.upload-token`
/// (masked in `policy_state` too), stays in Rust.
const UI_POLICY: &[&str] = &["connect.reconnect-minutes", "recording.mode"];

#[tauri::command]
fn session_policy_get(id: String) -> R<String> {
    if !UI_POLICY.contains(&id.as_str()) {
        return Err(format!("{id} is not readable here"));
    }
    Ok(crate::policy::get(&id))
}

/// Start/stop recording. Starting without the host's permission asks the host user first.
#[tauri::command]
fn session_record(key: String, start: bool) -> R<()> {
    if start {
        if crate::policy::get("recording.mode") == "off" {
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

#[derive(Serialize)]
struct RecordingsState {
    folder: String,
    /// The folder was picked in Settings (else policy or default).
    custom: bool,
    /// Policy fixes the folder: no picker.
    locked: bool,
    items: Vec<recording::Recording>,
}

/// The recordings folder and its recordings, newest first.
#[tauri::command]
async fn recordings_list() -> R<RecordingsState> {
    blocking(|| RecordingsState {
        folder: recording::folder().to_string_lossy().into_owned(),
        custom: recording::user_folder().is_some(),
        locked: recording::folder_locked(),
        items: recording::list_current(),
    })
    .await
}

/// `name` None opens the folder; else plays the recording, or selects it (`reveal`) in the file manager.
#[tauri::command]
fn recordings_open(name: Option<String>, reveal: Option<bool>) -> R<()> {
    recording::open(name.as_deref(), reveal.unwrap_or(false))
}

#[tauri::command]
async fn recordings_delete(name: String) -> R<()> {
    blocking(move || recording::delete(&name)).await?
}

/// Folder picker; the chosen folder is used from the next recording on. None = cancelled.
#[tauri::command]
async fn recordings_pick_folder(app: AppHandle) -> R<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    if recording::folder_locked() {
        return Err("locked:recording.folder: Your organisation manages the recording folder.".to_owned());
    }
    let start = recording::folder();
    let picked = blocking(move || app.dialog().file().set_title("Recording folder").set_directory(start).blocking_pick_folder()).await?;
    let Some(p) = picked.and_then(|p| p.into_path().ok()) else { return Ok(None) };
    let s = p.to_string_lossy().into_owned();
    recording::set_user_folder(&s)?;
    Ok(Some(s))
}

/// Back to the folder from policy (or the default).
#[tauri::command]
fn recordings_reset_folder() -> R<()> {
    recording::set_user_folder("")
}

/// Host side: load the consent question from policy (call once when the CM window opens).
#[tauri::command]
fn cm_record_consent_init() {
    cm::set_record_consent_text(crate::policy::get("recording.consent-text"));
}

/// Host side: answer to a `record_consent_request` event.
#[tauri::command]
fn cm_record_consent(conn_id: i32, allow: bool) {
    cm::record_consent(conn_id, allow);
}
// --- end reconnect/recording ---

#[cfg(test)]
mod tests {
    use tether_trust::policy::{resolve, Layer, Rule, State, PROFILE_TECHNICIAN};

    #[test]
    fn permissions_are_allow_listed_and_follow_policy_locks() {
        let open = resolve(None, PROFILE_TECHNICIAN, "", None);
        assert!(super::check_perm(&open, "clipboard", true).is_ok());
        assert!(super::check_perm(&open, "recording", true).is_ok());
        assert!(super::check_perm(&open, "privacy_mode", true).is_err());
        assert!(super::perms_locked_off(&open).is_empty());
        let machine: Layer = [("permission.clipboard".to_owned(), Rule { state: State::Off, value: None }), ("permission.file-transfer".to_owned(), Rule { state: State::Hidden, value: None })].into();
        let locked = resolve(None, PROFILE_TECHNICIAN, "", Some(&machine));
        assert_eq!(super::perms_locked_off(&locked), ["clipboard", "file"]);
        assert!(super::check_perm(&locked, "clipboard", true).unwrap_err().starts_with("locked:"));
        assert!(super::check_perm(&locked, "file", true).is_err());
        // Switching off is always allowed.
        assert!(super::check_perm(&locked, "clipboard", false).is_ok());
        assert!(super::check_perm(&locked, "keyboard", true).is_ok());
    }

    #[test]
    fn boundary_checks() {
        assert!(super::main_label("main").is_ok());
        assert!(super::main_label("cm").is_err());
        assert!(super::main_label("s-0123").is_err());
        assert_eq!(super::conn_type(None), Ok("default"));
        assert_eq!(super::conn_type(Some("file-transfer")), Ok("file-transfer"));
        assert!(super::conn_type(Some("terminal")).is_err());
        assert!(super::conn_type(Some("port-forward")).is_err());
        assert!(super::check_chat(&"x".repeat(super::CHAT_MAX + 1)).is_err());
        assert!(super::session_login("k".into(), "p".repeat(129), false).unwrap_err().contains("too long"));
        assert!(super::session_type_text("k".into(), "t".repeat(4097)).unwrap_err().contains("too long"));
        assert!(super::session_webcodecs_failed("k".into(), 8, 0).is_err());
        assert!(super::session_webcodecs_failed("k".into(), 255, 0).is_err());
        assert!(super::session_key("k".into(), "KeyA".into(), true, 0, Some("x".repeat(33))).is_err());
    }

    #[test]
    fn session_policy_get_refuses_secrets() {
        assert!(super::session_policy_get("recording.upload-token".into()).is_err());
        assert!(super::session_policy_get("recording.upload-target".into()).is_err());
    }
}

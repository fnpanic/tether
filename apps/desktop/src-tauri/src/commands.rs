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

// --- health monitoring agent through a session (spec 6.4, 7.4)

use librustdesk::ui::agent::{Action as AgentAction, TetherAgentAnswer};

/// A host that knows the message answers `status` at once; no answer within this is an old host.
const AGENT_STATUS_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// What `ask_agent` needs from a session (a fake in the tests).
trait AgentPeer {
    fn ask_agent(&self, action: AgentAction) -> hbb_common::tokio::sync::oneshot::Receiver<TetherAgentAnswer>;
}

impl AgentPeer for core::TauriSession {
    fn ask_agent(&self, action: AgentAction) -> hbb_common::tokio::sync::oneshot::Receiver<TetherAgentAnswer> {
        core::TauriSession::ask_agent(self, action)
    }
}

/// `status` answers at once; install and remove wait for the host's own bound and a little more.
fn agent_wait(action: AgentAction, status_wait: std::time::Duration) -> std::time::Duration {
    match action {
        AgentAction::Status => status_wait,
        _ => librustdesk::ui::agent::HOST_WAIT + status_wait,
    }
}

fn to_wire(a: crate::agent::AgentAnswer) -> TetherAgentAnswer {
    TetherAgentAnswer { state: a.state, version: a.version, installed_by: a.installed_by, detail: a.detail, at: a.at, ..Default::default() }
}

fn from_wire(a: TetherAgentAnswer) -> crate::agent::AgentAnswer {
    crate::agent::AgentAnswer { state: a.state, version: a.version, installed_by: a.installed_by, detail: a.detail, at: a.at }
}

/// Host: the core's handler for a technician's request (set in `main`, every process). `viewer`: the
/// name their program gave (the elevated path's dialog shows it as not verified).
pub fn host_agent(action: AgentAction, viewer: &str) -> TetherAgentAnswer {
    to_wire(match action {
        AgentAction::Status => crate::agent::status(),
        AgentAction::Install => crate::agent::request(crate::agent::Action::Install, viewer),
        AgentAction::Remove => crate::agent::request(crate::agent::Action::Remove, viewer),
    })
}

async fn ask_agent(peer: &impl AgentPeer, action: &str, status_wait: std::time::Duration) -> R<crate::agent::AgentAnswer> {
    let action = match action {
        "status" => AgentAction::Status,
        "install" => AgentAction::Install,
        "remove" => AgentAction::Remove,
        _ => return Err("unknown action".to_owned()),
    };
    let rx = peer.ask_agent(action);
    Ok(match hbb_common::tokio::time::timeout(agent_wait(action, status_wait), rx).await {
        // The host stopped waiting, the action goes on: the dialog says so and offers to ask again
        // (`status` shows the outcome). The sentence is the viewer's own, not the host's.
        Ok(Ok(a)) if a.state == "running" => crate::agent::AgentAnswer { state: "running".into(), detail: librustdesk::ui::agent::RUNNING.into(), ..Default::default() },
        Ok(Ok(a)) => from_wire(a),
        Ok(Err(_)) => crate::agent::AgentAnswer { state: "failed".into(), detail: "the session ended or asked again".into(), ..Default::default() },
        Err(_) if action == AgentAction::Status => crate::agent::AgentAnswer { state: "too-old".into(), detail: "Tether on this computer is too old (needs 0.7.0)".into(), ..Default::default() },
        Err(_) => crate::agent::AgentAnswer { state: "failed".into(), detail: "no answer from the remote computer".into(), ..Default::default() },
    })
}

/// The session's health agent dialog: `action` is `status`, `install` or `remove`. `state` is one of
/// `not-installed`, `installed`, `installed-by-hand`, `removed-locally`, `removed-by-tether`,
/// `failed`, `running` (still going on the host: ask `status` again), `busy`, `not-allowed`,
/// `needs-service`, `not-supported`, `too-old`; anything else is shown as a generic line with `detail`.
#[tauri::command]
async fn session_agent(key: String, action: String) -> R<crate::agent::AgentAnswer> {
    let s = core::session(&key).ok_or_else(|| "Not connected to the remote device.".to_owned())?;
    ask_agent(&s, &action, AGENT_STATUS_WAIT).await
}

/// Settings › this computer's agent (spec 7.3): whether its service is there, who installed it and
/// when; never a secret (agent.rs `local_status`).
#[tauri::command]
async fn health_agent_status() -> R<crate::agent::AgentAnswer> {
    blocking(crate::agent::local_status).await
}

/// The home view's Install card is shown (policy `install`, Tether without its service, no agent).
#[tauri::command]
async fn health_agent_offer(window: tauri::WebviewWindow) -> R<bool> {
    main_only(&window)?;
    blocking(crate::agent::offer).await
}

/// The card's Install…: the same pre-dialog and prompt as a technician's request.
#[tauri::command]
async fn health_agent_install(window: tauri::WebviewWindow) -> R<crate::agent::AgentAnswer> {
    main_only(&window)?;
    blocking(crate::agent::install_here).await
}

/// The pre-dialog waiting for an answer, if any (the main window shows it).
#[tauri::command]
fn health_agent_consent_pending(window: tauri::WebviewWindow) -> R<Option<crate::agent_elevated::Dialog>> {
    main_only(&window)?;
    Ok(crate::agent_elevated::pending())
}

/// The person's answer to pre-dialog `id` (only the main window; remote input never reaches it).
#[tauri::command]
fn health_agent_consent_answer(window: tauri::WebviewWindow, id: u64, yes: bool) -> R<bool> {
    main_only(&window)?;
    Ok(crate::agent_elevated::answer(id, yes))
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
        session_agent,
        health_agent_status,
        health_agent_offer,
        health_agent_install,
        health_agent_consent_pending,
        health_agent_consent_answer,
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
        team_status,
        team_status_host,
        team_status_sbom,
        team_status_software,
        team_status_find,
        team_status_link,
        team_status_unlink,
        team_status_remove,
        team_status_allow_claim,
        health_notify,
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

#[tauri::command]
async fn team_status() -> R<Value> {
    blocking(team::status).await?
}

#[tauri::command]
async fn team_status_host(relay: String, uuid: String) -> R<Value> {
    blocking(move || team::status_host(&relay, &uuid)).await?
}

#[tauri::command]
async fn team_status_sbom(relay: String, uuid: String) -> R<String> {
    blocking(move || team::status_sbom(&relay, &uuid)).await?
}

/// The software search of one relay (`software:` in the host list); the webview asks each relay.
#[tauri::command]
async fn team_status_software(relay: String, q: String, v: String) -> R<Value> {
    blocking(move || team::status_software(&relay, &q, &v)).await?
}

#[tauri::command]
async fn team_status_link(window: tauri::WebviewWindow, uuid: String, tether_id: String) -> R<()> {
    main_only(&window)?;
    blocking(move || team::status_link(&uuid, &tether_id)).await?
}

#[tauri::command]
async fn team_status_unlink(window: tauri::WebviewWindow, uuid: String) -> R<()> {
    main_only(&window)?;
    blocking(move || team::status_unlink(&uuid)).await?
}

#[tauri::command]
async fn team_status_remove(window: tauri::WebviewWindow, uuid: String) -> R<()> {
    main_only(&window)?;
    blocking(move || team::status_remove(&uuid)).await?
}

#[tauri::command]
async fn team_status_allow_claim(window: tauri::WebviewWindow, uuid: String) -> R<()> {
    main_only(&window)?;
    blocking(move || team::status_allow_claim(&uuid)).await?
}

/// The cached health summaries linked to a Tether ID (session window: two or more are a link
/// conflict); never a request.
#[tauri::command]
fn team_status_find(id: String) -> Vec<Value> {
    team::status_find(&id)
}

/// A health notification (host list, spec 10.5): a Windows toast. Other systems: nothing yet
/// (no notification facility in this app; see the health docs).
#[tauri::command]
async fn health_notify(title: String, body: String) -> R<()> {
    // Device-chosen host names end up here: plain text, bounded.
    let cut = |s: &str, n: usize| s.chars().filter(|c| !c.is_control()).take(n).collect::<String>();
    let (title, body) = (cut(&title, 120), cut(&body, 240));
    #[cfg(windows)]
    {
        blocking(move || {
            tauri_winrt_notification::Toast::new(tauri_winrt_notification::Toast::POWERSHELL_APP_ID)
                .title(&title)
                .text1(&body)
                .show()
                .map_err(|e| e.to_string())
        })
        .await??;
    }
    #[cfg(not(windows))]
    let _ = (title, body);
    Ok(())
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

    // --- health agent through a session (viewer command, host wiring, core message handling)

    use librustdesk::ui::agent::{self as core_agent, Action, InFlight, TetherAgentAnswer, TetherAgentRequest};
    use std::{cell::RefCell, rc::Rc, time::{Duration, Instant}};

    fn rt() -> hbb_common::tokio::runtime::Runtime {
        hbb_common::tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap()
    }

    /// A session: records what was asked and answers (or never answers, like an old host).
    struct FakePeer {
        asked: RefCell<Vec<Action>>,
        reply: Option<TetherAgentAnswer>,
        keep: RefCell<Vec<hbb_common::tokio::sync::oneshot::Sender<TetherAgentAnswer>>>,
    }

    impl FakePeer {
        fn new(reply: Option<TetherAgentAnswer>) -> Self {
            Self { asked: RefCell::default(), reply, keep: RefCell::default() }
        }
    }

    impl super::AgentPeer for FakePeer {
        fn ask_agent(&self, action: Action) -> hbb_common::tokio::sync::oneshot::Receiver<TetherAgentAnswer> {
            self.asked.borrow_mut().push(action);
            let (tx, rx) = hbb_common::tokio::sync::oneshot::channel();
            match &self.reply {
                Some(a) => drop(tx.send(a.clone())),
                None => self.keep.borrow_mut().push(tx),
            }
            rx
        }
    }

    fn wire(state: &str, detail: &str) -> TetherAgentAnswer {
        TetherAgentAnswer { state: state.into(), version: "0.2.0".into(), installed_by: "session".into(), detail: detail.into(), at: 7, ..Default::default() }
    }

    #[test]
    fn session_agent_sends_only_a_known_action() {
        let peer = FakePeer::new(Some(wire("installed", "")));
        for bad in ["", "upgrade", "Install", "install ", "status;remove", "../tether-monitor.exe", "https://x/y.exe"] {
            assert!(rt().block_on(super::ask_agent(&peer, bad, Duration::from_millis(50))).is_err(), "{bad}");
        }
        assert!(peer.asked.borrow().is_empty(), "nothing sent for an unknown action");
        for (s, a) in [("status", Action::Status), ("install", Action::Install), ("remove", Action::Remove)] {
            rt().block_on(super::ask_agent(&peer, s, Duration::from_millis(50))).unwrap();
            assert_eq!(peer.asked.borrow().last(), Some(&a));
        }
    }

    #[test]
    fn session_agent_returns_the_hosts_answer() {
        let peer = FakePeer::new(Some(wire("installed", "Installed 0.2.0")));
        let a = rt().block_on(super::ask_agent(&peer, "install", Duration::from_millis(50))).unwrap();
        assert_eq!(a, crate::agent::AgentAnswer { state: "installed".into(), version: "0.2.0".into(), installed_by: "session".into(), detail: "Installed 0.2.0".into(), at: 7 });
    }

    #[test]
    fn session_agent_old_host_is_too_old() {
        // An old host ignores the message: no answer within the short wait.
        let peer = FakePeer::new(None);
        let a = rt().block_on(super::ask_agent(&peer, "status", Duration::from_millis(50))).unwrap();
        assert_eq!(a.state, "too-old");
        assert!(a.detail.contains("too old"), "{}", a.detail);
        assert_eq!(super::AGENT_STATUS_WAIT, Duration::from_secs(10));
        // An install waits for the host's own bound (3 minutes) and a little more.
        assert!(super::agent_wait(Action::Install, super::AGENT_STATUS_WAIT) > core_agent::HOST_WAIT);
        assert_eq!(super::agent_wait(Action::Status, super::AGENT_STATUS_WAIT), super::AGENT_STATUS_WAIT);
        assert_eq!(core_agent::HOST_WAIT, Duration::from_secs(180));
    }

    #[test]
    fn session_agent_running_says_ask_again() {
        // The host stopped waiting while the install goes on: not "failed", and the host's own
        // text is not shown.
        let peer = FakePeer::new(Some(wire("running", "\u{1b}[2Jinjected")));
        let a = rt().block_on(super::ask_agent(&peer, "install", Duration::from_millis(50))).unwrap();
        assert_eq!(a.state, "running");
        assert_eq!(a.detail, core_agent::RUNNING);
        assert!(a.detail.contains("ask again"), "{}", a.detail);
    }

    #[test]
    fn host_wiring_carries_only_the_answer_fields() {
        let a = crate::agent::AgentAnswer { state: "failed".into(), version: "0.2.0".into(), installed_by: "policy".into(), detail: "token expired".into(), at: 9 };
        let w = super::to_wire(a.clone());
        assert_eq!(w, TetherAgentAnswer { state: "failed".into(), version: "0.2.0".into(), installed_by: "policy".into(), detail: "token expired".into(), at: 9, ..Default::default() });
        assert_eq!(super::from_wire(w), a);
        // The host's own answer on this machine comes from agent.rs (a test is not Tether's
        // LocalSystem service, or not Windows).
        let st = super::host_agent(Action::Status, "Sam");
        // Without Tether's service the elevated path answers; the policy here is `manual`.
        assert!(matches!(st.state.as_str(), "needs-service" | "not-supported" | "not-allowed"), "{}", st.state);
    }

    fn request(action: i32) -> TetherAgentRequest {
        let mut r = TetherAgentRequest::new();
        r.action = hbb_common::protobuf::EnumOrUnknown::from_i32(action);
        r
    }

    #[test]
    fn core_accepts_a_request_only_with_control() {
        // (Before login the core has no arm for the message: it is never checked or answered.)
        // Logged in without control (view only, or the permission switched off): refused.
        let refused = core_agent::check(&request(1), false).unwrap_err();
        assert_eq!(refused.state, "not-allowed");
        assert!(refused.detail.contains("control"), "{}", refused.detail);
        assert!(core_agent::check(&request(0), false).is_err());
        // An action outside the closed set (a newer viewer): refused, never run.
        let unknown = core_agent::check(&request(7), true).unwrap_err();
        assert_eq!(unknown.state, "not-supported");
        assert_eq!(core_agent::check(&request(0), true), Ok(Action::Status));
        assert_eq!(core_agent::check(&request(1), true), Ok(Action::Install));
        assert_eq!(core_agent::check(&request(2), true), Ok(Action::Remove));
    }

    #[test]
    fn core_message_parses_and_old_peers_skip_it() {
        use hbb_common::protobuf::Message as _;
        use librustdesk::ui::agent::{message, misc, Message};
        let m = core_agent::request_message(Action::Remove);
        let back = Message::parse_from_bytes(&m.write_to_bytes().unwrap()).unwrap();
        let Some(message::Union::Misc(misc)) = back.union else { panic!("not misc") };
        let Some(misc::Union::TetherAgentRequest(r)) = misc.union else { panic!("not the request") };
        assert_eq!(core_agent::check(&r, true), Ok(Action::Remove));
        let m = core_agent::answer_message(Action::Install, wire("installed", "x"));
        let back = Message::parse_from_bytes(&m.write_to_bytes().unwrap()).unwrap();
        let Some(message::Union::Misc(misc)) = back.union else { panic!("not misc") };
        // The answer echoes the request's action.
        assert_eq!(misc.union, Some(misc::Union::TetherAgentAnswer(TetherAgentAnswer { action: Action::Install.into(), ..wire("installed", "x") })));
        // A Misc variant this build does not know (field 1003, as 1001 and 1002 are to an old
        // peer) parses to no variant, which every Misc match in the core ignores (`_ => {}`).
        let mut bytes = core_agent::request_message(Action::Install).write_to_bytes().unwrap();
        // Field 1001, length-delimited: varint 0x1f4a = ca 3e, then the length 2 (action = 1).
        let pos = bytes.windows(3).position(|w| w == [0xca, 0x3e, 0x02]).expect("field 1001");
        bytes[pos] = 0xda; // field 1003: varint 0x1f5a = da 3e
        let old = Message::parse_from_bytes(&bytes).unwrap();
        let Some(message::Union::Misc(misc)) = old.union else { panic!("not misc") };
        assert_eq!(misc.union, None);
    }

    #[test]
    fn core_one_request_in_flight_per_connection() {
        let mut f = InFlight::default();
        let t = Instant::now();
        let g = f.start(Action::Status, t).expect("first");
        assert!(f.start(Action::Status, t).is_none(), "second while the first runs");
        assert!(f.start(Action::Install, t).is_none(), "an install while one runs");
        drop(g);
        assert!(f.start(Action::Status, t).is_some(), "free again");
    }

    #[test]
    fn core_install_or_remove_at_most_every_ten_seconds() {
        assert_eq!(core_agent::CHANGE_PACE, Duration::from_secs(10));
        let mut f = InFlight::default();
        let t = Instant::now();
        drop(f.start(Action::Install, t).expect("first install"));
        // Finished at once (a refusal), yet the next change within 10 s is busy.
        assert!(f.start(Action::Remove, t + Duration::from_secs(5)).is_none());
        assert!(f.start(Action::Install, t + Duration::from_millis(9999)).is_none());
        // `status` changes nothing and is not paced.
        drop(f.start(Action::Status, t + Duration::from_secs(5)).expect("status"));
        drop(f.start(Action::Remove, t + Duration::from_secs(10)).expect("after 10 s"));
        // A request refused as busy does not move the window.
        let held = f.start(Action::Status, t + Duration::from_secs(30)).unwrap();
        assert!(f.start(Action::Install, t + Duration::from_secs(30)).is_none());
        drop(held);
        drop(f.start(Action::Install, t + Duration::from_secs(31)).expect("not paced by the busy one"));
    }

    #[test]
    fn core_waits_for_the_app_at_most_the_bound() {
        fn slow(_: Action, _: &str) -> TetherAgentAnswer {
            std::thread::sleep(Duration::from_millis(400));
            TetherAgentAnswer { state: "installed".into(), ..Default::default() }
        }
        fn quick(a: Action, viewer: &str) -> TetherAgentAnswer {
            TetherAgentAnswer { state: format!("{a:?}"), detail: viewer.to_owned(), ..Default::default() }
        }
        let rt = hbb_common::tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_time().build().unwrap();
        // No handler (TetherQS, or an app that does not manage the agent): not supported.
        assert_eq!(rt.block_on(core_agent::host_answer(Action::Install, None, String::new(), Duration::from_secs(1))).state, "not-supported");
        let a = rt.block_on(core_agent::host_answer(Action::Remove, Some(quick), "Sam at the desk".into(), Duration::from_secs(1)));
        // The handler gets the name the viewer gave (shown on the host as not verified).
        assert_eq!((a.state.as_str(), a.detail.as_str()), ("Remove", "Sam at the desk"));
        let t = std::time::Instant::now();
        let late = rt.block_on(core_agent::host_answer(Action::Install, Some(slow), String::new(), Duration::from_millis(50)));
        assert!(t.elapsed() < Duration::from_millis(300));
        // The action goes on: "running", and a later status shows the outcome.
        assert_eq!(late.state, "running");
        assert_eq!(late.detail, core_agent::RUNNING);
    }

    /// What `serve` did: answers sent, connection-window notices, audit posts (with whether the
    /// connection's request guard was still held when the post ran).
    #[derive(Default)]
    struct Served {
        sent: Vec<TetherAgentAnswer>,
        cm: Vec<(String, String)>,
        posted: Vec<(serde_json::Value, bool)>,
    }

    fn serve(action: Action, handler: core_agent::Handler, post: impl Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = hbb_common::ResultType<()>>>>, post_wait: Duration) -> (Served, bool) {
        serve_within(action, handler, Duration::from_secs(5), post, post_wait)
    }

    fn serve_within(action: Action, handler: core_agent::Handler, wait: Duration, post: impl Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = hbb_common::ResultType<()>>>>, post_wait: Duration) -> (Served, bool) {
        use librustdesk::ui::agent::{message, misc};
        let gate = Rc::new(RefCell::new(InFlight::default()));
        let guard = gate.borrow_mut().start(action, Instant::now()).unwrap();
        let out = Rc::new(RefCell::new(Served::default()));
        let (o1, o2, o3, g3) = (out.clone(), out.clone(), out.clone(), gate.clone());
        rt().block_on(core_agent::serve(
            action,
            Some(handler),
            "Sam".to_owned(),
            wait,
            guard,
            move |m| {
                let Some(message::Union::Misc(misc)) = m.union else { panic!("not misc") };
                let Some(misc::Union::TetherAgentAnswer(a)) = misc.union else { panic!("not an answer") };
                o1.borrow_mut().sent.push(a);
            },
            move |a, outcome| o2.borrow_mut().cm.push((a.to_owned(), outcome)),
            move |info| {
                let held = g3.borrow_mut().start(Action::Status, Instant::now()).is_none();
                o3.borrow_mut().posted.push((info, held));
                post()
            },
            post_wait,
        ));
        let free = gate.borrow_mut().start(Action::Status, Instant::now() + Duration::from_secs(60)).is_some();
        let served = Rc::try_unwrap(out).ok().unwrap().into_inner();
        (served, free)
    }

    fn ok_post() -> std::pin::Pin<Box<dyn std::future::Future<Output = hbb_common::ResultType<()>>>> {
        Box::pin(async { Ok(()) })
    }

    #[test]
    fn core_serve_audits_only_what_the_app_did() {
        fn refused(_: Action, _: &str) -> TetherAgentAnswer {
            TetherAgentAnswer { state: "needs-service".into(), ..Default::default() }
        }
        fn busy(_: Action, _: &str) -> TetherAgentAnswer {
            TetherAgentAnswer { state: "busy".into(), ..Default::default() }
        }
        fn installed(_: Action, _: &str) -> TetherAgentAnswer {
            TetherAgentAnswer { state: "installed".into(), ..Default::default() }
        }
        // Refused or busy: answered, but no audit record and no line in the connection window.
        for h in [refused as core_agent::Handler, busy] {
            let (s, free) = serve(Action::Install, h, ok_post, Duration::from_secs(1));
            assert_eq!(s.sent.len(), 1);
            assert!(s.posted.is_empty() && s.cm.is_empty(), "{:?}", s.sent[0].state);
            assert!(free);
        }
        // Status changes nothing.
        let (s, _) = serve(Action::Status, installed, ok_post, Duration::from_secs(1));
        assert!(s.posted.is_empty() && s.cm.is_empty());
        // Acted: one post, made while the connection's guard is still held, freed afterwards.
        let (s, free) = serve(Action::Install, installed, ok_post, Duration::from_secs(1));
        assert_eq!(s.sent[0].state, "installed");
        assert_eq!(s.sent[0].action.enum_value(), Ok(Action::Install));
        assert_eq!(s.posted.len(), 1);
        assert_eq!(s.posted[0].0, serde_json::json!({"agent": "install", "result": "installed"}));
        assert!(s.posted[0].1, "guard held during the post");
        assert!(free, "guard freed after the post");
        // The host's connection window is told the finished outcome.
        assert_eq!(s.cm, [("install".to_owned(), "installed".to_owned())]);
    }

    #[test]
    fn core_serve_post_is_one_bounded_attempt() {
        fn removed(_: Action, _: &str) -> TetherAgentAnswer {
            TetherAgentAnswer { state: "removed-by-tether".into(), ..Default::default() }
        }
        // A relay that never answers: given up after the bound, the guard is freed.
        let t = Instant::now();
        let (s, free) = serve(Action::Remove, removed, || Box::pin(std::future::pending()), Duration::from_millis(50));
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
        assert_eq!(s.posted.len(), 1);
        assert!(free);
        // A failed post is only logged (no retry: the closure is called once).
        let (s, free) = serve(Action::Remove, removed, || Box::pin(async { hbb_common::bail!("status 502") }), Duration::from_secs(1));
        assert_eq!(s.posted.len(), 1);
        assert_eq!(s.sent[0].state, "removed-by-tether");
        assert!(free);
        assert!(core_agent::AUDIT_POST_WAIT <= Duration::from_secs(10));
    }

    /// `serve` with an app that takes 600 ms against a 50 ms wait; at 300 ms (answered `running`, the
    /// app still at work) a probe notes (answers sent, posts made, a new request accepted) and,
    /// with `close`, ends the session (the connection's channel is gone).
    fn serve_long(close: bool) -> (Vec<TetherAgentAnswer>, Vec<(String, String)>, Vec<serde_json::Value>, (Vec<String>, usize, bool)) {
        use librustdesk::ui::agent::{message, misc, Message};
        fn answer_of(m: Message) -> TetherAgentAnswer {
            let Some(message::Union::Misc(misc)) = m.union else { panic!("not misc") };
            let Some(misc::Union::TetherAgentAnswer(a)) = misc.union else { panic!("not an answer") };
            a
        }
        fn slow(_: Action, _: &str) -> TetherAgentAnswer {
            std::thread::sleep(Duration::from_millis(600));
            TetherAgentAnswer { state: "installed".into(), ..Default::default() }
        }
        let gate = RefCell::new(InFlight::default());
        let guard = gate.borrow_mut().start(Action::Install, Instant::now()).unwrap();
        let (cm, posted) = (RefCell::new(vec![]), RefCell::new(vec![]));
        let (tx, rx) = std::sync::mpsc::channel();
        let rx = RefCell::new(Some(rx));
        let between = rt().block_on(async {
            let served = core_agent::serve(
                Action::Install,
                Some(slow),
                "Sam".to_owned(),
                Duration::from_millis(50),
                guard,
                |m| drop(tx.send(m)),
                |a, o| cm.borrow_mut().push((a.to_owned(), o)),
                |info| {
                    posted.borrow_mut().push(info);
                    async { Ok(()) }
                },
                Duration::from_secs(1),
            );
            let probe = async {
                hbb_common::tokio::time::sleep(Duration::from_millis(300)).await;
                let sent = rx.borrow().as_ref().unwrap().try_iter().map(|m| answer_of(m).state).collect::<Vec<_>>();
                let free = gate.borrow_mut().start(Action::Status, Instant::now()).is_some();
                if close {
                    rx.borrow_mut().take();
                }
                (sent, posted.borrow().len(), free)
            };
            hbb_common::tokio::join!(served, probe).1
        });
        let sent = rx.into_inner().map(|rx| rx.try_iter().collect::<Vec<_>>()).unwrap_or_default();
        (sent.into_iter().map(answer_of).collect(), cm.into_inner(), posted.into_inner(), between)
    }

    #[test]
    fn core_serve_follows_a_long_action_to_its_outcome() {
        let (sent, cm, posted, between) = serve_long(false);
        // At the wait: `running` answered and audited, the guard freed while the app still works.
        assert_eq!(between, (vec!["running".to_owned()], 1, true), "(answers, posts, a new request accepted) in between");
        assert!(sent.is_empty(), "nothing more is sent to the viewer");
        // Then exactly one more record, with the outcome, and the connection window told once.
        assert_eq!(posted, [serde_json::json!({"agent": "install", "result": "running"}), serde_json::json!({"agent": "install", "result": "installed"})]);
        assert_eq!(cm, [("install".to_owned(), "installed".to_owned())]);
    }

    #[test]
    fn core_serve_audits_the_outcome_after_the_session_ended() {
        let (_, cm, posted, between) = serve_long(true);
        assert_eq!(between.1, 1);
        assert_eq!(posted.len(), 2, "{posted:?}");
        assert_eq!(posted[1], serde_json::json!({"agent": "install", "result": "installed"}));
        assert_eq!(cm.len(), 1);
    }

    #[test]
    fn core_cm_event_only_for_a_known_action_and_outcome() {
        use librustdesk::ui::cm::agent_event;
        assert_eq!(agent_event(3, "install", "installed"), Some(serde_json::json!({"id": 3, "action": "install", "outcome": "installed"})));
        assert!(agent_event(3, "remove", "removed-by-tether").is_some());
        for (a, o) in [("status", "installed"), ("upgrade", "installed"), ("install", "running"), ("install", "busy"), ("remove", "Click here: https://x"), ("install", "")] {
            assert_eq!(agent_event(3, a, o), None, "{a} {o}");
        }
    }

    #[test]
    fn core_detail_is_bounded_and_clean_on_both_sides() {
        use librustdesk::ui::agent::{message, misc};
        let long = format!("a\u{1b}[2J\r\nb{}", "\u{e9}".repeat(600));
        let hostile = TetherAgentAnswer { state: format!("installed\n{}", "x".repeat(100)), detail: long, ..Default::default() };
        // Host: the answer builder cleans before sending.
        let m = core_agent::answer_message(Action::Install, hostile.clone());
        let Some(message::Union::Misc(misc)) = m.union else { panic!() };
        let Some(misc::Union::TetherAgentAnswer(a)) = misc.union else { panic!() };
        assert_eq!(a.detail.chars().count(), core_agent::DETAIL_MAX);
        assert_eq!(core_agent::DETAIL_MAX, 512);
        assert!(a.detail.starts_with("a[2Jb\u{e9}"), "{}", &a.detail[..10]);
        assert!(!a.detail.chars().any(char::is_control));
        assert!(!a.state.contains('\n') && a.state.chars().count() <= 64);
        // Viewer: an answer from the peer is cleaned again on receipt.
        let w = core_agent::Waiter::default();
        let (tx, mut rx) = hbb_common::tokio::sync::oneshot::channel();
        *w.lock().unwrap() = Some((Action::Install, tx));
        core_agent::deliver(&w, TetherAgentAnswer { action: Action::Install.into(), ..hostile });
        let got = rx.try_recv().unwrap();
        assert_eq!(got.detail.chars().count(), 512);
        assert!(!got.detail.chars().any(char::is_control));
    }

    #[test]
    fn viewer_drops_an_answer_to_another_action() {
        let w = core_agent::Waiter::default();
        let (tx, mut rx) = hbb_common::tokio::sync::oneshot::channel();
        *w.lock().unwrap() = Some((Action::Install, tx));
        // A late status answer while the install is awaited, and an answer without the echo (a
        // host before it: read as status): dropped, the install is still awaited.
        core_agent::deliver(&w, TetherAgentAnswer { action: Action::Status.into(), ..wire("not-installed", "") });
        core_agent::deliver(&w, wire("not-installed", ""));
        assert!(rx.try_recv().is_err());
        assert!(w.lock().unwrap().is_some());
        core_agent::deliver(&w, TetherAgentAnswer { action: Action::Install.into(), ..wire("installed", "ok") });
        assert_eq!(rx.try_recv().unwrap().state, "installed");
        // Nobody waiting: dropped.
        core_agent::deliver(&w, TetherAgentAnswer { action: Action::Install.into(), ..wire("installed", "ok") });
        assert!(w.lock().unwrap().is_none());
    }

    #[test]
    fn core_audit_record_names_only_action_and_outcome() {
        assert_eq!(core_agent::audit_info(Action::Status, &wire("installed", "")), None);
        // Nothing done, nothing recorded (a session member cannot fill the relay with refusals).
        for s in ["not-supported", "needs-service", "not-allowed", "busy"] {
            assert_eq!(core_agent::audit_info(Action::Install, &wire(s, "")), None, "{s}");
            assert_eq!(core_agent::audit_info(Action::Remove, &wire(s, "")), None, "{s}");
        }
        assert!(core_agent::audit_info(Action::Install, &wire("running", "")).is_some());
        let a = TetherAgentAnswer { state: "installed".into(), version: "0.2.0".into(), installed_by: "session".into(), detail: "C:\\secret\\path token=abc".into(), at: 5, ..Default::default() };
        let info = core_agent::audit_info(Action::Install, &a).unwrap();
        assert_eq!(info, serde_json::json!({"agent": "install", "result": "installed"}));
        let info = core_agent::audit_info(Action::Remove, &wire("failed", "x")).unwrap();
        assert_eq!(info, serde_json::json!({"agent": "remove", "result": "failed"}));
        let r = core_agent::audit_record("123456789".into(), "dXVpZA==".into(), 4, &info);
        let keys: Vec<&str> = r.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["conn_id", "id", "info", "nonce", "typ", "uuid"]);
        assert_eq!(r["typ"], core_agent::AUDIT_TYP);
        assert_eq!(core_agent::AUDIT_TYP, 30);
        assert_eq!(r["conn_id"], 4);
        assert_eq!(r["info"], r#"{"agent":"remove","result":"failed"}"#);
        assert_eq!(r["nonce"].as_str().unwrap().len(), 36);
    }

    #[test]
    fn session_policy_get_refuses_secrets() {
        assert!(super::session_policy_get("recording.upload-token".into()).is_err());
        assert!(super::session_policy_get("recording.upload-target".into()).is_err());
    }
}

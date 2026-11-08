use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::OnceLock;

use serde::Serialize;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_deep_link::DeepLinkExt;

use crate::{cli, commands, policy, sink, update};

static APP: OnceLock<AppHandle> = OnceLock::new();

/// Main process: a host session is active (the `--cm` process said so): the connection window's
/// HWND on Windows, else 1; 0 without one. While set, a click on the app icon is for the
/// connection window only and the main window stays minimized (docs/quick-support.md, macos.md).
static HOST_SESSION: AtomicIsize = AtomicIsize::new(0);
/// Windows: the main window was hidden for the host session (minimized and off the taskbar).
#[cfg(windows)]
static MAIN_HIDDEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Broadcast an event to every window (no-op before the UI is up or in headless roles).
pub fn emit<S: Serialize + Clone>(event: &str, payload: S) {
    if let Some(app) = APP.get() {
        let _ = app.emit(event, payload);
    }
}

/// What this process was started for.
enum Mode {
    Main,
    /// Host side: incoming-connection manager, spawned by the `--server` process.
    Cm,
    /// Viewer opened from the command line or a `tether://` link (`cli.rs`).
    Connect { c: cli::Connect, password: Option<String> },
}

fn mode(args: &[String]) -> Mode {
    if args.first().map(String::as_str) == Some("--cm") {
        return Mode::Cm;
    }
    match cli::take_connect() {
        Some((c, password)) => Mode::Connect { c, password },
        None => Mode::Main,
    }
}

pub fn run(args: Vec<String>) {
    let mode = mode(&args);
    #[cfg(target_os = "macos")]
    let is_main = matches!(mode, Mode::Main);
    let mut ctx = tauri::generate_context!();
    // The single-instance lock is keyed by the identifier: dev profiles must not share it.
    if let Ok(p) = std::env::var("TETHER_PROFILE") {
        if !p.is_empty() {
            ctx.config_mut().identifier = format!("{}.{p}", ctx.config().identifier);
        }
    }
    // A session request while Tether runs opens in that process instead of a second one.
    #[cfg(any(windows, target_os = "macos"))]
    if let Mode::Connect { c, password } = &mode {
        if forward_to_running(&ctx.config().identifier, &cli::encode_forward(c, password.as_deref())) {
            return;
        }
    }
    // Started by the main window's process: the connection window gets no Dock icon of its own,
    // only while the main Dock icon can bring it back (docs/macos.md).
    #[cfg(target_os = "macos")]
    let mut cm_show = match mode {
        Mode::Cm if std::env::var_os(crate::MAIN_GUI_ENV).is_some() => listen_show_cm(&ctx.config().identifier),
        _ => None,
    };
    #[cfg(target_os = "macos")]
    let cm_accessory = cm_show.is_some();
    // Its own taskbar button, which toggles it natively: grouped with the main window, a click
    // only showed thumbnails and never minimized (docs/quick-support.md).
    #[cfg(windows)]
    if matches!(mode, Mode::Cm) {
        set_app_id(&format!("{}.cm", ctx.config().identifier));
    }
    let mut builder = tauri::Builder::default();
    if matches!(mode, Mode::Main) {
        // First plugin: a second start exits before doing anything else.
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| second_instance(app, &argv)));
    }
    #[allow(unused_mut)]
    let mut app = builder
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(commands::handler())
        .setup(move |app| {
            sink::install(app.handle().clone());
            let _ = APP.set(app.handle().clone());
            match &mode {
                Mode::Main => {
                    #[cfg(target_os = "macos")]
                    librustdesk::platform::macos::disable_app_nap();
                    librustdesk::ui::api::start_status_sync();
                    main_window(app.handle())?;
                    tray(app.handle())?;
                    policy::start(true);
                    update::start();
                    // Retry recording uploads left from earlier sessions.
                    std::thread::spawn(crate::recording::after_session);
                    // macOS delivers tether:// links to the running app; Windows starts a new
                    // process with the link as argument (cli.rs).
                    let handle = app.handle().clone();
                    app.deep_link().on_open_url(move |ev| {
                        for url in ev.urls() {
                            match cli::parse_uri(url.as_str()).and_then(|c| policy::check_outgoing().map(|_| c)) {
                                Ok(c) => {
                                    let _ = open_session(&handle, &c.peer, &c.conn);
                                }
                                Err(e) => hbb_common::log::warn!("link refused: {e}"),
                            }
                        }
                    });
                    // Debug builds only: lets automated tests set a known permanent password.
                    #[cfg(debug_assertions)]
                    if let Ok(pw) = std::env::var("TETHER_DEV_PASSWORD") {
                        std::thread::spawn(move || {
                            std::thread::sleep(std::time::Duration::from_secs(3));
                            let ok = librustdesk::ui::api::set_permanent_password(pw);
                            hbb_common::log::info!("dev password set: {ok}");
                        });
                    }
                }
                Mode::Cm => {
                    librustdesk::ui::cm::start();
                    cm_window(app.handle())?;
                    // Windows hides the main window for the whole session instead (host_session_started).
                    #[cfg(target_os = "macos")]
                    follow_cm_minimize(app.handle().clone());
                    #[cfg(target_os = "macos")]
                    if let Some(l) = cm_show.take() {
                        let app = app.handle().clone();
                        std::thread::spawn(move || {
                            for mut s in l.incoming().flatten() {
                                // Answer first: the restore animation outlasts the main process's
                                // 500 ms wait, which then also showed the main window.
                                let session = host_session_active();
                                let _ = std::io::Write::write_all(&mut s, if session { b"1" } else { b"0" });
                                drop(s);
                                if let Some(w) = app.get_webview_window("cm") {
                                    dock_click(&w, session);
                                }
                            }
                        });
                    }
                    refuse_incoming_while_update_required();
                }
                Mode::Connect { c, password } => {
                    let key = open_session_with(app.handle(), c)?;
                    if let Some(pw) = password {
                        sink::get().map(|s| s.set_pending_password(&key, pw.clone()));
                    }
                }
            }
            Ok(())
        })
        .build(ctx)
        .expect("failed to build tauri app");
    // Before the event loop starts, so the Dock icon never appears.
    #[cfg(target_os = "macos")]
    if cm_accessory {
        app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    }
    app.run(move |app, ev| match ev {
        tauri::RunEvent::ExitRequested { api, code, .. } => {
            // Keep running in the tray while the main window is closed; the local server
            // must stay up so this machine stays reachable.
            if code.is_none() && app.get_webview_window("main").is_none() && has_tray(app) {
                api.prevent_exit();
            }
        }
        // Dock icon clicked: during a session it toggles only the connection window, which has no
        // Dock icon of its own; otherwise the main window if nothing is shown (docs/macos.md).
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Reopen { has_visible_windows, .. } if is_main => {
            // The cm's answer can miss the timeout: during a session the main window stays as is.
            if !cm_dock_click(&app.config().identifier) && !has_visible_windows && HOST_SESSION.load(Ordering::Relaxed) == 0 {
                let _ = main_window(app);
            }
        }
        _ => {}
    });
}

/// Runs in the first instance when Tether is started again: a forwarded session request opens a
/// session window, anything else brings the main window to the front.
fn second_instance(app: &AppHandle, argv: &[String]) {
    match argv.get(1).map(String::as_str) {
        Some(MINIMIZE_MAIN_ARG) => return host_session_started(app, argv.get(2)),
        Some(SESSION_ENDED_ARG) => return host_session_ended(app),
        Some(cli::FORWARD_ARG) => {}
        _ => {
            #[cfg(windows)]
            if toggle_cm() {
                return;
            }
            let _ = main_window(app);
            return;
        }
    }
    let req = argv.get(2).ok_or_else(|| "empty request".to_owned()).and_then(|h| cli::decode_forward(h));
    let (c, pw) = match req.and_then(|r| policy::check_outgoing().map(|_| r)) {
        Ok(r) => r,
        Err(e) => return hbb_common::log::warn!("forwarded session refused: {e}"),
    };
    // Called from a window message on the main thread; building a webview there can deadlock.
    let app = app.clone();
    std::thread::spawn(move || match open_session_with(&app, &c) {
        Ok(key) => {
            if let (Some(pw), Some(s)) = (pw, sink::get()) {
                s.set_pending_password(&key, pw);
            }
        }
        Err(e) => hbb_common::log::warn!("forwarded session: {e}"),
    });
}

/// Hands a session request to a running Tether (its single-instance window), like the plugin does
/// for a plain second start, but with our payload instead of this process's argv. Linux: none, so
/// `--connect` runs in its own process.
#[cfg(any(windows, target_os = "macos"))]
fn forward_to_running(identifier: &str, payload: &str) -> bool {
    send_to_running(identifier, &format!("{}|{payload}", cli::FORWARD_ARG))
}

/// argv[1] the `--cm` process sends to the main window's process when a host session starts
/// (argv[2]: the connection window's HWND on Windows) and when the last one has ended.
const MINIMIZE_MAIN_ARG: &str = "--minimize-main";
const SESSION_ENDED_ARG: &str = "--host-session-ended";

/// A host session started (`sink.rs`): the main window's process, if running, minimizes it.
pub fn minimize_main(app: &AppHandle) {
    #[cfg(windows)]
    let cm = app.get_webview_window("cm").and_then(|w| w.hwnd().ok()).map_or(0, |h| h.0 as isize);
    #[cfg(not(windows))]
    let cm = 1;
    tell_main(app, &[MINIMIZE_MAIN_ARG, &cm.to_string()]);
}

/// The last host session ended (`sink.rs`): the main window's process stops guarding it.
pub fn host_session_ended_in_cm(app: &AppHandle) {
    tell_main(app, &[SESSION_ENDED_ARG]);
}

fn tell_main(app: &AppHandle, args: &[&str]) {
    let identifier = &app.config().identifier;
    #[cfg(any(windows, target_os = "macos"))]
    send_to_running(identifier, &args.join("|"));
    let _ = (identifier, args);
}

/// Main process, `--minimize-main`: the main window gets out of the way for the whole session.
fn host_session_started(app: &AppHandle, cm: Option<&String>) {
    let cm = cm.and_then(|c| c.parse::<isize>().ok()).filter(|&c| c != 0).unwrap_or(1);
    HOST_SESSION.store(cm, Ordering::Relaxed);
    // Not a hidden one: minimize would show it.
    if let Some(w) = app.get_webview_window("main").filter(|w| w.is_visible().unwrap_or(false)) {
        hbb_common::log::info!("host session started: minimizing the main window");
        let _ = w.minimize();
        // Windows: off the taskbar too, so the only Tether button is the connection window's and
        // no click on the app icon can bring the main window up (docs/quick-support.md).
        #[cfg(windows)]
        {
            let _ = w.hide();
            MAIN_HIDDEN.store(true, Ordering::Relaxed);
        }
    }
}

/// Main process, `--host-session-ended`: on Windows the main window is back on the taskbar,
/// still minimized.
fn host_session_ended(app: &AppHandle) {
    hbb_common::log::info!("host session ended");
    HOST_SESSION.store(0, Ordering::Relaxed);
    #[cfg(windows)]
    if MAIN_HIDDEN.swap(false, Ordering::Relaxed) {
        if let Some(h) = app.get_webview_window("main").and_then(|w| w.hwnd().ok()) {
            use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_SHOWMINNOACTIVE};
            unsafe { ShowWindow(h.0 as _, SW_SHOWMINNOACTIVE) };
        }
    }
    let _ = app;
}

/// Windows, Tether started again (pinned taskbar icon, Start menu) during a host session: toggles
/// the connection window like its own taskbar button; the main window stays minimized. False
/// without a live connection window.
#[cfg(windows)]
fn toggle_cm() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindow, SetForegroundWindow, ShowWindow, SW_MINIMIZE, SW_RESTORE};
    let h = HOST_SESSION.load(Ordering::Relaxed) as windows_sys::Win32::Foundation::HWND;
    if h.is_null() {
        return false;
    }
    // The cm process ended without telling (crash): back to normal.
    if unsafe { IsWindow(h) } == 0 {
        HOST_SESSION.store(0, Ordering::Relaxed);
        return false;
    }
    hbb_common::log::info!("started again during a host session: toggling the connection window");
    unsafe {
        if IsIconic(h) != 0 {
            ShowWindow(h, SW_RESTORE);
            SetForegroundWindow(h);
        } else {
            ShowWindow(h, SW_MINIMIZE);
        }
    }
    true
}

/// Its own taskbar button for this process's windows (an explicit AppUserModelID).
#[cfg(windows)]
fn set_app_id(id: &str) {
    #[link(name = "shell32")]
    extern "system" {
        fn SetCurrentProcessExplicitAppUserModelID(id: *const u16) -> i32;
    }
    let w: Vec<u16> = id.encode_utf16().chain(Some(0)).collect();
    let hr = unsafe { SetCurrentProcessExplicitAppUserModelID(w.as_ptr()) };
    hbb_common::log::info!("cm app id {id}: {hr:#x}");
}

/// argv for the running instance's single-instance window, `|`-separated.
#[cfg(windows)]
fn send_to_running(identifier: &str, args: &str) -> bool {
    use windows_sys::Win32::{
        System::DataExchange::COPYDATASTRUCT,
        UI::WindowsAndMessaging::{FindWindowW, SendMessageW, WM_COPYDATA},
    };
    let w = |s: String| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (class, name) = (w(format!("{identifier}-sic")), w(format!("{identifier}-siw")));
    let hwnd = unsafe { FindWindowW(class.as_ptr(), name.as_ptr()) };
    if hwnd.is_null() {
        return false;
    }
    // The plugin's wire format (tauri-plugin-single-instance windows.rs): "cwd|argv0|argv1…\0",
    // tagged 1542; its window procedure answers 1.
    let data = format!("|tether|{args}\0");
    let cds = COPYDATASTRUCT { dwData: 1542, cbData: data.len() as u32, lpData: data.as_ptr() as _ };
    unsafe { SendMessageW(hwnd, WM_COPYDATA, 0, &cds as *const _ as isize) == 1 }
}

/// macOS: the single-instance plugin's socket and wire format (its macos.rs): "cwd\0\0argv0\0argv1…".
/// /tmp is shared by all users: only a socket this user owns gets the request (it can carry a password).
#[cfg(target_os = "macos")]
fn send_to_running(identifier: &str, args: &str) -> bool {
    use std::os::unix::fs::MetadataExt;
    let path = format!("/tmp/{}_si.sock", identifier.replace(['.', '-'], "_"));
    if !std::fs::symlink_metadata(&path).is_ok_and(|m| m.uid() == unsafe { hbb_common::libc::getuid() }) {
        return false;
    }
    std::os::unix::net::UnixStream::connect(&path)
        .and_then(|mut s| std::io::Write::write_all(&mut s, format!("\0\0tether\0{}", args.replace('|', "\0")).as_bytes()))
        .is_ok()
}

fn has_tray(app: &AppHandle) -> bool {
    app.tray_by_id("tray").is_some()
}

fn base<'a>(app: &'a AppHandle, label: &str, route: &str) -> tauri::Result<WebviewWindowBuilder<'a, tauri::Wry, AppHandle>> {
    let b = WebviewWindowBuilder::new(app, label, WebviewUrl::App(format!("index.html#{route}").into()))
        .visible(false) // shown by the page after first paint, avoids a white flash
        // macOS: the first click into an inactive window clicks too (Accept needed two clicks)
        .accept_first_mouse(true)
        .title(crate::APP_NAME);
    // Separate profiles must not share a webview browser process or its storage.
    let b = match std::env::var("TETHER_PROFILE") {
        Ok(p) if !p.is_empty() => match app.path().app_local_data_dir() {
            Ok(dir) => b.data_directory(dir.join(format!("profile-{p}"))),
            Err(_) => b,
        },
        _ => b,
    };
    // macOS keeps native traffic lights over our own title bar; elsewhere we draw the controls.
    #[cfg(target_os = "macos")]
    let b = b.title_bar_style(tauri::TitleBarStyle::Overlay).hidden_title(true);
    #[cfg(not(target_os = "macos"))]
    let b = b.decorations(false);
    Ok(b)
}

pub fn main_window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    // Opened on purpose (tray) during a host session: left as it is when the session ends.
    #[cfg(windows)]
    MAIN_HIDDEN.store(false, Ordering::Relaxed);
    if let Some(w) = app.get_webview_window("main") {
        w.unminimize()?;
        w.show()?;
        w.set_focus()?;
        return Ok(w);
    }
    base(app, "main", "/")?
        .inner_size(1000.0, 660.0)
        .min_inner_size(820.0, 560.0)
        .center()
        .build()
}

/// Advanced mode is the main window's home view (docs/advanced.md): it needs a wider window, so
/// switching to it grows the window (never past the screen); switching back only relaxes the minimum.
pub fn fit_main(app: &AppHandle, advanced: bool) -> tauri::Result<()> {
    let Some(w) = app.get_webview_window("main") else { return Ok(()) };
    if !advanced {
        return w.set_min_size(Some(tauri::LogicalSize::new(820.0, 560.0)));
    }
    let s = w.scale_factor()?;
    let cur = w.inner_size()?.to_logical::<f64>(s);
    let (mut want_w, mut want_h) = (cur.width.max(1280.0), cur.height.max(800.0));
    if let Ok(Some(m)) = w.current_monitor() {
        let screen = m.size().to_logical::<f64>(m.scale_factor());
        want_w = want_w.min(screen.width - 40.0);
        want_h = want_h.min(screen.height - 80.0);
    }
    w.set_min_size(Some(tauri::LogicalSize::new(1000.0_f64.min(want_w), 600.0_f64.min(want_h))))?;
    if want_w > cur.width + 1.0 || want_h > cur.height + 1.0 {
        w.set_size(tauri::LogicalSize::new(want_w, want_h))?;
        w.center()?;
    }
    Ok(())
}

fn cm_window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    let w = base(app, "cm", "/cm")?
        .inner_size(380.0, 560.0)
        .resizable(false)
        .always_on_top(true)
        .build()?;
    if let Ok(Some(m)) = w.current_monitor() {
        let s = m.scale_factor();
        let size = m.size().to_logical::<f64>(s);
        let pos = m.position().to_logical::<f64>(s);
        // Never past the screen's top-left: on a short screen (1366x768 at 150 % = 512 logical
        // pixels high) the bottom-right anchor put the title bar and its controls off-screen.
        let _ = w.set_position(tauri::LogicalPosition::new(
            (pos.x + size.width - 380.0 - 16.0).max(pos.x),
            (pos.y + size.height - 560.0 - 56.0).max(pos.y),
        ));
    }
    Ok(w)
}

/// Where the `--cm` process listens for "show your window" (any connection means show).
#[cfg(target_os = "macos")]
fn cm_show_socket(identifier: &str) -> std::path::PathBuf {
    // $TMPDIR is per user on macOS; the identifier carries the dev profile.
    std::env::temp_dir().join(format!("{identifier}-cm.sock"))
}

/// Main process: tells the `--cm` process the Dock icon was clicked. True if that toggled the
/// connection window of an active session (then the main window stays as it is).
#[cfg(target_os = "macos")]
fn cm_dock_click(identifier: &str) -> bool {
    let Ok(mut s) = std::os::unix::net::UnixStream::connect(cm_show_socket(identifier)) else {
        return false;
    };
    let _ = s.set_read_timeout(Some(std::time::Duration::from_millis(500)));
    let mut r = [0u8];
    std::io::Read::read_exact(&mut s, &mut r).is_ok() && r[0] == b'1'
}

/// `--cm` process, macOS: each time the connection window becomes minimized during a session
/// (at the start, or by the user: yellow button, Dock click) the main window minimizes with it.
/// Up to 0.5.19 only the session start told the main window, once, so a connection window
/// minimized later left the main window on screen. There is no minimize event on macOS: polled.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn follow_cm_minimize(app: AppHandle) {
    std::thread::spawn(move || {
        let mut f = MinimizeFollower::default();
        while let Some(w) = app.get_webview_window("cm") {
            if f.step(w.is_minimized().unwrap_or(false), host_session_active()) {
                hbb_common::log::info!("connection window minimized: minimizing the main window");
                minimize_main(&app);
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
    });
}

/// Fires once per change of the connection window to minimized while a session is active.
#[derive(Default)]
struct MinimizeFollower {
    was: bool,
}

impl MinimizeFollower {
    fn step(&mut self, minimized: bool, session: bool) -> bool {
        let fire = minimized && !self.was && session;
        self.was = minimized;
        fire
    }
}

/// `--cm` process: an authorized client is connected.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn host_session_active() -> bool {
    sink::get().is_some_and(|s| s.cm_clients().iter().any(|c| c["authorized"] == true))
}

/// `--cm` process, Dock icon clicked: during a session the connection window toggles like a
/// single-window app (minimize when shown, else bring back); without one it is just shown.
#[cfg(target_os = "macos")]
fn dock_click(w: &WebviewWindow, session: bool) {
    let shown = w.is_visible().unwrap_or(false) && !w.is_minimized().unwrap_or(false);
    hbb_common::log::info!("dock click: session {session}, cm shown {shown}");
    if session && shown {
        let _ = w.minimize();
    } else if w.is_minimized().unwrap_or(false) {
        // show() right after unminimize() interrupts the restore animation on macOS and leaves an
        // empty, half-transparent window: restore only, focus once the animation is done.
        let _ = w.unminimize();
        let w = w.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(400));
            let _ = w.set_focus();
            hbb_common::log::info!(
                "dock click restored: visible {:?}, minimized {:?}, pos {:?}",
                w.is_visible(),
                w.is_minimized(),
                w.outer_position()
            );
        });
    } else {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// The Accessory-policy `--cm` window has no Dock icon: a click on the main Dock icon connects
/// here (`dock_click`). None if not listening (then keep the Dock icon).
#[cfg(target_os = "macos")]
fn listen_show_cm(identifier: &str) -> Option<std::os::unix::net::UnixListener> {
    let path = cm_show_socket(identifier);
    let _ = std::fs::remove_file(&path);
    std::os::unix::net::UnixListener::bind(&path)
        .map_err(|e| hbb_common::log::warn!("cm show socket {path:?}: {e}"))
        .ok()
}

/// Incoming sessions are refused while this version must be updated first: every connection the
/// CM sees is closed. Policy and update state come from the caches (`policy::init`, `update.rs`).
fn refuse_incoming_while_update_required() {
    if !update::blocked() {
        return;
    }
    hbb_common::log::warn!("update required: refusing incoming sessions");
    std::thread::spawn(|| loop {
        for c in sink::get().map(|s| s.cm_clients()).unwrap_or_default() {
            if let Some(id) = c.get("id").and_then(|v| v.as_i64()) {
                librustdesk::ui::cm::close(id as i32);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    });
}

pub fn open_session(app: &AppHandle, peer: &str, conn: &str) -> tauri::Result<String> {
    let c = cli::Connect { peer: peer.to_owned(), conn: conn.to_owned(), ..Default::default() };
    open_session_with(app, &c)
}

/// Open session windows: key, peer, connection type.
static WINDOWS: std::sync::Mutex<Vec<(String, String, String)>> = std::sync::Mutex::new(Vec::new());

/// Main process: a session runs (an outgoing session or file transfer window, or a host session
/// the `--cm` process reported). A forced update waits for this to clear (D7).
pub fn busy() -> bool {
    !open_sessions().is_empty() || HOST_SESSION.load(Ordering::Relaxed) != 0
}

/// Open session windows as `(key, peer, conn)`, oldest first (Advanced mode's session list).
pub fn open_sessions() -> Vec<(String, String, String)> {
    WINDOWS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Bring a session window to the front; false if it is gone.
pub fn focus_session(app: &AppHandle, key: &str) -> bool {
    match app.get_webview_window(&sink::session_label(key)) {
        Some(w) => {
            let _ = w.unminimize();
            let _ = w.show();
            w.set_focus().is_ok()
        }
        None => false,
    }
}

/// `view_only`/`relay` are passed to the session page as `&view_only=1&relay=1`.
/// One window per peer and connection type: a second request for it (a double-clicked Recent card,
/// Enter pressed twice, a link opened twice) brings that window to the front instead of starting a
/// second session, which left one window on "Connecting…" in front of the working one.
pub fn open_session_with(app: &AppHandle, c: &cli::Connect) -> tauri::Result<String> {
    let (peer, conn) = (&c.peer, &c.conn);
    let key = {
        let mut open = WINDOWS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((key, _, _)) = open.iter().find(|(_, p, c)| p == peer && c == conn) {
            // No window yet: the first request is still building it.
            if let Some(w) = app.get_webview_window(&sink::session_label(key)) {
                hbb_common::log::info!("session to {peer} ({conn}) already open: focusing it");
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
            return Ok(key.clone());
        }
        let key = uuid::Uuid::new_v4().simple().to_string();
        // Listed before the window exists, so two requests at once cannot both miss it. Not held
        // while building: that waits for the main thread, whose Destroyed handler takes this lock.
        open.push((key.clone(), peer.clone(), conn.clone()));
        key
    };
    let forget = |k: &str| WINDOWS.lock().unwrap_or_else(|e| e.into_inner()).retain(|(x, _, _)| x != k);
    let label = sink::session_label(&key);
    let mut route = format!("/session/{key}?peer={peer}&conn={conn}");
    if c.view_only {
        route.push_str("&view_only=1");
    }
    if c.relay {
        route.push_str("&relay=1");
    }
    let w = match base(app, &label, &route).and_then(|b| {
        b.fullscreen(c.fullscreen)
            .inner_size(1280.0, 800.0)
            .min_inner_size(640.0, 400.0)
            .center()
            .build()
    }) {
        Ok(w) => w,
        Err(e) => {
            forget(&key);
            return Err(e);
        }
    };
    let k = key.clone();
    w.on_window_event(move |e| {
        if let WindowEvent::Destroyed = e {
            forget(&k);
            if let Some(s) = sink::get() {
                s.detach_video(&k);
            }
            crate::keys::forget(&k);
            librustdesk::ui::session_close(&k);
        } else if let WindowEvent::Focused(f) = e {
            if *f {
                librustdesk::ui::session_focus(&k);
            }
            crate::keys::focus(&k, *f);
        }
    });
    Ok(key)
}

fn tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Tether", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;
    let mut b = TrayIconBuilder::with_id("tray")
        .tooltip(crate::APP_NAME)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| match ev.id().as_ref() {
            "open" => {
                let _ = main_window(app);
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                ..
            } = ev
            {
                let _ = main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        b = b.icon(icon.clone());
    }
    b.build(app)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::MinimizeFollower;

    #[test]
    fn main_window_follows_every_minimize_of_the_connection_window() {
        let mut f = MinimizeFollower::default();
        // No session: the user minimizing the window decides nothing for the main window.
        assert!(!f.step(true, false));
        assert!(!f.step(false, false));
        // Session starts, the connection window minimizes: once, not on every poll.
        assert!(!f.step(false, true));
        assert!(f.step(true, true));
        assert!(!f.step(true, true));
        // Restored (Dock click, chat) and minimized again by the user: again.
        assert!(!f.step(false, true));
        assert!(f.step(true, true));
        // Already minimized when the session starts: no change, nothing to do.
        let mut g = MinimizeFollower::default();
        assert!(!g.step(true, false));
        assert!(!g.step(true, true));
    }
}

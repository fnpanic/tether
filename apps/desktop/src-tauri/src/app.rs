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
    let mut ctx = tauri::generate_context!();
    // The single-instance lock is keyed by the identifier: dev profiles must not share it.
    if let Ok(p) = std::env::var("TETHER_PROFILE") {
        if !p.is_empty() {
            ctx.config_mut().identifier = format!("{}.{p}", ctx.config().identifier);
        }
    }
    // A session request while Tether runs opens in that process instead of a second one.
    #[cfg(windows)]
    if let Mode::Connect { c, password } = &mode {
        if forward_to_running(&ctx.config().identifier, &cli::encode_forward(c, password.as_deref())) {
            return;
        }
    }
    let mut builder = tauri::Builder::default();
    if matches!(mode, Mode::Main) {
        // First plugin: a second start exits before doing anything else.
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| second_instance(app, &argv)));
    }
    builder
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_deep_link::init())
        .invoke_handler(commands::handler())
        .setup(move |app| {
            sink::install(app.handle().clone());
            let _ = APP.set(app.handle().clone());
            match &mode {
                Mode::Main => {
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
        .expect("failed to build tauri app")
        .run(|app, ev| {
            if let tauri::RunEvent::ExitRequested { api, code, .. } = ev {
                // Keep running in the tray while the main window is closed; the local server
                // must stay up so this machine stays reachable.
                if code.is_none() && app.get_webview_window("main").is_none() && has_tray(app) {
                    api.prevent_exit();
                }
            }
        });
}

/// Runs in the first instance when Tether is started again: a forwarded session request opens a
/// session window, anything else brings the main window to the front.
fn second_instance(app: &AppHandle, argv: &[String]) {
    if argv.get(1).map(String::as_str) != Some(cli::FORWARD_ARG) {
        let _ = main_window(app);
        return;
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
/// for a plain second start, but with our payload instead of this process's argv.
/// shortcut: Windows only; elsewhere `--connect` still runs in its own process, as before.
#[cfg(windows)]
fn forward_to_running(identifier: &str, payload: &str) -> bool {
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
    let data = format!("|tether|{}|{payload}\0", cli::FORWARD_ARG);
    let cds = COPYDATASTRUCT { dwData: 1542, cbData: data.len() as u32, lpData: data.as_ptr() as _ };
    unsafe { SendMessageW(hwnd, WM_COPYDATA, 0, &cds as *const _ as isize) == 1 }
}

fn has_tray(app: &AppHandle) -> bool {
    app.tray_by_id("tray").is_some()
}

fn base<'a>(app: &'a AppHandle, label: &str, route: &str) -> tauri::Result<WebviewWindowBuilder<'a, tauri::Wry, AppHandle>> {
    let b = WebviewWindowBuilder::new(app, label, WebviewUrl::App(format!("index.html#{route}").into()))
        .visible(false) // shown by the page after first paint, avoids a white flash
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
        let _ = w.set_position(tauri::LogicalPosition::new(
            pos.x + size.width - 380.0 - 16.0,
            pos.y + size.height - 560.0 - 56.0,
        ));
    }
    Ok(w)
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

/// `view_only`/`relay` are passed to the session page as `&view_only=1&relay=1`.
pub fn open_session_with(app: &AppHandle, c: &cli::Connect) -> tauri::Result<String> {
    let (peer, conn) = (&c.peer, &c.conn);
    let key = uuid::Uuid::new_v4().simple().to_string();
    let label = sink::session_label(&key);
    let mut route = format!("/session/{key}?peer={peer}&conn={conn}");
    if c.view_only {
        route.push_str("&view_only=1");
    }
    if c.relay {
        route.push_str("&relay=1");
    }
    let w = base(app, &label, &route)?
        .fullscreen(c.fullscreen)
        .inner_size(1280.0, 800.0)
        .min_inner_size(640.0, 400.0)
        .center()
        .build()?;
    let k = key.clone();
    w.on_window_event(move |e| {
        if let WindowEvent::Destroyed = e {
            if let Some(s) = sink::get() {
                s.detach_video(&k);
            }
            librustdesk::ui::session_close(&k);
        } else if let WindowEvent::Focused(true) = e {
            librustdesk::ui::session_focus(&k);
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

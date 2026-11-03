use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

use crate::{commands, sink};

/// What this process was started for.
enum Mode {
    Main,
    /// Host side: incoming-connection manager, spawned by the `--server` process.
    Cm,
    /// Viewer opened directly from argv (`--connect <id> [password]`).
    Connect { peer: String, conn: String, password: Option<String> },
}

fn mode(args: &[String]) -> Mode {
    match args.first().map(String::as_str) {
        Some("--cm") => Mode::Cm,
        Some(c @ ("--connect" | "--file-transfer" | "--view-camera" | "--terminal"))
            if args.len() > 1 =>
        {
            Mode::Connect {
                peer: args[1].clone(),
                conn: c.trim_start_matches("--").replace("connect", "default"),
                password: args.get(2).filter(|p| !p.starts_with("--")).cloned(),
            }
        }
        _ => Mode::Main,
    }
}

pub fn run(args: Vec<String>) {
    let mode = mode(&args);
    tauri::Builder::default()
        .invoke_handler(commands::handler())
        .setup(move |app| {
            sink::install(app.handle().clone());
            match &mode {
                Mode::Main => {
                    librustdesk::ui::api::start_status_sync();
                    main_window(app.handle())?;
                    tray(app.handle())?;
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
                }
                Mode::Connect { peer, conn, password } => {
                    let key = open_session(app.handle(), peer, conn)?;
                    if let Some(pw) = password {
                        sink::get().map(|s| s.set_pending_password(&key, pw.clone()));
                    }
                }
            }
            Ok(())
        })
        .build(tauri::generate_context!())
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

pub fn open_session(app: &AppHandle, peer: &str, conn: &str) -> tauri::Result<String> {
    let key = uuid::Uuid::new_v4().simple().to_string();
    let label = sink::session_label(&key);
    let route = format!("/session/{key}?peer={peer}&conn={conn}");
    let w = base(app, &label, &route)?
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

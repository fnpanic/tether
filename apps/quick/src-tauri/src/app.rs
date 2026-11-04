//! One window. Unlike the technician app, the connection manager runs in this process too: the
//! core's server connects to the `_cm` IPC listener started here, so no `--cm` process (and no
//! second window) is spawned. If the core does start `--cm` (listener not up), that process shows
//! the same window.

use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::{commands, policy, sink, update};

pub fn run(args: Vec<String>) {
    let cm_process = args.first().map(String::as_str) == Some("--cm");
    let mut builder = tauri::Builder::default();
    // Started again while running: show the first window (one ID, one password). A `--cm`
    // process must keep running, the server talks to it.
    if !cm_process {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
        }));
    }
    builder
        .invoke_handler(commands::handler())
        .setup(move |app| {
            sink::install(app.handle().clone());
            crate::custom::log_status();
            if !cm_process {
                // `--cm` already started the status sync inside core_main.
                librustdesk::ui::api::start_status_sync();
                let h = app.handle().clone();
                policy::spawn(move || {
                    let _ = h.emit_to("main", "cm", serde_json::json!({ "name": "policy", "payload": null }));
                });
                update::spawn();
            }
            librustdesk::ui::cm::set_record_consent_text(policy::current().get("recording.consent-text").to_owned());
            librustdesk::ui::cm::start();
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title(policy::current().get("customer.title"))
                .inner_size(420.0, 560.0)
                .resizable(false)
                .maximizable(false)
                .decorations(false)
                .visible(false) // shown by the page after first paint, avoids a white flash
                .center()
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        // Closing the window ends support: the in-process server stops with the process.
        .expect("failed to start TetherQS");
}

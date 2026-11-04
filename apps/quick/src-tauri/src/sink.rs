//! Receives connection-manager events from the core and forwards them to the single window.
//! Also enforces, in the backend, what the UI only displays: required updates and a disabled
//! `connect.incoming` refuse sessions, `customer.close-after-session` quits.

use std::sync::{Arc, Mutex, OnceLock};

use librustdesk::ui::{cm, UiSink};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};

use crate::{policy, update};

#[derive(Serialize, Clone)]
struct Evt<'a> {
    name: &'a str,
    payload: Value,
}

pub struct TauriSink {
    app: AppHandle,
    /// Current incoming connections, so the window can render them after a reload.
    clients: Mutex<Vec<Value>>,
}

static SINK: OnceLock<Arc<TauriSink>> = OnceLock::new();

pub fn install(app: AppHandle) {
    let sink = SINK.get_or_init(|| Arc::new(TauriSink { app, clients: Default::default() })).clone();
    librustdesk::ui::set_sink(sink);
}

pub fn clients() -> Vec<Value> {
    SINK.get().map(|s| s.clients.lock().unwrap().clone()).unwrap_or_default()
}

/// The core does not re-announce a client after `authorize`/`switch_permission`: mirror it here.
pub fn set_field(id: i32, key: &str, value: bool) {
    if let Some(s) = SINK.get() {
        for c in s.clients.lock().unwrap().iter_mut().filter(|c| id_of(c) == Some(id)) {
            c[key] = Value::Bool(value);
        }
    }
}

pub fn has_session() -> bool {
    !clients().is_empty()
}

fn id_of(v: &Value) -> Option<i32> {
    v.get("id").and_then(Value::as_i64).map(|i| i as i32)
}

impl UiSink for TauriSink {
    fn session_event(&self, _session: &str, _name: &str, _payload: Value) {}

    fn video(&self, _session: &str, _packet: Vec<u8>) {}

    fn cm_event(&self, name: &str, payload: Value) {
        let id = id_of(&payload);
        if name == "add_connection" {
            if let Some(id) = id {
                let reason = if update::blocked() {
                    Some("update-required")
                } else if !policy::current().on("connect.incoming") {
                    Some("incoming-disabled")
                } else {
                    None
                };
                if let Some(reason) = reason {
                    hbb_common::log::warn!("refusing incoming connection {id}: {reason}");
                    // Not from inside the core's callback (it may hold its client lock).
                    std::thread::spawn(move || cm::close(id));
                    let _ = self.app.emit_to("main", "cm", Evt { name: "refused", payload: serde_json::json!({ "reason": reason }) });
                    return;
                }
            }
        }
        let mut ended_session = false;
        let remaining = {
            let mut c = self.clients.lock().unwrap();
            match name {
                "add_connection" => {
                    c.retain(|x| id_of(x) != id);
                    c.push(payload.clone());
                }
                "remove_connection" => {
                    ended_session = c.iter().any(|x| id_of(x) == id && x["authorized"] == true);
                    c.retain(|x| id_of(x) != id);
                }
                _ => {}
            }
            c.len()
        };
        let _ = self.app.emit_to("main", "cm", Evt { name, payload });
        if name == "add_connection" {
            if let Some(w) = self.app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                dock(&w);
                let _ = w.set_focus();
            }
        }
        if name == "chat" {
            // Flash the taskbar button until the customer looks; never take the focus mid-session.
            if let Some(w) = self.app.get_webview_window("main") {
                let r = w.request_user_attention(Some(tauri::UserAttentionType::Informational));
                hbb_common::log::info!("chat message for connection {id:?}: attention requested ({r:?})");
            }
        }
        if name == "remove_connection" && remaining == 0 {
            if let Some(w) = self.app.get_webview_window("main") {
                let _ = w.center();
            }
        }
        if ended_session && remaining == 0 && policy::current().on("customer.close-after-session") {
            let app = self.app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(2));
                if !has_session() {
                    app.exit(0);
                }
            });
        }
    }
}

/// During a request and session the window always sits in the same place: bottom right of the
/// screen it is on, out of the way of what the technician works on.
fn dock(w: &tauri::WebviewWindow) {
    let (Ok(Some(m)), Ok(size)) = (w.current_monitor(), w.outer_size()) else { return };
    let a = m.work_area();
    let margin = (16.0 * m.scale_factor()) as i32;
    let x = a.position.x + a.size.width as i32 - size.width as i32 - margin;
    let y = a.position.y + a.size.height as i32 - size.height as i32 - margin;
    let _ = w.set_position(tauri::PhysicalPosition::new(x.max(a.position.x), y.max(a.position.y)));
}

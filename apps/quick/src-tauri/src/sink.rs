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

/// The core does not re-announce a client after `authorize`: mirror it, so the window minimizes
/// like for a client that logged in with the password.
pub fn authorized(id: i32) {
    let Some(s) = SINK.get() else { return };
    let client = s.clients.lock().unwrap().iter().find(|c| id_of(c) == Some(id)).cloned();
    if let Some(mut client) = client {
        client["authorized"] = Value::Bool(true);
        s.cm_event("add_connection", client);
    }
}

/// The core does not re-announce a client after `switch_permission`: mirror it here.
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
        let (remaining, action) = {
            let mut c = self.clients.lock().unwrap();
            let was_authorized = c.iter().any(|x| id_of(x) == id && x["authorized"] == true);
            let action = window_action(name, was_authorized, &payload);
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
            (c.len(), action)
        };
        let _ = self.app.emit_to("main", "cm", Evt { name, payload });
        if let Some(w) = self.app.get_webview_window("main") {
            if name == "add_connection" {
                dock(&w);
            }
            apply(&w, action);
            if name == "chat" {
                hbb_common::log::info!("chat message for connection {id:?}: window restored, attention requested");
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

/// What the window does on a CM event. Same rules as the technician app's CM window
/// (`apps/desktop/src-tauri/src/sink.rs`).
#[derive(Debug, PartialEq, Clone, Copy)]
enum WindowAction {
    None,
    /// A session just started: out of the way, but on the taskbar/Dock (not hidden, not in a tray).
    Minimize,
    /// Something needs an answer (new request, recording consent): restore and focus.
    Prompt,
    /// A chat message: restore and flash the taskbar, never take the focus mid-session.
    Notify,
}

fn window_action(name: &str, was_authorized: bool, payload: &Value) -> WindowAction {
    match name {
        // Once per session: a re-announce of an authorized client does nothing.
        "add_connection" if payload["authorized"] == true => {
            if was_authorized { WindowAction::None } else { WindowAction::Minimize }
        }
        "add_connection" | "record_consent_request" => WindowAction::Prompt,
        "chat" => WindowAction::Notify,
        _ => WindowAction::None,
    }
}

fn apply(w: &tauri::WebviewWindow, action: WindowAction) {
    match action {
        WindowAction::None => {}
        WindowAction::Minimize => {
            let _ = w.show();
            let _ = w.minimize();
        }
        WindowAction::Prompt | WindowAction::Notify => {
            // Only when minimized: restoring a visible window would activate it.
            if w.is_minimized().unwrap_or(false) {
                let _ = w.unminimize();
            }
            let _ = w.show();
            if action == WindowAction::Prompt {
                let _ = w.set_focus();
            } else {
                let _ = w.request_user_attention(Some(tauri::UserAttentionType::Informational));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn window_follows_the_session() {
        let pending = json!({ "id": 1, "authorized": false });
        let active = json!({ "id": 1, "authorized": true });
        assert_eq!(window_action("add_connection", false, &pending), WindowAction::Prompt);
        assert_eq!(window_action("add_connection", false, &active), WindowAction::Minimize);
        assert_eq!(window_action("add_connection", true, &active), WindowAction::None);
        assert_eq!(window_action("record_consent_request", true, &json!({ "conn_id": 1 })), WindowAction::Prompt);
        assert_eq!(window_action("chat", true, &json!({ "id": 1, "text": "hi" })), WindowAction::Notify);
        assert_eq!(window_action("remove_connection", true, &json!({ "id": 1 })), WindowAction::None);
    }
}

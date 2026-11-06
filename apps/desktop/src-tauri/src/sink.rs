use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

use librustdesk::ui::UiSink;
use serde::Serialize;
use serde_json::Value;
use tauri::{
    ipc::{Channel, InvokeResponseBody},
    AppHandle, Emitter, EventTarget, Manager,
};

#[derive(Serialize, Clone)]
struct Evt<'a> {
    name: &'a str,
    payload: Value,
}

pub struct TauriSink {
    app: AppHandle,
    video: Mutex<HashMap<String, Channel<InvokeResponseBody>>>,
    /// Current incoming connections, so a CM window that loads after the first event still
    /// shows the pending request.
    cm_clients: Mutex<Vec<Value>>,
    /// Passwords given on the command line, consumed once by `session_start`.
    pending_passwords: Mutex<HashMap<String, String>>,
    /// New session key -> live session key whose login it reuses, consumed by `session_start`.
    pending_logins: Mutex<HashMap<String, String>>,
    /// Session key -> when the last natively decoded (RGBA) picture was sent and not yet drawn.
    rgba_in_flight: Mutex<HashMap<String, Instant>>,
}

static SINK: OnceLock<Arc<TauriSink>> = OnceLock::new();

pub fn install(app: AppHandle) -> Arc<TauriSink> {
    let sink = SINK
        .get_or_init(|| {
            Arc::new(TauriSink {
                app,
                video: Default::default(),
                cm_clients: Default::default(),
                pending_passwords: Default::default(),
                pending_logins: Default::default(),
                rgba_in_flight: Default::default(),
            })
        })
        .clone();
    librustdesk::ui::set_sink(sink.clone());
    sink
}

pub fn get() -> Option<Arc<TauriSink>> {
    SINK.get().cloned()
}

pub fn session_label(key: &str) -> String {
    format!("s-{key}")
}

impl TauriSink {
    pub fn attach_video(&self, key: &str, ch: Channel<InvokeResponseBody>) {
        self.video.lock().unwrap().insert(key.to_owned(), ch);
    }

    pub fn detach_video(&self, key: &str) {
        self.video.lock().unwrap().remove(key);
        self.rgba_in_flight.lock().unwrap().remove(key);
    }

    /// The webview drew the last RGBA picture of this session: the next one may be sent.
    pub fn rgba_drawn(&self, key: &str) {
        self.rgba_in_flight.lock().unwrap().remove(key);
    }

    pub fn set_pending_password(&self, key: &str, pw: String) {
        self.pending_passwords.lock().unwrap().insert(key.to_owned(), pw);
    }

    pub fn take_pending_password(&self, key: &str) -> Option<String> {
        self.pending_passwords.lock().unwrap().remove(key)
    }

    pub fn set_pending_login(&self, key: &str, from: String) {
        self.pending_logins.lock().unwrap().insert(key.to_owned(), from);
    }

    pub fn take_pending_login(&self, key: &str) -> Option<String> {
        self.pending_logins.lock().unwrap().remove(key)
    }

    pub fn cm_clients(&self) -> Vec<Value> {
        self.cm_clients.lock().unwrap().clone()
    }
}

impl UiSink for TauriSink {
    fn session_event(&self, session: &str, name: &str, payload: Value) {
        let _ = self.app.emit_to(
            EventTarget::webview_window(session_label(session)),
            "session",
            Evt { name, payload },
        );
    }

    fn video(&self, session: &str, packet: Vec<u8>) {
        // A native picture is several MB and the IPC channel has no backpressure: send it only
        // when the webview drew the previous one, else the queue (and the delay) grows without
        // bound. Encoded packets always pass: the decoder needs every one of them.
        if packet.first() == Some(&librustdesk::ui::PKT_RGBA) {
            let mut f = self.rgba_in_flight.lock().unwrap();
            let now = Instant::now();
            if !admit_rgba(f.get(session).copied(), now) {
                return;
            }
            f.insert(session.to_owned(), now);
        }
        let ch = self.video.lock().unwrap().get(session).cloned();
        if let Some(ch) = ch {
            let _ = ch.send(InvokeResponseBody::Raw(packet));
        }
    }

    fn cm_event(&self, name: &str, payload: Value) {
        let id = payload.get("id").cloned();
        let (action, ended) = {
            let mut c = self.cm_clients.lock().unwrap();
            let was_authorized = c.iter().any(|x| x.get("id") == id.as_ref() && x["authorized"] == true);
            let action = window_action(name, was_authorized, &payload);
            match name {
                "add_connection" => {
                    c.retain(|x| x.get("id") != id.as_ref());
                    c.push(payload.clone());
                }
                "remove_connection" => c.retain(|x| x.get("id") != id.as_ref()),
                _ => {}
            }
            (action, was_authorized && name == "remove_connection" && !c.iter().any(|x| x["authorized"] == true))
        };
        let _ = self
            .app
            .emit_to(EventTarget::webview_window("cm"), "cm", Evt { name, payload });
        if action != WindowAction::None {
            hbb_common::log::info!("cm {name}: {action:?}");
        }
        if let Some(w) = self.app.get_webview_window("cm") {
            apply(&w, action);
        }
        // Like the connection window, the main window (another process) gets out of the way.
        if action == WindowAction::Minimize {
            crate::app::minimize_main(&self.app);
        }
        if ended {
            crate::app::host_session_ended_in_cm(&self.app);
        }
    }
}

impl TauriSink {
    /// The core does not re-announce a client after `authorize`: mirror it, so the window shows
    /// the session and minimizes like for a client that logged in with the password.
    pub fn cm_authorized(&self, id: i32) {
        let client = {
            let c = self.cm_clients.lock().unwrap();
            c.iter().find(|x| x["id"] == id).cloned()
        };
        if let Some(mut client) = client {
            client["authorized"] = Value::Bool(true);
            self.cm_event("add_connection", client);
        }
    }
}

/// What the host window does on a CM event. Same rules as TetherQS (`apps/quick/src-tauri/src/sink.rs`).
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum WindowAction {
    None,
    /// A session just started: out of the way, but on the taskbar (not hidden, not in the tray).
    Minimize,
    /// Something needs an answer (new request, recording consent): restore and focus.
    Prompt,
    /// A chat message: restore and flash the taskbar, never take the focus mid-session.
    Notify,
}

pub fn window_action(name: &str, was_authorized: bool, payload: &Value) -> WindowAction {
    match name {
        // Once per session: a re-announce of an authorized client (privacy-mode rollback) does nothing.
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
            // Already minimized (a second session): show() would restore it on macOS and the
            // minimize after it gets lost in the restore animation.
            if w.is_minimized().unwrap_or(false) {
                return;
            }
            if !w.is_visible().unwrap_or(true) {
                let _ = w.show();
            }
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

/// Longest wait for the webview's "drawn" before the next native picture is sent anyway (a lost
/// acknowledgement must not freeze the picture).
const RGBA_ACK_TIMEOUT: Duration = Duration::from_secs(1);

fn admit_rgba(in_flight_since: Option<Instant>, now: Instant) -> bool {
    in_flight_since.map_or(true, |t| now.duration_since(t) >= RGBA_ACK_TIMEOUT)
}

#[cfg(test)]
mod rgba_tests {
    use super::*;

    #[test]
    fn native_pictures_wait_for_the_previous_one() {
        let t = Instant::now();
        assert!(admit_rgba(None, t));
        assert!(!admit_rgba(Some(t), t + Duration::from_millis(200)));
        assert!(admit_rgba(Some(t), t + RGBA_ACK_TIMEOUT));
    }
}

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

use librustdesk::ui::UiSink;
use serde::Serialize;
use serde_json::Value;
use tauri::{
    ipc::{Channel, InvokeResponseBody},
    AppHandle, Emitter, EventTarget,
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
    }

    pub fn set_pending_password(&self, key: &str, pw: String) {
        self.pending_passwords.lock().unwrap().insert(key.to_owned(), pw);
    }

    pub fn take_pending_password(&self, key: &str) -> Option<String> {
        self.pending_passwords.lock().unwrap().remove(key)
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
        let ch = self.video.lock().unwrap().get(session).cloned();
        if let Some(ch) = ch {
            let _ = ch.send(InvokeResponseBody::Raw(packet));
        }
    }

    fn cm_event(&self, name: &str, payload: Value) {
        let id = payload.get("id").cloned();
        {
            let mut c = self.cm_clients.lock().unwrap();
            match name {
                "add_connection" => {
                    c.retain(|x| x.get("id") != id.as_ref());
                    c.push(payload.clone());
                }
                "remove_connection" => c.retain(|x| x.get("id") != id.as_ref()),
                _ => {}
            }
        }
        let _ = self
            .app
            .emit_to(EventTarget::webview_window("cm"), "cm", Evt { name, payload });
    }
}

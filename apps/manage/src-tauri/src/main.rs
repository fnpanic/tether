#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod cdn;
mod diag;
mod keys;
mod live;
mod private;
mod release;

use keys::Keys;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Read};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};
use tether_trust::custom::{self, CustomProfile};
use tether_trust::policy::{self, Bundle, Effective, Layer};
use tether_trust::serverkeys::{KeyList, ServerKey};
use tether_trust::team::Roster;
use tether_trust::{envelope, SigningKey, VerifyingKey};

const DEFAULT_SERVER: &str = "https://tether-desk.duckdns.org";
/// The admin keys the apps are built with (comma-separated, as for Tether and TetherQS). After an
/// admin key rotation the relays still serve release indexes signed with the old key; Manage builds
/// on them only when it was built with that key listed here (docs/management.md).
const TRUSTED_ADMIN_KEYS: Option<&str> = option_env!("TETHER_ADMIN_PUB_KEY");

struct App {
    dir: PathBuf,
    keys: Mutex<Option<Keys>>,
    diag: Mutex<Option<Vec<u8>>>,
}

type R<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

impl App {
    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
    /// Write via temp file + rename so a crash never leaves a half-written key or policy file.
    fn write(&self, name: &str, data: &[u8]) -> R<()> {
        let p = self.path(name);
        fs::create_dir_all(p.parent().unwrap()).map_err(err)?;
        let tmp = p.with_extension("tmp");
        fs::write(&tmp, data).map_err(err)?;
        fs::rename(&tmp, &p).map_err(err)
    }
    fn settings(&self) -> Value {
        let mut s: Value = fs::read(self.path("settings.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(json!({}));
        if s.get("server").and_then(Value::as_str).map_or(true, str::is_empty) {
            s["server"] = json!(DEFAULT_SERVER);
        }
        s
    }
    fn server(&self) -> String {
        self.servers().remove(0)
    }
    /// Every relay node, in order (setting "server" may be a comma-separated list; see
    /// docs/relay-ha.md). The first one is used for reads.
    fn servers(&self) -> Vec<String> {
        let list = self.settings()["server"].as_str().unwrap_or(DEFAULT_SERVER).to_owned();
        let v: Vec<String> = list.split(',').map(|s| s.trim().trim_end_matches('/').to_owned()).filter(|s| !s.is_empty()).collect();
        if v.is_empty() { vec![DEFAULT_SERVER.to_owned()] } else { v }
    }
    fn admin_key(&self) -> R<SigningKey> {
        self.keys.lock().unwrap().as_ref().map(|k| k.admin.clone()).ok_or_else(|| "Unlock the keys first.".to_owned())
    }
    /// Reads give up after [`api::QUICK`] (a relay that never answers must not hold up a view);
    /// writes and diagnostics downloads (a zip may be large) keep the long client timeout.
    async fn call_on(&self, server: &str, method: &str, path: &str, body: Vec<u8>, ct: &str) -> R<Vec<u8>> {
        if method.eq_ignore_ascii_case("GET") && !path.starts_with("/api/admin/diag/") {
            return api::admin_quick(server, &self.admin_key()?, path).await;
        }
        api::admin(server, &self.admin_key()?, method, path, body, ct).await
    }
    /// Reads ask every node at once and take the first in list order that answered; writes go to
    /// every node (each keeps a full copy of policy, releases, download page and tickets), so any
    /// node can serve clients alone.
    async fn call(&self, method: &str, path: &str, body: Vec<u8>, ct: &str) -> R<Vec<u8>> {
        let servers = self.servers();
        if method.eq_ignore_ascii_case("GET") {
            let got = futures_util::future::join_all(servers.iter().map(|sv| self.call_on(sv, method, path, Vec::new(), ct))).await;
            let mut last = String::new();
            for (sv, r) in servers.iter().zip(got) {
                match r {
                    Ok(out) => return Ok(out),
                    Err(e) => last = if servers.len() > 1 { format!("{sv}: {e}") } else { e },
                }
            }
            return Err(last);
        }
        let mut first: Option<Vec<u8>> = None;
        let mut body = body;
        let mut failed = Vec::new();
        for sv in &servers {
            match self.call_on(sv, method, path, body.clone(), ct).await {
                Ok(out) => {
                    // The same diagnostics ticket code on every node: the first issues it.
                    if path == "/api/admin/diag-ticket" && first.is_none() {
                        if let (Ok(t), Ok(mut req)) = (serde_json::from_slice::<Value>(&out), serde_json::from_slice::<Value>(&body)) {
                            if let Some(id) = t.get("ticket") {
                                req["ticket"] = id.clone();
                                body = serde_json::to_vec(&req).unwrap();
                            }
                        }
                    }
                    first.get_or_insert(out);
                }
                Err(e) => failed.push(format!("{sv}: {e}")),
            }
        }
        match (first, failed.is_empty()) {
            (Some(out), true) => Ok(out),
            (Some(_), false) => Err(format!("Saved on some relays, but not on: {}", failed.join("; "))),
            (None, _) => Err(failed.join("; ")),
        }
    }
    async fn call_json(&self, method: &str, path: &str, body: Option<Value>) -> R<Value> {
        let body = body.map(|b| serde_json::to_vec(&b).unwrap()).unwrap_or_default();
        let out = self.call(method, path, body, "application/json").await?;
        if out.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&out).map_err(|_| format!("The server sent an unexpected answer: {}", String::from_utf8_lossy(&out).chars().take(200).collect::<String>()))
    }
    /// The same write to every relay, with one result per relay (the publish sheets show them).
    /// All relays at once: one slow relay does not delay the others.
    async fn fan_out(&self, method: &str, path: &str, body: Vec<u8>, ct: &str) -> Vec<RelayResult> {
        let servers = self.servers();
        let got = futures_util::future::join_all(servers.iter().map(|sv| self.call_on(sv, method, path, body.clone(), ct))).await;
        servers.iter().zip(got).map(|(sv, r)| RelayResult::of(sv, r.map(|_| Value::Null))).collect()
    }
    /// CDN credentials and base URL, when the CDN is on and the keys are unlocked.
    fn cdn_config(&self) -> Option<(cdn::Creds, String)> {
        let s = self.settings();
        if !s["cdn"]["enabled"].as_bool().unwrap_or(false) {
            return None;
        }
        let k = self.keys.lock().unwrap();
        let c = k.as_ref()?.cdn.clone()?;
        let worker = s["cdn"]["worker"].as_str().unwrap_or(cdn::DEFAULT_WORKER).to_owned();
        Some((cdn::Creds { account_id: c.account_id, token: c.token, worker }, s["cdn"]["base"].as_str().unwrap_or(cdn::DEFAULT_BASE).to_owned()))
    }
    fn published(&self) -> BTreeMap<u64, PathBuf> {
        fs::read_dir(self.path("policy/published"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| Some((e.path().file_stem()?.to_str()?.parse().ok()?, e.path())))
            .collect()
    }
    fn custom_local(&self, id: &str) -> Option<CustomLocal> {
        fs::read(self.path(&format!("custom/{id}.json"))).ok().and_then(|b| serde_json::from_slice(&b).ok())
    }
}

/// Outcome of one operation on one relay.
#[derive(Serialize, Debug)]
struct RelayResult {
    server: String,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    detail: Value,
}

impl RelayResult {
    fn of(server: &str, r: R<Value>) -> Self {
        match r {
            Ok(detail) => RelayResult { server: server.to_owned(), ok: true, error: None, detail },
            Err(e) => RelayResult { server: server.to_owned(), ok: false, error: Some(e), detail: Value::Null },
        }
    }
}

/// Error text when a write reached no relay at all; otherwise the per-relay results go to the UI.
fn none_ok(results: &[RelayResult]) -> Option<String> {
    (!results.iter().any(|r| r.ok)).then(|| results.iter().map(|r| format!("{}: {}", r.server, r.error.as_deref().unwrap_or("failed"))).collect::<Vec<_>>().join("; "))
}

// ---- keys and settings ----

#[tauri::command]
fn state(app: State<App>) -> Value {
    let k = app.keys.lock().unwrap();
    json!({
        "exists": app.path("keys.json").exists(),
        "unlocked": k.is_some(),
        "admin_pub": k.as_ref().map(|k| k.admin_pub()),
        "updater_pub": k.as_ref().map(|k| k.updater_pub()),
        "server_keys": k.as_ref().map(|k| k.server_pubs()),
        "settings": app.settings(),
        "servers": app.servers(),
        "data_dir": app.dir,
    })
}

#[tauri::command]
fn keys_create(app: State<App>, passphrase: String) -> R<()> {
    if app.path("keys.json").exists() {
        return Err("Keys already exist. Unlock them or import a backup.".into());
    }
    let k = Keys::generate()?;
    app.write("keys.json", &k.seal(&passphrase)?)?;
    *app.keys.lock().unwrap() = Some(k);
    Ok(())
}

#[tauri::command]
fn keys_unlock(app: State<App>, passphrase: String) -> R<()> {
    let data = fs::read(app.path("keys.json")).map_err(|_| "No keys yet.".to_owned())?;
    *app.keys.lock().unwrap() = Some(Keys::open(&data, &passphrase)?);
    Ok(())
}

#[tauri::command]
fn keys_lock(app: State<App>) {
    *app.keys.lock().unwrap() = None;
}

/// The backup is the sealed key file itself: still encrypted with the passphrase.
#[tauri::command]
fn keys_export(app: State<App>, path: String) -> R<()> {
    fs::copy(app.path("keys.json"), path).map(|_| ()).map_err(err)
}

#[tauri::command]
fn keys_import(app: State<App>, path: String, passphrase: String) -> R<()> {
    let data = fs::read(path).map_err(err)?;
    let k = Keys::open(&data, &passphrase)?;
    app.write("keys.json", &data)?;
    *app.keys.lock().unwrap() = Some(k);
    Ok(())
}

#[tauri::command]
fn settings_set(app: State<App>, settings: Value) -> R<()> {
    let server = settings.get("server").and_then(Value::as_str).unwrap_or("");
    for sv in server.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if !sv.starts_with("https://") {
            return Err(format!("Every server address must start with https:// ({sv})"));
        }
    }
    app.write("settings.json", &serde_json::to_vec_pretty(&settings).unwrap())
}

/// Generic signed admin call for the simple JSON endpoints (status, devices, releases, diag…).
#[tauri::command]
async fn admin(app: State<'_, App>, method: String, path: String, body: Option<Value>) -> R<Value> {
    if !path.starts_with("/api/admin/") {
        return Err("only admin endpoints".into());
    }
    app.call_json(&method, &path, body).await
}

/// Health of every configured relay (`GET /api/admin/status` on each).
#[tauri::command]
async fn relays_check(app: State<'_, App>) -> R<Vec<RelayResult>> {
    let key = app.admin_key()?;
    let servers = app.servers();
    let got = futures_util::future::join_all(servers.iter().map(|sv| api::admin_quick(sv, &key, "/api/admin/status"))).await;
    Ok(servers.iter().zip(got).map(|(sv, r)| RelayResult::of(sv, r.map(|b| serde_json::from_slice(&b).unwrap_or(Value::Null)))).collect())
}

/// Devices from every relay, merged by uuid (clients heartbeat to whichever relay they use).
#[tauri::command]
async fn devices_all(app: State<'_, App>) -> R<Value> {
    let (mut lists, mut failed) = (Vec::new(), Vec::new());
    let servers = app.servers();
    let got = futures_util::future::join_all(servers.iter().map(|sv| app.call_on(sv, "GET", "/api/admin/devices", Vec::new(), "application/json"))).await;
    for (sv, r) in servers.iter().zip(got) {
        match r {
            Ok(b) => lists.push(serde_json::from_slice::<Vec<Value>>(&b).map_err(|_| format!("{sv}: unexpected device list"))?),
            Err(e) => failed.push(format!("{sv}: {e}")),
        }
    }
    if lists.is_empty() {
        return Err(failed.join("; "));
    }
    Ok(json!({ "devices": live::merge_devices(lists), "failed": failed }))
}

/// Session audit records of every relay (`GET /api/admin/audit`), each tagged with `relay`. Hosts
/// post to the first relay of their build, so the lists differ; the page merges and dedupes them.
#[tauri::command]
async fn audit_all(app: State<'_, App>, since: u64, limit: u32) -> R<Value> {
    let (mut events, mut failed, mut answered) = (Vec::new(), Vec::new(), 0);
    let servers = app.servers();
    let path = format!("/api/admin/audit?since={since}&limit={limit}");
    let got = futures_util::future::join_all(servers.iter().map(|sv| app.call_on(sv, "GET", &path, Vec::new(), "application/json"))).await;
    for (sv, r) in servers.iter().zip(got) {
        match r {
            Ok(b) => {
                answered += 1;
                for mut e in serde_json::from_slice::<Vec<Value>>(&b).map_err(|_| format!("{sv}: unexpected audit list"))? {
                    e["relay"] = json!(sv);
                    events.push(e);
                }
            }
            Err(e) => failed.push(format!("{sv}: {e}")),
        }
    }
    if answered == 0 {
        return Err(failed.join("; "));
    }
    Ok(json!({ "events": events, "failed": failed }))
}

/// Writes a text export (Sessions CSV) to the path the save dialog returned.
#[tauri::command]
fn text_save(path: String, text: String) -> R<()> {
    fs::write(path, text).map_err(err)
}

// ---- server key rotation (docs/key-rotation.md) ----

/// A new server key pair, sealed into `keys.json` right away. Needs the passphrase again because
/// only the decrypted keys are kept in memory.
#[tauri::command]
fn serverkeys_generate(app: State<App>, passphrase: String) -> R<String> {
    let data = fs::read(app.path("keys.json")).map_err(|_| "No keys yet.".to_owned())?;
    let mut keys = Keys::open(&data, &passphrase)?;
    let mut guard = app.keys.lock().unwrap();
    let current = guard.as_ref().ok_or("Unlock the keys first.")?;
    if keys.admin.to_bytes() != current.admin.to_bytes() {
        return Err("The key file on disk is not the one that is unlocked. Unlock again.".into());
    }
    let public = keys.add_server_key();
    app.write("keys.json", &keys.seal(&passphrase)?)?;
    *guard = Some(keys);
    Ok(public)
}

/// The signed key list every relay serves (`GET /api/server-keys`), verified with this admin key.
async fn live_server_keys(server: &str, key: &VerifyingKey) -> Value {
    match api::public(server, "/api/server-keys").await {
        Ok(None) => json!({ "server": server, "reachable": true, "note": "No server key list published" }),
        Ok(Some(raw)) => match KeyList::open(&raw, &[*key]) {
            Ok(l) => json!({ "server": server, "reachable": true, "verified": true, "serial": l.serial, "issued_at": l.issued_at, "keys": l.keys }),
            Err(e) => json!({ "server": server, "reachable": true, "verified": false, "serial": unverified_header(&raw).map(|h| h.0),
                "note": if e.contains("untrusted") { "Signed by a different admin key".to_owned() } else { format!("Not a valid server key list: {e}") } }),
        },
        Err(e) => json!({ "server": server, "reachable": false, "note": e }),
    }
}

/// One above the highest known serial. Callers pass only serials of documents that verify with
/// this admin key and of this PC's own published copies: a relay can report or serve any number,
/// and a huge one would end publishing for good (devices never accept a lower serial).
fn next_serial(known: impl IntoIterator<Item = u64>, what: &str) -> R<u64> {
    known.into_iter().max().unwrap_or(0).checked_add(1).ok_or_else(|| format!("The {what} serial is exhausted (a document signed with this admin key has the highest possible serial)."))
}

fn verified_serial(live: &Value) -> Option<u64> {
    live["verified"].as_bool().filter(|v| *v).and(live["serial"].as_u64())
}

/// An expiry that has passed (or passes within the hour) makes devices present the
/// new key at once, to relays that do not accept it yet.
fn check_expiry(keys: &[ServerKey], now: u64) -> Result<(), String> {
    match keys.iter().find(|k| k.expires != 0 && k.expires < now + 3600) {
        Some(k) => Err(format!("The expiry of {}… must be at least an hour ahead.", k.key.get(..8).unwrap_or(&k.key))),
        None => Ok(()),
    }
}

/// Public key the relay's hbbs signs device identities with: hbbs `/pubkey` on its health port,
/// public through Caddy as `/rendezvous/pubkey`. Older hbbs or Caddyfile: an error.
async fn signing_key(server: &str) -> R<String> {
    const OLD: &str = "This relay does not report the key hbbs signs with (update hbbs and the Caddyfile)";
    let raw = api::public(server, "/rendezvous/pubkey").await?.ok_or(OLD)?;
    parse_signing_key(&raw).ok_or_else(|| OLD.to_owned())
}

/// One canonical base64 Ed25519 key (an older Caddyfile answers "Tether relay" instead).
fn parse_signing_key(raw: &[u8]) -> Option<String> {
    let k = std::str::from_utf8(raw).ok()?.trim();
    tether_trust::key_from_b64(k).ok().filter(|p| tether_trust::key_to_b64(p) == k).map(|_| k.to_owned())
}

#[tauri::command]
async fn serverkeys_live(app: State<'_, App>) -> R<Value> {
    let key = app.admin_key()?.verifying_key();
    let mut relays = Vec::new();
    for sv in app.servers() {
        let mut r = live_server_keys(&sv, &key).await;
        match signing_key(&sv).await {
            Ok(k) => r["signing"] = json!(k),
            Err(e) => r["signing_note"] = json!(e),
        }
        relays.push(r);
    }
    let rotation: Value = fs::read(app.path("server-keys/rotation.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(json!({}));
    Ok(json!({ "relays": relays, "rotation": rotation }))
}

/// Progress of a rotation (current and new public key, relays switched), kept on this PC.
#[tauri::command]
fn serverkeys_rotation_set(app: State<App>, rotation: Value) -> R<()> {
    app.write("server-keys/rotation.json", &serde_json::to_vec_pretty(&rotation).unwrap())
}

/// Sign the key list and publish it to every relay, then read each one back.
#[tauri::command]
async fn serverkeys_publish(app: State<'_, App>, keys: Vec<ServerKey>) -> R<Value> {
    let admin = app.admin_key()?;
    // Only serials of lists signed by this admin key count: a relay can report or serve any
    // number, and a huge one would end rotations for good (devices never accept a lower serial).
    let mut live = Vec::new();
    for sv in app.servers() {
        live.extend(verified_serial(&live_server_keys(&sv, &admin.verifying_key()).await));
    }
    let local = fs::read_dir(app.path("server-keys/published")).into_iter().flatten().flatten()
        .filter_map(|e| e.path().file_stem()?.to_str()?.parse::<u64>().ok());
    let serial = next_serial(live.into_iter().chain(local), "server key list")?;
    check_expiry(&keys, api::now())?;
    let list = KeyList { serial, issued_at: api::now(), keys };
    list.validate()?;
    let payload = serde_json::to_vec(&list).unwrap();
    let doc = serde_json::to_vec(&envelope::sign(&payload, &admin)).unwrap();
    let mut results = Vec::new();
    for sv in app.servers() {
        let r = match app.call_on(&sv, "PUT", "/api/admin/server-keys", doc.clone(), "application/json").await {
            Err(e) => Err(e),
            Ok(_) => match verified_serial(&live_server_keys(&sv, &admin.verifying_key()).await) {
                Some(s) if s == list.serial => Ok(json!({ "serial": s })),
                s => Err(format!("Uploaded, but the relay still serves serial {}", s.map_or("none".into(), |s| s.to_string()))),
            },
        };
        results.push(RelayResult::of(&sv, r));
    }
    if let Some(e) = none_ok(&results) {
        return Err(e);
    }
    app.write(&format!("server-keys/published/{}.json", list.serial), &doc)?;
    Ok(json!({ "serial": list.serial, "relays": results }))
}

/// `id_ed25519` and `id_ed25519.pub` of a server key generated here, for the relays. The private
/// key file can be opened by the current user only (private.rs); delete it after installing.
#[tauri::command]
fn serverkeys_save_files(app: State<App>, public: String, folder: String) -> R<()> {
    let (secret, public) = app.keys.lock().unwrap().as_ref().ok_or("Unlock the keys first.")?.server_key_files(&public).ok_or("This key was not generated on this PC.")?;
    let dir = PathBuf::from(folder);
    private::write(&dir.join("id_ed25519"), secret.as_bytes())?;
    fs::write(dir.join("id_ed25519.pub"), public).map_err(err)
}

/// Delete the key files "Save key files…" wrote into `folder` (only those two names).
#[tauri::command]
fn serverkeys_delete_files(folder: String) -> R<()> {
    for name in ["id_ed25519", "id_ed25519.pub"] {
        match fs::remove_file(PathBuf::from(&folder).join(name)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(format!("{name}: {e}")),
            _ => {}
        }
    }
    Ok(())
}

// ---- policy ----

#[tauri::command]
fn features() -> Value {
    json!(policy::FEATURES)
}

#[tauri::command]
async fn policy_draft(app: State<'_, App>) -> R<Bundle> {
    if let Ok(b) = fs::read(app.path("policy/draft.json")) {
        return serde_json::from_slice(&b).map_err(err);
    }
    // First run on this PC: start from what the server serves, but only if we signed it.
    let key = app.admin_key()?.verifying_key();
    let raw = api::public(&app.server(), "/api/policy").await.ok().flatten();
    Ok(raw.and_then(|r| envelope::open_json(&r, &[key]).ok()).and_then(|p| Bundle::from_json(&p).ok()).unwrap_or_default())
}

#[tauri::command]
fn policy_save_draft(app: State<App>, bundle: Bundle) -> R<()> {
    app.write("policy/draft.json", &serde_json::to_vec_pretty(&bundle).unwrap())
}

#[tauri::command]
fn policy_check(bundle: Bundle) -> R<()> {
    bundle.validate()
}

#[tauri::command]
fn policy_preview(bundle: Bundle, profile: String, device: String) -> Effective {
    policy::resolve(Some(&bundle), &profile, &device, None)
}

/// What one relay serves at `GET /api/policy`.
struct LiveDoc {
    raw: Option<Vec<u8>>,
    /// Verified with this app's admin key.
    bundle: Option<Bundle>,
    serial: Option<u64>,
    issued_at: Option<u64>,
    note: Option<String>,
}

/// Serial and time of a signed policy without trusting it (to explain "signed by another key").
fn unverified_header(raw: &[u8]) -> Option<(u64, u64)> {
    use base64::Engine;
    let doc: envelope::Signed = serde_json::from_slice(raw).ok()?;
    let payload = base64::engine::general_purpose::STANDARD.decode(doc.payload).ok()?;
    let v: Value = serde_json::from_slice(&payload).ok()?;
    Some((v["serial"].as_u64()?, v["issued_at"].as_u64().unwrap_or(0)))
}

fn verified_policy(raw: &[u8], key: &VerifyingKey) -> R<Bundle> {
    envelope::open_json(raw, &[*key]).and_then(|p| Bundle::from_json(&p))
}

async fn live_doc(server: &str, key: &VerifyingKey) -> LiveDoc {
    let mut d = LiveDoc { raw: None, bundle: None, serial: None, issued_at: None, note: None };
    match api::public_quick(server, "/api/policy").await {
        Ok(None) => d.note = Some("No policy published".into()),
        Ok(Some(raw)) => {
            match verified_policy(&raw, key) {
                Ok(b) => {
                    (d.serial, d.issued_at) = (Some(b.serial), Some(b.issued_at));
                    d.bundle = Some(b);
                }
                Err(e) => {
                    if let Some((s, t)) = unverified_header(&raw) {
                        (d.serial, d.issued_at) = (Some(s), Some(t));
                    }
                    d.note = Some(if e.contains("untrusted") {
                        "Signed by a different admin key: devices built with this app's key reject it".into()
                    } else {
                        format!("Not a valid policy: {e}")
                    });
                }
            }
            d.raw = Some(raw);
        }
        Err(e) => d.note = Some(e),
    }
    d
}

/// What is live now on every relay, the newest verified bundle, its effective values per profile,
/// and relays the policy names that are not in this app's relay list.
#[tauri::command]
async fn policy_live(app: State<'_, App>) -> R<Value> {
    let admin = app.admin_key()?;
    let key = admin.verifying_key();
    let servers = app.servers();
    let mut relays = Vec::new();
    let mut best: Option<Bundle> = None;
    // All relays at once, each request bounded by api::QUICK: one slow or dead relay must not
    // hold up the others (the view shows nothing until every relay has answered).
    let per = futures_util::future::join_all(servers.iter().map(|sv| futures_util::future::join(api::admin_quick(sv, &admin, "/api/admin/status"), live_doc(sv, &key)))).await;
    for (sv, (reach, d)) in servers.iter().zip(per) {
        let reach = reach.map(|_| ());
        relays.push(json!({
            "server": sv,
            "reachable": reach.is_ok(),
            "error": reach.err(),
            "serial": d.serial,
            "issued_at": d.issued_at,
            "verified": d.bundle.is_some(),
            "note": d.note,
        }));
        if let Some(b) = d.bundle {
            if best.as_ref().map_or(true, |x| x.serial < b.serial) {
                best = Some(b);
            }
        }
    }
    let draft: Option<Bundle> = fs::read(app.path("policy/draft.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let known: Vec<&Bundle> = best.iter().chain(draft.iter()).collect();
    Ok(json!({
        "relays": relays,
        "serial": best.as_ref().map(|b| b.serial),
        "issued_at": best.as_ref().map(|b| b.issued_at),
        "effective": {
            "technician": live::effective_rows(best.as_ref(), policy::PROFILE_TECHNICIAN),
            "customer": live::effective_rows(best.as_ref(), policy::PROFILE_CUSTOMER),
        },
        "unconfigured": live::unconfigured_hosts(&servers, &known),
        "bundle": best,
    }))
}

/// Changes from the live bundle (`None`: nothing published) to the draft, in plain words.
#[tauri::command]
fn policy_diff(from: Option<Bundle>, to: Bundle) -> Vec<live::Change> {
    live::changes(from.as_ref(), &to)
}

/// Sign and publish to every relay, then read each relay's `GET /api/policy` back: a relay counts
/// as done only when it serves the new serial.
#[tauri::command]
async fn policy_publish(app: State<'_, App>, bundle: Bundle) -> R<Value> {
    let mut b = bundle;
    b.validate()?;
    let key = app.admin_key()?;
    // An unreachable relay gets the document later (policy_sync); the serial only has to be above
    // every policy this admin key signed that a relay serves, and this PC's own published ones.
    // Not `/api/admin/status` policy_serial: that is whatever the relay claims (see next_serial).
    let mut live = Vec::new();
    for sv in app.servers() {
        live.extend(live_doc(&sv, &key.verifying_key()).await.bundle.map(|b| b.serial));
    }
    b.serial = next_serial(live.into_iter().chain(app.published().into_keys()), "policy")?;
    b.issued_at = api::now();
    let payload = serde_json::to_vec(&b).unwrap();
    let doc = serde_json::to_vec(&envelope::sign(&payload, &key)).unwrap();
    let results = put_policy(&app, &app.servers(), &doc, b.serial, &key.verifying_key()).await;
    if let Some(e) = none_ok(&results) {
        return Err(e);
    }
    app.write(&format!("policy/published/{}.json", b.serial), &payload)?;
    app.write("policy/draft.json", &serde_json::to_vec_pretty(&b).unwrap())?;
    Ok(json!({ "serial": b.serial, "relays": results }))
}

/// PUT a signed policy document to `servers` and confirm each one serves `serial` afterwards.
async fn put_policy(app: &App, servers: &[String], doc: &[u8], serial: u64, key: &VerifyingKey) -> Vec<RelayResult> {
    let mut out = Vec::new();
    for sv in servers {
        let r = match app.call_on(sv, "PUT", "/api/admin/policy", doc.to_vec(), "application/json").await {
            Err(e) => Err(e),
            Ok(_) => {
                let back = live_doc(sv, key).await;
                if back.bundle.as_ref().map(|b| b.serial) == Some(serial) {
                    Ok(json!({ "serial": serial }))
                } else {
                    Err(format!("Uploaded, but the relay still serves serial {}", back.serial.map_or("none".into(), |s| s.to_string())))
                }
            }
        };
        out.push(RelayResult::of(sv, r));
    }
    out
}

/// Bring relays that are behind (or serve a document this key did not sign) to the newest policy
/// any relay serves, by PUTting that same signed document (docs/relay-ha.md).
#[tauri::command]
async fn policy_sync(app: State<'_, App>) -> R<Vec<RelayResult>> {
    let key = app.admin_key()?.verifying_key();
    let mut docs = Vec::new();
    for sv in app.servers() {
        let d = live_doc(&sv, &key).await;
        docs.push((sv, d));
    }
    let (serial, raw) = docs
        .iter()
        .filter_map(|(_, d)| Some((d.bundle.as_ref()?.serial, d.raw.clone()?)))
        .max_by_key(|(s, _)| *s)
        .ok_or("No relay serves a policy signed by this app's admin key. Publish one first.")?;
    let behind: Vec<String> = docs.iter().filter(|(_, d)| d.bundle.as_ref().map(|b| b.serial) != Some(serial)).map(|(s, _)| s.clone()).collect();
    let mut out = put_policy(&app, &behind, &raw, serial, &key).await;
    for (sv, _) in docs.iter().filter(|(s, _)| !behind.contains(s)) {
        out.push(RelayResult::of(sv, Ok(json!({ "serial": serial, "already": true }))));
    }
    Ok(out)
}

/// Server history merged with the local copies.
#[tauri::command]
async fn policy_history(app: State<'_, App>) -> R<Vec<Value>> {
    let server = app.call_json("GET", "/api/admin/policy-history", None).await?;
    let local = app.published();
    let mut rows: BTreeMap<u64, Value> = BTreeMap::new();
    for h in server.as_array().into_iter().flatten() {
        if let Some(s) = h["serial"].as_u64() {
            rows.insert(s, json!({"serial": s, "issued_at": h["issued_at"], "server": true, "local": local.contains_key(&s)}));
        }
    }
    for (s, p) in &local {
        rows.entry(*s).or_insert_with(|| {
            let issued = fs::read(p).ok().and_then(|b| serde_json::from_slice::<Bundle>(&b).ok()).map(|b| b.issued_at);
            json!({"serial": s, "issued_at": issued, "server": false, "local": true})
        });
    }
    Ok(rows.into_values().rev().collect())
}

/// Content of a published version: the local copy, else the signed document kept by the server
/// (verified against this app's admin key, so a tampered server copy is never republished).
#[tauri::command]
async fn policy_local(app: State<'_, App>, serial: u64) -> R<Bundle> {
    if let Some(p) = app.published().remove(&serial) {
        return serde_json::from_slice(&fs::read(p).map_err(err)?).map_err(err);
    }
    let doc = app.call("GET", &format!("/api/admin/policy-history/{serial}"), Vec::new(), "application/json").await?;
    let payload = envelope::open_json(&doc, &[app.admin_key()?.verifying_key()])?;
    Bundle::from_json(&payload)
}

// ---- releases ----

#[derive(Deserialize)]
struct ReleaseArgs {
    file: String,
    kind: String,
    /// `windows-x86_64` or `darwin-universal`
    platform: String,
    channel: String,
    version: String,
    notes: String,
    mandatory: bool,
    minimum_version: String,
    forced: bool,
    /// Ramp preset (`standard` = D4 default, `fast`, `now`, `custom:<hours>=<percent>,…`); "" = standard.
    #[serde(default)]
    rollout: String,
    /// Unix seconds the ramp starts; 0 = now.
    #[serde(default)]
    rollout_start: u64,
    /// Security release: skips the ramp.
    #[serde(default)]
    security: bool,
}

/// One release as it goes to a relay: its files (artifact first) and the manifest fields.
struct Upload {
    channel: String,
    dir: String,
    platform: String,
    /// The updater artifact, the file `latest.json` points at.
    name: String,
    sig: String,
    files: Vec<(String, Vec<u8>)>,
    version: String,
    notes: String,
    mandatory: bool,
    minimum_version: String,
    forced: bool,
    pub_date: String,
    /// The gzip copy (TetherQS.exe), also in `files` as `<name>.gz`.
    packed: Option<Vec<u8>>,
}

/// Event `release-progress` for the publish sheet: one release (`dir`, `version`) going to one
/// relay. `file` "" = signing; `done` = this relay is finished with it (failed if `error`).
#[derive(Serialize, Clone)]
struct Progress {
    dir: String,
    version: String,
    server: String,
    file: String,
    sent: u64,
    total: u64,
    done: bool,
    error: Option<String>,
}

impl Progress {
    fn new(u: &Upload, server: &str, file: &str) -> Self {
        Progress { dir: u.dir.clone(), version: u.version.clone(), server: server.to_owned(), file: file.to_owned(), sent: 0, total: 0, done: false, error: None }
    }
}

fn emit(ui: &AppHandle, p: Progress) {
    let _ = ui.emit("release-progress", p);
}

/// [`upload_release`] with its outcome reported as the relay's `done` event.
async fn upload_release_reported(app: &App, ui: &AppHandle, server: &str, u: &Upload) -> R<String> {
    let r = upload_release(app, ui, server, u).await;
    emit(ui, Progress { done: true, error: r.as_ref().err().cloned(), ..Progress::new(u, server, "") });
    r
}

/// Every node gets the files and its own manifest: its download URL points at itself, so a node
/// can serve updates even when the others are down. Returns the update URL.
async fn upload_release(app: &App, ui: &AppHandle, server: &str, u: &Upload) -> R<String> {
    upload_files(app, ui, server, u).await?;
    put_manifest(app, ui, server, u).await
}

/// The release files (artifact first) to one relay.
async fn upload_files(app: &App, ui: &AppHandle, server: &str, u: &Upload) -> R<()> {
    let base = format!("/api/admin/release/{}/{}", u.channel, u.dir);
    for (f, bytes) in &u.files {
        let ct = if f.ends_with(".sig") { "text/plain" } else { "application/octet-stream" };
        let p = Progress { total: bytes.len() as u64, ..Progress::new(u, server, f) };
        emit(ui, p.clone());
        let ui = ui.clone();
        api::admin_upload(server, &app.admin_key()?, &format!("{base}/{f}"), bytes.clone(), ct, move |sent| emit(&ui, Progress { sent, ..p.clone() })).await?;
    }
    Ok(())
}

/// The legacy `latest.json` (clients up to 0.5.23), written last. Returns the update URL.
async fn put_manifest(app: &App, ui: &AppHandle, server: &str, u: &Upload) -> R<String> {
    let url = format!("{server}/download/{}/{}/{}", u.channel, u.dir, u.name);
    let new = release::platform_keys(&u.platform).iter().map(|k| (k.to_string(), release::Platform { signature: u.sig.clone(), url: url.clone() })).collect();
    // Read before writing: another OS's build of the same version in this manifest stays listed.
    let existing = api::public(server, &format!("/download/{}/{}/latest.json", u.channel, u.dir)).await?;
    let manifest = release::Manifest {
        version: u.version.clone(),
        notes: u.notes.clone(),
        pub_date: u.pub_date.clone(),
        platforms: release::merge_platforms(existing.as_deref(), &u.version, new),
        mandatory: u.mandatory,
        minimum_version: u.minimum_version.clone(),
        forced: u.forced,
    };
    let base = format!("/api/admin/release/{}/{}", u.channel, u.dir);
    emit(ui, Progress::new(u, server, "latest.json"));
    app.call_on(server, "PUT", &format!("{base}/latest.json"), serde_json::to_vec_pretty(&manifest).unwrap(), "application/json").await?;
    Ok(url)
}

/// Every file of one release, in upload order: the artifact, its signature, and for the mac
/// technician app the manual-download disk image with its signature.
fn release_files(kind: &str, platform: &str, version: &str, name: &str) -> Vec<String> {
    let mut v = vec![name.to_owned(), format!("{name}.sig")];
    if release::wants_packed(kind, platform) {
        v.push(format!("{name}.gz"));
    }
    if (kind, platform) == ("technician", release::MAC) {
        let dmg = release::dmg_name(version);
        v.push(format!("{dmg}.sig"));
        v.insert(2, dmg);
    }
    v
}

#[tauri::command]
async fn release_publish(app: State<'_, App>, ui: AppHandle, args: ReleaseArgs) -> R<Vec<RelayResult>> {
    let a = args;
    if !release::valid_channel(&a.channel) {
        return Err("Channel: use 1-32 lowercase letters, digits or dashes.".into());
    }
    if !release::valid_version(&a.version) {
        return Err("Version must look like 0.3.1 or 0.3.1-beta.".into());
    }
    if !a.minimum_version.is_empty() && !release::valid_version(&a.minimum_version) {
        return Err("Minimum version must look like 0.3.0.".into());
    }
    let rollout = release::rollout(&a.rollout, if a.rollout_start == 0 { api::now() } else { a.rollout_start })?;
    let (dir, name) = release::target(&a.kind, &a.platform, &a.version)?;
    let data = fs::read(&a.file).map_err(|e| format!("Could not read the file: {e}"))?;
    let sign = |bytes: &[u8], file: &str| -> R<String> {
        // No server yet: the sheet shows "Signing <file>".
        emit(&ui, Progress { dir: dir.clone(), version: a.version.clone(), server: String::new(), file: file.to_owned(), sent: 0, total: bytes.len() as u64, done: false, error: None });
        let k = app.keys.lock().unwrap();
        k.as_ref().ok_or("Unlock the keys first.")?.sign_update(bytes, file, &a.version)
    };
    let sig = sign(&data, &name)?;
    let mut files = vec![(name.clone(), data), (format!("{name}.sig"), sig.clone().into_bytes())];
    // The mac technician release also carries the .dmg for manual downloads (download page).
    if (a.kind.as_str(), a.platform.as_str()) == ("technician", release::MAC) {
        let dmg_name = release::dmg_name(&a.version);
        let path = std::path::Path::new(&a.file).with_file_name(&dmg_name);
        let bytes = fs::read(&path).map_err(|_| format!("Put {dmg_name} next to the .app.tar.gz (scripts/build-macos.sh writes both to dist/macos)."))?;
        let dmg_sig = sign(&bytes, &dmg_name)?;
        files.push((dmg_name.clone(), bytes));
        files.push((format!("{dmg_name}.sig"), dmg_sig.into_bytes()));
    }
    let packed = release::wants_packed(&a.kind, &a.platform).then(|| release::gzip(&files[0].1));
    if let Some(p) = &packed {
        files.push((format!("{name}.gz"), p.clone()));
    }
    let security = a.security;
    let u = Upload {
        channel: a.channel,
        dir,
        platform: a.platform,
        name,
        sig,
        files,
        version: a.version,
        notes: a.notes,
        mandatory: a.mandatory,
        minimum_version: a.minimum_version,
        forced: a.forced,
        pub_date: release::rfc3339(api::now()),
        packed,
    };
    publish_upload(&app, &ui, &u, security, rollout).await
}

/// The update file the CDN serves for a release: the gzip copy if there is one, else the artifact.
fn cdn_file(u: &Upload) -> (String, &[u8]) {
    match &u.packed {
        Some(p) => (format!("{}.gz", u.name), p.as_slice()),
        None => (u.name.clone(), u.files[0].1.as_slice()),
    }
}

/// Upload order (docs/update-design.md §5.9): files to every relay and the CDN, then the signed
/// release index to every relay, then the legacy `latest.json`. One result per relay (+ "CDN").
async fn publish_upload(app: &App, ui: &AppHandle, u: &Upload, security: bool, rollout: tether_trust::release::Rollout) -> R<Vec<RelayResult>> {
    let cdn_conf = app.cdn_config();
    if cdn_conf.is_some() {
        let (f, bytes) = cdn_file(u);
        cdn::check_size(&f, bytes.len() as u64)?;
    }
    let servers = app.servers();
    let mut errs: Vec<Option<String>> = Vec::new();
    for server in &servers {
        let r = upload_files(app, ui, server, u).await;
        if let Err(e) = &r {
            emit(ui, Progress { done: true, error: Some(e.clone()), ..Progress::new(u, server, "") });
        }
        errs.push(r.err());
    }
    if errs.iter().all(Option::is_some) {
        return Err(servers.iter().zip(&errs).map(|(s, e)| format!("{s}: {}", e.as_deref().unwrap_or(""))).collect::<Vec<_>>().join("; "));
    }
    let mut extra = Vec::new();
    let cdn_base = match cdn_publish(app, ui, u).await {
        Ok(b) => {
            if let Some(b) = &b {
                extra.push(RelayResult::of("CDN", Ok(json!({ "base": b }))));
            }
            b
        }
        Err(e) => {
            // The relays carry the release alone; clients use the relay URL.
            extra.push(RelayResult::of("CDN", Err(e)));
            None
        }
    };
    let (_, data) = (&u.files[0].0, &u.files[0].1);
    let m = release::Manifest {
        version: u.version.clone(),
        notes: u.notes.clone(),
        pub_date: u.pub_date.clone(),
        platforms: Default::default(),
        mandatory: u.mandatory,
        minimum_version: u.minimum_version.clone(),
        forced: u.forced,
    };
    let e = release::entry(&u.channel, &u.dir, &u.name, data, &u.sig, u.packed.as_deref(), cdn_base.as_deref(), &m, security, rollout);
    let key = u.dir.clone();
    // Only relays that hold the files get the new index: clients fetch `{relay}` URLs from the
    // relay the index came from, so a relay without the files keeps its old index (devices there
    // stay on the old release; those that see both relays take the higher serial).
    let uploaded: Vec<String> = servers.iter().zip(&errs).filter(|(_, e)| e.is_none()).map(|(s, _)| s.clone()).collect();
    let index = publish_index(app, &u.channel, &servers, &uploaded, move |r| {
        r.insert(key, e);
    })
    .await;
    let mut out = Vec::new();
    for (i, server) in servers.iter().enumerate() {
        let r = match (&errs[i], &index) {
            (Some(e), _) => Err(format!("{e} (release index not updated on this relay)")),
            (None, Err(e)) => Err(format!("release index: {e}")),
            (None, Ok(per)) => match per.get(server) {
                Some(Err(e)) => Err(format!("release index: {e}")),
                _ => put_manifest(app, ui, server, u).await,
            },
        };
        emit(ui, Progress { done: true, error: r.as_ref().err().cloned(), ..Progress::new(u, server, "") });
        out.push(RelayResult::of(server, r.map(|url| json!({ "url": url }))));
    }
    if let Some(e) = none_ok(&out) {
        return Err(e);
    }
    out.extend(extra);
    out.extend(auto_prune(app).await);
    Ok(out)
}

/// The CDN's next version: the release's update file plus what the CDN already holds. `Ok(None)`:
/// no CDN configured.
async fn cdn_publish(app: &App, ui: &AppHandle, u: &Upload) -> R<Option<String>> {
    let Some((creds, base)) = app.cdn_config() else { return Ok(None) };
    let r = cdn_deploy(app, ui, u, creds).await;
    // Same events as a relay, target "CDN": the progress view and its finished list show it.
    emit(ui, Progress { done: true, error: r.as_ref().err().cloned(), ..Progress::new(u, "CDN", "") });
    r.map(|()| Some(base))
}

async fn cdn_deploy(app: &App, ui: &AppHandle, u: &Upload, creds: cdn::Creds) -> R<()> {
    let (file, bytes) = cdn_file(u);
    let path = format!("/{}/{}/{}", u.channel, u.dir, file);
    let current: cdn::Assets = fs::read(app.path("cdn-assets.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let next = cdn::next_assets(&current, &format!("{}/{}", u.channel, u.dir), &u.version, &[(path.clone(), bytes)])?;
    let new = std::sync::Arc::new((path, bytes.to_vec()));
    let relay = app.server();
    let (ui2, base) = (ui.clone(), Progress::new(u, "CDN", ""));
    let on: cdn::OnProgress = std::sync::Arc::new(move |step: &str, sent, total| emit(&ui2, Progress { file: step.to_owned(), sent, total, ..base.clone() }));
    cdn::deploy(api::shared(), cdn::API, &creds, &next, move |p: String| {
        let (new, relay) = (new.clone(), relay.clone());
        async move {
            if p == new.0 {
                return Ok(new.1.clone());
            }
            // An older file Cloudflare no longer has: the relays keep every release file.
            api::public(&relay, &format!("/download{p}")).await?.ok_or_else(|| format!("{p} is missing on {relay}"))
        }
    }, on)
    .await?;
    app.write("cdn-assets.json", &serde_json::to_vec_pretty(&next).unwrap())
}

type LiveIndex = Option<(tether_trust::release::Index, Vec<u8>)>;

/// This PC's admin key and the ones the apps trust ([`TRUSTED_ADMIN_KEYS`]).
fn trusted_admin_keys(own: VerifyingKey, built_in: &str) -> Vec<VerifyingKey> {
    let mut keys = vec![own];
    keys.extend(built_in.split(',').filter(|k| !k.trim().is_empty()).filter_map(|k| tether_trust::key_from_b64(k).ok()).filter(|k| *k != own));
    keys
}

/// Per relay: its release index verified with `keys`, `Ok(None)` when it serves none (or cannot be
/// reached), `Err` when it serves one that no trusted key verifies.
fn open_indexes(got: Vec<(String, Option<Vec<u8>>)>, keys: &[VerifyingKey], channel: &str) -> Vec<(String, R<LiveIndex>)> {
    got.into_iter().map(|(sv, raw)| {
        let ix = raw.map(|raw| tether_trust::release::Index::open(&raw, keys, channel).map(|i| (i, raw))).transpose();
        (sv, ix)
    }).collect()
}

/// The indexes to build the next one on. A relay's index no trusted key verifies (signed with an
/// admin key this build does not list, or damaged) stops the publish: building without it would
/// drop its releases and could take a serial below the devices' floor.
fn usable_indexes(live: Vec<(String, R<LiveIndex>)>, channel: &str) -> R<Vec<(String, LiveIndex)>> {
    live.into_iter().map(|(sv, ix)| ix.map(|i| (sv.clone(), i)).map_err(|e| format!(
        "{sv} serves a release index for {channel} that no trusted admin key verifies ({e}). Publishing now would drop the releases in it. If the admin key was rotated, build Tether Manage with TETHER_ADMIN_PUB_KEY listing the keys the apps trust (docs/management.md)."
    ))).collect()
}

async fn fetch_indexes(app: &App, channel: &str, servers: &[String]) -> R<Vec<(String, R<LiveIndex>)>> {
    Ok(fetch_indexes_why(app, channel, servers).await?.0)
}

/// [`fetch_indexes`] plus the read error of each relay that did not answer (its index counts as
/// none). All relays at once, each bounded by [`api::QUICK`].
async fn fetch_indexes_why(app: &App, channel: &str, servers: &[String]) -> R<(Vec<(String, R<LiveIndex>)>, BTreeMap<String, String>)> {
    let keys = trusted_admin_keys(app.admin_key()?.verifying_key(), TRUSTED_ADMIN_KEYS.unwrap_or(""));
    let path = format!("/download/{channel}/releases.json");
    let raw = futures_util::future::join_all(servers.iter().map(|sv| api::public_quick(sv, &path))).await;
    let mut errors = BTreeMap::new();
    let got = servers.iter().zip(raw).map(|(sv, r)| (sv.clone(), r.unwrap_or_else(|e| { errors.insert(sv.clone(), e); None }))).collect();
    Ok((open_indexes(got, &keys, channel), errors))
}

/// Every relay's release index for `channel` that a trusted admin key signed: `(server, index, raw)`.
/// For views; one that does not verify counts as none.
async fn relay_indexes(app: &App, channel: &str, servers: &[String]) -> R<Vec<(String, LiveIndex)>> {
    Ok(fetch_indexes(app, channel, servers).await?.into_iter().map(|(s, i)| (s, i.ok().flatten())).collect())
}

/// Sign the next index (the highest verified one on `servers`, changed by `change`) and put it on
/// the relays in `targets`. Serial: max(relays, this PC's last) + 1. Per target: the outcome.
async fn publish_index(app: &App, channel: &str, servers: &[String], targets: &[String], change: impl FnOnce(&mut BTreeMap<String, tether_trust::release::Entry>)) -> R<BTreeMap<String, R<()>>> {
    let (live, unread) = fetch_indexes_why(app, channel, servers).await?;
    let live = usable_indexes(live, channel)?;
    let local_path = format!("releases/{channel}.serial");
    let local: u64 = fs::read_to_string(app.path(&local_path)).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    let base = live.iter().filter_map(|(_, i)| i.as_ref().map(|i| i.0.clone())).max_by_key(|i| i.serial);
    let serial = next_serial(live.iter().filter_map(|(_, i)| i.as_ref().map(|i| i.0.serial)).chain([local]), "release index")?;
    let ix = release::next_index(base, channel, serial, api::now(), change)?;
    let doc = serde_json::to_vec(&envelope::sign(&serde_json::to_vec(&ix).unwrap(), &app.admin_key()?)).unwrap();
    app.write(&local_path, serial.to_string().as_bytes())?;
    // Devices fetch `{relay}` URLs from the relay the index came from: a relay that lacks one of
    // the files keeps its old index.
    let paths = relay_paths(&ix);
    // Every relay and file at once (each check bounded by api::QUICK): a relay that does not answer
    // counts as lacking the files instead of holding up the publish.
    let checks = futures_util::future::join_all(targets.iter().map(|sv| futures_util::future::join_all(paths.iter().map(move |p| api::exists(sv, p))))).await;
    let mut missing = BTreeMap::new();
    for (sv, got) in targets.iter().zip(checks) {
        let mut m = Vec::new();
        for (p, r) in paths.iter().zip(got) {
            match r {
                Ok(true) => {}
                Ok(false) => m.push(p.clone()),
                Err(e) => m.push(format!("{p} (could not check: {e})")),
            }
        }
        missing.insert(sv.clone(), m);
    }
    let (send, mut out) = index_targets(targets, &missing);
    // A relay that did not answer the read just now is not written (the write could hang for long).
    let (send, silent): (Vec<String>, Vec<String>) = send.into_iter().partition(|sv| !unread.contains_key(sv));
    for sv in silent {
        let e = format!("not updated: {}", unread[&sv]);
        out.insert(sv, Err(e));
    }
    let path = format!("/api/admin/release/{channel}/releases.json");
    let put = futures_util::future::join_all(send.iter().map(|sv| app.call_on(sv, "PUT", &path, doc.clone(), "application/json"))).await;
    out.extend(send.into_iter().zip(put).map(|(sv, r)| (sv, r.map(|_| ()))));
    Ok(out)
}

/// The `/download/…` paths devices fetch from the relay that served `ix` (artifacts and packed copies).
fn relay_paths(ix: &tether_trust::release::Index) -> Vec<String> {
    let urls = ix.releases.values().flat_map(|e| e.urls.iter().chain(e.packed.iter().flat_map(|p| p.urls.iter())));
    let mut v: Vec<String> = urls.filter_map(|u| u.strip_prefix("https://{relay}").map(str::to_owned)).collect();
    v.sort();
    v.dedup();
    v
}

/// The targets that serve every file of the new index, and an error for each one that does not.
fn index_targets(targets: &[String], missing: &BTreeMap<String, Vec<String>>) -> (Vec<String>, BTreeMap<String, R<()>>) {
    let mut send = Vec::new();
    let mut skipped = BTreeMap::new();
    for sv in targets {
        match missing.get(sv).filter(|m| !m.is_empty()) {
            None => send.push(sv.clone()),
            Some(m) => {
                skipped.insert(sv.clone(), Err(format!("the relay lacks {}; not updated, use Publish to missing relays", m.join(", "))));
            }
        }
    }
    (send, skipped)
}

/// The rollout view: the highest verified index on the relays, with the ramp's percent now.
#[tauri::command]
async fn release_index(app: State<'_, App>, channel: String) -> R<Value> {
    if !release::valid_channel(&channel) {
        return Err("invalid channel".into());
    }
    let servers = app.servers();
    let (live, why) = fetch_indexes_why(&app, &channel, &servers).await?;
    let relays: Vec<Value> = live.iter().map(|(s, i)| json!({ "server": s, "serial": i.as_ref().ok().and_then(Option::as_ref).map(|i| i.0.serial), "error": i.as_ref().err().or(why.get(s)) })).collect();
    let Some(ix) = live.into_iter().filter_map(|(_, i)| i.ok().flatten().map(|i| i.0)).max_by_key(|i| i.serial) else {
        return Ok(json!({ "channel": channel, "serial": null, "relays": relays, "entries": [] }));
    };
    let now = api::now();
    let entries: Vec<Value> = ix
        .releases
        .iter()
        .map(|(k, e)| {
            json!({ "key": k, "version": e.version, "percent": e.percent(now), "halted": e.halted, "security": e.security,
                "mandatory": e.mandatory, "forced": e.forced, "start": e.rollout.start, "steps": e.rollout.steps, "cdn": e.urls.len() > 1 || e.packed.as_ref().is_some_and(|p| p.urls.len() > 1) })
        })
        .collect();
    Ok(json!({ "channel": channel, "serial": ix.serial, "expires": ix.expires, "days_left": (ix.expires.saturating_sub(now)) / 86_400, "stale": ix.stale(now), "relays": relays, "entries": entries }))
}

/// Halt, resume, or restart the ramp of one release: a re-signed index (new serial). Devices see it
/// within about a minute (hbbs push) and re-check right before installing.
#[tauri::command]
async fn release_control(app: State<'_, App>, channel: String, key: String, action: String, preset: Option<String>) -> R<Vec<RelayResult>> {
    if !release::valid_channel(&channel) {
        return Err("invalid channel".into());
    }
    let servers = app.servers();
    let preset = preset.unwrap_or_default();
    let ramp = if action == "ramp" { Some(release::rollout(&preset, api::now())?) } else { None };
    let mut missing = false;
    let per = publish_index(&app, &channel, &servers, &servers, |r| match r.get_mut(&key) {
        None => missing = true,
        Some(e) => match action.as_str() {
            "halt" => e.halted = true,
            "resume" => e.halted = false,
            "security" => e.security = true,
            _ => {
                if let Some(ramp) = ramp {
                    e.rollout = ramp;
                }
            }
        },
    })
    .await?;
    if missing {
        return Err(format!("{key} is not in the release index of {channel}"));
    }
    let out: Vec<RelayResult> = per.into_iter().map(|(s, r)| RelayResult::of(&s, r.map(|_| Value::Null))).collect();
    if let Some(e) = none_ok(&out) {
        return Err(e);
    }
    Ok(out)
}

/// After unlocking: re-sign every channel's index that expires within 15 days (D3: clients only
/// warn after expiry, so this keeps "update information is out of date" from showing).
#[tauri::command]
async fn release_index_refresh(app: State<'_, App>) -> R<Vec<Value>> {
    let servers = app.servers();
    let mut channels: Vec<String> = Vec::new();
    for sv in &servers {
        if let Ok(b) = app.call_on(sv, "GET", "/api/admin/releases", Vec::new(), "application/json").await {
            for f in serde_json::from_slice::<Vec<Value>>(&b).unwrap_or_default() {
                if f["path"] == "releases.json" {
                    if let Some(c) = f["channel"].as_str().filter(|c| !channels.iter().any(|x| x == c)) {
                        channels.push(c.to_owned());
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    for ch in channels {
        let live = relay_indexes(&app, &ch, &servers).await?;
        let Some(expires) = live.iter().filter_map(|(_, i)| i.as_ref()).max_by_key(|i| i.0.serial).map(|i| i.0.expires) else { continue };
        if expires > api::now() + 15 * 86_400 {
            continue;
        }
        let per = publish_index(&app, &ch, &servers, &servers, |_| {}).await?;
        out.push(json!({ "channel": ch, "ok": per.values().all(Result::is_ok) }));
    }
    Ok(out)
}

/// CDN settings (secrets are never sent to the UI).
#[tauri::command]
fn cdn_state(app: State<App>) -> Value {
    let s = app.settings();
    let k = app.keys.lock().unwrap();
    json!({
        "enabled": s["cdn"]["enabled"].as_bool().unwrap_or(false),
        "base": s["cdn"]["base"].as_str().unwrap_or(cdn::DEFAULT_BASE),
        "worker": s["cdn"]["worker"].as_str().unwrap_or(cdn::DEFAULT_WORKER),
        "credentials": k.as_ref().map(|k| k.cdn.as_ref().map(|c| c.account_id.clone())),
    })
}

/// Store the Cloudflare account API token and account id in the key vault (sealed with the
/// passphrase), from the fields or a `cf.env` file; base URL and worker name in the settings.
#[tauri::command]
fn cdn_configure(app: State<App>, enabled: bool, base: String, worker: String, account_id: Option<String>, token: Option<String>, env_file: Option<String>, passphrase: Option<String>) -> R<()> {
    if !base.starts_with("https://") || base.contains(char::is_whitespace) {
        return Err("The CDN address must start with https://".into());
    }
    if worker.is_empty() || !worker.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
        return Err("Worker name: lowercase letters, digits and dashes.".into());
    }
    let creds = match (env_file.filter(|f| !f.is_empty()), account_id.filter(|a| !a.is_empty()), token.filter(|t| !t.is_empty())) {
        (Some(f), _, _) => Some(cdn::parse_env(&fs::read_to_string(&f).map_err(|e| format!("{f}: {e}"))?)?),
        (None, Some(a), Some(t)) => Some((t, a)),
        _ => None,
    };
    if let Some((token, account_id)) = creds {
        let pass = passphrase.filter(|p| !p.is_empty()).ok_or("Enter the key passphrase to store the Cloudflare token.")?;
        let mut k = app.keys.lock().unwrap();
        let keys = k.as_mut().ok_or("Unlock the keys first.")?;
        keys.cdn = Some(keys::CdnCreds { account_id, token });
        let sealed = keys.seal(&pass)?;
        // The passphrase must be the vault's: check by opening what was just sealed is not enough.
        let on_disk = fs::read(app.path("keys.json")).map_err(err)?;
        Keys::open(&on_disk, &pass).map_err(|_| "Wrong passphrase.".to_owned())?;
        app.write("keys.json", &sealed)?;
    }
    if enabled && app.keys.lock().unwrap().as_ref().is_some_and(|k| k.cdn.is_none()) {
        return Err("Store the Cloudflare credentials first.".into());
    }
    let mut s = app.settings();
    s["cdn"] = json!({ "enabled": enabled, "base": base.trim_end_matches('/'), "worker": worker });
    app.write("settings.json", &serde_json::to_vec_pretty(&s).unwrap())
}

/// Files in `dir` and its subfolders, two levels deep (`dist/windows/...`, `dist/macos/...`).
fn files_below(dir: &std::path::Path, depth: u8, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if depth > 0 {
                files_below(&p, depth - 1, out);
            }
        } else {
            out.push(p);
        }
    }
}

/// The newest build of each app and platform in a local folder (e.g. `dist`), with the highest
/// version the relays publish on `channel` for it. `newer`: worth publishing.
#[tauri::command]
async fn release_scan(app: State<'_, App>, folder: String, channel: String) -> R<Vec<Value>> {
    if !release::valid_channel(&channel) {
        return Err("Channel: use 1-32 lowercase letters, digits or dashes.".into());
    }
    let root = std::path::Path::new(&folder);
    if !root.is_dir() {
        return Err(format!("{folder} is not a folder."));
    }
    let mut files = Vec::new();
    files_below(root, 2, &mut files);
    let mut out = Vec::new();
    let builds = release::latest_builds(files);
    let servers = app.servers();
    let dirs = builds.iter().map(|((kind, platform), (version, _))| release::target(kind, platform, version).map(|t| t.0)).collect::<R<Vec<_>>>()?;
    // Every relay's latest.json of every build at once, each bounded by api::QUICK.
    let ch = channel.as_str();
    let urls: Vec<(&str, String)> = dirs.iter().flat_map(|dir| servers.iter().map(move |sv| (sv.as_str(), format!("/download/{ch}/{dir}/latest.json")))).collect();
    let mut got = futures_util::future::join_all(urls.iter().map(|(sv, p)| api::public_quick(sv, p))).await.into_iter();
    for ((kind, platform), (version, path)) in builds {
        let mut published: Option<String> = None;
        for _ in &servers {
            let Some(Ok(Some(b))) = got.next() else { continue };
            let Ok(m) = serde_json::from_slice::<release::Manifest>(&b) else { continue };
            if published.as_ref().map_or(true, |p| tether_trust::version::compare(&m.version, p) == Some(std::cmp::Ordering::Greater)) {
                published = Some(m.version);
            }
        }
        let newer = published.as_ref().map_or(true, |p| tether_trust::version::compare(&version, p) == Some(std::cmp::Ordering::Greater));
        let dmg = release::dmg_name(&version);
        let problem = ((kind, platform) == ("technician", release::MAC) && !path.with_file_name(&dmg).exists()).then(|| format!("{dmg} is missing next to it"));
        out.push(json!({ "kind": kind, "platform": platform, "version": version, "file": path.to_string_lossy(), "published": published, "newer": newer, "problem": problem }));
    }
    Ok(out)
}

/// Per channel × app × platform: the highest uploaded version and what each relay's public
/// `latest.json` publishes now.
#[tauri::command]
async fn releases_overview(app: State<'_, App>) -> R<Value> {
    let servers = app.servers();
    let mut listings: Vec<Option<Vec<(String, String)>>> = Vec::new();
    let mut errors = Vec::new();
    let got = futures_util::future::join_all(servers.iter().map(|sv| app.call_on(sv, "GET", "/api/admin/releases", Vec::new(), "application/json"))).await;
    for (sv, r) in servers.iter().zip(got) {
        match r {
            Ok(b) => {
                let v: Vec<Value> = serde_json::from_slice(&b).unwrap_or_default();
                listings.push(Some(v.iter().filter_map(|f| Some((f["channel"].as_str()?.to_owned(), f["path"].as_str()?.to_owned()))).collect()));
            }
            Err(e) => {
                errors.push(format!("{sv}: {e}"));
                listings.push(None);
            }
        }
    }
    let latest = release::latest_versions(listings.iter().flatten().flatten().map(|(c, p)| (c.as_str(), p.as_str())));
    let mut rows = Vec::new();
    let targets = latest.into_iter().map(|(k, v)| release::target(k.1, k.2, &v).map(|t| (k, v, t))).collect::<R<Vec<_>>>()?;
    // Every relay's latest.json of every row at once, each bounded by api::QUICK.
    let urls: Vec<(&str, String)> = targets.iter().flat_map(|((channel, _, _), _, (dir, _))| servers.iter().map(move |sv| (sv.as_str(), format!("/download/{channel}/{dir}/latest.json")))).collect();
    let mut manifests = futures_util::future::join_all(urls.iter().map(|(sv, p)| api::public_quick(sv, p))).await.into_iter();
    for ((channel, kind, platform), version, (dir, name)) in targets {
        let mut relays = Vec::new();
        for (sv, listing) in servers.iter().zip(&listings) {
            let manifest = manifests.next().expect("one per relay and row");
            let published = match &manifest {
                Ok(Some(b)) => serde_json::from_slice::<release::Manifest>(b).ok().map(|m| m.version),
                _ => None,
            };
            relays.push(json!({
                "server": sv,
                "published": published,
                "has_latest": listing.as_ref().map(|l| l.iter().any(|(c, p)| c == &channel && p == &format!("{dir}/{name}"))),
                "error": manifest.err(),
            }));
        }
        rows.push(json!({ "channel": channel, "kind": kind, "platform": platform, "latest": version, "relays": relays }));
    }
    Ok(json!({ "rows": rows, "errors": errors }))
}

/// Copy a published release to the relays that do not publish it: the files come from a relay
/// whose `latest.json` has that version (signatures included, nothing is re-signed), and each
/// target gets its own manifest pointing at itself.
#[tauri::command]
async fn release_sync(app: State<'_, App>, ui: AppHandle, channel: String, kind: String, platform: String, version: String) -> R<Vec<RelayResult>> {
    if !release::valid_channel(&channel) || !release::valid_version(&version) {
        return Err("invalid channel or version".into());
    }
    let (dir, name) = release::target(&kind, &platform, &version)?;
    let servers = app.servers();
    let mut source = None;
    let mut targets = Vec::new();
    for sv in &servers {
        let m = api::public(sv, &format!("/download/{channel}/{dir}/latest.json")).await.ok().flatten().and_then(|b| serde_json::from_slice::<release::Manifest>(&b).ok());
        match m {
            Some(m) if m.version == version => {
                source.get_or_insert((sv.clone(), m));
            }
            _ => targets.push(sv.clone()),
        }
    }
    let (src, m) = source.ok_or_else(|| format!("No relay publishes {version} yet. Publish it with Sign and publish."))?;
    let mut files = Vec::new();
    for f in release_files(&kind, &platform, &version, &name) {
        match api::public(&src, &format!("/download/{channel}/{dir}/{f}")).await? {
            Some(bytes) => files.push((f, bytes)),
            // Releases from before the release index have no gzip copy.
            None if f.ends_with(".gz") => {}
            None => return Err(format!("{src} publishes {version} but has no {f}")),
        }
    }
    let sig = String::from_utf8(files[1].1.clone()).map_err(|_| format!("{name}.sig is not text"))?;
    let packed = files.iter().find(|(f, _)| f.ends_with(".gz")).map(|(_, b)| b.clone());
    let u = Upload {
        channel,
        dir,
        platform,
        name,
        sig,
        files,
        version,
        notes: m.notes,
        mandatory: m.mandatory,
        minimum_version: m.minimum_version,
        forced: m.forced,
        pub_date: m.pub_date,
        packed,
    };
    // The newest signed release index goes to the relays that lack it (the same document: relays
    // accept an identical copy), before their latest.json.
    let live = relay_indexes(&app, &u.channel, &servers).await?;
    let best = live.iter().filter_map(|(_, i)| i.as_ref()).max_by_key(|i| i.0.serial).map(|i| (i.0.serial, i.1.clone()));
    let mut out = Vec::new();
    for sv in &servers {
        let r = if targets.contains(sv) {
            let behind = live.iter().find(|(s, _)| s == sv).is_some_and(|(_, i)| i.as_ref().map(|i| i.0.serial) < best.as_ref().map(|b| b.0));
            match (&best, behind) {
                (Some((_, raw)), true) => async {
                    upload_files(&app, &ui, sv, &u).await?;
                    app.call_on(sv, "PUT", &format!("/api/admin/release/{}/releases.json", u.channel), raw.clone(), "application/json").await?;
                    let url = put_manifest(&app, &ui, sv, &u).await;
                    emit(&ui, Progress { done: true, error: url.as_ref().err().cloned(), ..Progress::new(&u, sv, "") });
                    url
                }
                .await
                .map(|url| json!({ "url": url })),
                _ => upload_release_reported(&app, &ui, sv, &u).await.map(|url| json!({ "url": url })),
            }
        } else {
            Ok(json!({ "already": true }))
        };
        out.push(RelayResult::of(sv, r));
    }
    Ok(out)
}

// ---- release clean-up (docs/release-cleanup.md) ----

/// One version of one release folder, as the prune endpoint names it.
#[derive(Deserialize, Serialize, Clone, PartialEq, Debug)]
struct PruneVer {
    channel: String,
    /// `<app>/<platform>`
    dir: String,
    version: String,
}

/// Older versions kept besides the published one (Releases › Clean up; decision 2026-11-07: 2).
fn prune_keep(app: &App, keep: Option<usize>) -> usize {
    keep.or_else(|| app.settings()["prune"]["keep"].as_u64().map(|k| k as usize)).unwrap_or(tether_trust::release::PRUNE_KEEP).min(tether_trust::release::PRUNE_KEEP_MAX)
}

/// Every version the CDN holds (`cdn-assets.json`): the next CDN deploy re-reads them from a relay.
fn cdn_pins(assets: &cdn::Assets) -> Vec<PruneVer> {
    let mut out: Vec<PruneVer> = Vec::new();
    for a in assets.values() {
        let Some((channel, dir)) = a.group.split_once('/') else { continue };
        let v = PruneVer { channel: channel.into(), dir: dir.into(), version: a.version.clone() };
        if !out.contains(&v) {
            out.push(v);
        }
    }
    out
}

fn prune_error(e: String) -> String {
    if e.contains("HTTP 404") {
        "This relay cannot clean up releases yet: update its tether-api first.".into()
    } else {
        e
    }
}

/// The relays' plans merged per channel/folder/version. A version is offered for deletion only
/// when every relay that has it would drop it (one relay's protection wins).
fn merge_plans(keep: usize, plans: &[(String, Value)]) -> Value {
    let mut groups: BTreeMap<(String, String), (BTreeMap<String, Value>, Vec<Value>)> = BTreeMap::new();
    let mut serials: BTreeMap<String, Vec<Option<u64>>> = BTreeMap::new();
    for (server, plan) in plans {
        for g in plan["groups"].as_array().into_iter().flatten() {
            let (Some(ch), Some(dir)) = (g["channel"].as_str(), g["dir"].as_str()) else { continue };
            serials.entry(ch.to_owned()).or_default().push(g["index_serial"].as_u64());
            let (versions, published) = groups.entry((ch.into(), dir.into())).or_default();
            published.push(json!({ "server": server, "version": g["published"] }));
            for state in ["keep", "drop"] {
                for v in g[state].as_array().into_iter().flatten() {
                    let Some(ver) = v["version"].as_str() else { continue };
                    let e = versions.entry(ver.to_owned()).or_insert_with(|| json!({ "version": ver, "files": v["files"], "bytes": v["bytes"], "why": [], "relays": {} }));
                    e["relays"][server] = json!(state);
                    for w in v["why"].as_array().into_iter().flatten() {
                        let why = e["why"].as_array_mut().expect("array");
                        if !why.contains(w) {
                            why.push(w.clone());
                        }
                    }
                }
            }
        }
    }
    let (mut n, mut files, mut bytes, mut kept) = (0u64, 0u64, 0u64, 0u64);
    let groups: Vec<Value> = groups
        .into_iter()
        .map(|((channel, dir), (versions, published))| {
            let mut vs: Vec<Value> = versions.into_values().collect();
            for v in &mut vs {
                let offer = v["relays"].as_object().is_some_and(|r| r.values().all(|s| s == "drop"));
                v["offer"] = json!(offer);
                if offer {
                    n += 1;
                    files += v["files"].as_array().map_or(0, |f| f.len() as u64);
                    bytes += v["bytes"].as_u64().unwrap_or(0);
                } else {
                    kept += 1;
                }
            }
            vs.sort_by(|a, b| tether_trust::version::compare(b["version"].as_str().unwrap_or(""), a["version"].as_str().unwrap_or("")).unwrap_or(std::cmp::Ordering::Equal));
            json!({ "channel": channel, "dir": dir, "published": published, "versions": vs })
        })
        .collect();
    let index_differs = serials.values().any(|s| s.iter().any(|x| *x != s[0]));
    json!({ "keep": keep, "relays": plans.iter().map(|p| &p.0).collect::<Vec<_>>(), "index_differs": index_differs, "groups": groups,
        "offer": { "versions": n, "files": files, "bytes": bytes }, "kept_versions": kept })
}

/// Dry run on every relay (pins: every CDN version), merged. A relay that does not answer stops it.
async fn prune_plan(app: &App, keep: usize) -> R<(Value, Vec<PruneVer>)> {
    let assets: cdn::Assets = fs::read(app.path("cdn-assets.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let pin = cdn_pins(&assets);
    let body = serde_json::to_vec(&json!({ "dry_run": true, "keep": keep, "pin": pin })).unwrap();
    let mut plans = Vec::new();
    for sv in app.servers() {
        let out = app.call_on(&sv, "POST", "/api/admin/releases/prune", body.clone(), "application/json").await.map_err(|e| format!("{sv}: {}", prune_error(e)))?;
        plans.push((sv, serde_json::from_slice::<Value>(&out).map_err(err)?));
    }
    Ok((merge_plans(keep, &plans), pin))
}

/// The versions a merged plan offers.
fn offered(plan: &Value) -> Vec<PruneVer> {
    let mut out = Vec::new();
    for g in plan["groups"].as_array().into_iter().flatten() {
        for v in g["versions"].as_array().into_iter().flatten().filter(|v| v["offer"] == true) {
            out.push(PruneVer { channel: g["channel"].as_str().unwrap_or("").into(), dir: g["dir"].as_str().unwrap_or("").into(), version: v["version"].as_str().unwrap_or("").into() });
        }
    }
    out
}

/// Execute on every relay: each recomputes its plan and deletes only what it still would drop.
async fn prune_execute(app: &App, keep: usize, pin: &[PruneVer], delete: &[PruneVer]) -> Vec<RelayResult> {
    let body = serde_json::to_vec(&json!({ "dry_run": false, "keep": keep, "pin": pin, "delete": delete })).unwrap();
    let mut out = Vec::new();
    for sv in app.servers() {
        let r = app.call_on(&sv, "POST", "/api/admin/releases/prune", body.clone(), "application/json").await.map_err(prune_error);
        out.push(RelayResult::of(&sv, r.and_then(|b| serde_json::from_slice::<Value>(&b).map_err(err))));
    }
    out
}

#[tauri::command]
async fn release_prune_plan(app: State<'_, App>, keep: Option<usize>) -> R<Value> {
    app.admin_key()?;
    Ok(prune_plan(&app, prune_keep(&app, keep)).await?.0)
}

/// Deletes the reviewed versions (`delete`, ticked in the sheet) on every relay.
#[tauri::command]
async fn release_prune(app: State<'_, App>, keep: Option<usize>, delete: Vec<PruneVer>) -> R<Vec<RelayResult>> {
    let keep = prune_keep(&app, keep);
    let (_, pin) = prune_plan(&app, keep).await?;
    let out = prune_execute(&app, keep, &pin, &delete).await;
    if let Some(e) = none_ok(&out) {
        return Err(e);
    }
    Ok(out)
}

/// Setting "Clean up after each publish": the merged plan's offer, executed without a prompt.
/// One result line "Cleanup" for the publish sheet.
async fn auto_prune(app: &App) -> Option<RelayResult> {
    if app.settings()["prune"]["auto"].as_bool() != Some(true) {
        return None;
    }
    let keep = prune_keep(app, None);
    let r: R<Value> = async {
        let (plan, pin) = prune_plan(app, keep).await?;
        let delete = offered(&plan);
        if delete.is_empty() {
            return Ok(json!({ "published": ["Cleanup: nothing to delete"] }));
        }
        let out = prune_execute(app, keep, &pin, &delete).await;
        let failed: Vec<String> = out.iter().filter(|r| !r.ok).map(|r| format!("{}: {}", r.server, r.error.as_deref().unwrap_or(""))).collect();
        if !failed.is_empty() {
            return Err(failed.join("; "));
        }
        let freed = out.iter().filter_map(|r| r.detail["freed_bytes"].as_u64()).max().unwrap_or(0);
        Ok(json!({ "published": [format!("Cleanup: {} old versions deleted, {:.1} MB freed per relay (keep {keep})", delete.len(), freed as f64 / 1e6)] }))
    }
    .await;
    Some(RelayResult::of("Cleanup", r))
}

// ---- download page ----

#[tauri::command]
async fn page_render(app: State<'_, App>, title: String, intro: String, technician: bool, customer: bool) -> R<String> {
    let server = app.server();
    // Halt changes only the signed index, not latest.json.
    let ix = relay_indexes(&app, "stable", &app.servers()).await?.into_iter().filter_map(|(_, i)| i.map(|i| i.0)).max_by_key(|i| i.serial);
    let mut halted = vec![];
    let mut sections = vec![];
    for (id, name, platform) in [("windows", "Windows", release::WINDOWS), ("macos", "macOS", release::MAC)] {
        let mut offers = vec![];
        for (on, kind, label) in [(technician, "technician", "Tether for technicians"), (customer, "customer", "Tether Quick Support")] {
            if !on {
                continue;
            }
            let path = format!("/download/stable/{kind}/{platform}/latest.json");
            let m: Option<Value> = match api::public(&server, &path).await? {
                Some(b) => Some(serde_json::from_slice(&b).map_err(|_| format!("{path} is not valid JSON"))?),
                None => None,
            };
            let version = m.as_ref().and_then(|m| m["version"].as_str()).unwrap_or("?").to_owned();
            if is_halted(ix.as_ref(), kind, platform, &version) {
                halted.push(format!("{label} {version} ({name})"));
                continue;
            }
            // The updater artifact latest.json points at; uploaded before latest.json by
            // release_publish, so it exists when the manifest does.
            let artifact = m.as_ref().and_then(|m| Some(m["platforms"][platform]["url"].as_str()?.rsplit('/').next()?.to_owned()));
            let detail = match (kind, platform) {
                ("technician", release::WINDOWS) => format!("Windows installer · version {version}"),
                ("technician", _) => format!("Disk image for Apple silicon and Intel · version {version}"),
                (_, release::WINDOWS) => "Windows · no installation needed · run it when support asks you to".to_owned(),
                _ => "Mac · no installation needed · unzip and open it when support asks you to".to_owned(),
            };
            let href = page_files(kind, platform, &version, artifact.as_deref());
            offers.push(release::Offer { label, detail, href });
        }
        sections.push(release::OsSection { id, name, offers });
    }
    if !halted.is_empty() {
        return Err(format!("{} halted in the stable release index. Resume it or publish a newer version, then publish the download page.", halted.join(", ") + if halted.len() > 1 { " are" } else { " is" }));
    }
    if sections.iter().flat_map(|s| &s.offers).all(|o| o.href.is_none()) {
        return Err("There is no stable release of the offered apps yet. Publish one first.".into());
    }
    Ok(release::download_page(&title, &intro, &sections))
}

/// `version` of `kind/platform` is halted in the signed index.
fn is_halted(ix: Option<&tether_trust::release::Index>, kind: &str, platform: &str, version: &str) -> bool {
    ix.and_then(|i| i.releases.get(&release::index_key(kind, platform))).is_some_and(|e| e.halted && e.version == version)
}

/// The primary download of a release and the files that belong to it ("More files").
fn page_files(kind: &str, platform: &str, version: &str, artifact: Option<&str>) -> Option<String> {
    let at = |f: &str| format!("/download/stable/{kind}/{platform}/{f}");
    match artifact {
        None => None,
        // Mac technician: the disk image is the download (the updater archive is for updates).
        Some(_) if (kind, platform) == ("technician", release::MAC) => Some(at(&release::dmg_name(version))),
        Some(a) => Some(at(a)),
    }
}

#[tauri::command]
async fn page_publish(app: State<'_, App>, html: String) -> R<Vec<RelayResult>> {
    let out = app.fan_out("PUT", "/api/admin/download-page", html.into_bytes(), "text/html; charset=utf-8").await;
    match none_ok(&out) {
        Some(e) => Err(e),
        None => Ok(out),
    }
}

/// What a custom page template can show (docs/download-page.md): every release in the highest
/// verified (signed) stable index, keyed `kind/platform`. For the macOS technician app the page
/// offers the disk image, which is not in the index: its size and SHA-256 come from the first
/// relay's copy (a missing one leaves the release out).
#[tauri::command]
async fn page_data(app: State<'_, App>) -> R<Value> {
    let live = relay_indexes(&app, "stable", &app.servers()).await?;
    let mut out = serde_json::Map::new();
    let Some(ix) = live.into_iter().filter_map(|(_, i)| i.map(|i| i.0)).max_by_key(|i| i.serial) else { return Ok(Value::Object(out)) };
    for (key, e) in ix.releases.iter().filter(|(_, e)| !e.halted) {
        let (mut file, mut size, mut sha256, mut cdn_url) = (e.file.clone(), e.size, e.sha256.clone(), (e.urls.len() > 1).then(|| e.urls[0].clone()));
        if key == &release::index_key("technician", release::MAC) {
            file = release::dmg_name(&e.version);
            let Some((n, h)) = dmg_facts(&app.server(), &file).await? else { continue };
            (size, sha256, cdn_url) = (n, h, None);
        }
        out.insert(key.clone(), json!({ "version": e.version, "file": file, "size": size, "sha256": sha256, "notes": e.notes,
            "minimum_version": e.minimum_version, "mandatory": e.mandatory, "date": e.rollout.start, "cdn_url": cdn_url }));
    }
    Ok(Value::Object(out))
}

/// Size and SHA-256 of a disk image on the relay; file names carry the version, so one download
/// per version and session.
async fn dmg_facts(server: &str, file: &str) -> R<Option<(u64, String)>> {
    static SEEN: Mutex<BTreeMap<String, (u64, String)>> = Mutex::new(BTreeMap::new());
    let key = format!("{server}/{file}");
    if let Some(f) = SEEN.lock().unwrap().get(&key) {
        return Ok(Some(f.clone()));
    }
    let Some(b) = api::public(server, &format!("/download/stable/technician/{}/{file}", release::MAC)).await? else { return Ok(None) };
    let f = (b.len() as u64, tether_trust::release::sha256_hex(&b));
    SEEN.lock().unwrap().insert(key, f.clone());
    Ok(Some(f))
}

/// Uploaded page templates: every version is kept as `page-templates/<millis>-<name>.html`.
const PAGE_TEMPLATES: &str = "page-templates";
const PAGE_TEMPLATE_MAX: usize = 512 << 10;

fn template_file_ok(f: &str) -> bool {
    (6..=100).contains(&f.len()) && f.ends_with(".html") && !f.starts_with('.') && f.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn template_file_name(millis: u128, name: &str) -> String {
    let stem: String = name.trim_end_matches(".html").trim_end_matches(".htm").chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).take(60).collect();
    format!("{millis}-{}.html", if stem.is_empty() { "template".into() } else { stem })
}

#[tauri::command]
fn page_template_save(app: State<App>, name: String, html: String) -> R<String> {
    if html.len() > PAGE_TEMPLATE_MAX {
        return Err("The template is larger than 512 KiB (the relay's page limit).".into());
    }
    let millis = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis();
    let file = template_file_name(millis, &name);
    app.write(&format!("{PAGE_TEMPLATES}/{file}"), html.as_bytes())?;
    Ok(file)
}

/// Stored templates, newest first: `{file, size}` (the file name starts with the save time).
#[tauri::command]
fn page_template_list(app: State<App>) -> Vec<Value> {
    let mut v: Vec<(String, u64)> = fs::read_dir(app.path(PAGE_TEMPLATES))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| Some((e.file_name().into_string().ok().filter(|f| template_file_ok(f))?, e.metadata().ok()?.len())))
        .collect();
    v.sort_by(|a, b| b.0.cmp(&a.0));
    v.into_iter().map(|(file, size)| json!({ "file": file, "size": size })).collect()
}

#[tauri::command]
fn page_template_read(app: State<App>, file: String) -> R<String> {
    if !template_file_ok(&file) {
        return Err("invalid template name".into());
    }
    fs::read_to_string(app.path(&format!("{PAGE_TEMPLATES}/{file}"))).map_err(|_| format!("The stored template {file} is missing or not UTF-8."))
}

/// Why `file` must stay: it is what the published download page is built from (the page is rebuilt
/// from it after every stable release). Standard design: every template may go.
fn template_in_use(settings: &Value, file: &str) -> Option<&'static str> {
    (settings["page_design"] == "custom" && settings["page_template"] == file)
        .then_some("The published download page uses this template. Publish another template or the Standard design first.")
}

#[tauri::command]
fn page_template_delete(app: State<App>, file: String) -> R<()> {
    if !template_file_ok(&file) {
        return Err("invalid template name".into());
    }
    if let Some(why) = template_in_use(&app.settings(), &file) {
        return Err(why.into());
    }
    match fs::remove_file(app.path(&format!("{PAGE_TEMPLATES}/{file}"))) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(err(e)),
        _ => Ok(()),
    }
}

// ---- custom apps ----

/// Local copy of a custom app (the relay list has no logo or settings): `custom/<id>.json`.
#[derive(Serialize, Deserialize)]
struct CustomLocal {
    profile: CustomProfile,
    token: String,
    expires: u64,
    channel: String,
}

#[derive(Deserialize)]
struct CustomArgs {
    /// `None`: create a new app.
    id: Option<String>,
    name: String,
    accent: String,
    welcome: String,
    logo_png: String,
    layer: Layer,
    channel: String,
    /// Link valid for this many days from now (1-366).
    days: u64,
    /// Issue a new token: the old link stops working.
    new_link: bool,
}

/// Random custom app id, `[a-z0-9]{16}` (fits `CustomProfile::validate`).
fn new_custom_id() -> String {
    custom::new_token().to_ascii_lowercase().chars().filter(char::is_ascii_alphanumeric).take(16).collect()
}

fn custom_expiry(now: u64, days: u64) -> R<u64> {
    if !(1..=366).contains(&days) {
        return Err("The link can be valid for 1 to 366 days.".into());
    }
    Ok(now + days * 86_400)
}

/// The `PUT /api/admin/custom/<id>` body (docs/api.md, "Custom apps").
fn custom_body(p: &CustomProfile, key: &SigningKey, token: &str, expires: u64, channel: &str) -> R<Value> {
    p.validate()?;
    let doc = envelope::sign(&serde_json::to_vec(p).unwrap(), key);
    Ok(json!({ "profile": doc, "token": token, "expires": expires, "channel": channel }))
}

/// Create (no id) or update a custom app on every relay. Updating keeps the link unless `new_link`.
#[tauri::command]
async fn custom_save(app: State<'_, App>, args: CustomArgs) -> R<Value> {
    let a = args;
    if !release::valid_channel(&a.channel) {
        return Err("Channel: use 1-32 lowercase letters, digits or dashes.".into());
    }
    let now = api::now();
    let expires = custom_expiry(now, a.days)?;
    let existing = a.id.as_deref().and_then(|id| app.custom_local(id));
    if a.id.is_some() && existing.is_none() {
        return Err("This custom app was created on another PC; its settings are not here. Revoke it and create a new one.".into());
    }
    let id = a.id.unwrap_or_else(new_custom_id);
    let token = match existing {
        Some(e) if !a.new_link => e.token,
        _ => custom::new_token(),
    };
    let profile = CustomProfile { id: id.clone(), name: a.name.trim().to_owned(), accent: a.accent.trim().to_owned(), welcome: a.welcome.trim().to_owned(), logo_png: a.logo_png, layer: a.layer, issued_at: now };
    let body = custom_body(&profile, &app.admin_key()?, &token, expires, &a.channel)?;
    let results = app.fan_out("PUT", &format!("/api/admin/custom/{id}"), serde_json::to_vec(&body).unwrap(), "application/json").await;
    if let Some(e) = none_ok(&results) {
        return Err(e);
    }
    let local = CustomLocal { profile, token: token.clone(), expires, channel: a.channel };
    app.write(&format!("custom/{id}.json"), &serde_json::to_vec_pretty(&local).unwrap())?;
    Ok(json!({ "id": id, "link": format!("{}/q/{token}", app.server()), "relays": results }))
}

/// Add another relay's download counters into `base` (same id and token; each relay counts the
/// downloads it served itself).
fn add_downloads(base: &mut Value, other: &Value) {
    for row in base.as_array_mut().into_iter().flatten() {
        let Some(o) = other.as_array().into_iter().flatten().find(|o| o["id"] == row["id"] && o["token"] == row["token"]) else { continue };
        row["downloads"] = json!(row["downloads"].as_u64().unwrap_or(0) + o["downloads"].as_u64().unwrap_or(0));
        if o["last_download"].as_u64() > row["last_download"].as_u64() {
            row["last_download"] = o["last_download"].clone();
        }
    }
}

/// The first relay's list (download counters summed over all relays) merged with the local copies
/// (logo, colours, settings).
#[tauri::command]
async fn custom_list(app: State<'_, App>) -> R<Vec<Value>> {
    let mut server = app.call_json("GET", "/api/admin/custom", None).await?;
    let first = app.server();
    let mut lists = Vec::new();
    for sv in app.servers() {
        if let Ok(b) = app.call_on(&sv, "GET", "/api/admin/custom", Vec::new(), "application/json").await {
            lists.push(serde_json::from_slice::<Value>(&b).unwrap_or(Value::Null));
        }
    }
    // `server` is one of `lists` (the first that answered): start from zero, then add every relay.
    for row in server.as_array_mut().into_iter().flatten() {
        row["downloads"] = json!(0);
    }
    for l in &lists {
        add_downloads(&mut server, l);
    }
    let mut local: BTreeMap<String, CustomLocal> = fs::read_dir(app.path("custom"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| serde_json::from_slice::<CustomLocal>(&fs::read(e.path()).ok()?).ok())
        .map(|c| (c.profile.id.clone(), c))
        .collect();
    let now = api::now();
    let mut out = Vec::new();
    for row in server.as_array().into_iter().flatten() {
        let mut r = row.clone();
        let id = r["id"].as_str().unwrap_or_default().to_owned();
        r["link"] = json!(format!("{first}/q/{}", r["token"].as_str().unwrap_or_default()));
        r["status"] = json!(if r["expires"].as_u64().unwrap_or(0) <= now { "expired" } else { "active" });
        r["local"] = local.remove(&id).map_or(Value::Null, |c| json!(c.profile));
        out.push(r);
    }
    // Saved here but unknown to the relay (revoked from another PC, or relay data lost).
    for (id, c) in local {
        out.push(json!({ "id": id, "name": c.profile.name, "token": c.token, "expires": c.expires, "channel": c.channel, "link": format!("{first}/q/{}", c.token), "status": "not-on-relay", "local": c.profile }));
    }
    Ok(out)
}

#[tauri::command]
async fn custom_delete(app: State<'_, App>, id: String) -> R<Vec<RelayResult>> {
    if id.is_empty() || id.len() > 40 || !id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
        return Err("invalid id".into());
    }
    let out = app.fan_out("DELETE", &format!("/api/admin/custom/{id}"), Vec::new(), "").await;
    // Unknown on a relay (404) counts as revoked there.
    if out.iter().all(|r| r.ok || r.error.as_deref().is_some_and(|e| e.contains("HTTP 404"))) {
        let _ = fs::remove_file(app.path(&format!("custom/{id}.json")));
    }
    Ok(out)
}

// ---- team (technician access) ----

/// The team roster of every relay (`GET /api/admin/team`): the first answer, plus the relays
/// whose roster differs and those that failed.
#[tauri::command]
async fn team_load(app: State<'_, App>) -> R<Value> {
    let (mut first, mut differ, mut failed) = (None::<Roster>, Vec::new(), Vec::new());
    for sv in app.servers() {
        let r = app.call_on(&sv, "GET", "/api/admin/team", Vec::new(), "application/json").await;
        match r.and_then(|b| serde_json::from_slice::<Roster>(&b).map_err(|_| "unexpected team roster".to_owned())) {
            Ok(r) => match &first {
                None => first = Some(r),
                Some(f) if *f != r => differ.push(sv),
                Some(_) => {}
            },
            Err(e) => failed.push(format!("{sv}: {e}")),
        }
    }
    let roster = first.ok_or_else(|| failed.join("; "))?;
    Ok(json!({ "roster": roster, "differ": differ, "failed": failed }))
}

/// `PUT /api/admin/team` to every relay (validated here first, the relays validate again).
#[tauri::command]
async fn team_publish(app: State<'_, App>, roster: Roster) -> R<Vec<RelayResult>> {
    roster.validate()?;
    let out = app.fan_out("PUT", "/api/admin/team", serde_json::to_vec(&roster).map_err(err)?, "application/json").await;
    match none_ok(&out) {
        Some(e) => Err(e),
        None => Ok(out),
    }
}

// ---- diagnostics ----

#[derive(Serialize)]
struct DiagFile {
    name: String,
    size: u64,
}

#[tauri::command]
async fn diag_open(app: State<'_, App>, ticket: String) -> R<Value> {
    if !diag::valid_ticket(&ticket) {
        return Err("invalid ticket".into());
    }
    // The first relay that has the upload (call() tries the next one on a 404).
    let zip = app.call("GET", &format!("/api/admin/diag/{ticket}"), vec![], "").await?;
    let mut ar = zip::ZipArchive::new(Cursor::new(&zip)).map_err(|e| format!("The upload is not a valid zip: {e}"))?;
    let mut files = vec![];
    let mut diagnostics = None;
    for i in 0..ar.len() {
        let mut f = ar.by_index(i).map_err(err)?;
        files.push(DiagFile { name: f.name().to_owned(), size: f.size() });
        if f.name().rsplit('/').next() == Some("diagnostics.json") && diagnostics.is_none() {
            let mut s = String::new();
            // Bounded read: a zip bomb must not blow up the app.
            (&mut f).take(4 << 20).read_to_string(&mut s).map_err(err)?;
            diagnostics = Some(serde_json::from_str::<Value>(&s).map(|v| serde_json::to_string_pretty(&v).unwrap()).unwrap_or(s));
        }
    }
    let size = zip.len();
    *app.diag.lock().unwrap() = Some(zip);
    Ok(json!({"files": files, "diagnostics": diagnostics, "size": size}))
}

/// Tickets of every relay, merged by code (`diag::merge`): active ones, or with `archived` the
/// closed ones. `failed`: relays that did not answer.
#[tauri::command]
async fn diag_list(app: State<'_, App>, archived: bool) -> R<Value> {
    let path = if archived { "/api/admin/diag?archived=1" } else { "/api/admin/diag" };
    let (mut lists, mut failed) = (Vec::new(), Vec::new());
    for sv in app.servers() {
        match app.call_on(&sv, "GET", path, Vec::new(), "application/json").await {
            Ok(b) => lists.push((sv.clone(), serde_json::from_slice::<Vec<Value>>(&b).map_err(|_| format!("{sv}: unexpected ticket list"))?)),
            Err(e) => failed.push(format!("{sv}: {e}")),
        }
    }
    if lists.is_empty() {
        return Err(failed.join("; "));
    }
    Ok(json!({ "tickets": diag::merge(lists, archived, api::now()), "failed": failed }))
}

/// Close (`close`: archive, no more uploads) or delete (for good, with the zip) a ticket on every
/// relay; one result per relay. A relay without the ticket counts as done.
async fn diag_change(app: &App, ticket: &str, close: bool) -> R<Vec<RelayResult>> {
    if !diag::valid_ticket(ticket) {
        return Err("invalid ticket".into());
    }
    let (method, path) = if close { ("POST", format!("/api/admin/diag/{ticket}/close")) } else { ("DELETE", format!("/api/admin/diag/{ticket}")) };
    let mut out = app.fan_out(method, &path, Vec::new(), "application/json").await;
    for r in &mut out {
        if r.error.as_deref().is_some_and(|e| e.contains("(HTTP 404")) {
            *r = RelayResult::of(&r.server, Ok(json!("not on this relay")));
        }
    }
    if let Some(e) = none_ok(&out) {
        return Err(e);
    }
    Ok(out)
}

#[tauri::command]
async fn diag_close(app: State<'_, App>, ticket: String) -> R<Vec<RelayResult>> {
    diag_change(&app, &ticket, true).await
}

#[tauri::command]
async fn diag_delete(app: State<'_, App>, ticket: String) -> R<Vec<RelayResult>> {
    diag_change(&app, &ticket, false).await
}

#[tauri::command]
fn diag_save(app: State<App>, path: String) -> R<()> {
    let d = app.diag.lock().unwrap();
    fs::write(path, d.as_ref().ok_or("Open a diagnostics upload first.")?).map_err(err)
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // TETHER_MANAGE_DIR: a separate vault (tests, or several deployments on one PC).
            let dir = match std::env::var_os("TETHER_MANAGE_DIR") {
                Some(d) => std::path::PathBuf::from(d),
                None => app.path().data_dir()?.join("Tether-Manage"),
            };
            fs::create_dir_all(&dir)?;
            app.manage(App { dir, keys: Mutex::new(None), diag: Mutex::new(None) });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            state,
            keys_create,
            keys_unlock,
            keys_lock,
            keys_export,
            keys_import,
            settings_set,
            admin,
            relays_check,
            devices_all,
            audit_all,
            text_save,
            features,
            serverkeys_generate,
            serverkeys_live,
            serverkeys_rotation_set,
            serverkeys_publish,
            serverkeys_save_files,
            serverkeys_delete_files,
            policy_draft,
            policy_save_draft,
            policy_check,
            policy_preview,
            policy_live,
            policy_diff,
            policy_publish,
            policy_sync,
            policy_history,
            policy_local,
            release_publish,
            release_index,
            release_control,
            release_index_refresh,
            cdn_state,
            cdn_configure,
            releases_overview,
            release_sync,
            release_prune_plan,
            release_prune,
            release_scan,
            page_render,
            page_publish,
            page_data,
            page_template_save,
            page_template_list,
            page_template_read,
            page_template_delete,
            custom_save,
            custom_list,
            custom_delete,
            team_load,
            team_publish,
            diag_open,
            diag_list,
            diag_close,
            diag_delete,
            diag_save
        ])
        .run(tauri::generate_context!())
        .expect("error while running Tether Manage");
}

#[cfg(test)]
mod server_list_tests {
    use super::*;

    fn app_with(server: Option<&str>) -> App {
        let dir = std::env::temp_dir().join(format!("tether-manage-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let app = App { dir, keys: Mutex::new(None), diag: Mutex::new(None) };
        let s = match server {
            Some(v) => json!({ "server": v }),
            None => json!({}),
        };
        app.write("settings.json", &serde_json::to_vec(&s).unwrap()).unwrap();
        app
    }

    #[test]
    fn several_relays_in_order_default_when_empty() {
        let a = app_with(Some(" https://a.example/ , https://b.example,,"));
        assert_eq!(a.servers(), vec!["https://a.example", "https://b.example"]);
        assert_eq!(a.server(), "https://a.example");
        let b = app_with(None);
        assert_eq!(b.servers(), vec![DEFAULT_SERVER]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signed_index(key: &SigningKey, serial: u64, versions: &[(&str, &str, bool)]) -> Vec<u8> {
        let m = |v: &str| release::Manifest { version: v.into(), notes: String::new(), pub_date: String::new(), platforms: BTreeMap::new(), mandatory: false, minimum_version: String::new(), forced: false };
        let ix = release::next_index(None, "stable", serial, 1000, |r| {
            for (dir, v, halted) in versions {
                let mut e = release::entry("stable", dir, "f", b"x", "S", None, None, &m(v), false, release::rollout("now", 0).unwrap());
                e.halted = *halted;
                r.insert((*dir).into(), e);
            }
        })
        .unwrap();
        serde_json::to_vec(&envelope::sign(&serde_json::to_vec(&ix).unwrap(), key)).unwrap()
    }

    #[test]
    fn indexes_signed_with_a_rotated_admin_key_are_built_on_or_refused() {
        let (old, new) = (tether_trust::generate_key(), tether_trust::generate_key());
        let raw = signed_index(&old, 7, &[("technician/windows-x86_64", "0.5.24", false), ("customer/windows-x86_64", "0.5.24", false)]);
        let got = || vec![("https://a".to_owned(), Some(raw.clone())), ("https://b".to_owned(), None)];
        // Built with the old key listed (the apps' TETHER_ADMIN_PUB_KEY): base and serial come from it.
        let keys = trusted_admin_keys(new.verifying_key(), &format!("{}, {} ,", tether_trust::key_to_b64(&old.verifying_key()), tether_trust::key_to_b64(&new.verifying_key())));
        assert_eq!(keys.len(), 2);
        let live = usable_indexes(open_indexes(got(), &keys, "stable"), "stable").unwrap();
        let base = live.iter().find_map(|(_, i)| i.as_ref()).unwrap();
        assert_eq!((base.0.serial, base.0.releases.len()), (7, 2));
        assert!(live[1].1.is_none());
        // Only this PC's key: the relay's index is not silently dropped, the publish is refused.
        let e = usable_indexes(open_indexes(got(), &trusted_admin_keys(new.verifying_key(), ""), "stable"), "stable").unwrap_err();
        assert!(e.contains("https://a") && e.contains("TETHER_ADMIN_PUB_KEY"), "{e}");
    }

    #[test]
    fn the_index_goes_only_to_relays_that_serve_all_its_files() {
        let k = tether_trust::generate_key();
        let ix = tether_trust::release::Index::open(&signed_index(&k, 1, &[("technician/windows-x86_64", "0.5.25", false), ("customer/windows-x86_64", "0.5.24", false)]), &[k.verifying_key()], "stable").unwrap();
        let paths = relay_paths(&ix);
        assert_eq!(paths, ["/download/stable/customer/windows-x86_64/f", "/download/stable/technician/windows-x86_64/f"]);
        let (a, b, c) = ("https://a".to_owned(), "https://b".to_owned(), "https://c".to_owned());
        let missing = BTreeMap::from([(a.clone(), vec![]), (b.clone(), vec![paths[1].clone()])]);
        let (send, skipped) = index_targets(&[a.clone(), b.clone(), c.clone()], &missing);
        assert_eq!(send, [a, c]);
        let e = skipped[&b].as_ref().unwrap_err();
        assert!(e.contains(&paths[1]) && e.contains("Publish to missing relays"), "{e}");
        assert_eq!(skipped.len(), 1);
    }

    #[test]
    fn halted_releases_are_not_offered_on_the_download_page() {
        let k = tether_trust::generate_key();
        let ix = tether_trust::release::Index::open(&signed_index(&k, 1, &[("technician/windows-x86_64", "0.5.24", true), ("customer/windows-x86_64", "0.5.24", false)]), &[k.verifying_key()], "stable").unwrap();
        assert!(is_halted(Some(&ix), "technician", release::WINDOWS, "0.5.24"));
        assert!(!is_halted(Some(&ix), "technician", release::WINDOWS, "0.5.23"), "latest.json already points elsewhere");
        assert!(!is_halted(Some(&ix), "customer", release::WINDOWS, "0.5.24"));
        assert!(!is_halted(None, "technician", release::WINDOWS, "0.5.24"));
    }

    #[test]
    fn the_published_template_cannot_be_deleted() {
        let s = json!({ "page_design": "custom", "page_template": "1-a.html" });
        assert!(template_in_use(&s, "1-a.html").is_some());
        assert!(template_in_use(&s, "2-b.html").is_none(), "other versions may go");
        let standard = json!({ "page_design": "standard", "page_template": "1-a.html" });
        assert!(template_in_use(&standard, "1-a.html").is_none(), "Standard published: the kept choice may go");
        assert!(template_in_use(&json!({}), "1-a.html").is_none());
    }

    #[test]
    fn page_template_names_stay_in_their_folder() {
        assert_eq!(template_file_name(1, "My Page (v2).html"), "1-My_Page__v2_.html");
        assert_eq!(template_file_name(1, "../../x.htm"), "1-______x.html");
        assert_eq!(template_file_name(1, ".html"), "1-template.html");
        assert!(template_file_ok(&template_file_name(1, "a b")) && template_file_ok("1-a.html"));
        assert!(!template_file_ok("../a.html") && !template_file_ok("a/b.html") && !template_file_ok("a.txt") && !template_file_ok(".x.html"));
    }

    #[test]
    fn build_folder_scan_two_levels_deep() {
        let root = std::env::temp_dir().join(format!("tether-scan-{}", std::process::id()));
        for (dir, f) in [("windows", "Tether_0.5.1_x64-setup.exe"), ("quick", "TetherQS-0.5.1.exe"), ("a/b", "TetherQS-0.4.0.exe"), ("a/b/c", "TetherQS-9.9.9.exe")] {
            fs::create_dir_all(root.join(dir)).unwrap();
            fs::write(root.join(dir).join(f), b"x").unwrap();
        }
        let mut files = Vec::new();
        files_below(&root, 2, &mut files);
        let b = release::latest_builds(files);
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(b[&("technician", release::WINDOWS)].0, "0.5.1");
        // a/b/c is three levels deep: not searched.
        assert_eq!(b[&("customer", release::WINDOWS)], ("0.5.1".to_owned(), root.join("quick").join("TetherQS-0.5.1.exe")));
    }

    #[test]
    fn custom_downloads_sum_over_relays_same_token_only() {
        let mut base = json!([{"id":"a","token":"t1","downloads":0,"last_download":null}]);
        add_downloads(&mut base, &json!([{"id":"a","token":"t1","downloads":2,"last_download":5}]));
        add_downloads(&mut base, &json!([{"id":"a","token":"t1","downloads":1,"last_download":9}]));
        add_downloads(&mut base, &json!([{"id":"a","token":"old","downloads":7,"last_download":99}]));
        assert_eq!(base[0]["downloads"], 3);
        assert_eq!(base[0]["last_download"], 9);
    }

    #[test]
    fn custom_app_body_is_what_the_relay_and_tetherqs_accept() {
        let key = tether_trust::generate_key();
        let id = new_custom_id();
        assert_eq!(id.len(), 16);
        assert_ne!(id, new_custom_id());
        let p = CustomProfile { id: id.clone(), name: "ACME Support".into(), issued_at: 1, ..Default::default() };
        let token = custom::new_token();
        let body = custom_body(&p, &key, &token, 99, "stable").unwrap();
        assert!(custom::valid_token(body["token"].as_str().unwrap()));
        assert_eq!((body["expires"].as_u64(), body["channel"].as_str()), (Some(99), Some("stable")));
        // The profile verifies with the admin key and passes CustomProfile::validate.
        let doc = serde_json::to_vec(&body["profile"]).unwrap();
        assert_eq!(custom::open(&doc, &[key.verifying_key()]).unwrap(), p);
        // Invalid profiles are refused before anything is sent.
        assert!(custom_body(&CustomProfile { name: " ".into(), ..p.clone() }, &key, &token, 99, "stable").is_err());
        assert!(custom_body(&CustomProfile { accent: "red".into(), ..p }, &key, &token, 99, "stable").is_err());
    }

    #[test]
    fn link_expiry_is_bounded() {
        assert_eq!(custom_expiry(1000, 30).unwrap(), 1000 + 30 * 86_400);
        assert!(custom_expiry(0, 366).is_ok());
        assert!(custom_expiry(0, 0).is_err() && custom_expiry(0, 367).is_err());
    }

    #[test]
    fn files_that_belong_to_a_release() {
        assert_eq!(release_files("technician", release::WINDOWS, "0.3.1", "Tether_0.3.1_x64-setup.exe"), ["Tether_0.3.1_x64-setup.exe", "Tether_0.3.1_x64-setup.exe.sig"]);
        assert_eq!(release_files("customer", release::WINDOWS, "0.3.1", "TetherQS-0.3.1.exe"), ["TetherQS-0.3.1.exe", "TetherQS-0.3.1.exe.sig", "TetherQS-0.3.1.exe.gz"]);
        assert_eq!(
            release_files("technician", release::MAC, "0.3.1", "Tether_0.3.1_universal.app.tar.gz"),
            ["Tether_0.3.1_universal.app.tar.gz", "Tether_0.3.1_universal.app.tar.gz.sig", "Tether_0.3.1_universal.dmg", "Tether_0.3.1_universal.dmg.sig"]
        );
        let href = page_files("technician", release::MAC, "0.3.1", Some("Tether_0.3.1_universal.app.tar.gz"));
        assert_eq!(href.unwrap(), "/download/stable/technician/darwin-universal/Tether_0.3.1_universal.dmg");
        let href = page_files("customer", release::WINDOWS, "0.3.1", Some("TetherQS-0.3.1.exe"));
        assert_eq!(href.unwrap(), "/download/stable/customer/windows-x86_64/TetherQS-0.3.1.exe");
        assert_eq!(page_files("customer", release::MAC, "0.3.1", None), None);
    }

    /// Writes the browser mock's download page preview:
    /// `TETHER_SAMPLE_PAGE=../src/lib/mock-page.html cargo test sample_page -- --ignored`
    #[test]
    #[ignore]
    fn sample_page() {
        let offer = |label, kind, platform, artifact: Option<&str>, detail: &str| {
            let href = page_files(kind, platform, "0.4.0", artifact);
            release::Offer { label, detail: detail.into(), href }
        };
        let sections = [
            release::OsSection {
                id: "windows",
                name: "Windows",
                offers: vec![
                    offer("Tether for technicians", "technician", release::WINDOWS, Some("Tether_0.4.0_x64-setup.exe"), "Windows installer · version 0.4.0"),
                    offer("Tether Quick Support", "customer", release::WINDOWS, Some("TetherQS-0.4.0.exe"), "Windows · no installation needed · run it when support asks you to"),
                ],
            },
            release::OsSection {
                id: "macos",
                name: "macOS",
                offers: vec![
                    offer("Tether for technicians", "technician", release::MAC, Some("Tether_0.4.0_universal.app.tar.gz"), "Disk image for Apple silicon and Intel · version 0.4.0"),
                    offer("Tether Quick Support", "customer", release::MAC, None, ""),
                ],
            },
        ];
        let html = release::download_page("Remote support", "Download Tether to get help from our support team.", &sections);
        fs::write(std::env::var("TETHER_SAMPLE_PAGE").expect("TETHER_SAMPLE_PAGE"), html).unwrap();
    }

    #[test]
    fn unverified_header_reads_serial_of_a_foreign_document() {
        let other = tether_trust::generate_key();
        let raw = serde_json::to_vec(&envelope::sign(br#"{"serial":3,"issued_at":7}"#, &other)).unwrap();
        assert_eq!(unverified_header(&raw), Some((3, 7)));
        assert_eq!(unverified_header(b"nope"), None);
    }

    #[test]
    fn policy_serial_ignores_what_a_relay_claims() {
        let admin = tether_trust::generate_key();
        let key = admin.verifying_key();
        let signed = |serial: u64, k: &SigningKey| serde_json::to_vec(&envelope::sign(&serde_json::to_vec(&Bundle { serial, ..Default::default() }).unwrap(), k)).unwrap();
        // What a malicious relay can do: report u64::MAX in /api/admin/status (no longer read),
        // serve it unsigned, or signed with its own key. None of it counts.
        let unsigned = serde_json::to_vec(&Bundle { serial: u64::MAX, ..Default::default() }).unwrap();
        let foreign = signed(u64::MAX, &tether_trust::generate_key());
        let ours = signed(4, &admin);
        let live: Vec<u64> = [unsigned, foreign, ours].iter().filter_map(|raw| verified_policy(raw, &key).ok().map(|b| b.serial)).collect();
        assert_eq!(live, [4]);
        // This PC published up to 6: the next serial is 7.
        assert_eq!(next_serial(live.into_iter().chain([2, 6]), "policy").unwrap(), 7);
        // Only a document this admin key really signed with u64::MAX can exhaust it, with a clear error.
        assert!(verified_policy(&signed(u64::MAX, &admin), &key).is_ok());
        assert!(next_serial([u64::MAX], "policy").unwrap_err().contains("policy serial is exhausted"));
    }

    #[test]
    fn signing_key_answer_must_be_one_key() {
        let k = tether_trust::key_to_b64(&tether_trust::generate_key().verifying_key());
        assert_eq!(parse_signing_key(format!("{k}\n").as_bytes()), Some(k.clone()));
        assert_eq!(parse_signing_key(b"Tether relay"), None);
        assert_eq!(parse_signing_key(format!("{k}{k}").as_bytes()), None);
    }

    #[test]
    fn prune_plans_merge_one_relay_protects() {
        let v = |ver: &str, why: &[&str]| json!({ "version": ver, "files": [format!("TetherQS-{ver}.exe"), format!("TetherQS-{ver}.exe.sig")], "bytes": 20, "why": why });
        let plan = |serial: u64, keep: Vec<Value>, drop: Vec<Value>| json!({ "groups": [{ "channel": "stable", "dir": "customer/windows-x86_64", "published": "0.5.24", "index_serial": serial, "keep": keep, "drop": drop }] });
        let a = plan(8, vec![v("0.5.24", &["index"])], vec![v("0.5.21", &[]), v("0.5.22", &[]), v("0.5.9", &[])]);
        // Relay b still publishes 0.5.22 (lagging) and never had 0.5.9.
        let b = plan(7, vec![v("0.5.24", &["newer"]), v("0.5.22", &["index"])], vec![v("0.5.21", &[])]);
        let m = merge_plans(2, &[("https://a".into(), a), ("https://b".into(), b)]);
        let vs = m["groups"][0]["versions"].as_array().unwrap();
        assert_eq!(vs.iter().map(|v| (v["version"].as_str().unwrap(), v["offer"] == true)).collect::<Vec<_>>(), [("0.5.24", false), ("0.5.22", false), ("0.5.21", true), ("0.5.9", true)]);
        assert_eq!(vs[0]["why"], json!(["index", "newer"]));
        assert_eq!(vs[1]["relays"], json!({ "https://a": "drop", "https://b": "keep" }));
        assert_eq!((m["offer"]["versions"].as_u64(), m["offer"]["files"].as_u64(), m["offer"]["bytes"].as_u64(), m["kept_versions"].as_u64()), (Some(2), Some(4), Some(40), Some(2)));
        assert_eq!(m["index_differs"], true);
        let d = |v: &str| PruneVer { channel: "stable".into(), dir: "customer/windows-x86_64".into(), version: v.into() };
        assert_eq!(offered(&m), vec![d("0.5.21"), d("0.5.9")]);
        // Same serials: no "sync first".
        let same = merge_plans(2, &[("https://a".into(), plan(8, vec![], vec![])), ("https://b".into(), plan(8, vec![], vec![]))]);
        assert_eq!(same["index_differs"], false);
    }

    #[test]
    fn cdn_assets_pin_their_versions() {
        let a = |g: &str, v: &str| cdn::Asset { hash: "h".into(), size: 1, group: g.into(), version: v.into() };
        let assets: cdn::Assets = [
            ("/stable/customer/windows-x86_64/TetherQS-0.5.24.exe.gz".to_owned(), a("stable/customer/windows-x86_64", "0.5.24")),
            ("/stable/customer/windows-x86_64/TetherQS-0.5.23.exe.gz".to_owned(), a("stable/customer/windows-x86_64", "0.5.23")),
            ("/stable/customer/windows-x86_64/TetherQS-0.5.23.exe.sig".to_owned(), a("stable/customer/windows-x86_64", "0.5.23")),
            ("/stable/technician/windows-x86_64/Tether_0.5.24_x64-setup.exe".to_owned(), a("stable/technician/windows-x86_64", "0.5.24")),
        ]
        .into_iter()
        .collect();
        let pins = cdn_pins(&assets);
        assert_eq!(pins.len(), 3);
        assert!(pins.contains(&PruneVer { channel: "stable".into(), dir: "customer/windows-x86_64".into(), version: "0.5.23".into() }));
        assert!(prune_error("Not found on the server. (HTTP 404: not found)".into()).contains("update its tether-api"));
    }

    #[test]
    fn none_ok_only_when_every_relay_failed() {
        let ok = RelayResult::of("a", Ok(Value::Null));
        assert!(none_ok(&[ok, RelayResult::of("b", Err("down".into()))]).is_none());
        assert_eq!(none_ok(&[RelayResult::of("b", Err("down".into()))]).unwrap(), "b: down");
    }

    #[test]
    fn server_key_serial_only_from_verified_lists_and_expiry_ahead() {
        // A relay serving or claiming a huge serial it could not get signed must not count.
        assert_eq!(verified_serial(&json!({ "verified": false, "serial": u64::MAX })), None);
        assert_eq!(verified_serial(&json!({ "reachable": true, "note": "none" })), None);
        assert_eq!(verified_serial(&json!({ "verified": true, "serial": 4 })), Some(4));
        assert_eq!(next_serial([3, 9, 4], "x").unwrap(), 10);
        assert_eq!(next_serial([], "x").unwrap(), 1);
        assert!(next_serial([u64::MAX], "x").unwrap_err().contains("exhausted"));
        let k = |expires| ServerKey { key: "k".repeat(44), not_before: 0, expires };
        assert!(check_expiry(&[k(0)], 1000).is_ok());
        assert!(check_expiry(&[k(1000 + 7200), k(0)], 1000).is_ok());
        assert!(check_expiry(&[k(999), k(0)], 1000).is_err(), "already passed");
        assert!(check_expiry(&[k(1000 + 60), k(0)], 1000).is_err(), "too soon");
    }
}

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod keys;
mod live;
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
use tether_trust::{envelope, SigningKey, VerifyingKey};

const DEFAULT_SERVER: &str = "https://tether-desk.duckdns.org";

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
    async fn call_on(&self, server: &str, method: &str, path: &str, body: Vec<u8>, ct: &str) -> R<Vec<u8>> {
        api::admin(server, &self.admin_key()?, method, path, body, ct).await
    }
    /// Reads go to the first node that answers; writes go to every node (each keeps a full copy of
    /// policy, releases, download page and tickets), so any node can serve clients alone.
    async fn call(&self, method: &str, path: &str, body: Vec<u8>, ct: &str) -> R<Vec<u8>> {
        let servers = self.servers();
        if method.eq_ignore_ascii_case("GET") {
            let mut last = String::new();
            for sv in &servers {
                match self.call_on(sv, method, path, body.clone(), ct).await {
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
    async fn fan_out(&self, method: &str, path: &str, body: Vec<u8>, ct: &str) -> Vec<RelayResult> {
        let mut out = Vec::new();
        for sv in self.servers() {
            let r = self.call_on(&sv, method, path, body.clone(), ct).await;
            out.push(RelayResult::of(&sv, r.map(|_| Value::Null)));
        }
        out
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
    app.admin_key()?;
    let mut out = Vec::new();
    for sv in app.servers() {
        let r = app.call_on(&sv, "GET", "/api/admin/status", Vec::new(), "application/json").await;
        out.push(RelayResult::of(&sv, r.map(|b| serde_json::from_slice(&b).unwrap_or(Value::Null))));
    }
    Ok(out)
}

/// Devices from every relay, merged by uuid (clients heartbeat to whichever relay they use).
#[tauri::command]
async fn devices_all(app: State<'_, App>) -> R<Value> {
    let (mut lists, mut failed) = (Vec::new(), Vec::new());
    for sv in app.servers() {
        match app.call_on(&sv, "GET", "/api/admin/devices", Vec::new(), "application/json").await {
            Ok(b) => lists.push(serde_json::from_slice::<Vec<Value>>(&b).map_err(|_| format!("{sv}: unexpected device list"))?),
            Err(e) => failed.push(format!("{sv}: {e}")),
        }
    }
    if lists.is_empty() {
        return Err(failed.join("; "));
    }
    Ok(json!({ "devices": live::merge_devices(lists), "failed": failed }))
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

async fn live_doc(server: &str, key: &VerifyingKey) -> LiveDoc {
    let mut d = LiveDoc { raw: None, bundle: None, serial: None, issued_at: None, note: None };
    match api::public(server, "/api/policy").await {
        Ok(None) => d.note = Some("No policy published".into()),
        Ok(Some(raw)) => {
            match envelope::open_json(&raw, &[*key]).and_then(|p| Bundle::from_json(&p)) {
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
    let key = app.admin_key()?.verifying_key();
    let servers = app.servers();
    let mut relays = Vec::new();
    let mut best: Option<Bundle> = None;
    for sv in &servers {
        let reach = app.call_on(sv, "GET", "/api/admin/status", Vec::new(), "application/json").await.map(|_| ());
        let d = live_doc(sv, &key).await;
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
    let mut server = 0;
    for sv in app.servers() {
        // An unreachable relay gets the document later (policy_sync); the serial only has to be
        // above what every reachable relay has.
        if let Ok(out) = app.call_on(&sv, "GET", "/api/admin/status", Vec::new(), "application/json").await {
            let status: Value = serde_json::from_slice(&out).unwrap_or(Value::Null);
            server = server.max(status["policy_serial"].as_u64().unwrap_or(0));
        }
    }
    let local = app.published().keys().next_back().copied().unwrap_or(0);
    b.serial = server.max(local) + 1;
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
    // Artifacts first, manifest last: clients never see a latest.json pointing at a missing file.
    for (f, bytes) in &u.files {
        let ct = if f.ends_with(".sig") { "text/plain" } else { "application/octet-stream" };
        let p = Progress { total: bytes.len() as u64, ..Progress::new(u, server, f) };
        emit(ui, p.clone());
        let ui = ui.clone();
        api::admin_upload(server, &app.admin_key()?, &format!("{base}/{f}"), bytes.clone(), ct, move |sent| emit(&ui, Progress { sent, ..p.clone() })).await?;
    }
    emit(ui, Progress::new(u, server, "latest.json"));
    app.call_on(server, "PUT", &format!("{base}/latest.json"), serde_json::to_vec_pretty(&manifest).unwrap(), "application/json").await?;
    Ok(url)
}

/// Every file of one release, in upload order: the artifact, its signature, and for the mac
/// technician app the manual-download disk image with its signature.
fn release_files(kind: &str, platform: &str, version: &str, name: &str) -> Vec<String> {
    let mut v = vec![name.to_owned(), format!("{name}.sig")];
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
    };
    let mut out = Vec::new();
    for server in app.servers() {
        let r = upload_release_reported(&app, &ui, &server, &u).await;
        out.push(RelayResult::of(&server, r.map(|url| json!({ "url": url }))));
    }
    if let Some(e) = none_ok(&out) {
        return Err(e);
    }
    Ok(out)
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
    for ((kind, platform), (version, path)) in release::latest_builds(files) {
        let (dir, _) = release::target(kind, platform, &version)?;
        let mut published: Option<String> = None;
        for sv in app.servers() {
            let Ok(Some(b)) = api::public(&sv, &format!("/download/{channel}/{dir}/latest.json")).await else { continue };
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
    for sv in &servers {
        match app.call_on(sv, "GET", "/api/admin/releases", Vec::new(), "application/json").await {
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
    for ((channel, kind, platform), version) in latest {
        let (dir, name) = release::target(kind, platform, &version)?;
        let mut relays = Vec::new();
        for (sv, listing) in servers.iter().zip(&listings) {
            let manifest = api::public(sv, &format!("/download/{channel}/{dir}/latest.json")).await;
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
        let bytes = api::public(&src, &format!("/download/{channel}/{dir}/{f}")).await?.ok_or_else(|| format!("{src} publishes {version} but has no {f}"))?;
        files.push((f, bytes));
    }
    let sig = String::from_utf8(files[1].1.clone()).map_err(|_| format!("{name}.sig is not text"))?;
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
    };
    let mut out = Vec::new();
    for sv in &servers {
        let r = if targets.contains(sv) { upload_release_reported(&app, &ui, sv, &u).await.map(|url| json!({ "url": url })) } else { Ok(json!({ "already": true })) };
        out.push(RelayResult::of(sv, r));
    }
    Ok(out)
}

// ---- download page ----

#[tauri::command]
async fn page_render(app: State<'_, App>, title: String, intro: String, technician: bool, customer: bool) -> R<String> {
    let server = app.server();
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
    if sections.iter().flat_map(|s| &s.offers).all(|o| o.href.is_none()) {
        return Err("There is no stable release of the offered apps yet. Publish one first.".into());
    }
    Ok(release::download_page(&title, &intro, &sections))
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

// ---- diagnostics ----

#[derive(Serialize)]
struct DiagFile {
    name: String,
    size: u64,
}

#[tauri::command]
async fn diag_open(app: State<'_, App>, ticket: String) -> R<Value> {
    if ticket.len() != 8 || !ticket.bytes().all(|b| b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789".contains(&b)) {
        return Err("invalid ticket".into());
    }
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
            features,
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
            releases_overview,
            release_sync,
            release_scan,
            page_render,
            page_publish,
            custom_save,
            custom_list,
            custom_delete,
            diag_open,
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
    fn none_ok_only_when_every_relay_failed() {
        let ok = RelayResult::of("a", Ok(Value::Null));
        assert!(none_ok(&[ok, RelayResult::of("b", Err("down".into()))]).is_none());
        assert_eq!(none_ok(&[RelayResult::of("b", Err("down".into()))]).unwrap(), "b: down");
    }
}

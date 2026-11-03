#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod keys;
mod release;

use keys::Keys;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Read};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{Manager, State};
use tether_trust::policy::{self, Bundle, Effective};
use tether_trust::{envelope, SigningKey};

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
        self.settings()["server"].as_str().unwrap_or(DEFAULT_SERVER).to_owned()
    }
    fn admin_key(&self) -> R<SigningKey> {
        self.keys.lock().unwrap().as_ref().map(|k| k.admin.clone()).ok_or_else(|| "Unlock the keys first.".to_owned())
    }
    async fn call(&self, method: &str, path: &str, body: Vec<u8>, ct: &str) -> R<Vec<u8>> {
        api::admin(&self.server(), &self.admin_key()?, method, path, body, ct).await
    }
    async fn call_json(&self, method: &str, path: &str, body: Option<Value>) -> R<Value> {
        let body = body.map(|b| serde_json::to_vec(&b).unwrap()).unwrap_or_default();
        let out = self.call(method, path, body, "application/json").await?;
        if out.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&out).map_err(|_| format!("The server sent an unexpected answer: {}", String::from_utf8_lossy(&out).chars().take(200).collect::<String>()))
    }
    fn published(&self) -> BTreeMap<u64, PathBuf> {
        fs::read_dir(self.path("policy/published"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| Some((e.path().file_stem()?.to_str()?.parse().ok()?, e.path())))
            .collect()
    }
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
    if !server.is_empty() && !server.starts_with("https://") {
        return Err("The server address must start with https://".into());
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

#[tauri::command]
async fn policy_publish(app: State<'_, App>, bundle: Bundle) -> R<u64> {
    let mut b = bundle;
    b.validate()?;
    let status = app.call_json("GET", "/api/admin/status", None).await?;
    let server = status["policy_serial"].as_u64().unwrap_or(0);
    let local = app.published().keys().next_back().copied().unwrap_or(0);
    b.serial = server.max(local) + 1;
    b.issued_at = api::now();
    let payload = serde_json::to_vec(&b).unwrap();
    let doc = envelope::sign(&payload, &app.admin_key()?);
    app.call("PUT", "/api/admin/policy", serde_json::to_vec(&doc).unwrap(), "application/json").await?;
    app.write(&format!("policy/published/{}.json", b.serial), &payload)?;
    app.write("policy/draft.json", &serde_json::to_vec_pretty(&b).unwrap())?;
    Ok(b.serial)
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

#[tauri::command]
async fn release_publish(app: State<'_, App>, args: ReleaseArgs) -> R<String> {
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
    // The mac technician release also carries the .dmg for manual downloads (download page).
    let dmg = if (a.kind.as_str(), a.platform.as_str()) == ("technician", release::MAC) {
        let dmg_name = release::dmg_name(&a.version);
        let path = std::path::Path::new(&a.file).with_file_name(&dmg_name);
        let bytes = fs::read(&path).map_err(|_| format!("Put {dmg_name} next to the .app.tar.gz (scripts/build-macos.sh writes both to dist/macos)."))?;
        Some((dmg_name, bytes))
    } else {
        None
    };
    let sign = |bytes: &[u8], file: &str| -> R<String> {
        let k = app.keys.lock().unwrap();
        k.as_ref().ok_or("Unlock the keys first.")?.sign_update(bytes, file, &a.version)
    };
    let sig = sign(&data, &name)?;
    let dmg_sig = dmg.as_ref().map(|(n, b)| sign(b, n)).transpose()?;
    let server = app.server();
    let base = format!("/api/admin/release/{}/{dir}", a.channel);
    let url = format!("{}/download/{}/{dir}/{name}", server.trim_end_matches('/'), a.channel);
    let new = release::platform_keys(&a.platform)
        .iter()
        .map(|k| (k.to_string(), release::Platform { signature: sig.clone(), url: url.clone() }))
        .collect();
    // Read before writing: another OS's build of the same version in this manifest stays listed.
    let existing = api::public(&server, &format!("/download/{}/{dir}/latest.json", a.channel)).await?;
    let manifest = release::Manifest {
        version: a.version.clone(),
        notes: a.notes,
        pub_date: release::rfc3339(api::now()),
        platforms: release::merge_platforms(existing.as_deref(), &a.version, new),
        mandatory: a.mandatory,
        minimum_version: a.minimum_version,
        forced: a.forced,
    };
    // Artifacts first, manifest last: clients never see a latest.json pointing at a missing file.
    app.call("PUT", &format!("{base}/{name}"), data, "application/octet-stream").await?;
    app.call("PUT", &format!("{base}/{name}.sig"), sig.into_bytes(), "text/plain").await?;
    if let (Some((dmg_name, bytes)), Some(dmg_sig)) = (dmg, dmg_sig) {
        app.call("PUT", &format!("{base}/{dmg_name}"), bytes, "application/octet-stream").await?;
        app.call("PUT", &format!("{base}/{dmg_name}.sig"), dmg_sig.into_bytes(), "text/plain").await?;
    }
    app.call("PUT", &format!("{base}/latest.json"), serde_json::to_vec_pretty(&manifest).unwrap(), "application/json").await?;
    Ok(url)
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
            let file = m.as_ref().and_then(|m| match (kind, platform) {
                // Uploaded before latest.json by release_publish, so it exists when the manifest does.
                ("technician", release::MAC) => Some(release::dmg_name(m["version"].as_str()?)),
                _ => Some(m["platforms"][platform]["url"].as_str()?.rsplit('/').next()?.to_owned()),
            });
            let detail = match (kind, platform) {
                ("technician", release::WINDOWS) => format!("Windows installer · version {version}"),
                ("technician", _) => format!("Disk image for Apple silicon and Intel · version {version}"),
                (_, release::WINDOWS) => "Windows · no installation needed · run it when support asks you to".to_owned(),
                _ => "Mac · no installation needed · unzip and open it when support asks you to".to_owned(),
            };
            offers.push(release::Offer { label, detail, href: file.map(|f| format!("/download/stable/{kind}/{platform}/{f}")) });
        }
        sections.push(release::OsSection { id, name, offers });
    }
    if sections.iter().flat_map(|s| &s.offers).all(|o| o.href.is_none()) {
        return Err("There is no stable release of the offered apps yet. Publish one first.".into());
    }
    Ok(release::download_page(&title, &intro, &sections))
}

#[tauri::command]
async fn page_publish(app: State<'_, App>, html: String) -> R<()> {
    app.call("PUT", "/api/admin/download-page", html.into_bytes(), "text/html; charset=utf-8").await.map(|_| ())
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
            features,
            policy_draft,
            policy_save_draft,
            policy_check,
            policy_preview,
            policy_publish,
            policy_history,
            policy_local,
            release_publish,
            page_render,
            page_publish,
            diag_open,
            diag_save
        ])
        .run(tauri::generate_context!())
        .expect("error while running Tether Manage");
}

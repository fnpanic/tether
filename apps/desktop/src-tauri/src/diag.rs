//! Diagnostics: page data, connection test, verbose logging, log export and upload to support.

use std::{
    io::{Read, Seek, SeekFrom, Write},
    net::ToSocketAddrs,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use hbb_common::{config::Config, log, websocket};
use serde_json::{json, Value};

use crate::policy;

const DAY: Duration = Duration::from_secs(24 * 3600);
/// Per log file, newest bytes kept in an export, and the total.
const LOG_TAIL: u64 = 1024 * 1024;
const LOG_TOTAL: u64 = 12 * 1024 * 1024;
/// `POST /api/diag/<ticket>` limit (docs/api.md).
const UPLOAD_MAX: usize = 2 * 1024 * 1024;

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default()
}

fn relay_host() -> &'static str {
    let h = policy::SERVER.unwrap_or_default();
    h.split(':').next().unwrap_or(h)
}

/// Proxy without credentials.
fn proxy_shown() -> Option<String> {
    let mut u = url::Url::parse(&policy::proxy_url()?).ok()?;
    let _ = u.set_password(None);
    let _ = u.set_username("");
    Some(u.to_string())
}

fn dns() -> Value {
    let t = Instant::now();
    match (relay_host(), 443).to_socket_addrs() {
        Ok(a) => json!({ "host": relay_host(), "addrs": a.map(|a| a.ip().to_string()).collect::<Vec<_>>(), "ms": t.elapsed().as_millis() as u64 }),
        Err(e) => json!({ "host": relay_host(), "error": e.to_string(), "ms": t.elapsed().as_millis() as u64 }),
    }
}

/// Everything the Diagnostics page shows (also `diagnostics.json` in an export).
pub fn info() -> Value {
    let eff = policy::effective();
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "id": librustdesk::ui::api::get_id(),
        "os": hbb_common::whoami::distro(),
        "policy": { "serial": eff.serial, "group": eff.group, "sources": eff.sources },
        "update": crate::update::status(),
        "relay": relay_host(),
        "proxy": proxy_shown(),
        "dns": dns(),
        "routes": websocket::route_attempts(),
        "errors": last_errors(40),
        "verboseUntil": verbose_until(),
        "logDir": Config::log_path(),
        "generated": now(),
    })
}

/// Connect every route to the relay now (TLS + WebSocket handshake) and time it.
pub async fn test_connection() -> Value {
    let url = format!("wss://{}/ws/id", relay_host());
    json!({ "routes": websocket::probe_routes(&url, 8000).await, "dns": dns() })
}

// ----------------------------------------------------------------------------- logs

/// Log files of every role (`<log dir>/<role>/*.log`), changed in the last 24 h, newest first.
fn recent_logs() -> Vec<(PathBuf, SystemTime)> {
    let root = Config::log_path();
    let mut out = vec![];
    let mut dirs = vec![root];
    for depth in 0..2 {
        let mut next = vec![];
        for d in &dirs {
            for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
                let Ok(m) = e.metadata() else { continue };
                if m.is_dir() && depth == 0 {
                    next.push(e.path());
                } else if m.is_file() && e.path().extension().is_some_and(|x| x == "log") {
                    let t = m.modified().unwrap_or(UNIX_EPOCH);
                    if t.elapsed().map_or(true, |a| a < DAY) {
                        out.push((e.path(), t));
                    }
                }
            }
        }
        dirs = next;
    }
    out.sort_by(|a, b| b.1.cmp(&a.1));
    out
}

fn tail(path: &Path, max: u64) -> Vec<u8> {
    let mut buf = vec![];
    if let Ok(mut f) = std::fs::File::open(path) {
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        let _ = f.seek(SeekFrom::Start(len.saturating_sub(max)));
        let _ = f.read_to_end(&mut buf);
    }
    buf
}

fn last_errors(n: usize) -> Vec<String> {
    let mut out = vec![];
    for (p, _) in recent_logs().iter().take(6) {
        let text = String::from_utf8_lossy(&tail(p, 256 * 1024)).into_owned();
        out.extend(text.lines().filter(|l| l.contains("] ERROR ") || l.contains("] WARN ")).map(|l| l.chars().take(400).collect()));
    }
    // Lines from several files: newest first by their leading timestamp.
    out.sort_by(|a: &String, b| b.cmp(a));
    out.truncate(n);
    out
}

pub fn open_log_folder() -> Result<(), String> {
    let dir = Config::log_path();
    let cmd = if cfg!(windows) { "explorer" } else if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    std::process::Command::new(cmd).arg(dir).spawn().map(|_| ()).map_err(|e| e.to_string())
}

// ----------------------------------------------------------------------------- verbose logging

const VERBOSE_FILE: &str = "verbose-until";

fn verbose_until() -> u64 {
    std::fs::read_to_string(Config::path(VERBOSE_FILE)).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

/// The core's logger cannot be reconfigured at runtime (its handle is dropped inside
/// `core_main`), so verbose logging is a flag with an expiry that takes effect at the next start
/// of each process (`apply_verbose_env`). Returns the expiry (0 = off).
pub fn set_verbose(on: bool) -> Result<u64, String> {
    policy::require("diagnostics.verbose-logging", "Your organisation does not allow verbose logging.")?;
    let path = Config::path(VERBOSE_FILE);
    if !on {
        let _ = std::fs::remove_file(path);
        return Ok(0);
    }
    let until = now() + 3600;
    policy::write_atomic(&path, until.to_string().as_bytes()).map_err(|e| e.to_string())?;
    Ok(until)
}

/// Startup hook (before the logger is created): trace level while the verbose flag is valid.
pub fn apply_verbose_env() {
    if verbose_until() > now() && policy::on("diagnostics.verbose-logging") && std::env::var_os("RUST_LOG").is_none() {
        std::env::set_var("RUST_LOG", "trace,reqwest=info,hyper=info,hyper_util=info,rustls=info,h2=info,webrtc=warn,webrtc-sctp=warn");
    }
}

// ----------------------------------------------------------------------------- export

fn sensitive(key: &str) -> bool {
    const WORDS: &[&str] = &["password", "key", "salt", "token", "secret", "pin", "otp", "2fa", "tfa", "trusted", "pk", "sk"];
    let k = key.to_ascii_lowercase();
    k.contains("password") || k.split(['-', '_', '.']).any(|w| WORDS.contains(&w))
}

fn redact(v: &mut hbb_common::toml::Value, sens: bool) {
    use hbb_common::toml::Value as V;
    match v {
        V::Table(t) => t.iter_mut().for_each(|(k, v)| redact(v, sens || sensitive(k))),
        V::Boolean(_) => {}
        V::String(s) if s.is_empty() => {}
        _ if sens => *v = V::String("<redacted>".into()),
        V::Array(a) => a.iter_mut().for_each(|v| redact(v, false)),
        _ => {}
    }
}

/// Config TOML with passwords, keys, salts and tokens replaced. Unparseable files are omitted.
pub fn redact_config(text: &str) -> String {
    match text.parse::<hbb_common::toml::Value>() {
        Ok(mut v) => {
            redact(&mut v, false);
            hbb_common::toml::to_string(&v).unwrap_or_default()
        }
        Err(_) => "# not readable, omitted\n".into(),
    }
}

fn build_zip(info: &Value, logs: &[(String, Vec<u8>)], configs: &[(String, String)]) -> Result<Vec<u8>, String> {
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opt = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut add = |name: &str, data: &[u8]| -> Result<(), String> {
        z.start_file(name, opt).map_err(|e| e.to_string())?;
        z.write_all(data).map_err(|e| e.to_string())
    };
    add("diagnostics.json", serde_json::to_string_pretty(info).unwrap_or_default().as_bytes())?;
    for (name, data) in logs {
        add(&format!("logs/{name}"), data)?;
    }
    for (name, text) in configs {
        add(&format!("config/{name}"), redact_config(text).as_bytes())?;
    }
    z.finish().map(|c| c.into_inner()).map_err(|e| e.to_string())
}

/// Diagnostics zip: `diagnostics.json`, last 24 h of logs of every role, redacted config.
pub fn export_bytes() -> Result<Vec<u8>, String> {
    let root = Config::log_path();
    let mut total = 0;
    let mut logs = vec![];
    for (p, _) in recent_logs() {
        if total >= LOG_TOTAL {
            break;
        }
        let data = tail(&p, LOG_TAIL.min(LOG_TOTAL - total));
        total += data.len() as u64;
        let name = p.strip_prefix(&root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
        logs.push((name, data));
    }
    let cfg_dir = Config::path("");
    let configs = std::fs::read_dir(&cfg_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
        .filter_map(|e| Some((e.file_name().to_string_lossy().into_owned(), std::fs::read_to_string(e.path()).ok()?)))
        .collect::<Vec<_>>();
    build_zip(&info(), &logs, &configs)
}

pub fn export_to(path: &Path) -> Result<(), String> {
    std::fs::write(path, export_bytes()?).map_err(|e| e.to_string())
}

pub fn export_to_downloads() -> Result<String, String> {
    let dir = hbb_common::directories_next::UserDirs::new()
        .and_then(|u| u.download_dir().map(Path::to_path_buf))
        .unwrap_or_else(std::env::temp_dir);
    let path = dir.join(format!("tether-diagnostics-{}.zip", now()));
    export_to(&path)?;
    Ok(path.to_string_lossy().into_owned())
}

/// Support ticket code: 8 of `[A-HJ-NP-Z2-9]`.
fn valid_ticket(t: &str) -> bool {
    t.len() == 8 && t.chars().all(|c| matches!(c, 'A'..='H' | 'J'..='N' | 'P'..='Z' | '2'..='9'))
}

/// `POST /api/diag/<ticket>` (blocking).
pub fn upload(ticket: &str) -> Result<(), String> {
    policy::require("diagnostics.upload", "Your organisation does not allow sending diagnostics.")?;
    let ticket = ticket.trim().to_ascii_uppercase();
    if !valid_ticket(&ticket) {
        return Err("Enter the 8-character code from your support contact.".into());
    }
    let mut data = export_bytes()?;
    if data.len() > UPLOAD_MAX {
        log::info!("diagnostics {} bytes, sending without logs", data.len());
        data = build_zip(&info(), &[], &[])?;
    }
    let r = policy::api(|c, base| {
        c.post(format!("{base}/api/diag/{ticket}")).header("Content-Type", "application/zip").body(data.clone())
    })
    .map_err(|e| e.replace("offline:", "Cannot reach the server:"))?;
    match r.status().as_u16() {
        200..=299 => Ok(()),
        404 => Err("This code is not valid or was already used. Ask your support contact for a new one.".into()),
        s => Err(format!("The server refused the upload ({s}).")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
id = "123456789"
password = "hunter2-Secret"
salt = "s4ltS4lt"
key_pair = [[11, 22, 33], [44, 55, 66]]
key_confirmed = true

[options]
approve-mode = "both"
enable-keyboard = "Y"
key = "SERVERPUBKEYxyz="
unlock_pin = "9911"
2fa = "TOTPSECRETBASE32"
recording-upload-token = "tok-abc-123"

[socks]
proxy = "http://proxy:3128"
username = "bob"
password = "proxy-pass-77"
"#;

    #[test]
    fn export_contains_no_secrets() {
        let zip = build_zip(&json!({}), &[], &[("Tether.toml".into(), SAMPLE.into())]).unwrap();
        let mut a = zip::ZipArchive::new(std::io::Cursor::new(zip)).unwrap();
        let mut text = String::new();
        a.by_name("config/Tether.toml").unwrap().read_to_string(&mut text).unwrap();
        for secret in ["hunter2-Secret", "s4ltS4lt", "11, 22", "SERVERPUBKEYxyz", "9911", "TOTPSECRET", "tok-abc-123", "proxy-pass-77"] {
            assert!(!text.contains(secret), "{secret} leaked:\n{text}");
        }
        // Useful, non-secret values stay.
        for kept in ["123456789", "enable-keyboard", "both", "proxy:3128"] {
            assert!(text.contains(kept), "{kept} missing:\n{text}");
        }
        assert!(a.by_name("diagnostics.json").is_ok());
    }

    #[test]
    fn tickets() {
        assert!(valid_ticket("K7Q2M9XD"));
        assert!(!valid_ticket("K7Q2M9X"));
        assert!(!valid_ticket("K7Q2M9XI")); // I is ambiguous
        assert!(!valid_ticket("k7q2m9xd"));
        assert!(!valid_ticket("../../x1"));
    }
}

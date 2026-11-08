//! Diagnostics: page data, connection test, verbose logging, log export and upload to support.

use std::{
    net::ToSocketAddrs,
    path::Path,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use hbb_common::{config::Config, log, websocket};
use serde_json::{json, Value};

use crate::{diag_bundle, policy};

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default()
}

fn relay_host() -> &'static str {
    policy::primary_host()
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

fn last_errors(n: usize) -> Vec<String> {
    diag_bundle::last_errors(&Config::log_path(), n)
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

/// Diagnostics zip: `diagnostics.json`, last 24 h of logs of every role, redacted config.
pub fn export_bytes() -> Result<Vec<u8>, String> {
    diag_bundle::bundle(&info(), &Config::log_path(), &Config::path(""))
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

/// `POST /api/diag/<ticket>` (blocking), to each relay until one takes it.
pub fn upload(ticket: &str) -> Result<(), String> {
    policy::require("diagnostics.upload", "Your organisation does not allow sending diagnostics.")?;
    let ticket = diag_bundle::ticket(ticket)?;
    let info = info();
    let data = diag_bundle::for_upload(&info, &Config::log_path(), &Config::path(""))?;
    let relays: Vec<String> = policy::relay_bases().into_iter().map(|r| r.0).collect();
    let r = diag_bundle::upload_each(&relays, |host| {
        policy::api_via(&[host.to_owned()], |c, base| c.post(format!("{base}/api/diag/{ticket}")).header("Content-Type", "application/zip").body(data.clone()))
            .map(|r| (r.status().as_u16(), tether_trust::read_limited(r, 256).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()))
            .map_err(|e| e.trim_start_matches("offline: ").to_owned())
    });
    match &r {
        Ok(()) => log::info!("diagnostics sent with ticket {ticket} ({} bytes)", data.len()),
        Err(e) => log::warn!("diagnostics upload with ticket {ticket} failed: {e}"),
    }
    r
}

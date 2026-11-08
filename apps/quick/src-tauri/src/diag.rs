//! "Send diagnostics" in the Help view and `TetherQS --diagnostics <out.zip>`: the technician
//! app's bundle (diag_bundle.rs, shared): `diagnostics.json`, the last 24 h of TetherQS's logs and
//! its config files with secrets redacted.

use std::path::Path;

use hbb_common::config::Config;
use serde_json::{json, Value};

use crate::{commands, diag_bundle, policy};

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default()
}

/// The Help view's facts plus OS, recent errors and the log folder.
fn info() -> Value {
    let mut v = serde_json::to_value(commands::diagnostics()).unwrap_or_else(|_| json!({}));
    v["app"] = json!("TetherQS");
    v["os"] = json!(hbb_common::whoami::distro());
    v["custom_app"] = json!(crate::custom::profile().map(|p| p.name.clone()));
    v["errors"] = json!(diag_bundle::last_errors(&Config::log_path(), 40));
    v["logDir"] = json!(Config::log_path());
    v["generated"] = json!(now());
    v
}

pub fn export_to(path: &Path) -> Result<(), String> {
    std::fs::write(path, diag_bundle::bundle(&info(), &Config::log_path(), &Config::path(""))?).map_err(|e| e.to_string())
}

/// Relay base URLs and the client for them. Debug builds: `TETHER_DEV_API` (comma-separated
/// `http://` stand-ins) replaces the compiled-in servers, for the end-to-end test.
fn targets() -> Result<(Vec<String>, reqwest::blocking::Client), String> {
    #[cfg(debug_assertions)]
    if let Ok(v) = std::env::var("TETHER_DEV_API") {
        let c = reqwest::blocking::Client::builder().no_proxy().timeout(std::time::Duration::from_secs(30)).build().map_err(|e| e.to_string())?;
        return Ok((v.split(',').map(|b| b.trim().trim_end_matches('/').to_owned()).filter(|b| !b.is_empty()).collect(), c));
    }
    Ok((policy::base_urls(), policy::http()?))
}

/// `POST /api/diag/<ticket>` (blocking), to each server until one takes it.
pub fn upload(ticket: &str) -> Result<(), String> {
    if !policy::current().on("diagnostics.upload") {
        return Err("Your organisation does not allow sending diagnostics.".into());
    }
    let ticket = diag_bundle::ticket(ticket)?;
    let data = diag_bundle::for_upload(&info(), &Config::log_path(), &Config::path(""))?;
    let (relays, client) = targets()?;
    let r = diag_bundle::upload_each(&relays, |base| {
        client
            .post(format!("{base}/api/diag/{ticket}"))
            .header("Content-Type", "application/zip")
            .body(data.clone())
            .send()
            .map(|r| r.status().as_u16())
            .map_err(|e| commands::with_causes(&e))
    });
    match &r {
        Ok(()) => hbb_common::log::info!("diagnostics sent with ticket {ticket} ({} bytes)", data.len()),
        Err(e) => hbb_common::log::warn!("diagnostics upload with ticket {ticket} failed: {e}"),
    }
    r
}

//! Team access (docs/advanced.md#team-sync): this app's own Ed25519 key signs requests to
//! `/api/team/*` once an admin has added its public key to the team roster in Tether Manage.
//! The private key never leaves this file and this process; the webview only sees the public half.

use hbb_common::config::Config;
use serde_json::Value;
use tether_trust::{reqauth, SigningKey};

type R<T> = Result<T, String>;

const DEVICES_MAX: usize = 4 * 1024 * 1024;
const TAGS_MAX: usize = 1024 * 1024;
const AUDIT_MAX: usize = 2 * 1024 * 1024;
/// The relay refuses larger tag documents (docs/api.md, team endpoints).
pub const TAGS_PUT_MAX: usize = 512 * 1024;

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<[u8; 32]> {
    let s = s.trim();
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// Load the key from `path`, creating it on first use (owner-only on macOS/Linux; the config
/// folder is per user on Windows).
pub fn key_at(path: &std::path::Path) -> R<SigningKey> {
    if let Ok(s) = std::fs::read_to_string(path) {
        return unhex(&s).map(|b| SigningKey::from_bytes(&b)).ok_or_else(|| "The team key file is damaged.".to_owned());
    }
    let k = tether_trust::generate_key();
    crate::policy::write_atomic(&path.to_path_buf(), hex(&k.to_bytes()).as_bytes()).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(k)
}

fn key() -> R<SigningKey> {
    key_at(&Config::path("team-key"))
}

/// `{key, fingerprint}`: what the technician gives the admin, and reads out to compare.
pub fn identity() -> R<Value> {
    let k = key()?.verifying_key();
    Ok(serde_json::json!({ "key": tether_trust::key_to_b64(&k), "fingerprint": tether_trust::team::fingerprint(&k) }))
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Readable answer for the UI; `offline:`-prefixed when no relay was reached.
fn status_error(code: u16, body: &str) -> String {
    match code {
        401 => "not-member: This app is not in your organisation's team. Send its key to your administrator.".into(),
        403 => "Your team membership does not allow this.".into(),
        404 => "unsupported: The server does not offer team access yet.".into(),
        413 => "The shared tags are too large for the server (512 KB).".into(),
        429 => "Too many requests; try again in a minute.".into(),
        _ => format!("The server answered {code}: {}", body.chars().take(200).collect::<String>()),
    }
}

/// One signed request. `path` includes the query string (it is part of the signature).
fn call(method: &str, path: &str, body: Option<&[u8]>, if_match: Option<u64>, limit: usize) -> R<(u16, Vec<u8>)> {
    let k = key()?;
    let body = body.unwrap_or_default().to_vec();
    let r = crate::policy::api(|c, base| {
        // A fresh signature (and nonce) per attempt: the relay rejects a replayed one.
        let sig = reqauth::sign_request(&k, method, path, &body, now());
        let url = format!("{base}{path}");
        let rq = if method == "PUT" { c.put(url).header("Content-Type", "application/json").body(body.clone()) } else { c.get(url) };
        let rq = rq.header(reqauth::HEADER, sig);
        match if_match {
            Some(rev) => rq.header("If-Match", rev.to_string()),
            None => rq,
        }
    })?;
    let code = r.status().as_u16();
    let raw = tether_trust::read_limited(r, limit)?;
    Ok((code, raw))
}

fn json_ok(code: u16, raw: &[u8]) -> R<Value> {
    if code != 200 {
        return Err(status_error(code, &String::from_utf8_lossy(raw)));
    }
    serde_json::from_slice(raw).map_err(|e| format!("bad answer from the server: {e}"))
}

pub fn me() -> R<Value> {
    let (c, b) = call("GET", "/api/team/me", None, None, 4096)?;
    json_ok(c, &b)
}

pub fn devices() -> R<Value> {
    let (c, b) = call("GET", "/api/team/devices", None, None, DEVICES_MAX)?;
    json_ok(c, &b)
}

pub fn tags_get() -> R<Value> {
    let (c, b) = call("GET", "/api/team/tags", None, None, TAGS_MAX)?;
    json_ok(c, &b)
}

/// `{ok:true, rev}` when stored, `{ok:false, conflict:{rev, doc, …}}` when someone else saved
/// first (the webview merges and retries).
pub fn tags_put(rev: u64, doc: &str) -> R<Value> {
    if doc.len() > TAGS_PUT_MAX {
        return Err(status_error(413, ""));
    }
    if !matches!(serde_json::from_str::<Value>(doc), Ok(Value::Object(_))) {
        return Err("invalid tag document".into());
    }
    let (c, b) = call("PUT", "/api/team/tags", Some(doc.as_bytes()), Some(rev), TAGS_MAX)?;
    match c {
        409 => Ok(serde_json::json!({ "ok": false, "conflict": json_ok(200, &b)? })),
        _ => {
            let v = json_ok(c, &b)?;
            Ok(serde_json::json!({ "ok": true, "rev": v.get("rev").cloned().unwrap_or(Value::Null) }))
        }
    }
}

pub fn audit(device: &str, since: u64) -> R<Value> {
    crate::security::check_peer_id(device)?;
    let (c, b) = call("GET", &format!("/api/team/audit?device={device}&since={since}&limit=200"), None, None, AUDIT_MAX)?;
    json_ok(c, &b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_is_created_once_and_reloaded() {
        let p = std::env::temp_dir().join(format!("tether-team-key-{}", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let a = key_at(&p).unwrap();
        let b = key_at(&p).unwrap();
        assert_eq!(a.to_bytes(), b.to_bytes());
        std::fs::write(&p, "nonsense").unwrap();
        assert!(key_at(&p).is_err(), "a damaged key is reported, never silently replaced");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn hex_round_trip() {
        let b = [7u8; 32];
        assert_eq!(unhex(&hex(&b)), Some(b));
        assert_eq!(unhex("zz"), None);
    }

    #[test]
    fn errors_are_classified() {
        assert!(status_error(401, "").starts_with("not-member:"));
        assert!(status_error(404, "").starts_with("unsupported:"));
        assert!(status_error(500, &"x".repeat(1000)).len() < 260);
    }
}

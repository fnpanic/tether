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
    match read_key(path)? {
        Some(k) => Ok(k),
        None => store_new(path, tether_trust::generate_key()),
    }
}

const DAMAGED: &str = "The team key file is damaged.";

/// The stored key; None only when there is no file. Unreadable or damaged is an error, never
/// "absent": a new key would replace the enrolled one.
fn read_key(path: &std::path::Path) -> R<Option<SigningKey>> {
    match std::fs::read_to_string(path) {
        Ok(s) => unhex(&s).map(|b| Some(SigningKey::from_bytes(&b))).ok_or_else(|| DAMAGED.to_owned()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("The team key file cannot be read: {e}")),
    }
}

/// Stores `k` unless a key is there already, and returns the key now in the file. Complete or
/// absent at every moment: written to a temp file (owner-only from the start), then linked into
/// place, which never replaces a file another process put there meanwhile.
fn store_new(path: &std::path::Path, k: SigningKey) -> R<SigningKey> {
    use std::io::{ErrorKind, Write as _};
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    let placed = o
        .open(&tmp)
        .and_then(|mut f| f.write_all(hex(&k.to_bytes()).as_bytes()).and_then(|_| f.sync_all()))
        .and_then(|_| match std::fs::hard_link(&tmp, path) {
            // shortcut: a volume without hard links gets a plain rename, which could replace a key
            // stored in the same instant; use a no-replace rename (renameat2, MoveFileEx) if that
            // ever happens in the field.
            Err(e) if e.kind() != ErrorKind::AlreadyExists && !path.exists() => std::fs::rename(&tmp, path),
            r => r,
        });
    let _ = std::fs::remove_file(&tmp);
    match placed {
        Ok(()) => Ok(k),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => read_key(path)?.ok_or_else(|| DAMAGED.to_owned()),
        Err(e) => Err(format!("The team key could not be stored: {e}")),
    }
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
        // Requests are signed for one relay (v2). A relay from before that, or one that does not
        // know its own name, refuses every member: say so instead of "not in the team".
        401 if body.contains("unsupported signature version") => "This server must be updated before team features work with this version of Tether.".into(),
        401 if body.contains("RELAY_HOST") || body.contains("signed for another relay") => "The server is not set up for this version of Tether yet (RELAY_HOST).".into(),
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
        // A fresh signature (and nonce) per attempt: the relay rejects a replayed one, and each
        // is made for the relay it goes to.
        let sig = reqauth::sign_request_for(&k, base, method, path, &body, now());
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
        // Unreadable as text (any read error but "not found"): reported, and the file stays.
        std::fs::write(&p, [0xff, 0xfe, 0x00]).unwrap();
        assert!(key_at(&p).is_err());
        assert_eq!(std::fs::read(&p).unwrap(), [0xff, 0xfe, 0x00]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::remove_file(&p);
            key_at(&p).unwrap();
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let _ = std::fs::remove_file(&p);
        // A key stored meanwhile (another process) is used, not replaced; no temp file stays.
        let (k1, k2) = (tether_trust::generate_key(), tether_trust::generate_key());
        assert_eq!(store_new(&p, k1.clone()).unwrap().to_bytes(), k1.to_bytes());
        assert_eq!(store_new(&p, k2).unwrap().to_bytes(), k1.to_bytes());
        assert_eq!(key_at(&p).unwrap().to_bytes(), k1.to_bytes());
        assert!(!p.with_extension(format!("{}.tmp", std::process::id())).exists());
        // A damaged file in the way is reported and left alone.
        std::fs::write(&p, "nonsense").unwrap();
        assert!(store_new(&p, tether_trust::generate_key()).is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "nonsense");
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
        assert!(status_error(401, "signature does not match").starts_with("not-member:"));
        assert!(status_error(401, "unsupported signature version").contains("must be updated"));
        for body in ["this relay does not know its own host name (RELAY_HOST), so it cannot accept v2 signatures", "request signed for another relay"] {
            assert!(status_error(401, body).contains("not set up"), "{body}");
        }
        assert!(status_error(404, "").starts_with("unsupported:"));
        assert!(status_error(500, &"x".repeat(1000)).len() < 260);
    }
}

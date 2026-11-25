//! Team access (docs/advanced.md#team-sync): this app's own Ed25519 key signs requests to
//! `/api/team/*` once an admin has added its public key to the team roster in Tether Manage.
//! The private key never leaves this file and this process; the webview only sees the public half.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use hbb_common::config::Config;
use serde_json::Value;
use tether_trust::{reqauth, SigningKey};

type R<T> = Result<T, String>;

const DEVICES_MAX: usize = 4 * 1024 * 1024;
const TAGS_MAX: usize = 1024 * 1024;
const AUDIT_MAX: usize = 2 * 1024 * 1024;
const STATUS_MAX: usize = 16 << 20;
/// One report (≤ 64 KiB) plus 90 days of history.
const REPORT_MAX: usize = 1 << 20;
/// A software list (SBOM): the relay keeps at most 512 KiB by default; more than 1 MiB is not shown.
const SBOM_MAX: usize = 1 << 20;
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
/// Only a real "no" forgets a relay's health list: membership gone (401, 403), no health
/// support (404) or reporting switched off. Anything else may pass, and the hosts must not
/// flip to "no agent" (and back, with a burst of notifications) because of it.
fn status_drops_cache(code: u16, body: &[u8]) -> bool {
    matches!(code, 401 | 403 | 404) || (code == 503 && body.starts_with(b"status reporting is off"))
}

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
        409 => format!("Refused by the server: {}", body.chars().take(200).collect::<String>()),
        503 if body.starts_with("status reporting is off") => "off: Health reporting is switched off on this server.".into(),
        429 => "Too many requests; try again in a minute.".into(),
        _ => format!("The server answered {code}: {}", body.chars().take(200).collect::<String>()),
    }
}

/// One signed request to the first reachable relay, or to the first reachable of `hosts`.
/// `path` includes the query string (it is part of the signature). Answers status, ETag, body.
fn call_via(hosts: Option<&[String]>, method: &str, path: &str, body: Option<&[u8]>, headers: &[(&str, String)], limit: usize) -> R<(u16, Option<String>, Vec<u8>)> {
    let k = key()?;
    let body = body.unwrap_or_default().to_vec();
    let build = |c: &reqwest::blocking::Client, base: &str| {
        // A fresh signature (and nonce) per attempt: the relay rejects a replayed one, and each
        // is made for the relay it goes to.
        let sig = reqauth::sign_request_for(&k, base, method, path, &body, now());
        let url = format!("{base}{path}");
        let rq = c.request(reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::GET), url);
        let rq = if body.is_empty() { rq } else { rq.header("Content-Type", "application/json").body(body.clone()) };
        headers.iter().fold(rq.header(reqauth::HEADER, sig), |rq, (n, v)| rq.header(*n, v))
    };
    let r = match hosts {
        Some(h) => crate::policy::api_via(h, build),
        None => crate::policy::api(build),
    }?;
    answer(r, limit)
}

/// Status, ETag and body of `r`, at most `limit` bytes after decoding. The relay API clients
/// (`policy::api_via`) keep reqwest's `gzip` feature on: every request says `Accept-Encoding: gzip`
/// and a gzip answer (the health summary) is decoded while it is read, so `limit` bounds the
/// decoded size; a relay that ignores the header answers plain and is read the same way.
fn answer(r: reqwest::blocking::Response, limit: usize) -> R<(u16, Option<String>, Vec<u8>)> {
    let code = r.status().as_u16();
    let etag = r.headers().get("ETag").and_then(|v| v.to_str().ok()).map(str::to_owned);
    let raw = tether_trust::read_limited(r, limit)?;
    Ok((code, etag, raw))
}

fn call(method: &str, path: &str, body: Option<&[u8]>, headers: &[(&str, String)], limit: usize) -> R<(u16, Vec<u8>)> {
    call_via(None, method, path, body, headers, limit).map(|(c, _, b)| (c, b))
}

fn json_ok(code: u16, raw: &[u8]) -> R<Value> {
    if code != 200 {
        return Err(status_error(code, &String::from_utf8_lossy(raw)));
    }
    serde_json::from_slice(raw).map_err(|e| format!("bad answer from the server: {e}"))
}

pub fn me() -> R<Value> {
    let (c, b) = call("GET", "/api/team/me", None, &[], 4096)?;
    json_ok(c, &b)
}

pub fn devices() -> R<Value> {
    let (c, b) = call("GET", "/api/team/devices", None, &[], DEVICES_MAX)?;
    json_ok(c, &b)
}

pub fn tags_get() -> R<Value> {
    let (c, b) = call("GET", "/api/team/tags", None, &[], TAGS_MAX)?;
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
    let (c, b) = call("PUT", "/api/team/tags", Some(doc.as_bytes()), &[("If-Match", rev.to_string())], TAGS_MAX)?;
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
    let (c, b) = call("GET", &format!("/api/team/audit?device={device}&since={since}&limit=200"), None, &[], AUDIT_MAX)?;
    json_ok(c, &b)
}

/// Each relay's last health summary answer (`{built, hosts}`) and its ETag ("" = none sent).
/// `Arc`: a 304 and the copy kept here share one value.
static STATUS_CACHE: Mutex<BTreeMap<String, (String, Arc<Value>)>> = Mutex::new(BTreeMap::new());

/// Health is kept per relay (docs/api.md): every relay is asked.
/// `[{"relay","hosts":[…]} | {"relay","error"} | {"relay","hosts","error"}]`; the last form is a relay
/// that could not be reached, with its last answer. Err only when every relay failed.
pub fn status() -> R<Value> {
    let relays: Vec<String> = crate::policy::relay_bases().into_iter().map(|(h, _)| h).collect();
    status_with(&relays, &STATUS_CACHE, |relay, inm| call_via(Some(&[relay.to_owned()]), "GET", "/api/team/status", None, inm, STATUS_MAX))
}

/// [`status`] with the relays, the cache and the signed request given (tests: a stand-in answer).
fn status_with(relays: &[String], cache: &Mutex<BTreeMap<String, (String, Arc<Value>)>>, call: impl Fn(&str, &[(&str, String)]) -> R<(u16, Option<String>, Vec<u8>)>) -> R<Value> {
    let mut out = Vec::new();
    let mut first_err = None;
    let mut all_failed = true;
    for relay in relays.iter().cloned() {
        let cached = cache.lock().unwrap().get(&relay).cloned();
        let inm: Vec<(&str, String)> = cached.iter().filter(|(e, _)| !e.is_empty()).map(|(e, _)| ("If-None-Match", e.clone())).collect();
        let hosts = |v: &Value| v.get("hosts").cloned().unwrap_or_else(|| Value::Array(vec![]));
        let entry = match call(&relay, &inm) {
            Ok((304, _, _)) if cached.is_some() => Ok(cached.as_ref().unwrap().1.clone()),
            Ok((c, etag, raw)) => {
                let v = json_ok(c, &raw).map(Arc::new);
                let mut cache = cache.lock().unwrap();
                match &v {
                    Ok(v) => {
                        cache.insert(relay.clone(), (etag.unwrap_or_default(), v.clone()));
                    }
                    // The relay answered no (off, membership gone): its old data is not shown any more.
                    Err(_) if status_drops_cache(c, &raw) => {
                        cache.remove(&relay);
                    }
                    // A passing failure (5xx, 429, 408, a garbled answer): handled as not reached below.
                    Err(_) => {}
                }
                v
            }
            Err(e) => Err(e),
        };
        // Not reached, or a passing failure (cache kept above): its last answer stays in the list,
        // with the error beside it.
        if let (Err(e), Some((_, v))) = (&entry, &cached) {
            if cache.lock().unwrap().contains_key(&relay) {
                first_err.get_or_insert_with(|| e.clone());
                out.push(serde_json::json!({ "relay": relay, "hosts": hosts(v), "error": e }));
                all_failed = false;
                continue;
            }
        }
        out.push(match entry {
            Ok(v) => {
                all_failed = false;
                serde_json::json!({ "relay": relay, "hosts": hosts(&v) })
            }
            Err(e) => {
                first_err.get_or_insert_with(|| e.clone());
                serde_json::json!({ "relay": relay, "error": e })
            }
        });
    }
    match first_err {
        Some(e) if all_failed => Err(e),
        _ if out.is_empty() => Err("offline: no server compiled into this build".into()),
        _ => Ok(Value::Array(out)),
    }
}

/// The webview names the relay and the host: signed requests go only to this build's own relays.
fn own_relay_host(relay: &str, uuid: &str) -> R<()> {
    if !tether_trust::status::valid_uuid(uuid) {
        return Err("invalid host id".into());
    }
    if !crate::policy::relay_bases().iter().any(|(h, _)| h == relay) {
        return Err("unknown server".into());
    }
    Ok(())
}

/// One host's last report and history from `relay` (one of this build's relays).
pub fn status_host(relay: &str, uuid: &str) -> R<Value> {
    own_relay_host(relay, uuid)?;
    let (c, _, b) = call_via(Some(&[relay.to_owned()]), "GET", &format!("/api/team/status/{uuid}"), None, &[], REPORT_MAX)?;
    json_ok(c, &b)
}

/// The software list a device sent, as the relay holds it (the text, unchanged: the webview reads it
/// and exports it byte for byte). The relay stores any JSON object, so nothing here parses it.
pub fn status_sbom(relay: &str, uuid: &str) -> R<String> {
    own_relay_host(relay, uuid)?;
    let (c, _, b) = call_via(Some(&[relay.to_owned()]), "GET", &format!("/api/team/status/{uuid}/sbom"), None, &[], SBOM_MAX).map_err(over_limit)?;
    sbom_text(c, b)
}

/// `tether_trust::read_limited` has no typed error: its text for an answer over the limit is matched here, and a test
/// pins it against the real function.
fn over_limit(e: String) -> String {
    if e.starts_with("the answer is larger than") {
        "too-large: The software list is too large to show.".into()
    } else {
        e
    }
}

/// 404 is "no document" (none stored, or the feature is off on that relay), not "no team access".
fn sbom_text(code: u16, raw: Vec<u8>) -> R<String> {
    match code {
        200 => String::from_utf8(raw).map_err(|_| "bad answer from the server: the software list is not text".into()),
        404 => Err("none: No software list on the server.".into()),
        _ => Err(status_error(code, &String::from_utf8_lossy(&raw))),
    }
}

/// Changes one health device on every relay of this build: pins (and so links) are kept per relay,
/// and the host list shows the newest summary of any of them. A relay that does not know the device
/// (404) is skipped. Err names each relay that refused or could not be reached.
fn status_change(method: &str, uuid: &str, suffix: &str, body: Option<&[u8]>) -> R<()> {
    if !tether_trust::status::valid_uuid(uuid) {
        return Err("invalid host id".into());
    }
    let (mut done, mut errs) = (false, Vec::new());
    for (relay, _) in crate::policy::relay_bases() {
        match call_via(Some(std::slice::from_ref(&relay)), method, &format!("/api/team/status/{uuid}{suffix}"), body, &[], 4096) {
            Ok((c, _, _)) if (200..300).contains(&c) => done = true,
            Ok((404, _, _)) => {}
            Ok((c, _, b)) => errs.push(format!("{relay}: {}", status_error(c, &String::from_utf8_lossy(&b)))),
            Err(e) => errs.push(format!("{relay}: {e}")),
        }
    }
    match (done, errs.is_empty()) {
        (_, false) => Err(errs.join(" · ")),
        (false, true) => Err("No server knows this agent any more.".into()),
        (true, true) => Ok(()),
    }
}

/// Confirms an agent's claim to `tether_id` (a member with write; the relay checks the claim).
pub fn status_link(uuid: &str, tether_id: &str) -> R<()> {
    if !tether_trust::status::valid_uuid(uuid) {
        return Err("invalid host id".into());
    }
    if !tether_trust::status::valid_tether_id(tether_id) || tether_id.starts_with("m:") {
        return Err("invalid Tether ID".into());
    }
    let body = serde_json::json!({ "tether_id": tether_id }).to_string();
    status_change("POST", uuid, "/link", Some(body.as_bytes()))
}

/// Removes an agent's link or claim.
pub fn status_unlink(uuid: &str) -> R<()> {
    status_change("DELETE", uuid, "/link", None)
}

/// After an Unlink: lets the agent claim a Tether ID again (still only a claim).
pub fn status_allow_claim(uuid: &str) -> R<()> {
    status_change("POST", uuid, "/allow-claim", None)
}

/// Removes an agent from health monitoring: pin, link or claim, and its stored reports.
pub fn status_remove(uuid: &str) -> R<()> {
    status_change("DELETE", uuid, "", None)
}

/// The cached summary entries linked to a Tether ID, one per device (its newest over all relays,
/// with `relay`); no request. Two or more: a link conflict, for which no device's health is shown.
pub fn status_find(tether_id: &str) -> Vec<Value> {
    find_in(&STATUS_CACHE.lock().unwrap(), tether_id)
}

fn find_in(cache: &BTreeMap<String, (String, Arc<Value>)>, tether_id: &str) -> Vec<Value> {
    let seen = |x: &Value| x.get("last_seen").and_then(Value::as_u64).unwrap_or(0);
    let mut by: BTreeMap<String, Value> = BTreeMap::new();
    for (relay, (_, v)) in cache {
        for h in v.get("hosts").and_then(Value::as_array).into_iter().flatten() {
            if h.get("tether_id").and_then(Value::as_str) != Some(tether_id) {
                continue;
            }
            let uuid = h.get("uuid").and_then(Value::as_str).unwrap_or_default().to_owned();
            // On a tie the first relay in name order, as the host list (health.ts mergeStatus).
            if by.get(&uuid).is_none_or(|b| seen(h) > seen(b)) {
                let mut h = h.clone();
                h["relay"] = Value::from(relay.as_str());
                by.insert(uuid, h);
            }
        }
    }
    by.into_values().collect()
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

    #[test]
    fn link_inputs_are_checked_before_any_request() {
        for uuid in ["bad uuid!", "short", "../../api/admin/status"] {
            assert_eq!(status_link(uuid, "123456789").unwrap_err(), "invalid host id");
            assert_eq!(status_unlink(uuid).unwrap_err(), "invalid host id");
            assert_eq!(status_remove(uuid).unwrap_err(), "invalid host id");
            assert_eq!(status_allow_claim(uuid).unwrap_err(), "invalid host id");
        }
        for id in ["", "a b", "m:dev-0001", &"1".repeat(33)] {
            assert_eq!(status_link("dev-0001", id).unwrap_err(), "invalid Tether ID", "{id:?}");
        }
    }

    fn cache_of(entries: &[(&str, Value)]) -> BTreeMap<String, (String, Arc<Value>)> {
        entries.iter().map(|(r, v)| (r.to_string(), (String::new(), Arc::new(v.clone())))).collect()
    }

    #[test]
    fn a_session_finds_every_device_linked_to_its_peer() {
        let h = |uuid: &str, id: &str, seen: u64| serde_json::json!({"uuid": uuid, "tether_id": id, "last_seen": seen});
        // One device on two relays: its newest entry, with the relay it came from.
        let c = cache_of(&[("a", serde_json::json!({"hosts": [h("u1", "111", 5), h("u9", "999", 1)]})), ("b", serde_json::json!({"hosts": [h("u1", "111", 7)]}))]);
        let found = find_in(&c, "111");
        assert_eq!((found.len(), &found[0]["relay"], &found[0]["last_seen"]), (1, &Value::from("b"), &Value::from(7)));
        // Two devices linked to one ID on different relays: both (a conflict, no health shown).
        let c = cache_of(&[("a", serde_json::json!({"hosts": [h("u1", "111", 5)]})), ("b", serde_json::json!({"hosts": [h("u2", "111", 9)]}))]);
        assert_eq!(find_in(&c, "111").iter().map(|x| x["uuid"].as_str().unwrap()).collect::<Vec<_>>(), ["u1", "u2"]);
        assert!(find_in(&c, "222").is_empty());
    }

    #[test]
    fn the_summary_cache_reuses_its_etag_and_is_dropped_only_on_a_real_no() {
        let relays = vec!["a".to_owned()];
        let cache = Mutex::new(BTreeMap::new());
        let body = br#"{"built":1,"hosts":[{"uuid":"u1","tether_id":"111"}]}"#.to_vec();
        let sent = Mutex::new(Vec::new());
        let ask = |answer: Result<(u16, Option<String>, Vec<u8>), String>| {
            status_with(&relays, &cache, |relay, h| {
                assert_eq!(relay, "a");
                sent.lock().unwrap().push(h.iter().find(|(n, _)| *n == "If-None-Match").map(|(_, v)| v.clone()));
                answer.clone()
            })
        };
        let hosts = |v: &Value| v[0]["hosts"][0]["uuid"].clone();
        let r = ask(Ok((200, Some("\"e1\"".into()), body.clone()))).unwrap();
        assert_eq!((hosts(&r), r[0].get("error")), (Value::from("u1"), None));
        // Unchanged: a 304 to the tag sent, the cached hosts.
        let r = ask(Ok((304, Some("\"e1\"".into()), vec![]))).unwrap();
        assert_eq!((hosts(&r), sent.lock().unwrap().last().cloned().flatten()), (Value::from("u1"), Some("\"e1\"".to_owned())));
        // Passing failures keep the cache: the hosts stay, with the error beside them.
        for a in [Ok((500, None, b"oops".to_vec())), Ok((503, None, b"busy".to_vec())), Ok((429, None, vec![])), Ok((408, None, vec![])), Err("offline: timed out".to_owned())] {
            let r = ask(a.clone()).unwrap();
            assert_eq!(hosts(&r), Value::from("u1"), "{a:?}");
            assert!(r[0]["error"].is_string(), "{a:?}");
        }
        assert_eq!(sent.lock().unwrap().last().cloned().flatten(), Some("\"e1\"".to_owned()), "the tag is still sent");
        // A real no (membership gone, no health support, reporting off): the cache goes.
        for a in [(401, b"unknown member".to_vec()), (403, vec![]), (404, vec![]), (503, b"status reporting is off".to_vec())] {
            ask(Ok((200, Some("\"e2\"".into()), body.clone()))).unwrap();
            assert!(ask(Ok((a.0, None, a.1.clone()))).is_err(), "{a:?}");
            assert!(cache.lock().unwrap().is_empty(), "{a:?}");
            assert!(ask(Ok((500, None, vec![]))).is_err(), "nothing cached to show: {a:?}");
            assert_eq!(sent.lock().unwrap().last().cloned().flatten(), None, "no tag after a drop: {a:?}");
        }
    }

    /// One answer per connection from a stand-in relay; the request heads it received.
    fn stand_in(answers: Vec<Vec<u8>>) -> (String, std::sync::mpsc::Receiver<String>) {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for a in answers {
                let (mut s, _) = listener.accept().unwrap();
                let mut head = Vec::new();
                let mut b = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") && s.read(&mut b).unwrap_or(0) == 1 {
                    head.push(b[0]);
                }
                let _ = tx.send(String::from_utf8_lossy(&head).to_lowercase());
                let _ = s.write_all(&a);
            }
        });
        (format!("http://{addr}/api/team/status"), rx)
    }

    fn gz(b: &[u8]) -> Vec<u8> {
        use std::io::Write as _;
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(b).unwrap();
        e.finish().unwrap()
    }

    fn reply(extra: &str, body: &[u8]) -> Vec<u8> {
        let mut r = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nETag: W/\"0123456789abcdef\"\r\nVary: Accept-Encoding\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
        r.extend_from_slice(body);
        r
    }

    /// The summary is asked for gzip (with the tag) and decoded, the decoded size bounded; a relay
    /// that ignores the header (an old one) is read as before.
    #[test]
    fn the_summary_is_asked_for_gzip_and_decoded_within_the_limit() {
        let plain = br#"{"built":1,"hosts":[{"uuid":"u1","tether_id":"111","last_seen":5,"stale":false}]}"#.repeat(1000);
        let bomb = gz(&vec![b' '; STATUS_MAX + 1]);
        assert!(bomb.len() < 64 << 10, "{}", bomb.len());
        let (url, heads) = stand_in(vec![reply("Content-Encoding: gzip\r\n", &gz(&plain)), reply("", &plain), reply("Content-Encoding: gzip\r\n", &bomb)]);
        // The relay API clients' settings that matter here (policy::api_via): reqwest's defaults.
        let c = reqwest::blocking::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build().unwrap();
        let get = || answer(c.get(&url).header("If-None-Match", "W/\"0123456789abcdef\"").send().unwrap(), STATUS_MAX);
        let (code, etag, raw) = get().unwrap();
        assert_eq!((code, etag.as_deref(), raw == plain), (200, Some("W/\"0123456789abcdef\""), true));
        let head = heads.recv().unwrap();
        assert!(head.lines().any(|l| l.starts_with("accept-encoding:") && l.contains("gzip")), "{head}");
        assert!(head.contains("if-none-match: w/\"0123456789abcdef\""), "{head}");
        // A relay that answers plain.
        assert_eq!(get().unwrap().2, plain);
        // A gzip answer that decodes to more than the limit is refused while it is read.
        assert!(get().unwrap_err().contains("larger than"));
    }

    #[test]
    fn sbom_answers() {
        // The text comes through unchanged, whatever it holds.
        let doc = "{\"components\": [ ]}
";
        assert_eq!(sbom_text(200, doc.as_bytes().to_vec()).unwrap(), doc);
        assert!(sbom_text(404, b"not found".to_vec()).unwrap_err().starts_with("none:"));
        assert!(sbom_text(200, vec![0xff, 0xfe]).unwrap_err().starts_with("bad answer"));
        assert!(sbom_text(403, vec![]).unwrap_err().contains("membership"));
        assert!(sbom_text(429, vec![]).unwrap_err().contains("Too many"));
        // An answer over the limit (the real reader, so a change of its text fails here) is "too large"; other errors pass.
        let e = tether_trust::read_limited(std::io::Cursor::new(vec![b'x'; SBOM_MAX + 1]), SBOM_MAX).unwrap_err();
        assert!(over_limit(e).starts_with("too-large:"));
        assert!(tether_trust::read_limited(std::io::Cursor::new(vec![b'x'; SBOM_MAX]), SBOM_MAX).is_ok());
        assert_eq!(over_limit("offline: connection refused".into()), "offline: connection refused");
        // Host and relay are checked before anything is sent.
        assert_eq!(status_sbom("relay.example", "../x").unwrap_err(), "invalid host id");
        assert_eq!(status_sbom("relay.example", "0123456789abcdef0123456789abcdef").unwrap_err(), "unknown server");
    }

    #[test]
    fn status_errors_are_classified() {
        assert!(status_error(503, "status reporting is off").starts_with("off:"));
        assert!(!status_error(503, "busy").starts_with("off:"));
        for (code, body) in [(401, "unknown member"), (403, "x"), (404, "not found"), (503, "status reporting is off")] {
            assert!(status_drops_cache(code, body.as_bytes()), "{code} {body}");
        }
        for (code, body) in [(500, "oops"), (502, ""), (503, "busy"), (429, "too many requests"), (408, ""), (200, "not json")] {
            assert!(!status_drops_cache(code, body.as_bytes()), "{code} {body}");
        }
    }
}

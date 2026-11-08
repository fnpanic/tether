//! Diagnostics tickets of every relay, merged. Manage opens a ticket on every relay with the same
//! code; the app uploads to whichever relay it reaches, so the upload is on one of them only.

use serde_json::{json, Value};

const TICKET_OPEN: u64 = 86_400;

const ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

pub fn valid_ticket(t: &str) -> bool {
    t.len() == 8 && t.bytes().all(|b| ALPHABET.contains(&b))
}

/// A random ticket code (Manage sends it to every relay, which accept a given code; see
/// `new_ticket` in server/api).
pub fn new_ticket() -> String {
    use chacha20poly1305::aead::{rand_core::RngCore, OsRng};
    let mut b = [0u8; 8];
    OsRng.fill_bytes(&mut b);
    b.iter().map(|x| ALPHABET[(x & 31) as usize] as char).collect()
}

/// `state` as the relay reports it; relays before close/archive have none: derived.
fn state(t: &Value, now: u64) -> &'static str {
    match t["state"].as_str() {
        Some("closed") => "closed",
        Some("uploaded") => "uploaded",
        Some("open") => "open",
        Some(_) => "expired",
        None if t["closed"].as_u64().is_some() => "closed",
        None if t["uploaded"].as_u64().is_some() => "uploaded",
        None if now < t["created"].as_u64().unwrap_or(0) + TICKET_OPEN => "open",
        None => "expired",
    }
}

fn rank(s: &str) -> u8 {
    match s {
        "closed" => 3,
        "uploaded" => 2,
        "open" => 1,
        _ => 0,
    }
}

/// One entry per ticket code: the fields of the relay with the furthest state (the newest upload
/// among uploads), `relays` = every relay that has the ticket, `upload_relay` = where the newest
/// zip is, `uploads` = every relay's upload of this code, newest first (`relay`, `uploaded`,
/// `size`). Each relay takes one upload per code, so two relays can each hold one: an app that
/// reached a relay where the code was still open after the other had taken an upload.
/// `archived`: keep only closed tickets (relays before close/archive ignore `?archived=1`).
pub fn merge(lists: Vec<(String, Vec<Value>)>, archived: bool, now: u64) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for (relay, list) in lists {
        for mut t in list {
            let Some(code) = t["ticket"].as_str().map(str::to_owned) else { continue };
            let st = state(&t, now);
            if (st == "closed") != archived {
                continue;
            }
            t["state"] = json!(st);
            t["relays"] = json!([relay]);
            t["uploads"] = json!([]);
            if let Some(at) = t["uploaded"].as_u64() {
                t["upload_relay"] = json!(relay);
                t["uploads"] = json!([{ "relay": relay, "uploaded": at, "size": t["size"] }]);
            }
            match out.iter_mut().find(|x| x["ticket"] == code.as_str()) {
                None => out.push(t),
                Some(m) => {
                    let mut relays = m["relays"].as_array().cloned().unwrap_or_default();
                    relays.push(json!(relay));
                    let mut uploads = m["uploads"].as_array().cloned().unwrap_or_default();
                    uploads.extend(t["uploads"].as_array().cloned().unwrap_or_default());
                    let newer = t["uploaded"].as_u64() > m["uploaded"].as_u64();
                    if rank(st) > rank(m["state"].as_str().unwrap_or("")) || (rank(st) == rank(m["state"].as_str().unwrap_or("")) && newer) {
                        *m = t;
                    }
                    m["relays"] = json!(relays);
                    m["uploads"] = json!(uploads);
                }
            }
        }
    }
    for t in &mut out {
        if let Some(u) = t["uploads"].as_array_mut() {
            u.sort_by_key(|u| std::cmp::Reverse(u["uploaded"].as_u64()));
        }
    }
    out.sort_by_key(|t| std::cmp::Reverse(t["created"].as_u64()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_by_code_and_finds_the_upload() {
        let now = 1_000_000;
        let a = vec![
            json!({"ticket": "AAAAAAAA", "note": "n", "created": now - 10, "uploaded": null, "size": null, "state": "open"}),
            json!({"ticket": "BBBBBBBB", "note": "old relay", "created": now - 200_000, "uploaded": null, "size": null}),
        ];
        let b = vec![
            json!({"ticket": "AAAAAAAA", "note": "n", "created": now - 10, "uploaded": now - 5, "size": 1234, "state": "uploaded"}),
            json!({"ticket": "CCCCCCCC", "note": "", "created": now - 5, "uploaded": null, "size": null, "state": "open"}),
        ];
        let m = merge(vec![("https://r1".into(), a.clone()), ("https://r2".into(), b.clone())], false, now);
        assert_eq!(m.len(), 3);
        assert_eq!(m[0]["ticket"], "CCCCCCCC");
        let t = m.iter().find(|t| t["ticket"] == "AAAAAAAA").unwrap();
        assert_eq!((t["state"].as_str(), t["size"].as_u64(), t["upload_relay"].as_str()), (Some("uploaded"), Some(1234), Some("https://r2")));
        assert_eq!(t["relays"], json!(["https://r1", "https://r2"]));
        assert_eq!(t["uploads"], json!([{"relay": "https://r2", "uploaded": now - 5, "size": 1234}]));
        // A relay without states: derived (24 h open).
        assert_eq!(m.iter().find(|t| t["ticket"] == "BBBBBBBB").unwrap()["state"], "expired");
        // The same lists in the other order give the same upload.
        let m2 = merge(vec![("https://r2".into(), b), ("https://r1".into(), a)], false, now);
        assert_eq!(m2.iter().find(|t| t["ticket"] == "AAAAAAAA").unwrap()["upload_relay"], "https://r2");
    }

    /// The same code uploaded on both relays (used on one while still open on the other): both
    /// uploads are listed, newest first, and the newest is the one `upload_relay` names.
    #[test]
    fn lists_an_upload_on_each_relay() {
        let now = 1_000_000;
        let a = vec![json!({"ticket": "AAAAAAAA", "note": "", "created": now - 10, "uploaded": now - 8, "size": 10, "state": "uploaded"})];
        let b = vec![json!({"ticket": "AAAAAAAA", "note": "", "created": now - 10, "uploaded": now - 3, "size": 20, "state": "uploaded"})];
        for lists in [vec![("r1".to_owned(), a.clone()), ("r2".to_owned(), b.clone())], vec![("r2".to_owned(), b.clone()), ("r1".to_owned(), a.clone())]] {
            let m = merge(lists, false, now);
            assert_eq!(m.len(), 1);
            assert_eq!(m[0]["upload_relay"], "r2");
            assert_eq!(m[0]["uploads"], json!([{"relay": "r2", "uploaded": now - 3, "size": 20}, {"relay": "r1", "uploaded": now - 8, "size": 10}]));
        }
        assert!((0..50).map(|_| new_ticket()).all(|t| valid_ticket(&t)));
    }

    #[test]
    fn archived_keeps_closed_only() {
        let now = 1_000_000;
        // An old relay answers ?archived=1 with its whole (active) list.
        let old = vec![json!({"ticket": "AAAAAAAA", "note": "", "created": now - 10, "uploaded": null, "size": null})];
        let new = vec![json!({"ticket": "BBBBBBBB", "note": "x", "created": now - 10, "uploaded": now - 9, "size": 9, "closed": now - 1, "state": "closed"})];
        let m = merge(vec![("o".into(), old.clone()), ("n".into(), new.clone())], true, now);
        assert_eq!(m.len(), 1);
        assert_eq!((m[0]["ticket"].as_str(), m[0]["state"].as_str()), (Some("BBBBBBBB"), Some("closed")));
        assert_eq!(merge(vec![("o".into(), old), ("n".into(), new)], false, now).len(), 1);
        assert!(valid_ticket("K7Q2M9XD") && !valid_ticket("K7Q2M9XI") && !valid_ticket("../x/abc"));
    }
}

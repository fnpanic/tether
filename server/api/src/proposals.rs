//! Proposals (design 12.4): drafts of a policy, a team document or a release publish that a person
//! signed in with a Tether Manage role (PolicyEditor, Publisher) stores on the relay for a key
//! holder to review, sign with the vault on their own PC and publish. A proposal is stored
//! unsigned in `proposals/<kind>/<id>.json` (0600), is served only on `GET
//! /api/admin/proposals/<kind>` and is never applied or served as live: what is live changes only
//! with an admin-signed publish (`PUT /api/admin/policy`, `team`, `release/…`).
//!
//! `PUT /api/admin/proposals/<kind>` body `{"base","note","doc"}`: `base` the SHA-256 (lower-case
//! hex) of the live document it was edited from, as the relay serves it (`GET /api/policy`,
//! `GET /api/admin/team`, the channel's release index; empty: there was none), so a proposal made
//! stale by a later publish is recognised; `doc` the unsigned payload (policy: a `Bundle`; team: a
//! `TeamDoc`, both validated as a publish would be; release: a JSON object). Stored with its
//! author (the session's person, or `admin` for an admin signature), time, base, serial (the
//! doc's). At most [`PER_KIND`] per kind. `GET` answers `{"live","proposals":[…oldest first]}`
//! (`live`: the hash of the policy or team document served now; `null` for releases or none).
//! `DELETE /api/admin/proposals/<kind>/<id>`: its author, a KeyHolder, or the admin signature.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tether_trust::policy::Bundle;
use tether_trust::team::{ManageRole, TeamDoc};

use crate::team::ManageCaller;
use crate::teamaudit::Actor;
use crate::{empty, internal, json, not_found, random_bytes, text, App, Resp};

pub const KINDS: [&str; 3] = ["policy", "team", "release"];
/// Proposals kept per kind: a new one beyond is refused until one is deleted.
pub const PER_KIND: usize = 10;
/// `PUT /api/admin/proposals/<kind>`: a team document (256 KiB) with room for the note and base.
pub const LIMIT_PROPOSAL: u64 = 400 << 10;
const NOTE_MAX: usize = 500;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Put {
    #[serde(default)]
    base: String,
    #[serde(default)]
    note: String,
    doc: Value,
}

#[derive(Serialize, Deserialize)]
struct Stored {
    id: String,
    kind: String,
    author: Actor,
    created: u64,
    base: String,
    note: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    serial: Option<u64>,
    doc: Value,
}

fn dir(data: &Path, kind: &str) -> PathBuf {
    data.join("proposals").join(kind)
}

pub fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// The doc's serial when it is valid for its kind.
fn check(kind: &str, doc: &Value) -> Result<Option<u64>, String> {
    match kind {
        "policy" => {
            let b = Bundle::from_json(&serde_json::to_vec(doc).map_err(|e| e.to_string())?)?;
            b.validate()?;
            Ok(Some(b.serial))
        }
        "team" => {
            let d: TeamDoc = serde_json::from_value(doc.clone()).map_err(|e| e.to_string())?;
            d.validate()?;
            Ok(Some(d.serial))
        }
        _ if doc.is_object() => Ok(doc["serial"].as_u64()),
        _ => Err("a release proposal is a JSON object".into()),
    }
}

fn valid_id(id: &str) -> bool {
    id.len() == 16 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn ids(d: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(d).into_iter().flatten().flatten().filter_map(|e| e.file_name().to_str()?.strip_suffix(".json").filter(|i| valid_id(i)).map(str::to_owned)).collect();
    v.sort();
    v
}

impl App {
    /// `/api/admin/proposals/<kind>[/<id>]` after the admin signature or a session with a role the
    /// route allows (main.rs `session_may`). `who`: the session, `None` for the admin signature.
    pub(crate) fn proposals(&mut self, method: &str, rest: &str, body: &[u8], actor: &Actor, who: Option<&ManageCaller>) -> Result<Resp, Resp> {
        let (kind, id) = rest.split_once('/').map_or((rest, None), |(k, i)| (k, Some(i)));
        if !KINDS.contains(&kind) {
            return Err(not_found());
        }
        let d = dir(&self.data, kind);
        match (method, id) {
            ("PUT", None) => {
                let p: Put = serde_json::from_slice(body).map_err(|e| text(400, &e.to_string()))?;
                if !(p.base.is_empty() || (p.base.len() == 64 && p.base.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))) {
                    return Err(text(400, "base: the SHA-256 of the live document in lower-case hex, or empty"));
                }
                if p.note.chars().count() > NOTE_MAX || p.note.chars().any(|c| c.is_control() && c != '\n') {
                    return Err(text(400, "note: at most 500 characters, no control characters but line breaks"));
                }
                let serial = check(kind, &p.doc).map_err(|e| text(400, &format!("not a valid {kind}: {e}")))?;
                if ids(&d).len() >= PER_KIND {
                    return Err(text(409, &format!("{PER_KIND} {kind} proposals are waiting: delete one first")));
                }
                self.check_space(body.len() as u64)?;
                let mut b = [0u8; 8];
                random_bytes(&mut b).map_err(internal)?;
                let id: String = b.iter().map(|x| format!("{x:02x}")).collect();
                let rec = Stored { id: id.clone(), kind: kind.into(), author: actor.clone(), created: crate::now(), base: p.base, note: p.note, serial, doc: p.doc };
                crate::mkdirs(&d, 0o700).map_err(internal)?;
                self.write_atomic(&d.join(format!("{id}.json")), &serde_json::to_vec(&rec).map_err(internal)?, 0o600).map_err(internal)?;
                if let Some(r) = self.audit_pending.as_mut() {
                    r.detail = json!({"kind": kind, "id": id, "serial": serial});
                }
                Ok(json(&json!({"id": id})))
            }
            ("GET", None) => {
                let live = match kind {
                    "policy" => fs::read(self.data.join("policy.json")).ok().map(|b| sha256_hex(&b)),
                    "team" => self.team_served().map(|b| sha256_hex(&b)),
                    _ => None,
                };
                self.defer(move |_| {
                    let mut list: Vec<Stored> = ids(&d).iter().filter_map(|i| serde_json::from_slice(&fs::read(d.join(format!("{i}.json"))).ok()?).ok()).collect();
                    list.sort_by_key(|s| s.created);
                    Ok(json(&json!({"live": live, "proposals": list})))
                })
            }
            ("DELETE", Some(id)) if valid_id(id) => {
                let f = d.join(format!("{id}.json"));
                let s: Stored = fs::read(&f).ok().and_then(|b| serde_json::from_slice(&b).ok()).ok_or_else(not_found)?;
                // A proposer removes their own; a key holder (after publishing or rejecting) and the admin any.
                if let Some(w) = who {
                    if !w.roles.contains(&ManageRole::KeyHolder) && s.author.oid.as_deref() != Some(w.oid.as_str()) {
                        return Err(text(403, "only its author or a key holder may delete a proposal"));
                    }
                }
                fs::remove_file(&f).map_err(internal)?;
                if let Some(r) = self.audit_pending.as_mut() {
                    r.detail = json!({"kind": kind, "id": id, "serial": s.serial, "author": s.author.name});
                }
                Ok(empty(204))
            }
            _ => Err(not_found()),
        }
    }
}

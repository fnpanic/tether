//! Team (technician) access: an admin-published roster of technician public keys
//! (`team/roster.json`, `tether_trust::team`) and the member endpoints under `/api/team/`, signed
//! with the member's own key (`reqauth`, same nonce cache as admin requests). Members read the
//! device list (no IPs), read/replace one shared tag document (`team/tags.json`, optimistic
//! concurrency on `rev`) and, with `audit`, one device's session audit records. docs/api.md.

use std::collections::HashMap;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tether_trust::{reqauth, team::Roster, VerifyingKey};
use tiny_http::Request;

use crate::{audit, client_ip, header, json, load_json, mkdirs, now, rate_ok, text, App, Body, Resp, LIMIT_SMALL};

pub const LIMIT_ROSTER: u64 = 64 << 10;
const LIMIT_TAGS: u64 = 512 << 10;
const KEY_PER_MIN: u32 = 120;
/// Failed member signatures per IP and minute: each failure tries every roster key under the lock.
const FAIL_PER_MIN: u32 = 60;
const AUDIT_DEFAULT: usize = 200;
const AUDIT_MAX: usize = 500;

#[derive(Serialize, Deserialize, Default, Clone)]
struct Tags {
    rev: u64,
    doc: Value,
    updated_by: String,
    updated_at: u64,
}

#[derive(Default)]
pub struct Team {
    roster: Roster,
    keys: Vec<VerifyingKey>,
    tags: Tags,
    key_hits: HashMap<String, (u64, u32)>,
    fail_hits: HashMap<String, (u64, u32)>,
}

impl Team {
    pub fn open(data: &Path) -> io::Result<Self> {
        mkdirs(&data.join("team"), 0o700)?;
        let roster: Roster = load_json(&data.join("team/roster.json"));
        let (roster, keys) = match roster.validate() {
            Ok(k) => (roster, k),
            Err(e) => {
                eprintln!("warning: ignoring invalid team/roster.json: {e}");
                (Roster::default(), Vec::new())
            }
        };
        Ok(Team { roster, keys, tags: load_json(&data.join("team/tags.json")), ..Default::default() })
    }

    pub fn maintain(&mut self, t: u64) {
        self.key_hits.retain(|_, (start, _)| t < *start + 60);
        self.fail_hits.retain(|_, (start, _)| t < *start + 60);
    }
}

impl App {
    /// `PUT /api/admin/team` (already admin-authenticated).
    pub(crate) fn put_team(&mut self, body: &[u8]) -> Result<Resp, Resp> {
        let roster: Roster = serde_json::from_slice(body).map_err(|e| text(400, &e.to_string()))?;
        let keys = roster.validate().map_err(|e| text(400, &e))?;
        self.check_space(body.len() as u64)?;
        let bytes = serde_json::to_vec(&roster).map_err(crate::internal)?;
        self.write_atomic(&self.data.join("team/roster.json"), &bytes, 0o600).map_err(crate::internal)?;
        self.team.roster = roster;
        self.team.keys = keys;
        Ok(crate::empty(204))
    }

    pub(crate) fn get_team(&self) -> Result<Resp, Resp> {
        Ok(json(&self.team.roster))
    }

    /// `/api/team/<rest>`: size check, member signature, rate limit, then the handler.
    pub(crate) fn team_route(&mut self, rq: &Request, body: &Body, method: &str, url: &str, rest: &str) -> Result<Resp, Resp> {
        let limit = match (method, rest) {
            ("GET", "me" | "devices" | "tags" | "audit") => LIMIT_SMALL,
            ("PUT", "tags") => LIMIT_TAGS,
            _ => return Err(crate::not_found()),
        };
        let body = body.bytes(limit)?;
        let i = self.member_auth(rq, method, url, body)?;
        let t = now();
        if !rate_ok(&mut self.team.key_hits, self.team.roster.members[i].key.clone(), t, KEY_PER_MIN) {
            return Err(text(429, "too many requests"));
        }
        let m = self.team.roster.members[i].clone();
        match (method, rest) {
            ("GET", "me") => Ok(json(&serde_json::json!({"name": m.name, "write": m.write, "audit": m.audit}))),
            ("GET", "devices") => {
                let mut v: Vec<_> = self.devices.values().collect();
                v.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
                let out: Vec<_> = v
                    .iter()
                    .map(|d| serde_json::json!({"uuid": d.uuid, "id": d.id, "profile": d.profile, "version": d.version, "os": d.os, "last_seen": d.last_seen}))
                    .collect();
                Ok(json(&out))
            }
            ("GET", "tags") => Ok(json(&self.team.tags)),
            ("PUT", "tags") => {
                if !m.write {
                    return Err(text(403, "this member may not edit the shared tags"));
                }
                let rev: u64 = header(rq, "If-Match")
                    .and_then(|v| v.trim().trim_matches('"').parse().ok())
                    .ok_or_else(|| text(400, "If-Match: <rev> required"))?;
                let doc: Value = serde_json::from_slice(body).map_err(|e| text(400, &e.to_string()))?;
                if !doc.is_object() {
                    return Err(text(400, "the tag document must be a JSON object"));
                }
                if rev != self.team.tags.rev {
                    let cur = serde_json::to_vec(&self.team.tags).map_err(crate::internal)?;
                    return Err(tiny_http::Response::from_data(cur)
                        .with_status_code(409)
                        .with_header(crate::hdr("Content-Type", "application/json"))
                        .boxed());
                }
                self.check_space(body.len() as u64)?;
                let next = Tags { rev: rev + 1, doc, updated_by: m.name, updated_at: t };
                let bytes = serde_json::to_vec(&next).map_err(crate::internal)?;
                self.write_atomic(&self.data.join("team/tags.json"), &bytes, 0o600).map_err(crate::internal)?;
                self.team.tags = next;
                Ok(json(&serde_json::json!({"rev": rev + 1})))
            }
            (_, _) => {
                if !m.audit {
                    return Err(text(403, "this member may not read the session history"));
                }
                self.team_audit(url)
            }
        }
    }

    /// Index of the roster member whose key signed the request. Same rules and nonce cache as
    /// `App::auth`.
    fn member_auth(&mut self, rq: &Request, method: &str, url: &str, body: &[u8]) -> Result<usize, Resp> {
        let ip = client_ip(rq);
        let t = now();
        if self.team.fail_hits.get(&ip).is_some_and(|h| t < h.0 + 60 && h.1 >= FAIL_PER_MIN) {
            return Err(text(429, "too many failed requests"));
        }
        let r = (|| {
            if self.team.keys.is_empty() {
                return Err("team access disabled".to_owned());
            }
            let sig = header(rq, reqauth::HEADER).ok_or_else(|| "missing signature".to_owned())?;
            let mut err = String::new();
            for (i, key) in self.team.keys.iter().enumerate() {
                let nonces = &mut self.nonces;
                let unused = |n: &str| nonces.insert(n.to_owned(), t).is_none();
                match reqauth::verify_request(&sig, key, method, url, body, t, unused) {
                    Ok(()) => return Ok(i),
                    Err(e) if err.is_empty() || e != "signature does not match" => err = e,
                    Err(_) => {}
                }
            }
            Err(if err == "signature does not match" { "not a team member".to_owned() } else { err })
        })();
        match r {
            Ok(i) => {
                let nonces = self.nonces.clone();
                self.save("nonces.json", &nonces).map_err(crate::internal)?;
                Ok(i)
            }
            Err(e) => {
                rate_ok(&mut self.team.fail_hits, ip, t, u32::MAX);
                Err(text(401, &e))
            }
        }
    }

    /// `GET /api/team/audit?device=<id or uuid>&since=<unix>&limit=<1..500>`.
    fn team_audit(&self, url: &str) -> Result<Resp, Resp> {
        let (mut since, mut limit, mut device) = (0, AUDIT_DEFAULT, String::new());
        for kv in url.split_once('?').map_or("", |(_, q)| q).split('&').filter(|s| !s.is_empty()) {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            let bad = || text(400, &format!("bad {k}"));
            match k {
                "since" => since = v.parse().map_err(|_| bad())?,
                "limit" => limit = v.parse::<usize>().ok().filter(|n| (1..=AUDIT_MAX).contains(n)).ok_or_else(bad)?,
                "device" => device = audit::unescape(v),
                _ => return Err(text(400, &format!("unknown parameter {k}"))),
            }
        }
        if device.is_empty() {
            return Err(text(400, "device required"));
        }
        Ok(json(&self.audit.read(since, limit, &device)))
    }
}

//! Team (technician) access: an admin-published roster of technician public keys
//! (`team/roster.json`, `tether_trust::team`) and the member endpoints under `/api/team/`, signed
//! with the member's own key (`reqauth`, same nonce cache as admin requests). Members read the
//! device list (no IPs), read/replace one shared tag document (`team/tags.json`, optimistic
//! concurrency on `rev`) and, with `audit`, one device's session audit records. docs/api.md.
//!
//! The admin-signed team document (`team/team.json`, the upload as received) replaces the plain
//! roster: once one is accepted, `roster.json` is removed and plain rosters are refused for as
//! long as `team.json` exists (deleting it by hand, like deleting `policy.json`, starts over).
//! It gives each member permissions (`sbom`, `customers`: enforced in status.rs) and may carry
//! the `oidc` part, served signed as `GET /api/team/config`.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tether_trust::team::{self, OpenError, Permissions, Roster, Upload};
use tether_trust::{reqauth, VerifyingKey};
use tiny_http::Request;

use crate::{audit, empty, hdr, header, json, load_json, mkdirs, now, rate_ok, text, App, Body, Resp, LIMIT_SMALL};

pub const LIMIT_ROSTER: u64 = 64 << 10;
/// `PUT /api/admin/team`: a signed upload; a plain roster stays at `LIMIT_ROSTER`.
pub const LIMIT_TEAM: u64 = team::MAX_UPLOAD as u64;
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

/// A device as members see it (no IP).
#[derive(Serialize)]
struct MemberDevice<'a> {
    uuid: &'a str,
    id: &'a str,
    profile: &'a str,
    version: &'a str,
    os: &'a str,
    last_seen: u64,
}

/// A member as the relay checks it.
#[derive(Clone)]
pub(crate) struct Who {
    pub name: String,
    /// Standard base64 (the rate limit's key).
    key: String,
    pub perms: Permissions,
    service: bool,
}

/// The signed team document in force.
struct Doc {
    /// 0: `team.json` exists but could not be read (nobody is a member until a new one arrives).
    serial: u64,
    /// `GET /api/team/config`: the config envelope as JSON, with the document's serial.
    config: Option<Vec<u8>>,
}

#[derive(Default)]
pub struct Team {
    roster: Roster,
    doc: Option<Doc>,
    /// Members whose keys are accepted, their keys and key ids (`kid`) in the same order.
    members: Vec<Who>,
    keys: Vec<VerifyingKey>,
    kids: HashMap<String, usize>,
    tags: Tags,
    key_hits: HashMap<String, (u64, u32)>,
    fail_hits: HashMap<String, (u64, u32)>,
}

impl Team {
    pub fn open(data: &Path) -> io::Result<Self> {
        mkdirs(&data.join("team"), 0o700)?;
        let mut t = Team { tags: load_json(&data.join("team/tags.json")), ..Default::default() };
        match fs::read(data.join("team/team.json")) {
            Ok(raw) => match open_stored(&raw) {
                Ok((d, keys)) => t.set_doc(&d, keys, &raw),
                Err(e) => {
                    eprintln!("error: team/team.json: {e}; no team member is accepted until a signed team document is published again");
                    t.doc = Some(Doc { serial: 0, config: None });
                }
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let roster: Roster = load_json(&data.join("team/roster.json"));
                match roster.validate() {
                    Ok(keys) => t.set_roster(roster, keys),
                    Err(e) => eprintln!("warning: ignoring invalid team/roster.json: {e}"),
                }
            }
            Err(e) => {
                eprintln!("error: team/team.json: {e}; no team member is accepted until it is readable");
                t.doc = Some(Doc { serial: 0, config: None });
            }
        }
        Ok(t)
    }

    fn set_members(&mut self, members: Vec<Who>, keys: Vec<VerifyingKey>) {
        self.kids = keys.iter().enumerate().map(|(i, k)| (team::key_id(k), i)).collect();
        (self.members, self.keys) = (members, keys);
    }

    fn set_roster(&mut self, roster: Roster, keys: Vec<VerifyingKey>) {
        let members = roster.members.iter().map(|m| Who { name: m.name.clone(), key: m.key.clone(), perms: m.permissions(), service: false }).collect();
        self.set_members(members, keys);
        self.roster = roster;
    }

    /// With `oidc_only`, only service entries keep their keys.
    fn set_doc(&mut self, d: &team::TeamDoc, keys: Vec<VerifyingKey>, raw: &[u8]) {
        let (members, keys) = d
            .members
            .iter()
            .zip(keys)
            .filter(|(m, _)| m.service || !d.oidc_only)
            .map(|(m, k)| (Who { name: m.name.clone(), key: m.key.clone(), perms: m.permissions(), service: m.service }, k))
            .unzip();
        self.set_members(members, keys);
        let config = serde_json::from_slice::<Upload>(raw).ok().and_then(|u| u.config).and_then(|c| serde_json::to_vec(&c).ok());
        self.roster = Roster::default();
        self.doc = Some(Doc { serial: d.serial, config });
    }

    pub fn maintain(&mut self, t: u64) {
        self.key_hits.retain(|_, (start, _)| t < *start + 60);
        self.fail_hits.retain(|_, (start, _)| t < *start + 60);
    }
}

/// A stored upload, checked against the keys it names (it was checked against `ADMIN_PUB_KEY`
/// when it arrived), so it stays in force across an admin key rotation, as the policy does.
fn open_stored(raw: &[u8]) -> Result<(team::TeamDoc, Vec<VerifyingKey>), String> {
    let up: Upload = serde_json::from_slice(raw).map_err(|e| e.to_string())?;
    let named = std::iter::once(&up.team).chain(&up.config).map(|s| tether_trust::key_from_b64(&s.key)).collect::<Result<Vec<_>, _>>()?;
    team::open(raw, &named).map_err(|e| match e {
        OpenError::Bad(e) | OpenError::Untrusted(e) => e,
    })
}

impl App {
    /// `PUT /api/admin/team` (already admin-authenticated): a signed upload, or a plain roster
    /// until the first signed one was accepted.
    pub(crate) fn put_team(&mut self, body: &[u8]) -> Result<Resp, Resp> {
        if serde_json::from_slice::<Upload>(body).is_ok() {
            return self.put_team_doc(body);
        }
        if body.len() as u64 > LIMIT_ROSTER {
            return Err(crate::too_large());
        }
        let roster: Roster = serde_json::from_slice(body).map_err(|e| text(400, &e.to_string()))?;
        if self.team.doc.is_some() {
            return Err(text(409, "a signed team document is in force on this relay: publish a signed team document"));
        }
        let keys = roster.validate().map_err(|e| text(400, &e))?;
        self.check_space(body.len() as u64)?;
        let bytes = serde_json::to_vec(&roster).map_err(crate::internal)?;
        self.write_atomic(&self.data.join("team/roster.json"), &bytes, 0o600).map_err(crate::internal)?;
        self.team.set_roster(roster, keys);
        Ok(empty(204))
    }

    fn put_team_doc(&mut self, body: &[u8]) -> Result<Resp, Resp> {
        let (d, keys) = team::open(body, &self.keys).map_err(|e| match e {
            OpenError::Bad(e) => text(400, &e),
            OpenError::Untrusted(e) => text(401, &e),
        })?;
        if self.team.doc.as_ref().is_some_and(|c| d.serial <= c.serial) {
            return Err(text(409, "serial must be greater than the current team document's"));
        }
        self.check_space(body.len() as u64)?;
        self.write_atomic(&self.data.join("team/team.json"), body, 0o600).map_err(crate::internal)?;
        // team.json exists from here on: a plain roster left behind would never be read again.
        if let Err(e) = fs::remove_file(self.data.join("team/roster.json")) {
            if e.kind() != io::ErrorKind::NotFound {
                eprintln!("warning: team/roster.json: {e}");
            }
        }
        self.team.set_doc(&d, keys, body);
        eprintln!("team: signed team document {} accepted ({} members, {} with a key here)", d.serial, d.members.len(), self.team.members.len());
        Ok(empty(204))
    }

    /// The plain roster, or the signed upload exactly as published.
    pub(crate) fn get_team(&self) -> Result<Resp, Resp> {
        if self.team.doc.is_none() {
            return Ok(json(&self.team.roster));
        }
        let raw = fs::read(self.data.join("team/team.json")).map_err(crate::internal)?;
        Ok(tiny_http::Response::from_data(raw).with_header(hdr("Content-Type", "application/json")).boxed())
    }

    /// `GET /api/team/config` (no signature): the `oidc` part of the team document, as the admin
    /// signed it (`tether_trust::team::Config`), `ETag: "<serial>"`.
    pub(crate) fn team_config(&self, rq: &Request) -> Result<Resp, Resp> {
        let (serial, cfg) = self.team.doc.as_ref().and_then(|d| Some((d.serial, d.config.as_ref()?))).ok_or_else(|| text(404, "no sign-in configured"))?;
        let etag = format!("\"{serial}\"");
        if header(rq, "If-None-Match").is_some_and(|v| v.split(',').any(|t| t.trim().trim_start_matches("W/") == etag)) {
            return Ok(empty(304).with_header(hdr("ETag", &etag)));
        }
        Ok(tiny_http::Response::from_data(cfg.clone()).with_header(hdr("Content-Type", "application/json")).with_header(hdr("ETag", &etag)).boxed())
    }

    /// `/api/team/<rest>`: size check, member signature, rate limit, then the handler.
    pub(crate) fn team_route(&mut self, rq: &Request, body: &Body, method: &str, url: &str, rest: &str) -> Result<Resp, Resp> {
        let limit = match (method, rest) {
            ("GET", "me" | "devices" | "tags" | "audit" | "status") => LIMIT_SMALL,
            ("GET", r) if r.starts_with("status/") => LIMIT_SMALL,
            ("POST", r) if r.starts_with("status/") && (r.ends_with("/link") || r.ends_with("/allow-claim")) => LIMIT_SMALL,
            ("DELETE", r) if r.starts_with("status/") => LIMIT_SMALL,
            ("PUT", "tags") => LIMIT_TAGS,
            _ => return Err(crate::not_found()),
        };
        let body = body.bytes(limit)?;
        let i = self.member_auth(rq, method, url, body)?;
        let t = now();
        if !rate_ok(&mut self.team.key_hits, self.team.members[i].key.clone(), t, KEY_PER_MIN) {
            return Err(text(429, "too many requests"));
        }
        let Who { name, perms, service, .. } = self.team.members[i].clone();
        let m = &perms;
        match (method, rest) {
            // The plain roster's answer stays as it was; with a signed document every permission.
            ("GET", "me") if self.team.doc.is_none() => Ok(json(&serde_json::json!({"name": name, "write": m.write, "audit": m.audit}))),
            ("GET", "me") => Ok(json(&serde_json::json!({"name": name, "write": m.write, "audit": m.audit, "sbom": m.sbom, "agent": m.agent, "customers": m.customers, "service": service}))),
            ("GET", "devices") => {
                let mut v: Vec<_> = self.devices.values().collect();
                v.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
                let out = v.iter().map(|d| MemberDevice { uuid: &d.uuid, id: &d.id, profile: &d.profile, version: &d.version, os: &d.os, last_seen: d.last_seen });
                self.json_file(|w| serde::Serializer::collect_seq(&mut serde_json::Serializer::new(w), out))
            }
            ("GET", "tags") => Ok(json(&self.team.tags)),
            // Every status route sees only the devices of the member's customers: one outside them
            // answers as an unknown one (status.rs).
            ("GET", "status") => self.team_status_summary(rq, m),
            ("GET", r) if r.strip_prefix("status/").is_some_and(|u| u.ends_with("/sbom")) => {
                if !m.sbom {
                    return Err(text(403, "this member may not read software lists"));
                }
                self.team_status_sbom(rq, r["status/".len()..].strip_suffix("/sbom").unwrap_or_default(), m)
            }
            ("GET", r) if r.starts_with("status/") => self.team_status_host(&r["status/".len()..], m),
            // Health links and removal change what the team sees for a host: `write` only.
            ("POST" | "DELETE", r) if r.starts_with("status/") => {
                if !m.write {
                    return Err(text(403, "this member may not change health monitoring"));
                }
                let r = &r["status/".len()..];
                match (method, r.strip_suffix("/link")) {
                    ("POST", Some(uuid)) => self.team_status_link(uuid, body, &name, m),
                    ("DELETE", Some(uuid)) => self.team_status_unlink(uuid, &name, m),
                    ("POST", None) => self.team_status_allow_claim(r.strip_suffix("/allow-claim").unwrap_or_default(), &name, m),
                    _ => self.admin_status_delete(r, &format!("team member {name}"), false, Some(m)),
                }
            }
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
                let next = Tags { rev: rev + 1, doc, updated_by: name, updated_at: t };
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
        let ip = crate::rate_ip(rq);
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
            // Once, not per roster key (up to 2000, under the state lock).
            let digest = reqauth::body_digest(body);
            // `kid=`: that key only (unknown: refused, no other key tried); without: every key.
            let range = match reqauth::header_kid(&sig) {
                Some(k) => {
                    let i = *self.team.kids.get(k).ok_or_else(|| "not a team member".to_owned())?;
                    i..i + 1
                }
                None => 0..self.team.keys.len(),
            };
            for (i, key) in self.team.keys.iter().enumerate().skip(range.start).take(range.len()) {
                let nonces = &mut self.nonces;
                let unused = |n: &str| !nonces.contains_key(n) && nonces.insert(n.to_owned(), t).is_none();
                match reqauth::verify_request_for(&sig, key, crate::sig_relay(), method, url, &digest, t, unused) {
                    Ok(()) => return Ok(i),
                    Err(e) if err.is_empty() || e != "signature does not match" => err = e,
                    Err(_) => {}
                }
            }
            Err(if err == "signature does not match" { "not a team member".to_owned() } else { err })
        })();
        match r {
            Ok(i) => {
                self.nonce_used();
                Ok(i)
            }
            Err(e) => {
                rate_ok(&mut self.team.fail_hits, ip, t, u32::MAX);
                Err(text(401, &e))
            }
        }
    }

    /// `GET /api/team/audit?device=<id or uuid>&since=<unix>&limit=<1..500>`.
    fn team_audit(&mut self, url: &str) -> Result<Resp, Resp> {
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
        let dir = self.audit.dir.clone();
        self.defer(move |_| Ok(audit::response(&dir, since, limit, &device)))
    }
}

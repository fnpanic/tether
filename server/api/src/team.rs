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
//!
//! Member requests that read or change a device's data leave a team audit record (`audited`,
//! teamaudit.rs), and `POST /api/team/audit-event` takes the desktop's own events.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tether_trust::team::{self, OpenError, Permissions, Roster, Upload};
use tether_trust::{reqauth, VerifyingKey};
use tiny_http::Request;

use crate::teamaudit::{self, Actor, Rec, Target};
use crate::{audit, empty, hdr, header, json, load_json, lock, mkdirs, now, rate_ok, text, App, Body, Resp, LIMIT_SMALL};

pub const LIMIT_ROSTER: u64 = 64 << 10;
/// `PUT /api/admin/team`: a signed upload; a plain roster stays at `LIMIT_ROSTER`.
pub const LIMIT_TEAM: u64 = team::MAX_UPLOAD as u64;
const LIMIT_TAGS: u64 = 512 << 10;
const KEY_PER_MIN: u32 = 120;
/// Failed member signatures per IP and minute: a failure without `kid` tries every member key.
const FAIL_PER_MIN: u32 = 60;
/// Signatures without `kid` being checked (up to 2000 keys, about 105 ms each in a release build):
/// two at a time for the whole relay and one per source address, so they cannot keep many workers
/// busy and one source cannot hold both; at most two requests (one per source) wait up to
/// `NO_KID_WAIT` for a slot (outside the state lock), any other gets `429 busy` at once and counts
/// as a failed signature. `kid` requests are not limited by it.
static NO_KID: Slots = Slots::new(2);
const NO_KID_WAIT: Duration = Duration::from_secs(1);

/// A fixed number of slots, at most one per source, and at most as many waiters as slots, at most
/// one per source: a waiter holds a worker, so everything beyond is refused at once.
struct Slots {
    /// Sources holding a slot, and sources waiting for one.
    held: Mutex<(Vec<String>, Vec<String>)>,
    freed: Condvar,
    max: usize,
}

impl Slots {
    const fn new(max: usize) -> Self {
        Slots { held: Mutex::new((Vec::new(), Vec::new())), freed: Condvar::new(), max }
    }

    /// A slot for `src`, waiting up to `wait` for one if a waiter's place is free; `None` after
    /// that, or at once.
    fn take(&'static self, src: &str, wait: Duration) -> Option<Slot> {
        let end = Instant::now() + wait;
        let mut g = self.held.lock().unwrap_or_else(|e| e.into_inner());
        let free = |g: &(Vec<String>, Vec<String>)| g.0.len() < self.max && !g.0.iter().any(|h| h == src);
        if !free(&g) && (g.1.len() >= self.max || g.1.iter().any(|w| w == src)) {
            return None;
        }
        let waiting = !free(&g);
        if waiting {
            g.1.push(src.to_owned());
        }
        let got = loop {
            if free(&g) {
                break true;
            }
            match end.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()) {
                Some(left) => g = self.freed.wait_timeout(g, left).unwrap_or_else(|e| e.into_inner()).0,
                None => break false,
            }
        };
        if waiting {
            if let Some(i) = g.1.iter().position(|w| w == src) {
                g.1.swap_remove(i);
            }
        }
        got.then(|| {
            g.0.push(src.to_owned());
            Slot(self, src.to_owned())
        })
    }
}

struct Slot(&'static Slots, String);

impl Drop for Slot {
    fn drop(&mut self) {
        let mut g = self.0.held.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = g.0.iter().position(|h| *h == self.1) {
            g.0.swap_remove(i);
        }
        self.0.freed.notify_all();
    }
}
/// `oidc_only` locks out every member key but the service ones: refused until this relay serves
/// sign-in sessions (`POST /api/team/session`, a later task sets this).
const OIDC_SESSIONS: bool = false;
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

/// The members whose keys are accepted, their keys and key ids (`kid`) in the same order. One
/// immutable snapshot, replaced whole when the roster or document changes: signatures are checked
/// on it without the state lock ([`member_verify`]).
#[derive(Default)]
pub(crate) struct Members {
    who: Vec<Who>,
    keys: Vec<VerifyingKey>,
    kids: HashMap<String, usize>,
}

/// What [`member_verify`] found, before the state lock: the snapshot, the index of the key that
/// signed and the nonce (still to be checked for replay), or why not.
pub(crate) struct Verified {
    r: Result<(Arc<Members>, usize, String), Refused>,
    t: u64,
}

enum Refused {
    /// Too many failed signatures from this IP.
    Limited,
    /// Another signature without `kid` is being checked.
    Busy,
    /// Not a member's valid signature (`401` with this text, counts as a failure).
    Auth(String),
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
    members: Arc<Members>,
    /// This request's [`member_verify`] (set by `serve` around `route`).
    pub(crate) verified: Option<Verified>,
    tags: Tags,
    key_hits: HashMap<String, (u64, u32)>,
    fail_hits: HashMap<String, (u64, u32)>,
    /// `POST /api/team/audit-event` per member key.
    posted_hits: HashMap<String, (u64, u32)>,
    /// Summary and device-list reads, one record per actor and hour (also the admin's, main.rs).
    pub(crate) reads: teamaudit::Folded,
    /// Sources refused by `FAIL_PER_MIN`: when their last `signature-limit` record was written.
    limit_noted: HashMap<String, u64>,
    /// `signature-limit` records relay-wide: (minute start, written in it, not written since the last one).
    limit_records: (u64, u32, u64),
}

/// `signature-limit` records (and journal lines) per minute, relay-wide: many source addresses
/// (IPv6) must not fill the audit log and the journal; the rest is counted (`more` of the next).
const LIMIT_RECORDS_PER_MIN: u32 = 60;

/// A `kid` as a source sent it, if it has the key-id form (16 lower-case hex); anything else is
/// the sender's own text and is not written anywhere.
fn kid_form(k: &str) -> Option<&str> {
    (k.len() == 16 && k.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))).then_some(k)
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

    fn set_members(&mut self, who: Vec<Who>, keys: Vec<VerifyingKey>) {
        let kids = keys.iter().enumerate().map(|(i, k)| (team::key_id(k), i)).collect();
        self.members = Arc::new(Members { who, keys, kids });
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

    /// A signed team document is in force (one that could not be read has no members).
    pub(crate) fn signed(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.serial > 0)
    }

    pub fn maintain(&mut self, t: u64) {
        self.key_hits.retain(|_, (start, _)| t < *start + 60);
        self.fail_hits.retain(|_, (start, _)| t < *start + 60);
        self.posted_hits.retain(|_, (start, _)| t < *start + 60);
        self.limit_noted.retain(|_, at| t < *at + 60);
    }

    /// Whether a `signature-limit` record may be written at `t` (relay-wide cap), and how many were
    /// left out since the last one written.
    fn limit_record(&mut self, t: u64) -> Option<u64> {
        let (start, n, more) = &mut self.limit_records;
        if t >= *start + 60 {
            (*start, *n) = (t, 0);
        }
        if *n >= LIMIT_RECORDS_PER_MIN {
            *more += 1;
            return None;
        }
        *n += 1;
        Some(std::mem::take(more))
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
        // The shape decides which parser answers: an object with `team` is a signed upload (and
        // gets its errors), anything else a plain roster.
        if serde_json::from_slice::<HashMap<String, serde::de::IgnoredAny>>(body).is_ok_and(|m| m.contains_key("team")) {
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
        if d.oidc_only && !OIDC_SESSIONS {
            return Err(text(400, "oidc_only needs sign-in support on this relay"));
        }
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
        eprintln!("team: signed team document {} accepted ({} members, {} with a key here)", d.serial, d.members.len(), self.team.members.who.len());
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
        let limit = route_limit(method, rest).ok_or_else(crate::not_found)?;
        let body = body.bytes(limit)?;
        let i = self.member_auth(rq)?;
        let t = now();
        if !rate_ok(&mut self.team.key_hits, self.team.members.who[i].key.clone(), t, KEY_PER_MIN) {
            return Err(text(429, "too many requests"));
        }
        let Who { name, perms, service, key } = self.team.members.who[i].clone();
        let m = &perms;
        let actor = Actor { key_fingerprint: team::key_id(&self.team.members.keys[i]), name: name.clone() };
        let ip = crate::client_ip(rq);
        if let ("GET", r @ ("status" | "devices")) = (method, rest) {
            self.audit_pending = self.team.reads.read(if r == "status" { "summary" } else { "device-list" }, actor.clone(), Some(ip.clone()), t);
        } else if let Some((event, target, detail)) = self.audited(method, rest, url, body) {
            self.audit_pending = Some(Rec::new(event, Some(actor.clone()), Some(ip.clone()), target, detail));
        }
        match (method, rest) {
            // What the member's desktop did on its own (an export, a session): recorded as it says.
            ("POST", "audit-event") => {
                if !rate_ok(&mut self.team.posted_hits, key, t, teamaudit::POSTED_PER_MIN) {
                    return Err(text(429, "too many requests"));
                }
                let (event, target, detail) = teamaudit::posted(body).map_err(|e| text(400, &e))?;
                self.audit_pending = Some(Rec { self_reported: true, ..Rec::new(event, Some(actor), Some(ip), target, detail) });
                Ok(empty(204))
            }
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
            // Hosts by installed program: `sbom` and the member's customers (status.rs).
            ("GET", "status/software") => {
                let signed = self.team.signed();
                self.team_status_software(url, m, &key, signed)
            }
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
                self.team_audit(url, m)
            }
        }
    }

    /// Index (in the current members) of the member whose key signed the request, from this
    /// request's [`member_verify`]: here, under the lock, only the replay check and whether that
    /// key is still a member (a document may have replaced the snapshot it was checked on).
    fn member_auth(&mut self, rq: &Request) -> Result<usize, Resp> {
        let ip = crate::rate_ip(rq);
        let Verified { r, t } = self.team.verified.take().ok_or_else(|| crate::internal("member signature not checked"))?;
        let r = match r {
            Err(Refused::Limited) => return Err(text(429, "too many failed requests")),
            // Not a failed signature (none was checked): several old desktops behind one address
            // must not lock it out. The waiter bound of `NO_KID` limits what they hold.
            Err(Refused::Busy) => return Err(text(429, "busy").with_header(hdr("Retry-After", "1"))),
            Err(Refused::Auth(e)) => Err(e),
            Ok((snap, i, nonce)) => {
                let cur = &self.team.members;
                let i = if Arc::ptr_eq(&snap, cur) { Some(i) } else { cur.keys.iter().position(|k| *k == snap.keys[i]) };
                match i {
                    None => Err("not a team member".to_owned()),
                    Some(_) if self.nonces.contains_key(&nonce) => Err("replayed request".to_owned()),
                    Some(i) => {
                        self.nonces.insert(nonce, t);
                        Ok(i)
                    }
                }
            }
        };
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

    /// The team audit record a member request leaves, before its handler runs (a removed device's
    /// pin is still there): event, target, detail. The summary and the device list are folded
    /// per member and hour (`team_route`, teamaudit::Folded).
    fn audited(&mut self, method: &str, rest: &str, url: &str, body: &[u8]) -> Option<(&'static str, Target, Value)> {
        let none = Value::Null;
        let dev = |r: &str, suffix: &str| r.strip_prefix("status/")?.strip_suffix(suffix).map(str::to_owned);
        Some(match (method, rest) {
            ("GET", "audit") => ("session-audit-read", query_param(url, "device").map(|d| Target::asked(&d)).unwrap_or_default(), none),
            // The term and version asked; the search adds the number of hits (never the hits).
            ("GET", "status/software") => ("search", Target::default(), crate::status::search_detail(url)),
            ("GET", r) => match dev(r, "/sbom") {
                Some(u) => ("software-list", self.audit_target(&u), none),
                None => ("report", self.audit_target(&dev(r, "")?), none),
            },
            ("POST", r) => match (dev(r, "/link"), dev(r, "/allow-claim")) {
                (Some(u), _) => {
                    let id = serde_json::from_slice::<Value>(body).ok().and_then(|v| v["tether_id"].as_str().filter(|i| tether_trust::status::valid_tether_id(i)).map(str::to_owned));
                    ("link", self.audit_target(&u), id.map_or(none, |i| serde_json::json!({"tether_id": i})))
                }
                (_, Some(u)) => ("allow-claim", self.audit_target(&u), none),
                _ => return None,
            },
            ("DELETE", r) => match dev(r, "/link") {
                Some(u) => ("unlink", self.audit_target(&u), none),
                None => ("remove", self.audit_target(&dev(r, "")?), none),
            },
            _ => return None,
        })
    }

    /// `GET /api/team/audit?device=<id or uuid>&since=<unix>&limit=<1..500>`.
    fn team_audit(&mut self, url: &str, scope: &Permissions) -> Result<Resp, Resp> {
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
        // Not every customer (only `*` covers devices without one): only the records of a device
        // in the scope, by the Tether ID its pin is verified for; any other answers as unknown.
        if !scope.sees(None) {
            match self.scoped_tether_id(&device, scope) {
                Some(id) => device = id,
                None => return Ok(audit::answer(Vec::new(), false)),
            }
        }
        let dir = self.audit.dir.clone();
        self.defer(move |_| Ok(audit::response(&dir, since, limit, &device)))
    }
}

/// The value of `k` in `url`'s query, URL-decoded.
pub(crate) fn query_param(url: &str, k: &str) -> Option<String> {
    url.split_once('?')?.1.split('&').find_map(|kv| kv.strip_prefix(k)?.strip_prefix('=')).map(audit::unescape)
}

/// The body limit of a member route; `None`: not one (`404`).
fn route_limit(method: &str, rest: &str) -> Option<u64> {
    match (method, rest) {
        ("GET", "me" | "devices" | "tags" | "audit" | "status") => Some(LIMIT_SMALL),
        ("GET", r) if r.starts_with("status/") => Some(LIMIT_SMALL),
        ("POST", r) if r.starts_with("status/") && (r.ends_with("/link") || r.ends_with("/allow-claim")) => Some(LIMIT_SMALL),
        ("POST", "audit-event") => Some(LIMIT_SMALL),
        ("DELETE", r) if r.starts_with("status/") => Some(LIMIT_SMALL),
        ("PUT", "tags") => Some(LIMIT_TAGS),
        _ => None,
    }
}

/// The member signature of a `/api/team/` request (not `config`), checked before `route` takes
/// the state lock: the lock only to read the failure limit and take the members snapshot, then
/// the signature on that snapshot (`kid`: that key only; without: every key, in one of the
/// `NO_KID` slots, `429 busy` when none came free within `NO_KID_WAIT`). `None`: not a member route, or a body
/// over the route's limit (answered by `team_route` before any signature is looked at).
pub(crate) fn member_verify(app: &Mutex<App>, rq: &Request, body: &Body) -> Option<Verified> {
    let url = rq.url();
    let method = rq.method().as_str();
    let rest = url.split('?').next().unwrap_or("").strip_prefix("/api/team/")?;
    let body = body.bytes(route_limit(method, rest)?).ok()?;
    let (t, ip) = (now(), crate::rate_ip(rq));
    let snap = {
        let mut a = lock(app);
        if a.team.fail_hits.get(&ip).is_some_and(|h| t < h.0 + 60 && h.1 >= FAIL_PER_MIN) {
            // Once per source and minute: the journal and the team audit log name the source and
            // the `kid` it tried (no actor: no signature verified).
            if a.team.limit_noted.get(&ip).is_none_or(|at| t >= at + 60) {
                a.team.limit_noted.insert(ip.clone(), t);
                if let Some(more) = a.team.limit_record(t) {
                    let kid = header(rq, reqauth::HEADER).and_then(|s| reqauth::header_kid(&s).and_then(kid_form).map(str::to_owned));
                    eprintln!("team: failed member signatures from {ip} reached {FAIL_PER_MIN} per minute (kid {}, unverified)", kid.as_deref().unwrap_or("none"));
                    let mut detail = serde_json::json!({"kid": kid, "unverified": true});
                    if more > 0 {
                        detail["more"] = more.into();
                    }
                    a.team_audit.submit(Rec::new("signature-limit", None, Some(crate::client_ip(rq)), Target::default(), detail).answered(429));
                }
            }
            return Some(Verified { r: Err(Refused::Limited), t });
        }
        a.team.members.clone()
    };
    let r = verify_on(&snap, rq, method, url, body, t).map(|(i, nonce)| (snap, i, nonce));
    Some(Verified { r, t })
}

/// The index of the key in `m` that signed the request, and its nonce (not yet checked for replay).
fn verify_on(m: &Members, rq: &Request, method: &str, url: &str, body: &[u8], t: u64) -> Result<(usize, String), Refused> {
    let auth = |e: &str| Refused::Auth(e.to_owned());
    if m.keys.is_empty() {
        return Err(auth("team access disabled"));
    }
    let sig = header(rq, reqauth::HEADER).ok_or_else(|| auth("missing signature"))?;
    // `kid=`: that key only (unknown: refused, no other key tried); without: every key.
    let (range, _slot) = match reqauth::header_kid(&sig) {
        Some(k) => {
            let i = *m.kids.get(k).ok_or_else(|| auth("not a team member"))?;
            (i..i + 1, None)
        }
        None => (0..m.keys.len(), Some(NO_KID.take(&crate::rate_ip(rq), NO_KID_WAIT).ok_or(Refused::Busy)?)),
    };
    // Once, not per key.
    let digest = reqauth::body_digest(body);
    let mut err = String::new();
    for i in range {
        let mut nonce = String::new();
        let seen = |n: &str| {
            nonce = n.to_owned();
            true
        };
        match reqauth::verify_request_for(&sig, &m.keys[i], crate::sig_relay(), method, url, &digest, t, seen) {
            Ok(()) => return Ok((i, nonce)),
            Err(e) if err.is_empty() || e != "signature does not match" => err = e,
            Err(_) => {}
        }
    }
    Err(auth(if err == "signature does not match" { "not a team member" } else { &err }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_limit_records_are_capped_per_minute_and_carry_the_rest() {
        let mut t = Team::default();
        assert!((0..60).all(|_| t.limit_record(1000) == Some(0)));
        assert_eq!((t.limit_record(1001), t.limit_record(1059)), (None, None));
        assert_eq!(t.limit_record(1060), Some(2), "the next minute's first record carries the two left out");
        assert_eq!(t.limit_record(1061), Some(0));
        assert_eq!((kid_form("0123456789abcdef"), kid_form("0123456789ABCDEF"), kid_form("x;rm -rf"), kid_form("0123456789abcdef0")), (Some("0123456789abcdef"), None, None, None));
    }

    #[test]
    fn no_kid_slots_are_two_and_one_per_source_and_waits_are_bounded() {
        static P: Slots = Slots::new(2);
        let wait = Duration::from_millis(300);
        let (a, b) = (P.take("a", wait).unwrap(), P.take("b", Duration::ZERO).unwrap());
        // Both held: a third source gets none, but only after the bound.
        let t = Instant::now();
        assert!(P.take("c", wait).is_none());
        assert!(t.elapsed() >= wait, "{:?}", t.elapsed());
        drop(b);
        // A free slot, but not for the source that holds the other one: it waits for its own.
        let t = Instant::now();
        assert!(P.take("a", wait).is_none());
        assert!(t.elapsed() >= wait);
        std::thread::scope(|sc| {
            let w = sc.spawn(|| {
                let t = Instant::now();
                let s = P.take("a", Duration::from_secs(5));
                (s.is_some(), t.elapsed())
            });
            std::thread::sleep(Duration::from_millis(100));
            drop(a);
            let (got, took) = w.join().unwrap();
            // Woken when the slot is freed, not at the end of the wait.
            assert!(got && took < Duration::from_secs(2), "{got} {took:?}");
        });
        assert_eq!(P.held.lock().unwrap().0.len(), 0);
        let _c = P.take("c", Duration::ZERO).unwrap();
        assert!(P.take("d", Duration::ZERO).is_some());
    }

    #[test]
    fn no_kid_waiters_are_as_many_as_slots_and_one_per_source() {
        static P: Slots = Slots::new(2);
        let (a, b) = (P.take("a", Duration::ZERO).unwrap(), P.take("b", Duration::ZERO).unwrap());
        std::thread::scope(|sc| {
            let w: Vec<_> = ["c", "d"].map(|s| sc.spawn(move || P.take(s, Duration::from_secs(5)).is_some())).into();
            while P.held.lock().unwrap().1.len() < 2 {
                std::thread::sleep(Duration::from_millis(5));
            }
            // Two waiting: a third source, and a second waiter of a waiting source, are refused at once.
            let t = Instant::now();
            assert!(P.take("e", Duration::from_secs(5)).is_none());
            assert!(P.take("c", Duration::from_secs(5)).is_none());
            assert!(t.elapsed() < Duration::from_millis(100), "{:?}", t.elapsed());
            drop((a, b));
            assert!(w.into_iter().all(|h| h.join().unwrap()));
        });
        assert_eq!(P.held.lock().unwrap().1.len(), 0);
    }
}

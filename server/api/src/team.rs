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
//!
//! Staff sign-in (design 12.3 (b)): with an `oidc` part in the document, any key may present an
//! access token of that provider in `POST /api/team/session`, signed with that key (the token is
//! the authorisation, the signature the proof of possession). The relay checks the token
//! (oidc.rs, before the state lock), maps its roles through the document's `roles` table and
//! binds the person to the key until min(token `exp`, `TEAM_SESSION_MAX_MINUTES`), in
//! `team/sessions.json`. A key with a live session is that person on every member route (also a
//! key that is on the roster: the session wins while it lasts); its permissions are recomputed
//! from the document in force on every request. `oidc_only`: roster keys only for `service` entries.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tether_trust::team::{self, Grants, ManageRole, Oidc, OpenError, Permissions, Roster, Upload};
use tether_trust::{reqauth, VerifyingKey};
use tiny_http::Request;

use crate::oidc::{self, Claims, Reason};
use crate::teamaudit::{self, Actor, Rec, Target};
use crate::{audit, empty, hdr, header, json, load_json, lock, mkdirs, now, rate_ok, text, App, Body, Resp, LIMIT_SMALL};

/// `POST /api/team/session`: the token (at most `oidc::TOKEN_MAX`) and the key.
const LIMIT_SESSION: u64 = 32 << 10;
/// Sessions kept (live, and expired ones for a day to answer "session expired"); a person's keys
/// at most `SESSIONS_PER_PERSON` (a new one replaces the oldest); tokens remembered until they
/// expire (one token binds one key) at most `TOKENS_MAX`. Full: `503 too-many-sessions`.
const SESSIONS_MAX: usize = 5000;
const SESSIONS_PER_PERSON: usize = 8;
const TOKENS_MAX: usize = 20_000;
const EXPIRED_KEEP: u64 = 86_400;
/// `team/sessions.json` larger than this is not read (it is written by this relay alone).
const SESSIONS_FILE_MAX: u64 = 16 << 20;
/// Whose session expired gets this (the desktop signs in again).
pub const SESSION_EXPIRED: &str = "session expired: sign in again";
/// `TEAM_SESSION_MAX_MINUTES` (default 60): the longest session, whatever the token's lifetime.
pub static SESSION_MAX_MINUTES: OnceLock<u64> = OnceLock::new();

fn session_max_secs() -> u64 {
    SESSION_MAX_MINUTES.get().copied().unwrap_or(60) * 60
}

/// What the document in force says about sign-in.
pub(crate) struct SignIn {
    oidc: Oidc,
    roles: BTreeMap<String, Grants>,
    deny: HashSet<String>,
}

/// A person signed in with a key (`team/sessions.json`).
#[derive(Serialize, Deserialize, Clone)]
struct Session {
    kid: String,
    /// Standard base64 of the key (the rate limit's key, as for roster members).
    key: String,
    oid: String,
    name: String,
    upn: String,
    /// The token's roles that the table had when it signed in (permissions come from the
    /// document in force, request by request).
    roles: Vec<String>,
    uti: String,
    exp: u64,
    since: u64,
}

impl Session {
    fn actor(&self) -> Actor {
        Actor { key_fingerprint: self.kid.clone(), name: self.name.clone(), oid: Some(self.oid.clone()), upn: Some(self.upn.clone()).filter(|u| !u.is_empty()) }
    }
}

/// A token bound to a key until it expires.
#[derive(Serialize, Deserialize, Clone)]
struct Bound {
    uti: String,
    kid: String,
    exp: u64,
}

/// What [`persist_sessions`] writes (read back entry by entry, [`Team::load_sessions`]).
#[derive(Serialize)]
struct SessionFile {
    /// The provider they were made with: another one in the document ends them all.
    oidc: Option<Oidc>,
    sessions: Vec<Session>,
    tokens: Vec<Bound>,
}

pub const LIMIT_ROSTER: u64 = 64 << 10;
/// `PUT /api/admin/team`: a signed upload; a plain roster stays at `LIMIT_ROSTER`.
pub const LIMIT_TEAM: u64 = team::MAX_UPLOAD as u64;
const LIMIT_TAGS: u64 = 512 << 10;
const KEY_PER_MIN: u32 = 120;
/// Failed member signatures per IP and minute: a failure without `kid` tries every member key.
const FAIL_PER_MIN: u32 = 60;
/// Sign-ins per member key and minute refused for their token: one key cannot be used to probe tokens,
/// and an office behind one address with skewed clocks does not lock itself out of every team route.
const TOKEN_FAIL_PER_MIN: u32 = 20;
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

/// What [`member_verify`] found, before the state lock: who signed and the nonce (still to be
/// checked for replay), or why not.
pub(crate) struct Verified {
    r: Result<Signer, Refused>,
    t: u64,
}

enum Signer {
    /// A roster or document key: the snapshot it was checked on and its index there.
    Key(Arc<Members>, usize, String),
    /// A key with a session (live or not when it was looked up): its kid and key.
    Session(String, VerifyingKey, String),
    /// `POST /api/team/session`, signed by the key in its body.
    SignIn(Box<SignInCheck>),
}

/// A sign-in as checked before the lock: the key, and the token against the provider of `cfg`
/// (the document in force then; `None`: none had `oidc`).
struct SignInCheck {
    kid: String,
    key: VerifyingKey,
    nonce: String,
    cfg: Option<Oidc>,
    token: Result<Claims, Reason>,
}

enum Refused {
    /// Too many failed signatures from this IP.
    Limited,
    /// Another signature without `kid` is being checked.
    Busy,
    /// Not a member's valid signature (`401` with this text, counts as a failure).
    Auth(String),
    /// A sign-in body that is not `{"access_token","key"}` (`400`).
    Bad(String),
}

/// Who a member request is from, as the handlers see it.
struct Caller {
    name: String,
    /// Standard base64 of the key (rate limits).
    key: String,
    perms: Permissions,
    service: bool,
    actor: Actor,
    /// Signed in: the session (its kid); `None`: a roster or document key.
    session: Option<String>,
}

/// A person signed in at this relay calling an admin route (`App::manage_session`).
pub(crate) struct ManageCaller {
    pub actor: Actor,
    /// Their Tether Manage roles now (sorted); may be empty.
    pub roles: Vec<ManageRole>,
    pub oid: String,
}

enum Authed {
    Member(Caller),
    SignIn(Box<SignInCheck>),
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
    /// Sign-ins refused for their token (`401`), per member key: not the source address's failures.
    token_fail_hits: HashMap<String, (u64, u32)>,
    /// `POST /api/team/audit-event` per member key.
    posted_hits: HashMap<String, (u64, u32)>,
    /// Summary and device-list reads, one record per actor and hour (also the admin's, main.rs).
    pub(crate) reads: teamaudit::Folded,
    /// Sources refused by `FAIL_PER_MIN`: when their last `signature-limit` record was written.
    limit_noted: HashMap<String, u64>,
    /// `signature-limit` records relay-wide: (minute start, written in it, not written since the last one).
    limit_records: (u64, u32, u64),
    /// `sign-in-refused` records without a verified person, relay-wide, the same way.
    refused_records: (u64, u32, u64),
    /// Sign-in (the document's `oidc`, roles, deny list); `None`: no sign-in on this relay.
    signin: Option<Arc<SignIn>>,
    /// Sessions by key id; tokens bound to a key by `uti`.
    sessions: HashMap<String, Session>,
    tokens: HashMap<String, Bound>,
    /// `team/sessions.json` is to be written before the answer ([`persist_sessions`]).
    pub(crate) sessions_dirty: bool,
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
                Ok((d, keys)) => {
                    t.set_doc(&d, keys, &raw);
                    t.load_sessions(data);
                }
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

    /// With `oidc_only`, only service entries keep their keys. Sessions the new document no
    /// longer allows end here (returned with why, for the audit log): every one when the provider
    /// changed or sign-in is gone (`document-changed`), a denied person's (`deny-list`), one left
    /// without a mapped role (`no-role`).
    fn set_doc(&mut self, d: &team::TeamDoc, keys: Vec<VerifyingKey>, raw: &[u8]) -> Vec<(Session, &'static str)> {
        let old = self.signin.as_ref().map(|s| s.oidc.clone());
        self.signin = d.oidc.clone().map(|oidc| Arc::new(SignIn { oidc, roles: d.roles.clone(), deny: d.deny.iter().cloned().collect() }));
        let changed = old.is_some() && old != d.oidc;
        let ended = self.end_sessions(changed);
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
        ended
    }

    /// Ends the sessions the document in force does not allow (all with `all`); the live ones
    /// ended are returned with why, expired ones go silently.
    fn end_sessions(&mut self, all: bool) -> Vec<(Session, &'static str)> {
        let (t, si, mut ended) = (now(), self.signin.clone(), Vec::new());
        self.sessions.retain(|_, s| {
            let why = match &si {
                _ if all => Some("document-changed"),
                None => Some("document-changed"),
                Some(si) if si.deny.contains(&s.oid) => Some("deny-list"),
                Some(si) if !oidc::has_role(&s.roles, &si.roles) => Some("no-role"),
                _ => None,
            };
            if let Some(w) = why.filter(|_| s.exp > t) {
                ended.push((s.clone(), w));
            }
            why.is_none()
        });
        self.sessions_dirty |= !ended.is_empty();
        ended
    }

    /// `team/sessions.json` as the last run left it: expired tokens and long-expired sessions
    /// dropped, and what the document in force does not allow. Read entry by entry with unknown
    /// fields ignored, so an entry or field a newer relay wrote costs that entry only and the token
    /// bindings (one token, one key) survive a downgrade, also those of another provider. A file
    /// that is no JSON at all is kept aside as `sessions.json.bad` (its bindings are lost).
    fn load_sessions(&mut self, data: &Path) {
        let p = data.join("team/sessions.json");
        if fs::metadata(&p).is_ok_and(|m| m.len() > SESSIONS_FILE_MAX) {
            eprintln!("error: team/sessions.json is too large; no session is kept");
            return;
        }
        let Ok(raw) = fs::read(&p) else { return };
        let Ok(v) = serde_json::from_slice::<Value>(&raw) else {
            eprintln!("error: team/sessions.json is not JSON; kept as team/sessions.json.bad, no session and no token binding is kept");
            let _ = fs::rename(&p, data.join("team/sessions.json.bad"));
            return;
        };
        let t = now();
        let list = |k: &str| v.get(k).and_then(Value::as_array).cloned().unwrap_or_default();
        for b in list("tokens").into_iter().filter_map(|b| serde_json::from_value::<Bound>(b).ok()).filter(|b| b.exp + oidc::SKEW > t).take(TOKENS_MAX) {
            self.tokens.insert(b.uti.clone(), b);
        }
        let made_with = serde_json::from_value::<Option<Oidc>>(v.get("oidc").cloned().unwrap_or(Value::Null));
        if made_with.as_ref().map_or(true, |o| o.is_some() && *o != self.signin.as_ref().map(|s| s.oidc.clone())) {
            eprintln!("team: the sessions in team/sessions.json were made with another provider; none is kept");
            return;
        }
        for s in list("sessions").into_iter().filter_map(|s| serde_json::from_value::<Session>(s).ok()).filter(|s| s.exp + EXPIRED_KEEP > t && tether_trust::key_from_b64(&s.key).is_ok_and(|k| team::key_id(&k) == s.kid)).take(SESSIONS_MAX) {
            self.sessions.insert(s.kid.clone(), s);
        }
        for (s, why) in self.end_sessions(false) {
            eprintln!("team: session of {} ({}) ended at start: {why}", s.oid, s.kid);
        }
        self.sessions_dirty = false;
    }

    /// A signed team document is in force (one that could not be read has no members).
    pub(crate) fn signed(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.serial > 0)
    }

    pub fn maintain(&mut self, t: u64) {
        self.key_hits.retain(|_, (start, _)| t < *start + 60);
        self.fail_hits.retain(|_, (start, _)| t < *start + 60);
        self.token_fail_hits.retain(|_, (start, _)| t < *start + 60);
        self.posted_hits.retain(|_, (start, _)| t < *start + 60);
        self.limit_noted.retain(|_, at| t < *at + 60);
        // In memory only: a restart drops them from the file the same way.
        self.sessions.retain(|_, s| s.exp + EXPIRED_KEEP > t);
        self.tokens.retain(|_, b| b.exp + oidc::SKEW > t);
    }

}

/// Whether a capped record (`signature-limit`, unverified `sign-in-refused`) may be written at `t`
/// (relay-wide, `LIMIT_RECORDS_PER_MIN`), and how many were left out since the last one written.
fn capped((start, n, more): &mut (u64, u32, u64), t: u64) -> Option<u64> {
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
        // A provider this build may not trust (production: https and a public address only).
        if let Some(o) = &d.oidc {
            oidc::issuer_ok(&o.issuer).map_err(|e| text(400, &e))?;
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
        let ended = self.team.set_doc(&d, keys, body);
        eprintln!("team: signed team document {} accepted ({} members, {} with a key here)", d.serial, d.members.len(), self.team.members.who.len());
        for (s, why) in ended {
            eprintln!("team: session of {} ({}) ended: {why}", s.oid, s.kid);
            self.team_audit.submit(Rec::new("session-ended", Some(s.actor()), None, Target::default(), serde_json::json!({"reason": why, "serial": d.serial})));
        }
        Ok(empty(204))
    }

    /// The plain roster, or the signed upload exactly as published.
    pub(crate) fn get_team(&self) -> Result<Resp, Resp> {
        let raw = self.team_served().ok_or_else(|| crate::internal("team/team.json unreadable"))?;
        Ok(tiny_http::Response::from_data(raw).with_header(hdr("Content-Type", "application/json")).boxed())
    }

    /// The bytes `GET /api/admin/team` answers: the plain roster, or the signed upload as received
    /// (proposals.rs hashes them: what a team proposal was based on).
    pub(crate) fn team_served(&self) -> Option<Vec<u8>> {
        match self.team.doc {
            None => serde_json::to_vec(&self.team.roster).ok(),
            Some(_) => fs::read(self.data.join("team/team.json")).ok(),
        }
    }

    /// Who signed an admin request with a key that has a sign-in session here (`kid` names it);
    /// `None` when the header names no such key: the admin signature is checked as before. The
    /// signature (one key, under the state lock like an admin signature), replay, the session's
    /// life, and its Tether Manage roles through the document in force (none: every route `403`,
    /// main.rs `session_may`). Failed signatures count against the source as on member routes.
    pub(crate) fn manage_session(&mut self, rq: &Request, method: &str, url: &str, body: &[u8]) -> Option<Result<ManageCaller, Resp>> {
        let sig = header(rq, reqauth::HEADER)?;
        let s = self.team.sessions.get(reqauth::header_kid(&sig)?)?.clone();
        let (t, ip) = (now(), crate::rate_ip(rq));
        let fail = |a: &mut App, e: &str| {
            rate_ok(&mut a.team.fail_hits, ip.clone(), t, u32::MAX);
            text(401, e)
        };
        Some((|| {
            if self.team.fail_hits.get(&ip).is_some_and(|h| t < h.0 + 60 && h.1 >= FAIL_PER_MIN) {
                return Err(text(429, "too many failed requests"));
            }
            let key = tether_trust::key_from_b64(&s.key).map_err(crate::internal)?;
            let nonce = match verify_key(&key, rq, method, url, body, t) {
                Ok(n) => n,
                Err(Refused::Auth(e)) => return Err(fail(self, &e)),
                Err(_) => return Err(fail(self, "not a team member")),
            };
            if self.nonces.contains_key(&nonce) {
                return Err(fail(self, "replayed request"));
            }
            self.remember_nonce(nonce, t)?;
            if s.exp <= t {
                return Err(text(401, SESSION_EXPIRED));
            }
            if !rate_ok(&mut self.team.key_hits, s.key.clone(), t, KEY_PER_MIN) {
                return Err(text(429, "too many requests"));
            }
            let roles = self.team.signin.as_ref().map(|si| oidc::manage_roles(&s.roles, &si.roles)).unwrap_or_default();
            Ok(ManageCaller { actor: s.actor(), roles, oid: s.oid.clone() })
        })())
    }

    /// The session a release upload's `kid` names: its key (the body is verified with it before
    /// the lock, main.rs `read_request_body`) and who it is, so the refusal is recorded with the
    /// person. `None`: no such session; the admin keys verify the upload.
    pub(crate) fn release_session(&self, rq: &Request) -> Option<(VerifyingKey, ManageCaller)> {
        let sig = header(rq, reqauth::HEADER)?;
        let s = self.team.sessions.get(reqauth::header_kid(&sig)?)?;
        let key = tether_trust::key_from_b64(&s.key).ok()?;
        let roles = self.team.signin.as_ref().map(|si| oidc::manage_roles(&s.roles, &si.roles)).unwrap_or_default();
        Some((key, ManageCaller { actor: s.actor(), roles, oid: s.oid.clone() }))
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
        let caller = match self.member_auth(rq, method, rest)? {
            Authed::Member(c) => c,
            Authed::SignIn(s) => return self.sign_in(rq, *s),
        };
        let t = now();
        if !rate_ok(&mut self.team.key_hits, caller.key.clone(), t, KEY_PER_MIN) {
            return Err(text(429, "too many requests"));
        }
        let Caller { name, perms, service, key, actor, session } = caller;
        let m = &perms;
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
            // Sign-out: the key's session ends (a roster key has none: nothing to end).
            ("DELETE", "session") => {
                if let Some(s) = session.and_then(|k| self.team.sessions.remove(&k)) {
                    self.team.sessions_dirty = true;
                    self.audit_pending = Some(Rec::new("sign-out", Some(s.actor()), Some(ip), Target::default(), Value::Null));
                }
                Ok(empty(204))
            }
            ("GET", "me") if session.is_some() => {
                let s = session.and_then(|k| self.team.sessions.get(&k)).ok_or_else(|| text(401, SESSION_EXPIRED))?;
                let si = self.team.signin.as_ref().ok_or_else(|| text(401, SESSION_EXPIRED))?;
                Ok(json(&session_answer(s, si)))
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

    /// Who signed the request, from this request's [`member_verify`]: here, under the lock, the
    /// replay check and whether the key is still a member now (a document may have replaced the
    /// snapshot it was checked on, a session may have ended or expired). A key with a live session
    /// is that person; without one, its roster entry if it has one (with `oidc_only` only service
    /// entries are there); a session that ended long ago or never existed: `not a team member`,
    /// one that expired: [`SESSION_EXPIRED`] (not counted as a failure). `sign_out`: an expired
    /// session may still be ended.
    fn member_auth(&mut self, rq: &Request, method: &str, rest: &str) -> Result<Authed, Resp> {
        let sign_out = (method, rest) == ("DELETE", "session");
        let ip = crate::rate_ip(rq);
        let Verified { r, t } = self.team.verified.take().ok_or_else(|| crate::internal("member signature not checked"))?;
        let fail = |a: &mut App, e: &str| {
            rate_ok(&mut a.team.fail_hits, ip.clone(), t, u32::MAX);
            text(401, e)
        };
        let signer = match r {
            Err(Refused::Limited) => return Err(text(429, "too many failed requests")),
            // Not a failed signature (none was checked): several old desktops behind one address
            // must not lock it out. The waiter bound of `NO_KID` limits what they hold.
            Err(Refused::Busy) => return Err(text(429, "busy").with_header(hdr("Retry-After", "1"))),
            Err(Refused::Bad(e)) => return Err(text(400, &e)),
            Err(Refused::Auth(e)) => return Err(fail(self, &e)),
            Ok(s) => s,
        };
        // A sign-in's nonce is kept only once its token verified: anyone may sign one with a fresh
        // key, and a refused token changes nothing (its replay is refused the same way again).
        let (nonce, keep) = match &signer {
            Signer::Key(_, _, n) | Signer::Session(_, _, n) => (n.clone(), true),
            Signer::SignIn(s) => (s.nonce.clone(), s.token.is_ok()),
        };
        if self.nonces.contains_key(&nonce) {
            return Err(fail(self, "replayed request"));
        }
        if keep {
            self.remember_nonce(nonce, t)?;
        }
        let roster = |a: &App, key: &VerifyingKey| {
            let i = a.team.members.keys.iter().position(|k| k == key)?;
            let w = &a.team.members.who[i];
            Some(Caller { name: w.name.clone(), key: w.key.clone(), perms: w.perms.clone(), service: w.service, actor: Actor { key_fingerprint: team::key_id(key), name: w.name.clone(), oid: None, upn: None }, session: None })
        };
        match signer {
            Signer::SignIn(s) => Ok(Authed::SignIn(s)),
            Signer::Key(snap, i, _) => {
                // Checked on the snapshot; the same key in the members now (a document may have come).
                let key = snap.keys[i];
                let i = if Arc::ptr_eq(&snap, &self.team.members) { Some(i) } else { self.team.members.keys.iter().position(|k| *k == key) };
                match i {
                    Some(_) => roster(self, &key).map(Authed::Member).ok_or_else(|| fail(self, "not a team member")),
                    None => Err(fail(self, "not a team member")),
                }
            }
            Signer::Session(kid, key, _) => {
                let s = self.team.sessions.get(&kid).filter(|s| tether_trust::key_from_b64(&s.key).is_ok_and(|k| k == key));
                let live = s.filter(|s| s.exp > t).and_then(|s| self.team.signin.as_ref().map(|si| (oidc::permissions(&s.roles, &si.roles), !oidc::manage_roles(&s.roles, &si.roles).is_empty())));
                let none = |s: &Session| Authed::Member(Caller { name: s.name.clone(), key: s.key.clone(), perms: no_permissions(), service: false, actor: s.actor(), session: Some(kid.clone()) });
                match (s, live) {
                    (Some(s), Some((Some(perms), _))) => Ok(Authed::Member(Caller { name: s.name.clone(), key: s.key.clone(), perms, service: false, actor: s.actor(), session: Some(kid) })),
                    (Some(s), _) if sign_out => Ok(none(s)),
                    // A Tether Manage sign-in: its own answer and sign-out, no team route (the relay's
                    // admin routes take it, `manage_session`).
                    (Some(s), Some((None, true))) if (method, rest) == ("GET", "me") => Ok(none(s)),
                    (Some(_), Some((None, true))) => Err(text(403, "this sign-in has only Tether Manage roles")),
                    (s, _) => match roster(self, &key) {
                        Some(c) => Ok(Authed::Member(c)),
                        None if s.is_some() => Err(text(401, SESSION_EXPIRED)),
                        None => Err(fail(self, "not a team member")),
                    },
                }
            }
        }
    }

    /// `POST /api/team/session` after its signature: the token's check (made before the lock)
    /// against the document in force now, the deny list, the role table, the token's key binding
    /// and the bounds; then the session. Every outcome is one team audit record.
    fn sign_in(&mut self, rq: &Request, s: SignInCheck) -> Result<Resp, Resp> {
        let (t, ip, key) = (now(), crate::client_ip(rq), tether_trust::key_to_b64(&s.key));
        if !rate_ok(&mut self.team.key_hits, key.clone(), t, KEY_PER_MIN) {
            return Err(text(429, "too many requests"));
        }
        if self.team.token_fail_hits.get(&key).is_some_and(|h| t < h.0 + 60 && h.1 >= TOKEN_FAIL_PER_MIN) {
            return Err(text(429, "too many refused tokens from this key").with_header(hdr("Retry-After", "60")));
        }
        let unverified = Actor { key_fingerprint: s.kid.clone(), name: "-".into(), oid: None, upn: None };
        match self.session_from(&s, key, t) {
            Ok(sess) => {
                let si = self.team.signin.clone().ok_or_else(|| crate::internal("session without sign-in"))?;
                let answer = session_answer(&sess, &si);
                self.audit_pending = Some(Rec::new("sign-in", Some(sess.actor()), Some(ip), Target::default(), serde_json::json!({"roles": sess.roles, "exp": sess.exp})));
                eprintln!("team: {} ({}) signed in with key {}", sess.oid, sess.upn, sess.kid);
                self.team.sessions.insert(sess.kid.clone(), sess);
                self.team.sessions_dirty = true;
                Ok(json(&answer))
            }
            Err((reason, actor)) => {
                // Before the token's own signature verified (any key may send junk): against the
                // source address, whose limit refuses every team route. After: a token from the
                // provider, refused for its claims; per key, not per address (staff behind one NAT).
                if matches!(reason, Reason::Malformed | Reason::Alg | Reason::NoKid | Reason::UnknownKid | Reason::BadSignature | Reason::NotConfigured) {
                    rate_ok(&mut self.team.fail_hits, crate::rate_ip(rq), t, u32::MAX);
                } else if reason.status() == 401 {
                    rate_ok(&mut self.team.token_fail_hits, tether_trust::key_to_b64(&s.key), t, u32::MAX);
                }
                let mut detail = serde_json::json!({"reason": reason.code()});
                let actor = match actor {
                    Some(a) => Some(a),
                    // No verified person: capped relay-wide like `signature-limit` records.
                    None => match capped(&mut self.team.refused_records, t) {
                        Some(more) => {
                            if more > 0 {
                                detail["more"] = more.into();
                            }
                            Some(unverified)
                        }
                        None => None,
                    },
                };
                if let Some(a) = actor {
                    self.audit_pending = Some(Rec::new("sign-in-refused", Some(a), Some(ip), Target::default(), detail));
                }
                Err(refusal(reason))
            }
        }
    }

    /// The session a checked sign-in gives, or why not (with the person when the token was
    /// verified). Makes room: expired sessions and tokens go first, then a person's oldest key.
    fn session_from(&mut self, s: &SignInCheck, key: String, t: u64) -> Result<Session, (Reason, Option<Actor>)> {
        let si = self.team.signin.clone().ok_or((Reason::NotConfigured, None))?;
        let c = s.token.clone().map_err(|r| (r, None))?;
        if s.cfg.as_ref() != Some(&si.oidc) {
            return Err((Reason::ConfigChanged, None));
        }
        let person = Actor { key_fingerprint: s.kid.clone(), name: c.name.clone(), oid: Some(c.oid.clone()), upn: Some(c.upn.clone()).filter(|u| !u.is_empty()) };
        let no = |r| Err((r, Some(person.clone())));
        if si.deny.contains(&c.oid) {
            return no(Reason::Denied);
        }
        let roles: Vec<String> = c.roles.iter().filter(|r| si.roles.contains_key(*r)).cloned().collect();
        if !oidc::has_role(&roles, &si.roles) {
            return no(Reason::NoRole);
        }
        if self.team.tokens.get(&c.uti).is_some_and(|b| b.kid != s.kid) {
            return no(Reason::TokenUsed);
        }
        let exp = c.exp.min(t + session_max_secs());
        // Within the clock skew after its `exp`: the token passed, but a session would be over.
        if exp <= t {
            return no(Reason::Expired);
        }
        let (sessions, tokens) = (&mut self.team.sessions, &mut self.team.tokens);
        if !sessions.contains_key(&s.kid) && sessions.len() >= SESSIONS_MAX {
            sessions.retain(|_, x| x.exp > t);
        }
        if !tokens.contains_key(&c.uti) && tokens.len() >= TOKENS_MAX {
            tokens.retain(|_, b| b.exp + oidc::SKEW > t);
        }
        if (!sessions.contains_key(&s.kid) && sessions.len() >= SESSIONS_MAX) || (!tokens.contains_key(&c.uti) && tokens.len() >= TOKENS_MAX) {
            return no(Reason::TooManySessions);
        }
        let mut mine: Vec<(u64, String)> = sessions.values().filter(|x| x.oid == c.oid && x.kid != s.kid && x.exp > t).map(|x| (x.since, x.kid.clone())).collect();
        if mine.len() >= SESSIONS_PER_PERSON {
            mine.sort_unstable();
            for (_, k) in &mine[..mine.len() + 1 - SESSIONS_PER_PERSON] {
                if let Some(old) = sessions.remove(k) {
                    eprintln!("team: session of {} ({}) replaced by a newer key", old.oid, old.kid);
                    self.team_audit.submit(Rec::new("session-ended", Some(old.actor()), None, Target::default(), serde_json::json!({"reason": "replaced", "kid": old.kid})));
                }
            }
        }
        tokens.insert(c.uti.clone(), Bound { uti: c.uti.clone(), kid: s.kid.clone(), exp: c.exp });
        Ok(Session { kid: s.kid.clone(), key, oid: c.oid, name: c.name, upn: c.upn, roles, uti: c.uti, exp, since: t })
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
        ("POST", "session") => Some(LIMIT_SESSION),
        ("DELETE", "session") => Some(LIMIT_SMALL),
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
    let (snap, session, cfg) = {
        let mut a = lock(app);
        if a.team.fail_hits.get(&ip).is_some_and(|h| t < h.0 + 60 && h.1 >= FAIL_PER_MIN) {
            // Once per source and minute: the journal and the team audit log name the source and
            // the `kid` it tried (no actor: no signature verified).
            if a.team.limit_noted.get(&ip).is_none_or(|at| t >= at + 60) {
                a.team.limit_noted.insert(ip.clone(), t);
                if let Some(more) = capped(&mut a.team.limit_records, t) {
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
        // A key with a session (live or expired: the answer then says so) is checked with that key.
        let kid = header(rq, reqauth::HEADER).and_then(|s| reqauth::header_kid(&s).map(str::to_owned));
        let session = kid.and_then(|k| Some((tether_trust::key_from_b64(&a.team.sessions.get(&k)?.key).ok()?, k)));
        (a.team.members.clone(), session, a.team.signin.as_ref().map(|s| s.oidc.clone()))
    };
    let r = match ((method, rest), session) {
        (("POST", "session"), _) => sign_in_verify(rq, method, url, body, t, cfg).map(|s| Signer::SignIn(Box::new(s))),
        (_, Some((key, kid))) => verify_key(&key, rq, method, url, body, t).map(|n| Signer::Session(kid, key, n)),
        (_, None) => verify_on(&snap, rq, method, url, body, t).map(|(i, nonce)| Signer::Key(snap, i, nonce)),
    };
    Some(Verified { r, t })
}

/// The nonce of a request signed with `key` (not yet checked for replay).
fn verify_key(key: &VerifyingKey, rq: &Request, method: &str, url: &str, body: &[u8], t: u64) -> Result<String, Refused> {
    let sig = header(rq, reqauth::HEADER).ok_or_else(|| Refused::Auth("missing signature".into()))?;
    let mut nonce = String::new();
    let seen = |n: &str| {
        nonce = n.to_owned();
        true
    };
    match reqauth::verify_request_for(&sig, key, crate::sig_relay(), method, url, &reqauth::body_digest(body), t, seen) {
        Ok(()) => Ok(nonce),
        Err(e) => Err(Refused::Auth(if e == "signature does not match" { "not a team member".into() } else { e })),
    }
}

/// `POST /api/team/session` before the lock: a signature, a body `{"access_token","key"}` with
/// `key` the key that signed it and `kid` naming that key, then the token against `cfg` (the
/// provider of the document in force; this may fetch its keys, oidc.rs).
fn sign_in_verify(rq: &Request, method: &str, url: &str, body: &[u8], t: u64, cfg: Option<Oidc>) -> Result<SignInCheck, Refused> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct SignInBody {
        access_token: String,
        key: String,
    }
    let sig = header(rq, reqauth::HEADER).ok_or_else(|| Refused::Auth("missing signature".into()))?;
    let b: SignInBody = serde_json::from_slice(body).map_err(|e| Refused::Bad(format!("expected {{\"access_token\",\"key\"}}: {e}")))?;
    let key = tether_trust::key_from_b64(&b.key).map_err(|_| Refused::Bad("key: standard base64 of a 32-byte Ed25519 key".into()))?;
    let kid = team::key_id(&key);
    if reqauth::header_kid(&sig) != Some(kid.as_str()) {
        return Err(Refused::Auth("kid must name the key in the body".into()));
    }
    let nonce = verify_key(&key, rq, method, url, body, t)?;
    let token = match &cfg {
        Some(c) => oidc::verify(&b.access_token, c, t),
        None => Err(Reason::NotConfigured),
    };
    Ok(SignInCheck { kid, key, nonce, cfg, token })
}

/// No team permission (a sign-in with only Tether Manage roles; a sign-out of an ended session).
fn no_permissions() -> Permissions {
    Permissions { write: false, audit: false, sbom: false, agent: false, customers: Vec::new() }
}

/// A session as `POST /api/team/session` and `GET /api/team/me` answer it; `manage`: its Tether
/// Manage roles (none: the field is left out, as before).
fn session_answer(s: &Session, si: &SignIn) -> Value {
    let m = oidc::permissions(&s.roles, &si.roles).unwrap_or_else(no_permissions);
    let mut v = serde_json::json!({"name": s.name, "write": m.write, "audit": m.audit, "sbom": m.sbom, "agent": m.agent, "customers": m.customers, "service": false, "oid": s.oid, "upn": s.upn, "exp": s.exp, "roles": s.roles});
    let manage = oidc::manage_roles(&s.roles, &si.roles);
    if !manage.is_empty() {
        v["manage"] = serde_json::json!(manage);
    }
    v
}

/// A refused sign-in: `{"reason","message"}` with the reason's status; `503` with `Retry-After`.
fn refusal(r: Reason) -> Resp {
    let body = serde_json::to_vec(&serde_json::json!({"reason": r.code(), "message": r.message()})).unwrap_or_default();
    let mut resp = tiny_http::Response::from_data(body).with_status_code(r.status()).with_header(hdr("Content-Type", "application/json")).boxed();
    if r.status() == 503 {
        resp = resp.with_header(hdr("Retry-After", if r == Reason::ConfigChanged { "1" } else { "60" }));
    }
    resp
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

/// Serialises the writers of `team/sessions.json`; the snapshot is taken inside it, so an older one
/// never replaces a newer file.
static SESSION_FILE: Mutex<()> = Mutex::new(());

/// Writes `team/sessions.json` (0600, synced, renamed) outside the state lock, before the answer
/// that changed it goes out (a sign-out is durable when it is answered). A failed write is logged:
/// the sessions stay right in memory.
pub(crate) fn persist_sessions(app: &Mutex<App>) {
    let _file = SESSION_FILE.lock().unwrap_or_else(|e| e.into_inner());
    let (snap, data) = {
        let a = lock(app);
        let f = SessionFile { oidc: a.team.signin.as_ref().map(|s| s.oidc.clone()), sessions: a.team.sessions.values().cloned().collect(), tokens: a.team.tokens.values().cloned().collect() };
        (f, a.data.clone())
    };
    let tmp = crate::tmp_path(&data);
    let r = (|| {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let f = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
        let mut w = io::BufWriter::new(&f);
        serde_json::to_writer(&mut w, &snap).map_err(io::Error::other)?;
        w.flush()?;
        drop(w);
        f.sync_all()?;
        fs::rename(&tmp, data.join("team/sessions.json"))
    })();
    if let Err(e) = r {
        let _ = fs::remove_file(&tmp);
        eprintln!("error: team/sessions.json: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_limit_records_are_capped_per_minute_and_carry_the_rest() {
        let mut t = Team::default();
        let c = &mut t.limit_records;
        assert!((0..60).all(|_| capped(c, 1000) == Some(0)));
        assert_eq!((capped(c, 1001), capped(c, 1059)), (None, None));
        assert_eq!(capped(c, 1060), Some(2), "the next minute's first record carries the two left out");
        assert_eq!(capped(c, 1061), Some(0));
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

//! Team roster: the technicians (by their own Ed25519 public key) an admin lets read the org's
//! device list and shared tags on a relay (`/api/team/*`, docs/api.md). Members sign their requests
//! with `reqauth` exactly like admin requests.
//!
//! Two forms: the plain [`Roster`] (0.6, `{"members":[{name,key,write,audit}]}`, trusted because
//! the admin request carrying it is signed) and the admin-signed [`TeamDoc`] (an [`Upload`]: the
//! document in an `envelope`, and with an `oidc` part also a signed [`Config`] a relay serves to
//! anyone). A relay that has accepted a signed document refuses plain rosters from then on.

use std::collections::BTreeMap;

use crate::envelope::{self, Signed};
use crate::key_from_b64;
use crate::status::valid_customer;
use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Members of a signed team document.
pub const MAX_MEMBERS: usize = 2000;
/// Members of a plain roster (unchanged since 0.6).
pub const ROSTER_MAX_MEMBERS: usize = 500;
/// A signed team document's payload.
pub const MAX_DOC: usize = 256 << 10;
/// An [`Upload`] (the payload in base64 plus the config): what a relay reads for `PUT /api/admin/team`.
pub const MAX_UPLOAD: usize = 360 << 10;
/// Customer patterns per member or role.
pub const MAX_PATTERNS: usize = 64;
/// [`Permissions::scope_key`] of a member who sees everything, software lists included (every
/// member of a plain roster).
pub const DEFAULT_SCOPE: &str = "sbom:*";

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Roster {
    pub members: Vec<Member>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Member {
    pub name: String,
    /// Standard base64 of the 32-byte Ed25519 public key.
    pub key: String,
    /// May replace the shared tag document.
    pub write: bool,
    /// May read the session audit log (one device at a time).
    pub audit: bool,
}

impl Member {
    /// A plain roster entry has today's rights: software lists, the agent menu, every customer.
    pub fn permissions(&self) -> Permissions {
        Permissions { write: self.write, audit: self.audit, sbom: true, agent: true, customers: vec!["*".into()] }
    }
}

impl Roster {
    /// Checks the roster and returns the members' keys in order.
    pub fn validate(&self) -> Result<Vec<VerifyingKey>, String> {
        if self.members.len() > ROSTER_MAX_MEMBERS {
            return Err(format!("at most {ROSTER_MAX_MEMBERS} members"));
        }
        member_keys(self.members.iter().map(|m| (m.name.as_str(), m.key.as_str())))
    }
}

/// Names checked, keys parsed (each at most once), in order.
fn member_keys<'a>(members: impl Iterator<Item = (&'a str, &'a str)>) -> Result<Vec<VerifyingKey>, String> {
    let mut keys: Vec<VerifyingKey> = Vec::new();
    for (name, key) in members {
        let n = name.chars().count();
        if name.trim() != name || !(1..=64).contains(&n) || name.chars().any(char::is_control) {
            return Err(format!("member name {name:?}: 1-64 characters, no control characters or surrounding spaces"));
        }
        let k = key_from_b64(key).map_err(|e| format!("member {name}: {e}"))?;
        // shortcut: O(n²) over at most 2000 keys (32-byte compares, a few ms), only when a document is accepted.
        if keys.contains(&k) {
            return Err(format!("member {name}: key is listed twice"));
        }
        keys.push(k);
    }
    Ok(keys)
}

/// What a member may do on a relay. `customers`: patterns ([`valid_pattern`]) of the customer
/// references whose devices it sees; a device without a reference only with `*`.
#[derive(Clone, Debug, PartialEq)]
pub struct Permissions {
    pub write: bool,
    pub audit: bool,
    pub sbom: bool,
    pub agent: bool,
    pub customers: Vec<String>,
}

/// What a member request needs besides seeing the device's customer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Route {
    /// Summary, report, history (any member).
    Read,
    /// Link, unlink, allow a claim, remove a device, shared tags.
    Write,
    /// Session audit records.
    Audit,
    /// Software lists (and later the software search).
    Sbom,
    /// The agent menu in a session (enforced by the viewer's desktop, not by the relay).
    Agent,
}

impl Permissions {
    /// The permission `route` needs, for a device with customer reference `customer`.
    pub fn allows(&self, route: Route, customer: Option<&str>) -> bool {
        let flag = match route {
            Route::Read => true,
            Route::Write => self.write,
            Route::Audit => self.audit,
            Route::Sbom => self.sbom,
            Route::Agent => self.agent,
        };
        flag && self.sees(customer)
    }

    /// Whether a device with customer reference `customer` is in this member's scope.
    pub fn sees(&self, customer: Option<&str>) -> bool {
        self.customers.iter().any(|p| pattern_matches(p, customer))
    }

    /// What the team summary this member gets depends on: `sbom:` or `-:`, then the patterns
    /// sorted, without repeats, joined by `,`; just `*` when `*` is one of them. Two members with
    /// the same key get the same summary.
    pub fn scope_key(&self) -> String {
        let mut c: Vec<&str> = self.customers.iter().map(String::as_str).collect();
        if c.contains(&"*") {
            c = vec!["*"];
        }
        c.sort_unstable();
        c.dedup();
        format!("{}:{}", if self.sbom { "sbom" } else { "-" }, c.join(","))
    }
}

/// A customer pattern: `*` (every device, also those without a customer reference), a customer
/// reference (`valid_customer`: 1-32 of `A-Za-z0-9._-`, matched exactly, case-sensitive), or a
/// customer reference followed by one `*` (every reference that starts with it, the reference
/// itself included). Nothing else: no `*` elsewhere, no other wildcard.
pub fn valid_pattern(p: &str) -> bool {
    p == "*" || valid_customer(p.strip_suffix('*').unwrap_or(p))
}

/// Whether pattern `p` covers a device with customer reference `customer`.
pub fn pattern_matches(p: &str, customer: Option<&str>) -> bool {
    match (p, customer) {
        ("*", _) => true,
        (_, None) => false,
        (p, Some(c)) => match p.strip_suffix('*') {
            Some(prefix) => c.starts_with(prefix),
            None => p == c,
        },
    }
}

fn check_patterns(what: &str, customers: &[String]) -> Result<(), String> {
    if customers.len() > MAX_PATTERNS {
        return Err(format!("{what}: at most {MAX_PATTERNS} customer patterns"));
    }
    match customers.iter().find(|p| !valid_pattern(p)) {
        Some(p) => Err(format!("{what}: customer pattern {p:?}: `*`, a customer reference, or one followed by `*`")),
        None => Ok(()),
    }
}

fn yes() -> bool {
    true
}

fn every_customer() -> Vec<String> {
    vec!["*".into()]
}

/// The admin-signed team document (`PUT /api/admin/team`, docs/api.md).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TeamDoc {
    /// Must rise with every document a relay accepts.
    pub serial: u64,
    /// The OpenID Connect provider staff sign in with (none: no sign-in).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oidc: Option<Oidc>,
    /// App role value → what it grants (the grants of a person's roles add up).
    #[serde(default)]
    pub roles: BTreeMap<String, Grants>,
    /// Object ids of people refused a sign-in session.
    #[serde(default)]
    pub deny: Vec<String>,
    /// Member keys work only for entries with `service`; people sign in.
    #[serde(default)]
    pub oidc_only: bool,
    pub members: Vec<TeamMember>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Oidc {
    pub issuer: String,
    pub tenant: String,
    pub audience: String,
    pub client_id: String,
    pub scope: String,
}

/// What one app role grants; a missing field grants nothing.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Grants {
    #[serde(default)]
    pub write: bool,
    #[serde(default)]
    pub audit: bool,
    #[serde(default)]
    pub sbom: bool,
    #[serde(default)]
    pub agent: bool,
    #[serde(default)]
    pub customers: Vec<String>,
}

/// A key entry of the signed document. Fields missing read as a plain roster entry's rights.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TeamMember {
    pub name: String,
    /// Standard base64 of the 32-byte Ed25519 public key.
    pub key: String,
    #[serde(default)]
    pub write: bool,
    #[serde(default)]
    pub audit: bool,
    #[serde(default = "yes")]
    pub sbom: bool,
    #[serde(default = "yes")]
    pub agent: bool,
    #[serde(default = "every_customer")]
    pub customers: Vec<String>,
    /// A service identity (the bridge, a script): keeps working with `oidc_only`.
    #[serde(default)]
    pub service: bool,
}

impl TeamMember {
    pub fn permissions(&self) -> Permissions {
        Permissions { write: self.write, audit: self.audit, sbom: self.sbom, agent: self.agent, customers: self.customers.clone() }
    }
}

/// `PUT /api/admin/team` in its signed form: the [`TeamDoc`] in an envelope, and with an `oidc`
/// part also the [`Config`] (signed apart, so a relay can serve it without the members).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Upload {
    pub team: Signed,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<Signed>,
}

/// `GET /api/team/config` (the payload of `Upload::config`): the team document's serial and `oidc`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub serial: u64,
    pub oidc: Oidc,
}

#[derive(Debug)]
pub enum OpenError {
    /// Not a valid upload or document (400).
    Bad(String),
    /// Not signed by a trusted key, or the signature does not match (401).
    Untrusted(String),
}

fn text_ok(what: &str, s: &str, max: usize) -> Result<(), String> {
    if (1..=max).contains(&s.chars().count()) && !s.chars().any(char::is_control) {
        Ok(())
    } else {
        Err(format!("{what}: 1-{max} characters, no control characters"))
    }
}

impl TeamDoc {
    /// Checks the document and returns the members' keys in order.
    pub fn validate(&self) -> Result<Vec<VerifyingKey>, String> {
        if self.serial == 0 {
            return Err("serial must be at least 1".into());
        }
        if self.members.len() > MAX_MEMBERS {
            return Err(format!("at most {MAX_MEMBERS} members"));
        }
        if let Some(o) = &self.oidc {
            for (what, v) in [("oidc.issuer", &o.issuer), ("oidc.tenant", &o.tenant), ("oidc.audience", &o.audience), ("oidc.client_id", &o.client_id), ("oidc.scope", &o.scope)] {
                text_ok(what, v, 512)?;
            }
        }
        for (name, g) in &self.roles {
            if !(1..=128).contains(&name.len()) || !name.bytes().all(|b| b.is_ascii_graphic()) {
                return Err(format!("role {name:?}: 1-128 printable ASCII characters"));
            }
            check_patterns(&format!("role {name}"), &g.customers)?;
        }
        if let Some(d) = self.deny.iter().find(|d| !(1..=128).contains(&d.len()) || !d.bytes().all(|b| b.is_ascii_graphic())) {
            return Err(format!("deny {d:?}: 1-128 printable ASCII characters"));
        }
        for m in &self.members {
            check_patterns(&format!("member {}", m.name), &m.customers)?;
        }
        member_keys(self.members.iter().map(|m| (m.name.as_str(), m.key.as_str())))
    }
}

/// Opens an [`Upload`]: both envelopes signed by one of `trusted`, the document valid, the config
/// present exactly when the document has `oidc`, and saying the same serial and `oidc`. Returns
/// the document and its members' keys in order.
pub fn open(raw: &[u8], trusted: &[VerifyingKey]) -> Result<(TeamDoc, Vec<VerifyingKey>), OpenError> {
    let bad = |e: String| OpenError::Bad(e);
    if raw.len() > MAX_UPLOAD {
        return Err(bad(format!("a team document upload is at most {} KiB", MAX_UPLOAD >> 10)));
    }
    let up: Upload = serde_json::from_slice(raw).map_err(|e| bad(format!("not a signed team document: {e}")))?;
    let payload = envelope::open(&up.team, trusted).map_err(OpenError::Untrusted)?;
    if payload.len() > MAX_DOC {
        return Err(bad(format!("a team document is at most {} KiB", MAX_DOC >> 10)));
    }
    let doc: TeamDoc = serde_json::from_slice(&payload).map_err(|e| bad(e.to_string()))?;
    let keys = doc.validate().map_err(bad)?;
    match (&doc.oidc, &up.config) {
        (None, None) => {}
        (Some(o), Some(c)) => {
            let cfg: Config = serde_json::from_slice(&envelope::open(c, trusted).map_err(OpenError::Untrusted)?).map_err(|e| bad(format!("config: {e}")))?;
            if cfg.serial != doc.serial || &cfg.oidc != o {
                return Err(bad("the config does not say the team document's serial and oidc".into()));
            }
        }
        (Some(_), None) => return Err(bad("a document with oidc needs its signed config".into())),
        (None, Some(_)) => return Err(bad("a config without oidc in the document".into())),
    }
    Ok((doc, keys))
}

/// `hex(sha256(key))`, first 16 hex digits, uppercase, in groups of 4: `9F3A-12B0-C4D1-77E2`.
pub fn fingerprint(key: &VerifyingKey) -> String {
    let d = Sha256::digest(key.as_bytes());
    d[..8].chunks(2).map(|c| format!("{:02X}{:02X}", c[0], c[1])).collect::<Vec<_>>().join("-")
}

/// The [`fingerprint`] as `Tether-Sig` names a key (`kid=`): 16 lower-case hex digits.
pub fn key_id(key: &VerifyingKey) -> String {
    Sha256::digest(key.as_bytes())[..8].iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{generate_key, key_to_b64};
    use base64::Engine;

    #[test]
    fn fingerprint_vector() {
        // Key bytes 0..32. The same vector is in apps/manage/src/lib/team.test.ts.
        let raw: [u8; 32] = std::array::from_fn(|i| i as u8);
        let k = VerifyingKey::from_bytes(&raw).unwrap();
        assert_eq!(key_to_b64(&k), "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=");
        assert_eq!(fingerprint(&k), "630D-CD29-66C4-3366");
    }

    fn member(name: &str, key: String) -> Member {
        Member { name: name.into(), key, write: false, audit: false }
    }

    #[test]
    fn roster_validation() {
        let a = key_to_b64(&generate_key().verifying_key());
        let b = key_to_b64(&generate_key().verifying_key());
        let ok = Roster { members: vec![member("Anna", a.clone()), member("Ben", b)] };
        assert_eq!(ok.validate().unwrap().len(), 2);
        assert!(Roster::default().validate().unwrap().is_empty());
        for bad in [
            vec![member("Anna", a.clone()), member("Ben", a.clone())],
            vec![member("", a.clone())],
            vec![member(" Anna", a.clone())],
            vec![member("A\nB", a.clone())],
            vec![member(&"x".repeat(65), a.clone())],
            vec![member("Anna", "AAAA".into())],
            vec![member("Anna", "not base64".into())],
        ] {
            assert!(Roster { members: bad.clone() }.validate().is_err(), "{bad:?}");
        }
        assert!(Roster { members: vec![member(&"ä".repeat(64), a.clone())] }.validate().is_ok());
        let many = (0..=ROSTER_MAX_MEMBERS).map(|i| member(&format!("m{i}"), key_to_b64(&generate_key().verifying_key()))).collect();
        assert!(Roster { members: many }.validate().is_err());
        assert!(serde_json::from_str::<Roster>(r#"{"members":[],"x":1}"#).is_err());
        let extra = format!(r#"{{"members":[{{"name":"A","key":"{a}","write":true,"audit":false,"admin":true}}]}}"#);
        assert!(serde_json::from_str::<Roster>(&extra).is_err());
        // The plain roster keeps its old limit and its old fields.
        let sbom = format!(r#"{{"members":[{{"name":"A","key":"{a}","write":true,"audit":false,"sbom":true}}]}}"#);
        assert!(serde_json::from_str::<Roster>(&sbom).is_err());
        assert_eq!(member("A", a.clone()).permissions(), Permissions { write: false, audit: false, sbom: true, agent: true, customers: vec!["*".into()] });
    }

    fn oidc() -> Oidc {
        Oidc { issuer: "https://login.example.com/t/v2.0".into(), tenant: "t".into(), audience: "api://tether".into(), client_id: "c".into(), scope: "api://tether/access".into() }
    }

    fn doc_json(serial: u64, members: serde_json::Value, extra: serde_json::Value) -> Vec<u8> {
        let mut d = serde_json::json!({"serial": serial, "members": members});
        for (k, v) in extra.as_object().unwrap() {
            d[k] = v.clone();
        }
        serde_json::to_vec(&d).unwrap()
    }

    fn upload(team: &[u8], config: Option<&[u8]>, k: &crate::SigningKey) -> Vec<u8> {
        let u = Upload { team: crate::envelope::sign(team, k), config: config.map(|c| crate::envelope::sign(c, k)) };
        serde_json::to_vec(&u).unwrap()
    }

    #[test]
    fn signed_document_opens_only_with_a_trusted_key() {
        let (admin, other) = (generate_key(), generate_key());
        let a = key_to_b64(&generate_key().verifying_key());
        let members = serde_json::json!([{"name": "Anna", "key": a, "write": true, "audit": false}]);
        let raw = upload(&doc_json(3, members.clone(), serde_json::json!({})), None, &admin);
        let (doc, keys) = open(&raw, &[other.verifying_key(), admin.verifying_key()]).unwrap();
        assert_eq!((doc.serial, keys.len(), doc.oidc.is_none(), doc.oidc_only), (3, 1, true, false));
        // Fields missing on an entry read as today's rights.
        assert_eq!(doc.members[0].permissions(), Permissions { write: true, audit: false, sbom: true, agent: true, customers: vec!["*".into()] });
        assert!(matches!(open(&raw, &[other.verifying_key()]), Err(OpenError::Untrusted(_))));
        // A tampered payload, a config signed by an untrusted key.
        let mut u: Upload = serde_json::from_slice(&raw).unwrap();
        u.team.payload = crate::b64().encode(doc_json(4, members.clone(), serde_json::json!({})));
        assert!(matches!(open(&serde_json::to_vec(&u).unwrap(), &[admin.verifying_key()]), Err(OpenError::Untrusted(_))));
        let with_oidc = doc_json(5, members.clone(), serde_json::json!({"oidc": oidc()}));
        let cfg = serde_json::to_vec(&Config { serial: 5, oidc: oidc() }).unwrap();
        let mut u = Upload { team: crate::envelope::sign(&with_oidc, &admin), config: Some(crate::envelope::sign(&cfg, &other)) };
        assert!(matches!(open(&serde_json::to_vec(&u).unwrap(), &[admin.verifying_key()]), Err(OpenError::Untrusted(_))));
        u.config = Some(crate::envelope::sign(&cfg, &admin));
        let (doc, _) = open(&serde_json::to_vec(&u).unwrap(), &[admin.verifying_key()]).unwrap();
        assert_eq!(doc.oidc, Some(oidc()));
        // The config must be there with oidc, absent without, and say what the document says.
        assert!(matches!(open(&upload(&with_oidc, None, &admin), &[admin.verifying_key()]), Err(OpenError::Bad(_))));
        assert!(matches!(open(&upload(&doc_json(5, members.clone(), serde_json::json!({})), Some(&cfg), &admin), &[admin.verifying_key()]), Err(OpenError::Bad(_))));
        let other_serial = serde_json::to_vec(&Config { serial: 6, oidc: oidc() }).unwrap();
        assert!(matches!(open(&upload(&with_oidc, Some(&other_serial), &admin), &[admin.verifying_key()]), Err(OpenError::Bad(_))));
        let other_oidc = serde_json::to_vec(&Config { serial: 5, oidc: Oidc { client_id: "x".into(), ..oidc() } }).unwrap();
        assert!(matches!(open(&upload(&with_oidc, Some(&other_oidc), &admin), &[admin.verifying_key()]), Err(OpenError::Bad(_))));
        // Not an upload, a plain roster, an oversized payload.
        assert!(matches!(open(b"{\"members\":[]}", &[admin.verifying_key()]), Err(OpenError::Bad(_))));
        let big = doc_json(7, members.clone(), serde_json::json!({"deny": vec!["x".repeat(64); MAX_DOC / 64]}));
        assert!(matches!(open(&upload(&big, None, &admin), &[admin.verifying_key()]), Err(OpenError::Bad(e)) if e.contains("KiB")));
    }

    #[test]
    fn document_validation() {
        let admin = generate_key();
        let a = key_to_b64(&generate_key().verifying_key());
        let ok = |d: Vec<u8>| open(&upload(&d, None, &admin), &[admin.verifying_key()]).map(|(d, _)| d);
        let m = |extra: serde_json::Value| {
            let mut e = serde_json::json!({"name": "Anna", "key": a, "write": false, "audit": false});
            for (k, v) in extra.as_object().unwrap() {
                e[k] = v.clone();
            }
            serde_json::json!([e])
        };
        let full = serde_json::json!({"sbom": false, "agent": false, "customers": ["4711", "47*"], "service": true});
        let d = ok(doc_json(1, m(full), serde_json::json!({"deny": ["0f3c-oid"], "oidc_only": true, "roles": {"Tether.Software": {"sbom": true}, "Tether.Customer.4711": {"customers": ["4711"]}}}))).unwrap();
        assert_eq!(d.members[0].permissions(), Permissions { write: false, audit: false, sbom: false, agent: false, customers: vec!["4711".into(), "47*".into()] });
        assert!(d.members[0].service && d.oidc_only && d.deny == ["0f3c-oid"]);
        assert_eq!(d.roles["Tether.Software"], Grants { sbom: true, ..Default::default() });
        for (bad, why) in [
            (doc_json(0, m(serde_json::json!({})), serde_json::json!({})), "serial 0"),
            (doc_json(1, m(serde_json::json!({"customers": ["4*7"]})), serde_json::json!({})), "star inside"),
            (doc_json(1, m(serde_json::json!({"customers": ["**"]})), serde_json::json!({})), "two stars"),
            (doc_json(1, m(serde_json::json!({"customers": [""]})), serde_json::json!({})), "empty pattern"),
            (doc_json(1, m(serde_json::json!({"customers": ["47 11"]})), serde_json::json!({})), "space"),
            (doc_json(1, m(serde_json::json!({"customers": vec!["47.*"; 65]})), serde_json::json!({})), "too many patterns"),
            (doc_json(1, m(serde_json::json!({"admin": true})), serde_json::json!({})), "unknown member field"),
            (doc_json(1, m(serde_json::json!({})), serde_json::json!({"x": 1})), "unknown field"),
            (doc_json(1, m(serde_json::json!({})), serde_json::json!({"deny": [""]})), "empty oid"),
            (doc_json(1, m(serde_json::json!({})), serde_json::json!({"roles": {"": {}}})), "empty role"),
            (doc_json(1, m(serde_json::json!({})), serde_json::json!({"roles": {"R": {"customers": ["a*b"]}}})), "role pattern"),
            (doc_json(1, m(serde_json::json!({})), serde_json::json!({"roles": {"R": {"admin": true}}})), "unknown grant"),
            (doc_json(1, serde_json::json!([{"name": "A", "key": a}, {"name": "B", "key": a}]), serde_json::json!({})), "key twice"),
            (doc_json(1, serde_json::json!([{"name": " A", "key": a}]), serde_json::json!({})), "name"),
        ] {
            assert!(ok(bad).is_err(), "{why}");
        }
        let many: Vec<serde_json::Value> = (0..=MAX_MEMBERS).map(|i| serde_json::json!({"name": format!("m{i}"), "key": key_to_b64(&generate_key().verifying_key())})).collect();
        assert!(ok(doc_json(1, serde_json::json!(many[..MAX_MEMBERS]), serde_json::json!({}))).is_ok(), "2000 fit in 256 KiB");
        assert!(ok(doc_json(1, serde_json::json!(many), serde_json::json!({}))).is_err());
    }

    #[test]
    fn customer_patterns() {
        for (p, ok) in [("*", true), ("4711", true), ("47*", true), ("a.b_c-d", true), ("x*", true), ("4*7", false), ("**", false), ("*4", false), ("", false), ("47 *", false), (&"1".repeat(32), true), (&format!("{}*", "1".repeat(32)), true), (&"1".repeat(33), false)] {
            assert_eq!(valid_pattern(p), ok, "{p:?}");
        }
        for (p, c, hit) in [("*", None, true), ("*", Some("4711"), true), ("4711", Some("4711"), true), ("4711", Some("47110"), false), ("4711", None, false), ("47*", Some("47"), true), ("47*", Some("4711"), true), ("47*", Some("4811"), false), ("47*", None, false), ("47*", Some("x47"), false), ("A*", Some("a1"), false)] {
            assert_eq!(pattern_matches(p, c), hit, "{p} {c:?}");
        }
        let p = Permissions { write: false, audit: true, sbom: false, agent: true, customers: vec!["47*".into(), "0815".into()] };
        assert!(p.allows(Route::Read, Some("4711")) && p.allows(Route::Audit, Some("0815")) && p.allows(Route::Agent, Some("47")));
        assert!(!p.allows(Route::Read, Some("4811")) && !p.allows(Route::Read, None));
        assert!(!p.allows(Route::Write, Some("4711")) && !p.allows(Route::Sbom, Some("4711")));
        // The cache key: the same set however written; `*` swallows the rest; sbom is part of it.
        let k = |c: &[&str], sbom: bool| Permissions { customers: c.iter().map(|s| s.to_string()).collect(), sbom, ..p.clone() }.scope_key();
        assert_eq!(k(&["47*", "0815", "47*"], true), k(&["0815", "47*"], true));
        assert_ne!(k(&["47*"], true), k(&["47*"], false));
        assert_ne!(k(&["47*"], true), k(&["4711"], true));
        assert_eq!(k(&["4711", "*"], true), k(&["*"], true));
        assert_ne!(k(&[], true), k(&["*"], true));
        assert_eq!(k(&["*"], true), DEFAULT_SCOPE);
    }

    #[test]
    fn key_id_is_the_fingerprint_in_lower_case() {
        let raw: [u8; 32] = std::array::from_fn(|i| i as u8);
        let k = VerifyingKey::from_bytes(&raw).unwrap();
        assert_eq!(key_id(&k), "630dcd2966c43366");
    }
}

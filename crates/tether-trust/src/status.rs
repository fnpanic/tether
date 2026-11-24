//! Device status: the enrollment token and the report a device-status agent posts to the relay
//! (`POST /api/status/enroll`, `POST /api/status`; requests signed with
//! [`crate::reqauth::sign_device_request`]).
//!
//! - Enrollment token: base64url (no padding) of the JSON of an admin-signed [`envelope::Signed`]
//!   whose payload is a [`TokenPayload`] (`purpose` [`TOKEN_PURPOSE`], a random `id`, optionally
//!   the one `tether_id` it is issued for). Reusable until `exp` unless issued for a Tether ID or
//!   revoked (the relay keeps both records). With `tether_proof` (a grant in a policy, public), it
//!   enrols only with a Tether proof: [`EnrollRequest::tether_proof`], see [`agent_link_message`].
//! - [`Report`]: a full report (`products`, `changes` and `body` present) or a heartbeat (all
//!   three absent). `body` is opaque to the relay and kept byte-exact by it.

use crate::envelope;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine;
use ed25519_dalek::{SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const TOKEN_PURPOSE: &str = "status-enroll";
/// Largest status post.
pub const REPORT_MAX: usize = 64 << 10;
/// Largest enrollment request.
pub const ENROLL_MAX: usize = 4096;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenPayload {
    pub purpose: String,
    /// Unix seconds; the token is refused from `exp` on.
    pub exp: u64,
    pub label: String,
    /// Random, [`valid_token_id`]: what a relay revokes, and records on each pin made with it.
    pub id: String,
    /// Issued for this Tether ID: the token enrols one device, which gets exactly this ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tether_id: Option<String>,
    /// The customer reference the relay sets on the device that enrols with this token ([`valid_customer`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub customer: Option<String>,
    /// Enrols only with a Tether proof ([`verify_agent_link`]), which names the ID: never with
    /// `tether_id`. Written only when set, so a token without it is what every relay knows.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tether_proof: bool,
}

const PROOF_AND_ID: &str = "a tether_proof token names no tether_id";

/// 1-32 characters `[A-Za-z0-9._-]`: a customer reference.
pub fn valid_customer(s: &str) -> bool {
    (1..=32).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// 16-32 characters `[A-Za-z0-9]`.
pub fn valid_token_id(s: &str) -> bool {
    (16..=32).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// A Tether ID a token may be issued for: [`valid_tether_id`], and never `m:…` (the technician
/// app's rows of agents without a Tether ID).
fn bindable(s: &str) -> bool {
    valid_tether_id(s) && !s.starts_with("m:")
}

/// 1-`max` characters, none of them a control character.
fn text(s: &str, max: usize) -> bool {
    !s.is_empty() && s.chars().count() <= max && !s.chars().any(char::is_control)
}

/// An optional short text (`time`, `since`, `version`): at most `max` characters, no control characters.
fn short(s: &str, max: usize) -> bool {
    s.chars().count() <= max && !s.chars().any(char::is_control)
}

/// An enrollment token for `label`, valid until `exp` (unix seconds), with a new random id; with
/// `tether_id`, issued for that one Tether ID; with `customer`, setting it on the device; with
/// `tether_proof`, enrolling only with a Tether proof (no `tether_id`). Answers `(token, id)`.
pub fn make_token(label: &str, exp: u64, tether_id: Option<&str>, customer: Option<&str>, tether_proof: bool, admin: &SigningKey) -> Result<(String, String), String> {
    if tether_proof && tether_id.is_some() {
        return Err(PROOF_AND_ID.into());
    }
    if !text(label, 64) {
        return Err("bad label".into());
    }
    if tether_id.is_some_and(|t| !bindable(t)) {
        return Err("bad tether_id".into());
    }
    if customer.is_some_and(|c| !valid_customer(c)) {
        return Err("bad customer".into());
    }
    let mut raw = [0u8; 16];
    rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut raw);
    let id: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    let p = TokenPayload { purpose: TOKEN_PURPOSE.into(), exp, label: label.into(), id: id.clone(), tether_id: tether_id.map(Into::into), customer: customer.map(Into::into), tether_proof };
    let doc = envelope::sign(&serde_json::to_vec(&p).map_err(|e| e.to_string())?, admin);
    Ok((B64URL.encode(serde_json::to_vec(&doc).map_err(|e| e.to_string())?), id))
}

/// The payload of a token signed by one of the `admin` keys and not expired at `now`.
pub fn verify_token(token: &str, admin: &[VerifyingKey], now: u64) -> Result<TokenPayload, String> {
    const NOT: &str = "not an enrollment token";
    let raw = B64URL.decode(token.trim()).map_err(|_| NOT.to_owned())?;
    let doc: envelope::Signed = serde_json::from_slice(&raw).map_err(|_| NOT.to_owned())?;
    let payload = envelope::open(&doc, admin)?;
    let p: TokenPayload = serde_json::from_slice(&payload).map_err(|_| NOT.to_owned())?;
    if p.purpose != TOKEN_PURPOSE {
        return Err(NOT.into());
    }
    if p.exp <= now {
        return Err("expired".into());
    }
    if !text(&p.label, 64) {
        return Err("bad label".into());
    }
    if !valid_token_id(&p.id) {
        return Err("bad id".into());
    }
    if p.tether_id.as_deref().is_some_and(|t| !bindable(t)) {
        return Err("bad tether_id".into());
    }
    if p.customer.as_deref().is_some_and(|c| !valid_customer(c)) {
        return Err("bad customer".into());
    }
    if p.tether_proof && p.tether_id.is_some() {
        return Err(PROOF_AND_ID.into());
    }
    Ok(p)
}

/// Body of `POST /api/status/enroll`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollRequest {
    pub uuid: String,
    /// Standard base64 of the 32-byte Ed25519 public key ([`crate::key_to_b64`]).
    pub pubkey: String,
    pub token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tether_id: Option<String>,
    /// With a `tether_proof` token: [`sign_agent_link`] by the Tether identity key of `tether_id`
    /// over this request's `tether_id`, `uuid` and `pubkey`. Ignored with any other token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tether_proof: Option<String>,
}

/// What a Tether proof signs: `"tether-agent-link-v1\n<tether_id>\n<uuid>\n<pubkey>"`, `pubkey`
/// as in the request (standard base64).
pub fn agent_link_message(tether_id: &str, uuid: &str, pubkey: &str) -> Vec<u8> {
    format!("tether-agent-link-v1\n{tether_id}\n{uuid}\n{pubkey}").into_bytes()
}

/// A Tether proof: standard base64 of the Ed25519 signature of [`agent_link_message`] by the
/// identity key whose 32-byte seed is `seed` (the first half of the core's 64-byte secret key).
pub fn sign_agent_link(seed: &[u8; 32], tether_id: &str, uuid: &str, pubkey: &str) -> String {
    let sig = ed25519_dalek::Signer::sign(&SigningKey::from_bytes(seed), &agent_link_message(tether_id, uuid, pubkey));
    crate::b64().encode(sig.to_bytes())
}

/// Check a Tether proof against `pk_b64`, the key hbbs holds for `tether_id` (standard base64).
/// Strict: a weak key or a non-canonical signature never verifies.
pub fn verify_agent_link(pk_b64: &str, sig_b64: &str, tether_id: &str, uuid: &str, pubkey: &str) -> Result<(), String> {
    let key = crate::key_from_b64(pk_b64)?;
    let sig = crate::b64().decode(sig_b64.trim()).map_err(|_| "bad signature encoding".to_owned())?;
    let sig = ed25519_dalek::Signature::from_slice(&sig).map_err(|_| "bad signature".to_owned())?;
    key.verify_strict(&agent_link_message(tether_id, uuid, pubkey), &sig).map_err(|_| "signature does not match".to_owned())
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Counts {
    pub ok: u32,
    pub warn: u32,
    pub crit: u32,
    pub unknown: u32,
    /// Unknown checks that are only "not configured".
    pub setup: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Product {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// `server`, `client` or `single`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

/// A check that is not OK.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fault {
    pub id: String,
    /// `warn`, `crit` or `unknown`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub id: String,
    pub from: String,
    pub to: String,
    pub time: String,
}

/// Body of `POST /api/status`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub v: u32,
    pub uuid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tether_id: Option<String>,
    pub host: String,
    pub seq: u64,
    pub time: String,
    pub next: u32,
    pub agent: String,
    pub packs: BTreeMap<String, u64>,
    pub overall: String,
    pub counts: Counts,
    pub faults: Vec<Fault>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub products: Option<Vec<Product>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changes: Option<Vec<Change>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<serde_json::Value>,
}

const STATES: &[&str] = &["ok", "warn", "crit", "unknown"];

fn valid_check_id(s: &str) -> bool {
    (1..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'/' | b'-'))
}

fn valid_pack(s: &str) -> bool {
    (1..=32).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl Report {
    /// The field rules of the wire contract (`seq` order and `tether_id` pinning are the relay's).
    pub fn validate(&self) -> Result<(), String> {
        if self.v != 1 {
            return Err("unsupported report version".into());
        }
        if !valid_uuid(&self.uuid) {
            return Err("bad uuid".into());
        }
        if self.tether_id.as_deref().is_some_and(|t| !valid_tether_id(t)) {
            return Err("bad tether_id".into());
        }
        if !text(&self.host, 64) {
            return Err("bad host".into());
        }
        if !short(&self.time, 40) {
            return Err("bad time".into());
        }
        if !(60..=3600).contains(&self.next) {
            return Err("next must be 60-3600".into());
        }
        if !crate::release::valid_version(&self.agent) {
            return Err("bad agent version".into());
        }
        if self.packs.len() > 32 || !self.packs.keys().all(|k| valid_pack(k)) {
            return Err("packs: at most 32, names [a-z0-9-]{1,32}".into());
        }
        if !STATES.contains(&self.overall.as_str()) {
            return Err("bad overall state".into());
        }
        if self.faults.len() > 50 {
            return Err("at most 50 faults".into());
        }
        for f in &self.faults {
            if !valid_check_id(&f.id) || !STATES[1..].contains(&f.state.as_str()) || f.since.as_deref().is_some_and(|s| !short(s, 40)) {
                return Err(format!("bad fault {:?}", f.id));
            }
        }
        match (&self.products, &self.changes, &self.body) {
            (None, None, None) => return Ok(()),
            (Some(_), Some(_), Some(_)) => {}
            _ => return Err("a full report has products, changes and body, a heartbeat none of them".into()),
        }
        let products = self.products.as_deref().unwrap_or_default();
        if products.len() > 32 {
            return Err("at most 32 products".into());
        }
        for p in products {
            let role_ok = p.role.as_deref().is_none_or(|r| matches!(r, "server" | "client" | "single"));
            if !text(&p.name, 64) || p.version.as_deref().is_some_and(|v| !short(v, 32)) || !role_ok {
                return Err(format!("bad product {:?}", p.name));
            }
        }
        let changes = self.changes.as_deref().unwrap_or_default();
        if changes.len() > 200 {
            return Err("at most 200 changes".into());
        }
        for c in changes {
            if !valid_check_id(&c.id) || !STATES.contains(&c.from.as_str()) || !STATES.contains(&c.to.as_str()) || !short(&c.time, 40) {
                return Err(format!("bad change {:?}", c.id));
            }
        }
        if !self.body.as_ref().is_some_and(serde_json::Value::is_object) {
            return Err("body must be a JSON object".into());
        }
        Ok(())
    }

    /// A full report, not a heartbeat.
    pub fn is_full(&self) -> bool {
        self.products.is_some() && self.changes.is_some() && self.body.is_some()
    }
}

/// The relay's answer to an enrollment and a status post.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    /// Opaque change hint for the monitor release index: compare for inequality only.
    pub releases: u64,
    /// Seconds until the next post.
    pub next: u64,
    /// The relay takes the software list (`PUT /api/status/sbom`). Sent only when true, so
    /// older agents and relays see the answer as before.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sbom: bool,
}

/// 8-64 characters `[A-Za-z0-9-]`.
pub fn valid_uuid(s: &str) -> bool {
    (8..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// 1-32 printable ASCII characters, no spaces.
pub fn valid_tether_id(s: &str) -> bool {
    (1..=32).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_graphic())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_fields_are_omitted_and_body_is_opaque() {
        let hb = r#"{"v":1,"uuid":"dev-0001","host":"PC-1","seq":6,"time":"2026-11-23T08:20:00Z","next":300,"agent":"0.1.0",
          "packs":{},"overall":"ok","counts":{"ok":1,"warn":0,"crit":0,"unknown":0,"setup":0},"faults":[{"id":"a","state":"warn"}]}"#;
        let r: Report = serde_json::from_str(hb).unwrap();
        assert!(r.validate().is_ok() && !r.is_full());
        let out = serde_json::to_string(&r).unwrap();
        for k in ["tether_id", "products", "changes", "body", "since"] {
            assert!(!out.contains(k), "{k} in {out}");
        }
        let full = hb.replace("\"faults\"", r#""products":[{"name":"X"}],"changes":[],"body":{"checks":[{"id":"a","hint":{"en":"x"},"new":1}],"other":true},"faults""#);
        assert!(serde_json::from_str::<Report>(&full).unwrap().validate().is_ok());
        let array_body = full.replace(r#""body":{"checks""#, r#""body":[{"checks""#).replace("true}", "true}]");
        assert_eq!(serde_json::from_str::<Report>(&array_body).unwrap().validate().unwrap_err(), "body must be a JSON object");
        let e = EnrollRequest { uuid: "dev-0001".into(), pubkey: "k".into(), token: "t".into(), tether_id: None, tether_proof: None };
        assert_eq!(serde_json::to_string(&e).unwrap(), r#"{"uuid":"dev-0001","pubkey":"k","token":"t"}"#);
        assert!(valid_uuid("dev-0001") && !valid_uuid("dev-001") && !valid_uuid("dev_0001"));
        assert!(valid_tether_id("123456789") && !valid_tether_id("1 2") && !valid_tether_id(""));
    }

    #[test]
    fn answer_names_sbom_only_when_true() {
        let a = Answer { releases: 3, next: 300, sbom: false };
        assert_eq!(serde_json::to_string(&a).unwrap(), r#"{"releases":3,"next":300}"#);
        assert_eq!(serde_json::to_string(&Answer { sbom: true, ..a }).unwrap(), r#"{"releases":3,"next":300,"sbom":true}"#);
        assert_eq!(serde_json::from_str::<Answer>(r#"{"releases":3,"next":300}"#).unwrap(), a);
        assert!(serde_json::from_str::<Answer>(r#"{"releases":3,"next":300,"sbom":true}"#).unwrap().sbom);
    }

    /// A token over any payload JSON, signed by `admin`.
    fn raw_token(payload: &str, admin: &SigningKey) -> String {
        B64URL.encode(serde_json::to_vec(&crate::envelope::sign(payload.as_bytes(), admin)).unwrap())
    }

    #[test]
    fn token_ids_are_random_and_required() {
        let admin = crate::generate_key();
        let (t1, id1) = make_token("a", 2_000, None, None, false, &admin).unwrap();
        let (_, id2) = make_token("a", 2_000, None, None, false, &admin).unwrap();
        assert!(valid_token_id(&id1) && id1 != id2, "{id1} {id2}");
        assert_eq!(verify_token(&t1, &[admin.verifying_key()], 1).unwrap().id, id1);
        for (id, ok) in [("abcdefghABCDEFGH", true), (&"Z9".repeat(16) as &str, true), ("abcdefghABCDEFG", false), (&"a".repeat(33), false), ("abcdefgh-BCDEFGH", false), ("abcdefghABCDEFGé", false)] {
            assert_eq!(valid_token_id(id), ok, "{id}");
        }
        let k = [admin.verifying_key()];
        let no_id = raw_token(r#"{"purpose":"status-enroll","exp":2000,"label":"a"}"#, &admin);
        assert_eq!(verify_token(&no_id, &k, 1).unwrap_err(), "not an enrollment token", "a token without an id is refused");
        let bad_id = raw_token(r#"{"purpose":"status-enroll","exp":2000,"label":"a","id":"short"}"#, &admin);
        assert_eq!(verify_token(&bad_id, &k, 1).unwrap_err(), "bad id");
        let ok = raw_token(r#"{"purpose":"status-enroll","exp":2000,"label":"a","id":"abcdefghABCDEFGH"}"#, &admin);
        assert_eq!(verify_token(&ok, &k, 1).unwrap().id, "abcdefghABCDEFGH");
    }

    #[test]
    fn bound_token_round_trip() {
        let admin = crate::generate_key();
        let k = [admin.verifying_key()];
        let (t, id) = make_token("front desk", 2_000, Some("123456789"), None, false, &admin).unwrap();
        assert_eq!(verify_token(&t, &k, 1).unwrap(), TokenPayload { purpose: TOKEN_PURPOSE.into(), exp: 2_000, label: "front desk".into(), id, tether_id: Some("123456789".into()), customer: None, tether_proof: false });
        for bad in ["", "1 2", "m:dev-0001", &"1".repeat(33)] {
            assert_eq!(make_token("x", 2_000, Some(bad), None, false, &admin).unwrap_err(), "bad tether_id", "{bad:?}");
            let forged = raw_token(&format!(r#"{{"purpose":"status-enroll","exp":2000,"label":"a","id":"abcdefghABCDEFGH","tether_id":{}}}"#, serde_json::to_string(bad).unwrap()), &admin);
            assert_eq!(verify_token(&forged, &k, 1).unwrap_err(), "bad tether_id", "{bad:?}");
        }
        // Unbound: no `tether_id` field at all.
        let (t, _) = make_token("x", 2_000, None, None, false, &admin).unwrap();
        let payload = crate::envelope::open(&serde_json::from_slice(&B64URL.decode(&t).unwrap()).unwrap(), &k).unwrap();
        assert!(!String::from_utf8(payload).unwrap().contains("tether_id"));
    }

    #[test]
    fn token_round_trip_and_refusals() {
        let admin = crate::generate_key();
        let (t, id) = make_token("rollout north", 2_000, None, None, false, &admin).unwrap();
        assert!(!t.contains('=') && !t.contains('+') && !t.contains('/'), "base64url without padding");
        let p = verify_token(&t, &[admin.verifying_key()], 1_999).unwrap();
        assert_eq!(p, TokenPayload { purpose: "status-enroll".into(), exp: 2_000, label: "rollout north".into(), id, tether_id: None, customer: None, tether_proof: false });
        assert_eq!(verify_token(&t, &[admin.verifying_key()], 2_000).unwrap_err(), "expired");
        assert_eq!(verify_token(&t, &[crate::generate_key().verifying_key()], 1).unwrap_err(), "signed by an untrusted key");
        let policy = crate::envelope::sign(br#"{"serial":1,"issued_at":1}"#, &admin);
        let as_token = B64URL.encode(serde_json::to_vec(&policy).unwrap());
        assert_eq!(verify_token(&as_token, &[admin.verifying_key()], 1).unwrap_err(), "not an enrollment token");
        assert!(make_token("", 2_000, None, None, false, &admin).is_err() && make_token(&"x".repeat(65), 2_000, None, None, false, &admin).is_err());
    }

    #[test]
    fn customer_rule() {
        for ok in ["4711", "A.b_c-9", &"a".repeat(32)] {
            assert!(valid_customer(ok), "{ok}");
        }
        for bad in ["", &"a".repeat(33), "47 11", "a/b", "a
", "ä"] {
            assert!(!valid_customer(bad), "{bad:?}");
        }
    }

    #[test]
    fn token_carries_customer() {
        let admin = crate::generate_key();
        let k = [admin.verifying_key()];
        let (t, _) = make_token("x", 2_000, None, Some("4711"), false, &admin).unwrap();
        assert_eq!(verify_token(&t, &k, 1).unwrap().customer.as_deref(), Some("4711"));
        let (t, _) = make_token("x", 2_000, None, None, false, &admin).unwrap();
        let payload = crate::envelope::open(&serde_json::from_slice(&B64URL.decode(&t).unwrap()).unwrap(), &k).unwrap();
        assert!(!String::from_utf8(payload).unwrap().contains("customer"));
        assert_eq!(verify_token(&t, &k, 1).unwrap().customer, None);
        assert_eq!(make_token("x", 2_000, None, Some("a/b"), false, &admin).unwrap_err(), "bad customer");
    }

    #[test]
    fn token_with_bad_customer_is_refused() {
        let admin = crate::generate_key();
        let forged = raw_token(r#"{"purpose":"status-enroll","exp":2000,"label":"a","id":"abcdefghABCDEFGH","customer":"a b"}"#, &admin);
        assert_eq!(verify_token(&forged, &[admin.verifying_key()], 1).unwrap_err(), "bad customer");
    }

    #[test]
    fn proof_token_round_trip_and_refusals() {
        let admin = crate::generate_key();
        let k = [admin.verifying_key()];
        let (t, id) = make_token("policy:monitored", 2_000, None, Some("4711"), true, &admin).unwrap();
        let p = verify_token(&t, &k, 1).unwrap();
        assert_eq!(p, TokenPayload { purpose: TOKEN_PURPOSE.into(), exp: 2_000, label: "policy:monitored".into(), id, tether_id: None, customer: Some("4711".into()), tether_proof: true });
        let payload = String::from_utf8(crate::envelope::open(&serde_json::from_slice(&B64URL.decode(&t).unwrap()).unwrap(), &k).unwrap()).unwrap();
        assert!(payload.ends_with(r#","tether_proof":true}"#), "{payload}");
        // The proof names the ID: a token with both is refused, made or received.
        assert_eq!(make_token("x", 2_000, Some("123456789"), None, true, &admin).unwrap_err(), "a tether_proof token names no tether_id");
        let forged = raw_token(r#"{"purpose":"status-enroll","exp":2000,"label":"a","id":"abcdefghABCDEFGH","tether_id":"123456789","tether_proof":true}"#, &admin);
        assert_eq!(verify_token(&forged, &k, 1).unwrap_err(), "a tether_proof token names no tether_id");
        // `"tether_proof":false` written out is read as no flag.
        let explicit = raw_token(r#"{"purpose":"status-enroll","exp":2000,"label":"a","id":"abcdefghABCDEFGH","tether_id":"123456789","tether_proof":false}"#, &admin);
        assert!(!verify_token(&explicit, &k, 1).unwrap().tether_proof);
        // The largest grant fits the policy feature that carries it.
        let (big, _) = make_token(&"x".repeat(64), u64::MAX, None, Some(&"c".repeat(32)), true, &admin).unwrap();
        assert!(big.len() <= 1024, "{}", big.len());
    }

    #[test]
    fn token_without_flag_is_unchanged() {
        // An old relay (deny_unknown_fields) must keep accepting every token made without the flag.
        let p = TokenPayload { purpose: TOKEN_PURPOSE.into(), exp: 2_000, label: "a".into(), id: "abcdefghABCDEFGH".into(), tether_id: Some("123456789".into()), customer: Some("4711".into()), tether_proof: false };
        assert_eq!(serde_json::to_string(&p).unwrap(), r#"{"purpose":"status-enroll","exp":2000,"label":"a","id":"abcdefghABCDEFGH","tether_id":"123456789","customer":"4711"}"#);
        let admin = crate::generate_key();
        let k = [admin.verifying_key()];
        let (t, _) = make_token("a", 2_000, None, None, false, &admin).unwrap();
        let payload = String::from_utf8(crate::envelope::open(&serde_json::from_slice(&B64URL.decode(&t).unwrap()).unwrap(), &k).unwrap()).unwrap();
        assert!(!payload.contains("tether_proof"), "{payload}");
        // A payload of today (no field) still parses.
        let old = raw_token(r#"{"purpose":"status-enroll","exp":2000,"label":"a","id":"abcdefghABCDEFGH"}"#, &admin);
        assert!(!verify_token(&old, &k, 1).unwrap().tether_proof);
        // The request field likewise: absent when not set, read when sent.
        let e: EnrollRequest = serde_json::from_str(r#"{"uuid":"dev-0001","pubkey":"k","token":"t","tether_id":"1","tether_proof":"c2ln"}"#).unwrap();
        assert_eq!(e.tether_proof.as_deref(), Some("c2ln"));
    }

    /// `s + ℓ`: the same signature, not in canonical form.
    fn non_canonical(sig: &[u8]) -> Vec<u8> {
        const L: [u8; 32] = [0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde, 0x14, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10];
        let mut out = sig.to_vec();
        let mut carry = 0u16;
        for i in 0..32 {
            let v = out[32 + i] as u16 + L[i] as u16 + carry;
            out[32 + i] = v as u8;
            carry = v >> 8;
        }
        out
    }

    #[test]
    fn agent_link_proof() {
        use base64::engine::general_purpose::STANDARD;
        let seed = [7u8; 32];
        let tether = SigningKey::from_bytes(&seed);
        let pk = crate::key_to_b64(&tether.verifying_key());
        let agent = crate::key_to_b64(&crate::generate_key().verifying_key());
        assert_eq!(agent_link_message("123456789", "dev-0001", &agent), format!("tether-agent-link-v1\n123456789\ndev-0001\n{agent}").into_bytes());
        let sig = sign_agent_link(&seed, "123456789", "dev-0001", &agent);
        assert_eq!(STANDARD.decode(&sig).unwrap().len(), 64);
        verify_agent_link(&pk, &sig, "123456789", "dev-0001", &agent).unwrap();
        let other = crate::key_to_b64(&crate::generate_key().verifying_key());
        for (id, uuid, key) in [("123456780", "dev-0001", agent.as_str()), ("123456789", "dev-0002", agent.as_str()), ("123456789", "dev-0001", other.as_str())] {
            assert!(verify_agent_link(&pk, &sig, id, uuid, key).is_err(), "{id} {uuid} {key}");
        }
        assert!(verify_agent_link(&other, &sig, "123456789", "dev-0001", &agent).is_err(), "another Tether key");
        let mut flipped = STANDARD.decode(&sig).unwrap();
        flipped[10] ^= 1;
        assert!(verify_agent_link(&pk, &STANDARD.encode(&flipped), "123456789", "dev-0001", &agent).is_err());
        let raw = STANDARD.decode(&sig).unwrap();
        assert!(verify_agent_link(&pk, &STANDARD.encode(non_canonical(&raw)), "123456789", "dev-0001", &agent).is_err(), "non-canonical s");
        assert!(verify_agent_link(&pk, "not base64!", "123456789", "dev-0001", &agent).is_err());
        assert!(verify_agent_link(&pk, &STANDARD.encode(&raw[..63]), "123456789", "dev-0001", &agent).is_err());
        // A weak (small-order) key with the signature that fits every message under it.
        let mut weak_sig = [0u8; 64];
        weak_sig[0] = 1;
        let weak = "AQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
        assert!(verify_agent_link(weak, &STANDARD.encode(weak_sig), "123456789", "dev-0001", &agent).is_err());
        // The core holds the identity key as a 64-byte sodium secret key (seed, then public key):
        // the same key, and the same (deterministic) signature, as from the seed.
        let mut sodium = [0u8; 64];
        sodium[..32].copy_from_slice(&seed);
        sodium[32..].copy_from_slice(tether.verifying_key().as_bytes());
        let from_sodium = SigningKey::from_keypair_bytes(&sodium).unwrap();
        assert_eq!(crate::key_to_b64(&from_sodium.verifying_key()), pk);
        let sodium_sig = ed25519_dalek::Signer::sign(&from_sodium, &agent_link_message("123456789", "dev-0001", &agent));
        assert_eq!(STANDARD.encode(sodium_sig.to_bytes()), sig);
    }

    #[test]
    fn report_shapes() {
        let full = r#"{"v":1,"uuid":"dev-0001","tether_id":"123456789","host":"PC-1","seq":5,"time":"2026-11-23T08:15:00Z","next":300,
          "agent":"0.1.0","packs":{"system":9},"overall":"crit","counts":{"ok":31,"warn":2,"crit":1,"unknown":3,"setup":1},
          "faults":[{"id":"app/db-service","state":"crit","since":"2026-11-23T07:55:00Z"}],
          "products":[{"name":"Example App","version":"26.3.2","role":"server"}],
          "changes":[{"id":"app/db-service","from":"ok","to":"crit","time":"2026-11-23T07:55:00Z"}],"body":{"checks":[]}}"#;
        let r: Report = serde_json::from_str(full).unwrap();
        assert!(r.validate().is_ok() && r.is_full());
        let hb: serde_json::Value = serde_json::from_str(full).unwrap();
        let mut hb = hb.as_object().unwrap().clone();
        for k in ["products", "changes", "body"] {
            hb.remove(k);
        }
        let h: Report = serde_json::from_value(hb.clone().into()).unwrap();
        assert!(h.validate().is_ok() && !h.is_full());
        hb.insert("body".into(), serde_json::json!({}));
        assert!(serde_json::from_value::<Report>(hb.into()).unwrap().validate().unwrap_err().contains("products, changes and body"));
        for bad in [("\"v\":1", "\"v\":2"), ("\"crit\",\"counts\"", "\"bad\",\"counts\""), ("\"next\":300", "\"next\":59"), ("\"role\":\"server\"", "\"role\":\"root\"")] {
            let r: Report = serde_json::from_str(&full.replace(bad.0, bad.1)).unwrap();
            assert!(r.validate().is_err(), "{bad:?}");
        }
        let mut r: Report = serde_json::from_str(full).unwrap();
        r.faults = vec![r.faults[0].clone(); 51];
        assert!(r.validate().is_err(), "faults capped at 50");
        assert!(serde_json::from_str::<Report>(&full.replace("\"v\":1", "\"v\":1,\"x\":1")).is_err());
    }
}

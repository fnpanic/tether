//! Request signatures (management app, technician app → relay API).
//!
//! Header: `Tether-Sig: v2 host=<relay host> ts=<unix secs> nonce=<16-64 [A-Za-z0-9]> sig=<b64>`
//! Signed message: `"tether-admin-v2\n" + host + "\n" + METHOD + "\n" + path + "\n" + ts + "\n" + nonce + "\n" + hex(sha256(body))`
//! `host` is the relay the request is for ([`host_of`]), so it is worth nothing at another relay.
//! v1 (older apps) has no `host=` and signs `"tether-admin-v1\n" + METHOD + "\n" + path + ...`: a
//! relay accepts it until that is switched off ([`Relay::v1`]).
//! Device requests (device-status agent → relay) use the v2 layout under their own header version
//! and prefix, so a device key never passes as an admin or member key and the reverse:
//! `Tether-Sig: device-v1 uuid=<uuid> host=<relay host> ts=<unix secs> nonce=<…> sig=<b64>`, signed
//! `"tether-device-v1\n" + host + "\n" + METHOD + ...` as v2. The uuid only selects the key.
//! A member header (v1 or v2) may end with ` kid=<16 hex>` ([`crate::team::key_id`]), not signed:
//! the relay then checks only that roster key ([`sign_request_for_kid`], [`header_kid`]).
//! The path includes the query string. The server allows `MAX_SKEW_SECS` clock skew and must
//! reject a nonce it has seen within the last `NONCE_TTL_SECS`.

use crate::b64;
use base64::Engine;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};

pub const HEADER: &str = "Tether-Sig";
pub const MAX_SKEW_SECS: u64 = 300;
/// Longer than the whole window (ts - skew ..= ts + skew) in which a signature is accepted.
pub const NONCE_TTL_SECS: u64 = 2 * MAX_SKEW_SECS + 1;
/// Header version of a device signature.
pub const DEVICE_VERSION: &str = "device-v1";
const V1_PREFIX: &str = "tether-admin-v1";
const V2_PREFIX: &str = "tether-admin-v2";
const DEVICE_PREFIX: &str = "tether-device-v1";

/// What a relay accepts: v2 signatures made for one of its own public host names (none known:
/// no v2), and v1 signatures (not bound to a relay) as long as `v1` is set.
#[derive(Debug, Clone, Copy)]
pub struct Relay<'a> {
    pub hosts: &'a [String],
    pub v1: bool,
}

/// The host a v2 signature names: of a base URL or `host[:port]`, lower-case, without scheme,
/// port, path and a trailing dot; an IPv6 address in brackets. Empty (no relay accepts that) for
/// anything a URL parser could read as another host: credentials, `\`, `%`, spaces, other
/// characters that are not part of a plain host name.
pub fn host_of(url: &str) -> String {
    let s = url.trim();
    let s = match s.split_once("://") {
        Some((scheme, rest)) if scheme.bytes().all(|b| b.is_ascii_alphanumeric()) => rest,
        _ => s,
    };
    let s = s.split(['/', '?', '#']).next().unwrap_or(s);
    let (host, port) = match s.rfind(':') {
        // A bare IPv6 address (`RELAY_HOST=2001:db8::1`).
        Some(_) if !s.starts_with('[') && s.matches(':').count() > 1 => (format!("[{s}]"), ""),
        Some(i) if !s[i..].contains(']') => (s[..i].to_owned(), &s[i + 1..]),
        _ => (s.to_owned(), ""),
    };
    let host = host.strip_suffix('.').unwrap_or(&host).to_ascii_lowercase();
    let v6 = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).is_some_and(|h| !h.is_empty() && h.bytes().all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.'));
    let name = host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'));
    if (v6 || name) && port.bytes().all(|b| b.is_ascii_digit()) {
        host
    } else {
        String::new()
    }
}

/// Hex SHA-256 of a request body, as signed.
pub fn body_digest(body: &[u8]) -> String {
    Sha256::digest(body).iter().map(|b| format!("{b:02x}")).collect()
}

fn message(prefix: &str, host: Option<&str>, method: &str, path: &str, ts: u64, nonce: &str, digest: &str) -> Vec<u8> {
    let head = match host {
        Some(h) => format!("{prefix}\n{h}"),
        None => prefix.to_owned(),
    };
    format!("{head}\n{}\n{path}\n{ts}\n{nonce}\n{digest}", method.to_ascii_uppercase()).into_bytes()
}

fn new_nonce() -> String {
    let mut n = [0u8; 16];
    rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut n);
    n.iter().map(|b| format!("{b:02x}")).collect()
}

/// Build the v1 header value for a request (not bound to a relay: use [`sign_request_for`]).
pub fn sign_request(key: &SigningKey, method: &str, path: &str, body: &[u8], now: u64) -> String {
    sign_request_with_nonce(key, method, path, body, now, &new_nonce())
}

pub fn sign_request_with_nonce(key: &SigningKey, method: &str, path: &str, body: &[u8], now: u64, nonce: &str) -> String {
    let sig = key.sign(&message(V1_PREFIX, None, method, path, now, nonce, &body_digest(body)));
    format!("v1 ts={now} nonce={nonce} sig={}", b64().encode(sig.to_bytes()))
}

/// Build the v2 header value for a request to `relay` (its base URL or host name).
pub fn sign_request_for(key: &SigningKey, relay: &str, method: &str, path: &str, body: &[u8], now: u64) -> String {
    let (host, nonce) = (host_of(relay), new_nonce());
    let sig = key.sign(&message(V2_PREFIX, Some(&host), method, path, now, &nonce, &body_digest(body)));
    format!("v2 host={host} ts={now} nonce={nonce} sig={}", b64().encode(sig.to_bytes()))
}

/// [`sign_request_for`] with ` kid=<team::key_id>` appended: tells a relay of 0.7 or later which
/// member key to check (an older relay refuses the header as malformed). Not signed: it only
/// selects the key the signature is then verified with.
pub fn sign_request_for_kid(key: &SigningKey, relay: &str, method: &str, path: &str, body: &[u8], now: u64) -> String {
    format!("{} kid={}", sign_request_for(key, relay, method, path, body, now), crate::team::key_id(&key.verifying_key()))
}

/// The `kid=` of a v1 or v2 header without verifying anything (key lookup before the check).
pub fn header_kid(header: &str) -> Option<&str> {
    let mut parts = header.split_whitespace();
    if !matches!(parts.next(), Some("v1" | "v2")) {
        return None;
    }
    parts.find_map(|p| p.strip_prefix("kid="))
}

fn valid_kid(k: &str) -> bool {
    k.len() == 16 && k.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Verify a v1 header. `nonce_unused` must return true and remember the nonce if it was not seen
/// before (replay protection); it is called only after the signature checks out.
pub fn verify_request(
    header: &str,
    key: &VerifyingKey,
    method: &str,
    path: &str,
    body: &[u8],
    now: u64,
    nonce_unused: impl FnOnce(&str) -> bool,
) -> Result<(), String> {
    verify_request_for(header, key, Relay { hosts: &[], v1: true }, method, path, &body_digest(body), now, nonce_unused)
}

/// Verify a v1 or v2 header as `relay`, with the body's [`body_digest`] (hashed once, however
/// many keys are tried). `nonce_unused` as for [`verify_request`].
#[allow(clippy::too_many_arguments)]
pub fn verify_request_for(
    header: &str,
    key: &VerifyingKey,
    relay: Relay,
    method: &str,
    path: &str,
    digest: &str,
    now: u64,
    nonce_unused: impl FnOnce(&str) -> bool,
) -> Result<(), String> {
    let mut parts = header.split_whitespace();
    let v2 = match parts.next() {
        Some("v2") => true,
        Some("v1") if relay.v1 => false,
        Some("v1") => return Err("v1 signatures are switched off on this relay: update the app".into()),
        _ => return Err("unsupported signature version".into()),
    };
    let (mut host, mut ts, mut nonce, mut sig) = (None, None, None, None);
    for p in parts {
        match p.split_once('=') {
            Some(("host", v)) if v2 => host = Some(v),
            Some(("ts", v)) => ts = v.parse::<u64>().ok(),
            Some(("nonce", v)) => nonce = Some(v),
            Some(("sig", v)) => sig = Some(v),
            // Selects the key (the caller's lookup, `header_kid`); not part of the signed message.
            Some(("kid", v)) if valid_kid(v) => {}
            _ => return Err("malformed signature header".into()),
        }
    }
    let (ts, nonce, sig) = match (ts, nonce, sig) {
        (Some(a), Some(b), Some(c)) if host.is_some() == v2 => (a, b, c),
        _ => return Err("incomplete signature header".into()),
    };
    let prefix = if v2 { V2_PREFIX } else { V1_PREFIX };
    check(prefix, host, relay.hosts, ts, nonce, sig, key, method, path, digest, now, nonce_unused)
}

/// Host, nonce, skew, signature, replay: the checks after parsing, shared by every header version.
#[allow(clippy::too_many_arguments)]
fn check(
    prefix: &str,
    host: Option<&str>,
    hosts: &[String],
    ts: u64,
    nonce: &str,
    sig: &str,
    key: &VerifyingKey,
    method: &str,
    path: &str,
    digest: &str,
    now: u64,
    nonce_unused: impl FnOnce(&str) -> bool,
) -> Result<(), String> {
    if let Some(h) = host {
        // What `sign_request_for` writes for a relay address `host_of` does not accept.
        if h.is_empty() {
            return Err("malformed signature header".into());
        }
        if hosts.is_empty() {
            return Err("this relay does not know its own host name (RELAY_HOST), so it cannot accept v2 signatures".into());
        }
        if !hosts.iter().any(|own| own == h) {
            return Err("request signed for another relay".into());
        }
    }
    if !(16..=64).contains(&nonce.len()) || !nonce.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err("bad nonce".into());
    }
    if now.abs_diff(ts) > MAX_SKEW_SECS {
        return Err("request timestamp outside allowed clock skew".into());
    }
    let sig = b64().decode(sig).map_err(|_| "bad signature encoding".to_owned())?;
    let sig = Signature::from_slice(&sig).map_err(|_| "bad signature".to_owned())?;
    // Strict: a weak (small-order) key or a non-canonical signature never verifies.
    key.verify_strict(&message(prefix, host, method, path, ts, nonce, digest), &sig)
        .map_err(|_| "signature does not match".to_owned())?;
    if !nonce_unused(nonce) {
        return Err("replayed request".into());
    }
    Ok(())
}

/// Build the device header value for a request of device `uuid` to `relay` (its base URL or host
/// name): `device-v1 uuid=<uuid> host=<host> ts=<ts> nonce=<nonce> sig=<b64>`.
pub fn sign_device_request(key: &SigningKey, uuid: &str, relay: &str, method: &str, path: &str, body: &[u8], now: u64) -> String {
    let (host, nonce) = (host_of(relay), new_nonce());
    let sig = key.sign(&message(DEVICE_PREFIX, Some(&host), method, path, now, &nonce, &body_digest(body)));
    format!("{DEVICE_VERSION} uuid={uuid} host={host} ts={now} nonce={nonce} sig={}", b64().encode(sig.to_bytes()))
}

/// The uuid of a device header without verifying anything (key lookup before the signature check).
pub fn device_uuid(header: &str) -> Option<&str> {
    let mut parts = header.split_whitespace();
    if parts.next() != Some(DEVICE_VERSION) {
        return None;
    }
    parts.find_map(|p| p.strip_prefix("uuid=")).filter(|u| !u.is_empty())
}

/// Verify a device header as a relay known as `hosts`, with the body's [`body_digest`].
/// `nonce_unused` as for [`verify_request`]. Admin and member (`v1`, `v2`) headers are refused.
#[allow(clippy::too_many_arguments)]
pub fn verify_device_request(
    header: &str,
    key: &VerifyingKey,
    hosts: &[String],
    method: &str,
    path: &str,
    digest: &str,
    now: u64,
    nonce_unused: impl FnOnce(&str) -> bool,
) -> Result<(), String> {
    let mut parts = header.split_whitespace();
    if parts.next() != Some(DEVICE_VERSION) {
        return Err("unsupported signature version".into());
    }
    let (mut uuid, mut host, mut ts, mut nonce, mut sig) = (None, None, None, None, None);
    for p in parts {
        match p.split_once('=') {
            Some(("uuid", v)) => uuid = Some(v),
            Some(("host", v)) => host = Some(v),
            Some(("ts", v)) => ts = v.parse::<u64>().ok(),
            Some(("nonce", v)) => nonce = Some(v),
            Some(("sig", v)) => sig = Some(v),
            _ => return Err("malformed signature header".into()),
        }
    }
    let (Some(_), Some(host), Some(ts), Some(nonce), Some(sig)) = (uuid, host, ts, nonce, sig) else {
        return Err("incomplete signature header".into());
    };
    check(DEVICE_PREFIX, Some(host), hosts, ts, nonce, sig, key, method, path, digest, now, nonce_unused)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_rejects_tamper_skew_replay() {
        let k = crate::generate_key();
        let pk = k.verifying_key();
        let h = sign_request(&k, "PUT", "/api/admin/policy", b"body", 1_000_000);
        assert!(verify_request(&h, &pk, "put", "/api/admin/policy", b"body", 1_000_100, |_| true).is_ok());
        assert!(verify_request(&h, &pk, "PUT", "/api/admin/policy", b"bodX", 1_000_000, |_| true).is_err());
        assert!(verify_request(&h, &pk, "PUT", "/api/admin/devices", b"body", 1_000_000, |_| true).is_err());
        assert!(verify_request(&h, &pk, "PUT", "/api/admin/policy", b"body", 1_000_400, |_| true).is_err());
        assert!(verify_request(&h, &pk, "PUT", "/api/admin/policy", b"body", 1_000_000, |_| false).is_err());
        let other = crate::generate_key().verifying_key();
        assert!(verify_request(&h, &other, "PUT", "/api/admin/policy", b"body", 1_000_000, |_| true).is_err());
    }

    #[test]
    fn v2_is_bound_to_the_relay() {
        let k = crate::generate_key();
        let pk = k.verifying_key();
        let d = body_digest(b"body");
        let hosts = ["relay-a.example.com".to_owned(), "alias.example.com".to_owned()];
        let (a, a_only) = (Relay { hosts: &hosts, v1: true }, Relay { hosts: &hosts, v1: false });
        let other = ["relay-b.example.com".to_owned()];
        let b = Relay { hosts: &other, v1: true };
        let check = |h: &str, r: Relay| verify_request_for(h, &pk, r, "PUT", "/api/admin/policy", &d, 1_000_000, |_| true);

        let h = sign_request_for(&k, "https://Relay-A.example.com:443/", "PUT", "/api/admin/policy", b"body", 1_000_000);
        assert!(h.starts_with("v2 host=relay-a.example.com ts="), "{h}");
        assert!(check(&h, a).is_ok() && check(&h, a_only).is_ok());
        assert_eq!(check(&h, b).unwrap_err(), "request signed for another relay");
        // Naming the other relay in the header does not help: the host is signed.
        assert_eq!(check(&h.replace("relay-a", "relay-b"), b).unwrap_err(), "signature does not match");
        // A relay that does not know its name accepts no v2 (and `verify_request` is v1 only).
        assert!(check(&h, Relay { hosts: &[], v1: true }).unwrap_err().contains("own host name"));
        assert!(verify_request(&h, &pk, "PUT", "/api/admin/policy", b"body", 1_000_000, |_| true).is_err());
        // A v2 signature passed off as v1 (and the reverse): the signed prefix differs.
        assert_eq!(check(&h.replace("v2 host=relay-a.example.com ", "v1 "), a).unwrap_err(), "signature does not match");
        let v1 = sign_request(&k, "PUT", "/api/admin/policy", b"body", 1_000_000);
        assert_eq!(check(&v1.replace("v1 ", "v2 host=relay-a.example.com "), a).unwrap_err(), "signature does not match");
        // v1 passes until it is switched off.
        assert!(check(&v1, a).is_ok());
        assert!(check(&v1, a_only).unwrap_err().contains("switched off"));
        assert!(check(&h.replace("host=relay-a.example.com ", ""), a).is_err(), "v2 needs a host");
    }

    #[test]
    fn device_and_admin_signatures_do_not_cross() {
        let k = crate::generate_key();
        let pk = k.verifying_key();
        let hosts = ["relay-a.example.com".to_owned()];
        let d = body_digest(b"{}");
        let h = sign_device_request(&k, "dev-0001", "https://relay-a.example.com", "POST", "/api/status", b"{}", 1_000_000);
        assert!(h.starts_with("device-v1 uuid=dev-0001 host=relay-a.example.com ts=1000000 nonce="), "{h}");
        assert_eq!(device_uuid(&h), Some("dev-0001"));
        assert!(verify_device_request(&h, &pk, &hosts, "post", "/api/status", &d, 1_000_299, |_| true).is_ok());
        assert_eq!(verify_device_request(&h, &pk, &hosts, "POST", "/api/status", &d, 1_000_301, |_| true).unwrap_err(), "request timestamp outside allowed clock skew");
        assert_eq!(verify_device_request(&h, &pk, &hosts, "POST", "/api/status", &d, 1_000_000, |_| false).unwrap_err(), "replayed request");
        // A device header is no admin header, and an admin v2 header made with the same key is no device header.
        let r = Relay { hosts: &hosts, v1: true };
        assert_eq!(verify_request_for(&h, &pk, r, "POST", "/api/status", &d, 1_000_000, |_| true).unwrap_err(), "unsupported signature version");
        let v2 = sign_request_for(&k, "https://relay-a.example.com", "POST", "/api/status", b"{}", 1_000_000);
        let forged = v2.replacen("v2 ", "device-v1 uuid=dev-0001 ", 1);
        assert_eq!(verify_device_request(&forged, &pk, &hosts, "POST", "/api/status", &d, 1_000_000, |_| true).unwrap_err(), "signature does not match");
        assert!(verify_device_request(&h, &pk, &["relay-b.example.com".to_owned()], "POST", "/api/status", &d, 1_000_000, |_| true).unwrap_err().contains("another relay"));
        assert!(device_uuid("v2 host=x ts=1 nonce=aaaaaaaaaaaaaaaa sig=x").is_none());
        // Admin headers on the device path, a header without uuid, a relay without its host name.
        assert_eq!(verify_device_request(&v2, &pk, &hosts, "POST", "/api/status", &d, 1_000_000, |_| true).unwrap_err(), "unsupported signature version");
        assert_eq!(verify_device_request(&h.replacen("uuid=dev-0001 ", "", 1), &pk, &hosts, "POST", "/api/status", &d, 1_000_000, |_| true).unwrap_err(), "incomplete signature header");
        assert!(verify_device_request(&h, &pk, &[], "POST", "/api/status", &d, 1_000_000, |_| true).unwrap_err().contains("RELAY_HOST"));
        assert!(verify_device_request(&h, &pk, &hosts, "POST", "/api/status?x", &d, 1_000_000, |_| true).is_err());
    }

    #[test]
    fn host_of_takes_the_bare_host() {
        for (url, host) in [
            ("https://Relay.Example.com", "relay.example.com"),
            ("https://relay.example.com:8443/base/", "relay.example.com"),
            ("relay.example.com:443", "relay.example.com"),
            ("http://127.0.0.1:21120", "127.0.0.1"),
            ("https://relay.example.com./x?y#z", "relay.example.com"),
            (" relay.example.com ", "relay.example.com"),
            ("http://[::1]:8080/", "[::1]"),
            ("[2001:DB8::1]", "[2001:db8::1]"),
            ("2001:db8::1", "[2001:db8::1]"),
            // Where a URL parser may go to another host than the one read here: no host at all.
            (r"https://evil.com\@relay.example.com", ""),
            ("https://relay.example.com@evil.com", ""),
            ("https://user:pw@relay.example.com", ""),
            ("https://relay.example.com:443@evil.com/", ""),
            ("https://relay.example.com%2eevil.com", ""),
            ("https://relay .example.com", ""),
            ("https://rel\tay.example.com", ""),
            ("https://relay.example.com:80x", ""),
            ("https://[::1", ""),
            ("https://[]", ""),
            ("https://", ""),
            ("", ""),
        ] {
            assert_eq!(host_of(url), host, "{url:?}");
        }
    }

    #[test]
    fn empty_host_never_verifies() {
        // A relay address `host_of` refuses: the signature is for no relay, whatever one accepts.
        let k = crate::generate_key();
        let h = sign_request_for(&k, r"https://evil.com\@relay.example.com", "GET", "/api/admin/status", b"", 1_000_000);
        assert!(h.starts_with("v2 host= ts="), "{h}");
        for hosts in [vec![], vec![String::new()], vec!["relay.example.com".to_owned(), "evil.com".to_owned()]] {
            let r = verify_request_for(&h, &k.verifying_key(), Relay { hosts: &hosts, v1: true }, "GET", "/api/admin/status", &body_digest(b""), 1_000_000, |_| true);
            assert!(r.is_err(), "{hosts:?}");
        }
    }

    #[test]
    fn kid_names_the_key_and_is_not_signed() {
        let k = crate::generate_key();
        let pk = k.verifying_key();
        let hosts = ["relay-a.example.com".to_owned()];
        let r = Relay { hosts: &hosts, v1: true };
        let d = body_digest(b"");
        let h = sign_request_for_kid(&k, "https://relay-a.example.com", "GET", "/api/team/me", b"", 1_000_000);
        let kid = crate::team::key_id(&pk);
        assert!(h.starts_with("v2 host=relay-a.example.com ts=1000000 nonce=") && h.ends_with(&format!(" kid={kid}")), "{h}");
        assert_eq!(header_kid(&h), Some(kid.as_str()));
        assert!(verify_request_for(&h, &pk, r, "GET", "/api/team/me", &d, 1_000_000, |_| true).is_ok());
        // Only a selector: changing it does not break the signature, it only names another key.
        let other = h.replace(&kid, "0123456789abcdef");
        assert_eq!(header_kid(&other), Some("0123456789abcdef"));
        assert!(verify_request_for(&other, &pk, r, "GET", "/api/team/me", &d, 1_000_000, |_| true).is_ok());
        assert!(verify_request_for(&h, &crate::generate_key().verifying_key(), r, "GET", "/api/team/me", &d, 1_000_000, |_| true).is_err());
        // Old headers have none; a malformed one is refused, not ignored.
        assert_eq!(header_kid(&sign_request_for(&k, "relay-a.example.com", "GET", "/", b"", 1)), None);
        assert_eq!(header_kid("device-v1 uuid=x kid=0123456789abcdef"), None);
        for bad in ["kid=0123", "kid=0123456789abcdeg", "kid=", "kid=0123456789abcdef0"] {
            let h = format!("{} {bad}", sign_request_for(&k, "relay-a.example.com", "GET", "/api/team/me", b"", 1_000_000));
            assert_eq!(verify_request_for(&h, &pk, r, "GET", "/api/team/me", &d, 1_000_000, |_| true).unwrap_err(), "malformed signature header", "{bad}");
        }
        let v1 = format!("{} kid={kid}", sign_request(&k, "GET", "/api/team/me", b"", 1_000_000));
        assert!(verify_request(&v1, &pk, "GET", "/api/team/me", b"", 1_000_000, |_| true).is_ok(), "v1 may carry one too");
    }

    #[test]
    fn weak_key_verifies_nothing() {
        // With the identity point as key, this fixed signature fits every message unless strict.
        let mut b = [0u8; 32];
        b[0] = 1;
        let weak = VerifyingKey::from_bytes(&b).unwrap();
        let mut sig = [0u8; 64];
        sig[0] = 1;
        let h = format!("v1 ts=1000000 nonce=aaaaaaaaaaaaaaaa sig={}", b64().encode(sig));
        assert_eq!(verify_request(&h, &weak, "GET", "/api/team/me", b"", 1_000_000, |_| true).unwrap_err(), "signature does not match");
    }
}

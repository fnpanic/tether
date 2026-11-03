//! Admin request signatures (management app → relay API).
//!
//! Header: `Tether-Sig: v1 ts=<unix secs> nonce=<16-64 [A-Za-z0-9]> sig=<b64>`
//! Signed message: `"tether-admin-v1\n" + METHOD + "\n" + path + "\n" + ts + "\n" + nonce + "\n" + hex(sha256(body))`
//! The path includes the query string. The server allows `MAX_SKEW_SECS` clock skew and must
//! reject a nonce it has seen within the last `NONCE_TTL_SECS`.

use crate::b64;
use base64::Engine;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

pub const HEADER: &str = "Tether-Sig";
pub const MAX_SKEW_SECS: u64 = 300;
pub const NONCE_TTL_SECS: u64 = 600;

fn message(method: &str, path: &str, ts: u64, nonce: &str, body: &[u8]) -> Vec<u8> {
    let digest: String = Sha256::digest(body).iter().map(|b| format!("{b:02x}")).collect();
    format!("tether-admin-v1\n{}\n{path}\n{ts}\n{nonce}\n{digest}", method.to_ascii_uppercase()).into_bytes()
}

/// Build the header value for a request.
pub fn sign_request(key: &SigningKey, method: &str, path: &str, body: &[u8], now: u64) -> String {
    let mut n = [0u8; 16];
    rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut n);
    let nonce: String = n.iter().map(|b| format!("{b:02x}")).collect();
    sign_request_with_nonce(key, method, path, body, now, &nonce)
}

pub fn sign_request_with_nonce(key: &SigningKey, method: &str, path: &str, body: &[u8], now: u64, nonce: &str) -> String {
    let sig = key.sign(&message(method, path, now, nonce, body));
    format!("v1 ts={now} nonce={nonce} sig={}", b64().encode(sig.to_bytes()))
}

/// Verify a header. `nonce_unused` must return true and remember the nonce if it was not seen
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
    let mut parts = header.split_whitespace();
    if parts.next() != Some("v1") {
        return Err("unsupported signature version".into());
    }
    let (mut ts, mut nonce, mut sig) = (None, None, None);
    for p in parts {
        match p.split_once('=') {
            Some(("ts", v)) => ts = v.parse::<u64>().ok(),
            Some(("nonce", v)) => nonce = Some(v),
            Some(("sig", v)) => sig = Some(v),
            _ => return Err("malformed signature header".into()),
        }
    }
    let (ts, nonce, sig) = match (ts, nonce, sig) {
        (Some(a), Some(b), Some(c)) => (a, b, c),
        _ => return Err("incomplete signature header".into()),
    };
    if !(16..=64).contains(&nonce.len()) || !nonce.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err("bad nonce".into());
    }
    if now.abs_diff(ts) > MAX_SKEW_SECS {
        return Err("request timestamp outside allowed clock skew".into());
    }
    let sig = b64().decode(sig).map_err(|_| "bad signature encoding".to_owned())?;
    let sig = Signature::from_slice(&sig).map_err(|_| "bad signature".to_owned())?;
    key.verify(&message(method, path, ts, nonce, body), &sig)
        .map_err(|_| "signature does not match".to_owned())?;
    if !nonce_unused(nonce) {
        return Err("replayed request".into());
    }
    Ok(())
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
}

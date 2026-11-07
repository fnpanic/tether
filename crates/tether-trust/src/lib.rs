//! Trust primitives shared by Tether's technician app, customer module, management app and the
//! relay API (`tether-api`). See `docs/api.md` for the wire contract.
//!
//! - [`envelope`]: Ed25519-signed documents (policy bundles). Signed over the exact payload bytes,
//!   so no JSON canonicalisation is needed.
//! - [`reqauth`]: signed admin requests (management app → relay API).
//! - [`policy`]: policy bundle, feature registry, resolution into effective settings.
//! - [`version`]: version ordering for update enforcement.
//! - [`custom`]: signed custom TetherQS apps (trailer appended to the exe, distribution tokens).
//! - [`release`]: signed release index, rollout, artifact binding (docs/update-design.md).
//! - [`serverkeys`]: signed list of accepted server keys (server key rotation).
//! - [`team`]: team roster (technician keys for `/api/team/*`) and key fingerprints.

pub mod custom;
pub mod envelope;
pub mod policy;
pub mod release;
pub mod reqauth;
pub mod serverkeys;
pub mod team;
pub mod version;

pub use ed25519_dalek::{SigningKey, VerifyingKey};

use base64::{engine::general_purpose::STANDARD as B64, Engine};

/// Base64 (standard, padded) of a public key: the form used in env vars and documents.
pub fn key_to_b64(k: &VerifyingKey) -> String {
    B64.encode(k.as_bytes())
}

/// Parse a base64 Ed25519 public key (`TETHER_ADMIN_PUB_KEY`).
pub fn key_from_b64(s: &str) -> Result<VerifyingKey, String> {
    let raw = B64.decode(s.trim()).map_err(|e| format!("bad key encoding: {e}"))?;
    let arr: [u8; 32] = raw.try_into().map_err(|_| "key must be 32 bytes".to_owned())?;
    VerifyingKey::from_bytes(&arr).map_err(|e| format!("bad key: {e}"))
}

/// A fresh random admin signing key (management app first run).
pub fn generate_key() -> SigningKey {
    SigningKey::generate(&mut rand_core::OsRng)
}

/// Read a response body of at most `limit` bytes: stops after `limit + 1` bytes and fails, so an
/// oversized (or endless) answer never ends up in memory as a whole.
pub fn read_limited(r: impl std::io::Read, limit: usize) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut out = Vec::new();
    r.take(limit as u64 + 1).read_to_end(&mut out).map_err(|e| format!("offline: {e}"))?;
    if out.len() > limit {
        return Err(format!("the answer is larger than {limit} bytes"));
    }
    Ok(out)
}

/// Write via a per-process temp file + rename: readers (and other process roles) never see a
/// half-written file.
pub fn write_atomic(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    std::fs::write(&tmp, data).and_then(|_| std::fs::rename(&tmp, path))
}

pub(crate) fn b64() -> &'static base64::engine::GeneralPurpose {
    &B64
}

#[cfg(test)]
mod tests {
    #[test]
    fn read_limited_stops_after_the_limit() {
        assert_eq!(super::read_limited(&b"abcd"[..], 4).unwrap(), b"abcd");
        assert!(super::read_limited(&b"abcde"[..], 4).is_err());
        // An endless body: only limit + 1 bytes are read before the error.
        assert!(super::read_limited(std::io::repeat(b'x'), 16 * 1024).is_err());
    }
}

//! Trust primitives shared by Tether's technician app, customer module, management app and the
//! relay API (`tether-api`). See `docs/api.md` for the wire contract.
//!
//! - [`envelope`]: Ed25519-signed documents (policy bundles). Signed over the exact payload bytes,
//!   so no JSON canonicalisation is needed.
//! - [`reqauth`]: signed admin requests (management app → relay API).
//! - [`policy`]: policy bundle, feature registry, resolution into effective settings.
//! - [`version`]: version ordering for update enforcement.

pub mod envelope;
pub mod policy;
pub mod reqauth;
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

pub(crate) fn b64() -> &'static base64::engine::GeneralPurpose {
    &B64
}

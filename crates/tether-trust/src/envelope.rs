//! Signed documents: `{"payload": b64(bytes), "sig": b64(ed25519(bytes)), "key": b64(pubkey)}`.

use crate::b64;
use base64::Engine;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Signed {
    pub payload: String,
    pub sig: String,
    pub key: String,
}

pub fn sign(payload: &[u8], key: &SigningKey) -> Signed {
    Signed {
        payload: b64().encode(payload),
        sig: b64().encode(key.sign(payload).to_bytes()),
        key: crate::key_to_b64(&key.verifying_key()),
    }
}

/// Verify against any of the trusted keys and return the payload bytes.
pub fn open(doc: &Signed, trusted: &[VerifyingKey]) -> Result<Vec<u8>, String> {
    let key = crate::key_from_b64(&doc.key)?;
    if !trusted.iter().any(|t| t == &key) {
        return Err("signed by an untrusted key".into());
    }
    let payload = b64().decode(&doc.payload).map_err(|e| format!("bad payload: {e}"))?;
    let sig = b64().decode(&doc.sig).map_err(|e| format!("bad signature: {e}"))?;
    let sig = Signature::from_slice(&sig).map_err(|e| format!("bad signature: {e}"))?;
    key.verify_strict(&payload, &sig).map_err(|_| "signature does not match".to_owned())?;
    Ok(payload)
}

/// Parse a signed JSON document and verify it.
pub fn open_json(raw: &[u8], trusted: &[VerifyingKey]) -> Result<Vec<u8>, String> {
    let doc: Signed = serde_json::from_slice(raw).map_err(|e| format!("not a signed document: {e}"))?;
    open(&doc, trusted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_tamper_and_untrusted_key() {
        let k = crate::generate_key();
        let other = crate::generate_key();
        let doc = sign(b"{\"serial\":1}", &k);
        assert_eq!(open(&doc, &[k.verifying_key()]).unwrap(), b"{\"serial\":1}");
        assert!(open(&doc, &[other.verifying_key()]).is_err());
        let mut bad = doc.clone();
        bad.payload = b64().encode(b"{\"serial\":2}");
        assert!(open(&bad, &[k.verifying_key()]).is_err());
        // Swapping in an attacker key that signs its own payload is still untrusted.
        let forged = sign(b"{\"serial\":9}", &other);
        assert!(open(&forged, &[k.verifying_key()]).is_err());
        // A weak key that got into the trusted list: its fixed signature fits no payload.
        let (mut id, mut sig) = ([0u8; 32], [0u8; 64]);
        (id[0], sig[0]) = (1, 1);
        let weak = VerifyingKey::from_bytes(&id).unwrap();
        let doc = Signed { payload: b64().encode(b"{\"serial\":9}"), sig: b64().encode(sig), key: b64().encode(id) };
        assert!(open(&doc, &[weak]).is_err());
    }
}

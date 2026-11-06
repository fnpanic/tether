//! Signed list of accepted server (hbbs/hbbr) public keys: rotation of the server key without
//! reinstalling clients (docs/key-rotation.md). Signed by the admin key like the policy, published
//! separately (`GET /api/server-keys`) because `policy::Bundle` rejects unknown fields.
//!
//! - trusted keys = the active keys of the latest verified list (without a list: the compiled-in
//!   key only; a list never adds the compiled-in key implicitly, so it can retire it);
//! - presented key (`licence_key`) = the first active key;
//! - serials only go up, so a relay cannot roll a device back to a retired key.

use crate::envelope;
use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Most keys one list may name.
pub const MAX_KEYS: usize = 8;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerKey {
    /// Base64 Ed25519 public key (`/var/lib/tether/id_ed25519.pub`).
    pub key: String,
    /// Unix seconds; 0 = always. Soft: ignored when no key would be active otherwise.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub not_before: u64,
    /// Unix seconds; 0 = never. Strict.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub expires: u64,
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

/// The signed document's payload.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyList {
    pub serial: u64,
    pub issued_at: u64,
    pub keys: Vec<ServerKey>,
}

impl KeyList {
    pub fn validate(&self) -> Result<(), String> {
        if self.keys.is_empty() || self.keys.len() > MAX_KEYS {
            return Err(format!("a server key list needs 1-{MAX_KEYS} keys"));
        }
        for (i, k) in self.keys.iter().enumerate() {
            crate::key_from_b64(&k.key).map_err(|e| format!("server key {}: {e}", i + 1))?;
            if self.keys[..i].iter().any(|o| o.key == k.key) {
                return Err(format!("server key {} is listed twice", i + 1));
            }
            if k.expires != 0 && k.expires <= k.not_before {
                return Err(format!("server key {} expires before it starts", i + 1));
            }
        }
        // A device whose clock runs ahead must never end up without any key.
        if !self.keys.iter().any(|k| k.expires == 0) {
            return Err("at least one server key must not expire".into());
        }
        Ok(())
    }

    pub fn from_json(payload: &[u8]) -> Result<Self, String> {
        let l: KeyList = serde_json::from_slice(payload).map_err(|e| format!("invalid server key list: {e}"))?;
        l.validate()?;
        Ok(l)
    }

    /// Verify a signed document against the admin keys and parse it.
    pub fn open(raw: &[u8], admin: &[VerifyingKey]) -> Result<Self, String> {
        Self::from_json(&envelope::open_json(raw, admin)?)
    }

    /// Keys to use at `now` (unix seconds), in list order: the first one is presented.
    pub fn active(&self, now: u64) -> Vec<String> {
        let live = |k: &&ServerKey| k.expires == 0 || now < k.expires;
        let v: Vec<String> = self.keys.iter().filter(live).filter(|k| now >= k.not_before).map(|k| k.key.clone()).collect();
        if !v.is_empty() {
            return v;
        }
        self.keys.iter().filter(live).map(|k| k.key.clone()).collect()
    }
}

/// Last verified list cached at `path` (the signed document as fetched), if still valid.
pub fn load(path: &Path, admin: &[VerifyingKey]) -> Option<KeyList> {
    KeyList::open(&std::fs::read(path).ok()?, admin).ok()
}

/// A fetched document: verified, validated and stored at `path` when its serial is higher than the
/// cached one. `Ok(None)`: same serial as the cache, nothing changed. A lower serial is an error.
pub fn accept(path: &Path, raw: &[u8], admin: &[VerifyingKey]) -> Result<Option<KeyList>, String> {
    let list = KeyList::open(raw, admin)?;
    if let Some(cached) = load(path, admin).map(|c| c.serial) {
        if list.serial < cached {
            return Err(format!("server key list serial {} is older than the cached {cached}", list.serial));
        }
        if list.serial == cached {
            return Ok(None);
        }
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, raw).and_then(|_| std::fs::rename(&tmp, path)).map_err(|e| format!("cannot cache server key list: {e}"))?;
    Ok(Some(list))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> String {
        crate::key_to_b64(&crate::generate_key().verifying_key())
    }
    fn k(key: &str, not_before: u64, expires: u64) -> ServerKey {
        ServerKey { key: key.into(), not_before, expires }
    }
    fn doc(serial: u64, keys: Vec<ServerKey>, admin: &crate::SigningKey) -> Vec<u8> {
        let l = KeyList { serial, issued_at: 1, keys };
        serde_json::to_vec(&envelope::sign(&serde_json::to_vec(&l).unwrap(), admin)).unwrap()
    }

    #[test]
    fn signature_and_validation() {
        let admin = crate::generate_key();
        let (a, b) = (key(), key());
        let trusted = [admin.verifying_key()];
        let l = KeyList::open(&doc(1, vec![k(&a, 0, 0), k(&b, 0, 0)], &admin), &trusted).unwrap();
        assert_eq!(l.active(5), [a.clone(), b.clone()]);
        // Signed by another key (a relay forging a list): rejected.
        let other = crate::generate_key();
        assert!(KeyList::open(&doc(9, vec![k(&a, 0, 0)], &other), &trusted).is_err());
        // Tampered payload.
        let mut signed: envelope::Signed = serde_json::from_slice(&doc(2, vec![k(&a, 0, 0)], &admin)).unwrap();
        signed.payload = crate::b64().encode(br#"{"serial":2,"issued_at":1,"keys":[{"key":"AAAA"}]}"#);
        assert!(KeyList::open(&serde_json::to_vec(&signed).unwrap(), &trusted).is_err());
        // Invalid contents, even when properly signed.
        for bad in [
            vec![],
            vec![k("not a key", 0, 0)],
            vec![k(&a, 0, 0), k(&a, 0, 0)],
            vec![k(&a, 0, 10)],
            vec![k(&a, 20, 10), k(&b, 0, 0)],
            (0..9).map(|_| k(&key(), 0, 0)).collect(),
        ] {
            assert!(KeyList::open(&doc(3, bad.clone(), &admin), &trusted).is_err(), "{bad:?} accepted");
        }
        let unknown = br#"{"serial":1,"issued_at":0,"keys":[],"extra":1}"#;
        assert!(KeyList::from_json(unknown).is_err());
    }

    use base64::Engine;

    #[test]
    fn expiry_and_not_before() {
        let (a, b) = (key(), key());
        let l = KeyList { serial: 1, issued_at: 0, keys: vec![k(&a, 0, 100), k(&b, 50, 0)] };
        l.validate().unwrap();
        assert_eq!(l.active(10), [a.clone()], "b not yet active, a presented");
        assert_eq!(l.active(60), [a.clone(), b.clone()]);
        assert_eq!(l.active(100), [b.clone()], "a expired: b presented");
        // Clock far behind: nothing active, not_before is ignored, expiry is not.
        let l = KeyList { serial: 1, issued_at: 0, keys: vec![k(&b, 50, 0)] };
        assert_eq!(l.active(10), [b.clone()]);
    }

    #[test]
    fn cache_and_monotonic_serial() {
        let admin = crate::generate_key();
        let trusted = [admin.verifying_key()];
        let dir = std::env::temp_dir().join(format!("tether-serverkeys-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("server-keys.json");
        let (a, b) = (key(), key());
        assert!(load(&path, &trusted).is_none());
        let l1 = accept(&path, &doc(4, vec![k(&a, 0, 0), k(&b, 0, 0)], &admin), &trusted).unwrap().unwrap();
        assert_eq!(load(&path, &trusted).unwrap(), l1, "survives a restart");
        assert!(accept(&path, &doc(4, vec![k(&a, 0, 0), k(&b, 0, 0)], &admin), &trusted).unwrap().is_none());
        // Rollback to an older list (e.g. one that still trusted a retired key): refused, cache kept.
        assert!(accept(&path, &doc(3, vec![k(&a, 0, 0)], &admin), &trusted).is_err());
        let l2 = accept(&path, &doc(5, vec![k(&b, 0, 0)], &admin), &trusted).unwrap().unwrap();
        assert_eq!(load(&path, &trusted).unwrap(), l2);
        // A forged document never reaches the cache.
        let other = crate::generate_key();
        assert!(accept(&path, &doc(99, vec![k(&a, 0, 0)], &other), &trusted).is_err());
        assert_eq!(load(&path, &trusted).unwrap().serial, 5);
        // A tampered cache file is ignored (the app then trusts the compiled-in key only).
        std::fs::write(&path, b"{\"payload\":\"x\",\"sig\":\"y\",\"key\":\"z\"}").unwrap();
        assert!(load(&path, &trusted).is_none());
        // Cached under a different admin key (new build): ignored, so the next fetch starts over.
        std::fs::write(&path, doc(7, vec![k(&a, 0, 0)], &other)).unwrap();
        assert!(load(&path, &trusted).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

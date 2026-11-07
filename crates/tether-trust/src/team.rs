//! Team roster: the technicians (by their own Ed25519 public key) an admin lets read the org's
//! device list and shared tags on a relay (`/api/team/*`, docs/api.md). Members sign their requests
//! with `reqauth` exactly like admin requests.

use crate::key_from_b64;
use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_MEMBERS: usize = 500;

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

impl Roster {
    /// Checks the roster and returns the members' keys in order.
    pub fn validate(&self) -> Result<Vec<VerifyingKey>, String> {
        if self.members.len() > MAX_MEMBERS {
            return Err(format!("at most {MAX_MEMBERS} members"));
        }
        let mut keys: Vec<VerifyingKey> = Vec::with_capacity(self.members.len());
        for m in &self.members {
            let n = m.name.chars().count();
            if m.name.trim() != m.name || !(1..=64).contains(&n) || m.name.chars().any(char::is_control) {
                return Err(format!("member name {:?}: 1-64 characters, no control characters or surrounding spaces", m.name));
            }
            let k = key_from_b64(&m.key).map_err(|e| format!("member {}: {e}", m.name))?;
            if keys.contains(&k) {
                return Err(format!("member {}: key is listed twice", m.name));
            }
            keys.push(k);
        }
        Ok(keys)
    }
}

/// `hex(sha256(key))`, first 16 hex digits, uppercase, in groups of 4: `9F3A-12B0-C4D1-77E2`.
pub fn fingerprint(key: &VerifyingKey) -> String {
    let d = Sha256::digest(key.as_bytes());
    d[..8].chunks(2).map(|c| format!("{:02X}{:02X}", c[0], c[1])).collect::<Vec<_>>().join("-")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{generate_key, key_to_b64};

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
        let many = (0..=MAX_MEMBERS).map(|i| member(&format!("m{i}"), key_to_b64(&generate_key().verifying_key()))).collect();
        assert!(Roster { members: many }.validate().is_err());
        assert!(serde_json::from_str::<Roster>(r#"{"members":[],"x":1}"#).is_err());
        let extra = format!(r#"{{"members":[{{"name":"A","key":"{a}","write":true,"audit":false,"admin":true}}]}}"#);
        assert!(serde_json::from_str::<Roster>(&extra).is_err());
    }
}

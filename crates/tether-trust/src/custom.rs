//! Custom TetherQS apps: an admin-signed profile (name, logo, colours, settings) that the relay
//! appends to the plain `TetherQS.exe` when it is downloaded through a private, expiring link.
//!
//! Wire format of the appended trailer (end of the exe file):
//!   `<signed profile JSON (envelope::Signed of CustomProfile)>` `<u32 LE length of it>` `MAGIC`
//! TetherQS reads its own file, verifies the signature against the compiled-in admin keys and
//! applies the profile; a missing, damaged or untrusted trailer means "plain TetherQS".
//! See docs/api.md ("Custom apps").

use crate::{envelope, policy::Layer};
use serde::{Deserialize, Serialize};

/// Last 8 bytes of a customised TetherQS.exe.
pub const MAGIC: &[u8; 8] = b"TQSPROF1";
/// Upper bound for the signed profile (logo included).
pub const MAX_PROFILE_BYTES: usize = 256 * 1024;
/// Upper bound for the decoded logo (PNG).
pub const MAX_LOGO_BYTES: usize = 96 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomProfile {
    /// Random id chosen by Tether Manage (`[a-z0-9-]{1,40}`); the same on every relay.
    pub id: String,
    /// Shown as the window title / company name (overrides `customer.title`).
    pub name: String,
    /// `#rrggbb` (overrides `customer.accent-color`); empty = policy/default.
    #[serde(default)]
    pub accent: String,
    /// Welcome text (overrides `customer.welcome`); empty = policy/default.
    #[serde(default)]
    pub welcome: String,
    /// Base64 (standard) PNG, at most [`MAX_LOGO_BYTES`] decoded; empty = Tether logo.
    #[serde(default)]
    pub logo_png: String,
    /// Settings for everyone using this download (customer-profile features only).
    #[serde(default)]
    pub layer: Layer,
    /// Unix seconds.
    pub issued_at: u64,
}

impl CustomProfile {
    /// Strict validation (also done by the relay before storing and by TetherQS before applying).
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty() || self.id.len() > 40 || !self.id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
            return Err("custom app id must be 1-40 characters [a-z0-9-]".into());
        }
        if self.name.trim().is_empty() || self.name.len() > 64 || self.name.chars().any(char::is_control) {
            return Err("custom app name must be 1-64 printable characters".into());
        }
        if !self.accent.is_empty() && !(self.accent.len() == 7 && self.accent.starts_with('#') && self.accent[1..].bytes().all(|b| b.is_ascii_hexdigit())) {
            return Err("accent colour must look like #0071e3".into());
        }
        if self.welcome.len() > 400 || self.welcome.chars().any(|c| c.is_control() && c != '\n') {
            return Err("welcome text: at most 400 characters".into());
        }
        if !self.logo_png.is_empty() {
            use base64::Engine;
            let png = crate::b64().decode(&self.logo_png).map_err(|_| "logo is not valid base64".to_owned())?;
            if png.len() > MAX_LOGO_BYTES || !png.starts_with(b"\x89PNG\r\n\x1a\n") {
                return Err(format!("logo must be a PNG of at most {} KiB", MAX_LOGO_BYTES / 1024));
            }
        }
        // Only customer-profile features, same value rules as the policy bundle.
        let probe = crate::policy::Bundle {
            serial: 1,
            profiles: [(crate::policy::PROFILE_CUSTOMER.to_owned(), self.layer.clone())].into(),
            ..Default::default()
        };
        probe.validate().map_err(|e| format!("custom app settings: {e}"))?;
        for id in self.layer.keys() {
            let f = crate::policy::feature(id).ok_or_else(|| format!("unknown feature {id}"))?;
            if !f.profiles.contains(&crate::policy::PROFILE_CUSTOMER) {
                return Err(format!("{id} is not a customer-module setting"));
            }
        }
        Ok(())
    }
}

/// The trailer bytes to append to a plain TetherQS.exe for a signed profile document.
pub fn trailer(signed_doc_json: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(signed_doc_json.len() + 12);
    out.extend_from_slice(signed_doc_json);
    out.extend_from_slice(&(signed_doc_json.len() as u32).to_le_bytes());
    out.extend_from_slice(MAGIC);
    out
}

/// Split a file into (plain exe bytes, signed profile JSON) if it carries a trailer.
pub fn split(file: &[u8]) -> (&[u8], Option<&[u8]>) {
    let n = file.len();
    if n < 12 || &file[n - 8..] != MAGIC {
        return (file, None);
    }
    let len = u32::from_le_bytes(file[n - 12..n - 8].try_into().unwrap()) as usize;
    if len == 0 || len > MAX_PROFILE_BYTES || len > n - 12 {
        return (file, None);
    }
    let start = n - 12 - len;
    (&file[..start], Some(&file[start..n - 12]))
}

/// Verify and parse a signed profile document.
pub fn open(signed_doc_json: &[u8], trusted: &[crate::VerifyingKey]) -> Result<CustomProfile, String> {
    let payload = envelope::open_json(signed_doc_json, trusted)?;
    let p: CustomProfile = serde_json::from_slice(&payload).map_err(|e| format!("invalid custom app: {e}"))?;
    p.validate()?;
    Ok(p)
}

/// A distribution token: 32 random bytes, base64url without padding (43 characters).
pub fn new_token() -> String {
    use base64::Engine;
    let mut b = [0u8; 32];
    rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut b);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

/// Shape check for tokens coming from the network (the relay looks them up exactly).
pub fn valid_token(t: &str) -> bool {
    t.len() == 43 && t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{Rule, State};

    fn profile() -> CustomProfile {
        CustomProfile {
            id: "acme-2026".into(),
            name: "ACME Support".into(),
            accent: "#c8102e".into(),
            welcome: "Hello".into(),
            logo_png: String::new(),
            layer: Layer::from([("security.approve-mode".into(), Rule { state: State::Locked, value: Some("click".into()) })]),
            issued_at: 1,
        }
    }

    #[test]
    fn trailer_roundtrip_and_signature() {
        let k = crate::generate_key();
        let doc = serde_json::to_vec(&envelope::sign(&serde_json::to_vec(&profile()).unwrap(), &k)).unwrap();
        let mut exe = b"MZ fake exe".to_vec();
        exe.extend(trailer(&doc));
        let (plain, prof) = split(&exe);
        assert_eq!(plain, b"MZ fake exe");
        assert_eq!(open(prof.unwrap(), &[k.verifying_key()]).unwrap(), profile());
        // Untrusted signer and plain files.
        assert!(open(prof.unwrap(), &[crate::generate_key().verifying_key()]).is_err());
        assert_eq!(split(b"MZ plain").1, None);
    }

    #[test]
    fn validation_and_tokens() {
        assert!(profile().validate().is_ok());
        let mut p = profile();
        p.layer.insert("recording.mode".into(), Rule { state: State::Locked, value: Some("off".into()) });
        assert!(p.validate().is_err(), "technician-only feature must be refused");
        let mut p = profile();
        p.accent = "red".into();
        assert!(p.validate().is_err());
        let t = new_token();
        assert!(valid_token(&t) && t != new_token());
        assert!(!valid_token("../../etc/passwd"));
    }
}

//! Custom TetherQS apps: an admin-signed profile (name, logo, colours, settings) that the relay
//! appends to the plain `TetherQS.exe` when it is downloaded through a private, expiring link.
//!
//! Wire format of the appended trailer (end of the exe file):
//!   `<signed profile JSON (envelope::Signed of CustomProfile)>` `<u32 LE length of it>` `MAGIC`
//! In an Authenticode-signed exe the trailer sits at the end of the certificate table, so the
//! code signature stays valid ([`attach`]).
//! TetherQS reads its own file, verifies the signature against the compiled-in admin keys and
//! applies the profile; a missing, damaged or untrusted trailer means "plain TetherQS".
//! See docs/api.md ("Custom apps").
//!
//! macOS: the `.app` is signed, so nothing can be added to it. The relay instead names the bundle
//! folder `<Name>-Support [<mac id>].app` (the folder name is not sealed) and serves the signed
//! profile at `/q/m/<mac id>`; TetherQS reads its own folder name, fetches and verifies the profile
//! and caches it. The mac id is derived from the token, so the link secret never appears in a path.

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

/// A plain TetherQS.exe with the trailer for `signed_doc_json`. If the exe is Authenticode-signed,
/// the trailer goes inside its certificate table (zero padding first, then the trailer, still the
/// last bytes of the file), and the table size in the PE header and in the certificate entry grow
/// to match. The signature hash leaves the table out, so the signature stays valid. Unsigned
/// exe: the trailer is simply appended.
/// shortcut: Windows' opt-in `EnableCertPaddingCheck` rejects any bytes after the PKCS#7 blob;
/// on such machines the custom exe counts as unsigned. If that becomes common, carry the profile
/// in an unauthenticated attribute of the signature instead.
pub fn attach(exe: &[u8], signed_doc_json: &[u8]) -> Vec<u8> {
    let (patches, tail) = attach_plan(&mut std::io::Cursor::new(exe), signed_doc_json).expect("in-memory reads do not fail");
    let mut out = [exe, &tail].concat();
    for (at, b) in patches {
        out[at as usize..at as usize + 4].copy_from_slice(&b);
    }
    out
}

/// [`attach`] for a file that should not be loaded into memory (the relay): the 4-byte fields to
/// overwrite (file offset, new bytes) and the bytes to append after the unchanged exe.
pub fn attach_plan<R: std::io::Read + std::io::Seek>(exe: &mut R, signed_doc_json: &[u8]) -> std::io::Result<(Vec<(u64, [u8; 4])>, Vec<u8>)> {
    let tr = trailer(signed_doc_json);
    let Some((dir, start, size)) = cert_table(exe)? else { return Ok((Vec::new(), tr)) };
    let pad = (8 - (size as usize + tr.len()) % 8) % 8;
    let new = ((size as usize + pad + tr.len()) as u32).to_le_bytes();
    let mut tail = vec![0u8; pad];
    tail.extend_from_slice(&tr);
    // IMAGE_DIRECTORY_ENTRY_SECURITY.Size and WIN_CERTIFICATE.dwLength
    Ok((vec![(dir + 4, new), (start, new)], tail))
}

/// (offset of the security directory entry, table start, table size) of a PE whose certificate
/// table holds a single entry and ends the file; `None` for anything else (unsigned, not a PE).
fn cert_table<R: std::io::Read + std::io::Seek>(f: &mut R) -> std::io::Result<Option<(u64, u64, u64)>> {
    use std::io::{Read, SeekFrom};
    let len = f.seek(SeekFrom::End(0))?;
    let mut head = Vec::new();
    f.seek(SeekFrom::Start(0))?;
    f.by_ref().take(4096).read_to_end(&mut head)?;
    let found = (|| {
        let u16_at = |o: usize| Some(u16::from_le_bytes(head.get(o..o + 2)?.try_into().ok()?) as usize);
        let u32_at = |o: usize| Some(u32::from_le_bytes(head.get(o..o + 4)?.try_into().ok()?) as u64);
        if head.get(..2)? != b"MZ" {
            return None;
        }
        let pe = u32_at(0x3c)? as usize;
        if head.get(pe..pe + 4)? != b"PE\0\0" {
            return None;
        }
        let opt = pe + 24;
        let dir = opt + match u16_at(opt)? { 0x10b => 96, 0x20b => 112, _ => return None } + 4 * 8;
        let (start, size) = (u32_at(dir)?, u32_at(dir + 4)?);
        (start > 0 && start % 8 == 0 && size >= 8 && start + size == len).then_some((dir as u64, start, size))
    })();
    let Some((dir, start, size)) = found else { return Ok(None) };
    let mut entry = [0u8; 4];
    f.seek(SeekFrom::Start(start))?;
    f.read_exact(&mut entry)?;
    let entry = u32::from_le_bytes(entry) as u64;
    Ok((entry >= 8 && (entry + 7) / 8 * 8 == size).then_some((dir, start, size)))
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

/// Lookup id of a link's profile for macOS: 16 hex characters of SHA-256 over the token. Not the
/// token: bundle paths end up in logs and crash reports, and the id cannot be turned back into a
/// download link.
pub fn mac_id(token: &str) -> String {
    use sha2::Digest;
    let h = sha2::Sha256::digest([b"tetherqs-mac-id\0".as_slice(), token.as_bytes()].concat());
    h[..8].iter().map(|b| format!("{b:02x}")).collect()
}

pub fn valid_mac_id(s: &str) -> bool {
    s.len() == 16 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `.app` folder name the relay gives a custom app for macOS (`name` already reduced to safe characters).
pub fn mac_bundle_name(name: &str, mac_id: &str) -> String {
    format!("{name} [{mac_id}].app")
}

/// The mac id in a bundle folder name, wherever it is (Finder may append " 2" to a second copy).
pub fn mac_id_from_bundle(folder: &str) -> Option<&str> {
    let stem = folder.strip_suffix(".app")?;
    stem.rmatch_indices('[').find_map(|(i, _)| stem.get(i + 1..i + 17).filter(|id| valid_mac_id(id) && stem[i + 17..].starts_with(']')))
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

    /// Minimal PE32+ with a security directory: table at 0x200, one entry of `cert` bytes.
    fn signed_pe(cert: usize) -> Vec<u8> {
        let mut pe = vec![0u8; 0x200];
        pe[..2].copy_from_slice(b"MZ");
        pe[0x3c] = 0x80;
        pe[0x80..0x84].copy_from_slice(b"PE\0\0");
        pe[0x98..0x9a].copy_from_slice(&0x20bu16.to_le_bytes());
        let dir = 0x98 + 112 + 32;
        let size = (cert + 7) / 8 * 8;
        pe[dir..dir + 4].copy_from_slice(&0x200u32.to_le_bytes());
        pe[dir + 4..dir + 8].copy_from_slice(&(size as u32).to_le_bytes());
        pe.extend((cert as u32).to_le_bytes());
        pe.extend([0x00, 0x02, 0x02, 0x00]); // revision 2, WIN_CERT_TYPE_PKCS_SIGNED_DATA
        pe.resize(0x200 + size, 0x30);
        pe
    }

    #[test]
    fn attach_inside_signed_cert_table() {
        let doc = br#"{"payload":"x","sig":"y","key":"z"}"#; // 35 bytes, so padding is needed
        for cert in [1003, 1008] {
            let exe = signed_pe(cert);
            let out = attach(&exe, doc);
            let (dir, start, size) = cert_table(&mut std::io::Cursor::new(&out)).unwrap().expect("still one entry that ends the file");
            let (dir, start, size) = (dir as usize, start as usize, size as usize);
            assert_eq!((dir, start), (0x98 + 112 + 32, 0x200));
            assert_eq!(size % 8, 0);
            assert_eq!(out.len(), start + size);
            assert_eq!(u32::from_le_bytes(out[start..start + 4].try_into().unwrap()) as usize, size);
            // Signed bytes unchanged: everything outside the two size fields and the table tail.
            assert_eq!(&out[..dir + 4], &exe[..dir + 4]);
            assert_eq!(&out[dir + 8..start], &exe[dir + 8..start]);
            assert_eq!(&out[start + 4..exe.len()], &exe[start + 4..]);
            assert_eq!(split(&out).1, Some(&doc[..]));
        }
        // Unsigned PE and non-PE: plain append.
        let mut plain = signed_pe(16);
        plain.truncate(0x200);
        plain[0x98 + 144..0x98 + 152].fill(0);
        assert_eq!(attach(&plain, doc), [&plain[..], &trailer(doc)].concat());
        assert_eq!(attach(b"MZ fake", doc), [&b"MZ fake"[..], &trailer(doc)].concat());
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

    #[test]
    fn mac_ids() {
        let t = "A".repeat(43);
        let id = mac_id(&t);
        assert!(valid_mac_id(&id) && id != mac_id(&"B".repeat(43)) && !t.contains(&id));
        assert_eq!(mac_id(&t), id, "stable: every relay derives the same id");
        let b = mac_bundle_name("ACME GmbH-Support", &id);
        assert_eq!(mac_id_from_bundle(&b), Some(id.as_str()));
        let copy = format!("{} 2.app", b.trim_end_matches(".app"));
        assert_eq!(mac_id_from_bundle(&copy), Some(id.as_str()));
        for bad in ["TetherQS.app", "x [0123456789abcdeF].app", "x [0123456789abcde].app", "x [0123456789abcdef0].app", "x [0123456789abcdef]"] {
            assert_eq!(mac_id_from_bundle(bad), None, "{bad}");
        }
    }
}

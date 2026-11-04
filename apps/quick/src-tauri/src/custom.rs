//! Custom TetherQS app (docs/api.md "Custom apps"): an admin-signed profile appended to this exe by
//! the relay's private download link. Verified against the compiled-in admin keys; a missing,
//! damaged or untrusted trailer means plain TetherQS.

use std::{
    io::{Read, Seek, SeekFrom},
    path::Path,
    sync::OnceLock,
};

use tether_trust::{
    custom::{self, CustomProfile, MAX_PROFILE_BYTES},
    policy::{Effective, Layer},
    VerifyingKey,
};

type Loaded = Result<(CustomProfile, Vec<u8>), String>;

/// The verified profile and this exe's trailer bytes (re-appended by self-update), or why there is
/// none ("" = plain exe).
static PROFILE: OnceLock<Loaded> = OnceLock::new();

/// Read once at startup, before policy is resolved.
pub fn load() {
    PROFILE.get_or_init(|| match std::env::current_exe() {
        Ok(exe) => from_file(&exe, &crate::policy::trusted_keys()),
        Err(e) => Err(e.to_string()),
    });
}

fn get() -> Option<&'static (CustomProfile, Vec<u8>)> {
    PROFILE.get_or_init(|| Err(String::new())).as_ref().ok()
}

/// [`load`] runs before the core opens the log file.
pub fn log_status() {
    match PROFILE.get() {
        Some(Ok((p, _))) => hbb_common::log::info!("custom app {} ({})", p.id, p.name),
        Some(Err(e)) if !e.is_empty() => hbb_common::log::warn!("custom app profile ignored, running as plain TetherQS: {e}"),
        _ => {}
    }
}

pub fn profile() -> Option<&'static CustomProfile> {
    get().map(|(p, _)| p)
}

pub fn layer() -> Option<&'static Layer> {
    profile().map(|p| &p.layer)
}

/// Empty for a plain TetherQS.
pub fn trailer() -> &'static [u8] {
    get().map_or(&[], |(_, t)| t.as_slice())
}

/// Name, accent and welcome of the custom app override `customer.*` (they are its branding).
pub fn brand(eff: &mut Effective) {
    let Some(p) = profile() else { return };
    for (k, v) in [("customer.title", &p.name), ("customer.accent-color", &p.accent), ("customer.welcome", &p.welcome)] {
        if !v.is_empty() {
            eff.values.insert(k.to_owned(), v.clone());
        }
    }
}

/// `data:` URL of the custom logo (validated PNG), empty if none.
pub fn logo_url() -> String {
    profile().filter(|p| !p.logo_png.is_empty()).map(|p| format!("data:image/png;base64,{}", p.logo_png)).unwrap_or_default()
}

fn from_file(exe: &Path, keys: &[VerifyingKey]) -> Loaded {
    let tail = read_tail(exe, MAX_PROFILE_BYTES as u64 + 12).map_err(|e| e.to_string())?;
    let Some(doc) = custom::split(&tail).1 else { return Err(String::new()) };
    Ok((custom::open(doc, keys)?, custom::trailer(doc)))
}

fn read_tail(p: &Path, max: u64) -> std::io::Result<Vec<u8>> {
    let mut f = std::fs::File::open(p)?;
    let len = f.metadata()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(max)))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tether_trust::envelope;

    #[test]
    fn reads_signed_trailer_and_ignores_tampered_or_untrusted() {
        let k = tether_trust::generate_key();
        let p = CustomProfile { id: "acme".into(), name: "ACME".into(), issued_at: 1, ..Default::default() };
        let doc = serde_json::to_vec(&envelope::sign(&serde_json::to_vec(&p).unwrap(), &k)).unwrap();
        let dir = std::env::temp_dir().join(format!("tetherqs-custom-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("a.exe");
        let mut bytes = vec![b'M'; 300_000]; // larger than the tail window
        bytes.extend(custom::trailer(&doc));
        std::fs::write(&exe, &bytes).unwrap();
        let (got, tr) = from_file(&exe, &[k.verifying_key()]).unwrap();
        assert_eq!(got, p);
        assert_eq!(tr, custom::trailer(&doc));
        assert!(from_file(&exe, &[tether_trust::generate_key().verifying_key()]).is_err());
        let n = bytes.len();
        bytes[n - 40] ^= 1; // inside the signed JSON
        std::fs::write(&exe, &bytes).unwrap();
        assert!(from_file(&exe, &[k.verifying_key()]).is_err());
        std::fs::write(&exe, b"MZ plain").unwrap();
        assert_eq!(from_file(&exe, &[k.verifying_key()]).unwrap_err(), "");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

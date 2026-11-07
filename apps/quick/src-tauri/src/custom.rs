//! Custom TetherQS app (docs/api.md "Custom apps"): an admin-signed profile appended to this exe by
//! the relay's private download link. Verified against the compiled-in admin keys; a missing,
//! damaged or untrusted trailer means plain TetherQS.
//!
//! macOS: the signed `.app` cannot carry a trailer. The relay names the bundle folder
//! `<Name>-Support [<mac id>].app`; at the first start TetherQS fetches the signed profile for that
//! id from `/q/m/<mac id>`, verifies it the same way and caches it (`custom-<mac id>.json` in its
//! config folder), so later starts work offline and a self-update (which keeps the folder name)
//! stays branded.

use std::{
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::OnceLock,
    time::Duration,
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
        Ok(exe) if cfg!(target_os = "macos") => from_bundle(&exe, &crate::policy::trusted_keys(), |id| hbb_common::config::Config::path(format!("custom-{id}.json")), fetch),
        Ok(exe) => from_file(&exe, &crate::policy::trusted_keys()),
        Err(e) => Err(e.to_string()),
    });
}

/// `--print-custom-profile`: what [`load`] found, for support and the Mac test (docs/macos.md).
/// Exit code 0 for a custom app, 1 for plain TetherQS.
pub fn print_status() -> bool {
    let exe = std::env::current_exe().map(|e| e.display().to_string()).unwrap_or_default();
    match PROFILE.get() {
        Some(Ok((p, _))) => println!("custom app {} ({}), accent {:?}, {} settings, logo {} bytes (base64); running from {exe}", p.id, p.name, p.accent, p.layer.len(), p.logo_png.len()),
        Some(Err(e)) if !e.is_empty() => println!("plain TetherQS: {e}; running from {exe}"),
        _ => println!("plain TetherQS (no custom app); running from {exe}"),
    }
    profile().is_some()
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

/// macOS: the mac id in the `.app` folder name around `exe`, then the cached or fetched profile.
/// A cached profile that no longer verifies (e.g. admin key rotated) is fetched again.
fn from_bundle(exe: &Path, keys: &[VerifyingKey], cache_path: impl Fn(&str) -> PathBuf, fetch: impl Fn(&str) -> Result<Vec<u8>, String>) -> Loaded {
    let folder = exe.ancestors().nth(3).and_then(Path::file_name).and_then(|n| n.to_str()).unwrap_or("");
    let Some(id) = custom::mac_id_from_bundle(folder) else { return Err(String::new()) };
    let cache = cache_path(id);
    if let Some(p) = std::fs::read(&cache).ok().and_then(|doc| custom::open(&doc, keys).ok()) {
        return Ok((p, Vec::new()));
    }
    let doc = fetch(id).map_err(|e| format!("custom app {id}: could not fetch the profile: {e}"))?;
    let p = custom::open(&doc, keys).map_err(|e| format!("custom app {id}: {e}"))?;
    // Not fatal: the profile is fetched again at the next start.
    if let Err(e) = cache.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::write(&cache, &doc)) {
        hbb_common::log::warn!("custom app {id}: profile not cached at {}: {e}", cache.display());
    }
    Ok((p, Vec::new()))
}

/// `GET https://<relay>/q/m/<mac id>` from each relay in turn (first start only).
fn fetch(id: &str) -> Result<Vec<u8>, String> {
    let client = crate::policy::http()?;
    let mut err = "no relay in this build".to_owned();
    for base in crate::policy::base_urls() {
        let r = client.get(format!("{base}/q/m/{id}")).timeout(Duration::from_secs(10)).send().and_then(|r| r.error_for_status()).and_then(|r| r.bytes());
        match r {
            Ok(b) if b.len() <= MAX_PROFILE_BYTES => return Ok(b.to_vec()),
            Ok(_) => err = format!("{base}: profile too large"),
            Err(e) => err = format!("{base}: {e}"),
        }
    }
    Err(err)
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

    #[test]
    fn mac_bundle_name_fetch_and_cache() {
        let k = tether_trust::generate_key();
        let p = CustomProfile { id: "acme".into(), name: "ACME".into(), issued_at: 1, ..Default::default() };
        let doc = serde_json::to_vec(&envelope::sign(&serde_json::to_vec(&p).unwrap(), &k)).unwrap();
        let dir = std::env::temp_dir().join(format!("tetherqs-mac-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let id = custom::mac_id(&custom::new_token());
        let exe = dir.join(format!("Translocated/d/ACME-Support [{id}] 2.app/Contents/MacOS/TetherQS"));
        let cache = |i: &str| dir.join(format!("prefs/custom-{i}.json"));
        let calls = std::cell::Cell::new(0);
        let relay = |i: &str| {
            calls.set(calls.get() + 1);
            if i == id { Ok(doc.clone()) } else { Err("HTTP 404".to_owned()) }
        };
        let keys = [k.verifying_key()];
        // First start: fetched, verified, cached; then offline from the cache.
        assert_eq!(from_bundle(&exe, &keys, cache, relay).unwrap().0, p);
        assert_eq!(std::fs::read(cache(&id)).unwrap(), doc);
        assert_eq!(from_bundle(&exe, &keys, cache, |_| Err("offline".into())).unwrap().0, p);
        assert_eq!(calls.get(), 1);
        // Untrusted signer: refused, from the cache and from the relay.
        let other = [tether_trust::generate_key().verifying_key()];
        assert!(from_bundle(&exe, &other, cache, relay).unwrap_err().contains("untrusted key"));
        // Plain bundle name: no lookup at all.
        let plain = dir.join("TetherQS.app/Contents/MacOS/TetherQS");
        assert_eq!(from_bundle(&plain, &keys, cache, |_| panic!("no fetch for a plain app")).unwrap_err(), "");
        // Unknown id (revoked app): plain TetherQS with the reason.
        let gone = dir.join("X-Support [0123456789abcdef].app/Contents/MacOS/TetherQS");
        assert!(from_bundle(&gone, &keys, cache, relay).unwrap_err().contains("HTTP 404"));
        // A damaged cache is fetched again.
        std::fs::write(cache(&id), b"{}").unwrap();
        assert_eq!(from_bundle(&exe, &keys, cache, relay).unwrap().0, p);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

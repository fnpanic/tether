//! Release metadata and update rules shared by both apps, Tether Manage and tether-api
//! (docs/update-design.md, docs/updates.md, wire format in docs/api.md "Release index").
//!
//! - [`Index`]: the signed release index `download/<channel>/releases.json`, an admin-key
//!   [`envelope`] like the policy. One document per channel holds every app and platform.
//! - Rollback floor per channel (`releases-<channel>.serial` beside the cached index): serials only
//!   go up; once a device accepted a signed index it never trusts unsigned `latest.json` flags.
//! - [`Candidate`]: what a client may install, from the index or from a legacy `latest.json`; its
//!   bytes are bound to it by size, sha256, minisign signature and the signed trusted comment
//!   (`file:` and `version:`), so a download host (relay, CDN) is never trusted.
//! - [`Packed`]: an optional gzip copy of an artifact (TetherQS.exe on the CDN, whose files may
//!   not exceed [`CDN_MAX_FILE`]); its own size and sha256 are checked before it is unpacked, the
//!   unpacked bytes then like any other.
//! - Staged rollout: a stable per-device [`bucket`] per release and a signed ramp of steps.
//! - [`jitter`] for poll intervals and hint spreads, [`parse_hints`] for the hbbs push hint.

use crate::envelope;
use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const INDEX_TYPE: &str = "tether-releases/1";
/// Largest index document a client reads.
pub const MAX_INDEX: usize = 64 * 1024;
/// Largest artifact a client downloads (also the relay's upload limit).
pub const MAX_ARTIFACT: u64 = 200 << 20;
/// Decision D2 (2026-11-07): builds with this set never use unsigned `latest.json` metadata, even
/// before they accepted a first signed index. Off for the first release with the index; turn it on
/// for the next one, once every relay and Manage write `releases.json`.
pub const REQUIRE_SIGNED_INDEX: bool = false;
/// Decision D3: Manage signs indexes valid for 30 days; past `expires` clients only warn.
pub const EXPIRY_SECS: u64 = 30 * 86_400;
/// Cloudflare Workers Static Assets limit per file (decision D1): larger files stay relay-only.
pub const CDN_MAX_FILE: u64 = 25 << 20;
/// At most one hint-triggered index fetch per relay in this time.
pub const HINT_THROTTLE_SECS: u64 = 60;
/// Largest `spread_secs` a client honours.
pub const MAX_SPREAD_SECS: u32 = 900;
/// Option the rendezvous mediator writes the hbbs `ReleaseHint`s into: `host=G/spread;…`.
pub const HINT_OPTION: &str = "tether-release-hint";

/// Decision D4: the default ramp, 1 % → 10 % after 1 h → 50 % after 6 h → 100 % after 24 h.
pub const RAMP_STANDARD: &[(u64, u8)] = &[(0, 1), (3600, 10), (21_600, 50), (86_400, 100)];
pub const RAMP_FAST: &[(u64, u8)] = &[(0, 10), (3600, 100)];
pub const RAMP_NOW: &[(u64, u8)] = &[(0, 100)];

/// The ramp presets Tether Manage offers: `standard`, `fast`, `now`.
pub fn preset(name: &str) -> Option<&'static [(u64, u8)]> {
    match name {
        "standard" => Some(RAMP_STANDARD),
        "fast" => Some(RAMP_FAST),
        "now" | "immediately" => Some(RAMP_NOW),
        _ => None,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rollout {
    /// Unix seconds the ramp starts.
    pub start: u64,
    /// `[offset seconds, percent]`, offsets ascending: a step function over `now - start`.
    pub steps: Vec<(u64, u8)>,
}

impl Rollout {
    /// Percent of devices eligible at `now` (0 before `start`).
    pub fn percent(&self, now: u64) -> u8 {
        if now < self.start {
            return 0;
        }
        let t = now - self.start;
        self.steps.iter().take_while(|(off, _)| *off <= t).last().map_or(0, |s| s.1)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Entry {
    pub version: String,
    /// Artifact file name; the minisign trusted comment must name it.
    pub file: String,
    pub size: u64,
    /// Lowercase hex.
    pub sha256: String,
    /// Base64 of the minisign `.sig` file (as in `latest.json`).
    pub signature: String,
    /// Download URLs, tried in order. `https://{relay}` stands for the relay the index came from.
    pub urls: Vec<String>,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub mandatory: bool,
    #[serde(default)]
    pub minimum_version: String,
    #[serde(default)]
    pub forced: bool,
    /// Skips the rollout ramp (security fix).
    #[serde(default)]
    pub security: bool,
    #[serde(default)]
    pub rollout: Rollout,
    /// A halted rollout: devices that did not install it yet do not install it.
    #[serde(default)]
    pub halted: bool,
    /// A gzip copy, tried before `urls`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub packed: Option<Packed>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Packed {
    /// Only `gzip`.
    pub encoding: String,
    pub size: u64,
    pub sha256: String,
    pub urls: Vec<String>,
}

fn valid_sha(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn valid_urls(urls: &[String]) -> bool {
    !urls.is_empty() && urls.len() <= 8 && !urls.iter().any(|u| !u.starts_with("https://") || u.len() > 1024 || u.chars().any(char::is_whitespace))
}

fn subst(urls: &[String], relay_base: &str) -> Vec<String> {
    urls.iter()
        .filter_map(|u| match u.strip_prefix("https://{relay}") {
            Some(rest) => Some(format!("{}{rest}", relay_base.trim_end_matches('/'))),
            None => (u.starts_with("https://") && !u.contains('{')).then(|| u.clone()),
        })
        .collect()
}

/// Payload of `download/<channel>/releases.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Index {
    #[serde(rename = "type")]
    pub kind: String,
    pub channel: String,
    pub serial: u64,
    pub issued: u64,
    pub expires: u64,
    /// `<app>/<platform>` (`technician/windows-x86_64`, `customer/darwin-universal`, …).
    pub releases: BTreeMap<String, Entry>,
}

pub fn valid_channel(c: &str) -> bool {
    (1..=32).contains(&c.len()) && c.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

pub fn valid_version(v: &str) -> bool {
    !v.is_empty() && v.len() <= 32 && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-') && crate::version::compare(v, v).is_some()
}

fn valid_file(f: &str) -> bool {
    !f.is_empty() && f.len() <= 255 && f != "." && f != ".." && f.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn valid_key(k: &str) -> bool {
    let mut it = k.split('/');
    matches!((it.next(), it.next(), it.next()), (Some(a), Some(p), None) if matches!(a, "technician" | "customer") && valid_file(p))
}

impl Entry {
    pub fn validate(&self) -> Result<(), String> {
        if !valid_version(&self.version) {
            return Err(format!("bad version {:?}", self.version));
        }
        if !valid_file(&self.file) {
            return Err(format!("bad file name {:?}", self.file));
        }
        if self.size == 0 || self.size > MAX_ARTIFACT {
            return Err(format!("size must be 1..={MAX_ARTIFACT}"));
        }
        if !valid_sha(&self.sha256) {
            return Err("sha256 must be 64 lowercase hex digits".into());
        }
        if let Some(p) = &self.packed {
            if p.encoding != "gzip" || p.size == 0 || p.size > MAX_ARTIFACT || !valid_sha(&p.sha256) || !valid_urls(&p.urls) {
                return Err("packed: gzip, size, sha256 and 1-8 https:// URLs".into());
            }
        }
        if self.signature.is_empty() || self.signature.len() > 4096 {
            return Err("bad signature".into());
        }
        if !valid_urls(&self.urls) {
            return Err("1-8 https:// URLs".into());
        }
        if !self.minimum_version.is_empty() && !valid_version(&self.minimum_version) {
            return Err("bad minimumVersion".into());
        }
        if self.notes.len() > 8192 {
            return Err("notes too long".into());
        }
        let s = &self.rollout.steps;
        if s.is_empty() || s.len() > 16 || s.iter().any(|(_, p)| *p > 100) || s.windows(2).any(|w| w[0].0 >= w[1].0) {
            return Err("rollout: 1-16 steps, offsets ascending, percent 0-100".into());
        }
        Ok(())
    }

    /// Percent of devices eligible at `now`: 100 for a security release, else the ramp.
    pub fn percent(&self, now: u64) -> u8 {
        if self.security {
            100
        } else {
            self.rollout.percent(now)
        }
    }

    /// Download URLs with `https://{relay}` replaced by `relay_base` (the `https://host` the index
    /// came from; the app chooses it, so a debug build may pass its local stand-in). Literal URLs
    /// stay https only.
    pub fn urls_for(&self, relay_base: &str) -> Vec<String> {
        subst(&self.urls, relay_base)
    }
}

impl Index {
    pub fn validate(&self) -> Result<(), String> {
        if self.kind != INDEX_TYPE {
            return Err(format!("not a release index (type {:?})", self.kind));
        }
        if !valid_channel(&self.channel) {
            return Err("bad channel".into());
        }
        if self.serial == 0 || self.expires <= self.issued {
            return Err("serial must be > 0 and expires after issued".into());
        }
        if self.releases.len() > 16 {
            return Err("too many releases".into());
        }
        for (k, e) in &self.releases {
            if !valid_key(k) {
                return Err(format!("bad release key {k:?}"));
            }
            e.validate().map_err(|err| format!("{k}: {err}"))?;
        }
        Ok(())
    }

    pub fn from_json(payload: &[u8]) -> Result<Self, String> {
        let i: Index = serde_json::from_slice(payload).map_err(|e| format!("invalid release index: {e}"))?;
        i.validate()?;
        Ok(i)
    }

    /// Verify a signed document against the admin keys, parse it, and require `channel` (a
    /// document from another channel is rejected).
    pub fn open(raw: &[u8], admin: &[VerifyingKey], channel: &str) -> Result<Self, String> {
        if raw.len() > MAX_INDEX {
            return Err("release index too large".into());
        }
        let i = Self::from_json(&envelope::open_json(raw, admin)?)?;
        if i.channel != channel {
            return Err(format!("release index is for channel {:?}, not {channel:?}", i.channel));
        }
        Ok(i)
    }

    /// Past `expires`: still used, but "update information is out of date" (D3: warn only).
    pub fn stale(&self, now: u64) -> bool {
        now > self.expires
    }
}

// ----------------------------------------------------------------------------- floor and cache

/// The cached index (the signed document as fetched) at `path` if valid and not below the floor.
pub fn load(path: &Path, admin: &[VerifyingKey], channel: &str) -> Option<Index> {
    Index::open(&std::fs::read(path).ok()?, admin, channel).ok().filter(|i| i.serial >= floor(path, admin))
}

fn floor_path(path: &Path) -> PathBuf {
    path.with_extension("serial")
}

/// Highest serial ever accepted for this cache ("<serial> <signing admin key>"); it counts only
/// while that key is trusted. 0: this device never accepted a signed index.
pub fn floor(path: &Path, admin: &[VerifyingKey]) -> u64 {
    let s = std::fs::read_to_string(floor_path(path)).unwrap_or_default();
    let Some((serial, key)) = s.trim().split_once(' ') else { return 0 };
    match crate::key_from_b64(key) {
        Ok(k) if admin.contains(&k) => serial.parse().unwrap_or(0),
        _ => 0,
    }
}

fn write_atomic(path: &Path, data: &[u8]) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    std::fs::write(&tmp, data).and_then(|_| std::fs::rename(&tmp, path)).map_err(|e| format!("cannot cache release index: {e}"))
}

/// Of several fetched documents (`(relay base, raw)`), the valid one with the highest serial that
/// is not below the floor at `path`; when it is newer than the floor it is cached and the floor
/// raised. `Err` only when no document is usable and at least one was invalid or too old (the
/// reasons, for the log). `Ok(None)`: nothing fetched.
pub fn choose(path: &Path, docs: &[(String, Vec<u8>)], admin: &[VerifyingKey], channel: &str) -> Result<Option<(String, Index)>, String> {
    let fl = floor(path, admin);
    let mut best: Option<(usize, Index)> = None;
    let mut errors = Vec::new();
    for (i, (base, raw)) in docs.iter().enumerate() {
        match Index::open(raw, admin, channel) {
            Ok(ix) if ix.serial < fl => errors.push(format!("{base}: release index serial {} is older than the accepted {fl}", ix.serial)),
            Ok(ix) if best.as_ref().is_none_or(|b| ix.serial > b.1.serial) => best = Some((i, ix)),
            Ok(_) => {}
            Err(e) => errors.push(format!("{base}: {e}")),
        }
    }
    let Some((i, ix)) = best else {
        return if errors.is_empty() { Ok(None) } else { Err(errors.join("; ")) };
    };
    if ix.serial > fl || std::fs::read(path).ok().as_deref() != Some(&docs[i].1[..]) {
        let signer = serde_json::from_slice::<envelope::Signed>(&docs[i].1).map_err(|e| e.to_string())?.key;
        write_atomic(path, &docs[i].1)?;
        write_atomic(&floor_path(path), format!("{} {signer}", ix.serial).as_bytes())?;
    }
    Ok(Some((docs[i].0.clone(), ix)))
}

// ----------------------------------------------------------------------------- rollout

/// Stable bucket 0.0..100.0 of this device for one release: `u16(SHA-256("tether-rollout" ‖
/// install_uuid ‖ app ‖ version)[0..2]) / 655.36`. The version salt reshuffles who goes first.
pub fn bucket(install_uuid: &str, app: &str, version: &str) -> f64 {
    let mut h = Sha256::new();
    for part in ["tether-rollout", install_uuid, app, version] {
        h.update(part.as_bytes());
        h.update([0u8]);
    }
    let d = h.finalize();
    u16::from_be_bytes([d[0], d[1]]) as f64 / 655.36
}

/// Policy `update.rollout`: `early` joins the first wave, `late` waits for 100 %.
pub fn eligible(entry: &Entry, bucket: f64, now: u64, mode: &str) -> bool {
    if entry.halted {
        return false;
    }
    let p = entry.percent(now);
    match mode {
        "early" => p > 0,
        "late" => p >= 100,
        _ => bucket < p as f64,
    }
}

// ----------------------------------------------------------------------------- artifacts

/// `key:value` fields of a Tauri / Tether Manage trusted comment (`timestamp:…\tfile:…\tversion:…`).
pub fn trusted_fields(comment: &str) -> BTreeMap<&str, &str> {
    comment.split('\t').filter_map(|f| f.split_once(':')).collect()
}

fn public_key(pubkey: &str) -> Result<minisign_verify::PublicKey, String> {
    let b64 = crate::b64();
    use base64::Engine;
    b64.decode(pubkey.trim())
        .ok()
        .and_then(|raw| String::from_utf8(raw).ok())
        .filter(|t| t.contains("untrusted comment"))
        .map(|t| minisign_verify::PublicKey::decode(&t))
        .unwrap_or_else(|| minisign_verify::PublicKey::from_base64(pubkey.trim()))
        .map_err(|e| format!("bad updater key: {e}"))
}

/// Minisign check in the Tauri updater format (`signature`: base64 of the `.sig` file, `pubkey`:
/// base64 of the public key file or a bare `RW…` key) **and** the signed trusted comment: it must
/// name `file` and `version`. This binds the bytes to the offered version, so an older signed
/// binary offered under a higher version (W9 rollback) is refused.
pub fn verify_artifact(data: &[u8], signature: &str, pubkey: &str, file: &str, version: &str) -> Result<(), String> {
    use base64::Engine;
    let pk = public_key(pubkey)?;
    let text = crate::b64().decode(signature.trim()).map_err(|e| format!("bad signature encoding: {e}"))?;
    let text = String::from_utf8(text).map_err(|_| "bad signature encoding".to_owned())?;
    let sig = minisign_verify::Signature::decode(&text).map_err(|e| format!("bad signature: {e}"))?;
    pk.verify(data, &sig, true).map_err(|e| format!("signature does not match: {e}"))?;
    // Only now is the trusted comment authentic (the global signature covers it).
    let f = trusted_fields(sig.trusted_comment());
    if f.get("version").copied() != Some(version) {
        return Err(format!("the update is signed as version {:?}, not {version}", f.get("version").copied().unwrap_or("none")));
    }
    if f.get("file").copied() != Some(file) {
        return Err(format!("the update is signed as file {:?}, not {file}", f.get("file").copied().unwrap_or("none")));
    }
    Ok(())
}

pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

/// The last path segment of a URL (`…/Tether_0.5.24_x64-setup.exe?x` → `Tether_0.5.24_x64-setup.exe`).
pub fn file_of(url: &str) -> &str {
    url.split(['?', '#']).next().unwrap_or("").rsplit('/').next().unwrap_or("")
}

/// What a client may install: from the signed index (`serial > 0`) or a legacy `latest.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Candidate {
    pub version: String,
    pub notes: String,
    pub file: String,
    pub urls: Vec<String>,
    pub signature: String,
    /// Index only: required size and sha256 of the bytes.
    pub size: u64,
    pub sha256: String,
    pub mandatory: bool,
    pub minimum_version: String,
    pub forced: bool,
    pub security: bool,
    pub halted: bool,
    /// Index serial; 0 = unsigned legacy manifest.
    pub serial: u64,
    pub expires: u64,
    pub rollout: Rollout,
    /// The gzip copy, URLs already substituted.
    pub packed: Option<Packed>,
}

impl Candidate {
    pub fn from_entry(e: &Entry, relay_base: &str, ix: &Index) -> Self {
        Candidate {
            version: e.version.clone(),
            notes: e.notes.clone(),
            file: e.file.clone(),
            urls: e.urls_for(relay_base),
            signature: e.signature.clone(),
            size: e.size,
            sha256: e.sha256.clone(),
            mandatory: e.mandatory,
            minimum_version: e.minimum_version.clone(),
            forced: e.forced,
            security: e.security,
            halted: e.halted,
            serial: ix.serial,
            expires: ix.expires,
            rollout: e.rollout.clone(),
            packed: e.packed.as_ref().map(|p| Packed { urls: subst(&p.urls, relay_base), ..p.clone() }),
        }
    }

    /// Every download to try, in order: `(url, packed)`.
    pub fn sources(&self) -> Vec<(String, bool)> {
        let packed = self.packed.iter().flat_map(|p| p.urls.iter().map(|u| (u.clone(), true)));
        packed.chain(self.urls.iter().map(|u| (u.clone(), false))).collect()
    }

    /// Largest number of bytes to read from a source.
    pub fn limit(&self, packed: bool) -> u64 {
        match (&self.packed, packed) {
            (Some(p), true) => p.size,
            _ if self.size > 0 => self.size,
            _ => MAX_ARTIFACT,
        }
    }

    /// Downloaded bytes → the verified artifact. From a packed source, bytes matching the packed
    /// size and sha256 are unpacked with `gunzip(bytes, max_out)`; anything else (a proxy or CDN
    /// that decoded the transfer) is taken as the artifact itself. Then [`Self::verify`].
    pub fn accept(&self, data: Vec<u8>, packed: bool, pubkey: &str, gunzip: impl Fn(&[u8], u64) -> Result<Vec<u8>, String>) -> Result<Vec<u8>, String> {
        let plain = match &self.packed {
            Some(p) if packed && data.len() as u64 == p.size && sha256_hex(&data) == p.sha256 => gunzip(&data, if self.size > 0 { self.size } else { MAX_ARTIFACT })?,
            _ => data,
        };
        self.verify(&plain, pubkey)?;
        Ok(plain)
    }

    /// A legacy `latest.json` platform entry: no size or hash, everything else as announced.
    pub fn legacy(version: &str, notes: &str, url: &str, signature: &str, mandatory: bool, minimum_version: &str, forced: bool) -> Self {
        Candidate {
            version: version.into(),
            notes: notes.into(),
            file: file_of(url).into(),
            urls: vec![url.into()],
            signature: signature.into(),
            mandatory,
            minimum_version: minimum_version.into(),
            forced,
            rollout: Rollout { start: 0, steps: vec![(0, 100)] },
            ..Default::default()
        }
    }

    pub fn signed(&self) -> bool {
        self.serial > 0
    }

    pub fn stale(&self, now: u64) -> bool {
        self.signed() && now > self.expires
    }

    pub fn eligible(&self, bucket: f64, now: u64, mode: &str) -> bool {
        if !self.signed() {
            return true;
        }
        let e = Entry { halted: self.halted, security: self.security, rollout: self.rollout.clone(), ..Default::default() };
        eligible(&e, bucket, now, mode)
    }

    /// Size and sha256 (index only), then signature and trusted comment.
    pub fn verify(&self, data: &[u8], pubkey: &str) -> Result<(), String> {
        if self.signed() {
            if data.len() as u64 != self.size {
                return Err(format!("the download has {} bytes, the release index says {}", data.len(), self.size));
            }
            if sha256_hex(data) != self.sha256 {
                return Err("the download does not match the release index (sha256)".into());
            }
        }
        verify_artifact(data, &self.signature, pubkey, &self.file, &self.version)
    }
}

// ----------------------------------------------------------------------------- hints and timing

/// Uniform random seconds in `0..=max`.
pub fn uniform_secs(max: u64) -> u64 {
    if max == 0 {
        return 0;
    }
    let mut b = [0u8; 8];
    rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut b);
    u64::from_le_bytes(b) % (max + 1)
}

/// `base` plus 0..=`extra_pct` % (Chromium-style poll jitter: no fixed clock times).
pub fn jitter(base: Duration, extra_pct: u64) -> Duration {
    base + Duration::from_secs(uniform_secs(base.as_secs() * extra_pct / 100))
}

/// The mediator's [`HINT_OPTION`] value: `host=G/spread;…` → host → (generation, spread secs).
pub fn parse_hints(s: &str) -> BTreeMap<String, (u64, u32)> {
    s.split(';')
        .filter_map(|e| {
            let (host, rest) = e.trim().split_once('=')?;
            let (g, spread) = rest.split_once('/').unwrap_or((rest, "60"));
            let host = crate::policy::server_host(host.trim());
            if host.is_empty() {
                return None;
            }
            Some((host.to_owned(), (g.trim().parse().ok()?, spread.trim().parse::<u32>().ok()?.min(MAX_SPREAD_SECS))))
        })
        .collect()
}

/// Per-relay release generation hints (hbbs push or heartbeat answer) → whether to fetch the
/// index now: a changed value, or a host's first one above 0 (hbbs sends the current G after a
/// registration, so a device that was offline during a push catches up), at most once per
/// [`HINT_THROTTLE_SECS`] per host. `G` is unsigned on purpose: a relay can only cause a fetch.
#[derive(Default)]
pub struct Hints {
    seen: BTreeMap<String, u64>,
    last_fire: BTreeMap<String, u64>,
}

impl Hints {
    pub fn update(&mut self, host: &str, generation: u64, now: u64) -> bool {
        let prev = self.seen.insert(host.to_owned(), generation);
        if prev == Some(generation) || (prev.is_none() && generation == 0) {
            return false;
        }
        if self.last_fire.get(host).is_some_and(|t| now < t + HINT_THROTTLE_SECS) {
            // Keep the old value so the change fires once the throttle is over.
            self.seen.insert(host.to_owned(), prev.unwrap_or(0));
            return false;
        }
        self.last_fire.insert(host.to_owned(), now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn sha(d: &[u8]) -> String {
        sha256_hex(d)
    }

    fn minisign_keys() -> (minisign::KeyPair, String) {
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        use base64::Engine;
        let pk = crate::b64().encode(kp.pk.to_box().unwrap().to_string());
        (kp, pk)
    }

    fn sign(kp: &minisign::KeyPair, data: &[u8], trusted: &str) -> String {
        use base64::Engine;
        let s = minisign::sign(Some(&kp.pk), &kp.sk, Cursor::new(data), Some(trusted), Some("signature from tauri secret key")).unwrap();
        crate::b64().encode(s.to_string())
    }

    fn entry(version: &str, data: &[u8], sig: &str) -> Entry {
        Entry {
            version: version.into(),
            file: format!("Tether_{version}_x64-setup.exe"),
            size: data.len() as u64,
            sha256: sha(data),
            signature: sig.into(),
            urls: vec!["https://dl.example.workers.dev/stable/x".into(), "https://{relay}/download/stable/technician/windows-x86_64/x".into()],
            rollout: Rollout { start: 1000, steps: RAMP_STANDARD.to_vec() },
            ..Default::default()
        }
    }

    fn index(serial: u64, releases: Vec<(&str, Entry)>) -> Index {
        Index { kind: INDEX_TYPE.into(), channel: "stable".into(), serial, issued: 1000, expires: 1000 + EXPIRY_SECS, releases: releases.into_iter().map(|(k, e)| (k.to_owned(), e)).collect() }
    }

    fn doc(i: &Index, k: &crate::SigningKey) -> Vec<u8> {
        serde_json::to_vec(&envelope::sign(&serde_json::to_vec(i).unwrap(), k)).unwrap()
    }

    #[test]
    fn sign_verify_and_rejections() {
        let admin = crate::generate_key();
        let trusted = [admin.verifying_key()];
        let (kp, _) = minisign_keys();
        let e = entry("0.5.24", b"x", &sign(&kp, b"x", "timestamp:1\tfile:Tether_0.5.24_x64-setup.exe\tversion:0.5.24"));
        let ix = index(57, vec![("technician/windows-x86_64", e.clone())]);
        assert_eq!(Index::open(&doc(&ix, &admin), &trusted, "stable").unwrap(), ix);
        // Wrong key, wrong channel, wrong type, tampered, oversized.
        assert!(Index::open(&doc(&ix, &crate::generate_key()), &trusted, "stable").is_err());
        assert!(Index::open(&doc(&ix, &admin), &trusted, "test").unwrap_err().contains("channel"));
        let mut bad = ix.clone();
        bad.kind = "tether-policy/1".into();
        assert!(Index::open(&doc(&bad, &admin), &trusted, "stable").unwrap_err().contains("not a release index"));
        let mut signed: envelope::Signed = serde_json::from_slice(&doc(&ix, &admin)).unwrap();
        use base64::Engine;
        signed.payload = crate::b64().encode(serde_json::to_vec(&index(58, vec![])).unwrap());
        assert!(Index::open(&serde_json::to_vec(&signed).unwrap(), &trusted, "stable").is_err());
        assert!(Index::open(&vec![b' '; MAX_INDEX + 1], &trusted, "stable").unwrap_err().contains("too large"));
        // Invalid contents, even when signed.
        for f in [
            |e: &mut Entry| e.urls = vec!["http://plain/x".into()],
            |e: &mut Entry| e.sha256 = "ABC".into(),
            |e: &mut Entry| e.size = MAX_ARTIFACT + 1,
            |e: &mut Entry| e.file = "../x".into(),
            |e: &mut Entry| e.rollout.steps = vec![(10, 50), (5, 100)],
            |e: &mut Entry| e.rollout.steps = vec![(0, 101)],
            |e: &mut Entry| e.version = "1.x".into(),
        ] {
            let mut e2 = e.clone();
            f(&mut e2);
            assert!(Index::open(&doc(&index(9, vec![("technician/windows-x86_64", e2)]), &admin), &trusted, "stable").is_err());
        }
        assert!(Index::open(&doc(&index(9, vec![("other/windows-x86_64", e.clone())]), &admin), &trusted, "stable").is_err());
        // Unknown fields are refused (same rule as the policy).
        assert!(Index::from_json(br#"{"type":"tether-releases/1","channel":"stable","serial":1,"issued":0,"expires":9,"releases":{},"x":1}"#).is_err());
    }

    #[test]
    fn serial_floor_is_monotonic_and_survives_a_deleted_cache() {
        let admin = crate::generate_key();
        let trusted = [admin.verifying_key()];
        let dir = std::env::temp_dir().join(format!("tether-release-floor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("releases-stable.json");
        let d = |s| ("https://r1".to_owned(), doc(&index(s, vec![]), &admin));
        assert_eq!(floor(&path, &trusted), 0);
        assert_eq!(choose(&path, &[], &trusted, "stable").unwrap(), None);
        // Highest serial across relays wins; the floor goes up.
        let (base, ix) = choose(&path, &[d(4), ("https://r2".into(), doc(&index(6, vec![]), &admin)), d(5)], &trusted, "stable").unwrap().unwrap();
        assert_eq!((base.as_str(), ix.serial), ("https://r2", 6));
        assert_eq!(floor(&path, &trusted), 6);
        assert_eq!(load(&path, &trusted, "stable").unwrap().serial, 6);
        // Older serial from a relay: refused; with a newer one beside it, the newer wins.
        assert!(choose(&path, &[d(5)], &trusted, "stable").unwrap_err().contains("older than the accepted 6"));
        assert_eq!(choose(&path, &[d(5), d(6)], &trusted, "stable").unwrap().unwrap().1.serial, 6);
        // Deleting the cache keeps the floor; an old document copied over it is not loaded.
        std::fs::remove_file(&path).unwrap();
        assert!(choose(&path, &[d(5)], &trusted, "stable").is_err());
        std::fs::write(&path, d(3).1).unwrap();
        assert!(load(&path, &trusted, "stable").is_none());
        // A forged document never counts.
        assert!(choose(&path, &[("https://evil".into(), doc(&index(99, vec![]), &crate::generate_key()))], &trusted, "stable").is_err());
        assert_eq!(floor(&path, &trusted), 6);
        // A floor written under an admin key this build no longer trusts does not count.
        let new_admin = crate::generate_key();
        assert_eq!(floor(&path, &[new_admin.verifying_key()]), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn expiry_only_flags_stale() {
        let ix = index(1, vec![]);
        assert!(!ix.stale(ix.expires) && ix.stale(ix.expires + 1));
        let c = Candidate { serial: 1, expires: 100, ..Default::default() };
        assert!(c.stale(101) && !c.stale(100));
        // Legacy manifests have no expiry.
        assert!(!Candidate::legacy("1.0.0", "", "https://h/x.exe", "s", false, "", false).stale(u64::MAX));
    }

    #[test]
    fn bucket_is_stable_and_uniform() {
        assert_eq!(bucket("u1", "technician", "0.5.24"), bucket("u1", "technician", "0.5.24"));
        assert_ne!(bucket("u1", "technician", "0.5.24"), bucket("u1", "technician", "0.5.25"));
        assert_ne!(bucket("u1", "technician", "0.5.24"), bucket("u1", "customer", "0.5.24"));
        let n = 100_000;
        let mut hist = [0u32; 100];
        for i in 0..n {
            let b = bucket(&format!("uuid-{i}"), "technician", "0.5.24");
            assert!((0.0..100.0).contains(&b));
            hist[b as usize] += 1;
        }
        // Each percent within ±0.3 % of the devices.
        for (p, c) in hist.iter().enumerate() {
            assert!((*c as f64 / n as f64 - 0.01).abs() < 0.003, "percent {p}: {c}");
        }
    }

    #[test]
    fn ramp_math_and_eligibility() {
        let r = Rollout { start: 1000, steps: RAMP_STANDARD.to_vec() };
        assert_eq!([r.percent(999), r.percent(1000), r.percent(4599), r.percent(4600), r.percent(22_600), r.percent(87_400), r.percent(u64::MAX)], [0, 1, 1, 10, 50, 100, 100]);
        assert_eq!(Rollout { start: 0, steps: vec![(10, 5)] }.percent(5), 0);
        let mut e = Entry { rollout: r, ..Default::default() };
        assert!(eligible(&e, 0.5, 1000, "default") && !eligible(&e, 1.0, 1000, "default"));
        assert!(eligible(&e, 9.99, 4600, "default") && !eligible(&e, 10.0, 4600, ""));
        assert!(eligible(&e, 99.0, 1000, "early") && !eligible(&e, 0.0, 999, "early"));
        assert!(!eligible(&e, 0.0, 22_600, "late") && eligible(&e, 99.9, 87_400, "late"));
        e.security = true;
        assert!(eligible(&e, 99.9, 0, "late"), "security skips the ramp");
        e.halted = true;
        assert!(!eligible(&e, 0.0, u64::MAX, "early"), "halted stops everyone");
        for (n, steps) in [("standard", RAMP_STANDARD), ("fast", RAMP_FAST), ("now", RAMP_NOW)] {
            assert_eq!(preset(n), Some(steps));
        }
        assert_eq!(preset("x"), None);
    }

    #[test]
    fn relay_substitution_is_https_only() {
        let e = Entry { urls: vec!["https://cdn/x".into(), "https://{relay}/download/x".into(), "http://plain/x".into(), "https://{other}/x".into()], ..Default::default() };
        assert_eq!(e.urls_for("https://r1"), ["https://cdn/x", "https://r1/download/x"]);
        // The app picks the base (a debug build's local API is http).
        assert_eq!(e.urls_for("http://127.0.0.1:22130/")[1], "http://127.0.0.1:22130/download/x");
        assert_eq!(file_of("https://h/a/b/Tether_1_x64-setup.exe?x=1"), "Tether_1_x64-setup.exe");
    }

    #[test]
    fn trusted_comment_binds_file_and_version() {
        let (kp, pk) = minisign_keys();
        let v1 = b"old signed binary 0.5.23";
        let sig_v1 = sign(&kp, v1, "timestamp:1700000000\tfile:TetherQS-0.5.23.exe\tversion:0.5.23");
        assert!(verify_artifact(v1, &sig_v1, &pk, "TetherQS-0.5.23.exe", "0.5.23").is_ok());
        // Bare RW… key form works too.
        assert!(verify_artifact(v1, &sig_v1, &kp.pk.to_base64(), "TetherQS-0.5.23.exe", "0.5.23").is_ok());
        // W9 rollback: the old signed binary offered as 0.5.99 is refused.
        assert!(verify_artifact(v1, &sig_v1, &pk, "TetherQS-0.5.23.exe", "0.5.99").unwrap_err().contains("signed as version \"0.5.23\""));
        assert!(verify_artifact(v1, &sig_v1, &pk, "TetherQS-0.5.99.exe", "0.5.23").unwrap_err().contains("signed as file"));
        // A signature without version (plain `tauri signer sign` before 2.x) is refused.
        let no_version = sign(&kp, v1, "timestamp:1\tfile:TetherQS-0.5.23.exe");
        assert!(verify_artifact(v1, &no_version, &pk, "TetherQS-0.5.23.exe", "0.5.23").unwrap_err().contains("none"));
        // Tampered bytes and foreign keys.
        assert!(verify_artifact(b"tampered", &sig_v1, &pk, "TetherQS-0.5.23.exe", "0.5.23").is_err());
        let (_, other) = minisign_keys();
        assert!(verify_artifact(v1, &sig_v1, &other, "TetherQS-0.5.23.exe", "0.5.23").is_err());
        assert_eq!(trusted_fields("timestamp:1\tfile:a.exe\tversion:1.2")["version"], "1.2");
    }

    #[test]
    fn candidate_binds_size_hash_and_signature() {
        let (kp, pk) = minisign_keys();
        let data = b"new installer bytes";
        let e = entry("0.5.24", data, &sign(&kp, data, "timestamp:1\tfile:Tether_0.5.24_x64-setup.exe\tversion:0.5.24"));
        let ix = index(3, vec![("technician/windows-x86_64", e.clone())]);
        let c = Candidate::from_entry(&e, "https://r1", &ix);
        assert_eq!(c.urls[1], "https://r1/download/stable/technician/windows-x86_64/x");
        assert!(c.verify(data, &pk).is_ok());
        assert!(c.verify(b"new installer byteS", &pk).unwrap_err().contains("sha256"));
        assert!(c.verify(b"short", &pk).unwrap_err().contains("bytes"));
        // A downgraded binary under the new entry: right size and hash written by an attacker who
        // controls the index? Impossible without the admin key; with only the CDN, the hash fails.
        let old = b"old installer bytes";
        let mut forged = c.clone();
        forged.sha256 = sha(old);
        forged.signature = sign(&kp, old, "timestamp:1\tfile:Tether_0.5.23_x64-setup.exe\tversion:0.5.23");
        assert!(forged.verify(old, &pk).unwrap_err().contains("signed as version"));
        // Legacy manifest: no hash, but the trusted comment still binds the version.
        let l = Candidate::legacy("0.5.24", "", "https://h/d/Tether_0.5.24_x64-setup.exe", &e.signature, false, "", false);
        assert!(l.verify(data, &pk).is_ok() && l.eligible(99.0, 0, "late"));
        let l2 = Candidate::legacy("0.5.25", "", "https://h/d/Tether_0.5.25_x64-setup.exe", &e.signature, false, "", false);
        assert!(l2.verify(data, &pk).is_err());
    }

    #[test]
    fn packed_copy_is_checked_before_and_after_unpacking() {
        let (kp, pk) = minisign_keys();
        let exe = b"TetherQS plain exe bytes".to_vec();
        let sig = sign(&kp, &exe, "timestamp:1\tfile:TetherQS-0.5.24.exe\tversion:0.5.24");
        // A stand-in codec: "packing" reverses the bytes.
        let pack = |d: &[u8]| d.iter().rev().copied().collect::<Vec<u8>>();
        let gunzip = |d: &[u8], max: u64| {
            let out = pack(d);
            if out.len() as u64 > max { Err("too big".to_owned()) } else { Ok(out) }
        };
        let packed = pack(&exe);
        let mut e = entry("0.5.24", &exe, &sig);
        e.file = "TetherQS-0.5.24.exe".into();
        e.urls = vec!["https://{relay}/download/stable/customer/windows-x86_64/TetherQS-0.5.24.exe".into()];
        e.packed = Some(Packed { encoding: "gzip".into(), size: packed.len() as u64, sha256: sha(&packed), urls: vec!["https://tether-dl.example.workers.dev/stable/customer/windows-x86_64/TetherQS-0.5.24.exe.gz".into()] });
        e.validate().unwrap();
        let ix = index(4, vec![("customer/windows-x86_64", e.clone())]);
        let c = Candidate::from_entry(&e, "https://r1", &ix);
        assert_eq!(c.sources(), [(e.packed.as_ref().unwrap().urls[0].clone(), true), ("https://r1/download/stable/customer/windows-x86_64/TetherQS-0.5.24.exe".into(), false)]);
        assert_eq!((c.limit(true), c.limit(false)), (packed.len() as u64, exe.len() as u64));
        assert_eq!(c.accept(packed.clone(), true, &pk, gunzip).unwrap(), exe);
        // Already decoded on the way (Content-Encoding): taken as the artifact.
        assert_eq!(c.accept(exe.clone(), true, &pk, gunzip).unwrap(), exe);
        // A tampered packed copy is not unpacked, and fails as an artifact.
        let mut bad = packed.clone();
        bad[0] ^= 1;
        assert!(c.accept(bad, true, &pk, gunzip).is_err());
        // From a plain URL, packed bytes are never unpacked.
        assert!(c.accept(packed, false, &pk, gunzip).is_err());
        let mut e2 = e.clone();
        e2.packed.as_mut().unwrap().encoding = "xz".into();
        assert!(e2.validate().is_err());
        // Old index readers: an entry without `packed` still parses, and serializes without it.
        let mut e3 = e.clone();
        e3.packed = None;
        assert!(!serde_json::to_string(&e3).unwrap().contains("packed"));
    }

    #[test]
    fn jitter_bounds() {
        for _ in 0..1000 {
            let d = jitter(Duration::from_secs(6 * 3600), 20);
            assert!(d >= Duration::from_secs(6 * 3600) && d <= Duration::from_secs(6 * 3600 * 120 / 100));
            assert!(uniform_secs(60) <= 60);
        }
        assert_eq!(uniform_secs(0), 0);
        let spread: std::collections::BTreeSet<u64> = (0..2000).map(|_| uniform_secs(3)).collect();
        assert_eq!(spread.len(), 4, "every value 0..=3 shows up");
    }

    #[test]
    fn hint_parsing_and_throttle() {
        let h = parse_hints("tether-desk.duckdns.org=7/60; wss://r2.example/ws/id=9/5000;bad;x=notanumber/1;=3/1");
        assert_eq!(h["tether-desk.duckdns.org"], (7, 60));
        assert_eq!(h["r2.example"], (9, MAX_SPREAD_SECS));
        assert_eq!(h.len(), 2);
        assert_eq!(parse_hints("a=5")["a"], (5, 60));
        let mut s = Hints::default();
        assert!(!s.update("a", 0, 0), "first value 0: nothing published yet");
        assert!(!s.update("a", 0, 1));
        assert!(s.update("a", 6, 2));
        assert!(!s.update("a", 6, 3), "unchanged");
        assert!(!s.update("a", 7, 30), "throttled: one fetch per host per minute");
        assert!(s.update("a", 7, 62), "fires after the throttle");
        assert!(s.update("b", 4, 62), "first value above 0 (after a registration): catch up");
        assert!(!s.update("b", 4, 63) && s.update("b", 5, 200), "per host");
    }
}

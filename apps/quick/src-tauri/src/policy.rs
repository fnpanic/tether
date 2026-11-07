//! Minimal policy client for the `customer` profile (docs/api.md): fetch the signed bundle,
//! verify it against the compiled-in admin key(s), reject older serials, cache it, resolve it and
//! apply the core options. Heartbeat after every check. The signed server key list
//! (docs/key-rotation.md) is fetched, cached (`server-keys.json`) and applied the same way.

use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, RwLock,
    },
    time::Duration,
};

use hbb_common::config::{Config, LocalConfig, DEFAULT_SETTINGS, OVERWRITE_SETTINGS};
use tether_trust::{
    envelope,
    policy::{parse_servers, resolve_with, server_host, Bundle, Effective, FLOOR, PROFILE_CUSTOMER, SERVERS_LIST, DIRECT},
    serverkeys,
    VerifyingKey,
};

const ADMIN_KEYS: Option<&str> = option_env!("TETHER_ADMIN_PUB_KEY");
const SERVER: Option<&str> = option_env!("TETHER_SERVER");
const MAX_DOC: usize = 256 * 1024;

static EFFECTIVE: RwLock<Option<Effective>> = RwLock::new(None);
/// Core options this module put into OVERWRITE_SETTINGS, removed again when policy unlocks them.
static APPLIED: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn current() -> Effective {
    EFFECTIVE
        .read()
        .unwrap()
        .clone()
        .unwrap_or_else(|| resolved(None, ""))
}

/// Policy for this module: bundle layers plus the custom app's settings and branding, if any.
fn resolved(bundle: Option<&Bundle>, device: &str) -> Effective {
    let mut eff = resolve_with(bundle, PROFILE_CUSTOMER, device, crate::custom::layer(), None);
    crate::custom::brand(&mut eff);
    eff
}

/// The API server that answered last (else the first compiled-in one), or None in builds without
/// a pinned server.
pub fn base_url() -> Option<String> {
    base_urls().into_iter().next()
}

/// `https://<host>` of every server (compiled-in, then policy `servers.list`), last good first.
pub fn base_urls() -> Vec<String> {
    if SERVER.is_none() {
        return Vec::new(); // never the upstream public servers
    }
    let mut v: Vec<String> = hbb_common::config::own_rendezvous_servers()
        .iter()
        .map(|h| server_host(h.trim()))
        .filter(|h| !h.is_empty())
        .map(|h| format!("https://{h}"))
        .collect();
    v.dedup();
    let last = LAST_BASE.lock().unwrap().clone();
    if let Some(i) = v.iter().position(|b| *b == last) {
        let b = v.remove(i);
        v.insert(0, b);
    }
    v
}

static LAST_BASE: Mutex<String> = Mutex::new(String::new());

/// `servers.list` → extra rendezvous servers (host names only, see `parse_servers`); the host
/// registers with all of them. True when the list changed.
fn apply_servers(eff: &Effective) -> bool {
    let list = parse_servers(eff.get(SERVERS_LIST)).unwrap_or_default();
    let mut cur = hbb_common::config::POLICY_RENDEZVOUS_SERVERS.write().unwrap();
    if *cur == list {
        return false;
    }
    hbb_common::log::info!("policy servers: {list:?}");
    *cur = list;
    true
}

/// HTTPS client through the detected system proxy (same detection as the relay connection).
pub fn http() -> Result<reqwest::blocking::Client, String> {
    let target = base_url().unwrap_or_else(|| "https://example.com".into());
    let mut b = reqwest::blocking::Client::builder()
        .use_native_tls() // Windows trust store: corporate TLS-inspection roots work.
        .https_only(true)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("TetherQS/", env!("CARGO_PKG_VERSION")));
    b = match crate::netpolicy::detect_proxy(&format!("{target}/")) {
        Some(p) => b.proxy(reqwest::Proxy::all(p).map_err(|e| e.to_string())?),
        None => b.no_proxy(),
    };
    b.build().map_err(|e| e.to_string())
}

pub fn trusted_keys() -> Vec<VerifyingKey> {
    ADMIN_KEYS
        .unwrap_or("")
        .split(',')
        .filter(|k| !k.trim().is_empty())
        .filter_map(|k| tether_trust::key_from_b64(k).ok())
        .collect()
}

fn open(raw: &[u8], keys: &[VerifyingKey]) -> Result<Bundle, String> {
    Bundle::from_json(&envelope::open_json(raw, keys)?)
}

/// A fetched document is accepted only if valid and not older than the last accepted serial.
/// `Ok(None)`: same serial, nothing to do.
fn accept(raw: &[u8], keys: &[VerifyingKey], last_serial: u64) -> Result<Option<Bundle>, String> {
    let b = open(raw, keys)?;
    if b.serial < last_serial {
        return Err(format!("policy serial {} is older than {last_serial}", b.serial));
    }
    Ok((b.serial > last_serial).then_some(b))
}

/// Stable per-install id for heartbeat and group assignment.
pub fn device_uuid() -> String {
    let u = LocalConfig::get_option("tether-uuid");
    if !u.is_empty() {
        return u;
    }
    let u = uuid::Uuid::new_v4().to_string();
    LocalConfig::set_option("tether-uuid".into(), u.clone());
    u
}

fn set(bundle: Option<&Bundle>) {
    let eff = resolved(bundle, &device_uuid());
    apply_core(&eff);
    // Servers first: apply_direct points STUN at every server.
    let servers_changed = apply_servers(&eff);
    crate::netpolicy::apply_direct(eff.get(DIRECT));
    *EFFECTIVE.write().unwrap() = Some(eff);
    if servers_changed {
        librustdesk::ui::api::restart_rendezvous();
    }
}

/// Locked features become overrides, the rest defaults. Floor keys are never touched.
fn apply_core(eff: &Effective) {
    let mut applied = APPLIED.lock().unwrap();
    let mut o = OVERWRITE_SETTINGS.write().unwrap();
    for k in applied.drain(..) {
        o.remove(&k);
    }
    for (k, v) in eff.core_overrides().into_iter().filter(|(k, _)| !FLOOR.contains(k)) {
        o.insert(k.to_owned(), v);
        applied.push(k.to_owned());
    }
    drop(o);
    let mut d = DEFAULT_SETTINGS.write().unwrap();
    for (k, v) in eff.core_defaults().into_iter().filter(|(k, _)| !FLOOR.contains(k)) {
        d.insert(k.to_owned(), v);
    }
}

/// Serial of the applied server key list (heartbeat `server_keys_serial`; 0 = none).
static SERVER_KEYS_SERIAL: AtomicU64 = AtomicU64::new(0);

/// Cached server key list → the core's presented and trusted server keys (none: the compiled-in
/// key only). Re-run on every check so expiries take effect.
fn apply_server_keys(keys: &[VerifyingKey]) {
    let list = serverkeys::load(&Config::path("server-keys.json"), keys);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let active = list.as_ref().map(|l| l.active(now)).unwrap_or_default();
    SERVER_KEYS_SERIAL.store(list.map_or(0, |l| l.serial), Ordering::Relaxed);
    let mut cur = hbb_common::config::POLICY_SERVER_KEYS.write().unwrap();
    if *cur != active {
        hbb_common::log::info!("server keys: {active:?}");
        *cur = active;
    }
}

/// `GET /api/server-keys` from the first server that answers; a newer list replaces the cache.
fn fetch_server_keys(keys: &[VerifyingKey]) -> Result<(), String> {
    let serial = SERVER_KEYS_SERIAL.load(Ordering::Relaxed);
    let client = http()?;
    let mut resp = Err("no server".to_owned());
    for base in base_urls() {
        resp = client.get(format!("{base}/api/server-keys")).header("If-None-Match", format!("\"{serial}\"")).send().map_err(|e| e.to_string());
        if resp.is_ok() {
            break;
        }
    }
    let resp = resp?;
    match resp.status().as_u16() {
        304 | 404 => return Ok(()),
        200 => {}
        s => return Err(format!("server key list fetch: HTTP {s}")),
    }
    let raw = tether_trust::read_limited(resp, 16 * 1024).map_err(|e| format!("server key list: {e}"))?;
    if let Some(l) = serverkeys::accept(&Config::path("server-keys.json"), &raw, keys)? {
        hbb_common::log::info!("server key list {} applied", l.serial);
    }
    Ok(())
}

/// Before the core starts: last verified policy (or built-in defaults).
pub fn load_cached() {
    apply_server_keys(&trusted_keys());
    let cached = std::fs::read(Config::path("policy.json"))
        .ok()
        .and_then(|raw| open(&raw, &trusted_keys()).ok());
    set(cached.as_ref());
}

fn fetch(keys: &[VerifyingKey]) -> Result<bool, String> {
    let serial = current().serial;
    let client = http()?;
    // First server that answers.
    let mut resp = Err("no server".to_owned());
    for base in base_urls() {
        resp = client.get(format!("{base}/api/policy")).header("If-None-Match", format!("\"{serial}\"")).send().map_err(|e| e.to_string());
        if resp.is_ok() {
            *LAST_BASE.lock().unwrap() = base;
            break;
        }
    }
    let resp = resp?;
    match resp.status().as_u16() {
        304 | 404 => return Ok(false),
        200 => {}
        s => return Err(format!("policy fetch: HTTP {s}")),
    }
    let raw = tether_trust::read_limited(resp, MAX_DOC).map_err(|e| format!("policy document: {e}"))?;
    let Some(b) = accept(&raw, keys, serial)? else { return Ok(false) };
    let path = Config::path("policy.json");
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, &raw).and_then(|_| std::fs::rename(&tmp, &path)).is_err() {
        hbb_common::log::warn!("could not cache policy");
    }
    hbb_common::log::info!("policy serial {} applied", b.serial);
    set(Some(&b));
    Ok(true)
}

fn heartbeat() -> Result<(), String> {
    let base = base_url().ok_or("no server")?;
    let mut body = serde_json::json!({
        "uuid": device_uuid(),
        "id": librustdesk::ui::api::get_id(),
        "profile": PROFILE_CUSTOMER,
        "version": env!("CARGO_PKG_VERSION"),
        "os": format!("{} {}", std::env::consts::OS, hbb_common::whoami::distro()).chars().take(64).collect::<String>(),
        "serial": current().serial,
    });
    // Only with a list: relay APIs from before key rotation reject unknown fields.
    let keys_serial = SERVER_KEYS_SERIAL.load(Ordering::Relaxed);
    if keys_serial > 0 {
        body["server_keys_serial"] = serde_json::json!(keys_serial);
    }
    let plain = body.clone();
    // Update state only to relays that answered with a release generation: older ones reject
    // unknown fields (400), and then the plain heartbeat goes again.
    let new_relay = RELAY_HAS_RELEASES.load(Ordering::Relaxed);
    if new_relay {
        let (state, serial, version) = crate::update::heartbeat_fields();
        body["release_serial"] = serde_json::json!(serial);
        body["update_state"] = serde_json::json!(state);
        if tether_trust::release::valid_version(&version) {
            body["update_version"] = serde_json::json!(version);
        }
    }
    let client = http()?;
    let send = |b: &serde_json::Value| client.post(format!("{base}/api/heartbeat")).json(b).send().map_err(|e| e.to_string());
    let mut resp = send(&body)?;
    if new_relay && resp.status().as_u16() == 400 {
        RELAY_HAS_RELEASES.store(false, Ordering::Relaxed);
        resp = send(&plain)?;
    }
    // `{"releases": G}`: the relay's release generation; a change means "check for updates".
    if resp.status().as_u16() == 200 {
        if let Some(g) = tether_trust::read_limited(resp, 1024).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()).and_then(|v| v["releases"].as_u64()) {
            RELAY_HAS_RELEASES.store(true, Ordering::Relaxed);
            crate::update::hint(server_host(base.trim_start_matches("https://")), g, 60);
        }
    }
    Ok(())
}

/// The heartbeat relay answered with a release generation (it knows the update fields).
static RELAY_HAS_RELEASES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Background refresh every `policy.refresh-minutes` (1 with `policy.fast-apply`).
/// Builds without an admin key (debug only, see build.rs) keep the defaults and stay offline.
pub fn spawn(on_change: impl Fn() + Send + 'static) {
    let keys = trusted_keys();
    if keys.is_empty() {
        hbb_common::log::warn!("no TETHER_ADMIN_PUB_KEY: policy disabled, using defaults");
        return;
    }
    std::thread::spawn(move || loop {
        if let Err(e) = fetch_server_keys(&keys) {
            hbb_common::log::warn!("server key list: {e}");
        }
        apply_server_keys(&keys);
        match fetch(&keys) {
            Ok(true) => on_change(),
            Ok(false) => {}
            Err(e) => hbb_common::log::warn!("policy: {e}"),
        }
        if let Err(e) = heartbeat() {
            hbb_common::log::debug!("heartbeat: {e}");
        }
        let eff = current();
        let mins: u64 = if eff.on("policy.fast-apply") { 1 } else { eff.get("policy.refresh-minutes").parse().unwrap_or(15) };
        std::thread::sleep(Duration::from_secs(mins.max(1) * 60));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use tether_trust::policy::resolve;

    fn doc(serial: u64, key: &tether_trust::SigningKey, extra: &str) -> Vec<u8> {
        let payload = format!(r#"{{"serial":{serial},"issued_at":0,"profiles":{{"customer":{{{extra}}}}}}}"#);
        serde_json::to_vec(&envelope::sign(payload.as_bytes(), key)).unwrap()
    }

    #[test]
    fn accepts_newer_rejects_older_and_untrusted() {
        let k = tether_trust::generate_key();
        let keys = [k.verifying_key()];
        assert!(accept(&doc(5, &k, ""), &keys, 3).unwrap().is_some());
        assert!(accept(&doc(5, &k, ""), &keys, 5).unwrap().is_none());
        assert!(accept(&doc(2, &k, ""), &keys, 5).is_err());
        let other = tether_trust::generate_key();
        assert!(accept(&doc(9, &other, ""), &keys, 0).is_err());
    }

    #[test]
    fn applies_customer_policy_without_touching_the_floor() {
        crate::security::apply_policy();
        let k = tether_trust::generate_key();
        let raw = doc(1, &k, r#""permission.clipboard":{"state":"off"},"customer.title":{"state":"locked","value":"ACME"}"#);
        let b = open(&raw, &[k.verifying_key()]).unwrap();
        let eff = resolve(Some(&b), PROFILE_CUSTOMER, "dev", None);
        apply_core(&eff);
        assert_eq!(eff.get("customer.title"), "ACME");
        assert_eq!(OVERWRITE_SETTINGS.read().unwrap().get("enable-clipboard").map(String::as_str), Some("N"));
        assert_eq!(OVERWRITE_SETTINGS.read().unwrap().get("allow-insecure-tls-fallback").map(String::as_str), Some("N"));
        // Unlocking removes the override again.
        apply_core(&resolve(None, PROFILE_CUSTOMER, "dev", None));
        assert!(OVERWRITE_SETTINGS.read().unwrap().get("enable-clipboard").is_none());
    }
}

//! Admin policy client (profile `technician`): signed bundle from `GET /api/policy`, cached in the
//! config dir, resolved with the machine layer (GPO/MDM) and applied to the core options.
//! Contract: docs/api.md; model and crypto: crates/tether-trust; feature list: docs/policy.md.
//!
//! Every process role loads the cached bundle at startup without network (`init`); the main UI and
//! the `--server` role keep it fresh (`start`), only the main UI sends heartbeats.
//! The signed server key list (docs/key-rotation.md) travels the same way: `server-keys.json`.

use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        LazyLock, RwLock,
    },
    time::Duration,
};

use hbb_common::{
    config::{Config, DEFAULT_DISPLAY_SETTINGS, DEFAULT_SETTINGS, OVERWRITE_DISPLAY_SETTINGS, OVERWRITE_SETTINGS},
    log,
};
use serde_json::{json, Value};
use tether_trust::{
    envelope,
    policy::{parse_servers, resolve, server_host, Bundle, Effective, Layer, FEATURES, FLOOR, PROFILE_TECHNICIAN, SERVERS_LIST, DIRECT},
    serverkeys::{self, KeyList},
    VerifyingKey,
};

pub const PROFILE: &str = PROFILE_TECHNICIAN;
/// Comma-separated base64 Ed25519 keys; release builds refuse to compile without (build.rs).
const ADMIN_KEYS: Option<&str> = option_env!("TETHER_ADMIN_PUB_KEY");
pub const SERVER: Option<&str> = option_env!("TETHER_SERVER");
/// Options that change which server this app talks to (`settings.server`).
const SERVER_OPTIONS: &[&str] = &["custom-rendezvous-server", "relay-server", "key"];
/// Per-peer session options (the viewer reads them from the peer config, not the global options),
/// so policy must go to the display-settings layers for them to take effect.
const DISPLAY_OPTIONS: &[&str] = &["codec-preference"];

/// First compiled-in server host, without port (`TETHER_SERVER` may list several).
pub fn primary_host() -> &'static str {
    server_host(SERVER.unwrap_or_default().split(',').next().unwrap_or_default().trim())
}

/// API hosts, best first: the last one that answered, then compiled-in and policy servers.
pub fn api_hosts() -> Vec<String> {
    if SERVER.is_none() {
        return Vec::new(); // never the upstream public servers
    }
    let mut v: Vec<String> = hbb_common::config::own_rendezvous_servers()
        .iter()
        .map(|h| server_host(h).to_owned())
        .filter(|h| !h.is_empty())
        .collect();
    v.dedup();
    let last = LAST_API_HOST.read().unwrap().clone();
    if let Some(i) = v.iter().position(|h| *h == last) {
        let h = v.remove(i);
        v.insert(0, h);
    }
    v
}

static LAST_API_HOST: RwLock<String> = RwLock::new(String::new());

/// `servers.list` → extra rendezvous servers in the core (hosts register with every one, viewers
/// ask them all). Only host names pass, see `parse_servers`. True when the list changed.
fn apply_servers(eff: &Effective) -> bool {
    let list = parse_servers(eff.get(SERVERS_LIST)).unwrap_or_else(|e| {
        log::error!("policy {SERVERS_LIST} ignored: {e}");
        Vec::new()
    });
    let mut cur = hbb_common::config::POLICY_RENDEZVOUS_SERVERS.write().unwrap();
    if *cur == list {
        return false;
    }
    log::info!("policy servers: {list:?}");
    *cur = list;
    true
}

#[derive(Default)]
struct State {
    bundle: Option<Bundle>,
    eff: Effective,
    /// Overwrite keys this module set, removed again when a feature is unlocked.
    applied: Vec<String>,
}

static STATE: LazyLock<RwLock<State>> = LazyLock::new(|| {
    let eff = resolve(None, PROFILE, "", None);
    RwLock::new(State { eff, ..Default::default() })
});

// ----------------------------------------------------------------------------- public API

/// Effective value of a feature ("" for unknown ids).
pub fn get(id: &str) -> String {
    STATE.read().unwrap().eff.get(id).to_owned()
}

/// Bool feature is on ("Y").
pub fn on(id: &str) -> bool {
    STATE.read().unwrap().eff.on(id)
}

pub fn effective() -> Effective {
    STATE.read().unwrap().eff.clone()
}

/// `locked:<feature>: <text>` error when `feature` is off.
pub fn require(id: &str, text: &str) -> Result<(), String> {
    if on(id) {
        Ok(())
    } else {
        Err(format!("locked:{id}: {text}"))
    }
}

/// Opening an outgoing session: allowed by policy and not blocked by a required update.
pub fn check_outgoing() -> Result<(), String> {
    require("connect.outgoing", "Your organisation does not allow connecting to other devices.")?;
    crate::update::guard()
}

/// `set_option` guard: refuses options locked by policy and server fields unless allowed.
pub fn check_option_write(eff: &Effective, key: &str) -> Result<(), String> {
    if SERVER_OPTIONS.contains(&key) && !eff.on("settings.server") {
        return Err("locked:settings.server: Your organisation manages the server settings.".into());
    }
    match FEATURES.iter().find(|f| f.option == Some(key)) {
        Some(f) if eff.is_locked(f.id) => Err(format!("locked:{}: Managed by your organisation.", f.id)),
        _ => Ok(()),
    }
}

/// State for the UI (`policy_state` command and `policy` event). Secrets are not sent.
pub fn state_json() -> Value {
    let mut eff = effective();
    if let Some(v) = eff.values.get_mut("recording.upload-token") {
        if !v.is_empty() {
            *v = "********".into();
        }
    }
    serde_json::to_value(eff).unwrap_or_default()
}

/// Install uuid (heartbeat `uuid`, group assignment): generated once per config dir.
pub fn install_uuid() -> String {
    let path = Config::path("install-uuid");
    if let Ok(s) = std::fs::read_to_string(&path) {
        let s = s.trim();
        if (8..=64).contains(&s.len()) && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return s.to_owned();
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    if let Err(e) = write_atomic(&path, id.as_bytes()) {
        log::warn!("cannot store install uuid: {e}");
    }
    id
}

// ----------------------------------------------------------------------------- load / apply

/// The compiled-in admin keys (policy, server key list, release index).
pub(crate) fn admin_keys() -> Vec<VerifyingKey> {
    trusted_keys()
}

fn trusted_keys() -> Vec<VerifyingKey> {
    ADMIN_KEYS
        .unwrap_or("")
        .split(',')
        .filter(|k| !k.trim().is_empty())
        .filter_map(|k| tether_trust::key_from_b64(k).map_err(|e| log::error!("admin key: {e}")).ok())
        .collect()
}

fn open(raw: &[u8], keys: &[VerifyingKey]) -> Result<Bundle, String> {
    Bundle::from_json(&envelope::open_json(raw, keys)?)
}

fn last_serial() -> u64 {
    std::fs::read_to_string(Config::path("policy-serial"))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

pub(crate) fn write_atomic(path: &PathBuf, data: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}

/// Serial of the applied server key list (heartbeat `server_keys_serial`; 0 = none).
static SERVER_KEYS_SERIAL: AtomicU64 = AtomicU64::new(0);

/// The cached server key list → the core's presented and trusted server keys. Without a list the
/// core uses the compiled-in key only. Re-run on every check so expiries take effect.
fn apply_server_keys(keys: &[VerifyingKey]) {
    let list = serverkeys::load(&Config::path("server-keys.json"), keys);
    let active = list.as_ref().map(|l| l.active(api_now())).unwrap_or_default();
    SERVER_KEYS_SERIAL.store(list.map_or(0, |l| l.serial), Ordering::Relaxed);
    let mut cur = hbb_common::config::POLICY_SERVER_KEYS.write().unwrap();
    if *cur != active {
        log::info!("server keys: {active:?}");
        *cur = active;
    }
}

fn api_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// `GET /api/server-keys`: a newer list signed by the admin key replaces the cached one.
fn refresh_server_keys(keys: &[VerifyingKey]) -> Result<Option<KeyList>, String> {
    let serial = SERVER_KEYS_SERIAL.load(Ordering::Relaxed);
    let r = api(|c, base| {
        let rq = c.get(format!("{base}/api/server-keys"));
        if serial > 0 { rq.header("If-None-Match", format!("\"{serial}\"")) } else { rq }
    })?;
    match r.status().as_u16() {
        304 | 404 => return Ok(None), // 404: nothing published, or a relay API without the endpoint
        200 => {}
        s => return Err(format!("server key list: server answered {s}")),
    }
    let raw = tether_trust::read_limited(r, 16 * 1024).map_err(|e| format!("server key list: {e}"))?;
    serverkeys::accept(&Config::path("server-keys.json"), &raw, keys)
}

/// Startup, every role, no network: cached bundle (if still valid) + machine layer.
pub fn init() {
    let keys = trusted_keys();
    apply_server_keys(&keys);
    let bundle = std::fs::read(Config::path("policy.json")).ok().and_then(|raw| {
        open(&raw, &keys)
            .map_err(|e| log::warn!("cached policy ignored: {e}"))
            .ok()
            .filter(|b| b.serial >= last_serial())
    });
    apply(bundle);
}

fn apply(bundle: Option<Bundle>) -> bool {
    let machine = machine_layer();
    let eff = resolve(bundle.as_ref(), PROFILE, &install_uuid(), machine.as_ref());
    let mut st = STATE.write().unwrap();
    let changed = st.eff != eff;
    apply_core(&eff, &mut st.applied);
    // Servers first: apply_direct points STUN at every server.
    let servers_changed = apply_servers(&eff);
    crate::netpolicy::apply_direct(eff.get(DIRECT));
    st.eff = eff;
    st.bundle = bundle;
    drop(st);
    if servers_changed {
        librustdesk::ui::api::restart_rendezvous();
    }
    if changed {
        crate::app::emit("policy", state_json());
    }
    changed
}

/// Locked features → overwrite settings, unlocked ones → defaults. Floor options are never touched
/// and the security settings are re-applied last.
fn apply_core(eff: &Effective, applied: &mut Vec<String>) {
    let mut o = OVERWRITE_SETTINGS.write().unwrap();
    let mut od = OVERWRITE_DISPLAY_SETTINGS.write().unwrap();
    for k in applied.drain(..) {
        o.remove(&k);
        od.remove(&k);
    }
    for (k, v) in eff.core_overrides().into_iter().filter(|(k, _)| !FLOOR.contains(k)) {
        let map = if DISPLAY_OPTIONS.contains(&k) { &mut od } else { &mut o };
        map.insert(k.to_owned(), v);
        applied.push(k.to_owned());
    }
    // A server or key stored while settings.server was allowed (or written into the config file)
    // must not stay the trust anchor: locked means the built-in servers and signed key list.
    // These are FLOOR keys, so never in `applied`: policy can only blank them, not set them.
    for k in SERVER_OPTIONS {
        if eff.on("settings.server") {
            o.remove(*k);
        } else {
            o.insert((*k).to_owned(), String::new());
        }
    }
    drop((o, od));
    let mut d = DEFAULT_SETTINGS.write().unwrap();
    let mut dd = DEFAULT_DISPLAY_SETTINGS.write().unwrap();
    for (k, v) in eff.core_defaults().into_iter().filter(|(k, _)| !FLOOR.contains(k)) {
        let map = if DISPLAY_OPTIONS.contains(&k) { &mut dd } else { &mut d };
        map.insert(k.to_owned(), v);
    }
    drop((d, dd));
    crate::security::enforce();
}

/// GPO/MDM layer: a `Layer` JSON in `HKLM\SOFTWARE\Policies\Tether` `PolicyJson` (REG_SZ) or
/// `/Library/Managed Preferences/net.tether.app.plist` key `PolicyJson`.
fn machine_layer() -> Option<Layer> {
    let raw = machine_json()?;
    serde_json::from_str(&raw).map_err(|e| log::error!("machine policy ignored: {e}")).ok()
}

#[cfg(windows)]
fn machine_json() -> Option<String> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (key, name) = (wide("SOFTWARE\\Policies\\Tether"), wide("PolicyJson"));
    unsafe {
        let mut size = 0u32;
        let get = |buf: *mut std::ffi::c_void, size: &mut u32| {
            RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf, size)
        };
        if get(std::ptr::null_mut(), &mut size) != 0 || size == 0 || size > 1 << 20 {
            return None;
        }
        let mut buf = vec![0u16; size as usize / 2];
        if get(buf.as_mut_ptr().cast(), &mut size) != 0 {
            return None;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]))
    }
}

#[cfg(target_os = "macos")]
fn machine_json() -> Option<String> {
    // shortcut: reads via `defaults`, no plist crate; fine for one string key. Upgrade (plist crate
    // or CFPreferences) when a second managed key or a non-string value is read.
    let out = std::process::Command::new("/usr/bin/defaults")
        .args(["read", "/Library/Managed Preferences/net.tether.app", "PolicyJson"])
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

#[cfg(not(any(windows, target_os = "macos")))]
fn machine_json() -> Option<String> {
    None
}

// ----------------------------------------------------------------------------- network

/// Proxy URL for API requests: the user's or the detected system proxy (`netpolicy.rs`).
pub fn proxy_url() -> Option<String> {
    let s = Config::get_socks()?;
    let p = s.proxy.trim();
    if p.is_empty() {
        return None;
    }
    let mut u = url::Url::parse(&if p.contains("://") { p.to_owned() } else { format!("http://{p}") }).ok()?;
    if !s.username.is_empty() {
        let _ = u.set_username(&s.username);
        let _ = u.set_password(Some(&s.password));
    }
    Some(u.to_string())
}

/// HTTPS request to the relay API, tried like the relay route racer: port 443 then 80, through
/// the proxy first (if any), then direct. TLS is always verified (OS trust store, so
/// TLS-inspecting corporate proxies with a deployed root work). Errors starting `offline:` mean
/// no route reached the server.
pub(crate) fn api(
    build: impl Fn(&reqwest::blocking::Client, &str) -> reqwest::blocking::RequestBuilder,
) -> Result<reqwest::blocking::Response, String> {
    api_via(&api_hosts(), build)
}

/// Debug builds only: tests point the API at a local stand-in (`TETHER_DEV_API=http://127.0.0.1:<port>`).
fn dev_api() -> Option<String> {
    #[cfg(debug_assertions)]
    if let Ok(base) = std::env::var("TETHER_DEV_API") {
        return Some(base.trim_end_matches('/').to_owned());
    }
    None
}

/// Relays to ask one by one (update checks ask every relay, the highest release wins), with the
/// base URL `{relay}` stands for in the release index. A debug build's dev API is the only one.
pub(crate) fn relay_bases() -> Vec<(String, String)> {
    match dev_api() {
        Some(b) => vec![(server_host(b.trim_start_matches("http://")).to_owned(), b)],
        None => api_hosts().into_iter().map(|h| (h.clone(), format!("https://{h}"))).collect(),
    }
}

/// [`api`] against the given hosts only, in order.
pub(crate) fn api_via(
    hosts: &[String],
    build: impl Fn(&reqwest::blocking::Client, &str) -> reqwest::blocking::RequestBuilder,
) -> Result<reqwest::blocking::Response, String> {
    if let Some(base) = dev_api() {
        let client = reqwest::blocking::Client::builder().no_proxy().timeout(Duration::from_secs(10)).build().map_err(|e| e.to_string())?;
        return build(&client, &base).send().map_err(|e| format!("offline: {e}"));
    }
    if hosts.is_empty() {
        return Err("offline: no server compiled into this build".into());
    }
    let proxy = proxy_url();
    let mut last = String::new();
    // First reachable server: each one over all routes before the next.
    for host in hosts {
        for via_proxy in [true, false] {
            if via_proxy && proxy.is_none() {
                continue;
            }
            let mut b = reqwest::blocking::Client::builder()
                .use_rustls_tls()
                .no_proxy()
                .connect_timeout(Duration::from_secs(6))
                .timeout(Duration::from_secs(30))
                .user_agent(concat!("Tether/", env!("CARGO_PKG_VERSION")));
            if via_proxy {
                b = b.proxy(reqwest::Proxy::all(proxy.as_deref().unwrap_or_default()).map_err(|e| e.to_string())?);
            }
            let client = b.build().map_err(|e| e.to_string())?;
            for base in [format!("https://{host}"), format!("https://{host}:80")] {
                match build(&client, &base).send() {
                    Ok(r) => {
                        *LAST_API_HOST.write().unwrap() = host.clone();
                        return Ok(r);
                    }
                    Err(e) => last = e.to_string(),
                }
            }
        }
    }
    Err(format!("offline: {last}"))
}

/// GET of any https URL (release downloads, also on a CDN): through the proxy first (if any),
/// then direct; TLS verified against the OS trust store. `http://` only for a debug build's dev API.
pub(crate) fn fetch(url: &str, timeout: Duration) -> Result<reqwest::blocking::Response, String> {
    if !url.starts_with("https://") && !dev_api().is_some_and(|b| url.starts_with(&b)) {
        return Err(format!("not an https URL: {url}"));
    }
    let proxy = proxy_url();
    let mut last = String::new();
    for via_proxy in [true, false] {
        if via_proxy && proxy.is_none() {
            continue;
        }
        let mut b = reqwest::blocking::Client::builder()
            .use_rustls_tls()
            .no_proxy()
            .connect_timeout(Duration::from_secs(10))
            .timeout(timeout)
            .user_agent(concat!("Tether/", env!("CARGO_PKG_VERSION")));
        if via_proxy {
            b = b.proxy(reqwest::Proxy::all(proxy.as_deref().unwrap_or_default()).map_err(|e| e.to_string())?);
        }
        match b.build().map_err(|e| e.to_string())?.get(url).send().and_then(|r| r.error_for_status()) {
            Ok(r) => return Ok(r),
            Err(e) => last = e.to_string(),
        }
    }
    Err(format!("download failed: {last}"))
}

/// One policy check (and server key list check). `Ok(true)` when a newer bundle was applied.
pub fn refresh() -> Result<bool, String> {
    let keys = trusted_keys();
    if keys.is_empty() {
        // Debug builds without TETHER_ADMIN_PUB_KEY run with defaults (+ machine layer) only.
        return Ok(apply(None));
    }
    match refresh_server_keys(&keys) {
        Ok(Some(l)) => log::info!("server key list {} applied", l.serial),
        Ok(None) => {}
        Err(e) => log::info!("server key list check: {e}"),
    }
    apply_server_keys(&keys);
    let serial = STATE.read().unwrap().bundle.as_ref().map(|b| b.serial);
    let r = api(|c, base| {
        let rq = c.get(format!("{base}/api/policy"));
        match serial {
            Some(s) => rq.header("If-None-Match", format!("\"{s}\"")),
            None => rq,
        }
    })?;
    let cached = STATE.read().unwrap().bundle.clone();
    match r.status().as_u16() {
        304 | 404 => return Ok(apply(cached)),
        200 => {}
        s => return Err(format!("policy server answered {s}")),
    }
    let raw = tether_trust::read_limited(r, 512 * 1024).map_err(|e| format!("policy document: {e}"))?;
    let bundle = open(&raw, &keys)?;
    if bundle.serial < last_serial() {
        return Err(format!("policy serial {} is older than the last accepted {}", bundle.serial, last_serial()));
    }
    let new = serial != Some(bundle.serial);
    if new {
        write_atomic(&Config::path("policy.json"), &raw).map_err(|e| e.to_string())?;
        write_atomic(&Config::path("policy-serial"), bundle.serial.to_string().as_bytes()).map_err(|e| e.to_string())?;
        log::info!("policy {} applied", bundle.serial);
    }
    Ok(apply(Some(bundle)) || new)
}

/// `POST /api/heartbeat`, best effort.
pub fn heartbeat() {
    let mut body = json!({
        "uuid": install_uuid(),
        "id": librustdesk::ui::api::get_id(),
        "profile": PROFILE,
        "version": env!("CARGO_PKG_VERSION"),
        "os": hbb_common::whoami::distro(),
        "serial": effective().serial,
    });
    // Only with a list: relay APIs from before key rotation reject unknown fields.
    let keys_serial = SERVER_KEYS_SERIAL.load(Ordering::Relaxed);
    if keys_serial > 0 {
        body["server_keys_serial"] = json!(keys_serial);
    }
    let plain = body.clone();
    // Update state only to relays that answered with a release generation: older ones reject
    // unknown fields (400), and then the plain heartbeat goes again.
    let new_relay = RELAY_HAS_RELEASES.load(Ordering::Relaxed);
    if new_relay {
        let (state, serial, version) = crate::update::heartbeat_fields();
        body["release_serial"] = json!(serial);
        body["update_state"] = json!(state);
        if tether_trust::release::valid_version(&version) {
            body["update_version"] = json!(version);
        }
    }
    let mut r = api(|c, base| c.post(format!("{base}/api/heartbeat")).json(&body));
    if new_relay && r.as_ref().is_ok_and(|r| r.status().as_u16() == 400) {
        RELAY_HAS_RELEASES.store(false, Ordering::Relaxed);
        r = api(|c, base| c.post(format!("{base}/api/heartbeat")).json(&plain));
    }
    match r {
        // `{"releases": G}`: the relay's release generation; a change means "check for updates".
        Ok(r) if r.status().as_u16() == 200 => {
            // The relay that answered is first (api_hosts puts the last good one first).
            let host = relay_bases().into_iter().next().map(|h| h.0).unwrap_or_default();
            if let Some(g) = tether_trust::read_limited(r, 1024).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).and_then(|v| v["releases"].as_u64()) {
                RELAY_HAS_RELEASES.store(true, Ordering::Relaxed);
                crate::update::hint(&host, g, 60);
            }
        }
        Ok(r) if r.status().is_success() || r.status().as_u16() == 429 => {}
        Ok(r) => log::debug!("heartbeat: {}", r.status()),
        Err(e) => log::debug!("heartbeat: {e}"),
    }
}

/// The heartbeat relay answered with a release generation (it knows the update fields).
static RELAY_HAS_RELEASES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn interval() -> Duration {
    if on("policy.fast-apply") {
        return Duration::from_secs(60);
    }
    Duration::from_secs(get("policy.refresh-minutes").parse::<u64>().unwrap_or(15).max(1) * 60)
}

/// Background refresh loop. `ui`: this is the main UI process (heartbeat, update checks).
pub fn start(ui: bool) {
    std::thread::spawn(move || loop {
        match refresh() {
            Ok(true) if ui => crate::update::on_policy_change(),
            Ok(_) => {}
            Err(e) => log::info!("policy check: {e}"),
        }
        // Dev builds without admin keys stay out of the production device list.
        if ui && ADMIN_KEYS.is_some_and(|k| !k.trim().is_empty()) {
            heartbeat();
        }
        std::thread::sleep(interval());
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use tether_trust::policy::{Group, Rule, State as S};

    fn bundle_locking_everything() -> Bundle {
        let layer: Layer = FEATURES
            .iter()
            .filter(|f| f.profiles.contains(&PROFILE))
            .map(|f| (f.id.to_owned(), Rule { state: S::Off, value: None }))
            .collect();
        let mut b = Bundle { serial: 3, ..Default::default() };
        b.profiles.insert(PROFILE.into(), layer);
        b.groups.insert("g".into(), Group { profile: PROFILE.into(), overrides: Layer::new() });
        b.validate().unwrap();
        b
    }

    #[test]
    fn floor_and_security_are_never_weakened() {
        crate::security::apply_policy();
        let eff = resolve(Some(&bundle_locking_everything()), PROFILE, "dev", None);
        let mut applied = vec![];
        apply_core(&eff, &mut applied);
        assert!(applied.iter().all(|k| !FLOOR.contains(&k.as_str())));
        assert!(applied.contains(&"enable-clipboard".to_owned()));
        let o = OVERWRITE_SETTINGS.read().unwrap();
        for (k, v) in crate::security::ENFORCED {
            assert_eq!(o.get(*k).map(String::as_str), Some(*v), "{k} weakened");
        }
        assert_eq!(o.get("enable-clipboard").map(String::as_str), Some("N"));
        drop(o);
        // video.codec is a per-peer option: it must be forced where the viewer reads it.
        assert!(applied.contains(&"codec-preference".to_owned()));
        assert!(OVERWRITE_DISPLAY_SETTINGS.read().unwrap().contains_key("codec-preference"));
        assert!(!OVERWRITE_SETTINGS.read().unwrap().contains_key("codec-preference"));
        // Unlocking removes what policy forced; settings.server stays off, so the server options
        // stay blanked.
        let mut applied2 = applied.clone();
        apply_core(&resolve(None, PROFILE, "dev", None), &mut applied2);
        assert!(applied2.is_empty());
        assert_eq!(OVERWRITE_SETTINGS.read().unwrap().get("key").map(String::as_str), Some(""));
        let mut allow = resolve(None, PROFILE, "dev", None);
        allow.values.insert("settings.server".into(), "Y".into());
        apply_core(&allow, &mut applied2);
        assert!(OVERWRITE_SETTINGS.read().unwrap().get("key").is_none());
        assert!(!OVERWRITE_DISPLAY_SETTINGS.read().unwrap().contains_key("codec-preference"));
        assert!(OVERWRITE_SETTINGS.read().unwrap().get("enable-clipboard").is_none());
        crate::security::enforce();
    }

    #[test]
    fn locked_options_are_refused() {
        let eff = resolve(Some(&bundle_locking_everything()), PROFILE, "dev", None);
        let e = check_option_write(&eff, "enable-clipboard").unwrap_err();
        assert!(e.starts_with("locked:permission.clipboard"), "{e}");
        assert!(check_option_write(&eff, "relay-server").unwrap_err().starts_with("locked:settings.server"));
        let defaults = resolve(None, PROFILE, "dev", None);
        assert!(check_option_write(&defaults, "enable-clipboard").is_ok());
        // settings.server defaults to off.
        assert!(check_option_write(&defaults, "key").is_err());
        let mut allow = defaults.clone();
        allow.values.insert("settings.server".into(), "Y".into());
        assert!(check_option_write(&allow, "key").is_ok());
    }

    #[test]
    fn signed_bundles_verify_and_untrusted_ones_do_not() {
        let k = tether_trust::generate_key();
        let payload = serde_json::to_vec(&bundle_locking_everything()).unwrap();
        let doc = serde_json::to_vec(&envelope::sign(&payload, &k)).unwrap();
        assert_eq!(open(&doc, &[k.verifying_key()]).unwrap().serial, 3);
        assert!(open(&doc, &[tether_trust::generate_key().verifying_key()]).is_err());
    }

    /// An oversized answer is cut off after limit + 1 bytes and refused, whatever its length header says.
    #[test]
    fn oversized_response_is_refused_while_reading() {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut s, _) = listener.accept().unwrap();
                let _ = s.read(&mut [0u8; 4096]);
                // Chunked: no Content-Length, so only the reader can stop it.
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n");
                let chunk = vec![b'x'; 4096];
                for _ in 0..1024 {
                    if s.write_all(format!("{:x}\r\n", chunk.len()).as_bytes()).and_then(|_| s.write_all(&chunk)).and_then(|_| s.write_all(b"\r\n")).is_err() {
                        break;
                    }
                }
            }
        });
        let get = || reqwest::blocking::Client::builder().no_proxy().build().unwrap().get(format!("http://{addr}/")).send().unwrap();
        assert!(tether_trust::read_limited(get(), 16 * 1024).unwrap_err().contains("larger than 16384"));
        assert_eq!(tether_trust::read_limited(get().take(100), 16 * 1024).unwrap().len(), 100);
    }

    /// docs/policy.md lists every feature. `TETHER_WRITE_POLICY_DOC=1 cargo test policy_doc`
    /// regenerates it from `FEATURES`.
    #[test]
    fn policy_doc_lists_every_feature() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../docs/policy.md");
        if std::env::var("TETHER_WRITE_POLICY_DOC").is_ok() {
            std::fs::write(&path, render_doc()).unwrap();
        }
        let doc = std::fs::read_to_string(&path).expect("docs/policy.md missing");
        for f in FEATURES {
            assert!(doc.contains(&format!("| `{}` |", f.id)), "docs/policy.md misses {}", f.id);
        }
    }

    fn render_doc() -> String {
        let mut s = String::from(
            "# Policy features\n\n\
             Generated from `tether_trust::policy::FEATURES` by `TETHER_WRITE_POLICY_DOC=1 cargo test policy_doc` \
             in `apps/desktop/src-tauri`; a test fails when a feature is missing here.\n\n\
             States per layer: `on` (user may change, `value` is the default), `off` (locked off), `locked` \
             (locked to `value`), `hidden` (locked off and not shown). Layers, later wins: defaults < profile < \
             group < machine policy (`HKLM\\SOFTWARE\\Policies\\Tether` REG_SZ `PolicyJson`, or \
             `/Library/Managed Preferences/net.tether.app.plist` key `PolicyJson`, both a `Layer` JSON such as \
             `{\"connect.outgoing\":{\"state\":\"off\"}}`). Core options in `FLOOR` (TLS, encryption, 443-only \
             networking, no silent recording) can never be set by policy.\n\n",
        );
        s.push_str("| Feature | Area | Description | Default | Profiles | Core option |\n| --- | --- | --- | --- | --- | --- |\n");
        for f in FEATURES {
            let def = if f.default.is_empty() { "–".to_owned() } else { format!("`{}`", f.default) };
            let choices = if f.choices.is_empty() { String::new() } else { format!(" ({})", f.choices.join(", ")) };
            s.push_str(&format!(
                "| `{}` | {} | {}{} | {} | {} | {} |\n",
                f.id,
                f.area,
                f.en.replace('|', "\\|"),
                choices,
                def,
                f.profiles.join(", "),
                f.option.map(|o| format!("`{o}`")).unwrap_or_else(|| "–".into()),
            ));
        }
        // Hand-written notes after the table survive a regeneration.
        let old = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../docs/policy.md")).unwrap_or_default();
        if let Some(i) = old.find("\n## ") {
            s.push_str(&old[i..]);
        }
        s
    }
}

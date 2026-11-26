//! Staff sign-in with OpenID Connect (design 12.2, 12.6; Microsoft Entra ID is the documented
//! provider). The provider settings come only from the relays' `GET /api/team/config`, signed by
//! the compiled-in admin key and at or above the serial last seen from that relay (a floor kept per
//! relay in `team-config-floors.json`); every relay that answers must name the same provider.
//! Authorization code with PKCE (S256) in the system browser; the redirect goes to
//! `http://localhost:<port>/` (what the app registration names; only the port may vary) and is taken
//! on `127.0.0.1` and `[::1]` only, one request, within a few minutes. The access token lives only in
//! this process's memory while it is posted (not kept after), the refresh token only in the OS
//! credential store (Windows Credential Manager, macOS Keychain). The token is posted to every relay
//! at once, signed with this app's team key and `kid`, and renewed before the relays' sessions end;
//! signing out ends them everywhere. No token or code is ever logged, written to a file or shown.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read as _, Write as _};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};
use sha2::Digest as _;
use tether_trust::team::{Config as TeamConfig, Oidc};
use tether_trust::VerifyingKey;

type R<T> = Result<T, String>;

/// How long the browser has to come back (MFA and an account picker included).
const WAIT: Duration = Duration::from_secs(300);
/// Renew this long before the earliest relay session ends.
const RENEW_BEFORE: u64 = 300;
/// A failed renewal (provider or relays not reached) is tried again after this long.
const RETRY: Duration = Duration::from_secs(60);
const PROVIDER_MAX: usize = 64 << 10;
/// Debug builds take `http://` on a loopback host (the mock provider); release builds only public `https://`.
const RELEASE: bool = !cfg!(debug_assertions);
/// The viewer name the session's login sends (vendor/rustdesk `keys::OPTION_DISPLAY_NAME`).
const DISPLAY_NAME: &str = "display-name";

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn b64url(b: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

/// `n` bytes from the OS generator, base64url.
fn random(n: usize) -> String {
    let mut b = vec![0u8; n];
    getrandom::getrandom(&mut b).expect("the OS random generator failed");
    b64url(&b)
}

/// RFC 7636 S256.
fn challenge(verifier: &str) -> String {
    b64url(&sha2::Sha256::digest(verifier.as_bytes()))
}

/// Text from elsewhere (provider, relay) as one plain line.
fn clip(s: &str, n: usize) -> String {
    s.lines().next().unwrap_or_default().chars().filter(|c| !c.is_control()).take(n).collect()
}

// ----------------------------------------------------------------------------- state

#[derive(Clone)]
struct Endpoints {
    authorize: String,
    token: String,
    logout: Option<String>,
}

#[derive(Clone, serde::Serialize, PartialEq, Debug)]
struct RelayAnswer {
    relay: String,
    ok: bool,
    text: String,
}

struct Live {
    cfg: Oidc,
    ep: Endpoints,
    /// The earliest session end among the relays that accepted it.
    exp: u64,
    me: Value,
}

#[derive(Default)]
struct State {
    live: Option<Live>,
    relays: Vec<RelayAnswer>,
    /// Why the last sign-in failed or the session ended (until the next sign-in).
    note: Option<String>,
    /// A relay said the token was used by another key (design correction 14).
    alarm: Option<String>,
    /// After the last check: the relays' provider (None: no relay offers sign-in), or why none can be used.
    offered: Option<Result<Option<Oidc>, String>>,
    /// Bumped by every sign-in, sign-out and cancel: work of an older one stops and changes nothing.
    gen: u64,
    busy: bool,
    resumed_at: Option<Instant>,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(Mutex::default);
/// Relays (host names) that serve `GET /api/team/config`, so they take `kid` (0.7 and later).
static KID_RELAYS: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn alive(gen: u64) -> bool {
    state().gen == gen
}

pub fn signed_in() -> bool {
    state().live.is_some()
}

/// Whether a request to `base` names its key with `kid`: always with a session (the relay finds a
/// session key only by `kid`), else for relays known to take it (an older relay refuses the header).
pub fn sends_kid(base: &str) -> bool {
    signed_in() || KID_RELAYS.lock().unwrap().contains(&tether_trust::reqauth::host_of(base))
}

/// The signed-in person's permissions (the relay's session answer), if signed in.
pub fn me() -> Option<Value> {
    state().live.as_ref().map(|l| l.me.clone())
}

/// For the UI: what is offered, the person, the relays' answers, notes and the alarm. No token.
pub fn status_json() -> Value {
    let s = state();
    let (offered, problem) = match &s.offered {
        None => (Value::Null, None),
        Some(Ok(o)) => (Value::Bool(o.is_some()), None),
        Some(Err(e)) => (Value::Bool(false), Some(e.clone())),
    };
    let mut v = json!({"offered": offered, "problem": problem, "busy": s.busy, "signed_in": s.live.is_some(), "relays": s.relays, "note": s.note, "alarm": s.alarm});
    if let Some(l) = &s.live {
        for k in ["name", "upn", "roles", "write", "audit", "sbom", "agent", "customers"] {
            v[k] = l.me.get(k).cloned().unwrap_or(Value::Null);
        }
        v["exp"] = l.exp.into();
    }
    v
}

// ----------------------------------------------------------------------------- hooks (tests)

#[cfg(not(test))]
fn admin_keys() -> Vec<VerifyingKey> {
    crate::policy::admin_keys()
}
#[cfg(test)]
static TEST_KEYS: Mutex<Vec<VerifyingKey>> = Mutex::new(Vec::new());
#[cfg(test)]
fn admin_keys() -> Vec<VerifyingKey> {
    TEST_KEYS.lock().unwrap().clone()
}

#[cfg(not(test))]
fn open_browser(url: &str) -> R<()> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
        let w = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
        let (verb, file) = (w("open"), w(url));
        // > 32 is success (ShellExecute's documented convention).
        let r = unsafe { ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) };
        if r as isize > 32 { Ok(()) } else { Err("The web browser could not be opened.".into()) }
    }
    #[cfg(not(windows))]
    {
        let cmd = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        std::process::Command::new(cmd).arg(url).spawn().map(|_| ()).map_err(|_| "The web browser could not be opened.".to_owned())
    }
}
/// Tests: what the browser does with the URL (follow the provider's redirect to the listener).
#[cfg(test)]
#[allow(clippy::type_complexity)]
static TEST_BROWSER: Mutex<Option<Box<dyn Fn(String) + Send>>> = Mutex::new(None);
#[cfg(test)]
fn open_browser(url: &str) -> R<()> {
    if let Some(f) = TEST_BROWSER.lock().unwrap().as_ref() {
        f(url.to_owned());
    }
    Ok(())
}

/// The refresh token in the OS credential store, split over entries of at most 2000 bytes (a
/// Windows credential holds 2560; Entra's refresh tokens are often longer than half of that).
mod secret {
    const SERVICE: &str = "Tether sign-in";
    const CHUNK: usize = 2000;
    const CHUNKS: usize = 8;

    fn user(client_id: &str, i: usize) -> String {
        format!("refresh-token {client_id} {i}")
    }

    pub fn store(client_id: &str, token: &str) -> Result<(), String> {
        delete(client_id);
        if token.len() > CHUNK * CHUNKS {
            return Err("the sign-in is too large for the credential store".into());
        }
        for (i, c) in token.as_bytes().chunks(CHUNK).enumerate() {
            set(&user(client_id, i), c).inspect_err(|_| delete(client_id))?;
        }
        Ok(())
    }

    pub fn load(client_id: &str) -> Option<String> {
        let mut out = Vec::new();
        for i in 0..CHUNKS {
            match get(&user(client_id, i)) {
                Some(c) => out.extend(c),
                None => break,
            }
        }
        String::from_utf8(out).ok().filter(|s| !s.is_empty())
    }

    pub fn delete(client_id: &str) {
        for i in 0..CHUNKS {
            del(&user(client_id, i));
        }
    }

    #[cfg(not(test))]
    fn entry(user: &str) -> Option<keyring::Entry> {
        keyring::Entry::new(SERVICE, user).ok()
    }
    #[cfg(not(test))]
    fn set(user: &str, b: &[u8]) -> Result<(), String> {
        entry(user).ok_or("no credential store")?.set_secret(b).map_err(|e| format!("the credential store refused it: {e}"))
    }
    #[cfg(not(test))]
    fn get(user: &str) -> Option<Vec<u8>> {
        entry(user)?.get_secret().ok()
    }
    #[cfg(not(test))]
    fn del(user: &str) {
        if let Some(e) = entry(user) {
            let _ = e.delete_credential();
        }
    }

    /// Tests: an in-memory store (never the user's real one).
    #[cfg(test)]
    pub static TEST: std::sync::Mutex<std::collections::BTreeMap<String, Vec<u8>>> = std::sync::Mutex::new(std::collections::BTreeMap::new());
    #[cfg(test)]
    fn set(user: &str, b: &[u8]) -> Result<(), String> {
        TEST.lock().unwrap().insert(format!("{SERVICE}/{user}"), b.to_vec());
        Ok(())
    }
    #[cfg(test)]
    fn get(user: &str) -> Option<Vec<u8>> {
        TEST.lock().unwrap().get(&format!("{SERVICE}/{user}")).cloned()
    }
    #[cfg(test)]
    fn del(user: &str) {
        TEST.lock().unwrap().remove(&format!("{SERVICE}/{user}"));
    }
}

// ----------------------------------------------------------------------------- provider settings

/// Whether `u` may be a provider address: https (and in a release build not loopback, private or
/// link-local, nor `localhost`); a debug build also takes http on a loopback host (the mock).
fn endpoint_ok(u: &str, release: bool) -> R<()> {
    let bad = || format!("The sign-in provider's address is not allowed: {}", clip(u, 200));
    let url = url::Url::parse(u).map_err(|_| bad())?;
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err(bad());
    }
    let host = url.host_str().unwrap_or_default().trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    let ip = host.parse::<IpAddr>().ok();
    let local = host == "localhost" || host.ends_with(".localhost") || ip.is_some_and(|ip| !public(ip));
    let loopback = host == "localhost" || ip.is_some_and(|ip| ip.is_loopback());
    match url.scheme() {
        "https" if !(release && local) && !host.is_empty() => Ok(()),
        "http" if !release && loopback => Ok(()),
        _ => Err(bad()),
    }
}

fn public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => {
            let o = a.octets();
            !(a.is_loopback() || a.is_private() || a.is_link_local() || a.is_unspecified() || a.is_broadcast() || a.is_multicast() || o[0] == 0 || o[0] >= 240 || (o[0] == 100 && o[1] & 0xc0 == 64))
        }
        IpAddr::V6(a) => match a.to_ipv4_mapped() {
            Some(v4) => public(IpAddr::V4(v4)),
            None => {
                let s = a.segments()[0];
                !(a.is_loopback() || a.is_unspecified() || a.is_multicast() || s & 0xfe00 == 0xfc00 || s & 0xffc0 == 0xfe80)
            }
        },
    }
}

/// Release builds: the provider's name must not resolve to a loopback or private address either.
/// shortcut: checked once here, the connection resolves again; the relay checks at connect time.
fn resolves_public(u: &str) -> R<()> {
    use std::net::ToSocketAddrs as _;
    let url = url::Url::parse(u).map_err(|e| e.to_string())?;
    let host = url.host_str().unwrap_or_default().trim_start_matches('[').trim_end_matches(']').to_owned();
    let addrs: Vec<_> = (host.as_str(), url.port_or_known_default().unwrap_or(443)).to_socket_addrs().map_err(|e| format!("provider: The sign-in provider cannot be reached: {e}"))?.collect();
    if addrs.is_empty() || addrs.iter().any(|a| !public(a.ip())) {
        return Err(format!("The sign-in provider's address is not allowed: {host} is a local or private address."));
    }
    Ok(())
}

/// One relay's answer to `GET /api/team/config`: its provider (signed by an admin key, not older
/// than `floor`, which rises to it) or none. `kid`: the relay serves the route (0.7 or later).
fn read_offer(code: u16, body: &[u8], keys: &[VerifyingKey], floor: &mut u64) -> R<(Option<Oidc>, bool)> {
    match code {
        200 => {}
        404 if body.starts_with(b"no sign-in configured") => return Ok((None, true)),
        _ => return Ok((None, false)),
    }
    let payload = tether_trust::envelope::open_json(body, keys).map_err(|e| format!("its sign-in settings are not signed by your organisation ({e})"))?;
    let c: TeamConfig = serde_json::from_slice(&payload).map_err(|e| format!("its sign-in settings cannot be read ({e})"))?;
    if c.serial < *floor {
        return Err(format!("it served older sign-in settings (team document {}) than it did before ({})", c.serial, floor));
    }
    *floor = c.serial;
    endpoint_ok(&c.oidc.issuer, RELEASE)?;
    Ok((Some(c.oidc), true))
}

/// Every relay's answer to its provider: the one provider all that answered name (None: none offers
/// sign-in); relays not reached are left out. Refused when a relay's document is unsigned, older
/// than before, or names another provider, or when one offers sign-in and another does not.
fn agree(answers: &[(String, R<(u16, Vec<u8>)>)], keys: &[VerifyingKey], floors: &mut BTreeMap<String, u64>) -> R<(Option<Oidc>, Vec<String>)> {
    let (mut with, mut without, mut kid) = (Vec::<(String, Oidc)>::new(), Vec::new(), Vec::new());
    for (relay, r) in answers {
        let Ok((code, body)) = r else { continue };
        let floor = floors.entry(relay.clone()).or_insert(0);
        let (o, k) = read_offer(*code, body, keys, floor).map_err(|e| format!("Sign-in refused: the server {relay}: {e}."))?;
        if k {
            kid.push(relay.clone());
        }
        match o {
            Some(o) => with.push((relay.clone(), o)),
            None => without.push(relay.clone()),
        }
    }
    if let Some((first, o)) = with.first() {
        if let Some((other, _)) = with.iter().find(|(_, x)| x != o) {
            return Err(format!("Sign-in refused: the servers {first} and {other} name different sign-in settings. Your administrator's new team document has not reached every server yet; try again later."));
        }
        if let Some(other) = without.first() {
            return Err(format!("Sign-in refused: the server {first} offers sign-in and {other} does not. Your administrator's team document has not reached every server yet; try again later."));
        }
    }
    Ok((with.into_iter().next().map(|(_, o)| o), kid))
}

fn floors_path() -> std::path::PathBuf {
    crate::team::data_path("team-config-floors.json")
}

/// Asks every relay for its sign-in settings and records the outcome (also which relays take `kid`).
pub fn check() -> R<Option<Oidc>> {
    let relays: Vec<String> = crate::policy::relay_bases().into_iter().map(|(h, _)| h).collect();
    let answers: Vec<(String, R<(u16, Vec<u8>)>)> = std::thread::scope(|s| {
        let hs: Vec<_> = relays.iter().map(|r| s.spawn(move || crate::team::config_get(r))).collect();
        relays.iter().cloned().zip(hs.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("failed".into())))).collect()
    });
    let mut floors: BTreeMap<String, u64> = std::fs::read(floors_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let before = floors.clone();
    let r = agree(&answers, &admin_keys(), &mut floors);
    if floors != before {
        let _ = crate::policy::write_atomic(&floors_path(), &serde_json::to_vec(&floors).unwrap_or_default());
    }
    if let Ok((_, kid)) = &r {
        KID_RELAYS.lock().unwrap().extend(kid.iter().map(|h| tether_trust::reqauth::host_of(h)));
    }
    let r = r.map(|(o, _)| o);
    state().offered = Some(r.clone());
    r
}

/// The provider's endpoints from its discovery document (its `issuer` must be the configured one).
fn discover(issuer: &str) -> R<Endpoints> {
    if RELEASE {
        resolves_public(issuer)?;
    }
    let url = format!("{}/.well-known/openid-configuration", issuer.trim_end_matches('/'));
    let (code, body) = provider(|c| c.get(&url))?;
    if code != 200 {
        return Err(format!("provider: The sign-in provider answered {code}."));
    }
    let d: Value = serde_json::from_slice(&body).map_err(|_| "provider: The sign-in provider's settings cannot be read.".to_owned())?;
    let s = |k: &str| d.get(k).and_then(Value::as_str).map(str::to_owned);
    if s("issuer").as_deref() != Some(issuer) {
        return Err("provider: The sign-in provider names another issuer than your organisation's settings.".into());
    }
    let ep = |k: &str| s(k).filter(|u| endpoint_ok(u, RELEASE).is_ok()).ok_or_else(|| format!("provider: The sign-in provider's {k} is missing or not allowed."));
    Ok(Endpoints { authorize: ep("authorization_endpoint")?, token: ep("token_endpoint")?, logout: ep("end_session_endpoint").ok() })
}

/// One request to the provider: through the proxy first (if any), then direct; no redirects.
fn provider(build: impl Fn(&reqwest::blocking::Client) -> reqwest::blocking::RequestBuilder) -> R<(u16, Vec<u8>)> {
    let proxy = crate::policy::proxy_url();
    let mut last = String::new();
    for via_proxy in [true, false] {
        if via_proxy && proxy.is_none() {
            continue;
        }
        let mut b = reqwest::blocking::Client::builder().use_rustls_tls().no_proxy().redirect(reqwest::redirect::Policy::none()).connect_timeout(Duration::from_secs(10)).timeout(Duration::from_secs(30)).user_agent(concat!("Tether/", env!("CARGO_PKG_VERSION")));
        if via_proxy {
            b = b.proxy(reqwest::Proxy::all(proxy.as_deref().unwrap_or_default()).map_err(|e| e.to_string())?);
        }
        match build(&b.build().map_err(|e| e.to_string())?).send() {
            Ok(r) => {
                let code = r.status().as_u16();
                return Ok((code, tether_trust::read_limited(r, PROVIDER_MAX)?));
            }
            // reqwest's text names the URL; the form body (code, refresh token) is never in it.
            Err(e) => last = e.without_url().to_string(),
        }
    }
    Err(format!("provider: The sign-in provider cannot be reached: {last}"))
}

// ----------------------------------------------------------------------------- browser and redirect

/// The redirect listener: `127.0.0.1:<port>` and, where possible, `[::1]:<port>`.
struct Loopback {
    v4: TcpListener,
    v6: Option<TcpListener>,
    port: u16,
}

const PAGE_OK: &str = "Sign-in finished. You can close this tab and return to Tether.";
const PAGE_FAIL: &str = "Sign-in did not finish. Return to Tether to see why.";

impl Loopback {
    fn bind() -> R<Self> {
        let err = |e: std::io::Error| format!("Tether cannot wait for the sign-in: {e}");
        let v4 = TcpListener::bind("127.0.0.1:0").map_err(err)?;
        let port = v4.local_addr().map_err(err)?.port();
        let v6 = TcpListener::bind(("::1", port)).ok();
        v4.set_nonblocking(true).map_err(err)?;
        if let Some(l) = &v6 {
            l.set_nonblocking(true).map_err(err)?;
        }
        Ok(Self { v4, v6, port })
    }

    fn redirect_uri(&self) -> String {
        format!("http://localhost:{}/", self.port)
    }

    /// The code from the browser's redirect. Only a `GET /` counts (other paths get 404, a
    /// connection that sends nothing is dropped); it ends the wait and the listeners close.
    fn wait(self, state: &str, deadline: Instant, alive: impl Fn() -> bool) -> R<String> {
        loop {
            if !alive() {
                return Err("cancelled: Sign-in cancelled.".into());
            }
            if Instant::now() > deadline {
                return Err("timeout: The browser did not come back within five minutes. Sign in again.".into());
            }
            let got = [Some(&self.v4), self.v6.as_ref()].into_iter().flatten().find_map(|l| l.accept().ok());
            let Some((mut s, _)) = got else {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            };
            let Some(target) = request_target(&mut s) else { continue };
            let Some(query) = target.strip_prefix("/?").or((target == "/").then_some("")) else {
                answer(&mut s, "404 Not Found", "Not found.");
                continue;
            };
            let q: BTreeMap<String, String> = url::form_urlencoded::parse(query.as_bytes()).into_owned().collect();
            let r = callback(&q, state);
            answer(&mut s, "200 OK", if r.is_ok() { PAGE_OK } else { PAGE_FAIL });
            return r;
        }
    }
}

/// The request line's target of a `GET`, read within 5 s (at most 8 KiB of head).
fn request_target(s: &mut TcpStream) -> Option<String> {
    s.set_nonblocking(false).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut head = Vec::new();
    let mut b = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < 8192 {
        match s.read(&mut b) {
            Ok(0) | Err(_) => break,
            Ok(n) => head.extend_from_slice(&b[..n]),
        }
    }
    let line = String::from_utf8_lossy(&head).lines().next()?.to_owned();
    let mut parts = line.split(' ');
    match (parts.next(), parts.next(), parts.next()) {
        (Some("GET"), Some(t), Some(v)) if v.starts_with("HTTP/") => Some(t.to_owned()),
        _ => None,
    }
}

/// A plain page; the script takes the code out of the address bar and the history entry.
fn answer(s: &mut TcpStream, status: &str, text: &str) {
    let body = format!("<!doctype html><meta charset=\"utf-8\"><title>Tether</title><script>history.replaceState(null, \"\", \"/\")</script><p style=\"font:16px system-ui;margin:3em\">{text}</p>");
    let _ = write!(s, "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}", body.len());
    let _ = s.flush();
}

/// The redirect's parameters: the code when `state` is ours; the provider's refusal in words.
fn callback(q: &BTreeMap<String, String>, state: &str) -> R<String> {
    if q.get("state").map(String::as_str) != Some(state) {
        return Err("The sign-in answer did not belong to this sign-in (state). Sign in again.".into());
    }
    if let Some(e) = q.get("error") {
        let d = q.get("error_description").map(|d| clip(d, 200)).unwrap_or_default();
        return Err(format!("provider: The sign-in provider refused: {}{}", clip(e, 64), if d.is_empty() { String::new() } else { format!(" ({d})") }));
    }
    q.get("code").filter(|c| !c.is_empty()).cloned().ok_or_else(|| "The sign-in answer had no code. Sign in again.".into())
}

fn authorize_url(ep: &Endpoints, cfg: &Oidc, redirect: &str, state: &str, verifier: &str) -> String {
    let mut u = url::Url::parse(&ep.authorize).expect("checked by endpoint_ok");
    u.query_pairs_mut()
        .append_pair("client_id", &cfg.client_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", redirect)
        .append_pair("response_mode", "query")
        .append_pair("scope", &scope(cfg))
        .append_pair("state", state)
        .append_pair("code_challenge", &challenge(verifier))
        .append_pair("code_challenge_method", "S256");
    u.into()
}

/// The API's scope always, so the token is for Tether's API and not for Microsoft Graph (design correction 12).
fn scope(cfg: &Oidc) -> String {
    format!("{} openid offline_access", cfg.scope)
}

// ----------------------------------------------------------------------------- tokens

struct Tokens {
    access: String,
    refresh: Option<String>,
}

/// `Err((final, text))`: `final` when the provider refused (account disabled, consent revoked, an
/// unknown or expired refresh token); otherwise the provider was not reached or answered garbage.
fn token(ep: &Endpoints, form: &[(&str, &str)]) -> Result<Tokens, (bool, String)> {
    let (code, body) = provider(|c| c.post(&ep.token).form(form)).map_err(|e| (false, e))?;
    let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let s = |k: &str| v.get(k).and_then(Value::as_str).filter(|x| !x.is_empty()).map(str::to_owned);
    match (code, s("access_token")) {
        (200, Some(access)) => Ok(Tokens { access, refresh: s("refresh_token") }),
        (400 | 401, _) if s("error").is_some() => {
            let d = s("error_description").map(|d| clip(&d, 200)).unwrap_or_default();
            Err((true, format!("The sign-in provider refused: {}{}", clip(&s("error").unwrap_or_default(), 64), if d.is_empty() { String::new() } else { format!(" ({d})") })))
        }
        _ => Err((false, format!("provider: The sign-in provider answered {code}."))),
    }
}

/// The sentence for a relay's refusal (`reason` of `POST /api/team/session`, docs/api.md).
fn reason_text(reason: &str, relay: &str) -> String {
    match reason {
        "token-used" => format!("This sign-in was already used at {relay}: someone may have your sign-in. Sign out and tell your administrator."),
        "no-role" => "Your account has no Tether role. Ask your administrator to assign one.".into(),
        "denied" => "Your account is blocked for Tether by your administrator.".into(),
        "not-configured" => "This server does not offer sign-in.".into(),
        "v1-token" => "The server takes version 2.0 access tokens only: your administrator must set requestedAccessTokenVersion to 2 on the Tether API registration.".into(),
        "wrong-audience" => "The sign-in was for another application (audience): your administrator must check the API registration and its scope.".into(),
        "wrong-issuer" | "wrong-tenant" | "wrong-client" => "The sign-in came from another organisation or application than the server expects.".into(),
        "expired" | "not-yet-valid" => "The sign-in is outside its validity time: check this computer's clock, then sign in again.".into(),
        "overage" => "Your account is in too many groups for the sign-in to name them: your administrator must assign the Tether roles (app roles) to you or your groups.".into(),
        "provider-unavailable" => "The server cannot reach the sign-in provider right now; try again in a minute.".into(),
        "provider-address" => "The server refuses the sign-in provider's address.".into(),
        "config-changed" => "The sign-in settings changed just now; sign in again.".into(),
        "too-many-sessions" => "The server has too many sessions right now; try again later.".into(),
        r => format!("The server could not accept the sign-in ({}).", clip(r, 32)),
    }
}

/// One relay's answer to the posted session: its session (`name`, permissions, `exp`) or why not.
fn relay_answer(relay: &str, r: R<(u16, Vec<u8>)>) -> (RelayAnswer, Option<Value>, bool) {
    let (ok, text, me, used) = match r {
        Ok((200, b)) => match serde_json::from_slice::<Value>(&b) {
            Ok(v) if v.get("name").is_some_and(Value::is_string) && v.get("exp").is_some_and(Value::is_u64) => (true, "Signed in.".to_owned(), Some(v), false),
            _ => (false, "The server's answer cannot be read.".to_owned(), None, false),
        },
        Ok((code, b)) => match serde_json::from_slice::<Value>(&b).ok().and_then(|v| v.get("reason").and_then(Value::as_str).map(str::to_owned)) {
            Some(reason) => (false, reason_text(&reason, relay), None, reason == "token-used"),
            None => (false, crate::team::error_text(code, &String::from_utf8_lossy(&b)), None, false),
        },
        Err(e) => (false, format!("Not reached: {}", e.trim_start_matches("offline: ")), None, false),
    };
    (RelayAnswer { relay: relay.to_owned(), ok, text }, me, used)
}

/// Posts the token to every relay at once (each with this app's key and `kid`).
fn post_all(access: &str) -> Vec<(RelayAnswer, Option<Value>, bool)> {
    let relays: Vec<String> = crate::policy::relay_bases().into_iter().map(|(h, _)| h).collect();
    std::thread::scope(|s| {
        let hs: Vec<_> = relays.iter().map(|r| s.spawn(move || crate::team::session_post(r, access))).collect();
        relays.iter().zip(hs).map(|(r, h)| relay_answer(r, h.join().unwrap_or_else(|_| Err("failed".into())))).collect()
    })
}

/// Ends the relays' sessions of this key (sign-out, a renewal refused).
fn delete_all() -> Vec<RelayAnswer> {
    let relays: Vec<String> = crate::policy::relay_bases().into_iter().map(|(h, _)| h).collect();
    std::thread::scope(|s| {
        let hs: Vec<_> = relays.iter().map(|r| s.spawn(move || crate::team::session_delete(r))).collect();
        relays
            .iter()
            .zip(hs)
            .map(|(r, h)| match h.join().unwrap_or_else(|_| Err("failed".into())) {
                Ok((200 | 204, _)) => RelayAnswer { relay: r.clone(), ok: true, text: "Signed out.".into() },
                Ok((code, b)) => RelayAnswer { relay: r.clone(), ok: false, text: crate::team::error_text(code, &String::from_utf8_lossy(&b)) },
                Err(e) => RelayAnswer { relay: r.clone(), ok: false, text: format!("Not reached: {}", e.trim_start_matches("offline: ")) },
            })
            .collect()
    })
}

fn set_viewer_name(name: Option<&str>) {
    let mut b = hbb_common::config::BUILTIN_SETTINGS.write().unwrap();
    match name {
        Some(n) => b.insert(DISPLAY_NAME.to_owned(), n.to_owned()),
        None => b.remove(DISPLAY_NAME),
    };
}

/// Posts fresh tokens to the relays and makes them the live session. Err: no relay took them.
fn establish(gen: u64, cfg: Oidc, ep: Endpoints, t: Tokens) -> R<()> {
    let answers = post_all(&t.access);
    let used: Vec<String> = answers.iter().filter(|a| a.2).map(|a| a.0.relay.clone()).collect();
    let accepted: Vec<&Value> = answers.iter().filter_map(|a| a.1.as_ref()).collect();
    let exp = accepted.iter().filter_map(|v| v["exp"].as_u64()).min();
    let me = accepted.first().map(|v| (*v).clone());
    let relays: Vec<RelayAnswer> = answers.iter().map(|a| a.0.clone()).collect();
    let mut s = state();
    if s.gen != gen {
        return Err("cancelled: Sign-in cancelled.".into());
    }
    s.relays = relays.clone();
    if !used.is_empty() {
        s.alarm = Some(reason_text("token-used", &used.join(", ")));
    }
    let (Some(me), Some(exp)) = (me, exp) else {
        drop(s);
        return Err(relays.iter().map(|a| format!("{}: {}", a.relay, a.text)).collect::<Vec<_>>().join(" · "));
    };
    if let Some(rt) = &t.refresh {
        if let Err(e) = secret::store(&cfg.client_id, rt) {
            s.note = Some(format!("Signed in, but {e}: sign in again when this session ends."));
        }
    }
    set_viewer_name(me.get("name").and_then(Value::as_str));
    s.live = Some(Live { cfg, ep, exp, me });
    drop(s);
    for relay in used {
        // A relay that refused the token as used by another key: the person's own record of it.
        crate::team::audit_event(json!({"event": "token-used", "relay": relay}));
    }
    Ok(())
}

/// Takes the busy slot for a new sign-in; its generation.
fn begin() -> R<u64> {
    let mut s = state();
    if s.busy {
        return Err("A sign-in is already running.".into());
    }
    s.busy = true;
    s.gen += 1;
    Ok(s.gen)
}

fn finish(gen: u64, r: &R<()>) {
    let mut s = state();
    if s.gen == gen {
        s.busy = false;
        if let Err(e) = r {
            s.note = Some(e.clone());
        }
    } else if s.busy && r.is_err() {
        // A cancel or sign-out started meanwhile; its own note stands.
        s.busy = false;
    }
}

/// Interactive sign-in through the system browser. Blocks until done, failed or cancelled.
pub fn sign_in() -> R<Value> {
    let gen = begin()?;
    {
        let mut s = state();
        (s.note, s.alarm) = (None, None);
    }
    let r = (|| {
        let cfg = check()?.ok_or("Sign-in is not set up for your team's servers.")?;
        let ep = discover(&cfg.issuer)?;
        let (verifier, st) = (random(32), random(16));
        let lb = Loopback::bind()?;
        let redirect = lb.redirect_uri();
        open_browser(&authorize_url(&ep, &cfg, &redirect, &st, &verifier))?;
        let code = lb.wait(&st, Instant::now() + WAIT, || alive(gen))?;
        let sc = scope(&cfg);
        let t = token(&ep, &[("grant_type", "authorization_code"), ("client_id", &cfg.client_id), ("code", &code), ("redirect_uri", &redirect), ("code_verifier", &verifier), ("scope", &sc)]).map_err(|(_, e)| e)?;
        establish(gen, cfg, ep, t)
    })();
    finish(gen, &r);
    if r.is_ok() {
        std::thread::spawn(move || renew_loop(gen));
    }
    r.map(|_| status_json())
}

/// Stops a sign-in that waits for the browser.
pub fn cancel() -> Value {
    let mut s = state();
    if s.busy {
        s.gen += 1;
        s.busy = false;
        s.note = Some("Sign-in cancelled.".into());
    }
    drop(s);
    status_json()
}

/// One renewal with the stored refresh token. `Err((final, text))` as [`token`].
fn renew(gen: u64, cfg: Oidc, ep: Endpoints) -> Result<(), (bool, String)> {
    let rt = secret::load(&cfg.client_id).ok_or((true, "No stored sign-in to renew.".to_owned()))?;
    let sc = scope(&cfg);
    let t = token(&ep, &[("grant_type", "refresh_token"), ("client_id", &cfg.client_id), ("refresh_token", &rt), ("scope", &sc)])?;
    // Relays not reached: try again; every reached relay refusing (no role, deny list): final.
    establish(gen, cfg, ep, t).map_err(|e| (!e.contains("Not reached") || e.contains("cancelled"), e))
}

/// Ends the live session of generation `gen` with `why` (shown), ending it on the relays too.
fn end(gen: u64, why: String, forget: bool) {
    let mut s = state();
    if s.gen != gen {
        return;
    }
    let cfg = s.live.take().map(|l| l.cfg);
    s.gen += 1;
    s.note = Some(why);
    drop(s);
    set_viewer_name(None);
    if let (true, Some(c)) = (forget, &cfg) {
        secret::delete(&c.client_id);
    }
    let relays = delete_all();
    state().relays = relays;
}

/// Renews before the earliest relay session ends, until signed out or a renewal is refused.
fn renew_loop(gen: u64) {
    loop {
        let live = {
            let s = state();
            s.live.as_ref().filter(|_| s.gen == gen).map(|l| (l.exp, l.cfg.clone(), l.ep.clone()))
        };
        let Some((exp, cfg, ep)) = live else { return };
        while now() + RENEW_BEFORE < exp {
            std::thread::sleep(Duration::from_secs(1));
            if !alive(gen) {
                return;
            }
        }
        match renew(gen, cfg, ep) {
            Ok(()) => {}
            Err((true, e)) => return end(gen, format!("Your sign-in ended: {e}"), true),
            Err((false, e)) if now() >= exp => return end(gen, format!("Your sign-in expired and could not be renewed: {}", e.trim_start_matches("provider: ")), false),
            Err(_) => {
                let until = Instant::now() + RETRY;
                while Instant::now() < until {
                    std::thread::sleep(Duration::from_secs(1));
                    if !alive(gen) {
                        return;
                    }
                }
            }
        }
    }
}

/// The team area's check: the relays' provider, and a session taken up again from the stored
/// refresh token (no browser) when there is none in this process yet.
pub fn status(check_relays: bool) -> Value {
    if check_relays {
        if let Ok(Some(cfg)) = check() {
            resume(cfg);
        }
    }
    status_json()
}

fn resume(cfg: Oidc) {
    {
        let s = state();
        if s.live.is_some() || s.busy || s.resumed_at.is_some_and(|t| t.elapsed() < RETRY) || secret::load(&cfg.client_id).is_none() {
            return;
        }
    }
    let Ok(gen) = begin() else { return };
    state().resumed_at = Some(Instant::now());
    let r = discover(&cfg.issuer).map_err(|e| (false, e)).and_then(|ep| renew(gen, cfg.clone(), ep));
    let shown = r.clone().map_err(|(fin, e)| if fin { format!("Your sign-in ended: {e}") } else { format!("Your sign-in could not be taken up again: {}", e.trim_start_matches("provider: ")) });
    if let Err((true, _)) = &r {
        secret::delete(&cfg.client_id);
    }
    finish(gen, &shown);
    if r.is_ok() {
        std::thread::spawn(move || renew_loop(gen));
    }
}

/// Signs out: forgets the stored refresh token, ends the session on every relay and opens the
/// provider's sign-out page.
pub fn sign_out() -> Value {
    let mut s = state();
    s.gen += 1;
    s.busy = false;
    let live = s.live.take();
    let cfg = live.as_ref().map(|l| l.cfg.clone()).or_else(|| s.offered.clone().and_then(Result::ok).flatten());
    (s.note, s.alarm) = (Some("Signed out.".into()), None);
    drop(s);
    set_viewer_name(None);
    if let Some(c) = &cfg {
        secret::delete(&c.client_id);
    }
    let relays = delete_all();
    state().relays = relays;
    if let Some(u) = live.and_then(|l| l.ep.logout) {
        let _ = open_browser(&u);
    }
    status_json()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Tests that change the sign-in state, the dev API variable or the test hooks.
    pub(crate) static SERIAL: Mutex<()> = Mutex::new(());

    fn oidc(iss: &str) -> Oidc {
        Oidc { issuer: iss.into(), tenant: "t".into(), audience: "a".into(), client_id: "c".into(), scope: "api://x/access".into() }
    }

    fn signed(k: &tether_trust::SigningKey, serial: u64, iss: &str) -> Vec<u8> {
        serde_json::to_vec(&tether_trust::envelope::sign(&serde_json::to_vec(&TeamConfig { serial, oidc: oidc(iss) }).unwrap(), k)).unwrap()
    }

    #[test]
    fn pkce_values() {
        let v = random(32);
        assert_eq!(v.len(), 43);
        assert!(v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
        assert_ne!(v, random(32));
        // RFC 7636 appendix B.
        assert_eq!(challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        let ep = Endpoints { authorize: "https://login.example/t/oauth2/v2.0/authorize".into(), token: String::new(), logout: None };
        let u = url::Url::parse(&authorize_url(&ep, &oidc("https://login.example/t/v2.0"), "http://localhost:5000/", "st", &v)).unwrap();
        let q: BTreeMap<String, String> = u.query_pairs().into_owned().collect();
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["code_challenge"], challenge(&v));
        assert_eq!(q["redirect_uri"], "http://localhost:5000/");
        assert_eq!(q["scope"], "api://x/access openid offline_access");
        assert!(!u.as_str().contains(&v), "the verifier never leaves this process before the token request");
    }

    #[test]
    fn provider_addresses() {
        for u in ["https://login.microsoftonline.com/x/v2.0", "https://idp.example.org"] {
            assert!(endpoint_ok(u, true).is_ok(), "{u}");
        }
        for u in ["http://login.microsoftonline.com/x", "https://localhost/x", "https://a.localhost", "https://127.0.0.1/", "https://10.1.2.3/", "https://192.168.0.1", "https://169.254.169.254", "https://100.64.0.1", "https://[::1]/", "https://[fd00::1]/", "https://[::ffff:127.0.0.1]/", "https://u:p@idp.example.org", "https://idp.example.org/#x", "ftp://idp.example.org", "not a url"] {
            assert!(endpoint_ok(u, true).is_err(), "{u}");
        }
        // The mock in debug builds: http on loopback only.
        assert!(endpoint_ok("http://127.0.0.1:5000/t/v2.0", false).is_ok());
        assert!(endpoint_ok("http://localhost:5000/t/v2.0", false).is_ok());
        assert!(endpoint_ok("http://10.0.0.1:5000/t/v2.0", false).is_err());
        assert!(endpoint_ok("http://idp.example.org", false).is_err());
    }

    #[test]
    fn config_must_be_signed_agree_and_not_go_back() {
        let (admin, other) = (tether_trust::generate_key(), tether_trust::generate_key());
        let keys = [admin.verifying_key()];
        let ok = |b: Vec<u8>| Ok((200, b));
        let mut floors = BTreeMap::new();
        // Two relays, same provider: taken; floors rise; both take kid.
        let a = [("r1".to_owned(), ok(signed(&admin, 5, "https://idp.example.org/t/v2.0"))), ("r2".to_owned(), ok(signed(&admin, 6, "https://idp.example.org/t/v2.0")))];
        let (o, kid) = agree(&a, &keys, &mut floors).unwrap();
        assert_eq!((o.unwrap().issuer.as_str(), kid.len(), floors["r1"], floors["r2"]), ("https://idp.example.org/t/v2.0", 2, 5, 6));
        // An older document from a relay than it served before: refused, floor unchanged.
        let e = agree(&[("r2".to_owned(), ok(signed(&admin, 5, "https://idp.example.org/t/v2.0")))], &keys, &mut floors).unwrap_err();
        assert!(e.contains("r2") && e.contains("older"), "{e}");
        assert_eq!(floors["r2"], 6);
        // Signed by a key that is not the admin key, or not signed at all.
        assert!(agree(&[("r1".to_owned(), ok(signed(&other, 9, "https://idp.example.org/t/v2.0")))], &keys, &mut floors).unwrap_err().contains("not signed by your organisation"));
        assert!(agree(&[("r1".to_owned(), ok(br#"{"serial":9,"oidc":{}}"#.to_vec()))], &keys, &mut floors).unwrap_err().contains("not signed"));
        // Two providers, or one relay with and one without sign-in.
        let e = agree(&[("r1".to_owned(), ok(signed(&admin, 7, "https://idp.example.org/t/v2.0"))), ("r2".to_owned(), ok(signed(&admin, 7, "https://other.example.org/t/v2.0")))], &keys, &mut floors).unwrap_err();
        assert!(e.contains("different sign-in settings"), "{e}");
        let e = agree(&[("r1".to_owned(), ok(signed(&admin, 7, "https://idp.example.org/t/v2.0"))), ("r2".to_owned(), Ok((404, b"no sign-in configured".to_vec())))], &keys, &mut floors).unwrap_err();
        assert!(e.contains("r2 does not"), "{e}");
        // No relay offers it: none, and the roster works as before. An old relay (no route) takes no kid;
        // one not reached is left out.
        let (o, kid) = agree(&[("r1".to_owned(), Ok((404, b"no sign-in configured".to_vec()))), ("r2".to_owned(), Ok((404, b"not found".to_vec()))), ("r3".to_owned(), Err("offline: x".into()))], &keys, &mut floors).unwrap();
        assert_eq!((o, kid), (None, vec!["r1".to_owned()]));
        // A release build refuses a loopback or http issuer even when signed.
        if RELEASE {
            assert!(agree(&[("r9".to_owned(), ok(signed(&admin, 1, "http://127.0.0.1:1/t/v2.0")))], &keys, &mut BTreeMap::new()).is_err());
        }
        let mut f = 0;
        assert!(read_offer(200, &signed(&admin, 1, "http://127.0.0.1:1/t/v2.0"), &keys, &mut f).is_ok() != RELEASE);
    }

    fn get(port: u16, v6: bool, target: &str) -> String {
        let mut s = if v6 { TcpStream::connect(("::1", port)).unwrap() } else { TcpStream::connect(("127.0.0.1", port)).unwrap() };
        write!(s, "GET {target} HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n").unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        out
    }

    #[test]
    fn the_redirect_is_taken_once_on_loopback_and_checks_state() {
        // A good redirect (after a connection that sends nothing and a favicon request).
        let lb = Loopback::bind().unwrap();
        let (port, v6) = (lb.port, lb.v6.is_some());
        assert_eq!(lb.redirect_uri(), format!("http://localhost:{port}/"));
        assert!(lb.v4.local_addr().unwrap().ip().is_loopback());
        let t = std::thread::spawn(move || lb.wait("st4te", Instant::now() + Duration::from_secs(20), || true));
        drop(TcpStream::connect(("127.0.0.1", port)).unwrap());
        assert!(get(port, false, "/favicon.ico").starts_with("HTTP/1.1 404"));
        let page = get(port, v6, "/?code=c0de&state=st4te");
        assert!(page.starts_with("HTTP/1.1 200") && page.contains(PAGE_OK) && page.contains("history.replaceState") && page.contains("no-store") && !page.contains("c0de"), "{page}");
        assert_eq!(t.join().unwrap().unwrap(), "c0de");
        // The listeners are closed after the one request.
        assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
        // Another state: refused, and the page says to go back to Tether.
        let lb = Loopback::bind().unwrap();
        let port = lb.port;
        let t = std::thread::spawn(move || lb.wait("st4te", Instant::now() + Duration::from_secs(20), || true));
        assert!(get(port, false, "/?code=c0de&state=other").contains(PAGE_FAIL));
        assert!(t.join().unwrap().unwrap_err().contains("(state)"));
        // The provider's refusal, as words; a deadline; a cancel.
        let q = |s: &str| url::form_urlencoded::parse(s.as_bytes()).into_owned().collect::<BTreeMap<String, String>>();
        let e = callback(&q("error=access_denied&error_description=AADSTS50057%3A+User+account+is+disabled.%0D%0ATrace+ID%3A+x&state=s"), "s").unwrap_err();
        assert_eq!(e, "provider: The sign-in provider refused: access_denied (AADSTS50057: User account is disabled.)");
        assert!(callback(&q("state=s"), "s").unwrap_err().contains("no code"));
        assert!(Loopback::bind().unwrap().wait("s", Instant::now(), || true).unwrap_err().starts_with("timeout:"));
        assert!(Loopback::bind().unwrap().wait("s", Instant::now() + WAIT, || false).unwrap_err().starts_with("cancelled:"));
    }

    #[test]
    fn relay_refusals_read_as_sentences() {
        let (a, _, used) = relay_answer("relay.example", Ok((409, br#"{"reason":"token-used","message":"x"}"#.to_vec())));
        assert!(!a.ok && used && a.text == "This sign-in was already used at relay.example: someone may have your sign-in. Sign out and tell your administrator.", "{a:?}");
        for (code, reason, words) in [(403, "no-role", "no Tether role"), (403, "denied", "blocked"), (401, "overage", "too many groups"), (503, "provider-unavailable", "try again in a minute"), (401, "v1-token", "requestedAccessTokenVersion"), (401, "bad-signature", "(bad-signature)")] {
            let (a, me, used) = relay_answer("r", Ok((code, format!(r#"{{"reason":"{reason}","message":"m"}}"#).into_bytes())));
            assert!(!a.ok && me.is_none() && !used && a.text.contains(words), "{reason}: {a:?}");
        }
        let (a, me, _) = relay_answer("r", Ok((200, br#"{"name":"Anna","exp":5,"sbom":true}"#.to_vec())));
        assert!(a.ok && me.unwrap()["name"] == "Anna");
        assert!(relay_answer("r", Err("offline: refused".into())).0.text.starts_with("Not reached: refused"));
        assert!(relay_answer("r", Ok((401, b"kid must name the key in the body".to_vec()))).0.text.contains("not in your organisation's team"));
    }

    type Log = std::sync::Arc<Mutex<Vec<(String, String, String)>>>;
    type Flag = std::sync::Arc<std::sync::atomic::AtomicBool>;
    type Secs = std::sync::Arc<std::sync::atomic::AtomicU64>;

    /// A stand-in relay of 0.7: the signed config, sessions (signature and `kid` checked; the token's
    /// claims read without a check, which is server/api's own test), `me`, and every request in the
    /// log (`"METHOD path"`, `Tether-Sig`, body). The flag: it answers `409 token-used`; the seconds:
    /// how long its sessions last.
    fn stand_in_relay(config: Vec<u8>) -> (String, Log, Flag, Secs) {
        use std::sync::atomic::Ordering::Relaxed;
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        let (log, used, life): (Log, Flag, Secs) = Default::default();
        life.store(3600, Relaxed);
        let (log2, used2, life2) = (log.clone(), used.clone(), life.clone());
        std::thread::spawn(move || {
            let mut sessions: BTreeMap<String, Value> = BTreeMap::new();
            for s in l.incoming() {
                let Ok(mut s) = s else { continue };
                let mut head = Vec::new();
                let mut b = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") && s.read(&mut b).unwrap_or(0) == 1 {
                    head.push(b[0]);
                }
                let head = String::from_utf8_lossy(&head).to_string();
                let hdr = |n: &str| head.lines().find_map(|x| x.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case(n)).map(|(_, v)| v.trim().to_owned()));
                let mut body = vec![0u8; hdr("content-length").and_then(|n| n.parse().ok()).unwrap_or(0)];
                let _ = s.read_exact(&mut body);
                let mut rl = head.split(' ');
                let (method, path) = (rl.next().unwrap_or_default().to_owned(), rl.next().unwrap_or_default().to_owned());
                let sig = hdr("tether-sig").unwrap_or_default();
                log2.lock().unwrap().push((format!("{method} {path}"), sig.clone(), String::from_utf8_lossy(&body).to_string()));
                let kid = tether_trust::reqauth::header_kid(&sig).map(str::to_owned).unwrap_or_default();
                let (code, out): (u16, Vec<u8>) = match (method.as_str(), path.as_str()) {
                    ("GET", "/api/team/config") => (200, config.clone()),
                    ("POST", "/api/team/session") => {
                        let v: Value = serde_json::from_slice(&body).unwrap_or_default();
                        let key = tether_trust::key_from_b64(v["key"].as_str().unwrap_or_default()).unwrap();
                        let fields: Vec<&String> = v.as_object().unwrap().keys().collect();
                        let signed = tether_trust::reqauth::verify_request_for(&sig, &key, tether_trust::reqauth::Relay { hosts: &["127.0.0.1".to_owned()], v1: false }, "POST", &path, &tether_trust::reqauth::body_digest(&body), now(), |_| true);
                        let claims: Value = v["access_token"].as_str().and_then(|t| t.split('.').nth(1)).and_then(|p| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
                        let roles: Vec<&str> = claims["roles"].as_array().map(|r| r.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
                        if fields != ["access_token", "key"] || kid != tether_trust::team::key_id(&key) || signed.is_err() {
                            (401, b"kid must name the key in the body".to_vec())
                        } else if used2.load(Relaxed) {
                            (409, br#"{"reason":"token-used","message":"this token is in use with another key"}"#.to_vec())
                        } else if !roles.contains(&"Tether.Technician") {
                            (403, br#"{"reason":"no-role","message":"none of your roles is mapped"}"#.to_vec())
                        } else {
                            let a = json!({"name": claims["name"], "write": roles.contains(&"Tether.Technician.Write"), "audit": roles.contains(&"Tether.SessionAudit"), "sbom": roles.contains(&"Tether.Software"), "agent": roles.contains(&"Tether.Agent"), "customers": ["*"], "service": false, "oid": claims["oid"], "upn": claims["preferred_username"], "exp": now() + life2.load(Relaxed), "roles": roles});
                            sessions.insert(kid, a.clone());
                            (200, a.to_string().into_bytes())
                        }
                    }
                    ("DELETE", "/api/team/session") => {
                        sessions.remove(&kid);
                        (204, vec![])
                    }
                    ("GET", "/api/team/me") => sessions.get(&kid).map_or((401, b"not a team member".to_vec()), |a| (200, a.to_string().into_bytes())),
                    ("POST", "/api/team/audit-event") => (204, vec![]),
                    _ => (404, b"not found".to_vec()),
                };
                let _ = write!(s, "HTTP/1.1 {code} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", out.len());
                let _ = s.write_all(&out);
            }
        });
        (base, log, used, life)
    }

    fn until(what: &str, f: impl Fn() -> bool) {
        let t = Instant::now();
        while !f() {
            assert!(t.elapsed() < Duration::from_secs(30), "timed out: {what}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn viewer_name() -> Option<String> {
        hbb_common::config::BUILTIN_SETTINGS.read().unwrap().get(DISPLAY_NAME).cloned()
    }

    /// Every file under `dir`, with its bytes.
    fn files(dir: &std::path::Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(files(&p));
            } else {
                out.push((p.clone(), std::fs::read(&p).unwrap_or_default()));
            }
        }
        out
    }

    /// The whole path against `tools/oidc-mock` and two stand-in relays: sign-in through the
    /// (simulated) browser, the session posted to both relays with `kid`, permissions, the viewer
    /// name, no token in any file, renewal, a refused renewal, sign-out, a token used elsewhere, no role.
    #[test]
    fn sign_in_renew_and_sign_out_against_the_mock_provider() {
        use std::sync::atomic::Ordering::Relaxed;
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("tether-signin-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        *crate::team::TEST_DIR.lock().unwrap() = Some(dir.clone());
        let admin = tether_trust::generate_key();
        *TEST_KEYS.lock().unwrap() = vec![admin.verifying_key()];
        let m = oidc_mock::Mock::start(oidc_mock::Config::default(), "127.0.0.1:0", false).unwrap();
        let mc = m.config().clone();
        let cfg = Oidc { issuer: m.issuer(), tenant: mc.tenant, audience: mc.audience, client_id: mc.client_id, scope: mc.scope };
        let doc = serde_json::to_vec(&tether_trust::envelope::sign(&serde_json::to_vec(&TeamConfig { serial: 3, oidc: cfg.clone() }).unwrap(), &admin)).unwrap();
        let (a, log_a, used_a, life_a) = stand_in_relay(doc.clone());
        let (b, log_b, used_b, life_b) = stand_in_relay(doc);
        std::env::set_var("TETHER_DEV_API", format!("{a},{b}"));
        // The browser: follows the provider's redirect (the mock signs in the `login` user) to the listener.
        let opened: std::sync::Arc<Mutex<Vec<String>>> = Default::default();
        let o2 = opened.clone();
        *TEST_BROWSER.lock().unwrap() = Some(Box::new(move |url: String| {
            o2.lock().unwrap().push(url.clone());
            if url.contains("/authorize") {
                std::thread::spawn(move || {
                    let c = reqwest::blocking::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build().unwrap();
                    let r = c.get(&url).send().unwrap();
                    let to = r.headers().get("location").unwrap().to_str().unwrap().to_owned();
                    assert!(to.starts_with("http://localhost:"), "{to}");
                    let page = c.get(&to).send().unwrap().text().unwrap();
                    assert!(page.contains(PAGE_OK), "{page}");
                });
            }
        }));
        let posts = |log: &Log| log.lock().unwrap().iter().filter(|(r, _, _)| r == "POST /api/team/session").cloned().collect::<Vec<_>>();
        let deletes = |log: &Log| log.lock().unwrap().iter().filter(|(r, _, _)| r == "DELETE /api/team/session").count();

        // Technician + Software: a session at both relays, sbom but not agent. The relays' sessions
        // last 302 s, so the renewal (300 s before the end) comes within seconds.
        life_a.store(302, Relaxed);
        life_b.store(302, Relaxed);
        m.control("login", json!({"user": "software@tether.test"})).unwrap();
        let v = sign_in().unwrap();
        let rt = secret::load(&cfg.client_id).unwrap();
        assert_eq!((&v["signed_in"], &v["name"], &v["sbom"], &v["agent"], v["relays"].as_array().unwrap().iter().filter(|r| r["ok"] == true).count()), (&json!(true), &json!("Software Technician"), &json!(true), &json!(false), 2), "{v}");
        let url = opened.lock().unwrap()[0].clone();
        assert!(url.contains("code_challenge_method=S256") && url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A") && url.contains("offline_access"), "{url}");
        for log in [&log_a, &log_b] {
            let p = posts(log);
            assert!(p[0].1.starts_with("v2 ") && p[0].1.contains(" kid="), "{}", p[0].1);
        }
        assert_eq!(viewer_name().as_deref(), Some("Software Technician"));
        assert!(crate::team::allows("sbom") && !crate::team::allows("agent"));
        // Every team request now names the key (the relay finds a session key only by kid).
        assert_eq!(crate::team::me().unwrap()["name"], "Software Technician");
        let me_rq = log_a.lock().unwrap().iter().chain(log_b.lock().unwrap().iter()).find(|(r, _, _)| r == "GET /api/team/me").cloned().unwrap();
        assert!(me_rq.1.contains(" kid="));
        // The refresh token only in the credential store; no token in any file of the profile.
        let access: String = serde_json::from_str::<Value>(&posts(&log_a)[0].2).unwrap()["access_token"].as_str().unwrap().to_owned();
        let floors: BTreeMap<String, u64> = serde_json::from_slice(&std::fs::read(floors_path()).unwrap()).unwrap();
        assert_eq!(floors.values().copied().collect::<Vec<_>>(), [3, 3]);
        assert!(files(&dir).len() >= 2, "the key and the floors");
        for (p, bytes) in files(&dir) {
            let text = String::from_utf8_lossy(&bytes);
            assert!(!text.contains(&rt) && !text.contains(&access[access.len() - 40..]), "a token in {}", p.display());
        }
        assert!(!status_json().to_string().contains(&access[access.len() - 40..]));

        // Renewal before the session ends: a new token posted to both relays, the stored one replaced.
        until("a renewal at both relays", || posts(&log_a).len() >= 3 && posts(&log_b).len() >= 3);
        assert_ne!(posts(&log_a)[0].2, posts(&log_a)[2].2, "a new access token");
        assert_ne!(secret::load(&cfg.client_id), Some(rt), "the stored refresh token was replaced");
        // The account disabled: the next renewal is refused; the session ends with the reason, everywhere.
        m.control("user", json!({"upn": "software@tether.test", "disabled": true})).unwrap();
        until("the session ended", || !signed_in());
        let v = status_json();
        assert!(v["note"].as_str().unwrap().contains("disabled"), "{v}");
        until("sign-out at both relays", || deletes(&log_a) == 1 && deletes(&log_b) == 1);
        assert_eq!((secret::load(&cfg.client_id), viewer_name()), (None, None));
        m.control("user", json!({"upn": "software@tether.test", "disabled": false})).unwrap();
        life_a.store(3600, Relaxed);
        life_b.store(3600, Relaxed);

        // A person with every permission; then sign-out: the stored token gone, both relays told,
        // the provider's sign-out page opened.
        m.control("login", json!({"user": "writer@tether.test"})).unwrap();
        let v = sign_in().unwrap();
        assert_eq!((&v["agent"], &v["write"]), (&json!(true), &json!(true)));
        assert!(crate::team::allows("agent") && secret::load(&cfg.client_id).is_some());
        let v = sign_out();
        assert_eq!((&v["signed_in"], secret::load(&cfg.client_id), viewer_name()), (&json!(false), None, None));
        assert_eq!((deletes(&log_a), deletes(&log_b)), (2, 2));
        assert!(opened.lock().unwrap().last().unwrap().contains("/logout"));
        // Every DELETE names the key too.
        assert!(log_a.lock().unwrap().iter().filter(|(r, _, _)| r.starts_with("DELETE")).all(|(_, s, _)| s.contains(" kid=")));

        // One relay says the token was used with another key: signed in at the other, with the alarm,
        // and the person's own record of it posted.
        used_b.store(true, Relaxed);
        let v = sign_in().unwrap();
        let alarm = v["alarm"].as_str().unwrap();
        assert!(alarm.starts_with(&format!("This sign-in was already used at {b}")) && alarm.contains("tell your administrator"), "{v}");
        assert!(v["relays"].as_array().unwrap().iter().any(|r| r["relay"] == b.as_str() && r["ok"] == false));
        until("the token-used event", || log_a.lock().unwrap().iter().chain(log_b.lock().unwrap().iter()).any(|(r, _, body)| r == "POST /api/team/audit-event" && body.contains("token-used")));
        sign_out();
        used_b.store(false, Relaxed);
        assert!(!used_a.load(Relaxed));

        // No role: refused with the reason at each relay, nothing stored.
        m.control("login", json!({"user": "noroles@tether.test"})).unwrap();
        let e = sign_in().unwrap_err();
        assert!(e.contains("no Tether role") && e.contains(&a) && e.contains(&b), "{e}");
        assert!(!signed_in() && secret::load(&cfg.client_id).is_none());
        assert!(status_json()["note"].as_str().unwrap().contains("no Tether role"));

        std::env::remove_var("TETHER_DEV_API");
        *crate::team::ME.lock().unwrap() = None;
        *TEST_BROWSER.lock().unwrap() = None;
        *crate::team::TEST_DIR.lock().unwrap() = None;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// One line of a child's stdout that starts with `prefix` (the rest of it).
    fn line_of(r: &mut impl std::io::BufRead, prefix: &str) -> String {
        let mut l = String::new();
        loop {
            l.clear();
            assert!(r.read_line(&mut l).unwrap() > 0, "no line {prefix}");
            if let Some(x) = l.trim().strip_prefix(prefix) {
                return x.to_owned();
            }
        }
    }

    /// A shell command in WSL (`Ubuntu-22.04`; no quotes or own variables: wsl.exe's shell expands it first); its stdout.
    fn wsl(cmd: &str) -> (std::process::Child, std::io::BufReader<std::process::ChildStdout>) {
        let mut c = std::process::Command::new("wsl").args(["-d", "Ubuntu-22.04", "--", "bash", "-lc", cmd]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn().unwrap();
        let out = std::io::BufReader::new(c.stdout.take().unwrap());
        (c, out)
    }

    /// The plan's end-to-end run (C6) against real relays: `tools/oidc-mock` and two `tether-api`
    /// debug builds in WSL (`~/c6e2e/{oidc-mock,tether-api}`, copied from their build folders),
    /// the desktop's own code here. Sign in, the host list, Software only with `sbom`, sign-out.
    /// By hand: `TETHER_E2E_WSL=1 cargo test e2e_against_local_relays -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn e2e_against_local_relays() {
        if std::env::var("TETHER_E2E_WSL").is_err() {
            return;
        }
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("tether-signin-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        *crate::team::TEST_DIR.lock().unwrap() = Some(dir.clone());
        let admin = tether_trust::generate_key();
        *TEST_KEYS.lock().unwrap() = vec![admin.verifying_key()];
        let (mock, mut mo) = wsl("exec timeout 600 $HOME/c6e2e/oidc-mock --listen 127.0.0.1:0");
        let maddr = line_of(&mut mo, "listening on ");
        let issuer = line_of(&mut mo, "issuer ");
        let mock_base = format!("http://{maddr}");
        let d = oidc_mock::Config::default();
        let oidc = json!({"issuer": issuer, "tenant": d.tenant, "audience": d.audience, "client_id": d.client_id, "scope": d.scope});
        let pk = tether_trust::key_to_b64(&admin.verifying_key());
        let mut relays = Vec::new();
        let mut kids = Vec::new();
        for i in 0..2 {
            let (c, mut o) = wsl(&format!("rm -rf $HOME/c6e2e/relay{i}; mkdir -p $HOME/c6e2e/relay{i}; ADMIN_PUB_KEY={pk} RELAY_HOST=127.0.0.1 STATUS_SBOM=on exec timeout 600 $HOME/c6e2e/tether-api --data $HOME/c6e2e/relay{i} --listen 127.0.0.1:0"));
            relays.push(format!("http://{}", line_of(&mut o, "listening on ")));
            kids.push(c);
        }
        let http = reqwest::blocking::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build().unwrap();
        // The signed team document with the provider and the role table, on both relays.
        let doc = json!({"serial": 1, "members": [], "oidc": oidc, "roles": {"Tether.Technician": {}, "Tether.Software": {"sbom": true}, "Tether.Agent": {"agent": true}}});
        let up = json!({"team": tether_trust::envelope::sign(&serde_json::to_vec(&doc).unwrap(), &admin), "config": tether_trust::envelope::sign(&serde_json::to_vec(&json!({"serial": 1, "oidc": oidc})).unwrap(), &admin)}).to_string();
        for r in &relays {
            let sig = tether_trust::reqauth::sign_request_for(&admin, r, "PUT", "/api/admin/team", up.as_bytes(), now());
            let a = http.put(format!("{r}/api/admin/team")).header(tether_trust::reqauth::HEADER, sig).body(up.clone()).send().unwrap();
            assert_eq!(a.status().as_u16(), 204, "{}", a.text().unwrap());
        }
        std::env::set_var("TETHER_DEV_API", relays.join(","));
        *TEST_BROWSER.lock().unwrap() = Some(Box::new(move |url: String| {
            if url.contains("/authorize") {
                std::thread::spawn(move || {
                    let c = reqwest::blocking::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build().unwrap();
                    let to = c.get(&url).send().unwrap().headers().get("location").unwrap().to_str().unwrap().to_owned();
                    let _ = c.get(&to).send();
                });
            }
        }));
        let login = |u: &str| assert!(http.post(format!("{mock_base}/control/login")).json(&json!({"user": u})).send().unwrap().status().is_success());
        let dev = "0123456789abcdef0123456789abcdef";

        // Technician + Software: a session at both relays; the host list; Software asked (none stored: 404, not 403).
        login("software@tether.test");
        let v = sign_in().unwrap();
        println!("signed in: {v}");
        assert_eq!((&v["name"], &v["sbom"], v["relays"].as_array().unwrap().iter().filter(|r| r["ok"] == true).count()), (&json!("Software Technician"), &json!(true), 2), "{v}");
        assert_eq!(crate::team::me().unwrap()["name"], "Software Technician");
        let st = crate::team::status().unwrap();
        println!("host list: {st}");
        assert!(st.as_array().unwrap().iter().all(|r| r.get("hosts").is_some() && r.get("error").is_none()), "{st}");
        let e = crate::team::status_sbom(&relays[0], dev).unwrap_err();
        println!("Software with sbom: {e}");
        assert!(e.starts_with("none:"), "{e}");
        sign_out();
        assert!(crate::team::me().is_err(), "the session is gone at the relay");

        // Technician only: the relay refuses the software list (403).
        login("technician@tether.test");
        let v = sign_in().unwrap();
        assert_eq!(&v["sbom"], &json!(false), "{v}");
        let e = crate::team::status_sbom(&relays[1], dev).unwrap_err();
        println!("Software without sbom: {e}");
        assert!(e.contains("does not allow"), "{e}");
        sign_out();

        // No role: refused by both relays with the sentence.
        login("noroles@tether.test");
        let e = sign_in().unwrap_err();
        println!("no role: {e}");
        assert!(e.matches("no Tether role").count() == 2, "{e}");

        std::env::remove_var("TETHER_DEV_API");
        *crate::team::ME.lock().unwrap() = None;
        *TEST_BROWSER.lock().unwrap() = None;
        *crate::team::TEST_DIR.lock().unwrap() = None;
        for mut c in kids.into_iter().chain([mock]) {
            let _ = c.kill();
        }
        let _ = std::process::Command::new("wsl").args(["-d", "Ubuntu-22.04", "--", "bash", "-lc", "pkill -f c6e2e/[ot][ie][dt]; rm -rf $HOME/c6e2e/relay*"]).status();
        let _ = std::fs::remove_dir_all(&dir);
    }
}

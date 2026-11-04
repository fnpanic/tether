//! Network policy: every connection Tether makes leaves on TCP 443 as TLS/WebSocket, so it works
//! behind corporate proxies and zero-trust filters (e.g. Zscaler) that only allow HTTPS.
//!
//! - Rendezvous and relay run over `wss://<relay>/ws/id` and `wss://<relay>/ws/relay` (443).
//! - No UDP to the internet: no STUN, NAT tests, hole punching or WebRTC/ICE.
//! - Direct on the LAN (policy `network.direct`, default auto): the viewer asks the LAN for the
//!   host (UDP broadcast lookup) beside the relay request and races a direct TCP connection
//!   against the relay; the key exchange is the same on both, the first secured path wins.
//! - Otherwise sessions are relayed (WebSocket mode forces relay); end-to-end encryption between the
//!   two peers is unchanged, the relay and any TLS-inspecting proxy only see ciphertext.
//! - The system proxy (env, Windows static/PAC/WPAD, macOS) is detected and used automatically;
//!   a proxy configured explicitly by the user still wins.

use std::{collections::HashMap, time::Duration};

use hbb_common::config::{DEFAULT_SETTINGS, OVERWRITE_LOCAL_SETTINGS, OVERWRITE_SETTINGS};

/// Relay host compiled into this build (also used by hbb_common for the rendezvous server).
const RELAY: Option<&str> = option_env!("TETHER_SERVER");

pub fn apply() {
    let mut o = OVERWRITE_SETTINGS.write().unwrap();
    for (k, v) in enforced(RELAY) {
        o.insert(k, v);
    }
    drop(o);
    let mut l = OVERWRITE_LOCAL_SETTINGS.write().unwrap();
    for k in ["enable-webrtc", "enable-udp-punch", "enable-ipv6-punch"] {
        l.insert(k.to_owned(), "N".to_owned());
    }
    drop(l);
    refresh_system_proxy();
    // Laptops move between office, home and VPN: keep the detected proxy current.
    std::thread::spawn(|| loop {
        std::thread::sleep(Duration::from_secs(60));
        refresh_system_proxy();
    });
}

/// Windows firewall rule the installer creates for the direct listener (hooks.nsh).
const FIREWALL_RULE: &str = "Tether";

/// Policy `network.direct`: `auto` races direct paths (LAN) beside the relay, `off` keeps every
/// session on the relay. The host side listens only where that raises no firewall prompt.
pub fn apply_direct(mode: &str) {
    let on = mode != "off";
    let listen = on && host_may_listen(FIREWALL_RULE);
    let mut o = OVERWRITE_SETTINGS.write().unwrap();
    o.insert("tether-direct".to_owned(), if on { "Y" } else { "N" }.to_owned());
    o.insert("direct-server".to_owned(), if listen { "Y" } else { "N" }.to_owned());
}

/// Whether the host can listen for direct connections without a firewall prompt: on Windows an
/// inbound rule named `rule` must exist (the installer adds it, admins add one for TetherQS); on
/// macOS the application firewall must be off. Checked once per process.
pub fn host_may_listen(rule: &str) -> bool {
    static MAY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *MAY.get_or_init(|| {
        let ok = may_listen(rule);
        hbb_common::log::info!("direct listener allowed: {ok}");
        ok
    })
}

#[cfg(windows)]
fn may_listen(rule: &str) -> bool {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("netsh")
        .args(["advfirewall", "firewall", "show", "rule", &format!("name={rule}"), "dir=in"])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn may_listen(_rule: &str) -> bool {
    std::process::Command::new("/usr/libexec/ApplicationFirewall/socketfilterfw")
        .arg("--getglobalstate")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("disabled"))
        .unwrap_or(false)
}

#[cfg(not(any(windows, target_os = "macos")))]
fn may_listen(_rule: &str) -> bool {
    true
}

/// Settings forced for every process role.
fn enforced(relay: Option<&str>) -> Vec<(String, String)> {
    let mut v = vec![
        ("allow-websocket", "Y".to_owned()),
        ("disable-udp", "Y".to_owned()),
        ("enable-lan-discovery", "N".to_owned()),
        // Off until the policy is applied (apply_direct).
        ("direct-server", "N".to_owned()),
        ("tether-direct", "N".to_owned()),
    ];
    // `TETHER_SERVER` may list several servers; they all share the scheme, the first decides it.
    let first = relay.and_then(|h| h.split(',').next()).map(str::trim);
    if let Some(host) = first.map(tether_trust::policy::server_host).filter(|h| !h.is_empty()) {
        // hbb_common picks wss:// (TLS on 443) only when api-server is https.
        v.push(("api-server", format!("https://{host}")));
    }
    v.into_iter().map(|(k, val)| (k.to_owned(), val)).collect()
}

fn refresh_system_proxy() {
    let host = crate::policy::primary_host();
    let target = format!("https://{}/", if host.is_empty() { "example.com" } else { host });
    let detected = detect_proxy(&target);
    let mut d = DEFAULT_SETTINGS.write().unwrap();
    match detected {
        Some(url) => {
            if d.get("proxy-url") != Some(&url) {
                hbb_common::log::info!("using system proxy {url}");
            }
            d.insert("proxy-url".to_owned(), url);
        }
        None => {
            if d.remove("proxy-url").is_some() {
                hbb_common::log::info!("system proxy removed, connecting directly");
            }
        }
    }
}

/// Proxy URL (`http://host:port`) for reaching `target`, or None for a direct connection.
pub fn detect_proxy(target: &str) -> Option<String> {
    from_env(|k| std::env::var(k).ok()).or_else(|| platform_proxy(target))
}

fn from_env(get: impl Fn(&str) -> Option<String>) -> Option<String> {
    ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"]
        .iter()
        .filter_map(|k| get(k))
        .map(|v| v.trim().to_owned())
        .find(|v| !v.is_empty())
        .map(|v| normalize(&v))
}

/// `host:port` or `scheme://host:port` → `scheme://host:port` (http by default).
fn normalize(p: &str) -> String {
    let p = p.trim().trim_end_matches('/');
    if p.contains("://") {
        p.to_owned()
    } else {
        format!("http://{p}")
    }
}

/// Parse a WinHTTP/IE proxy list: `host:port`, or `http=a:1;https=b:2`, possibly several entries
/// separated by `;` or whitespace. Prefers the https entry, then a scheme-less one, then http.
fn parse_proxy_list(list: &str) -> Option<String> {
    let mut by_scheme: HashMap<String, String> = HashMap::new();
    let mut plain = None;
    for entry in list.split([';', ' ', '\t']).map(str::trim).filter(|e| !e.is_empty()) {
        // PAC-style keywords ("PROXY host:port; DIRECT") arrive as separate tokens.
        if ["DIRECT", "PROXY", "HTTPS", "HTTP", "SOCKS", "SOCKS5"].iter().any(|k| entry.eq_ignore_ascii_case(k)) {
            continue;
        }
        match entry.split_once('=') {
            Some((scheme, addr)) => {
                by_scheme.insert(scheme.to_ascii_lowercase(), addr.to_owned());
            }
            None if plain.is_none() => plain = Some(entry.to_owned()),
            None => {}
        }
    }
    by_scheme
        .remove("https")
        .or(plain)
        .or_else(|| by_scheme.remove("http"))
        .map(|p| normalize(&p))
}

#[cfg(windows)]
fn platform_proxy(target: &str) -> Option<String> {
    winhttp::proxy_for(target)
}

#[cfg(target_os = "macos")]
fn platform_proxy(_target: &str) -> Option<String> {
    // shortcut: static HTTPS proxy only; PAC on macOS needs CFNetwork, add when a customer needs it.
    let out = std::process::Command::new("/usr/sbin/scutil").arg("--proxy").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let get = |k: &str| {
        text.lines()
            .map(str::trim)
            .find_map(|l| l.strip_prefix(k).and_then(|r| r.trim().strip_prefix(':')).map(|v| v.trim().to_owned()))
    };
    if get("HTTPSEnable").as_deref() != Some("1") {
        return None;
    }
    Some(format!("http://{}:{}", get("HTTPSProxy")?, get("HTTPSPort").unwrap_or_else(|| "8080".into())))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn platform_proxy(_target: &str) -> Option<String> {
    None
}

#[cfg(windows)]
mod winhttp {
    //! WinHTTP resolves exactly what browsers use: IE/Edge static proxy, PAC URL, and WPAD.
    use windows_sys::Win32::{
        Foundation::{GlobalFree, BOOL},
        Networking::WinHttp::*,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe fn take(p: *mut u16) -> Option<String> {
        if p.is_null() {
            return None;
        }
        let len = (0..).take_while(|&i| *p.add(i) != 0).count();
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
        GlobalFree(p as _);
        Some(s)
    }

    pub fn proxy_for(target: &str) -> Option<String> {
        unsafe {
            let mut ie: WINHTTP_CURRENT_USER_IE_PROXY_CONFIG = std::mem::zeroed();
            let have_ie = WinHttpGetIEProxyConfigForCurrentUser(&mut ie) != 0;
            let static_proxy = if have_ie { take(ie.lpszProxy) } else { None };
            let pac_url = if have_ie { take(ie.lpszAutoConfigUrl) } else { None };
            let _bypass = if have_ie { take(ie.lpszProxyBypass) } else { None };
            let auto_detect: BOOL = if have_ie { ie.fAutoDetect } else { 0 };

            if pac_url.is_some() || auto_detect != 0 {
                if let Some(p) = resolve_pac(target, pac_url.as_deref(), auto_detect != 0) {
                    return Some(p);
                }
            }
            static_proxy.as_deref().and_then(super::parse_proxy_list)
        }
    }

    unsafe fn resolve_pac(target: &str, pac_url: Option<&str>, auto_detect: bool) -> Option<String> {
        let agent = wide("Tether");
        let session = WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_NO_PROXY, std::ptr::null(), std::ptr::null(), 0);
        if session.is_null() {
            return None;
        }
        // PAC/WPAD can hang on broken networks; never block startup for long.
        WinHttpSetTimeouts(session, 3000, 3000, 3000, 3000);
        let url = wide(target);
        let pac = pac_url.map(wide);
        let mut opts: WINHTTP_AUTOPROXY_OPTIONS = std::mem::zeroed();
        opts.fAutoLogonIfChallenged = 1;
        if let Some(pac) = &pac {
            opts.dwFlags = WINHTTP_AUTOPROXY_CONFIG_URL;
            opts.lpszAutoConfigUrl = pac.as_ptr();
        } else if auto_detect {
            opts.dwFlags = WINHTTP_AUTOPROXY_AUTO_DETECT;
            opts.dwAutoDetectFlags = WINHTTP_AUTO_DETECT_TYPE_DHCP | WINHTTP_AUTO_DETECT_TYPE_DNS_A;
        }
        let mut info: WINHTTP_PROXY_INFO = std::mem::zeroed();
        let ok = WinHttpGetProxyForUrl(session, url.as_ptr(), &mut opts, &mut info) != 0;
        WinHttpCloseHandle(session);
        if !ok {
            return None;
        }
        let list = take(info.lpszProxy);
        let _ = take(info.lpszProxyBypass);
        if info.dwAccessType == WINHTTP_ACCESS_TYPE_NO_PROXY {
            return None;
        }
        list.as_deref().and_then(super::parse_proxy_list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enforces_443_only_settings() {
        let e: HashMap<_, _> = enforced(Some("relay.example.com")).into_iter().collect();
        assert_eq!(e["allow-websocket"], "Y");
        assert_eq!(e["disable-udp"], "Y");
        assert_eq!(e["enable-lan-discovery"], "N");
        assert_eq!(e["api-server"], "https://relay.example.com");
        let e: HashMap<_, _> = enforced(Some("relay.example.com:21116")).into_iter().collect();
        assert_eq!(e["api-server"], "https://relay.example.com");
        let e: HashMap<_, _> = enforced(Some("relay.example.com, relay2.example.com")).into_iter().collect();
        assert_eq!(e["api-server"], "https://relay.example.com");
    }

    #[test]
    fn parses_proxy_lists() {
        assert_eq!(parse_proxy_list("proxy.corp:8080").as_deref(), Some("http://proxy.corp:8080"));
        assert_eq!(parse_proxy_list("http=a:3128;https=b:8443").as_deref(), Some("http://b:8443"));
        assert_eq!(parse_proxy_list("http=a:3128").as_deref(), Some("http://a:3128"));
        assert_eq!(parse_proxy_list("PROXY gw.zscaler.net:80; DIRECT").as_deref(), Some("http://gw.zscaler.net:80"));
        assert_eq!(parse_proxy_list("DIRECT"), None);
        assert_eq!(parse_proxy_list(""), None);
    }

    #[test]
    fn env_proxy_wins_and_is_normalized() {
        let env = |k: &str| (k == "https_proxy").then(|| "corp-proxy:3128".to_owned());
        assert_eq!(from_env(env).as_deref(), Some("http://corp-proxy:3128"));
        assert_eq!(from_env(|_| None), None);
    }
}

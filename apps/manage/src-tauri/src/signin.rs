//! Staff sign-in for Tether Manage (design 12.4): the desktop's sign-in module
//! (`apps/desktop/src-tauri/src/oidc.rs`, included in main.rs with `#[path]` as `oidc`) does the
//! OpenID Connect flow; this file is what it calls as `crate::policy` and `crate::team`. Manage
//! signs in with a session key of its own, made per run and kept only in memory: the relay binds
//! the person's session to it, and Manage signs admin requests with it (`kid`) when its vault is
//! locked. A session never signs a document: that stays with the vault (`App::admin_key`).

use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::Duration;

use tether_trust::{reqauth, SigningKey, VerifyingKey};

type R<T> = Result<T, String>;

/// Manage's data folder (set once at start).
pub static DIR: OnceLock<PathBuf> = OnceLock::new();
/// The admin public key of this PC's vault, remembered at unlock (`admin-key.pub`): the relays'
/// sign-in settings are verified with it and the compiled-in keys.
pub static VAULT_PUB: Mutex<Option<VerifyingKey>> = Mutex::new(None);
/// The key this run's sign-in session is bound to.
pub static SESSION_KEY: LazyLock<SigningKey> = LazyLock::new(tether_trust::generate_key);

const ADMIN_PUB_FILE: &str = "admin-key.pub";

fn dir() -> PathBuf {
    DIR.get().cloned().unwrap_or_default()
}

/// Remembers the vault's admin public key (at create, unlock, import).
pub fn remember_admin(k: VerifyingKey) {
    *VAULT_PUB.lock().unwrap() = Some(k);
    let _ = policy::write_atomic(&dir().join(ADMIN_PUB_FILE), tether_trust::key_to_b64(&k).as_bytes());
}

/// The Tether Manage roles of the live sign-in (`manage` of the relay's session answer), kebab-case.
pub fn roles() -> Vec<String> {
    crate::oidc::me().and_then(|m| m.get("manage").cloned()).and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}


fn client() -> R<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder().redirect(reqwest::redirect::Policy::none()).connect_timeout(Duration::from_secs(10)).timeout(Duration::from_secs(30)).user_agent(concat!("tether-manage/", env!("CARGO_PKG_VERSION"))).build().map_err(|e| e.to_string())
}

fn answer(r: reqwest::Result<reqwest::blocking::Response>) -> R<(u16, Vec<u8>)> {
    let r = r.map_err(|e| format!("offline: {}", e.without_url()))?;
    let code = r.status().as_u16();
    Ok((code, tether_trust::read_limited(r, 64 << 10)?))
}

/// A request to `relay` (a base URL) signed with the session key and `kid`.
fn signed(relay: &str, method: &str, path: &str, body: &[u8]) -> R<(u16, Vec<u8>)> {
    let sig = reqauth::sign_request_for_kid(&SESSION_KEY, relay, method, path, body, crate::api::now());
    let m = reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| e.to_string())?;
    let mut rq = client()?.request(m, format!("{}{path}", relay.trim_end_matches('/'))).header(reqauth::HEADER, sig);
    if !body.is_empty() {
        rq = rq.header("Content-Type", "application/json").body(body.to_vec());
    }
    answer(rq.send())
}

pub mod policy {
    use super::*;

    /// The compiled-in admin keys and this PC's vault key (remembered at unlock).
    pub fn admin_keys() -> Vec<VerifyingKey> {
        let mut v: Vec<VerifyingKey> = crate::TRUSTED_ADMIN_KEYS.unwrap_or("").split(',').filter_map(|k| tether_trust::key_from_b64(k.trim()).ok()).collect();
        let vault = *VAULT_PUB.lock().unwrap();
        let remembered = std::fs::read_to_string(dir().join(ADMIN_PUB_FILE)).ok().and_then(|k| tether_trust::key_from_b64(k.trim()).ok());
        for k in vault.into_iter().chain(remembered) {
            if !v.contains(&k) {
                v.push(k);
            }
        }
        v
    }

    /// Every relay of the settings (base URLs), as the desktop's module names relays.
    pub fn relay_bases() -> Vec<(String, String)> {
        crate::servers_in(&dir()).into_iter().map(|s| (s.clone(), s)).collect()
    }

    /// Manage has no proxy setting: the provider is reached directly.
    pub fn proxy_url() -> Option<String> {
        None
    }

    pub fn write_atomic(path: &PathBuf, data: &[u8]) -> std::io::Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = crate::tmp_path(path);
        std::fs::write(&tmp, data)?;
        std::fs::rename(&tmp, path)
    }
}

pub mod team {
    use super::*;

    pub fn data_path(name: &str) -> PathBuf {
        dir().join(name)
    }

    pub fn config_get(relay: &str) -> R<(u16, Vec<u8>)> {
        answer(client()?.get(format!("{}/api/team/config", relay.trim_end_matches('/'))).send())
    }

    /// `POST /api/team/session`: the access token and the session key, signed by it with `kid`.
    pub fn session_post(relay: &str, access_token: &str) -> R<(u16, Vec<u8>)> {
        let body = serde_json::json!({"access_token": access_token, "key": tether_trust::key_to_b64(&SESSION_KEY.verifying_key())}).to_string();
        signed(relay, "POST", "/api/team/session", body.as_bytes())
    }

    pub fn session_delete(relay: &str) -> R<(u16, Vec<u8>)> {
        signed(relay, "DELETE", "/api/team/session", b"")
    }

    /// A relay's refusal as one line.
    pub fn error_text(code: u16, body: &str) -> String {
        let b: String = body.trim().chars().filter(|c| !c.is_control()).take(300).collect();
        match code {
            401 => format!("The server did not accept this sign-in ({b})."),
            403 => format!("The server refused: {b}"),
            404 => "This server does not offer sign-in (update it).".into(),
            _ => format!("The server answered {code}: {b}"),
        }
    }

    /// The desktop posts its own `token-used` record; a Tether Manage session cannot post team
    /// events, and the relay that refused the token records `sign-in-refused` (`token-used`) itself.
    pub fn audit_event_everywhere(_: serde_json::Value) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    /// A relay stand-in that answers one request and hands back its head and body.
    fn one_shot() -> (String, std::thread::JoinHandle<(String, Vec<u8>)>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
        let h = std::thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            let mut r = BufReader::new(s);
            let (mut head, mut len) = (String::new(), 0);
            loop {
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
                if line == "\r\n" {
                    break;
                }
                head.push_str(&line);
            }
            let mut body = vec![0; len];
            r.read_exact(&mut body).unwrap();
            r.get_mut().write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").unwrap();
            (head, body)
        });
        (base, h)
    }

    #[test]
    fn the_session_is_posted_signed_by_the_runs_key_with_kid() {
        let (base, h) = one_shot();
        assert_eq!(team::session_post(&base, "tok").unwrap(), (200, b"{}".to_vec()));
        let (head, body) = h.join().unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v, serde_json::json!({"access_token": "tok", "key": tether_trust::key_to_b64(&SESSION_KEY.verifying_key())}));
        let sig = head.lines().find_map(|l| l.split_once(": ").filter(|(k, _)| k.eq_ignore_ascii_case(reqauth::HEADER)).map(|(_, v)| v.trim().to_owned())).unwrap();
        assert_eq!(reqauth::header_kid(&sig), Some(tether_trust::team::key_id(&SESSION_KEY.verifying_key()).as_str()));
        let hosts = [reqauth::host_of(&base)];
        let relay = reqauth::Relay { hosts: &hosts, v1: false };
        reqauth::verify_request_for(&sig, &SESSION_KEY.verifying_key(), relay, "POST", "/api/team/session", &reqauth::body_digest(&body), crate::api::now(), |_| true).unwrap();
    }

    #[test]
    fn settings_are_verified_with_the_vault_key_too() {
        let d = std::env::temp_dir().join(format!("tm-signin-{}", std::process::id()));
        let _ = DIR.set(d.clone());
        let k = tether_trust::generate_key().verifying_key();
        remember_admin(k);
        *VAULT_PUB.lock().unwrap() = None;
        // Remembered across runs (the file), not only while unlocked.
        assert!(policy::admin_keys().contains(&k));
        assert_eq!(std::fs::read_to_string(DIR.get().unwrap().join(ADMIN_PUB_FILE)).unwrap(), tether_trust::key_to_b64(&k));
        let _ = std::fs::remove_dir_all(d);
    }
}

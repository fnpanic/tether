//! Mock OpenID Connect provider shaped like Microsoft Entra ID v2.0, for tests and local
//! development. NO SECURITY: anyone who reaches it gets tokens for any configured user. It listens
//! on loopback only (unless told otherwise) and `/control` answers loopback peers only.
//!
//! Endpoints under the issuer `http://<addr>/<tenant>/v2.0`: `/.well-known/openid-configuration`
//! (also at the root), `/jwks`, `/authorize`, `/token`, `/logout`; and `/control/<call>` (see the
//! README). Keys are RSA 2048 generated at start from the seed or OS entropy, never stored.

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use rand_chacha::ChaCha20Rng;
use rand_core::{OsRng, RngCore, SeedableRng};
use rsa::pkcs1v15::SigningKey;
use rsa::signature::{SignatureEncoding, Signer};
use rsa::traits::PublicKeyParts;
use rsa::RsaPrivateKey;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Read;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use tiny_http::{Header, Request, Response, Server};

/// Seconds an authorization code stays redeemable (Entra: about 10 minutes).
pub const CODE_LIFETIME: u64 = 60;
/// The tenant the `wrong_tid` / `wrong_iss` shapes name.
pub const OTHER_TENANT: &str = "90000000-0000-4000-8000-000000000009";
/// The audience the `wrong_aud` shape names.
pub const OTHER_AUDIENCE: &str = "90000000-0000-4000-8000-0000000000aa";

/// Start configuration (also the JSON of `--config`; missing fields take the defaults).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Entra tenant id (`tid`, and part of the issuer).
    pub tenant: String,
    /// `aud` of access tokens: the API app registration's client id.
    pub audience: String,
    /// The public client (desktop, Manage) allowed at `/authorize` and `/token`.
    pub client_id: String,
    /// The API scope; its last segment is the token's `scp`.
    pub scope: String,
    /// Access and id token lifetime in seconds.
    pub lifetime: u64,
    pub users: Vec<User>,
    /// Seed for keys, codes and token ids; none: OS entropy.
    pub seed: Option<u64>,
    /// Fixed clock (unix seconds); none: the system clock.
    pub now: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct User {
    pub oid: String,
    pub name: String,
    /// `preferred_username`, and what `login_hint` names (an oid works too).
    pub upn: String,
    pub roles: Vec<String>,
    /// Emitted as `groups` when not empty; more than 200 become the overage pointer, as in Entra.
    pub groups: Vec<String>,
    /// Sign-in and refresh refused (Entra AADSTS50057).
    pub disabled: bool,
}

fn user(n: u8, short: &str, name: &str, roles: &[&str]) -> User {
    User { oid: format!("00000000-0000-4000-8000-0000000000{n:02}"), name: name.to_owned(), upn: format!("{short}@tether.test"), roles: roles.iter().map(|r| r.to_string()).collect(), ..User::default() }
}

impl Default for Config {
    fn default() -> Self {
        let t = "Tether.Technician";
        let mut disabled = user(11, "disabled", "Disabled Person", &[t]);
        disabled.disabled = true;
        Config {
            tenant: "10000000-0000-4000-8000-000000000001".into(),
            audience: "30000000-0000-4000-8000-000000000003".into(),
            client_id: "20000000-0000-4000-8000-000000000002".into(),
            scope: "api://tether-test/access".into(),
            lifetime: 3600,
            users: vec![
                user(1, "technician", "Test Technician", &[t]),
                user(2, "software", "Software Technician", &[t, "Tether.Software"]),
                user(3, "writer", "Senior Technician", &[t, "Tether.Technician.Write", "Tether.Software", "Tether.Agent", "Tether.SessionAudit"]),
                user(4, "customer", "Customer Technician", &[t, "Tether.Customer.4711"]),
                user(5, "auditor", "Manage Auditor", &["Tether.Manage.Auditor"]),
                user(6, "operator", "Manage Operator", &["Tether.Manage.Operator"]),
                user(7, "editor", "Policy Editor", &["Tether.Manage.PolicyEditor"]),
                user(8, "publisher", "Release Publisher", &["Tether.Manage.Publisher"]),
                user(9, "keyholder", "Key Holder", &["Tether.Manage.KeyHolder"]),
                user(10, "noroles", "No Roles", &[]),
                disabled,
            ],
            seed: None,
            now: None,
        }
    }
}

/// Token shapes `/control/force` and `/control/token` make the mock issue.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Shape {
    Expired,
    NotYetValid,
    WrongAud,
    WrongIss,
    WrongTid,
    WrongVer,
    Overage,
    UnknownKid,
    BadSignature,
}

struct Key {
    kid: String,
    sk: SigningKey<Sha256>,
    n: Vec<u8>,
    e: Vec<u8>,
}

fn new_key(rng: &mut ChaCha20Rng) -> Key {
    let k = RsaPrivateKey::new(rng, 2048).expect("RSA key generation");
    let (n, e) = (k.n().to_bytes_be(), k.e().to_bytes_be());
    Key { kid: B64.encode(&Sha256::digest(&n)[..12]), sk: SigningKey::<Sha256>::new(k), n, e }
}

struct Code {
    client_id: String,
    redirect_uri: String,
    challenge: String,
    oid: String,
    nonce: Option<String>,
    scope: String,
    at: u64,
}

struct Reply {
    status: u16,
    ctype: &'static str,
    body: String,
    location: Option<String>,
}

fn reply_json(status: u16, v: Value) -> Reply {
    Reply { status, ctype: "application/json", body: v.to_string(), location: None }
}

fn page(status: u16, html: &str) -> Reply {
    Reply { status, ctype: "text/html; charset=utf-8", body: format!("<!doctype html><title>oidc-mock</title><p>oidc-mock: test provider, no security.</p>{html}"), location: None }
}

fn redirect(to: String) -> Reply {
    Reply { status: 302, ctype: "text/plain", body: String::new(), location: Some(to) }
}

/// Entra-like token endpoint error.
fn oauth_error(error: &str, code: u32, text: &str) -> Reply {
    reply_json(400, json!({"error": error, "error_description": format!("AADSTS{code}: {text}"), "error_codes": [code]}))
}

fn enc(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

/// `application/x-www-form-urlencoded` (query strings and token requests).
fn parse_form(s: &str) -> HashMap<String, String> {
    let dec = |v: &str| {
        let b = v.as_bytes();
        let mut out = Vec::with_capacity(b.len());
        let mut i = 0;
        while i < b.len() {
            match b[i] {
                b'+' => out.push(b' '),
                b'%' => match v.get(i + 1..i + 3).filter(|h| h.bytes().all(|c| c.is_ascii_hexdigit())) {
                    Some(h) => {
                        out.push(u8::from_str_radix(h, 16).unwrap());
                        i += 2;
                    }
                    None => out.push(b'%'),
                },
                c => out.push(c),
            }
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    };
    s.split('&').filter(|p| !p.is_empty()).map(|p| p.split_once('=').unwrap_or((p, ""))).map(|(k, v)| (dec(k), dec(v))).collect()
}

/// `http://` on 127.0.0.1, localhost or [::1], any port, no fragment, no user info.
fn loopback_redirect(u: &str) -> bool {
    let Some(rest) = u.strip_prefix("http://") else { return false };
    if !u.bytes().all(|b| b.is_ascii_graphic()) || u.contains('#') {
        return false;
    }
    let auth = &rest[..rest.find(['/', '?']).unwrap_or(rest.len())];
    let (host, port) = match auth.rfind(':') {
        Some(i) if !auth[i..].contains(']') => (&auth[..i], &auth[i + 1..]),
        _ => (auth, ""),
    };
    matches!(host, "127.0.0.1" | "localhost" | "[::1]") && port.bytes().all(|b| b.is_ascii_digit())
}

/// Whether a peer may use `/control` (an IPv4-mapped IPv6 loopback counts).
pub fn loopback_peer(peer: Option<&SocketAddr>) -> bool {
    match peer.map(|a| a.ip()) {
        Some(IpAddr::V4(v4)) => v4.is_loopback(),
        Some(IpAddr::V6(v6)) => v6.to_ipv4_mapped().map_or(v6.is_loopback(), |v4| v4.is_loopback()),
        None => false,
    }
}

fn with_params(base: &str, params: &[(&str, Option<&str>)]) -> String {
    let q: Vec<String> = params.iter().filter_map(|(k, v)| v.map(|v| format!("{k}={}", enc(v)))).collect();
    format!("{base}{}{}", if base.contains('?') { '&' } else { '?' }, q.join("&"))
}

struct State {
    cfg: Config,
    issuer: String,
    keys: Vec<Key>,
    rng: ChaCha20Rng,
    codes: HashMap<String, Code>,
    /// refresh token → (oid, scope); never expire (Entra: up to 90 days).
    refresh: HashMap<String, (String, String)>,
    force: Option<Shape>,
    login: Option<String>,
}

impl State {
    fn now(&self) -> u64 {
        self.cfg.now.unwrap_or_else(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs())
    }

    fn random(&mut self, n: usize) -> String {
        let mut b = vec![0u8; n];
        self.rng.fill_bytes(&mut b);
        B64.encode(b)
    }

    fn find(&self, who: &str) -> Option<User> {
        self.cfg.users.iter().find(|u| u.oid == who || u.upn.eq_ignore_ascii_case(who)).cloned()
    }

    fn sign(&mut self, claims: &Value, shape: Option<Shape>) -> String {
        let kid = if shape == Some(Shape::UnknownKid) { format!("unknown-{}", self.random(9)) } else { self.keys.last().unwrap().kid.clone() };
        let input = format!("{}.{}", B64.encode(json!({"typ": "JWT", "alg": "RS256", "kid": kid}).to_string()), B64.encode(claims.to_string()));
        let mut sig = self.keys.last().unwrap().sk.sign(input.as_bytes()).to_vec();
        if shape == Some(Shape::BadSignature) {
            sig[10] ^= 0x55;
        }
        format!("{input}.{}", B64.encode(sig))
    }

    /// Entra's `sub` is pairwise per application; here a hash of client id and oid.
    fn sub(&self, u: &User) -> String {
        B64.encode(&Sha256::digest(format!("{}\n{}", self.cfg.client_id, u.oid).as_bytes())[..24])
    }

    fn access_token(&mut self, u: &User, shape: Option<Shape>) -> String {
        let (now, life) = (self.now() as i64, self.cfg.lifetime as i64);
        let (iat, nbf, exp) = match shape {
            Some(Shape::Expired) => (now - life - 600, now - life - 600, now - 600),
            Some(Shape::NotYetValid) => (now, now + 600, now + 600 + life),
            _ => (now, now, now + life),
        };
        let tenant = self.cfg.tenant.clone();
        let mut c = json!({
            "aud": if shape == Some(Shape::WrongAud) { OTHER_AUDIENCE } else { &self.cfg.audience },
            "iss": if shape == Some(Shape::WrongIss) { self.issuer.replace(&tenant, OTHER_TENANT) } else { self.issuer.clone() },
            "iat": iat, "nbf": nbf, "exp": exp,
            "azp": self.cfg.client_id, "azpacr": "0",
            "name": u.name, "oid": u.oid, "preferred_username": u.upn,
            "scp": self.cfg.scope.rsplit('/').next().unwrap_or(""),
            "sub": self.sub(u),
            "tid": if shape == Some(Shape::WrongTid) { OTHER_TENANT } else { &tenant },
            "uti": self.random(16),
            "ver": if shape == Some(Shape::WrongVer) { "1.0" } else { "2.0" },
        });
        if !u.roles.is_empty() {
            c["roles"] = json!(u.roles);
        }
        if shape == Some(Shape::Overage) || u.groups.len() > 200 {
            c["_claim_names"] = json!({"groups": "src1"});
            c["_claim_sources"] = json!({"src1": {"endpoint": format!("https://graph.microsoft.com/v1.0/users/{}/getMemberObjects", u.oid)}});
        } else if !u.groups.is_empty() {
            c["groups"] = json!(u.groups);
        }
        self.sign(&c, shape)
    }

    fn id_token(&mut self, u: &User, nonce: Option<&str>) -> String {
        let now = self.now();
        let mut c = json!({"aud": self.cfg.client_id, "iss": self.issuer, "iat": now, "nbf": now, "exp": now + self.cfg.lifetime, "name": u.name, "oid": u.oid, "preferred_username": u.upn, "sub": self.sub(u), "tid": self.cfg.tenant, "ver": "2.0"});
        if let Some(n) = nonce {
            c["nonce"] = json!(n);
        }
        self.sign(&c, None)
    }

    fn issue(&mut self, u: &User, scope: &str, nonce: Option<&str>) -> Reply {
        let mut t = json!({"token_type": "Bearer", "scope": scope, "expires_in": self.cfg.lifetime, "ext_expires_in": self.cfg.lifetime, "access_token": self.access_token(u, self.force)});
        let words: Vec<&str> = scope.split(' ').collect();
        if words.contains(&"openid") {
            t["id_token"] = json!(self.id_token(u, nonce));
        }
        if words.contains(&"offline_access") {
            let rt = self.random(48);
            self.refresh.insert(rt.clone(), (u.oid.clone(), scope.to_owned()));
            t["refresh_token"] = json!(rt);
        }
        reply_json(200, t)
    }

    fn handle(&mut self, method: &str, path: &str, query: &str, body: &str, peer: Option<&SocketAddr>) -> Reply {
        let prefix = format!("/{}/v2.0", self.cfg.tenant);
        let p = path.strip_prefix(prefix.as_str()).filter(|p| !p.is_empty()).unwrap_or(path);
        if let Some(call) = p.strip_prefix("/control/") {
            if !loopback_peer(peer) {
                return reply_json(403, json!({"error": "control answers loopback callers only"}));
            }
            if method != "POST" {
                return reply_json(405, json!({"error": "POST"}));
            }
            let body = if body.trim().is_empty() { Ok(json!({})) } else { serde_json::from_str(body) };
            return match body.map_err(|e| e.to_string()).and_then(|b| self.control(call, b)) {
                Ok(v) => reply_json(200, v),
                Err(e) => reply_json(400, json!({"error": e})),
            };
        }
        match (method, p) {
            ("GET", "/.well-known/openid-configuration") => reply_json(200, self.discovery()),
            ("GET", "/jwks") => reply_json(200, self.jwks()),
            ("GET", "/authorize") => self.authorize(query),
            ("POST", "/token") => self.token(&parse_form(body)),
            ("GET", "/logout") => match parse_form(query).get("post_logout_redirect_uri") {
                Some(u) if loopback_redirect(u) => redirect(u.clone()),
                _ => page(200, "<p>Signed out.</p>"),
            },
            _ => reply_json(404, json!({"error": "not found"})),
        }
    }

    fn discovery(&self) -> Value {
        let i = &self.issuer;
        json!({
            "issuer": i,
            "authorization_endpoint": format!("{i}/authorize"),
            "token_endpoint": format!("{i}/token"),
            "jwks_uri": format!("{i}/jwks"),
            "end_session_endpoint": format!("{i}/logout"),
            "response_types_supported": ["code"],
            "response_modes_supported": ["query"],
            "subject_types_supported": ["pairwise"],
            "id_token_signing_alg_values_supported": ["RS256"],
            "scopes_supported": ["openid", "profile", "email", "offline_access"],
            "grant_types_supported": ["authorization_code", "refresh_token"],
            "token_endpoint_auth_methods_supported": ["none"],
            "code_challenge_methods_supported": ["S256"],
            "claims_supported": ["aud", "iss", "iat", "nbf", "exp", "name", "oid", "preferred_username", "roles", "sub", "tid", "uti", "ver", "nonce"],
        })
    }

    fn jwks(&self) -> Value {
        json!({"keys": self.keys.iter().map(|k| json!({"kty": "RSA", "use": "sig", "kid": k.kid, "n": B64.encode(&k.n), "e": B64.encode(&k.e)})).collect::<Vec<_>>()})
    }

    fn authorize(&mut self, raw: &str) -> Reply {
        let q = parse_form(raw);
        let get = |k: &str| q.get(k).map(String::as_str).unwrap_or("");
        let redirect_uri = get("redirect_uri").to_owned();
        if !loopback_redirect(&redirect_uri) {
            return page(400, "<p>redirect_uri must be http:// on 127.0.0.1, localhost or [::1] (any port, no fragment).</p>");
        }
        if get("client_id") != self.cfg.client_id {
            return page(400, "<p>AADSTS700016: unknown client_id.</p>");
        }
        if get("response_type") != "code" {
            return page(400, "<p>Only response_type=code.</p>");
        }
        if get("code_challenge_method") != "S256" || get("code_challenge").len() != 43 {
            return page(400, "<p>PKCE required: code_challenge (43 characters) with code_challenge_method=S256.</p>");
        }
        let Some(who) = q.get("login_hint").cloned().or_else(|| self.login.clone()) else {
            let raw = html_escape(raw);
            let rows: String = self.cfg.users.iter().map(|u| format!("<li><a href=\"?{raw}&amp;login_hint={}\">{} ({})</a> {}{}</li>", enc(&u.upn), html_escape(&u.name), html_escape(&u.upn), html_escape(&u.roles.join(", ")), if u.disabled { " (disabled)" } else { "" })).collect();
            return page(200, &format!("<p>Sign in as:</p><ul>{rows}</ul>"));
        };
        let Some(u) = self.find(&who) else { return page(400, "<p>Unknown user in login_hint.</p>") };
        let state = q.get("state").map(String::as_str);
        if u.disabled {
            return redirect(with_params(&redirect_uri, &[("error", Some("access_denied")), ("error_description", Some("AADSTS50057: The user account is disabled.")), ("state", state)]));
        }
        let code = self.random(32);
        let c = Code { client_id: get("client_id").into(), redirect_uri: redirect_uri.clone(), challenge: get("code_challenge").into(), oid: u.oid, nonce: q.get("nonce").cloned(), scope: get("scope").into(), at: self.now() };
        self.codes.insert(code.clone(), c);
        redirect(with_params(&redirect_uri, &[("code", Some(&code)), ("state", state)]))
    }

    fn token(&mut self, f: &HashMap<String, String>) -> Reply {
        let get = |k: &str| f.get(k).map(String::as_str).unwrap_or("");
        if get("client_id") != self.cfg.client_id {
            return oauth_error("invalid_client", 700016, "Application with the given client_id was not found.");
        }
        match get("grant_type") {
            "authorization_code" => {
                // Used up by the first attempt, whatever its outcome.
                let Some(c) = self.codes.remove(get("code")) else { return oauth_error("invalid_grant", 54005, "The authorization code is unknown or was already redeemed.") };
                if self.now() > c.at + CODE_LIFETIME {
                    return oauth_error("invalid_grant", 70008, "The provided authorization code has expired.");
                }
                if c.client_id != get("client_id") || c.redirect_uri != get("redirect_uri") {
                    return oauth_error("invalid_grant", 50011, "The redirect_uri does not match the one of the authorization request.");
                }
                let v = get("code_verifier");
                if !(43..=128).contains(&v.len()) {
                    return oauth_error("invalid_request", 50148, "code_verifier missing or not 43 to 128 characters.");
                }
                if B64.encode(Sha256::digest(v.as_bytes())) != c.challenge {
                    return oauth_error("invalid_grant", 501481, "The Code_Verifier does not match the code_challenge supplied in the authorization request.");
                }
                match self.find(&c.oid) {
                    Some(u) if !u.disabled => self.issue(&u, &c.scope, c.nonce.as_deref()),
                    _ => oauth_error("invalid_grant", 50057, "The user account is disabled."),
                }
            }
            "refresh_token" => {
                let Some((oid, scope)) = self.refresh.get(get("refresh_token")).cloned() else { return oauth_error("invalid_grant", 9002313, "Invalid request. The refresh token is unknown.") };
                match self.find(&oid) {
                    Some(u) if !u.disabled => self.issue(&u, &scope, None),
                    _ => oauth_error("invalid_grant", 50057, "The user account is disabled."),
                }
            }
            _ => oauth_error("unsupported_grant_type", 70003, "Only authorization_code and refresh_token."),
        }
    }

    fn control(&mut self, call: &str, b: Value) -> Result<Value, String> {
        let shape = |v: &Value| -> Result<Option<Shape>, String> {
            if v.is_null() {
                Ok(None)
            } else {
                serde_json::from_value(v.clone()).map(Some).map_err(|e| format!("shape: {e}"))
            }
        };
        match call {
            "force" => self.force = shape(&b["shape"])?,
            "token" => {
                let s = shape(&b["shape"])?;
                let u = self.find(b["user"].as_str().unwrap_or("")).ok_or("unknown user")?;
                if u.disabled {
                    return Err("user is disabled".into());
                }
                return Ok(json!({"access_token": self.access_token(&u, s)}));
            }
            "rotate" => {
                let k = new_key(&mut self.rng);
                if !b["keep_old"].as_bool().unwrap_or(false) {
                    self.keys.clear();
                }
                let kid = k.kid.clone();
                self.keys.push(k);
                return Ok(json!({"kid": kid, "kids": self.keys.iter().map(|k| k.kid.clone()).collect::<Vec<_>>()}));
            }
            "lifetime" => self.cfg.lifetime = b["seconds"].as_u64().filter(|s| *s > 0).ok_or("seconds: a positive number")?,
            "clock" => {
                if let Some(a) = b["advance"].as_i64() {
                    self.cfg.now = Some(self.now().saturating_add_signed(a));
                } else if b["now"].is_null() {
                    self.cfg.now = None;
                } else {
                    self.cfg.now = Some(b["now"].as_u64().ok_or("now: unix seconds or null; or advance: seconds")?);
                }
                return Ok(json!({"now": self.now()}));
            }
            "login" => {
                self.login = match b["user"].as_str() {
                    Some(w) => Some(self.find(w).ok_or("unknown user")?.oid),
                    None => None,
                }
            }
            "user" => self.upsert_user(&b)?,
            _ => return Err(format!("unknown control call {call}")),
        }
        Ok(json!({"ok": true}))
    }

    /// Changes the given fields of the user named by `oid` (else `upn`), or adds a new user.
    fn upsert_user(&mut self, b: &Value) -> Result<(), String> {
        let obj = b.as_object().ok_or("a JSON object")?;
        let (oid, upn) = (obj.get("oid").and_then(Value::as_str), obj.get("upn").and_then(Value::as_str));
        let at = match (oid, upn) {
            (Some(o), _) => self.cfg.users.iter().position(|u| u.oid == o),
            (None, Some(n)) => Some(self.cfg.users.iter().position(|u| u.upn.eq_ignore_ascii_case(n)).ok_or("unknown upn (a new user needs an oid)")?),
            (None, None) => return Err("oid or upn".into()),
        };
        match at {
            Some(i) => {
                let mut v = serde_json::to_value(&self.cfg.users[i]).unwrap();
                for (k, x) in obj {
                    v[k] = x.clone();
                }
                self.cfg.users[i] = serde_json::from_value(v).map_err(|e| e.to_string())?;
            }
            None => {
                let mut u: User = serde_json::from_value(b.clone()).map_err(|e| e.to_string())?;
                if u.upn.is_empty() {
                    u.upn = u.oid.clone();
                }
                if u.name.is_empty() {
                    u.name = u.upn.clone();
                }
                self.cfg.users.push(u);
            }
        }
        Ok(())
    }
}

fn serve(st: &Mutex<State>, mut req: Request) {
    let peer = req.remote_addr().copied();
    let url = req.url().to_owned();
    let (path, query) = url.split_once('?').unwrap_or((&url, ""));
    let mut body = String::new();
    let _ = req.as_reader().take(1 << 20).read_to_string(&mut body);
    let r = st.lock().unwrap_or_else(|e| e.into_inner()).handle(req.method().as_str(), path, query, &body, peer.as_ref());
    let h = |k: &str, v: &str| Header::from_bytes(k, v).unwrap();
    let mut resp = Response::from_string(r.body).with_status_code(r.status).with_header(h("Content-Type", r.ctype)).with_header(h("Cache-Control", "no-store"));
    if let Some(l) = r.location {
        resp = resp.with_header(h("Location", &l));
    }
    let _ = req.respond(resp);
}

/// A running mock; stopped when dropped.
pub struct Mock {
    cfg: Config,
    addr: SocketAddr,
    issuer: String,
    state: Arc<Mutex<State>>,
    server: Arc<Server>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Mock {
    /// Binds `listen` (port 0: any free port). A non-loopback address is refused unless
    /// `allow_network`.
    pub fn start(cfg: Config, listen: &str, allow_network: bool) -> Result<Mock, String> {
        let addrs: Vec<SocketAddr> = listen.to_socket_addrs().map_err(|e| format!("{listen}: {e}"))?.collect();
        if addrs.is_empty() || (!allow_network && !addrs.iter().all(|a| loopback_peer(Some(a)))) {
            return Err(format!("refusing to listen on {listen}: not a loopback address (this mock has no security; --allow-network overrides)"));
        }
        let server = Arc::new(Server::http(&addrs[..]).map_err(|e| format!("{listen}: {e}"))?);
        let addr = server.server_addr().to_ip().ok_or("not an IP listener")?;
        let issuer = format!("http://{addr}/{}/v2.0", cfg.tenant);
        let mut rng = ChaCha20Rng::seed_from_u64(cfg.seed.unwrap_or_else(|| OsRng.next_u64()));
        let keys = vec![new_key(&mut rng)];
        let state = Arc::new(Mutex::new(State { cfg: cfg.clone(), issuer: issuer.clone(), keys, rng, codes: HashMap::new(), refresh: HashMap::new(), force: None, login: None }));
        let (srv, st) = (server.clone(), state.clone());
        let thread = std::thread::spawn(move || {
            for req in srv.incoming_requests() {
                serve(&st, req);
            }
        });
        Ok(Mock { cfg, addr, issuer, state, server, thread: Some(thread) })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// `http://<addr>`.
    pub fn base(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// `http://<addr>/<tenant>/v2.0`, the `iss` of good tokens.
    pub fn issuer(&self) -> String {
        self.issuer.clone()
    }

    /// The start configuration (later `/control` changes are not reflected).
    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// The same calls as `POST /control/<call>` with a JSON body.
    pub fn control(&self, call: &str, body: Value) -> Result<Value, String> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).control(call, body)
    }

    /// An access token for `user` (upn or oid), optionally in an error shape (`"expired"`, ...).
    pub fn token(&self, user: &str, shape: Option<&str>) -> Result<String, String> {
        self.control("token", json!({"user": user, "shape": shape})).map(|v| v["access_token"].as_str().unwrap_or_default().to_owned())
    }

    /// Serves until the process ends.
    pub fn wait(mut self) {
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.server.unblock();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_decoding() {
        let f = parse_form("a=1+2&b=%41%2f&c=%zz&d=%4&g=%+1&e&=x");
        assert_eq!((f["a"].as_str(), f["b"].as_str(), f["c"].as_str(), f["d"].as_str(), f["g"].as_str(), f["e"].as_str()), ("1 2", "A/", "%zz", "%4", "% 1", ""));
    }

    #[test]
    fn redirect_hosts() {
        for ok in ["http://127.0.0.1:1/x", "http://localhost", "http://[::1]", "http://[::1]:80/?a=b"] {
            assert!(loopback_redirect(ok), "{ok}");
        }
        for bad in ["http://127.0.0.1:x/", "http://[::2]:1/", "http://localhost.evil/", "http://127.0.0.1 /", "http://127.0.0.1/\r\nX: y", "http://user@127.0.0.1/"] {
            assert!(!loopback_redirect(bad), "{bad}");
        }
    }
}

//! The mock against a plain HTTP client (std TcpStream) and a reference relying party: `verify`
//! checks a token the way the relay must (spec 12.2): RS256 against the JWKS, `iss`, `tid`,
//! `aud`, `ver` 2.0, `exp`/`nbf` with 300 s skew, no groups overage pointer.

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use oidc_mock::{Config, Mock};
use rsa::pkcs1v15::{Signature, VerifyingKey};
use rsa::signature::Verifier;
use rsa::{BigUint, RsaPublicKey};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;

const SKEW: i64 = 300;

struct Resp {
    status: u16,
    headers: HashMap<String, String>,
    body: String,
}
impl Resp {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("{e}: {}", self.body))
    }
}

/// One request, `Connection: close`, the answer read to the end.
fn http(base: &str, method: &str, path: &str, body: Option<(&str, &str)>) -> Resp {
    let addr = base.strip_prefix("http://").unwrap().split('/').next().unwrap();
    let mut s = TcpStream::connect(addr).unwrap();
    let (ctype, data) = body.unwrap_or(("text/plain", ""));
    write!(s, "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n\r\n{data}", data.len()).unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    let mut lines = head.lines();
    let status = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    let headers = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned())).collect();
    Resp { status, headers, body: body.to_owned() }
}

/// Path and query of a URL on the mock.
fn path_of(url: &str) -> String {
    let rest = url.strip_prefix("http://").unwrap();
    rest[rest.find('/').unwrap()..].to_owned()
}

fn enc(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

fn form(pairs: &[(&str, &str)]) -> String {
    pairs.iter().map(|(k, v)| format!("{k}={}", enc(v))).collect::<Vec<_>>().join("&")
}

fn query(url: &str) -> HashMap<String, String> {
    let q = url.split_once('?').map(|x| x.1).unwrap_or("");
    q.split('&').filter_map(|p| p.split_once('=')).map(|(k, v)| (k.to_owned(), pct_decode(v))).collect()
}

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
            i += 3;
        } else {
            out.push(if b[i] == b'+' { b' ' } else { b[i] });
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

/// What a relying party knows from its configuration (the signed team document).
struct Expect<'a> {
    issuer: &'a str,
    tenant: &'a str,
    audience: &'a str,
    now: i64,
}

/// Reference token check: `Ok(claims)` or the reason it is refused.
fn verify(token: &str, jwks: &Value, x: &Expect) -> Result<Value, &'static str> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err("not a JWT");
    }
    let header: Value = serde_json::from_slice(&B64.decode(parts[0]).map_err(|_| "header")?).map_err(|_| "header")?;
    if header["alg"] != "RS256" {
        return Err("alg");
    }
    let key = jwks["keys"].as_array().unwrap().iter().find(|k| k["kid"] == header["kid"]).ok_or("unknown kid")?;
    let n = BigUint::from_bytes_be(&B64.decode(key["n"].as_str().unwrap()).unwrap());
    let e = BigUint::from_bytes_be(&B64.decode(key["e"].as_str().unwrap()).unwrap());
    let vk = VerifyingKey::<Sha256>::new(RsaPublicKey::new(n, e).unwrap());
    let sig = Signature::try_from(B64.decode(parts[2]).map_err(|_| "bad signature")?.as_slice()).map_err(|_| "bad signature")?;
    vk.verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &sig).map_err(|_| "bad signature")?;
    let c: Value = serde_json::from_slice(&B64.decode(parts[1]).map_err(|_| "claims")?).map_err(|_| "claims")?;
    if c["iss"] != x.issuer {
        return Err("iss");
    }
    if c["tid"] != x.tenant {
        return Err("tid");
    }
    if c["aud"] != x.audience {
        return Err("aud");
    }
    if c["ver"] != "2.0" {
        return Err("ver");
    }
    if c["exp"].as_i64().ok_or("exp")? + SKEW < x.now {
        return Err("expired");
    }
    if c["nbf"].as_i64().ok_or("nbf")? - SKEW > x.now {
        return Err("not yet valid");
    }
    if c.get("_claim_names").is_some() || c.get("_claim_sources").is_some() {
        return Err("groups overage");
    }
    Ok(c)
}

fn pkce() -> (String, String) {
    let verifier = B64.encode(Sha256::digest(format!("verifier {}", now()).as_bytes())); // 43 chars
    let challenge = B64.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

fn mock() -> Mock {
    Mock::start(Config { seed: Some(7), ..Config::default() }, "127.0.0.1:0", false).unwrap()
}

fn discovery(base: &str) -> Value {
    let r = http(base, "GET", &path_of(&format!("{base}/.well-known/openid-configuration")), None);
    assert_eq!(r.status, 200);
    r.json()
}

/// Authorize as `hint` and return the redirect's query.
fn authorize(m: &Mock, hint: &str, challenge: &str, redirect: &str) -> Resp {
    let q = form(&[("client_id", &m.config().client_id), ("response_type", "code"), ("redirect_uri", redirect), ("scope", &format!("{} openid profile offline_access", m.config().scope)), ("state", "st-1"), ("nonce", "nn-1"), ("code_challenge", challenge), ("code_challenge_method", "S256"), ("login_hint", hint)]);
    http(&m.base(), "GET", &format!("{}?{q}", path_of(&format!("{}/authorize", m.issuer()))), None)
}

fn exchange(m: &Mock, code: &str, verifier: &str, redirect: &str) -> Resp {
    let body = form(&[("grant_type", "authorization_code"), ("client_id", &m.config().client_id), ("code", code), ("redirect_uri", redirect), ("code_verifier", verifier)]);
    http(&m.base(), "POST", &path_of(&format!("{}/token", m.issuer())), Some(("application/x-www-form-urlencoded", &body)))
}

fn refresh(m: &Mock, rt: &str) -> Resp {
    let body = form(&[("grant_type", "refresh_token"), ("client_id", &m.config().client_id), ("refresh_token", rt)]);
    http(&m.base(), "POST", &path_of(&format!("{}/token", m.issuer())), Some(("application/x-www-form-urlencoded", &body)))
}

fn jwks(m: &Mock) -> Value {
    let r = http(&m.base(), "GET", &path_of(&format!("{}/jwks", m.issuer())), None);
    assert_eq!(r.status, 200);
    r.json()
}

fn expect<'a>(m: &'a Mock, issuer: &'a str) -> Expect<'a> {
    Expect { issuer, tenant: &m.config().tenant, audience: &m.config().audience, now: now() }
}

fn control(m: &Mock, action: &str, body: Value) -> Resp {
    http(&m.base(), "POST", &format!("/control/{action}"), Some(("application/json", &body.to_string())))
}

const REDIRECT: &str = "http://127.0.0.1:53111/callback";

/// The whole flow: returns the token answer.
fn sign_in(m: &Mock, hint: &str) -> Value {
    let (verifier, challenge) = pkce();
    let r = authorize(m, hint, &challenge, REDIRECT);
    assert_eq!(r.status, 302, "{}", r.body);
    let q = query(&r.headers["location"]);
    let t = exchange(m, &q["code"], &verifier, REDIRECT);
    assert_eq!(t.status, 200, "{}", t.body);
    t.json()
}

#[test]
fn discovery_and_jwks_shapes() {
    let m = mock();
    let d = discovery(&m.base());
    let iss = m.issuer();
    assert_eq!(iss, format!("{}/{}/v2.0", m.base(), m.config().tenant));
    assert_eq!(d["issuer"], iss);
    for (k, p) in [("authorization_endpoint", "authorize"), ("token_endpoint", "token"), ("jwks_uri", "jwks"), ("end_session_endpoint", "logout")] {
        assert_eq!(d[k], format!("{iss}/{p}"));
    }
    assert_eq!(d["id_token_signing_alg_values_supported"], json!(["RS256"]));
    assert_eq!(d["code_challenge_methods_supported"], json!(["S256"]));
    assert_eq!(d["response_types_supported"], json!(["code"]));
    // The issuer path serves the same discovery document (what a relying party fetches).
    assert_eq!(http(&m.base(), "GET", &path_of(&format!("{iss}/.well-known/openid-configuration")), None).json(), d);
    let j = jwks(&m);
    let keys = j["keys"].as_array().unwrap();
    assert_eq!(keys.len(), 1);
    let k = &keys[0];
    assert_eq!((k["kty"].as_str(), k["use"].as_str(), k["e"].as_str()), (Some("RSA"), Some("sig"), Some("AQAB")));
    assert_eq!(B64.decode(k["n"].as_str().unwrap()).unwrap().len(), 256, "2048-bit modulus");
    assert!(k["kid"].as_str().unwrap().len() >= 8);
    // Seeded: the same seed gives the same key, another seed another.
    assert_eq!(jwks(&mock()), j);
    let other = Mock::start(Config { seed: Some(8), ..Config::default() }, "127.0.0.1:0", false).unwrap();
    assert_ne!(jwks(&other)["keys"][0]["n"], k["n"]);
}

#[test]
fn code_flow_with_pkce() {
    let m = mock();
    let (verifier, challenge) = pkce();
    let r = authorize(&m, "software@tether.test", &challenge, REDIRECT);
    assert_eq!(r.status, 302, "{}", r.body);
    let loc = &r.headers["location"];
    assert!(loc.starts_with("http://127.0.0.1:53111/callback?"), "{loc}");
    let q = query(loc);
    assert_eq!(q["state"], "st-1");
    // Wrong and missing verifier refused; the code is used up by the first attempt.
    let bad = exchange(&m, &q["code"], &"x".repeat(43), REDIRECT);
    assert_eq!((bad.status, bad.json()["error"].as_str()), (400, Some("invalid_grant")));
    let again = exchange(&m, &q["code"], &verifier, REDIRECT);
    assert_eq!((again.status, again.json()["error"].as_str()), (400, Some("invalid_grant")), "single use");

    let r = authorize(&m, "software@tether.test", &challenge, REDIRECT);
    let code = query(&r.headers["location"])["code"].clone();
    let body = form(&[("grant_type", "authorization_code"), ("client_id", &m.config().client_id), ("code", &code), ("redirect_uri", REDIRECT)]);
    let missing = http(&m.base(), "POST", &path_of(&format!("{}/token", m.issuer())), Some(("application/x-www-form-urlencoded", &body)));
    assert_eq!(missing.status, 400, "no verifier: {}", missing.body);

    // Another redirect_uri at the token endpoint than at authorize is refused.
    let r = authorize(&m, "software@tether.test", &challenge, REDIRECT);
    let code = query(&r.headers["location"])["code"].clone();
    assert_eq!(exchange(&m, &code, &verifier, "http://127.0.0.1:53112/callback").status, 400);

    let t = sign_in(&m, "software@tether.test");
    assert_eq!(t["token_type"], "Bearer");
    assert!(t["expires_in"].as_u64().unwrap() > 0);
    assert!(t["refresh_token"].as_str().unwrap().len() > 20);
    let c = verify(t["access_token"].as_str().unwrap(), &jwks(&m), &expect(&m, &m.issuer())).unwrap();
    assert_eq!(c["preferred_username"], "software@tether.test");
    assert_eq!(c["roles"], json!(["Tether.Technician", "Tether.Software"]));
    for k in ["oid", "name", "uti", "iat", "sub", "azp", "scp"] {
        assert!(c.get(k).is_some(), "claim {k}");
    }
    // The id token carries the nonce and is addressed to the client.
    let id = verify(t["id_token"].as_str().unwrap(), &jwks(&m), &Expect { audience: &m.config().client_id, ..expect(&m, &m.issuer()) }).unwrap();
    assert_eq!((id["nonce"].as_str(), id["oid"].clone()), (Some("nn-1"), c["oid"].clone()));
}

#[test]
fn authorize_refuses_bad_requests() {
    let m = mock();
    let (_, challenge) = pkce();
    for bad in ["http://example.com/cb", "https://127.0.0.1/cb", "http://127.0.0.1@example.com/cb", "http://127.0.0.1.example.com/cb", "http://127.0.0.1:5/cb#x", "custom:/cb"] {
        let r = authorize(&m, "technician@tether.test", &challenge, bad);
        assert_eq!(r.status, 400, "{bad}");
        assert!(!r.headers.contains_key("location"), "never redirected to {bad}");
    }
    for ok in ["http://localhost:4000/", "http://[::1]:4000/cb", "http://127.0.0.1/"] {
        assert_eq!(authorize(&m, "technician@tether.test", &challenge, ok).status, 302, "{ok}");
    }
    // No challenge, or method plain: refused.
    let base = path_of(&format!("{}/authorize", m.issuer()));
    let q = form(&[("client_id", &m.config().client_id), ("response_type", "code"), ("redirect_uri", REDIRECT), ("state", "s"), ("login_hint", "technician@tether.test")]);
    assert_eq!(http(&m.base(), "GET", &format!("{base}?{q}"), None).status, 400);
    assert_eq!(http(&m.base(), "GET", &format!("{base}?{q}&code_challenge={challenge}&code_challenge_method=plain"), None).status, 400);
    // Unknown client.
    let q2 = q.replace(&m.config().client_id, "99999999-0000-0000-0000-000000000000");
    assert_eq!(http(&m.base(), "GET", &format!("{base}?{q2}&code_challenge={challenge}&code_challenge_method=S256"), None).status, 400);
    // Without login_hint: a page listing the users, each a link that signs in at once.
    let q3 = form(&[("client_id", &m.config().client_id), ("response_type", "code"), ("redirect_uri", REDIRECT), ("state", "s"), ("code_challenge", &challenge), ("code_challenge_method", "S256")]);
    let page = http(&m.base(), "GET", &format!("{base}?{q3}"), None);
    assert_eq!(page.status, 200);
    assert!(page.headers["content-type"].starts_with("text/html"));
    for u in &m.config().users {
        assert!(page.body.contains(&format!("login_hint={}", enc(&u.upn))), "{}", u.upn);
    }
    // /control/login selects the user for requests without login_hint.
    assert_eq!(control(&m, "login", json!({"user": "writer@tether.test"})).status, 200);
    let r = http(&m.base(), "GET", &format!("{base}?{q3}"), None);
    assert_eq!(r.status, 302);
    // A disabled user is sent back with access_denied and the state.
    let r = authorize(&m, "disabled@tether.test", &challenge, REDIRECT);
    let q = query(&r.headers["location"]);
    assert_eq!((q["error"].as_str(), q["state"].as_str()), ("access_denied", "st-1"));
    assert!(!q.contains_key("code"));
}

#[test]
fn codes_are_short_lived() {
    let m = Mock::start(Config { seed: Some(7), now: Some(1_900_000_000), ..Config::default() }, "127.0.0.1:0", false).unwrap();
    let (verifier, challenge) = pkce();
    let code = query(&authorize(&m, "technician@tether.test", &challenge, REDIRECT).headers["location"])["code"].clone();
    assert_eq!(control(&m, "clock", json!({"advance": 61})).status, 200);
    let r = exchange(&m, &code, &verifier, REDIRECT);
    assert_eq!((r.status, r.json()["error"].as_str()), (400, Some("invalid_grant")));
    // The fixed clock is what the tokens carry.
    let t = sign_in(&m, "technician@tether.test");
    let c = verify(t["access_token"].as_str().unwrap(), &jwks(&m), &Expect { now: 1_900_000_061, ..expect(&m, &m.issuer()) }).unwrap();
    assert_eq!(c["iat"], 1_900_000_061);
    assert_eq!(c["exp"], 1_900_000_061 + 3600);
}

#[test]
fn refresh_and_disabled_user() {
    let m = mock();
    let t = sign_in(&m, "technician@tether.test");
    let rt = t["refresh_token"].as_str().unwrap();
    let r = refresh(&m, rt);
    assert_eq!(r.status, 200, "{}", r.body);
    let t2 = r.json();
    let c1 = verify(t["access_token"].as_str().unwrap(), &jwks(&m), &expect(&m, &m.issuer())).unwrap();
    let c2 = verify(t2["access_token"].as_str().unwrap(), &jwks(&m), &expect(&m, &m.issuer())).unwrap();
    assert_eq!(c1["oid"], c2["oid"]);
    assert_ne!(c1["uti"], c2["uti"]);
    assert!(t2["refresh_token"].as_str().is_some());
    // Unknown refresh token.
    assert_eq!(refresh(&m, "nope").json()["error"], "invalid_grant");
    // Disabled: refresh fails like Entra's AADSTS50057.
    assert_eq!(control(&m, "user", json!({"upn": "technician@tether.test", "disabled": true})).status, 200);
    let r = refresh(&m, t2["refresh_token"].as_str().unwrap());
    let j = r.json();
    assert_eq!((r.status, j["error"].as_str()), (400, Some("invalid_grant")));
    assert_eq!(j["error_codes"], json!([50057]));
    // Re-enabled, refresh works again; a new user and new roles via /control/user.
    control(&m, "user", json!({"upn": "technician@tether.test", "disabled": false}));
    assert_eq!(refresh(&m, rt).status, 200);
    assert_eq!(control(&m, "user", json!({"oid": "aaaaaaaa-0000-4000-8000-00000000000a", "upn": "new@tether.test", "name": "New Person", "roles": ["Tether.Agent"]})).status, 200);
    let c = verify(sign_in(&m, "new@tether.test")["access_token"].as_str().unwrap(), &jwks(&m), &expect(&m, &m.issuer())).unwrap();
    assert_eq!((c["name"].as_str(), c["roles"].clone()), (Some("New Person"), json!(["Tether.Agent"])));
    // No offline_access: no refresh token.
    let (verifier, challenge) = pkce();
    let q = form(&[("client_id", &m.config().client_id), ("response_type", "code"), ("redirect_uri", REDIRECT), ("scope", &m.config().scope), ("state", "s"), ("code_challenge", &challenge), ("code_challenge_method", "S256"), ("login_hint", "technician@tether.test")]);
    let r = http(&m.base(), "GET", &format!("{}?{q}", path_of(&format!("{}/authorize", m.issuer()))), None);
    let t = exchange(&m, &query(&r.headers["location"])["code"], &verifier, REDIRECT).json();
    assert!(t.get("refresh_token").is_none());
}

#[test]
fn lifetime_control() {
    let m = mock();
    assert_eq!(control(&m, "lifetime", json!({"seconds": 120})).status, 200);
    let t = sign_in(&m, "technician@tether.test");
    assert_eq!(t["expires_in"], 120);
    let c = verify(t["access_token"].as_str().unwrap(), &jwks(&m), &expect(&m, &m.issuer())).unwrap();
    assert_eq!(c["exp"].as_i64().unwrap() - c["iat"].as_i64().unwrap(), 120);
}

#[test]
fn each_error_shape_is_refused_for_its_reason() {
    let m = mock();
    let iss = m.issuer();
    for (shape, reason) in [("expired", "expired"), ("not_yet_valid", "not yet valid"), ("wrong_aud", "aud"), ("wrong_iss", "iss"), ("wrong_tid", "tid"), ("wrong_ver", "ver"), ("overage", "groups overage"), ("unknown_kid", "unknown kid"), ("bad_signature", "bad signature")] {
        // Through the flow (sticky force) and through /control/token: the same shape.
        assert_eq!(control(&m, "force", json!({"shape": shape})).status, 200, "{shape}");
        let t = sign_in(&m, "technician@tether.test");
        assert_eq!(verify(t["access_token"].as_str().unwrap(), &jwks(&m), &expect(&m, &iss)).err(), Some(reason), "{shape} through the flow");
        control(&m, "force", json!({"shape": null}));
        let r = control(&m, "token", json!({"user": "technician@tether.test", "shape": shape}));
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(verify(r.json()["access_token"].as_str().unwrap(), &jwks(&m), &expect(&m, &iss)).err(), Some(reason), "{shape} through /control/token");
    }
    // Cleared: tokens are good again, through the flow and through the library helper.
    assert!(verify(sign_in(&m, "technician@tether.test")["access_token"].as_str().unwrap(), &jwks(&m), &expect(&m, &iss)).is_ok());
    assert!(verify(&m.token("technician@tether.test", None).unwrap(), &jwks(&m), &expect(&m, &iss)).is_ok());
    assert_eq!(verify(&m.token("technician@tether.test", Some("overage")).unwrap(), &jwks(&m), &expect(&m, &iss)).err(), Some("groups overage"));
    assert!(m.token("nobody@tether.test", None).is_err());
    assert_eq!(control(&m, "force", json!({"shape": "nonsense"})).status, 400);
    // The overage shape is Entra's: a pointer instead of a groups claim.
    let ov = m.token("technician@tether.test", Some("overage")).unwrap();
    let c: Value = serde_json::from_slice(&B64.decode(ov.split('.').nth(1).unwrap()).unwrap()).unwrap();
    assert_eq!(c["_claim_names"]["groups"], "src1");
    assert!(c["_claim_sources"]["src1"]["endpoint"].as_str().unwrap().contains("getMemberObjects"));
    assert!(c.get("groups").is_none());
}

#[test]
fn key_rotation_with_and_without_overlap() {
    let m = mock();
    let iss = m.issuer();
    let old = m.token("technician@tether.test", None).unwrap();
    let kid0 = jwks(&m)["keys"][0]["kid"].clone();
    let r = control(&m, "rotate", json!({"keep_old": true}));
    assert_eq!(r.status, 200);
    let kid1 = r.json()["kid"].clone();
    assert_ne!(kid0, kid1);
    let j = jwks(&m);
    assert_eq!(j["keys"].as_array().unwrap().len(), 2);
    assert!(verify(&old, &j, &expect(&m, &iss)).is_ok(), "overlap: the old key still verifies");
    let new = m.token("technician@tether.test", None).unwrap();
    let hdr: Value = serde_json::from_slice(&B64.decode(new.split('.').next().unwrap()).unwrap()).unwrap();
    assert_eq!(hdr["kid"], kid1, "new tokens are signed with the new key");
    assert!(verify(&new, &j, &expect(&m, &iss)).is_ok());
    // Without overlap: only the newest key is published; the older tokens' kid is unknown.
    control(&m, "rotate", json!({"keep_old": false}));
    let j = jwks(&m);
    assert_eq!(j["keys"].as_array().unwrap().len(), 1);
    assert_eq!(verify(&old, &j, &expect(&m, &iss)).err(), Some("unknown kid"));
    assert_eq!(verify(&new, &j, &expect(&m, &iss)).err(), Some("unknown kid"));
    assert!(verify(&m.token("technician@tether.test", None).unwrap(), &j, &expect(&m, &iss)).is_ok());
}

#[test]
fn control_answers_loopback_peers_only() {
    use std::net::SocketAddr;
    for (peer, ok) in [("127.0.0.1:1", true), ("127.9.9.9:1", true), ("[::1]:1", true), ("[::ffff:127.0.0.1]:1", true), ("10.0.0.5:1", false), ("192.168.1.2:1", false), ("[::ffff:10.0.0.5]:1", false), ("[fe80::1]:1", false)] {
        assert_eq!(oidc_mock::loopback_peer(Some(&peer.parse::<SocketAddr>().unwrap())), ok, "{peer}");
    }
    assert!(!oidc_mock::loopback_peer(None));
    // A non-loopback listen address is refused unless allowed explicitly.
    assert!(Mock::start(Config::default(), "0.0.0.0:0", false).is_err());
    assert!(Mock::start(Config::default(), "192.168.1.2:0", false).is_err());
}

/// A real non-loopback peer: Linux only (on Windows a listener on a LAN address raises the
/// firewall prompt). Uses the address of the default route; skipped when there is none.
#[cfg(target_os = "linux")]
#[test]
fn control_refuses_a_lan_peer() {
    let Ok(probe) = std::net::UdpSocket::bind("0.0.0.0:0") else { return };
    if probe.connect("192.0.2.1:9").is_err() {
        return;
    }
    let ip = probe.local_addr().unwrap().ip();
    if ip.is_loopback() || ip.is_unspecified() {
        return;
    }
    let m = Mock::start(Config { seed: Some(7), ..Config::default() }, &format!("{ip}:0"), true).unwrap();
    let base = format!("http://{ip}:{}", m.addr().port());
    assert_eq!(http(&base, "POST", "/control/lifetime", Some(("application/json", "{\"seconds\":5}"))).status, 403);
    assert_eq!(http(&base, "GET", &path_of(&format!("{}/jwks", m.issuer())), None).status, 200, "the provider endpoints still answer");
}

/// The binary as a test would run it: a child process on port 0, the address read from its first
/// line, then the whole code flow with a plain HTTP client and the token checked against the JWKS
/// named in the discovery document.
#[test]
fn end_to_end_against_the_binary() {
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_oidc-mock")).args(["--listen", "127.0.0.1:0", "--seed", "42"]).stdout(std::process::Stdio::piped()).spawn().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    let base = format!("http://{}", line.trim().strip_prefix("listening on ").expect("listen line"));
    let mut line2 = String::new();
    out.read_line(&mut line2).unwrap();
    let issuer = line2.trim().strip_prefix("issuer ").expect("issuer line").to_owned();

    let d = http(&base, "GET", &path_of(&format!("{issuer}/.well-known/openid-configuration")), None).json();
    assert_eq!(d["issuer"], issuer);
    let (verifier, challenge) = pkce();
    let tenant = issuer.trim_end_matches("/v2.0").rsplit('/').next().unwrap().to_owned();
    let defaults = Config::default();
    let q = form(&[("client_id", &defaults.client_id), ("response_type", "code"), ("redirect_uri", REDIRECT), ("scope", "openid offline_access"), ("state", "e2e"), ("nonce", "n-e2e"), ("code_challenge", &challenge), ("code_challenge_method", "S256"), ("login_hint", "writer@tether.test")]);
    let r = http(&base, "GET", &format!("{}?{q}", path_of(d["authorization_endpoint"].as_str().unwrap())), None);
    assert_eq!(r.status, 302, "{}", r.body);
    let cb = query(&r.headers["location"]);
    assert_eq!(cb["state"], "e2e");
    let body = form(&[("grant_type", "authorization_code"), ("client_id", &defaults.client_id), ("code", &cb["code"]), ("redirect_uri", REDIRECT), ("code_verifier", &verifier)]);
    let t = http(&base, "POST", &path_of(d["token_endpoint"].as_str().unwrap()), Some(("application/x-www-form-urlencoded", &body))).json();
    let keys = http(&base, "GET", &path_of(d["jwks_uri"].as_str().unwrap()), None).json();
    let c = verify(t["access_token"].as_str().unwrap(), &keys, &Expect { issuer: &issuer, tenant: &tenant, audience: &defaults.audience, now: now() }).unwrap();
    assert_eq!(c["preferred_username"], "writer@tether.test");
    assert!(c["roles"].as_array().unwrap().iter().any(|r| r == "Tether.Technician.Write"));
    // Logout redirects to a loopback address only.
    let lo = http(&base, "GET", &format!("{}?post_logout_redirect_uri={}", path_of(d["end_session_endpoint"].as_str().unwrap()), enc("http://127.0.0.1:9/bye")), None);
    assert_eq!((lo.status, lo.headers["location"].as_str()), (302, "http://127.0.0.1:9/bye"));
    assert_eq!(http(&base, "GET", &format!("{}?post_logout_redirect_uri={}", path_of(d["end_session_endpoint"].as_str().unwrap()), enc("http://example.com/")), None).status, 200);
    child.kill().unwrap();
    child.wait().unwrap();

    // The binary refuses a non-loopback listen address without --allow-network.
    let st = std::process::Command::new(env!("CARGO_BIN_EXE_oidc-mock")).args(["--listen", "0.0.0.0:0"]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().unwrap();
    assert!(!st.success());
}

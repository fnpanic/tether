//! The mock against a plain HTTP client (std TcpStream) and a reference relying party, `verify`.

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use oidc_mock::{Config, Mock};
use rsa::pkcs1v15::{Signature, SigningKey, VerifyingKey};
use rsa::signature::{SignatureEncoding, Signer, Verifier};
use rsa::traits::PublicKeyParts;
use rsa::{BigUint, RsaPrivateKey, RsaPublicKey};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;

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

/// One request with the given header lines (each ending in \r\n), `Connection: close`, the answer read to the end.
fn http_raw(base: &str, method: &str, path: &str, headers: &str, body: &str) -> Resp {
    let addr = base.strip_prefix("http://").unwrap().split('/').next().unwrap();
    let mut s = TcpStream::connect(addr).unwrap();
    write!(s, "{method} {path} HTTP/1.1\r\n{headers}Connection: close\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    let mut lines = head.lines();
    let status = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    let headers = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned())).collect();
    Resp { status, headers, body: body.to_owned() }
}

fn http(base: &str, method: &str, path: &str, body: Option<(&str, &str)>) -> Resp {
    let addr = base.strip_prefix("http://").unwrap().split('/').next().unwrap();
    let (ctype, data) = body.unwrap_or(("text/plain", ""));
    http_raw(base, method, path, &format!("Host: {addr}\r\nContent-Type: {ctype}\r\n"), data)
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

// ---------------------------------------------------------------------------------------------
// Reference relying-party check. TEST CODE, written to be read by the author of the relay's
// verifier (spec 12.2), not a library to call: no JWKS fetching, caching or refresh, the clock is
// passed in, and the error strings exist for these tests. Every check below is one the relay needs.
// ---------------------------------------------------------------------------------------------

/// Clock skew allowed on `exp` and `nbf` (the same 300 s as the request signatures).
const SKEW: i64 = 300;
/// Longest `exp - nbf` accepted: Entra's long-lived (CAE) tokens live 20 to 28 hours.
const MAX_LIFETIME: i64 = 28 * 3600;
/// Smallest RSA modulus accepted.
const MIN_RSA_BITS: usize = 2048;

/// What a relying party knows from its configuration (the signed team document).
struct Expect<'a> {
    issuer: &'a str,
    tenant: &'a str,
    /// The API's client id (v2 tokens carry it as `aud`).
    audience: &'a str,
    /// The public client the token must have been issued to (`azp`); `None` skips the check.
    client_id: Option<&'a str>,
    now: i64,
}

fn b64_json(s: &str) -> Option<Value> {
    serde_json::from_slice(&B64.decode(s).ok()?).ok()
}

/// `Ok(claims)` or the reason the token is refused.
fn verify(token: &str, jwks: &Value, x: &Expect) -> Result<Value, &'static str> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err("not a JWT");
    }
    let header = b64_json(parts[0]).ok_or("header")?;
    // The algorithm is pinned here, never taken from the token or the key.
    if header["alg"] != "RS256" {
        return Err("alg");
    }
    // A token without kid matches no key (never "try every key").
    let kid = header["kid"].as_str().ok_or("no kid")?;
    let key = jwks["keys"].as_array().ok_or("jwks")?.iter().find(|k| k["kid"].as_str() == Some(kid)).ok_or("unknown kid")?;
    if key["kty"] != "RSA" {
        return Err("key type");
    }
    if key.get("use").is_some_and(|u| u != "sig") {
        return Err("key use");
    }
    if key.get("alg").is_some_and(|a| a != "RS256") {
        return Err("key alg");
    }
    let field = |f: &str| key[f].as_str().and_then(|s| B64.decode(s).ok()).map(|b| BigUint::from_bytes_be(&b)).ok_or("jwks");
    let (n, e) = (field("n")?, field("e")?);
    if n.bits() < MIN_RSA_BITS {
        return Err("key too small");
    }
    let vk = VerifyingKey::<Sha256>::new(RsaPublicKey::new(n, e).map_err(|_| "jwks")?);
    let sig = Signature::try_from(B64.decode(parts[2]).map_err(|_| "bad signature")?.as_slice()).map_err(|_| "bad signature")?;
    vk.verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &sig).map_err(|_| "bad signature")?;
    let c = b64_json(parts[1]).ok_or("claims")?;
    if c["iss"] != x.issuer {
        return Err("iss");
    }
    if c["tid"] != x.tenant {
        return Err("tid");
    }
    let aud_ok = match &c["aud"] {
        Value::String(a) => a == x.audience,
        Value::Array(a) => a.iter().any(|a| a == x.audience),
        _ => false,
    };
    if !aud_ok {
        return Err("aud");
    }
    if c["ver"] != "2.0" {
        return Err("ver");
    }
    if let Some(client) = x.client_id {
        if c["azp"] != client {
            return Err("azp");
        }
    }
    let (exp, nbf) = (c["exp"].as_i64().ok_or("exp")?, c["nbf"].as_i64().ok_or("nbf")?);
    if x.now > exp + SKEW {
        return Err("expired");
    }
    if nbf - SKEW > x.now {
        return Err("not yet valid");
    }
    if exp - nbf > MAX_LIFETIME {
        return Err("lifetime");
    }
    // Roles only: any sign of the groups overage means the token's authorization is incomplete.
    if c.get("_claim_names").is_some() || c.get("_claim_sources").is_some() || c.get("hasgroups").is_some() {
        return Err("groups overage");
    }
    Ok(c)
}

// ---------------------------------------------------------------------------------------------

fn pkce() -> (String, String) {
    let verifier = B64.encode(Sha256::digest(format!("verifier {}", now()).as_bytes())); // 43 chars
    let challenge = B64.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

fn mock() -> Mock {
    Mock::start(Config { seed: Some(7), ..Config::default() }, "127.0.0.1:0", false).unwrap()
}

/// An endpoint's path, from the discovery document.
fn ep(m: &Mock, name: &str) -> String {
    path_of(m.discovery()[name].as_str().unwrap())
}

fn api_scope(m: &Mock) -> String {
    format!("{} openid profile offline_access", m.config().scope)
}

/// Authorize as `hint` with the API scope.
fn authorize(m: &Mock, hint: &str, challenge: &str, redirect: &str) -> Resp {
    authorize_scope(m, hint, challenge, redirect, &api_scope(m))
}

fn authorize_scope(m: &Mock, hint: &str, challenge: &str, redirect: &str, scope: &str) -> Resp {
    let q = form(&[("client_id", &m.config().client_id), ("response_type", "code"), ("redirect_uri", redirect), ("scope", scope), ("state", "st-1"), ("nonce", "nn-1"), ("code_challenge", challenge), ("code_challenge_method", "S256"), ("login_hint", hint)]);
    http(&m.base(), "GET", &format!("{}?{q}", ep(m, "authorization_endpoint")), None)
}

fn exchange(m: &Mock, code: &str, verifier: &str, redirect: &str) -> Resp {
    let body = form(&[("grant_type", "authorization_code"), ("client_id", &m.config().client_id), ("code", code), ("redirect_uri", redirect), ("code_verifier", verifier)]);
    http(&m.base(), "POST", &ep(m, "token_endpoint"), Some(("application/x-www-form-urlencoded", &body)))
}

fn refresh(m: &Mock, rt: &str) -> Resp {
    let body = form(&[("grant_type", "refresh_token"), ("client_id", &m.config().client_id), ("refresh_token", rt)]);
    http(&m.base(), "POST", &ep(m, "token_endpoint"), Some(("application/x-www-form-urlencoded", &body)))
}

fn jwks(m: &Mock) -> Value {
    let r = http(&m.base(), "GET", &ep(m, "jwks_uri"), None);
    assert_eq!(r.status, 200);
    r.json()
}

fn expect<'a>(m: &'a Mock, issuer: &'a str) -> Expect<'a> {
    Expect { issuer, tenant: &m.config().tenant, audience: &m.config().audience, client_id: Some(&m.config().client_id), now: now() }
}

fn control(m: &Mock, action: &str, body: Value) -> Resp {
    http(&m.base(), "POST", &format!("/control/{action}"), Some(("application/json", &body.to_string())))
}

fn claims(token: &str) -> Value {
    b64_json(token.split('.').nth(1).unwrap()).unwrap()
}

fn header(token: &str) -> Value {
    b64_json(token.split('.').next().unwrap()).unwrap()
}

/// Design 12.2 registers `http://localhost` (any port, this path).
const REDIRECT: &str = "http://localhost:53111/";

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
    let iss = m.issuer();
    assert_eq!(iss, format!("{}/{}/v2.0", m.base(), m.config().tenant));
    let d = http(&m.base(), "GET", &path_of(&format!("{iss}/.well-known/openid-configuration")), None).json();
    assert_eq!(d, m.discovery());
    assert_eq!(d["issuer"], iss);
    // Entra's layout: the endpoints are not under the issuer path.
    let root = format!("{}/{}", m.base(), m.config().tenant);
    for (k, p) in [("authorization_endpoint", "oauth2/v2.0/authorize"), ("token_endpoint", "oauth2/v2.0/token"), ("jwks_uri", "discovery/v2.0/keys"), ("end_session_endpoint", "oauth2/v2.0/logout")] {
        assert_eq!(d[k], format!("{root}/{p}"));
    }
    assert_eq!(d["id_token_signing_alg_values_supported"], json!(["RS256"]));
    assert_eq!(d["code_challenge_methods_supported"], json!(["S256"]));
    assert_eq!(d["response_types_supported"], json!(["code"]));
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

/// A client that builds endpoint URLs from the issuer instead of reading discovery gets 404.
#[test]
fn endpoints_are_found_through_discovery_only() {
    let m = mock();
    let iss = m.issuer();
    let t = &m.config().tenant;
    for p in ["/.well-known/openid-configuration", "/jwks", "/token", "/authorize", "/logout", "/discovery/v2.0/keys", "/oauth2/v2.0/token"] {
        assert_eq!(http(&m.base(), "GET", p, None).status, 404, "root {p}");
    }
    for p in ["/jwks", "/token", "/authorize", "/logout", "/keys"] {
        assert_eq!(http(&m.base(), "GET", &path_of(&format!("{iss}{p}")), None).status, 404, "issuer{p}");
        assert_eq!(http(&m.base(), "POST", &path_of(&format!("{iss}{p}")), Some(("application/x-www-form-urlencoded", "grant_type=refresh_token"))).status, 404, "POST issuer{p}");
    }
    assert_eq!(http(&m.base(), "GET", &format!("/{t}/.well-known/openid-configuration"), None).status, 404, "the v1 metadata path is not served");
    assert_eq!(http(&m.base(), "GET", &format!("/{t}/control/lifetime"), None).status, 404, "control is at the root only");
}

#[test]
fn code_flow_with_pkce() {
    let m = mock();
    let (verifier, challenge) = pkce();
    let r = authorize(&m, "software@tether.test", &challenge, REDIRECT);
    assert_eq!(r.status, 302, "{}", r.body);
    let loc = &r.headers["location"];
    assert!(loc.starts_with("http://localhost:53111/?"), "{loc}");
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
    let missing = http(&m.base(), "POST", &ep(&m, "token_endpoint"), Some(("application/x-www-form-urlencoded", &body)));
    assert_eq!(missing.status, 400, "no verifier: {}", missing.body);

    // Another redirect_uri at the token endpoint than at authorize is refused (even one that matches the registration).
    let r = authorize(&m, "software@tether.test", &challenge, REDIRECT);
    let code = query(&r.headers["location"])["code"].clone();
    assert_eq!(exchange(&m, &code, &verifier, "http://localhost:53112/").status, 400);

    let t = sign_in(&m, "software@tether.test");
    assert_eq!(t["token_type"], "Bearer");
    assert!(t["expires_in"].as_u64().unwrap() > 0);
    assert!(t["refresh_token"].as_str().unwrap().len() > 20);
    let c = verify(t["access_token"].as_str().unwrap(), &jwks(&m), &expect(&m, &m.issuer())).unwrap();
    assert_eq!(c["preferred_username"], "software@tether.test");
    assert_eq!(c["roles"], json!(["Tether.Technician", "Tether.Software"]));
    assert_eq!((c["aud"].as_str(), c["scp"].as_str()), (Some(m.config().audience.as_str()), Some("access")));
    for k in ["oid", "name", "uti", "iat", "sub", "azp", "scp"] {
        assert!(c.get(k).is_some(), "claim {k}");
    }
    // The id token carries the nonce and is addressed to the client.
    let id = verify(t["id_token"].as_str().unwrap(), &jwks(&m), &Expect { audience: &m.config().client_id, client_id: None, ..expect(&m, &m.issuer()) }).unwrap();
    assert_eq!((id["nonce"].as_str(), id["oid"].clone()), (Some("nn-1"), c["oid"].clone()));
}

/// A client that forgets the API scope gets what Entra gives it: a Microsoft Graph token (for
/// UserInfo) that the relay's check refuses, with no roles and not addressed to the API.
#[test]
fn without_the_api_scope_the_token_is_for_graph() {
    let m = mock();
    let (verifier, challenge) = pkce();
    let r = authorize_scope(&m, "writer@tether.test", &challenge, REDIRECT, "openid profile offline_access");
    let t = exchange(&m, &query(&r.headers["location"])["code"], &verifier, REDIRECT).json();
    let at = t["access_token"].as_str().unwrap();
    assert_eq!(verify(at, &jwks(&m), &expect(&m, &m.issuer())).err(), Some("bad signature"), "Graph tokens do not verify as JWTs");
    let c = claims(at);
    assert_eq!((c["aud"].as_str(), c["ver"].as_str()), (Some(oidc_mock::GRAPH_AUDIENCE), Some("1.0")));
    assert!(c.get("roles").is_none(), "no API roles");
    assert_ne!(c["scp"], "access");
    assert!(header(at)["nonce"].is_string());
    // Its refresh gives the same kind of token; the id token is unaffected.
    assert!(verify(t["id_token"].as_str().unwrap(), &jwks(&m), &Expect { audience: &m.config().client_id, client_id: None, ..expect(&m, &m.issuer()) }).is_ok());
    let r = refresh(&m, t["refresh_token"].as_str().unwrap()).json();
    assert_eq!(claims(r["access_token"].as_str().unwrap())["aud"], oidc_mock::GRAPH_AUDIENCE);
    // A forced shape does not turn it into an API token.
    control(&m, "force", json!({"shape": "wrong_aud"}));
    let r = refresh(&m, t["refresh_token"].as_str().unwrap()).json();
    assert_eq!(claims(r["access_token"].as_str().unwrap())["aud"], oidc_mock::GRAPH_AUDIENCE);
}

#[test]
fn redirect_uris_match_the_registration() {
    let m = mock();
    let (_, challenge) = pkce();
    // Not registered: 127.0.0.1 is another host than localhost; the path counts; [::1] is not supported by Entra.
    for bad in ["http://127.0.0.1:53111/", "http://localhost:53111/callback", "http://[::1]:53111/", "https://localhost/", "http://example.com/", "http://localhost@example.com/", "http://localhost.example.com/", "http://localhost:5/#x", "custom:/cb"] {
        let r = authorize(&m, "technician@tether.test", &challenge, bad);
        assert_eq!(r.status, 400, "{bad}");
        assert!(r.body.contains("AADSTS50011"), "{bad}: {}", r.body);
        assert!(!r.headers.contains_key("location"), "never redirected to {bad}");
    }
    // The port is ignored for localhost; no path and / are the same.
    for ok in ["http://localhost:4000/", "http://localhost", "http://localhost:1/"] {
        assert_eq!(authorize(&m, "technician@tether.test", &challenge, ok).status, 302, "{ok}");
    }
    // A registration that lists 127.0.0.1 with a path accepts it on any port, and only it.
    let m2 = Mock::start(Config { seed: Some(7), redirect_uris: vec!["http://127.0.0.1/cb".into()], ..Config::default() }, "127.0.0.1:0", false).unwrap();
    assert_eq!(authorize(&m2, "technician@tether.test", &challenge, "http://127.0.0.1:53111/cb").status, 302);
    assert_eq!(authorize(&m2, "technician@tether.test", &challenge, "http://localhost:53111/cb").status, 400);
}

#[test]
fn authorize_refuses_bad_requests() {
    let m = mock();
    let (_, challenge) = pkce();
    // No challenge, or method plain: refused.
    let base = ep(&m, "authorization_endpoint");
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

/// Default: a user without roles signs in (a token without `roles`); with "assignment required"
/// the sign-in is refused at authorize with AADSTS50105.
#[test]
fn assignment_required() {
    let m = mock();
    let (_, challenge) = pkce();
    let c = verify(sign_in(&m, "noroles@tether.test")["access_token"].as_str().unwrap(), &jwks(&m), &expect(&m, &m.issuer())).unwrap();
    assert!(c.get("roles").is_none());
    assert_eq!(control(&m, "assignment", json!({"required": true})).status, 200);
    let q = query(&authorize(&m, "noroles@tether.test", &challenge, REDIRECT).headers["location"]);
    assert_eq!((q["error"].as_str(), q["state"].as_str()), ("access_denied", "st-1"));
    assert!(q["error_description"].starts_with("AADSTS50105"), "{}", q["error_description"]);
    assert!(!q.contains_key("code"));
    assert_eq!(authorize(&m, "technician@tether.test", &challenge, REDIRECT).status, 302, "a user with a role still signs in");
    control(&m, "assignment", json!({"required": false}));
    assert!(query(&authorize(&m, "noroles@tether.test", &challenge, REDIRECT).headers["location"]).contains_key("code"));
    assert_eq!(control(&m, "assignment", json!({})).status, 400);
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
    let r = authorize_scope(&m, "technician@tether.test", &challenge, REDIRECT, &m.config().scope);
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
    // Longer than any real token: refused by the lifetime bound.
    control(&m, "lifetime", json!({"seconds": 29 * 3600}));
    assert_eq!(verify(&m.token("technician@tether.test", None).unwrap(), &jwks(&m), &expect(&m, &m.issuer())).err(), Some("lifetime"));
}

#[test]
fn each_error_shape_is_refused_for_its_reason() {
    let m = mock();
    let iss = m.issuer();
    let shapes = [("expired", "expired"), ("not_yet_valid", "not yet valid"), ("wrong_aud", "aud"), ("wrong_iss", "iss"), ("wrong_tid", "tid"), ("wrong_ver", "ver"), ("wrong_azp", "azp"), ("overage", "groups overage"), ("hasgroups", "groups overage"), ("unknown_kid", "unknown kid"), ("bad_signature", "bad signature"), ("v1", "iss"), ("aud_app_id_uri", "aud"), ("graph", "bad signature")];
    for (shape, reason) in shapes {
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
    assert!(m.token("nobody@tether.test", None).is_err());
    assert_eq!(control(&m, "force", json!({"shape": "nonsense"})).status, 400);
    // The overage shape is Entra's: a pointer instead of a groups claim.
    let c = claims(&m.token("technician@tether.test", Some("overage")).unwrap());
    assert_eq!(c["_claim_names"]["groups"], "src1");
    assert!(c["_claim_sources"]["src1"]["endpoint"].as_str().unwrap().contains("getMemberObjects"));
    assert!(c.get("groups").is_none());
    assert_eq!(claims(&m.token("technician@tether.test", Some("hasgroups")).unwrap())["hasgroups"], true);
    assert_eq!(claims(&m.token("technician@tether.test", Some("wrong_azp")).unwrap())["azp"], oidc_mock::OTHER_CLIENT);
}

/// A registration left at the default access token version gets v1 tokens: other issuer, `aud` the
/// App ID URI, `appid` instead of `azp`, `upn`/`unique_name` instead of `preferred_username`.
#[test]
fn v1_and_app_id_uri_shapes() {
    let m = mock();
    let app_id_uri = m.config().scope.rsplit_once('/').unwrap().0.to_owned();
    let t = m.token("writer@tether.test", Some("v1")).unwrap();
    let c = claims(&t);
    assert_eq!(c["iss"], format!("https://sts.windows.net/{}/", m.config().tenant));
    assert_eq!((c["ver"].as_str(), c["aud"].as_str(), c["appid"].as_str()), (Some("1.0"), Some(app_id_uri.as_str()), Some(m.config().client_id.as_str())));
    assert_eq!((c["upn"].as_str(), c["unique_name"].as_str()), (Some("writer@tether.test"), Some("writer@tether.test")));
    for absent in ["azp", "azpacr", "preferred_username"] {
        assert!(c.get(absent).is_none(), "{absent}");
    }
    assert_eq!((c["appidacr"].as_str(), c["scp"].as_str()), (Some("0"), Some("access")));
    assert!(c["roles"].as_array().unwrap().iter().any(|r| r == "Tether.Agent"));
    assert_eq!(header(&t)["x5t"], header(&t)["kid"]);
    // A properly signed token: refused for what it says, not for its signature.
    let v1 = Expect { issuer: c["iss"].as_str().unwrap(), audience: &app_id_uri, ..expect(&m, "") };
    assert_eq!(verify(&t, &jwks(&m), &v1).err(), Some("ver"));
    // A v2 token addressed to the App ID URI: refused by a check expecting the client id, accepted by one expecting the URI.
    let t = m.token("writer@tether.test", Some("aud_app_id_uri")).unwrap();
    assert_eq!(claims(&t)["aud"], app_id_uri);
    assert_eq!(verify(&t, &jwks(&m), &expect(&m, &m.issuer())).err(), Some("aud"));
    assert!(verify(&t, &jwks(&m), &Expect { audience: &app_id_uri, ..expect(&m, &m.issuer()) }).is_ok());
}

/// A token signed by a key the test makes, for the checks the mock's own tokens cannot reach.
fn self_signed(bits: usize, header: Value, claims: &Value) -> (String, Value) {
    let mut rng = <rand_chacha::ChaCha20Rng as rand_core::SeedableRng>::seed_from_u64(bits as u64);
    let k = RsaPrivateKey::new(&mut rng, bits).unwrap();
    let jwks = json!({"keys": [{"kty": "RSA", "use": "sig", "kid": "test", "n": B64.encode(k.n().to_bytes_be()), "e": B64.encode(k.e().to_bytes_be())}]});
    let input = format!("{}.{}", B64.encode(header.to_string()), B64.encode(claims.to_string()));
    let sig = SigningKey::<Sha256>::new(k).sign(input.as_bytes()).to_vec();
    (format!("{input}.{}", B64.encode(sig)), jwks)
}

#[test]
fn verify_refuses_bad_headers_and_keys() {
    let m = mock();
    let iss = m.issuer();
    let x = expect(&m, &iss);
    let good = m.token("technician@tether.test", None).unwrap();
    let j = jwks(&m);
    assert!(verify(&good, &j, &x).is_ok());
    let p: Vec<&str> = good.split('.').collect();
    let (body, sig) = (p[1], p[2]);
    let kid = header(&good)["kid"].clone();
    let with_header = |h: Value| format!("{}.{body}.{sig}", B64.encode(h.to_string()));
    assert_eq!(verify(&with_header(json!({"typ": "JWT", "alg": "RS256"})), &j, &x).err(), Some("no kid"));
    assert_eq!(verify(&with_header(json!({"typ": "JWT", "alg": "none", "kid": kid})), &j, &x).err(), Some("alg"));
    assert_eq!(verify(&with_header(json!({"typ": "JWT", "alg": "HS256", "kid": kid})), &j, &x).err(), Some("alg"));
    // The key published for the kid must be an RSA signing key; malformed JWKS fields are refusals, not panics.
    let mut k = j.clone();
    k["keys"][0]["kty"] = json!("EC");
    assert_eq!(verify(&good, &k, &x).err(), Some("key type"));
    let mut k = j.clone();
    k["keys"][0]["use"] = json!("enc");
    assert_eq!(verify(&good, &k, &x).err(), Some("key use"));
    let mut k = j.clone();
    k["keys"][0].as_object_mut().unwrap().remove("use");
    assert!(verify(&good, &k, &x).is_ok(), "use is optional");
    let mut k = j.clone();
    k["keys"][0]["n"] = json!(42);
    assert_eq!(verify(&good, &k, &x).err(), Some("jwks"));
    let mut k = j.clone();
    k["keys"][0]["e"] = json!("***");
    assert_eq!(verify(&good, &k, &x).err(), Some("jwks"));
    assert_eq!(verify(&good, &json!({"keys": 1}), &x).err(), Some("jwks"));
    // A 1024-bit key is refused even with a valid signature.
    let c = claims(&good);
    let (t, k) = self_signed(1024, json!({"typ": "JWT", "alg": "RS256", "kid": "test"}), &c);
    assert_eq!(verify(&t, &k, &x).err(), Some("key too small"));
    // aud as an array: accepted when it names the API, refused otherwise.
    let h = json!({"typ": "JWT", "alg": "RS256", "kid": "test"});
    let mut c2 = c.clone();
    c2["aud"] = json!([oidc_mock::OTHER_AUDIENCE, m.config().audience]);
    let (t, k) = self_signed(2048, h.clone(), &c2);
    assert!(verify(&t, &k, &x).is_ok());
    c2["aud"] = json!([oidc_mock::OTHER_AUDIENCE]);
    let (t, k) = self_signed(2048, h, &c2);
    assert_eq!(verify(&t, &k, &x).err(), Some("aud"));
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
    assert_eq!(r.json()["signing"], kid1);
    let j = jwks(&m);
    assert_eq!(j["keys"].as_array().unwrap().len(), 2);
    assert!(verify(&old, &j, &expect(&m, &iss)).is_ok(), "overlap: the old key still verifies");
    let new = m.token("technician@tether.test", None).unwrap();
    assert_eq!(header(&new)["kid"], kid1, "new tokens are signed with the new key");
    assert!(verify(&new, &j, &expect(&m, &iss)).is_ok());
    // Without overlap: only the newest key is published; the older tokens' kid is unknown.
    control(&m, "rotate", json!({"keep_old": false}));
    let j = jwks(&m);
    assert_eq!(j["keys"].as_array().unwrap().len(), 1);
    assert_eq!(verify(&old, &j, &expect(&m, &iss)).err(), Some("unknown kid"));
    assert_eq!(verify(&new, &j, &expect(&m, &iss)).err(), Some("unknown kid"));
    assert!(verify(&m.token("technician@tether.test", None).unwrap(), &j, &expect(&m, &iss)).is_ok());
}

/// A real rollover: the new key is published first, tokens are still signed with the old one; when
/// signing switches, a relying party that fetched the JWKS after publication verifies without refetching.
#[test]
fn key_rotation_published_before_use() {
    let m = mock();
    let iss = m.issuer();
    let kid0 = jwks(&m)["keys"][0]["kid"].clone();
    let r = control(&m, "rotate", json!({"stage": true})).json();
    let staged = r["kid"].clone();
    assert_ne!(staged, kid0);
    assert_eq!(r["signing"], kid0, "still signing with the old key");
    let cached = jwks(&m);
    assert_eq!(cached["keys"].as_array().unwrap().len(), 2, "the new key is published");
    let t = m.token("technician@tether.test", None).unwrap();
    assert_eq!(header(&t)["kid"], kid0);
    // Switch: the staged key signs now (no new key is made), the old one is dropped.
    let r = control(&m, "rotate", json!({"keep_old": false})).json();
    assert_eq!((r["kid"].clone(), r["signing"].clone()), (staged.clone(), staged.clone()));
    let t = m.token("technician@tether.test", None).unwrap();
    assert_eq!(header(&t)["kid"], staged);
    assert!(verify(&t, &cached, &expect(&m, &iss)).is_ok(), "the JWKS fetched before the switch verifies the new tokens");
    assert_eq!(jwks(&m)["keys"].as_array().unwrap().len(), 1);
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

/// A web page in a developer's browser, or a DNS-rebound name, reaches the mock from a loopback
/// peer: it is refused for its Host header or for not sending application/json.
#[test]
fn control_refuses_browser_requests() {
    let m = mock();
    let addr = m.addr();
    let body = "{\"seconds\":5}";
    let send = |headers: String| http_raw(&m.base(), "POST", "/control/lifetime", &headers, body).status;
    assert_eq!(send(format!("Host: {addr}\r\nContent-Type: application/json\r\n")), 200);
    assert_eq!(send(format!("Host: localhost:{}\r\nContent-Type: application/json; charset=utf-8\r\n", addr.port())), 200);
    assert_eq!(send(format!("Host: rebind.example:{}\r\nContent-Type: application/json\r\n", addr.port())), 403, "DNS rebinding");
    assert_eq!(send("Content-Type: application/json\r\n".into()), 403, "no Host");
    for ctype in ["text/plain", "application/x-www-form-urlencoded", "multipart/form-data; boundary=x"] {
        assert_eq!(send(format!("Host: {addr}\r\nContent-Type: {ctype}\r\n")), 415, "{ctype}");
    }
    assert_eq!(send(format!("Host: {addr}\r\n")), 415, "no Content-Type");
}

/// The issuer must be usable: `0.0.0.0` needs a named host; the named host is what the issuer carries.
#[test]
fn issuer_host() {
    let e = Mock::start(Config::default(), "0.0.0.0:0", true).err().unwrap();
    assert!(e.contains("--issuer-host"), "{e}");
    assert!(Mock::start(Config::default(), "[::]:0", true).is_err());
    assert!(Mock::start(Config { issuer_host: Some("a/b".into()), ..Config::default() }, "127.0.0.1:0", false).is_err());
    let m = Mock::start(Config { seed: Some(7), issuer_host: Some("localhost".into()), ..Config::default() }, "127.0.0.1:0", false).unwrap();
    assert_eq!(m.issuer(), format!("http://localhost:{}/{}/v2.0", m.addr().port(), m.config().tenant));
    assert!(m.discovery()["token_endpoint"].as_str().unwrap().starts_with(&format!("http://localhost:{}/", m.addr().port())));
    assert_eq!(claims(&m.token("technician@tether.test", None).unwrap())["iss"], m.issuer());
    // The binary: 0.0.0.0 with --allow-network but without --issuer-host is refused before listening.
    let st = std::process::Command::new(env!("CARGO_BIN_EXE_oidc-mock")).args(["--listen", "0.0.0.0:0", "--allow-network"]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().unwrap();
    assert_eq!(st.code(), Some(2));
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
    // Even with a loopback Host header: the peer decides.
    let r = http_raw(&base, "POST", "/control/lifetime", &format!("Host: 127.0.0.1:{}\r\nContent-Type: application/json\r\n", m.addr().port()), "{\"seconds\":5}");
    assert_eq!(r.status, 403);
    assert_eq!(http(&base, "GET", &ep(&m, "jwks_uri"), None).status, 200, "the provider endpoints still answer");
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
    let scope = format!("{} openid offline_access", defaults.scope);
    let q = form(&[("client_id", &defaults.client_id), ("response_type", "code"), ("redirect_uri", REDIRECT), ("scope", &scope), ("state", "e2e"), ("nonce", "n-e2e"), ("code_challenge", &challenge), ("code_challenge_method", "S256"), ("login_hint", "writer@tether.test")]);
    let r = http(&base, "GET", &format!("{}?{q}", path_of(d["authorization_endpoint"].as_str().unwrap())), None);
    assert_eq!(r.status, 302, "{}", r.body);
    let cb = query(&r.headers["location"]);
    assert_eq!(cb["state"], "e2e");
    let body = form(&[("grant_type", "authorization_code"), ("client_id", &defaults.client_id), ("code", &cb["code"]), ("redirect_uri", REDIRECT), ("code_verifier", &verifier)]);
    let t = http(&base, "POST", &path_of(d["token_endpoint"].as_str().unwrap()), Some(("application/x-www-form-urlencoded", &body))).json();
    let keys = http(&base, "GET", &path_of(d["jwks_uri"].as_str().unwrap()), None).json();
    let c = verify(t["access_token"].as_str().unwrap(), &keys, &Expect { issuer: &issuer, tenant: &tenant, audience: &defaults.audience, client_id: Some(&defaults.client_id), now: now() }).unwrap();
    assert_eq!(c["preferred_username"], "writer@tether.test");
    assert!(c["roles"].as_array().unwrap().iter().any(|r| r == "Tether.Technician.Write"));
    // Logout redirects to a registered redirect URI only.
    let logout = path_of(d["end_session_endpoint"].as_str().unwrap());
    let lo = http(&base, "GET", &format!("{logout}?post_logout_redirect_uri={}", enc("http://localhost:9/")), None);
    assert_eq!((lo.status, lo.headers["location"].as_str()), (302, "http://localhost:9/"));
    for other in ["http://example.com/", "http://127.0.0.1:9/", "http://localhost:9/bye"] {
        let r = http(&base, "GET", &format!("{logout}?post_logout_redirect_uri={}", enc(other)), None);
        assert_eq!(r.status, 200, "{other}");
        assert!(!r.headers.contains_key("location"));
    }
    child.kill().unwrap();
    child.wait().unwrap();

    // The binary refuses a non-loopback listen address without --allow-network.
    let st = std::process::Command::new(env!("CARGO_BIN_EXE_oidc-mock")).args(["--listen", "0.0.0.0:0"]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().unwrap();
    assert!(!st.success());
}

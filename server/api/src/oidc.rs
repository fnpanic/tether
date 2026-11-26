//! Staff sign-in (design 12.2, 12.3): an access token of the OpenID Connect provider named in the
//! admin-signed team document is checked here, before the state lock, and turned into a session
//! bound to the member key that signed `POST /api/team/session` (team.rs).
//!
//! What the relay trusts about the provider comes only from the signed team document (`oidc`:
//! issuer, tenant, audience, client id); from the provider's discovery document it takes only the
//! `jwks_uri`, and only on the issuer's own scheme, host and port. The token is hostile input:
//! bounded sizes, strict base64url, RS256 only (pinned here, never taken from the token), the key
//! chosen by `kid` alone, then every claim against the document. A refusal is a [`Reason`]: a
//! code for the audit log and the desktop, and a message for an administrator.
//!
//! Keys: discovery and JWKS are fetched on a thread of their own (one at a time relay-wide,
//! others get `503` at once), with timeouts, size caps, no redirect, no proxy and certificate
//! checks (ureq with rustls and the webpki roots); kept 24 h, fetched again for an unknown `kid`
//! at most once a minute. A production build refuses a provider that is not `https://` or whose
//! address is loopback, private or link-local (checked when the name is resolved for the
//! connection); test builds ([`crate::TEST_HOOKS`]) also accept `http://` on loopback (the mock).

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, Read};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::{mpsc, Arc, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::de::IgnoredAny;
use serde::Deserialize;
use serde_json::Value;
use tether_trust::team::{Grants, Oidc, Permissions};

/// Clock skew allowed on `exp`, `nbf` and `iat` (as for request signatures).
pub const SKEW: u64 = 300;
/// The longest token lifetime accepted (`exp - nbf`): Entra's tokens with continuous access
/// evaluation live up to 28 h; ordinary ones 60 to 90 minutes.
pub const LIFETIME_MAX: u64 = 28 * 3600;
/// Bytes of a whole token, and of its three parts (base64url text).
pub const TOKEN_MAX: usize = 16 << 10;
const HEADER_MAX: usize = 2 << 10;
const PAYLOAD_MAX: usize = 12 << 10;
/// A 4096-bit signature is 683 characters.
const SIG_MAX: usize = 700;
const KID_MAX: usize = 128;
/// `oid` and `uti`: at most this many printable ASCII characters.
const ID_MAX: usize = 128;
/// Display name and user name kept: at most this many characters, control characters dropped.
const NAME_MAX: usize = 128;
/// Discovery document and JWKS: at most this many bytes, keys at most this many.
const DOC_MAX: u64 = 64 << 10;
const JWKS_KEYS_MAX: usize = 64;
const CACHE_TTL: Duration = Duration::from_secs(24 * 3600);
const REFETCH_MIN: Duration = Duration::from_secs(60);
/// Connect and whole-request timeout of each of the two fetches.
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a sign-in waits for a fetch (the fetch thread may outlive it, holding the one slot).
const FETCH_WAIT: Duration = Duration::from_secs(11);
/// `aud` of a Microsoft Graph token (no API scope requested): named in the refusal.
const GRAPH_AUDIENCES: [&str; 2] = ["00000003-0000-0000-c000-000000000000", "https://graph.microsoft.com"];

/// Why a sign-in was refused: `code()` for the audit log and the desktop, `status()`, `message()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reason {
    Malformed,
    Alg,
    NoKid,
    UnknownKid,
    BadSignature,
    V1Token,
    WrongVersion,
    WrongAudience,
    WrongIssuer,
    WrongTenant,
    WrongClient,
    Expired,
    NotYetValid,
    Lifetime,
    Overage,
    NoScope,
    NoOid,
    NoUti,
    Denied,
    NoRole,
    TokenUsed,
    NotConfigured,
    Unavailable,
    ProviderAddress,
    ConfigChanged,
    TooManySessions,
}

impl Reason {
    pub fn code(self) -> &'static str {
        use Reason::*;
        match self {
            Malformed => "malformed",
            Alg => "alg",
            NoKid => "no-kid",
            UnknownKid => "unknown-kid",
            BadSignature => "bad-signature",
            V1Token => "v1-token",
            WrongVersion => "wrong-version",
            WrongAudience => "wrong-audience",
            WrongIssuer => "wrong-issuer",
            WrongTenant => "wrong-tenant",
            WrongClient => "wrong-client",
            Expired => "expired",
            NotYetValid => "not-yet-valid",
            Lifetime => "lifetime",
            Overage => "overage",
            NoScope => "no-scope",
            NoOid => "no-oid",
            NoUti => "no-uti",
            Denied => "denied",
            NoRole => "no-role",
            TokenUsed => "token-used",
            NotConfigured => "not-configured",
            Unavailable => "provider-unavailable",
            ProviderAddress => "provider-address",
            ConfigChanged => "config-changed",
            TooManySessions => "too-many-sessions",
        }
    }

    pub fn status(self) -> u16 {
        use Reason::*;
        match self {
            Denied | NoRole => 403,
            TokenUsed => 409,
            NotConfigured => 404,
            Unavailable | ProviderAddress | ConfigChanged | TooManySessions => 503,
            _ => 401,
        }
    }

    /// What helps an administrator; it names no expected value (issuer, tenant, audience).
    pub fn message(self) -> &'static str {
        use Reason::*;
        match self {
            Malformed => "not a well-formed access token",
            Alg => "the token is not signed with RS256",
            NoKid => "the token names no signing key (kid)",
            UnknownKid => "the token's signing key is not among the identity provider's published keys",
            BadSignature => "the token's signature does not verify",
            V1Token => "a v1.0 access token: the API's app registration must issue v2.0 tokens (manifest: requestedAccessTokenVersion = 2)",
            WrongVersion => "not a v2.0 access token",
            WrongAudience => "the token is for another audience: the sign-in must request the API scope named in the team document (a token for Microsoft Graph is not accepted)",
            WrongIssuer => "the token is from another issuer than the team document names",
            WrongTenant => "the token is from another tenant than the team document names",
            WrongClient => "the token was issued to another client than the team document's client_id",
            Expired => "the token has expired: sign in again",
            NotYetValid => "the token is not valid yet (check the clocks)",
            Lifetime => "the token's lifetime is longer than this relay accepts",
            Overage => "the token carries a groups overage: assign groups to the API's app roles instead of using group claims",
            NoScope => "not a delegated user token (no scp claim)",
            NoOid => "the token has no object id (oid)",
            NoUti => "the token has no token id (uti)",
            Denied => "this account may not sign in to this team",
            NoRole => "none of the account's app roles is mapped in the team document",
            TokenUsed => "this token was already used by another key: sign in again on this device",
            NotConfigured => "no sign-in configured",
            Unavailable => "the identity provider's keys cannot be fetched now: try again later",
            ProviderAddress => "the identity provider's address is not allowed on this relay (https and a public address only)",
            ConfigChanged => "the sign-in configuration changed while the token was checked: try again",
            TooManySessions => "too many sign-in sessions on this relay: try again later",
        }
    }
}

/// The claims of a checked token that a session keeps.
#[derive(Clone, Debug)]
pub(crate) struct Claims {
    pub oid: String,
    pub uti: String,
    /// Display name (`name`), else the user name, else the oid.
    pub name: String,
    /// `preferred_username` (may be empty).
    pub upn: String,
    pub roles: Vec<String>,
    pub exp: u64,
}

/// An RSA public key of the provider (big-endian modulus and exponent, checked when read).
#[derive(Debug, PartialEq)]
pub(crate) struct Jwk {
    n: Vec<u8>,
    e: Vec<u8>,
}

#[derive(Deserialize)]
struct Header {
    alg: String,
    kid: Option<String>,
    /// Extensions the token says must be understood: none is.
    crit: Option<IgnoredAny>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Aud {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize)]
struct Payload {
    iss: Option<String>,
    tid: Option<String>,
    aud: Option<Aud>,
    ver: Option<String>,
    azp: Option<String>,
    exp: Option<u64>,
    nbf: Option<u64>,
    iat: Option<u64>,
    oid: Option<String>,
    uti: Option<String>,
    scp: Option<String>,
    name: Option<String>,
    preferred_username: Option<String>,
    roles: Option<Vec<String>>,
    #[serde(rename = "_claim_names")]
    claim_names: Option<IgnoredAny>,
    #[serde(rename = "_claim_sources")]
    claim_sources: Option<IgnoredAny>,
    hasgroups: Option<IgnoredAny>,
}

/// `aud` is exactly the audience: the string, or an array holding only it.
fn aud_is(aud: &Option<Aud>, audience: &str) -> bool {
    match aud {
        Some(Aud::One(a)) => a == audience,
        Some(Aud::Many(v)) => v.len() == 1 && v[0] == audience,
        None => false,
    }
}

fn id_ok(s: &str) -> bool {
    (1..=ID_MAX).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_graphic())
}

fn clean(s: Option<&str>) -> String {
    s.unwrap_or_default().chars().filter(|c| !c.is_control()).take(NAME_MAX).collect()
}

/// Checks `token` against the document's `cfg` at unix time `t`; `key` finds the provider's key of
/// a `kid` (it may fetch). Work before the signature check is bounded by the size limits.
pub(crate) fn check(token: &str, cfg: &Oidc, t: u64, key: impl FnOnce(&str) -> Result<Arc<Jwk>, Reason>) -> Result<Claims, Reason> {
    use Reason::*;
    if token.len() > TOKEN_MAX {
        return Err(Malformed);
    }
    let mut it = token.split('.');
    let (Some(h), Some(p), Some(s), None) = (it.next(), it.next(), it.next(), it.next()) else { return Err(Malformed) };
    if h.is_empty() || p.is_empty() || s.is_empty() || h.len() > HEADER_MAX || p.len() > PAYLOAD_MAX || s.len() > SIG_MAX {
        return Err(Malformed);
    }
    // Strict: the URL-safe alphabet, no padding, no stray bits.
    let dec = |x: &str| URL_SAFE_NO_PAD.decode(x).map_err(|_| Malformed);
    let header: Header = serde_json::from_slice(&dec(h)?).map_err(|_| Malformed)?;
    if header.crit.is_some() {
        return Err(Malformed);
    }
    if header.alg != "RS256" {
        return Err(Alg);
    }
    // A token without kid matches no key (never "try every key").
    let kid = header.kid.filter(|k| !k.is_empty() && k.len() <= KID_MAX).ok_or(NoKid)?;
    let sig = dec(s)?;
    let c: Payload = serde_json::from_slice(&dec(p)?).map_err(|_| Malformed)?;
    // Unverified, only to refuse early with the reason an administrator needs (and without
    // fetching keys for a token that is not for this API): another audience or a v1.0 token.
    let v1 = c.ver.as_deref() == Some("1.0");
    if v1 || !aud_is(&c.aud, &cfg.audience) {
        let graph = match &c.aud {
            Some(Aud::One(a)) => GRAPH_AUDIENCES.contains(&a.as_str()),
            Some(Aud::Many(v)) => v.iter().any(|a| GRAPH_AUDIENCES.contains(&a.as_str())),
            None => false,
        };
        return Err(if v1 && !graph { V1Token } else { WrongAudience });
    }
    let k = key(&kid)?;
    let signed = &token.as_bytes()[..h.len() + 1 + p.len()];
    ring::signature::RsaPublicKeyComponents { n: &k.n[..], e: &k.e[..] }.verify(&ring::signature::RSA_PKCS1_2048_8192_SHA256, signed, &sig).map_err(|_| BadSignature)?;
    // Verified from here on.
    if c.ver.as_deref() != Some("2.0") {
        return Err(WrongVersion);
    }
    if c.iss.as_deref() != Some(cfg.issuer.as_str()) {
        return Err(WrongIssuer);
    }
    if c.tid.as_deref() != Some(cfg.tenant.as_str()) {
        return Err(WrongTenant);
    }
    if c.azp.as_deref() != Some(cfg.client_id.as_str()) {
        return Err(WrongClient);
    }
    let (Some(exp), Some(nbf), Some(iat)) = (c.exp, c.nbf, c.iat) else { return Err(Malformed) };
    if t > exp.saturating_add(SKEW) {
        return Err(Expired);
    }
    if nbf > t + SKEW || iat > t + SKEW {
        return Err(NotYetValid);
    }
    if exp <= nbf || exp - nbf > LIFETIME_MAX || iat > exp {
        return Err(Lifetime);
    }
    // Roles only: any sign of a groups overage means the token's picture of the person is incomplete.
    if c.claim_names.is_some() || c.claim_sources.is_some() || c.hasgroups.is_some() {
        return Err(Overage);
    }
    if c.scp.as_deref().is_none_or(|s| s.trim().is_empty()) {
        return Err(NoScope);
    }
    let oid = c.oid.filter(|o| id_ok(o)).ok_or(NoOid)?;
    let uti = c.uti.filter(|u| id_ok(u)).ok_or(NoUti)?;
    let upn = clean(c.preferred_username.as_deref());
    let mut name = clean(c.name.as_deref());
    if name.trim().is_empty() {
        name = if upn.is_empty() { oid.clone() } else { upn.clone() };
    }
    let mut roles = c.roles.unwrap_or_default();
    roles.sort_unstable();
    roles.dedup();
    Ok(Claims { oid, uti, name, upn, roles, exp })
}

/// What the token's `roles` grant through the document's `roles` table: the union of the mapped
/// roles; customers the union of theirs, `*` when none names any (design 12.4). `None`: no role is
/// in the table (a role not in it gives nothing).
pub(crate) fn permissions(roles: &[String], table: &BTreeMap<String, Grants>) -> Option<Permissions> {
    let mut p = Permissions { write: false, audit: false, sbom: false, agent: false, customers: Vec::new() };
    let mut any = false;
    for g in roles.iter().filter_map(|r| table.get(r)) {
        any = true;
        p.write |= g.write;
        p.audit |= g.audit;
        p.sbom |= g.sbom;
        p.agent |= g.agent;
        p.customers.extend(g.customers.iter().cloned());
    }
    if p.customers.is_empty() {
        p.customers.push("*".into());
    }
    p.customers.sort_unstable();
    p.customers.dedup();
    any.then_some(p)
}

/// Whether `ip` may be the identity provider's address in a production build.
pub(crate) fn public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => {
            let o = a.octets();
            !(a.is_loopback() || a.is_private() || a.is_link_local() || a.is_unspecified() || a.is_broadcast() || a.is_multicast() || a.is_documentation() || o[0] == 0 || (o[0] == 100 && o[1] & 0xc0 == 64) || (o[0] == 198 && o[1] & 0xfe == 18) || o[0] >= 240)
        }
        IpAddr::V6(a) => {
            if let Some(v4) = a.to_ipv4_mapped() {
                return public(IpAddr::V4(v4));
            }
            let s = a.segments();
            !(a.is_loopback() || a.is_unspecified() || a.is_multicast() || s[0] & 0xfe00 == 0xfc00 || s[0] & 0xffc0 == 0xfe80 || s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] || s[0] == 0x2002 || s[..6] == [0; 6] || (s[0], s[1]) == (0x2001, 0xdb8))
        }
    }
}

/// The issuer as a URL, if this build may trust it: written as the provider writes it (normal
/// form), no user name, query or fragment; `https://` and not `localhost` or a loopback, private or
/// link-local address; with `local` (test builds) also `http://` on a loopback host.
pub(crate) fn issuer_url(issuer: &str, local: bool) -> Result<url::Url, String> {
    let u = url::Url::parse(issuer).map_err(|e| format!("oidc.issuer: {e}"))?;
    if !u.username().is_empty() || u.password().is_some() || u.query().is_some() || u.fragment().is_some() {
        return Err("oidc.issuer: no user name, query or fragment".into());
    }
    if u.as_str() != issuer {
        return Err(format!("oidc.issuer: write it in normal form, as the provider does: {u}"));
    }
    let host = u.host().ok_or("oidc.issuer: no host")?;
    let loopback = match &host {
        url::Host::Domain(d) => *d == "localhost" || d.ends_with(".localhost"),
        url::Host::Ipv4(a) => a.is_loopback(),
        url::Host::Ipv6(a) => a.is_loopback(),
    };
    match u.scheme() {
        "https" => {}
        "http" if local && loopback => {}
        _ => return Err("oidc.issuer: must be an https:// URL".into()),
    }
    if !local {
        let private = match host {
            url::Host::Domain(_) => loopback,
            url::Host::Ipv4(a) => !public(IpAddr::V4(a)),
            url::Host::Ipv6(a) => !public(IpAddr::V6(a)),
        };
        if private {
            return Err("oidc.issuer: a loopback, private or link-local address is not an identity provider".into());
        }
    }
    Ok(u)
}

/// Whether this build may trust `issuer` (checked when a team document is published).
pub(crate) fn issuer_ok(issuer: &str) -> Result<(), String> {
    issuer_url(issuer, crate::TEST_HOOKS).map(drop)
}

/// The `jwks_uri` of the discovery document, if it is on the issuer's own scheme, host and port.
fn jwks_url(issuer: &url::Url, raw: &str) -> Result<url::Url, String> {
    let u = url::Url::parse(raw).map_err(|e| format!("jwks_uri: {e}"))?;
    if u.scheme() != issuer.scheme() || u.host_str() != issuer.host_str() || u.port_or_known_default() != issuer.port_or_known_default() || !u.username().is_empty() || u.password().is_some() || u.fragment().is_some() {
        return Err("discovery names a jwks_uri that is not on the issuer's own host".into());
    }
    Ok(u)
}

thread_local! {
    /// The resolver refused an address (production build: loopback or private).
    static REFUSED: Cell<bool> = const { Cell::new(false) };
}

/// Resolves `netloc` for the connection itself (no second lookup an attacker could answer
/// differently); without `local`, an address that is not public refuses the whole name.
fn resolve(netloc: &str, local: bool) -> io::Result<Vec<SocketAddr>> {
    let addrs: Vec<SocketAddr> = netloc.to_socket_addrs()?.collect();
    if !local && addrs.iter().any(|a| !public(a.ip())) {
        REFUSED.with(|r| r.set(true));
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "the identity provider resolves to a loopback or private address"));
    }
    Ok(addrs)
}

fn get_json(agent: &ureq::Agent, url: &str) -> Result<Value, (Reason, String)> {
    let r = agent.get(url).call().map_err(|e| (if REFUSED.with(Cell::get) { Reason::ProviderAddress } else { Reason::Unavailable }, format!("{url}: {e}")))?;
    if r.status() != 200 {
        return Err((Reason::Unavailable, format!("{url}: answered {}", r.status())));
    }
    let mut buf = Vec::new();
    r.into_reader().take(DOC_MAX + 1).read_to_end(&mut buf).map_err(|e| (Reason::Unavailable, format!("{url}: {e}")))?;
    if buf.len() as u64 > DOC_MAX {
        return Err((Reason::Unavailable, format!("{url}: more than {} KiB", DOC_MAX >> 10)));
    }
    serde_json::from_slice(&buf).map_err(|e| (Reason::Unavailable, format!("{url}: {e}")))
}

/// The usable keys of a JWKS: `kty` RSA, `use` sig and `alg` RS256 when present, a modulus of 2048
/// to 4096 bits; a `kid` named twice is dropped (ambiguous).
fn parse_jwks(v: &Value) -> Result<HashMap<String, Arc<Jwk>>, String> {
    let keys = v.get("keys").and_then(Value::as_array).ok_or("the JWKS has no keys array")?;
    if keys.len() > JWKS_KEYS_MAX {
        return Err(format!("the JWKS has more than {JWKS_KEYS_MAX} keys"));
    }
    let b64 = |s: &str| URL_SAFE_NO_PAD.decode(s).ok();
    let (mut out, mut twice) = (HashMap::new(), HashSet::new());
    for k in keys {
        let s = |f: &str| k.get(f).and_then(Value::as_str);
        let Some(kid) = s("kid").filter(|x| !x.is_empty() && x.len() <= KID_MAX) else { continue };
        if s("kty") != Some("RSA") || k.get("use").is_some_and(|u| u.as_str() != Some("sig")) || k.get("alg").is_some_and(|a| a.as_str() != Some("RS256")) {
            continue;
        }
        let (Some(n), Some(e)) = (s("n").and_then(b64), s("e").and_then(b64)) else { continue };
        let bits = n.first().map_or(0, |b| n.len() * 8 - b.leading_zeros() as usize);
        if n.first() == Some(&0) || !(2048..=4096).contains(&bits) || !(1..=4).contains(&e.len()) || e[0] == 0 {
            continue;
        }
        if out.insert(kid.to_owned(), Arc::new(Jwk { n, e })).is_some() {
            twice.insert(kid.to_owned());
        }
    }
    out.retain(|k, _| !twice.contains(k));
    if out.is_empty() {
        return Err("the JWKS has no usable RS256 signing key".into());
    }
    Ok(out)
}

/// Discovery, then the JWKS it names (on the issuer's host).
fn fetch(issuer: &str) -> Result<HashMap<String, Arc<Jwk>>, (Reason, String)> {
    let local = crate::TEST_HOOKS;
    let base = issuer_url(issuer, local).map_err(|e| (Reason::ProviderAddress, e))?;
    REFUSED.with(|r| r.set(false));
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(FETCH_TIMEOUT)
        .timeout(FETCH_TIMEOUT)
        .redirects(0)
        .try_proxy_from_env(false)
        .max_idle_connections(0)
        .user_agent("tether-api")
        .resolver(move |netloc: &str| resolve(netloc, local))
        .build();
    let disc = get_json(&agent, &format!("{}/.well-known/openid-configuration", issuer.trim_end_matches('/')))?;
    if disc.get("issuer").and_then(Value::as_str) != Some(issuer) {
        return Err((Reason::Unavailable, "the discovery document names another issuer".into()));
    }
    let uri = disc.get("jwks_uri").and_then(Value::as_str).ok_or((Reason::Unavailable, "the discovery document has no jwks_uri".to_owned()))?;
    let jwks = jwks_url(&base, uri).map_err(|e| (Reason::Unavailable, e))?;
    parse_jwks(&get_json(&agent, jwks.as_str())?).map_err(|e| (Reason::Unavailable, e))
}

/// The provider's keys as last fetched, for one issuer.
#[derive(Default)]
pub(crate) struct Cache {
    issuer: String,
    keys: HashMap<String, Arc<Jwk>>,
    fetched: Option<Instant>,
    tried: Option<Instant>,
    failed: Option<Reason>,
    /// A fetch is running (its thread clears this when it really ends).
    running: bool,
}

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(Default::default);

fn lock(c: &Mutex<Cache>) -> MutexGuard<'_, Cache> {
    c.lock().unwrap_or_else(|e| e.into_inner())
}

/// The key `kid` of `issuer` at `now`: from the cache while it is under 24 h old; otherwise, or for
/// an unknown `kid`, one fetch by `fetch` on its own thread if none ran in the last minute and none
/// is running (else at once: `unknown-kid` with fresh keys, the last failure or `503` without).
pub(crate) fn key_for(cache: &'static Mutex<Cache>, issuer: &str, kid: &str, now: Instant, fetch: impl FnOnce() -> Result<HashMap<String, Arc<Jwk>>, (Reason, String)> + Send + 'static) -> Result<Arc<Jwk>, Reason> {
    let fresh = |c: &Cache| c.fetched.is_some_and(|f| now.saturating_duration_since(f) < CACHE_TTL);
    let rx = {
        let mut c = lock(cache);
        if c.issuer != issuer {
            // Another document: nothing of the old issuer is used; a fetch still running keeps the slot.
            let running = c.running;
            *c = Cache { issuer: issuer.to_owned(), running, ..Default::default() };
        }
        if fresh(&c) {
            if let Some(k) = c.keys.get(kid) {
                return Ok(k.clone());
            }
        }
        if c.running {
            return Err(Reason::Unavailable);
        }
        if c.tried.is_some_and(|t| now.saturating_duration_since(t) < REFETCH_MIN) {
            return Err(if fresh(&c) { Reason::UnknownKid } else { c.failed.unwrap_or(Reason::Unavailable) });
        }
        (c.running, c.tried) = (true, Some(now));
        let (tx, rx) = mpsc::channel();
        let issuer = issuer.to_owned();
        std::thread::spawn(move || {
            let r = fetch();
            let mut c = lock(cache);
            c.running = false;
            if c.issuer == issuer {
                match r {
                    Ok(keys) => {
                        eprintln!("oidc: {} signing keys of {issuer} fetched", keys.len());
                        (c.keys, c.fetched, c.failed) = (keys, Some(now), None);
                    }
                    Err((reason, why)) => {
                        eprintln!("oidc: the keys of {issuer} could not be fetched: {why}");
                        c.failed = Some(reason);
                    }
                }
            }
            drop(c);
            let _ = tx.send(());
        });
        rx
    };
    let _ = rx.recv_timeout(FETCH_WAIT);
    let c = lock(cache);
    match c.keys.get(kid) {
        Some(k) if fresh(&c) => Ok(k.clone()),
        None if fresh(&c) => Err(Reason::UnknownKid),
        _ => Err(c.failed.unwrap_or(Reason::Unavailable)),
    }
}

/// [`check`] with the provider's keys ([`key_for`] on the relay's cache).
pub(crate) fn verify(token: &str, cfg: &Oidc, t: u64) -> Result<Claims, Reason> {
    check(token, cfg, t, |kid| {
        let issuer = cfg.issuer.clone();
        key_for(&CACHE, &cfg.issuer, kid, Instant::now(), move || fetch(&issuer))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::pkcs1v15::SigningKey;
    use rsa::signature::{SignatureEncoding, Signer};
    use rsa::traits::PublicKeyParts;
    use rsa::RsaPrivateKey;
    use serde_json::json;
    use sha2::Sha256;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const T: u64 = 1_800_000_000;

    fn cfg() -> Oidc {
        Oidc { issuer: "https://login.example.com/t1/v2.0".into(), tenant: "t1".into(), audience: "api-id".into(), client_id: "client-id".into(), scope: "api://x/access".into() }
    }

    fn claims() -> Value {
        json!({"iss": "https://login.example.com/t1/v2.0", "tid": "t1", "aud": "api-id", "azp": "client-id", "ver": "2.0", "iat": T, "nbf": T, "exp": T + 3600, "oid": "oid-1", "uti": "uti-1", "scp": "access", "name": "Anna\u{7}", "preferred_username": "anna@example.com", "roles": ["B", "A", "B"]})
    }

    fn key(bits: usize) -> RsaPrivateKey {
        use rand_core::SeedableRng;
        RsaPrivateKey::new(&mut rand_chacha::ChaCha20Rng::seed_from_u64(bits as u64), bits).unwrap()
    }

    fn jwk(k: &RsaPrivateKey) -> Arc<Jwk> {
        Arc::new(Jwk { n: k.n().to_bytes_be(), e: k.e().to_bytes_be() })
    }

    fn b64(v: &[u8]) -> String {
        URL_SAFE_NO_PAD.encode(v)
    }

    fn sign(k: &RsaPrivateKey, header: &Value, c: &Value) -> String {
        let input = format!("{}.{}", b64(header.to_string().as_bytes()), b64(c.to_string().as_bytes()));
        let sig = SigningKey::<Sha256>::new(k.clone()).sign(input.as_bytes()).to_vec();
        format!("{input}.{}", b64(&sig))
    }

    fn hdr() -> Value {
        json!({"typ": "JWT", "alg": "RS256", "kid": "k1"})
    }

    #[test]
    fn a_good_token_gives_its_claims() {
        let k = key(2048);
        let tok = sign(&k, &hdr(), &claims());
        let mut asked = None;
        let c = check(&tok, &cfg(), T + 10, |kid| {
            asked = Some(kid.to_owned());
            Ok(jwk(&k))
        })
        .unwrap();
        assert_eq!(asked.as_deref(), Some("k1"));
        assert_eq!((c.oid.as_str(), c.uti.as_str(), c.name.as_str(), c.upn.as_str(), c.roles.clone(), c.exp), ("oid-1", "uti-1", "Anna", "anna@example.com", vec!["A".to_owned(), "B".to_owned()], T + 3600));
        // aud as an array holding only the audience.
        let mut c2 = claims();
        c2["aud"] = json!(["api-id"]);
        assert!(check(&sign(&k, &hdr(), &c2), &cfg(), T, |_| Ok(jwk(&k))).is_ok());
    }

    /// Each refusal for its own reason; the key is looked up only for a token that is for this
    /// audience, and never for one without kid or with another algorithm.
    #[test]
    fn every_refusal_has_its_reason() {
        let k = key(2048);
        let other = key(3072);
        let c = |f: &dyn Fn(&mut Value)| {
            let mut v = claims();
            f(&mut v);
            v
        };
        let tok = |v: Value| sign(&k, &hdr(), &v);
        let run = |t: &str| check(t, &cfg(), T + 10, |_| Ok(jwk(&k))).err();
        use Reason::*;
        let cases: Vec<(String, Reason)> = vec![
            (tok(c(&|v| v["ver"] = json!("1.0"))), V1Token),
            (tok(c(&|v| v["ver"] = json!("3.0"))), WrongVersion),
            (tok(c(&|v| drop(v.as_object_mut().unwrap().remove("ver")))), WrongVersion),
            (tok(c(&|v| v["iss"] = json!("https://login.example.com/t2/v2.0"))), WrongIssuer),
            (tok(c(&|v| v["tid"] = json!("t2"))), WrongTenant),
            (tok(c(&|v| v["azp"] = json!("other"))), WrongClient),
            (tok(c(&|v| drop(v.as_object_mut().unwrap().remove("azp")))), WrongClient),
            (tok(c(&|v| (v["iat"], v["nbf"], v["exp"]) = (json!(T - 8000), json!(T - 8000), json!(T - 400)))), Expired),
            (tok(c(&|v| (v["nbf"], v["exp"]) = (json!(T + 400), json!(T + 4000)))), NotYetValid),
            (tok(c(&|v| v["iat"] = json!(T + 400))), NotYetValid),
            (tok(c(&|v| v["exp"] = json!(T + LIFETIME_MAX + 1))), Lifetime),
            (tok(c(&|v| v["_claim_names"] = json!({"groups": "src1"}))), Overage),
            (tok(c(&|v| v["_claim_sources"] = json!({}))), Overage),
            (tok(c(&|v| v["hasgroups"] = json!(true))), Overage),
            (tok(c(&|v| v["scp"] = json!(" "))), NoScope),
            (tok(c(&|v| drop(v.as_object_mut().unwrap().remove("scp")))), NoScope),
            (tok(c(&|v| drop(v.as_object_mut().unwrap().remove("oid")))), NoOid),
            (tok(c(&|v| v["oid"] = json!("a b"))), NoOid),
            (tok(c(&|v| v["oid"] = json!("x".repeat(129)))), NoOid),
            (tok(c(&|v| drop(v.as_object_mut().unwrap().remove("uti")))), NoUti),
            (tok(c(&|v| drop(v.as_object_mut().unwrap().remove("exp")))), Malformed),
            (tok(c(&|v| v["exp"] = json!(1.5e9))), Malformed),
            (tok(c(&|v| v["exp"] = json!(-1))), Malformed),
            (tok(c(&|v| v["roles"] = json!("A"))), Malformed),
            (sign(&other, &hdr(), &claims()), BadSignature),
        ];
        for (i, (t, want)) in cases.iter().enumerate() {
            assert_eq!(run(t), Some(*want), "case {i}");
        }
        // Not for this API: refused before any key is looked up.
        let no_key = |t: &str| check(t, &cfg(), T + 10, |_| panic!("no key lookup")).err();
        assert_eq!(no_key(&tok(c(&|v| v["aud"] = json!("other-api")))), Some(WrongAudience));
        assert_eq!(no_key(&tok(c(&|v| v["aud"] = json!("api://x")))), Some(WrongAudience), "the App ID URI is not accepted");
        assert_eq!(no_key(&tok(c(&|v| v["aud"] = json!(["api-id", "other"])))), Some(WrongAudience));
        assert_eq!(no_key(&tok(c(&|v| v["aud"] = json!([])))), Some(WrongAudience));
        assert_eq!(no_key(&tok(c(&|v| drop(v.as_object_mut().unwrap().remove("aud"))))), Some(WrongAudience));
        assert_eq!(no_key(&tok(c(&|v| (v["ver"], v["aud"]) = (json!("1.0"), json!("api://x"))))), Some(V1Token));
        assert_eq!(no_key(&tok(c(&|v| (v["ver"], v["aud"]) = (json!("1.0"), json!(GRAPH_AUDIENCES[0]))))), Some(WrongAudience), "Graph: the API scope was not requested");
        assert_eq!(no_key(&tok(c(&|v| v["ver"] = json!("1.0")))), Some(V1Token));
        // Headers: alg pinned, kid required, crit refused; nothing looked up.
        let body = b64(claims().to_string().as_bytes());
        let good = tok(claims());
        let sig = good.rsplit('.').next().unwrap();
        let with = |h: Value| format!("{}.{body}.{sig}", b64(h.to_string().as_bytes()));
        assert_eq!(no_key(&with(json!({"alg": "none", "kid": "k1"}))), Some(Alg));
        assert_eq!(no_key(&with(json!({"alg": "HS256", "kid": "k1"}))), Some(Alg));
        assert_eq!(no_key(&with(json!({"alg": "rs256", "kid": "k1"}))), Some(Alg));
        assert_eq!(no_key(&with(json!({"alg": "RS256"}))), Some(NoKid));
        assert_eq!(no_key(&with(json!({"alg": "RS256", "kid": ""}))), Some(NoKid));
        assert_eq!(no_key(&with(json!({"alg": "RS256", "kid": "k".repeat(129)}))), Some(NoKid));
        assert_eq!(no_key(&with(json!({"alg": "RS256", "kid": "k1", "crit": ["b64"]}))), Some(Malformed));
        assert_eq!(no_key(&format!("{}.{body}.{sig}", b64(br#"{"alg":"RS256","alg":"none","kid":"k1"}"#))), Some(Malformed), "a name twice");
        // `alg: none` with an empty signature, and HS256 keyed with the public key: never verified as such.
        let n = k.n().to_bytes_be();
        let hs = {
            use ring::hmac;
            let input = format!("{}.{body}", b64(json!({"alg": "HS256", "kid": "k1"}).to_string().as_bytes()));
            let tag = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, &n), input.as_bytes());
            format!("{input}.{}", b64(tag.as_ref()))
        };
        assert_eq!(no_key(&hs), Some(Alg));
        assert_eq!(no_key(&format!("{}.{body}.", b64(br#"{"alg":"none","kid":"k1"}"#))), Some(Malformed));
        // Shapes and sizes: two or four parts, padding, the standard alphabet, too long.
        for bad in [format!("{}.{body}", b64(hdr().to_string().as_bytes())), format!("{good}.x"), format!("{good}="), good.replace('-', "+").replace('_', "/") + "+/", "a".repeat(TOKEN_MAX + 1), format!("{}.{}.{sig}", b64(hdr().to_string().as_bytes()), "a".repeat(PAYLOAD_MAX + 1)), String::new(), "..".into()] {
            assert_eq!(no_key(&bad), Some(Malformed), "{}", &bad[..bad.len().min(40)]);
        }
        // A signature over other bytes (one payload byte changed) or cut.
        let mut p: Vec<&str> = good.split('.').collect();
        let changed = b64(c(&|v| v["roles"] = json!(["Admin"])).to_string().as_bytes());
        p[1] = &changed;
        assert_eq!(run(&p.join(".")), Some(BadSignature));
        let short = URL_SAFE_NO_PAD.decode(sig).unwrap();
        let cut = format!("{}.{}.{}", p[0], body, b64(&short[..short.len() - 1]));
        assert_eq!(run(&cut), Some(BadSignature));
        // The key lookup's own answer is passed on.
        assert_eq!(check(&good, &cfg(), T, |_| Err(UnknownKid)).err(), Some(UnknownKid));
        assert_eq!(check(&good, &cfg(), T, |_| Err(Unavailable)).err(), Some(Unavailable));
        // Skew: 300 s either side.
        assert!(check(&good, &cfg(), T + 3600 + SKEW, |_| Ok(jwk(&k))).is_ok());
        assert_eq!(check(&good, &cfg(), T + 3600 + SKEW + 1, |_| Ok(jwk(&k))).err(), Some(Expired));
        assert!(check(&good, &cfg(), T - SKEW, |_| Ok(jwk(&k))).is_ok());
        assert_eq!(check(&good, &cfg(), T - SKEW - 1, |_| Ok(jwk(&k))).err(), Some(NotYetValid));
    }

    #[test]
    fn jwks_keys_are_checked() {
        let (k, small) = (key(2048), key(1024));
        let entry = |k: &RsaPrivateKey, extra: Value| {
            let mut e = json!({"kty": "RSA", "use": "sig", "kid": "k1", "n": b64(&k.n().to_bytes_be()), "e": b64(&k.e().to_bytes_be())});
            for (a, b) in extra.as_object().unwrap() {
                e[a] = b.clone();
            }
            e
        };
        let keys = |v: Vec<Value>| parse_jwks(&json!({"keys": v})).map(|m| m.keys().cloned().collect::<Vec<_>>());
        assert_eq!(keys(vec![entry(&k, json!({}))]), Ok(vec!["k1".into()]));
        assert!(keys(vec![entry(&k, json!({"alg": "RS256", "x5c": ["..."]}))]).is_ok());
        let mut no_use = entry(&k, json!({}));
        no_use.as_object_mut().unwrap().remove("use");
        assert!(keys(vec![no_use]).is_ok(), "use is optional");
        for bad in [json!({"use": "enc"}), json!({"kty": "EC"}), json!({"alg": "RS512"}), json!({"n": 42}), json!({"e": "***"}), json!({"n": "AA" }), json!({"kid": ""}), json!({"e": "AAAB"})] {
            assert!(keys(vec![entry(&k, bad.clone())]).is_err(), "{bad}");
        }
        assert!(keys(vec![entry(&small, json!({}))]).is_err(), "1024 bits");
        // Padding or the standard alphabet in n: not strict base64url.
        let n = b64(&k.n().to_bytes_be());
        assert!(keys(vec![entry(&k, json!({"n": format!("{n}==")}))]).is_err());
        // A kid twice: neither is used; another key beside them still is.
        let other = key(3072);
        assert_eq!(keys(vec![entry(&k, json!({})), entry(&other, json!({})), entry(&other, json!({"kid": "k2"}))]), Ok(vec!["k2".into()]));
        assert!(parse_jwks(&json!({"keys": 1})).is_err());
        assert!(parse_jwks(&json!({"keys": vec![entry(&k, json!({})); JWKS_KEYS_MAX + 1]})).is_err());
    }

    #[test]
    fn roles_map_through_the_signed_table_only() {
        let table: BTreeMap<String, Grants> = serde_json::from_value(json!({
            "Tether.Technician": {},
            "Tether.Software": {"sbom": true},
            "Tether.Technician.Write": {"write": true},
            "Tether.Customer.4711": {"customers": ["4711"]},
            "Tether.Customer.47": {"customers": ["47*", "4711"]},
        }))
        .unwrap();
        let p = |r: &[&str]| permissions(&r.iter().map(|s| s.to_string()).collect::<Vec<_>>(), &table);
        assert_eq!(p(&["Tether.Technician", "Tether.Software"]), Some(Permissions { write: false, audit: false, sbom: true, agent: false, customers: vec!["*".into()] }));
        assert_eq!(p(&["Tether.Technician", "Tether.Customer.4711", "Tether.Customer.47"]).unwrap().customers, vec!["47*".to_owned(), "4711".to_owned()]);
        // A role not in the table gives nothing, also beside one that is.
        assert_eq!(p(&["Tether.Admin", "Tether.Technician"]).unwrap(), p(&["Tether.Technician"]).unwrap());
        assert_eq!(p(&["Tether.Admin", "tether.software"]), None);
        assert_eq!(p(&[]), None);
    }

    #[test]
    fn issuers_a_production_build_refuses() {
        for ok in ["https://login.microsoftonline.com/10000000-0000-4000-8000-000000000001/v2.0", "https://8.8.8.8/t/v2.0", "https://[2001:4860::1]/t/v2.0"] {
            assert!(issuer_url(ok, false).is_ok(), "{ok}");
        }
        for bad in ["http://login.microsoftonline.com/t/v2.0", "http://127.0.0.1:8080/t/v2.0", "https://127.0.0.1/t/v2.0", "https://localhost/t/v2.0", "https://idp.localhost/t/v2.0", "https://10.1.2.3/t/v2.0", "https://192.168.1.1/t", "https://169.254.169.254/t", "https://[::1]/t", "https://[fd00::1]/t", "https://[::ffff:127.0.0.1]/t", "https://100.64.0.1/t", "https://u:p@login.example.com/t", "https://login.example.com/t?x=1", "https://LOGIN.example.com/t", "ftp://login.example.com/t", "not a url"] {
            assert!(issuer_url(bad, false).is_err(), "{bad}");
        }
        // Test builds: http on loopback (the mock), nothing else besides https.
        assert!(issuer_url("http://127.0.0.1:8080/t/v2.0", true).is_ok());
        assert!(issuer_url("http://localhost:8080/t/v2.0", true).is_ok());
        assert!(issuer_url("http://login.example.com/t/v2.0", true).is_err());
        assert!(issuer_url("http://10.0.0.1/t/v2.0", true).is_err());
        // The resolver: a name resolving to loopback is refused in production.
        assert!(resolve("localhost:443", false).is_err());
        assert!(resolve("127.0.0.1:443", true).is_ok());
    }

    /// Ranges that reach a private or special network (NAT64, 6to4, IPv4-compatible, documentation,
    /// benchmarking) are not a provider's address; their neighbours are.
    #[test]
    fn special_address_ranges_are_not_public() {
        for bad in ["64:ff9b::808:808", "64:ff9b::a00:1", "2002:808:808::1", "2002::", "::a00:1", "::808:808", "2001:db8::1", "2001:db8:ffff::1", "198.18.0.1", "198.19.255.255"] {
            assert!(!public(bad.parse().unwrap()), "{bad}");
        }
        for ok in ["64:ff9a::1", "64:ff9b:1::1", "2003::1", "::1:0:0:1", "2001:db9::1", "2001:4860::1", "198.17.255.255", "198.20.0.0", "8.8.8.8"] {
            assert!(public(ok.parse().unwrap()), "{ok}");
        }
    }

    #[test]
    fn jwks_uri_must_be_on_the_issuers_host() {
        let iss = url::Url::parse("https://login.example.com/t/v2.0").unwrap();
        assert!(jwks_url(&iss, "https://login.example.com/t/discovery/v2.0/keys").is_ok());
        assert!(jwks_url(&iss, "https://login.example.com:443/keys").is_ok());
        for bad in ["https://evil.example.com/keys", "http://login.example.com/keys", "https://login.example.com:8443/keys", "https://u@login.example.com/keys", "https://login.example.com.evil.net/keys", "/keys"] {
            assert!(jwks_url(&iss, bad).is_err(), "{bad}");
        }
    }

    /// One fetch at a time, at most one a minute (also for unknown kids), 24 h of trust in the
    /// keys, the issuer of a new document starting from nothing.
    #[test]
    fn key_fetches_are_bounded() {
        static C: LazyLock<Mutex<Cache>> = LazyLock::new(Default::default);
        static N: AtomicUsize = AtomicUsize::new(0);
        let k = key(2048);
        let keys = Arc::new(HashMap::from([("k1".to_owned(), jwk(&k))]));
        let good = {
            let keys = keys.clone();
            move || {
                let keys = (*keys).clone();
                move || {
                    N.fetch_add(1, Ordering::SeqCst);
                    Ok(keys.iter().map(|(a, b)| (a.clone(), b.clone())).collect())
                }
            }
        };
        let t0 = Instant::now();
        assert!(key_for(&C, "i1", "k1", t0, good()).is_ok());
        assert_eq!(N.load(Ordering::SeqCst), 1);
        // Random kids: one fetch at most a minute, the rest refused at once.
        for i in 0..50 {
            assert_eq!(key_for(&C, "i1", &format!("r{i}"), t0 + Duration::from_secs(1), good()).err(), Some(Reason::UnknownKid));
        }
        assert_eq!(N.load(Ordering::SeqCst), 1);
        assert_eq!(key_for(&C, "i1", "r", t0 + Duration::from_secs(61), good()).err(), Some(Reason::UnknownKid));
        assert_eq!(N.load(Ordering::SeqCst), 2);
        assert!(key_for(&C, "i1", "k1", t0 + Duration::from_secs(62), good()).is_ok(), "from the cache");
        assert_eq!(N.load(Ordering::SeqCst), 2);
        // After 24 h the keys are fetched again; a failure then is 503, not the old keys.
        let fail = || || Err((Reason::Unavailable, "down".to_owned()));
        let day = t0 + Duration::from_secs(61) + CACHE_TTL;
        assert_eq!(key_for(&C, "i1", "k1", day, fail()).err(), Some(Reason::Unavailable));
        assert_eq!(key_for(&C, "i1", "k1", day + Duration::from_secs(10), good()).err(), Some(Reason::Unavailable), "no new try within the minute");
        assert!(key_for(&C, "i1", "k1", day + Duration::from_secs(61), good()).is_ok());
        // A fetch running: everyone else at once 503, and no second fetch.
        let (release, wait) = mpsc::channel::<()>();
        let slow = move || {
            let _ = wait.recv();
            Err((Reason::Unavailable, "slow".to_owned()))
        };
        let t1 = day + Duration::from_secs(200);
        std::thread::scope(|s| {
            let h = s.spawn(|| key_for(&C, "i2", "k1", t1, slow));
            while !lock(&C).running {
                std::thread::sleep(Duration::from_millis(5));
            }
            let at = Instant::now();
            assert_eq!(key_for(&C, "i2", "k1", t1 + Duration::from_secs(100), good()).err(), Some(Reason::Unavailable));
            assert_eq!(key_for(&C, "i1", "k1", t1 + Duration::from_secs(100), good()).err(), Some(Reason::Unavailable), "another issuer waits for the slot too");
            assert!(at.elapsed() < Duration::from_millis(100));
            release.send(()).unwrap();
            assert_eq!(h.join().unwrap().err(), Some(Reason::Unavailable));
        });
        // The resolver's refusal is remembered as the reason.
        let refused = || || Err((Reason::ProviderAddress, "private".to_owned()));
        let t2 = t1 + Duration::from_secs(500);
        assert_eq!(key_for(&C, "i3", "k1", t2, refused()).err(), Some(Reason::ProviderAddress));
        assert_eq!(key_for(&C, "i3", "k1", t2 + Duration::from_secs(1), good()).err(), Some(Reason::ProviderAddress));
    }
}

//! Staff sign-in sessions end to end (`POST`/`DELETE /api/team/session`, design 12.2-12.5): the
//! relay as a child process, the mock OpenID provider (`tools/oidc-mock`) in this process. Same
//! harness as tests/team.rs.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tether_trust::{envelope, generate_key, key_to_b64, reqauth, team, SigningKey};

static SEQ: AtomicU32 = AtomicU32::new(0);

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

struct Srv {
    child: Child,
    addr: String,
    dir: PathBuf,
    journal: Arc<Mutex<String>>,
}

impl Drop for Srv {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

struct Reply {
    status: u16,
    body: Vec<u8>,
    head: String,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|_| panic!("not JSON: {}", self.text()))
    }
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    fn retry_after(&self) -> Option<String> {
        self.head.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("retry-after")).map(|(_, v)| v.trim().to_owned()))
    }
    /// A refusal's reason code.
    fn reason(&self) -> String {
        self.json()["reason"].as_str().unwrap_or_default().to_owned()
    }
}

fn new_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tether-api-session-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn spawn_in(dir: PathBuf, admin: &SigningKey, env: &[(&str, &str)]) -> Srv {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_tether-api"));
    cmd.args(["--data", dir.to_str().unwrap(), "--listen", "127.0.0.1:0"]).env("ADMIN_PUB_KEY", key_to_b64(&admin.verifying_key())).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().unwrap();
    let journal = Arc::new(Mutex::new(String::new()));
    let (err, j) = (child.stderr.take().unwrap(), journal.clone());
    std::thread::spawn(move || {
        for l in BufReader::new(err).lines().map_while(Result::ok) {
            j.lock().unwrap().push_str(&(l + "\n"));
        }
    });
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let addr = line.trim().strip_prefix("listening on ").expect("listen line").to_owned();
    Srv { child, addr, dir, journal }
}

fn restart(mut s: Srv, admin: &SigningKey, env: &[(&str, &str)]) -> Srv {
    let _ = s.child.kill();
    let _ = s.child.wait();
    let dir = s.dir.clone();
    std::mem::forget(s); // its Drop would remove the folder
    spawn_in(dir, admin, env)
}

impl Srv {
    fn req(&self, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Reply {
        let mut s = TcpStream::connect(&self.addr).unwrap();
        let mut head = format!("{method} {path} HTTP/1.1\r\nHost: t\r\nConnection: close\r\nContent-Length: {}\r\n", body.len());
        for (k, v) in headers {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        head.push_str("\r\n");
        s.write_all(head.as_bytes()).unwrap();
        let _ = s.write_all(body);
        let mut raw = Vec::new();
        let _ = s.read_to_end(&mut raw);
        let split = raw.windows(4).position(|w| w == b"\r\n\r\n").expect("response head");
        let head = String::from_utf8_lossy(&raw[..split]).into_owned();
        let status = head.lines().next().unwrap().split_whitespace().nth(1).unwrap().parse().unwrap();
        Reply { status, body: raw[split + 4..].to_vec(), head }
    }

    /// Signed with `key`, naming it with `kid` (as 0.7 desktops do) or not.
    fn signed(&self, key: &SigningKey, kid: bool, method: &str, path: &str, body: &[u8]) -> Reply {
        let mut sig = reqauth::sign_request(key, method, path, body, now());
        if kid {
            sig = format!("{sig} kid={}", team::key_id(&key.verifying_key()));
        }
        self.req(method, path, &[(reqauth::HEADER, &sig)], body)
    }

    fn me(&self, key: &SigningKey) -> Reply {
        self.signed(key, true, "GET", "/api/team/me", b"")
    }

    fn sign_in(&self, key: &SigningKey, token: &str) -> Reply {
        let body = json!({"access_token": token, "key": key_to_b64(&key.verifying_key())}).to_string();
        self.signed(key, true, "POST", "/api/team/session", body.as_bytes())
    }

    fn sign_out(&self, key: &SigningKey) -> Reply {
        self.signed(key, true, "DELETE", "/api/team/session", b"")
    }

    fn publish(&self, admin: &SigningKey, body: &[u8]) -> Reply {
        self.signed(admin, false, "PUT", "/api/admin/team", body)
    }

    /// The team audit records of `event`.
    fn records(&self, admin: &SigningKey, event: &str) -> Vec<Value> {
        let r = self.signed(admin, false, "GET", &format!("/api/admin/team-audit?event={event}&limit=5000"), b"");
        assert_eq!(r.status, 200, "{}", r.text());
        r.json()["records"].as_array().unwrap().clone()
    }

    fn journal(&self) -> String {
        self.journal.lock().unwrap().clone()
    }

    /// Whether the journal gets `what` within 2 s (stderr is read by another thread).
    fn journal_has(&self, what: &str) -> bool {
        let t = Instant::now();
        while !self.journal().contains(what) && t.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(10));
        }
        self.journal().contains(what)
    }
}

fn mock() -> oidc_mock::Mock {
    oidc_mock::Mock::start(oidc_mock::Config { seed: Some(7), ..oidc_mock::Config::default() }, "127.0.0.1:0", false).unwrap()
}

fn oidc_of(m: &oidc_mock::Mock) -> Value {
    let c = m.config();
    json!({"issuer": m.issuer(), "tenant": c.tenant, "audience": c.audience, "client_id": c.client_id, "scope": c.scope})
}

fn roles() -> Value {
    json!({
        "Tether.Technician": {},
        "Tether.Software": {"sbom": true},
        "Tether.Technician.Write": {"write": true},
        "Tether.SessionAudit": {"audit": true},
        "Tether.Customer.4711": {"customers": ["4711"]},
    })
}

/// The upload of a signed team document `{serial, members, …extra}` with its signed config.
fn team_doc(admin: &SigningKey, serial: u64, members: Value, extra: Value) -> Vec<u8> {
    let mut d = json!({"serial": serial, "members": members});
    for (k, v) in extra.as_object().unwrap() {
        d[k] = v.clone();
    }
    let mut up = json!({"team": envelope::sign(&serde_json::to_vec(&d).unwrap(), admin)});
    if let Some(o) = d.get("oidc") {
        up["config"] = json!(envelope::sign(&serde_json::to_vec(&json!({"serial": serial, "oidc": o})).unwrap(), admin));
    }
    serde_json::to_vec(&up).unwrap()
}

fn entry(name: &str, key: &SigningKey, extra: Value) -> Value {
    let mut e = json!({"name": name, "key": key_to_b64(&key.verifying_key()), "sbom": true, "agent": true, "customers": ["*"]});
    for (k, v) in extra.as_object().unwrap() {
        e[k] = v.clone();
    }
    e
}

/// A relay with a document naming the mock as provider, the roles above and `members`.
fn relay(m: &oidc_mock::Mock, members: Value, env: &[(&str, &str)]) -> (Srv, SigningKey) {
    let admin = generate_key();
    let s = spawn_in(new_dir(), &admin, env);
    let r = s.publish(&admin, &team_doc(&admin, 1, members, json!({"oidc": oidc_of(m), "roles": roles()})));
    assert_eq!(r.status, 204, "{}", r.text());
    (s, admin)
}

fn oid(n: u8) -> String {
    format!("00000000-0000-4000-8000-0000000000{n:02}")
}

#[cfg(any(debug_assertions, feature = "test-hooks"))]
mod with_the_mock {
    use super::*;

    #[test]
    fn a_token_with_roles_gives_a_session_with_their_permissions() {
        let m = mock();
        let (s, admin) = relay(&m, json!([]), &[]);
        let k = generate_key();
        assert_eq!(s.me(&k).status, 401, "no session yet: not a member");
        let tok = m.token("software@tether.test", None).unwrap();
        let r = s.sign_in(&k, &tok);
        assert_eq!(r.status, 200, "{}", r.text());
        let v = r.json();
        let exp = v["exp"].as_u64().unwrap();
        assert!(exp > now() + 3000 && exp <= now() + 3600, "{exp}");
        assert_eq!(v, json!({"name": "Software Technician", "write": false, "audit": false, "sbom": true, "agent": false, "customers": ["*"], "service": false, "oid": oid(2), "upn": "software@tether.test", "exp": exp, "roles": ["Tether.Software", "Tether.Technician"]}));
        // Every member request of that key now carries the person; `me` answers the same.
        assert_eq!(s.me(&k).json(), v);
        assert_eq!(s.signed(&k, true, "GET", "/api/team/devices", b"").status, 200);
        assert_eq!(s.signed(&k, true, "PUT", "/api/team/tags", b"{}").status, 403, "no write");
        // A session needs `kid` in the header (the key is found by it).
        assert_eq!(s.signed(&k, false, "GET", "/api/team/me", b"").status, 401);
        // The customer role scopes; a role not in the table gives nothing; no mapped role: refused.
        let k2 = generate_key();
        assert_eq!(s.sign_in(&k2, &m.token("customer@tether.test", None).unwrap()).json()["customers"], json!(["4711"]));
        let k3 = generate_key();
        let r = s.sign_in(&k3, &m.token("auditor@tether.test", None).unwrap());
        assert_eq!((r.status, r.reason()), (403, "no-role".into()), "Tether.Manage.Auditor is not in the table");
        let r = s.sign_in(&k3, &m.token("noroles@tether.test", None).unwrap());
        assert_eq!((r.status, r.reason()), (403, "no-role".into()));
        assert_eq!(s.me(&k3).status, 401);
        // The audit log names the person and the key.
        let rec = s.records(&admin, "sign-in");
        assert_eq!(rec.len(), 2);
        assert_eq!(rec[0]["actor"], json!({"key_fingerprint": team::key_id(&k.verifying_key()), "name": "Software Technician", "oid": oid(2), "upn": "software@tether.test"}));
        assert_eq!((rec[0]["detail"]["roles"].clone(), rec[0]["result"].clone()), (json!(["Tether.Software", "Tether.Technician"]), json!(200)));
        let dev = s.records(&admin, "device-list");
        assert_eq!(dev[0]["actor"]["oid"], oid(2), "every member record names the person");
        // Sign-out: the key is no member any more; recorded.
        assert_eq!(s.sign_out(&k).status, 204);
        assert_eq!(s.me(&k).status, 401);
        assert_eq!(s.records(&admin, "sign-out")[0]["actor"]["oid"], oid(2));
        assert_eq!(s.sign_out(&k).status, 401, "nothing left to sign out: not a member");
    }

    #[test]
    fn the_request_must_be_signed_by_the_key_it_binds() {
        let m = mock();
        let (s, _admin) = relay(&m, json!([]), &[]);
        let (k, other) = (generate_key(), generate_key());
        let tok = m.token("technician@tether.test", None).unwrap();
        let body = |key: &SigningKey| json!({"access_token": tok, "key": key_to_b64(&key.verifying_key())}).to_string();
        // Signed by `other`, binding `k`: refused, whatever kid says.
        let b = body(&k);
        for kid in [team::key_id(&k.verifying_key()), team::key_id(&other.verifying_key())] {
            let sig = format!("{} kid={kid}", reqauth::sign_request(&other, "POST", "/api/team/session", b.as_bytes(), now()));
            assert_eq!(s.req("POST", "/api/team/session", &[(reqauth::HEADER, &sig)], b.as_bytes()).status, 401, "{kid}");
        }
        // Without kid, or unsigned: refused.
        let sig = reqauth::sign_request(&k, "POST", "/api/team/session", b.as_bytes(), now());
        assert_eq!(s.req("POST", "/api/team/session", &[(reqauth::HEADER, &sig)], b.as_bytes()).status, 401);
        assert_eq!(s.req("POST", "/api/team/session", &[], b.as_bytes()).status, 401);
        // Bad bodies.
        for bad in [json!({"access_token": tok}).to_string(), json!({"access_token": tok, "key": "x"}).to_string(), json!({"access_token": tok, "key": key_to_b64(&k.verifying_key()), "more": 1}).to_string(), "[]".into()] {
            assert_eq!(s.signed(&k, true, "POST", "/api/team/session", bad.as_bytes()).status, 400, "{bad}");
        }
        assert_eq!(s.sign_in(&k, &tok).status, 200);
        // The same signed request again: a replay.
        let sig = format!("{} kid={}", reqauth::sign_request(&k, "POST", "/api/team/session", b.as_bytes(), now()), team::key_id(&k.verifying_key()));
        assert_eq!(s.req("POST", "/api/team/session", &[(reqauth::HEADER, &sig)], b.as_bytes()).status, 200);
        assert_eq!(s.req("POST", "/api/team/session", &[(reqauth::HEADER, &sig)], b.as_bytes()).text(), "replayed request");
    }

    /// Every error shape of the mock is refused for its own reason, recorded without the token.
    #[test]
    fn every_token_shape_is_refused_with_its_reason_and_audited() {
        let m = mock();
        let (s, admin) = relay(&m, json!([]), &[]);
        let k = generate_key();
        assert_eq!(s.sign_in(&k, &m.token("technician@tether.test", None).unwrap()).status, 200, "the keys are fetched");
        let shapes = [
            ("expired", 401, "expired"),
            ("not_yet_valid", 401, "not-yet-valid"),
            ("wrong_aud", 401, "wrong-audience"),
            ("aud_app_id_uri", 401, "wrong-audience"),
            ("graph", 401, "wrong-audience"),
            ("wrong_iss", 401, "wrong-issuer"),
            ("wrong_tid", 401, "wrong-tenant"),
            ("wrong_azp", 401, "wrong-client"),
            ("wrong_ver", 401, "v1-token"),
            ("v1", 401, "v1-token"),
            ("overage", 401, "overage"),
            ("hasgroups", 401, "overage"),
            ("unknown_kid", 401, "unknown-kid"),
            ("bad_signature", 401, "bad-signature"),
        ];
        let mut tokens = Vec::new();
        for (shape, status, reason) in shapes {
            let tok = m.token("technician@tether.test", Some(shape)).unwrap();
            let r = s.sign_in(&generate_key(), &tok);
            assert_eq!((r.status, r.reason()), (status, reason.to_owned()), "{shape}: {}", r.text());
            assert!(!r.json()["message"].as_str().unwrap().is_empty());
            tokens.push(tok);
        }
        let r = s.sign_in(&generate_key(), "not.a.token");
        assert_eq!((r.status, r.reason()), (401, "malformed".into()));
        assert!(s.sign_in(&generate_key(), "").reason() == "malformed");
        // Each refusal one record with its reason and the key, nothing of the token.
        let rec = s.records(&admin, "sign-in-refused");
        let reasons: Vec<&str> = rec.iter().map(|r| r["detail"]["reason"].as_str().unwrap()).collect();
        let want: Vec<&str> = shapes.iter().map(|x| x.2).chain(["malformed", "malformed"]).collect();
        assert_eq!(reasons, want);
        assert!(rec.iter().all(|r| r["actor"]["key_fingerprint"].as_str().is_some_and(|k| k.len() == 16) && r["actor"].get("oid").is_none()), "unverified claims are not recorded");
        let log: String = std::fs::read_dir(s.dir.join("team-audit")).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".jsonl")).map(|e| std::fs::read_to_string(e.path()).unwrap()).collect();
        for t in &tokens {
            for part in t.split('.') {
                assert!(!log.contains(&part[..part.len().min(40)]), "a token part in the audit log");
            }
        }
        assert!(!s.journal().contains(&tokens[0][..60]), "a token in the journal");
    }

    #[test]
    fn a_token_binds_one_key() {
        let m = mock();
        let (s, admin) = relay(&m, json!([]), &[]);
        let (a, b) = (generate_key(), generate_key());
        let tok = m.token("writer@tether.test", None).unwrap();
        assert_eq!(s.sign_in(&a, &tok).status, 200);
        assert_eq!(s.sign_in(&a, &tok).status, 200, "the same key again (a retry)");
        let r = s.sign_in(&b, &tok);
        assert_eq!((r.status, r.reason()), (409, "token-used".into()));
        assert_eq!(s.me(&b).status, 401);
        // Also after the first key signed out, and after a restart.
        assert_eq!(s.sign_out(&a).status, 204);
        assert_eq!(s.sign_in(&b, &tok).reason(), "token-used");
        let s = restart(s, &admin, &[]);
        let r = s.sign_in(&b, &tok);
        assert_eq!((r.status, r.reason()), (409, "token-used".into()), "the binding is kept in sessions.json");
        // A fresh token for b works.
        assert_eq!(s.sign_in(&b, &m.token("writer@tether.test", None).unwrap()).json()["write"], true);
    }

    /// A person's ninth key replaces the oldest session: one `session-ended` record, reason `replaced`.
    #[test]
    fn a_ninth_key_replaces_a_session_and_says_so() {
        let m = mock();
        let (s, admin) = relay(&m, json!([]), &[]);
        let keys: Vec<SigningKey> = (0..9).map(|_| generate_key()).collect();
        for k in &keys[..8] {
            assert_eq!(s.sign_in(k, &m.token("technician@tether.test", None).unwrap()).status, 200);
        }
        assert!(s.records(&admin, "session-ended").is_empty());
        assert_eq!(s.sign_in(&keys[8], &m.token("technician@tether.test", None).unwrap()).status, 200);
        let ended = s.records(&admin, "session-ended");
        assert_eq!(ended.len(), 1, "{ended:?}");
        let gone: Vec<&SigningKey> = keys.iter().filter(|k| s.me(k).status == 401).collect();
        assert_eq!(gone.len(), 1, "exactly one session replaced");
        let kid = team::key_id(&gone[0].verifying_key());
        assert_eq!((ended[0]["detail"]["reason"].clone(), ended[0]["detail"]["kid"].clone(), ended[0]["actor"]["oid"].clone()), (json!("replaced"), json!(kid), json!(oid(1))));
    }

    /// Tokens refused after their signature verified (expired, wrong audience, ...) count per key
    /// (20 a minute, `429` with `Retry-After`), not against the source address, whose limit (60
    /// failures) refuses every team route.
    #[test]
    fn refused_tokens_count_per_key_and_not_per_address() {
        let m = mock();
        let roster = generate_key();
        let (s, _admin) = relay(&m, json!([entry("Roster Anna", &roster, json!({}))]), &[]);
        let expired = m.token("technician@tether.test", Some("expired")).unwrap();
        // 4 keys x 18 refusals from one address: 72, over the address limit of 60.
        for _ in 0..4 {
            let k = generate_key();
            for _ in 0..18 {
                assert_eq!(s.sign_in(&k, &expired).status, 401);
            }
        }
        assert_eq!(s.me(&roster).status, 200, "the address is not locked out");
        assert_eq!(s.sign_in(&generate_key(), &m.token("technician@tether.test", None).unwrap()).status, 200);
        // One key: 20 refusals, then 429 with Retry-After, also for a good token; others unaffected.
        let k = generate_key();
        for _ in 0..20 {
            assert_eq!(s.sign_in(&k, &expired).status, 401);
        }
        let r = s.sign_in(&k, &m.token("technician@tether.test", None).unwrap());
        assert_eq!((r.status, r.retry_after()), (429, Some("60".into())), "{}", r.text());
        assert_eq!(s.sign_in(&generate_key(), &m.token("technician@tether.test", None).unwrap()).status, 200);
        assert_eq!(s.me(&roster).status, 200);
    }

    /// Sign-ins with a fresh key each and a junk token (anyone may send them): nothing is kept per
    /// request (no nonce), they count against the source address (60 a minute, then `429`), their
    /// `sign-in-refused` records are capped relay-wide (60 a minute), and a heartbeat sent during
    /// the flood is answered before it ends.
    #[test]
    fn junk_sign_ins_with_fresh_keys_stay_bounded() {
        let m = mock();
        let roster = generate_key();
        let (s, admin) = relay(&m, json!([entry("Roster Anna", &roster, json!({}))]), &[]);
        let sent = std::sync::atomic::AtomicUsize::new(0);
        let junk = |ip: &str| {
            sent.fetch_add(1, Ordering::SeqCst);
            let k = generate_key();
            let body = json!({"access_token": "not.a.token", "key": key_to_b64(&k.verifying_key())}).to_string();
            let sig = format!("{} kid={}", reqauth::sign_request(&k, "POST", "/api/team/session", body.as_bytes(), now()), team::key_id(&k.verifying_key()));
            s.req("POST", "/api/team/session", &[(reqauth::HEADER, &sig), ("X-Forwarded-For", ip)], body.as_bytes()).status
        };
        let ips = ["198.51.100.1", "198.51.100.2", "198.51.100.3"];
        std::thread::scope(|sc| {
            let flood = sc.spawn(|| {
                let got: Vec<Vec<u16>> = ips.iter().map(|ip| (0..70).map(|_| junk(ip)).collect()).collect();
                (got, Instant::now())
            });
            // Early in the flood (210 requests), so it is still running when the heartbeat is answered.
            while sent.load(Ordering::SeqCst) < 20 {
                std::thread::sleep(Duration::from_millis(1));
            }
            let t = Instant::now();
            let hb = r#"{"uuid":"hb-00002","id":"111222334","profile":"technician","version":"0.5.21","os":"windows 10.0.26300","serial":3}"#;
            assert_eq!(s.req("POST", "/api/heartbeat", &[], hb.as_bytes()).status, 200);
            let (took, done) = (t.elapsed(), Instant::now());
            let (got, end) = flood.join().unwrap();
            assert!(took < Duration::from_millis(300) && done < end, "the heartbeat waited for the flood: {took:?}");
            for (ip, g) in ips.iter().zip(&got) {
                assert!(g[..60].iter().all(|&c| c == 401) && g[60..].iter().all(|&c| c == 429), "{ip}: {g:?}");
            }
        });
        let nonces = std::fs::read_to_string(s.dir.join("nonces.log")).unwrap_or_default().lines().count() + std::fs::read(s.dir.join("nonces.json")).ok().map_or(0, |b| serde_json::from_slice::<std::collections::HashMap<String, u64>>(&b).unwrap().len());
        assert!(nonces < 10, "{nonces} nonces stored");
        let refused = s.records(&admin, "sign-in-refused");
        assert!(refused.len() <= 60, "{} sign-in-refused records", refused.len());
        assert_eq!(s.me(&roster).status, 200, "another address is not locked out");
        assert_eq!(s.sign_in(&generate_key(), &m.token("technician@tether.test", None).unwrap()).status, 200);
    }

    /// Nonces in `nonces.log` and `nonces.json`.
    fn stored_nonces(s: &Srv) -> usize {
        std::fs::read_to_string(s.dir.join("nonces.log")).unwrap_or_default().lines().count() + std::fs::read(s.dir.join("nonces.json")).ok().map_or(0, |b| serde_json::from_slice::<std::collections::HashMap<String, u64>>(&b).unwrap().len())
    }

    /// Sign-ins refused after their token verified (here `no-role`; anyone with a token of the
    /// tenant may send them, each with a fresh key): no nonce is kept, 20 a minute per person, then
    /// `429` with `Retry-After` and no further `sign-in-refused` record; others are not affected.
    #[test]
    fn refused_sign_ins_of_one_person_keep_no_nonce_and_are_limited() {
        let m = mock();
        let (s, admin) = relay(&m, json!([]), &[]);
        let tok = m.token("noroles@tether.test", None).unwrap();
        let before = stored_nonces(&s);
        for i in 0..20 {
            let r = s.sign_in(&generate_key(), &tok);
            assert_eq!((r.status, r.reason()), (403, "no-role".into()), "sign-in {i}");
        }
        assert_eq!(stored_nonces(&s), before, "a refused sign-in keeps no nonce");
        for _ in 0..5 {
            let r = s.sign_in(&generate_key(), &tok);
            assert_eq!((r.status, r.retry_after()), (429, Some("60".into())), "{}", r.text());
        }
        assert_eq!(stored_nonces(&s), before);
        assert_eq!(s.sign_in(&generate_key(), &m.token("technician@tether.test", None).unwrap()).status, 200);
        assert_eq!(stored_nonces(&s), before + 1, "a sign-in that succeeds keeps its nonce");
        assert_eq!(s.records(&admin, "sign-in-refused").len(), 20);
    }

    /// Sign-ins whose token does not verify do not count toward the key's request limit (120 a
    /// minute): nothing is kept for a key before its token verified (they count per address).
    #[test]
    fn unverified_sign_ins_are_not_counted_per_key() {
        let m = mock();
        let (s, _admin) = relay(&m, json!([]), &[]);
        let k = generate_key();
        let body = json!({"access_token": "not.a.token", "key": key_to_b64(&k.verifying_key())}).to_string();
        for i in 0..125 {
            let ip = format!("198.51.100.{}", 20 + i / 50);
            let sig = format!("{} kid={}", reqauth::sign_request(&k, "POST", "/api/team/session", body.as_bytes(), now()), team::key_id(&k.verifying_key()));
            assert_eq!(s.req("POST", "/api/team/session", &[(reqauth::HEADER, &sig), ("X-Forwarded-For", &ip)], body.as_bytes()).status, 401, "sign-in {i}");
        }
    }

    /// A Tether Manage session's key over its limit (120 a minute) is refused before its nonce is
    /// kept on the admin routes too.
    #[test]
    fn a_manage_session_key_is_limited_before_its_nonce_is_kept() {
        let m = mock();
        let admin = generate_key();
        let s = spawn_in(new_dir(), &admin, &[]);
        assert_eq!(s.publish(&admin, &team_doc(&admin, 1, json!([]), json!({"oidc": oidc_of(&m), "roles": manage_roles()}))).status, 204);
        let k = generate_key();
        assert_eq!(s.sign_in(&k, &m.token("auditor@tether.test", None).unwrap()).status, 200);
        let before = stored_nonces(&s);
        let codes: Vec<u16> = (0..130).map(|_| s.signed(&k, true, "GET", "/api/admin/status", b"").status).collect();
        let ok = codes.iter().take_while(|&&c| c == 200).count();
        assert!(ok >= 118 && codes[ok..].iter().all(|&c| c == 429), "{codes:?}");
        assert_eq!(stored_nonces(&s), before + ok, "the refused ones keep no nonce");
    }

    /// An entry or field a newer relay wrote must not wipe the one-token-one-key bindings; a file
    /// that is no JSON is kept aside as `sessions.json.bad`.
    #[test]
    fn sessions_json_is_read_tolerantly_and_a_corrupt_one_is_kept_aside() {
        let m = mock();
        let (s, admin) = relay(&m, json!([]), &[]);
        let (a, b) = (generate_key(), generate_key());
        let tok = m.token("writer@tether.test", None).unwrap();
        assert_eq!(s.sign_in(&a, &tok).status, 200);
        let stop = |mut s: Srv| {
            let _ = s.child.kill();
            let _ = s.child.wait();
            let dir = s.dir.clone();
            std::mem::forget(s);
            dir
        };
        let dir = stop(s);
        let file = dir.join("team/sessions.json");
        let mut v: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        v["from_a_newer_relay"] = json!({"x": 1});
        v["sessions"][0]["more"] = json!(true);
        v["tokens"][0]["more"] = json!([1]);
        v["tokens"].as_array_mut().unwrap().push(json!({"shape": "unknown"}));
        std::fs::write(&file, serde_json::to_vec(&v).unwrap()).unwrap();
        let s = spawn_in(dir, &admin, &[]);
        assert_eq!(s.me(&a).status, 200, "the session survived");
        assert_eq!(s.sign_in(&b, &tok).reason(), "token-used", "and the binding");
        let dir = stop(s);
        std::fs::write(dir.join("team/sessions.json"), b"{\"sessions\": [").unwrap();
        let s = spawn_in(dir.clone(), &admin, &[]);
        assert_eq!(std::fs::read(dir.join("team/sessions.json.bad")).unwrap(), b"{\"sessions\": [");
        assert!(s.journal_has("sessions.json.bad"), "{}", s.journal());
        assert_eq!(s.me(&a).status, 401);
    }

    /// The deny list refuses at sign-in and ends live sessions when the document arrives; a
    /// document that leaves a session no mapped role, or changes the provider, ends it too.
    #[test]
    fn the_deny_list_and_a_new_document_end_sessions() {
        let m = mock();
        let (s, admin) = relay(&m, json!([]), &[]);
        let (a, b, c) = (generate_key(), generate_key(), generate_key());
        assert_eq!(s.sign_in(&a, &m.token("technician@tether.test", None).unwrap()).status, 200);
        assert_eq!(s.sign_in(&b, &m.token("software@tether.test", None).unwrap()).status, 200);
        assert_eq!(s.sign_in(&c, &m.token("writer@tether.test", None).unwrap()).status, 200);
        assert_eq!(s.publish(&admin, &team_doc(&admin, 2, json!([]), json!({"oidc": oidc_of(&m), "roles": roles(), "deny": [oid(1)]}))).status, 204);
        assert_eq!(s.me(&a).status, 401, "ended at once");
        assert_eq!(s.me(&b).status, 200);
        let r = s.sign_in(&a, &m.token("technician@tether.test", None).unwrap());
        assert_eq!((r.status, r.reason()), (403, "denied".into()));
        let ended = s.records(&admin, "session-ended");
        assert_eq!((ended.len(), ended[0]["actor"]["oid"].clone(), ended[0]["detail"]["reason"].clone()), (1, json!(oid(1)), json!("deny-list")));
        // Fewer rights on the next request; the software role gone from the table: b keeps
        // Technician, c keeps write.
        let mut r2 = roles();
        r2.as_object_mut().unwrap().remove("Tether.Software");
        assert_eq!(s.publish(&admin, &team_doc(&admin, 3, json!([]), json!({"oidc": oidc_of(&m), "roles": r2}))).status, 204);
        assert_eq!(s.me(&b).json()["sbom"], false);
        assert_eq!(s.me(&c).json()["write"], true);
        // Only Technician.Write left: b has no mapped role any more.
        let r3 = json!({"Tether.Technician.Write": {"write": true}});
        assert_eq!(s.publish(&admin, &team_doc(&admin, 4, json!([]), json!({"oidc": oidc_of(&m), "roles": r3}))).status, 204);
        assert_eq!(s.me(&b).status, 401);
        assert_eq!(s.me(&c).status, 200);
        // Another provider configuration: every session ends.
        let mut o = oidc_of(&m);
        o["client_id"] = json!("another-client");
        assert_eq!(s.publish(&admin, &team_doc(&admin, 5, json!([]), json!({"oidc": o, "roles": roles()}))).status, 204);
        assert_eq!(s.me(&c).status, 401);
        let reasons: Vec<Value> = s.records(&admin, "session-ended").iter().map(|r| r["detail"]["reason"].clone()).collect();
        assert_eq!(reasons, [json!("deny-list"), json!("no-role"), json!("document-changed")]);
    }

    #[test]
    fn a_session_expires_with_its_token_or_the_relay_cap() {
        let m = mock();
        let roster = generate_key();
        let (s, _admin) = relay(&m, json!([entry("Roster Anna", &roster, json!({}))]), &[]);
        m.control("lifetime", json!({"seconds": 3})).unwrap();
        let k = generate_key();
        let r = s.sign_in(&k, &m.token("technician@tether.test", None).unwrap());
        assert!(r.json()["exp"].as_u64().unwrap() <= now() + 3);
        // A roster key that signs in: the session wins while it lasts, the roster entry after.
        assert_eq!(s.sign_in(&roster, &m.token("software@tether.test", None).unwrap()).json()["name"], "Software Technician");
        assert_eq!(s.me(&roster).json()["name"], "Software Technician");
        std::thread::sleep(Duration::from_secs(4));
        let r = s.me(&k);
        assert_eq!((r.status, r.text()), (401, "session expired: sign in again".into()));
        assert_eq!(s.me(&roster).json()["name"], "Roster Anna");
        // TEAM_SESSION_MAX_MINUTES caps a long token.
        m.control("lifetime", json!({"seconds": 3600})).unwrap();
        let admin = generate_key();
        let s = spawn_in(new_dir(), &admin, &[("TEAM_SESSION_MAX_MINUTES", "1")]);
        assert_eq!(s.publish(&admin, &team_doc(&admin, 1, json!([]), json!({"oidc": oidc_of(&m), "roles": roles()}))).status, 204);
        let exp = s.sign_in(&k, &m.token("technician@tether.test", None).unwrap()).json()["exp"].as_u64().unwrap();
        assert!(exp <= now() + 60 && exp + 5 >= now() + 60, "{exp}");
    }

    #[test]
    fn oidc_only_leaves_keys_to_service_entries() {
        let m = mock();
        let admin = generate_key();
        let s = spawn_in(new_dir(), &admin, &[]);
        let (anna, svc) = (generate_key(), generate_key());
        let members = json!([entry("Anna", &anna, json!({})), entry("Bridge", &svc, json!({"service": true}))]);
        assert_eq!(s.publish(&admin, &team_doc(&admin, 1, members, json!({"oidc": oidc_of(&m), "roles": roles(), "oidc_only": true}))).status, 204);
        assert_eq!(s.me(&anna).status, 401);
        assert_eq!(s.signed(&anna, false, "GET", "/api/team/me", b"").status, 401);
        assert_eq!(s.me(&svc).json()["service"], true);
        // Anna signs in with her key: a session, her person and roles.
        assert_eq!(s.sign_in(&anna, &m.token("technician@tether.test", None).unwrap()).status, 200);
        assert_eq!(s.me(&anna).json()["name"], "Test Technician");
    }

    /// The provider down: sessions keep working until they expire (also across a restart, from
    /// sessions.json), roster members are unaffected, new sign-ins get 503.
    #[test]
    fn sessions_outlive_the_provider_and_a_restart() {
        let m = mock();
        let roster = generate_key();
        let (s, admin) = relay(&m, json!([entry("Roster Anna", &roster, json!({}))]), &[]);
        let (k, gone) = (generate_key(), generate_key());
        assert_eq!(s.sign_in(&k, &m.token("software@tether.test", None).unwrap()).status, 200);
        assert_eq!(s.sign_in(&gone, &m.token("technician@tether.test", None).unwrap()).status, 200);
        assert_eq!(s.sign_out(&gone).status, 204);
        let later = m.token("writer@tether.test", None).unwrap();
        drop(m);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(s.dir.join("team/sessions.json")).unwrap().permissions().mode() & 0o777, 0o600);
        let s = restart(s, &admin, &[]);
        assert_eq!(s.me(&k).json()["name"], "Software Technician", "survives the restart");
        assert_eq!(s.me(&gone).status, 401, "signed out stays signed out");
        assert_eq!(s.me(&roster).status, 200);
        let t = Instant::now();
        let r = s.sign_in(&generate_key(), &later);
        assert_eq!((r.status, r.reason()), (503, "provider-unavailable".into()), "{}", r.text());
        assert!(t.elapsed() < Duration::from_secs(12));
        // Within the minute after a failed fetch: 503 at once, no new fetch.
        let t = Instant::now();
        assert_eq!(s.sign_in(&generate_key(), &later).status, 503);
        assert!(t.elapsed() < Duration::from_millis(500), "{:?}", t.elapsed());
        assert!(s.journal_has("could not be fetched"), "{}", s.journal());
        assert_eq!(s.me(&k).status, 200);
    }

    /// A provider that accepts and never answers: the sign-in waiting for it holds one worker,
    /// the next sign-in gets 503 at once, device heartbeats are answered before it.
    #[test]
    fn a_hanging_provider_delays_no_heartbeat() {
        let hang = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let held = Arc::new(Mutex::new(Vec::new()));
        let h = held.clone();
        let port = hang.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for c in hang.incoming().flatten() {
                h.lock().unwrap().push(c);
            }
        });
        let m = mock();
        let admin = generate_key();
        let s = spawn_in(new_dir(), &admin, &[]);
        let mut o = oidc_of(&m);
        o["issuer"] = json!(format!("http://127.0.0.1:{port}/t/v2.0"));
        assert_eq!(s.publish(&admin, &team_doc(&admin, 1, json!([]), json!({"oidc": o, "roles": roles()}))).status, 204);
        let tok = m.token("technician@tether.test", None).unwrap();
        std::thread::scope(|sc| {
            let first = sc.spawn(|| {
                let r = s.sign_in(&generate_key(), &tok);
                (r.status, Instant::now())
            });
            let t = Instant::now();
            while held.lock().unwrap().is_empty() {
                assert!(t.elapsed() < Duration::from_secs(10), "the relay never asked the provider");
                std::thread::sleep(Duration::from_millis(10));
            }
            let t = Instant::now();
            let r = s.sign_in(&generate_key(), &tok);
            assert_eq!((r.status, r.reason()), (503, "provider-unavailable".into()));
            let hb = r#"{"uuid":"hb-00001","id":"111222333","profile":"technician","version":"0.5.21","os":"windows 10.0.26300","serial":3}"#;
            assert_eq!(s.req("POST", "/api/heartbeat", &[], hb.as_bytes()).status, 200);
            let (took, done) = (t.elapsed(), Instant::now());
            assert!(took < Duration::from_millis(300), "{took:?}");
            let (status, end) = first.join().unwrap();
            assert_eq!(status, 503);
            assert!(done < end, "the heartbeat waited for the sign-in");
        });
    }

    /// A discovery document whose jwks_uri points to another host is not followed.
    #[test]
    fn a_jwks_uri_on_another_host_is_refused() {
        let fake = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = fake.server_addr().to_ip().unwrap().port();
        let issuer = format!("http://127.0.0.1:{port}/t/v2.0");
        let m = mock();
        let jwks = m.discovery()["jwks_uri"].as_str().unwrap().replace("127.0.0.1", "localhost");
        let disc = json!({"issuer": issuer, "jwks_uri": jwks}).to_string();
        let asked = Arc::new(Mutex::new(Vec::new()));
        let a = asked.clone();
        std::thread::spawn(move || {
            for rq in fake.incoming_requests() {
                a.lock().unwrap().push(rq.url().to_owned());
                let _ = rq.respond(tiny_http::Response::from_string(disc.clone()));
            }
        });
        let admin = generate_key();
        let s = spawn_in(new_dir(), &admin, &[]);
        let mut o = oidc_of(&m);
        o["issuer"] = json!(issuer);
        assert_eq!(s.publish(&admin, &team_doc(&admin, 1, json!([]), json!({"oidc": o, "roles": roles()}))).status, 204);
        let r = s.sign_in(&generate_key(), &m.token("technician@tether.test", None).unwrap());
        assert_eq!((r.status, r.reason()), (503, "provider-unavailable".into()));
        assert_eq!(*asked.lock().unwrap(), ["/t/v2.0/.well-known/openid-configuration"]);
        assert!(s.journal_has("not on the issuer's own host"), "{}", s.journal());
    }

    #[test]
    fn no_sign_in_without_oidc_in_the_document() {
        let admin = generate_key();
        let s = spawn_in(new_dir(), &admin, &[]);
        let k = generate_key();
        let r = s.sign_in(&k, "a.b.c");
        assert_eq!((r.status, r.reason()), (404, "not-configured".into()));
        assert_eq!(s.publish(&admin, &team_doc(&admin, 1, json!([]), json!({}))).status, 204);
        assert_eq!(s.sign_in(&k, "a.b.c").reason(), "not-configured");
    }

    /// The roles above and the five Tether Manage roles of design 12.4.
    fn manage_roles() -> Value {
        let mut r = roles();
        for (role, m) in [("Auditor", "auditor"), ("Operator", "operator"), ("PolicyEditor", "policy-editor"), ("Publisher", "publisher"), ("KeyHolder", "key-holder")] {
            r[format!("Tether.Manage.{role}")] = json!({"manage": m});
        }
        r
    }

    fn sha256_hex(b: &[u8]) -> String {
        use sha2::Digest;
        sha2::Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
    }

    /// Each Tether Manage role reaches exactly its row of design 12.4 at the relay's admin routes;
    /// everything that signs or publishes stays with the admin signature; a proposal is stored
    /// unsigned, never served as live, and goes live only when the admin key publishes it.
    #[test]
    fn manage_roles_reach_exactly_their_admin_routes() {
        let m = mock();
        let admin = generate_key();
        let s = spawn_in(new_dir(), &admin, &[]);
        let tech = generate_key();
        let doc = team_doc(&admin, 1, json!([entry("Tech", &tech, json!({}))]), json!({"oidc": oidc_of(&m), "roles": manage_roles()}));
        assert_eq!(s.publish(&admin, &doc).status, 204);
        let person = |user: &str| {
            let k = generate_key();
            let r = s.sign_in(&k, &m.token(&format!("{user}@tether.test"), None).unwrap());
            assert_eq!(r.status, 200, "{user}: {}", r.text());
            (k, r.json())
        };
        let (auditor, a) = person("auditor");
        let (operator, _) = person("operator");
        let (editor, _) = person("editor");
        let (publisher, _) = person("publisher");
        let (holder, _) = person("keyholder");
        // The sign-in answer names the Manage role; no team permission comes with it.
        assert_eq!((a["manage"].clone(), a["customers"].clone(), a["sbom"].clone()), (json!(["auditor"]), json!([]), json!(false)));
        assert_eq!(s.me(&auditor).json()["manage"], json!(["auditor"]));
        assert_eq!(s.signed(&auditor, true, "GET", "/api/team/devices", b"").status, 403, "a Manage role is no team member");
        assert_eq!(s.signed(&auditor, true, "GET", "/api/team/status", b"").status, 403);

        let policy = json!({"serial": 3, "issued_at": 1});
        let put = |k: &SigningKey, kind: &str, doc: &Value| s.signed(k, true, "PUT", &format!("/api/admin/proposals/{kind}"), json!({"base": "", "note": "why", "doc": doc}).to_string().as_bytes());
        let signed_policy = serde_json::to_vec(&envelope::sign(&serde_json::to_vec(&policy).unwrap(), &admin)).unwrap();
        // (route, body, [auditor, operator, editor, publisher, key holder] may)
        let reads = ["/api/admin/status", "/api/admin/devices", "/api/admin/status/devices", "/api/admin/status/revoked", "/api/admin/audit?device=123456789", "/api/admin/team-audit", "/api/admin/policy-history", "/api/admin/releases"];
        for path in reads {
            for (who, k) in [("auditor", &auditor), ("operator", &operator), ("editor", &editor), ("publisher", &publisher), ("holder", &holder)] {
                let r = s.signed(k, true, "GET", path, b"");
                assert_eq!(r.status, 200, "{who} GET {path}: {}", r.text());
            }
        }
        let rows: Vec<(&str, &str, Vec<u8>, [bool; 5])> = vec![
            ("GET", "/api/admin/diag", vec![], [false, true, false, false, false]),
            ("POST", "/api/admin/diag-ticket", br#"{"note":"n"}"#.to_vec(), [false, true, false, false, false]),
            ("DELETE", "/api/admin/status/dev-9999", vec![], [false, true, false, false, false]),
            ("POST", "/api/admin/status/revoke", br#"{"id":"x"}"#.to_vec(), [false, true, false, false, false]),
            ("GET", "/api/admin/team", vec![], [false, false, true, false, true]),
            ("GET", "/api/admin/proposals/policy", vec![], [false, false, true, false, true]),
            ("GET", "/api/admin/proposals/team", vec![], [false, false, true, false, true]),
            ("GET", "/api/admin/proposals/release", vec![], [false, false, false, true, true]),
            // Signing and publishing: never with a session, whatever the role.
            ("PUT", "/api/admin/policy", signed_policy.clone(), [false; 5]),
            ("PUT", "/api/admin/team", doc.clone(), [false; 5]),
            ("PUT", "/api/admin/download-page", b"<p>x</p>".to_vec(), [false; 5]),
            ("GET", "/api/admin/custom", vec![], [false; 5]),
            ("POST", "/api/admin/releases/prune", br#"{"dry_run":true}"#.to_vec(), [false; 5]),
            ("DELETE", "/api/admin/release/stable/x/y.exe", vec![], [false; 5]),
            // Verified with the session's key before the lock, then refused like the others (not 401).
            ("PUT", "/api/admin/release/stable/x/y.exe", b"build".to_vec(), [false; 5]),
        ];
        for (method, path, body, may) in &rows {
            for ((who, k), ok) in [("auditor", &auditor), ("operator", &operator), ("editor", &editor), ("publisher", &publisher), ("holder", &holder)].into_iter().zip(may) {
                let r = s.signed(k, true, method, path, body);
                assert_eq!(r.status != 403, *ok, "{who} {method} {path}: {} {}", r.status, r.text());
                assert!(r.status != 401, "{who} {method} {path}: {}", r.text());
            }
        }
        assert_eq!(s.req("GET", "/api/policy", &[], b"").status, 404, "nothing was published");
        // Proposals: who may write which kind.
        for (k, kind, d, code) in [(&editor, "policy", &policy, 200), (&editor, "team", &json!({"serial": 2, "members": []}), 200), (&publisher, "release", &json!({"channel": "stable", "version": "0.7.0"}), 200), (&editor, "release", &json!({}), 403), (&publisher, "policy", &policy, 403), (&auditor, "policy", &policy, 403), (&operator, "team", &json!({"serial": 2, "members": []}), 403), (&holder, "policy", &policy, 403)] {
            assert_eq!(put(k, kind, d).status, code, "{kind}");
        }
        // Validated like a publish; bounded.
        assert_eq!(put(&editor, "policy", &json!({"serial": 0})).status, 400);
        assert_eq!(put(&editor, "team", &json!({"serial": 2, "members": [], "x": 1})).status, 400);
        assert_eq!(s.signed(&editor, true, "PUT", "/api/admin/proposals/policy", json!({"base": "nothex", "doc": policy}).to_string().as_bytes()).status, 400);
        assert_eq!(s.signed(&editor, true, "PUT", "/api/admin/proposals/other", json!({"doc": policy}).to_string().as_bytes()).status, 403);
        for _ in 1..10 {
            assert_eq!(put(&editor, "policy", &policy).status, 200);
        }
        let r = put(&editor, "policy", &policy);
        assert_eq!(r.status, 409, "{}", r.text());
        // Stored unsigned with its author; never live.
        let list = s.signed(&holder, true, "GET", "/api/admin/proposals/policy", b"").json();
        let p0 = list["proposals"][0].clone();
        assert_eq!((list["live"].clone(), list["proposals"].as_array().unwrap().len()), (Value::Null, 10));
        assert_eq!((p0["kind"].clone(), p0["serial"].clone(), p0["doc"].clone(), p0["note"].clone(), p0["author"]["oid"].clone()), (json!("policy"), json!(3), policy.clone(), json!("why"), json!(oid(7))));
        assert_eq!(s.req("GET", "/api/policy", &[], b"").status, 404, "a proposal is never served as live");
        // Only its author or a key holder removes one; the key holder signs and publishes it.
        let id = p0["id"].as_str().unwrap();
        let admin_put = s.signed(&admin, false, "PUT", "/api/admin/proposals/policy", json!({"doc": policy}).to_string().as_bytes());
        assert_eq!(admin_put.status, 409, "full; the admin signature is accepted too");
        let ids: Vec<String> = list["proposals"].as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap().to_owned()).collect();
        assert_eq!(s.signed(&editor, true, "DELETE", &format!("/api/admin/proposals/policy/{}", ids[1]), b"").status, 204, "its author");
        assert_eq!(s.signed(&publisher, true, "DELETE", &format!("/api/admin/proposals/policy/{}", ids[2]), b"").status, 403);
        assert_eq!(s.signed(&admin, false, "PUT", "/api/admin/policy", &signed_policy).status, 204);
        assert_eq!(s.signed(&holder, true, "DELETE", &format!("/api/admin/proposals/policy/{id}"), b"").status, 204, "a key holder, after publishing");
        let served = s.req("GET", "/api/policy", &[], b"");
        assert_eq!(served.body, signed_policy);
        let list = s.signed(&editor, true, "GET", "/api/admin/proposals/policy", b"").json();
        assert_eq!((list["live"].clone(), list["proposals"].as_array().unwrap().len()), (json!(sha256_hex(&signed_policy)), 8), "the live hash shows what remaining proposals were based on");
        let team_live = s.signed(&holder, true, "GET", "/api/admin/team", b"");
        assert_eq!(s.signed(&holder, true, "GET", "/api/admin/proposals/team", b"").json()["live"], json!(sha256_hex(&team_live.body)));
        // A session signature that does not verify, a replay, or no kid: 401 as before.
        assert_eq!(s.signed(&auditor, false, "GET", "/api/admin/status", b"").status, 401);
        let sig = format!("{} kid={}", reqauth::sign_request(&tech, "GET", "/api/admin/status", b"", now()), team::key_id(&auditor.verifying_key()));
        assert_eq!(s.req("GET", "/api/admin/status", &[(reqauth::HEADER, &sig)], b"").status, 401);
        // Recorded with the person: proposals, the refused publish, the refused other route.
        let p = s.records(&admin, "proposal-put");
        assert_eq!(p.iter().filter(|r| r["result"] == 200).count(), 12);
        assert_eq!((p[0]["actor"]["oid"].clone(), p[0]["detail"]["kind"].clone(), p[0]["detail"]["serial"].clone()), (json!(oid(7)), json!("policy"), json!(3)));
        assert!(p[0]["detail"]["id"].is_string());
        let refused: Vec<Value> = s.records(&admin, "policy-publish").into_iter().filter(|r| r["result"] == 403).collect();
        assert_eq!(refused.len(), 5);
        assert_eq!(refused[0]["actor"]["oid"], oid(5));
        let other = s.records(&admin, "admin-refused");
        assert!(other.iter().any(|r| r["detail"]["route"] == "GET /api/admin/custom" && r["actor"]["oid"] == oid(5)), "{other:?}");
        let del = s.records(&admin, "proposal-delete");
        assert_eq!(del.iter().map(|r| r["result"].as_u64().unwrap()).collect::<Vec<_>>(), [204, 403, 204]);
        // Who proposed it and whether the remover did: the author removing their own, then the key
        // holder after publishing someone else's (the publish itself is recorded under the admin key).
        assert_eq!((del[0]["detail"]["author_oid"].clone(), del[0]["detail"]["same_person"].clone(), del[0]["detail"]["id"].clone()), (json!(oid(7)), json!(true), json!(ids[1])));
        assert_eq!((del[2]["detail"]["author_oid"].clone(), del[2]["detail"]["same_person"].clone(), del[2]["detail"]["serial"].clone()), (json!(oid(7)), json!(false), json!(3)));
        let up: Vec<Value> = s.records(&admin, "release-upload").into_iter().filter(|r| r["detail"]["path"] == "stable/x/y.exe").collect();
        assert_eq!(up.len(), 5, "a session's release upload is recorded with the person: {up:?}");
        assert!(up.iter().all(|r| r["result"] == 403 && r["actor"]["oid"].is_string()), "{up:?}");
        // Manage's own exports, self-reported by any role (and the admin signature); nothing else.
        let ev = |k: &SigningKey, kid: bool, b: Value| s.signed(k, kid, "POST", "/api/admin/audit-event", b.to_string().as_bytes()).status;
        assert_eq!(ev(&auditor, true, json!({"event": "export", "what": "team-activity", "rows": 12})), 204);
        assert_eq!(ev(&admin, false, json!({"event": "export", "what": "sessions", "rows": 3})), 204);
        assert_eq!(ev(&auditor, true, json!({"event": "export", "what": "hosts", "rows": 1})), 400);
        assert_eq!(ev(&auditor, true, json!({"event": "policy-publish", "what": "devices", "rows": 1})), 400);
        assert_eq!(ev(&tech, true, json!({"event": "export", "what": "devices", "rows": 1})), 401, "a team key is no admin");
        let ex = s.records(&admin, "export");
        assert_eq!(ex.iter().map(|r| (r["actor"]["name"].as_str().unwrap().to_owned(), r["self_reported"] == true, r["detail"].clone(), r["result"].as_u64().unwrap())).collect::<Vec<_>>(), [("Manage Auditor".to_owned(), true, json!({"what": "team-activity", "rows": 12}), 204), ("admin".to_owned(), true, json!({"what": "sessions", "rows": 3}), 204)]);
        // A document that drops the Manage role takes it away at once.
        let doc2 = team_doc(&admin, 2, json!([]), json!({"oidc": oidc_of(&m), "roles": roles()}));
        assert_eq!(s.publish(&admin, &doc2).status, 204);
        assert_eq!(s.signed(&auditor, true, "GET", "/api/admin/status", b"").status, 401, "session ended: no mapped role");
    }
}

/// A production build (no debug assertions, no `test-hooks`) never trusts a provider on http or on
/// a loopback or private address: the document is refused, and a name that resolves to loopback is
/// refused when the keys are fetched. Run by scripts/test-relay.sh (`-- production_build`).
#[cfg(not(any(debug_assertions, feature = "test-hooks")))]
#[test]
fn production_build_refuses_a_loopback_or_http_issuer() {
    let m = mock();
    let admin = generate_key();
    let s = spawn_in(new_dir(), &admin, &[]);
    for bad in [m.issuer(), "http://login.example.com/t/v2.0".into(), "https://127.0.0.1/t/v2.0".into(), "https://localhost/t/v2.0".into(), "https://10.0.0.1/t/v2.0".into()] {
        let mut o = oidc_of(&m);
        o["issuer"] = json!(bad);
        let r = s.publish(&admin, &team_doc(&admin, 1, json!([]), json!({"oidc": o, "roles": roles()})));
        assert_eq!(r.status, 400, "{bad}: {}", r.text());
    }
    // A host name that resolves to ::1 (/etc/hosts): refused at the connection, never fetched.
    let mut o = oidc_of(&m);
    o["issuer"] = json!("https://ip6-localhost/t/v2.0");
    assert_eq!(s.publish(&admin, &team_doc(&admin, 1, json!([]), json!({"oidc": o, "roles": roles()}))).status, 204);
    let r = s.sign_in(&generate_key(), &m.token("technician@tether.test", None).unwrap());
    assert_eq!((r.status, r.reason()), (503, "provider-address".into()), "{}", r.text());
}

/// Memory of a real key fetch over TLS (Microsoft's own public tenant; run by hand:
/// `cargo test --release --test session -- --ignored --nocapture measure`): the relay's VmRSS and
/// VmHWM before and after one sign-in attempt that fetches discovery and keys.
#[test]
#[ignore]
fn measure_memory_of_a_tls_key_fetch() {
    let tid = "72f988bf-86f1-41af-91ab-2d7cd011db47";
    let admin = generate_key();
    let s = spawn_in(new_dir(), &admin, &[]);
    let mem = |s: &Srv| {
        let st = std::fs::read_to_string(format!("/proc/{}/status", s.child.id())).unwrap();
        let f = |k: &str| st.lines().find(|l| l.starts_with(k)).unwrap().split_whitespace().nth(1).unwrap().to_owned();
        format!("VmRSS {} kB, VmHWM {} kB", f("VmRSS:"), f("VmHWM:"))
    };
    let o = json!({"issuer": format!("https://login.microsoftonline.com/{tid}/v2.0"), "tenant": tid, "audience": "aud", "client_id": "c", "scope": "api://x/access"});
    assert_eq!(s.publish(&admin, &team_doc(&admin, 1, json!([]), json!({"oidc": o, "roles": roles()}))).status, 204);
    eprintln!("idle: {}", mem(&s));
    let b64 = |v: &Value| base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, v.to_string());
    let tok = format!("{}.{}.{}", b64(&json!({"alg": "RS256", "kid": "none-such"})), b64(&json!({"aud": "aud", "ver": "2.0"})), "AAAA");
    let t = Instant::now();
    let r = s.sign_in(&generate_key(), &tok);
    eprintln!("sign-in: {} {} after {:?}; {}", r.status, r.text(), t.elapsed(), mem(&s));
    eprintln!("{}", s.journal());
}

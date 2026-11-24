//! Team (technician) endpoints end to end: roster published with the admin key, member requests
//! signed with each technician's own key. Same harness as tests/api.rs.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

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
    fn header(&self, name: &str) -> Option<String> {
        self.head.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case(name).then(|| v.trim().to_owned())
        })
    }
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

fn start(admin_key: &SigningKey) -> Srv {
    let dir = std::env::temp_dir().join(format!("tether-api-team-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    spawn_in(dir, &key_to_b64(&admin_key.verifying_key()))
}

fn spawn_in(dir: PathBuf, admin_env: &str) -> Srv {
    let mut child = Command::new(env!("CARGO_BIN_EXE_tether-api"))
        .args(["--data", dir.to_str().unwrap(), "--listen", "127.0.0.1:0"])
        .env("ADMIN_PUB_KEY", admin_env)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let addr = line.trim().strip_prefix("listening on ").expect("listen line").to_owned();
    Srv { child, addr, dir }
}

/// Stops `s` and starts the relay again on its folder with `ADMIN_PUB_KEY=<admin_env>`.
fn restart(mut s: Srv, admin_env: &str) -> Srv {
    let _ = s.child.kill();
    let _ = s.child.wait();
    let dir = s.dir.clone();
    std::mem::forget(s); // its Drop would remove the folder
    spawn_in(dir, admin_env)
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

    /// A request signed with `key` (admin or member: same scheme), plus extra headers.
    fn signed(&self, key: &SigningKey, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Reply {
        let sig = reqauth::sign_request(key, method, path, body, now());
        let mut h = vec![(reqauth::HEADER, sig.as_str())];
        h.extend_from_slice(headers);
        self.req(method, path, &h, body)
    }

    fn get(&self, key: &SigningKey, path: &str) -> Reply {
        self.signed(key, "GET", path, &[], b"")
    }

    fn put_tags(&self, key: &SigningKey, rev: &str, body: &[u8]) -> Reply {
        self.signed(key, "PUT", "/api/team/tags", &[("If-Match", rev)], body)
    }

    fn roster(&self, admin: &SigningKey, members: &[(&str, &SigningKey, bool, bool)]) -> Reply {
        let m: Vec<Value> = members
            .iter()
            .map(|(n, k, w, a)| json!({"name": n, "key": key_to_b64(&k.verifying_key()), "write": w, "audit": a}))
            .collect();
        self.signed(admin, "PUT", "/api/admin/team", &[], json!({ "members": m }).to_string().as_bytes())
    }
}

fn hb(uuid: &str, id: &str) -> String {
    format!(r#"{{"uuid":"{uuid}","id":"{id}","profile":"technician","version":"0.5.21","os":"windows 10.0.26300","serial":3}}"#)
}

#[test]
fn roster_publish_and_validation() {
    let admin = generate_key();
    let s = start(&admin);
    let (anna, ben) = (generate_key(), generate_key());
    assert_eq!(s.get(&admin, "/api/admin/team").json(), json!({"members": []}));
    assert_eq!(s.req("GET", "/api/admin/team", &[], b"").status, 401);
    // A member key is not an admin key.
    assert_eq!(s.roster(&admin, &[("Anna", &anna, true, false)]).status, 204);
    assert_eq!(s.roster(&anna, &[("Anna", &anna, true, true)]).status, 401);
    let r = s.get(&admin, "/api/admin/team").json();
    assert_eq!(r["members"][0], json!({"name": "Anna", "key": key_to_b64(&anna.verifying_key()), "write": true, "audit": false}));

    let put = |body: String| s.signed(&admin, "PUT", "/api/admin/team", &[], body.as_bytes());
    let a = key_to_b64(&anna.verifying_key());
    let b = key_to_b64(&ben.verifying_key());
    let m = |name: &str, key: &str| json!({"name": name, "key": key, "write": false, "audit": false});
    for (bad, why) in [
        (json!({"members": [m("Anna", "AAAA")]}), "bad key"),
        (json!({"members": [m("Anna", "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHg==")]}), "not 32 bytes"),
        (json!({"members": [m("Anna", &a), m("Ben", &a)]}), "duplicate key"),
        (json!({"members": [m("", &a)]}), "empty name"),
        (json!({"members": [m("Anna", &a)], "x": 1}), "unknown field"),
        (json!({"members": [{"name": "Anna", "key": a, "write": false, "audit": false, "admin": true}]}), "unknown member field"),
    ] {
        assert_eq!(put(bad.to_string()).status, 400, "{why}");
    }
    let many: Vec<Value> = (0..501).map(|i| m(&format!("m{i}"), &key_to_b64(&generate_key().verifying_key()))).collect();
    assert_eq!(put(json!({ "members": many }).to_string()).status, 400, "too many");
    assert_eq!(put("x".repeat((64 << 10) + 1)).status, 413);
    // Nothing invalid replaced the stored roster.
    assert_eq!(s.get(&admin, "/api/admin/team").json()["members"].as_array().unwrap().len(), 1);
    assert_eq!(put(json!({"members": [m("Anna", &a), m("Ben", &b)]}).to_string()).status, 204);

    // Stored privately.
    let meta = std::fs::metadata(s.dir.join("team/roster.json")).unwrap();
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
}

#[test]
fn member_auth_me_and_devices() {
    let admin = generate_key();
    let s = start(&admin);
    let (anna, ben, eve) = (generate_key(), generate_key(), generate_key());
    // No roster yet: nobody is a member.
    let r = s.get(&anna, "/api/team/me");
    assert_eq!(r.status, 401);
    assert_eq!(s.roster(&admin, &[("Anna", &anna, false, false), ("Ben", &ben, true, true)]).status, 204);

    assert_eq!(s.get(&anna, "/api/team/me").json(), json!({"name": "Anna", "write": false, "audit": false}));
    assert_eq!(s.get(&ben, "/api/team/me").json(), json!({"name": "Ben", "write": true, "audit": true}));
    let r = s.get(&eve, "/api/team/me");
    assert_eq!(r.status, 401);
    assert_eq!(r.text(), "not a team member");
    assert_eq!(s.req("GET", "/api/team/me", &[], b"").status, 401);
    // The admin key alone is not a member key, and member keys do not open admin endpoints.
    assert_eq!(s.get(&admin, "/api/team/me").status, 401);
    assert_eq!(s.get(&anna, "/api/admin/devices").status, 401);
    assert_eq!(s.get(&anna, "/api/team/nope").status, 404);

    // Replayed nonce (shared cache), tampered path, skewed clock.
    let sig = reqauth::sign_request(&anna, "GET", "/api/team/me", b"", now());
    assert_eq!(s.req("GET", "/api/team/me", &[(reqauth::HEADER, &sig)], b"").status, 200);
    let r = s.req("GET", "/api/team/me", &[(reqauth::HEADER, &sig)], b"");
    assert_eq!(r.status, 401);
    assert!(r.text().contains("replayed"));
    let sig = reqauth::sign_request(&anna, "GET", "/api/team/me", b"", now());
    assert_eq!(s.req("GET", "/api/team/devices", &[(reqauth::HEADER, &sig)], b"").status, 401);
    let sig = reqauth::sign_request(&anna, "GET", "/api/team/me", b"", now() - 1000);
    assert_eq!(s.req("GET", "/api/team/me", &[(reqauth::HEADER, &sig)], b"").status, 401);

    // Device list without IPs or policy serials.
    let ip = [("X-Forwarded-For", "203.0.113.9")];
    assert_eq!(s.req("POST", "/api/heartbeat", &ip, hb("dev-0001", "111222333").as_bytes()).status, 200);
    let d = s.get(&anna, "/api/team/devices").json();
    assert_eq!(d.as_array().unwrap().len(), 1);
    let keys: Vec<&str> = d[0].as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(keys, ["id", "last_seen", "os", "profile", "uuid", "version"]);
    assert_eq!(d[0]["id"], "111222333");
    assert!(!String::from_utf8_lossy(&s.get(&anna, "/api/team/devices").body).contains("203.0.113.9"));

    // Removed from the roster: locked out at once. Empty roster: nobody.
    assert_eq!(s.roster(&admin, &[("Ben", &ben, true, true)]).status, 204);
    assert_eq!(s.get(&anna, "/api/team/me").status, 401);
    assert_eq!(s.get(&ben, "/api/team/me").status, 200);
    assert_eq!(s.roster(&admin, &[]).status, 204);
    assert_eq!(s.get(&ben, "/api/team/me").status, 401);
}

#[test]
fn shared_tags() {
    let admin = generate_key();
    let s = start(&admin);
    let (anna, ben) = (generate_key(), generate_key());
    assert_eq!(s.roster(&admin, &[("Anna", &anna, false, false), ("Ben", &ben, true, false)]).status, 204);

    assert_eq!(s.get(&anna, "/api/team/tags").json(), json!({"rev": 0, "doc": null, "updated_by": "", "updated_at": 0}));
    let doc = br#"{"devices":{"111222333":["Accounting"]}}"#;
    assert_eq!(s.put_tags(&anna, "0", doc).status, 403, "read-only member");
    assert_eq!(s.signed(&ben, "PUT", "/api/team/tags", &[], doc).status, 400, "no If-Match");
    assert_eq!(s.put_tags(&ben, "abc", doc).status, 400, "bad If-Match");
    assert_eq!(s.put_tags(&ben, "0", b"[1,2]").status, 400, "not an object");
    assert_eq!(s.put_tags(&ben, "0", b"{nope").status, 400, "not JSON");
    assert_eq!(s.put_tags(&ben, "0", doc).json(), json!({"rev": 1}));
    let cur = s.get(&anna, "/api/team/tags").json();
    assert_eq!((cur["rev"].clone(), cur["doc"].clone(), cur["updated_by"].clone()), (json!(1), json!({"devices": {"111222333": ["Accounting"]}}), json!("Ben")));
    assert!(cur["updated_at"].as_u64().unwrap() >= now() - 10);

    // Stale rev: 409 with the current document.
    let r = s.put_tags(&ben, "0", br#"{"devices":{}}"#);
    assert_eq!(r.status, 409);
    assert_eq!(r.json(), cur);
    assert_eq!(s.put_tags(&ben, "\"1\"", br#"{"devices":{}}"#).json(), json!({"rev": 2}), "quoted If-Match");

    let big = format!(r#"{{"x":"{}"}}"#, "a".repeat(512 << 10));
    assert_eq!(s.put_tags(&ben, "2", big.as_bytes()).status, 413);
    let meta = std::fs::metadata(s.dir.join("team/tags.json")).unwrap();
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
}

fn conn_new(conn: i64, nonce: &str, id: &str) -> String {
    format!(r#"{{"action":"new","conn_id":{conn},"id":"{id}","ip":"198.51.100.20","nonce":"{nonce}","session_id":0,"uuid":"QUJDLTEyMw=="}}"#)
}

#[test]
fn audit_needs_flag_and_device() {
    let admin = generate_key();
    let s = start(&admin);
    let (anna, ben) = (generate_key(), generate_key());
    assert_eq!(s.roster(&admin, &[("Anna", &anna, true, false), ("Ben", &ben, false, true)]).status, 204);
    let ip = [("X-Forwarded-For", "203.0.113.50")];
    for (i, id) in [(1, "111222333"), (2, "111222333"), (3, "444555666")] {
        assert_eq!(s.req("POST", "/api/audit/conn", &ip, conn_new(i, &format!("n-{i}"), id).as_bytes()).status, 204);
    }
    assert_eq!(s.get(&anna, "/api/team/audit?device=111222333").status, 403);
    assert_eq!(s.get(&ben, "/api/team/audit").status, 400, "device required");
    assert_eq!(s.get(&ben, "/api/team/audit?device=111222333&limit=501").status, 400);
    assert_eq!(s.get(&ben, "/api/team/audit?device=111222333&x=1").status, 400);
    let r = s.get(&ben, "/api/team/audit?device=111222333").json();
    assert_eq!(r.as_array().unwrap().len(), 2);
    assert!(r.as_array().unwrap().iter().all(|x| x["id"] == "111222333" && x["action"] == "connect"));
    assert_eq!(s.get(&ben, "/api/team/audit?device=111222333&limit=1").json().as_array().unwrap().len(), 1);
    assert_eq!(s.get(&ben, &format!("/api/team/audit?device=111222333&since={}", now() + 100)).json(), json!([]));
}

#[test]
fn member_rate_limit() {
    let admin = generate_key();
    let s = start(&admin);
    let (anna, ben) = (generate_key(), generate_key());
    assert_eq!(s.roster(&admin, &[("Anna", &anna, false, false), ("Ben", &ben, false, false)]).status, 204);
    for i in 0..120 {
        assert_eq!(s.get(&anna, "/api/team/me").status, 200, "request {i}");
    }
    assert_eq!(s.get(&anna, "/api/team/me").status, 429);
    assert_eq!(s.get(&ben, "/api/team/me").status, 200, "per key");

    // Failed signatures are limited per IP (each tries every roster key).
    let ip = [("X-Forwarded-For", "198.51.100.77")];
    let stranger = generate_key();
    let try_once = || {
        let sig = reqauth::sign_request(&stranger, "GET", "/api/team/me", b"", now());
        s.req("GET", "/api/team/me", &[(reqauth::HEADER, &sig), ip[0]], b"").status
    };
    for _ in 0..60 {
        assert_eq!(try_once(), 401);
    }
    assert_eq!(try_once(), 429);
}

// ---- The signed team document (`tether_trust::team`), `GET /api/team/config`, `kid`.

/// The upload of a signed team document `{serial, members, …extra}`, signed with `signer`; with
/// `oidc` in `extra` also its signed config.
fn team_doc(signer: &SigningKey, serial: u64, members: Value, extra: Value) -> Vec<u8> {
    let mut d = json!({"serial": serial, "members": members});
    for (k, v) in extra.as_object().unwrap() {
        d[k] = v.clone();
    }
    let mut up = json!({"team": envelope::sign(&serde_json::to_vec(&d).unwrap(), signer)});
    if let Some(o) = d.get("oidc") {
        up["config"] = json!(envelope::sign(&serde_json::to_vec(&json!({"serial": serial, "oidc": o})).unwrap(), signer));
    }
    serde_json::to_vec(&up).unwrap()
}

/// A member entry: name, key and the fields in `extra`.
fn entry(name: &str, key: &SigningKey, extra: Value) -> Value {
    let mut e = json!({"name": name, "key": key_to_b64(&key.verifying_key())});
    for (k, v) in extra.as_object().unwrap() {
        e[k] = v.clone();
    }
    e
}

impl Srv {
    fn publish(&self, admin: &SigningKey, body: &[u8]) -> Reply {
        self.signed(admin, "PUT", "/api/admin/team", &[], body)
    }

    /// `GET /api/team/me` signed by `signer`, the header naming key id `kid`.
    fn me_kid(&self, signer: &SigningKey, kid: &str) -> Reply {
        let h = format!("{} kid={kid}", reqauth::sign_request(signer, "GET", "/api/team/me", b"", now()));
        self.req("GET", "/api/team/me", &[(reqauth::HEADER, &h)], b"")
    }
}

fn kid(k: &SigningKey) -> String {
    team::key_id(&k.verifying_key())
}

#[test]
fn a_signed_document_replaces_the_plain_roster_for_good() {
    let admin = generate_key();
    let s = start(&admin);
    let (anna, ben) = (generate_key(), generate_key());
    assert_eq!(s.roster(&admin, &[("Anna", &anna, true, false)]).status, 204);
    let doc = team_doc(&admin, 2, json!([entry("Anna", &anna, json!({"write": true})), entry("Ben", &ben, json!({"sbom": false, "customers": ["47*"]}))]), json!({}));
    assert_eq!(s.publish(&admin, &doc).status, 204);
    assert!(!s.dir.join("team/roster.json").exists(), "the plain roster is gone");
    assert_eq!(s.get(&admin, "/api/admin/team").body, doc, "served as published");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(s.dir.join("team/team.json")).unwrap().permissions().mode() & 0o777, 0o600);
    // Missing fields read as today's rights; `me` names them all once a document is in force.
    assert_eq!(s.get(&anna, "/api/team/me").json(), json!({"name": "Anna", "write": true, "audit": false, "sbom": true, "agent": true, "customers": ["*"], "service": false}));
    assert_eq!(s.get(&ben, "/api/team/me").json(), json!({"name": "Ben", "write": false, "audit": false, "sbom": false, "agent": true, "customers": ["47*"], "service": false}));

    // The serial must rise; another key's document is not trusted (the request itself is the admin's).
    for serial in [1, 2] {
        let r = s.publish(&admin, &team_doc(&admin, serial, json!([]), json!({})));
        assert_eq!((r.status, r.text().contains("serial")), (409, true), "{serial}");
    }
    let other = generate_key();
    assert_eq!(s.publish(&admin, &team_doc(&other, 3, json!([]), json!({}))).status, 401);
    assert_eq!(s.publish(&admin, &team_doc(&admin, 3, json!([entry("Anna", &anna, json!({"customers": ["4*7"]}))]), json!({}))).status, 400);
    // A plain roster is refused from now on, also after a restart.
    let r = s.roster(&admin, &[("Anna", &anna, true, true)]);
    assert_eq!((r.status, r.text().contains("signed team document")), (409, true), "{}", r.text());
    assert_eq!(s.get(&ben, "/api/team/me").json()["sbom"], false, "nothing refused replaced it");
    let admin_env = key_to_b64(&admin.verifying_key());
    let s = restart(s, &admin_env);
    assert_eq!(s.roster(&admin, &[("Anna", &anna, true, true)]).status, 409);
    assert_eq!(s.publish(&admin, &team_doc(&admin, 2, json!([]), json!({}))).status, 409, "the serial floor survives");
    assert_eq!(s.get(&ben, "/api/team/me").json()["customers"], json!(["47*"]));

    // Admin key rotation as for the policy: the stored document stays in force under the new key
    // alone (checked against the key it names); new documents need a key in ADMIN_PUB_KEY.
    let new = generate_key();
    let s = restart(s, &format!("{},{admin_env}", key_to_b64(&new.verifying_key())));
    assert_eq!(s.publish(&new, &team_doc(&new, 3, json!([entry("Anna", &anna, json!({})), entry("Ben", &ben, json!({}))]), json!({}))).status, 204, "either key during the change");
    let s = restart(s, &key_to_b64(&new.verifying_key()));
    assert_eq!(s.get(&ben, "/api/team/me").status, 200);
    assert_eq!(s.publish(&new, &team_doc(&admin, 4, json!([]), json!({}))).status, 401, "the old key is no longer trusted");
    assert_eq!(s.publish(&new, &team_doc(&new, 4, json!([entry("Anna", &anna, json!({}))]), json!({}))).status, 204);
    assert_eq!(s.get(&ben, "/api/team/me").status, 401);
    assert_eq!(s.get(&anna, "/api/team/me").status, 200);

    // An unreadable stored document: nobody is a member, and plain rosters stay refused.
    std::fs::write(s.dir.join("team/team.json"), b"{").unwrap();
    let s = restart(s, &key_to_b64(&new.verifying_key()));
    assert_eq!(s.get(&anna, "/api/team/me").status, 401);
    assert_eq!(s.roster(&new, &[("Anna", &anna, true, true)]).status, 409);
    assert_eq!(s.publish(&new, &team_doc(&new, 5, json!([entry("Anna", &anna, json!({}))]), json!({}))).status, 204);
    assert_eq!(s.get(&anna, "/api/team/me").status, 200);
}

#[test]
fn team_config_serves_only_the_signed_oidc_part() {
    let admin = generate_key();
    let s = start(&admin);
    let anna = generate_key();
    let get = || s.req("GET", "/api/team/config", &[], b"");
    assert_eq!(get().status, 404);
    assert_eq!(s.publish(&admin, &team_doc(&admin, 1, json!([entry("Anna", &anna, json!({}))]), json!({}))).status, 204);
    assert_eq!(get().status, 404, "no oidc, nothing to sign in with");
    let oidc = json!({"issuer": "https://login.example.com/t/v2.0", "tenant": "t", "audience": "api://tether", "client_id": "c", "scope": "api://tether/access"});
    let extra = json!({"oidc": oidc, "deny": ["oid-1"], "oidc_only": false, "roles": {"Tether.Technician": {}}});
    assert_eq!(s.publish(&admin, &team_doc(&admin, 2, json!([entry("Anna", &anna, json!({}))]), extra)).status, 204);
    let r = get();
    assert_eq!((r.status, r.header("ETag")), (200, Some("\"2\"".into())));
    let env: envelope::Signed = serde_json::from_slice(&r.body).unwrap();
    // Verified with the admin key alone, not trusting the relay; members, roles and deny are not in it.
    let cfg: Value = serde_json::from_slice(&envelope::open(&env, &[admin.verifying_key()]).unwrap()).unwrap();
    assert_eq!(cfg, json!({"serial": 2, "oidc": oidc}));
    assert_eq!(s.req("GET", "/api/team/config", &[("If-None-Match", "\"2\"")], b"").status, 304);
    // Members still work with their keys (`deny` names people who sign in, not keys).
    assert_eq!(s.get(&anna, "/api/team/me").status, 200);
    // A document with oidc but without its signed config is refused.
    let mut up: Value = serde_json::from_slice(&team_doc(&admin, 3, json!([]), json!({"oidc": oidc}))).unwrap();
    up.as_object_mut().unwrap().remove("config");
    assert_eq!(s.publish(&admin, up.to_string().as_bytes()).status, 400);
}

#[test]
fn kid_picks_one_key() {
    let admin = generate_key();
    let s = start(&admin);
    let (anna, ben, svc) = (generate_key(), generate_key(), generate_key());
    // Also with the plain roster.
    assert_eq!(s.roster(&admin, &[("Anna", &anna, false, false), ("Ben", &ben, false, false)]).status, 204);
    assert_eq!(s.me_kid(&anna, &kid(&anna)).json()["name"], "Anna");
    // Another member's kid on Anna's signature: refused, the other keys are not tried.
    for k in [kid(&ben), "0123456789abcdef".into()] {
        let r = s.me_kid(&anna, &k);
        assert_eq!((r.status, r.text()), (401, "not a team member".into()), "{k}");
    }
    assert_eq!(s.me_kid(&anna, "0123").status, 401, "malformed");
    assert_eq!(s.me_kid(&anna, &kid(&anna).to_uppercase()).status, 401, "lower case only");
    assert_eq!(s.get(&anna, "/api/team/me").status, 200, "no kid: every key is tried");

    // oidc_only: keys only for service entries, with kid or without; a removed member's kid.
    let doc = team_doc(&admin, 1, json!([entry("Anna", &anna, json!({})), entry("Svc", &svc, json!({"service": true}))]), json!({"oidc_only": true}));
    assert_eq!(s.publish(&admin, &doc).status, 204);
    assert_eq!(s.me_kid(&anna, &kid(&anna)).status, 401);
    assert_eq!(s.get(&anna, "/api/team/me").status, 401);
    assert_eq!(s.me_kid(&svc, &kid(&svc)).json()["service"], true);
    assert_eq!(s.get(&svc, "/api/team/me").status, 200);
    assert_eq!(s.me_kid(&ben, &kid(&ben)).status, 401);
}

#[test]
fn member_lookup_cost_with_2000_keys() {
    let admin = generate_key();
    let s = start(&admin);
    let last = generate_key();
    let mut members: Vec<Value> = (0..1999).map(|i| entry(&format!("m{i}"), &generate_key(), json!({}))).collect();
    members.push(entry("Last", &last, json!({})));
    assert_eq!(s.publish(&admin, &team_doc(&admin, 1, json!(members), json!({}))).status, 204);
    // Few requests: a debug build verifies a signature in about 10 ms (release: about 80 µs).
    let time = |with_kid: bool, n: u32| {
        let t = std::time::Instant::now();
        for _ in 0..n {
            let r = if with_kid { s.me_kid(&last, &kid(&last)) } else { s.get(&last, "/api/team/me") };
            assert_eq!(r.status, 200);
        }
        t.elapsed() / n
    };
    let (loop_t, kid_t) = (time(false, 1), time(true, 5));
    eprintln!("member lookup over 2000 keys: {loop_t:?} per request without kid, {kid_t:?} with kid");
    assert!(kid_t < loop_t);
}

//! Team (technician) endpoints end to end: roster published with the admin key, member requests
//! signed with each technician's own key. Same harness as tests/api.rs.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tether_trust::{generate_key, key_to_b64, reqauth, SigningKey};

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
}

impl Reply {
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
    let mut child = Command::new(env!("CARGO_BIN_EXE_tether-api"))
        .args(["--data", dir.to_str().unwrap(), "--listen", "127.0.0.1:0"])
        .env("ADMIN_PUB_KEY", key_to_b64(&admin_key.verifying_key()))
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let addr = line.trim().strip_prefix("listening on ").expect("listen line").to_owned();
    Srv { child, addr, dir }
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
        Reply { status, body: raw[split + 4..].to_vec() }
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

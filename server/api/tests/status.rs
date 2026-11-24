//! Device status end to end: enrollment with an admin token, device-signed reports and heartbeats,
//! the team summary and per-host report, removal of a pin. Same harness as tests/team.rs.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tether_trust::{generate_key, key_to_b64, reqauth, status, SigningKey};

static SEQ: AtomicU32 = AtomicU32::new(0);
const RELAY: &str = "https://relay.test";

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

struct Srv {
    child: Child,
    addr: String,
    dir: PathBuf,
    /// The relay's stderr (its journal), appended to across a restart.
    err: PathBuf,
}

impl Drop for Srv {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
        let _ = std::fs::remove_file(&self.err);
    }
}

struct Reply {
    status: u16,
    body: Vec<u8>,
    head: String,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    fn header(&self, name: &str) -> Option<String> {
        self.head.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case(name).then(|| v.trim().to_owned())
        })
    }
}

fn new_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tether-api-status-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn start_in(dir: PathBuf, admin: &SigningKey) -> Srv {
    spawn(dir, &key_to_b64(&admin.verifying_key()))
}

/// The relay with `ADMIN_PUB_KEY=<admin_env>` (empty: no admin keys).
fn spawn(dir: PathBuf, admin_env: &str) -> Srv {
    spawn_env(dir, admin_env, &[])
}

fn spawn_env(dir: PathBuf, admin_env: &str, env: &[(&str, &str)]) -> Srv {
    let err = dir.with_extension("err");
    let errf = std::fs::OpenOptions::new().create(true).append(true).open(&err).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tether-api"))
        .args(["--data", dir.to_str().unwrap(), "--listen", "127.0.0.1:0"])
        .env("ADMIN_PUB_KEY", admin_env)
        .env("RELAY_HOST", "relay.test")
        .envs(env.iter().copied())
        .stdout(Stdio::piped())
        .stderr(errf)
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let addr = line.trim().strip_prefix("listening on ").expect("listen line").to_owned();
    Srv { child, addr, dir, err }
}

fn server() -> (Srv, SigningKey) {
    let admin = generate_key();
    (start_in(new_dir(), &admin), admin)
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

    /// Admin (or member) v1 signature.
    fn admin(&self, key: &SigningKey, method: &str, path: &str, body: &[u8]) -> Reply {
        let sig = reqauth::sign_request(key, method, path, body, now());
        self.req(method, path, &[(reqauth::HEADER, &sig)], body)
    }
}

/// A device request signed with `dev` for `uuid`.
fn signed(s: &Srv, dev: &SigningKey, uuid: &str, method: &str, path: &str, body: &[u8]) -> Reply {
    let sig = reqauth::sign_device_request(dev, uuid, RELAY, method, path, body, now());
    s.req(method, path, &[(reqauth::HEADER, &sig)], body)
}

fn enroll(s: &Srv, dev: &SigningKey, uuid: &str, tether_id: Option<&str>, token: &str) -> Reply {
    let mut b = json!({"uuid": uuid, "pubkey": key_to_b64(&dev.verifying_key()), "token": token});
    if let Some(t) = tether_id {
        b["tether_id"] = t.into();
    }
    signed(s, dev, uuid, "POST", "/api/status/enroll", b.to_string().as_bytes())
}

fn post(s: &Srv, dev: &SigningKey, uuid: &str, body: &str) -> Reply {
    signed(s, dev, uuid, "POST", "/api/status", body.as_bytes())
}

/// A full report; `body` keys deliberately out of alphabetical order (stored byte-exact).
fn full(uuid: &str, seq: u64, overall: &str) -> String {
    format!(
        r#"{{"v":1,"uuid":"{uuid}","tether_id":"123456789","host":"PC-1","seq":{seq},"time":"2026-11-23T08:15:00Z","next":300,"agent":"0.1.0","packs":{{"system":9}},"overall":"{overall}","counts":{{"ok":31,"warn":0,"crit":1,"unknown":0,"setup":0}},"faults":[{{"id":"app/db-service","state":"crit","since":"2026-11-23T07:55:00Z"}}],"products":[{{"name":"Example App","version":"1.2","role":"server"}}],"changes":[{{"id":"app/db-service","from":"ok","to":"crit","time":"2026-11-23T07:55:00Z"}}],"body":{{"zeta":1,"alpha":{{"y":2,"b":3}},  "checks":[]}}}}"#
    )
}

fn heartbeat(uuid: &str, seq: u64) -> String {
    format!(
        r#"{{"v":1,"uuid":"{uuid}","host":"PC-1","seq":{seq},"time":"2026-11-23T08:20:00Z","next":300,"agent":"0.1.0","packs":{{}},"overall":"crit","counts":{{"ok":31,"warn":0,"crit":1,"unknown":0,"setup":0}},"faults":[]}}"#
    )
}

fn update_json(p: &Path, f: impl FnOnce(&mut serde_json::Map<String, Value>)) {
    let mut m: serde_json::Map<String, Value> = std::fs::read(p).ok().map(|b| serde_json::from_slice(&b).unwrap()).unwrap_or_default();
    f(&mut m);
    std::fs::write(p, serde_json::to_vec(&m).unwrap()).unwrap();
}

/// Pins `uuid` to `key` and stores a full report seen at `last_seen`, before the relay starts.
fn prep_device(dir: &Path, uuid: &str, key: &SigningKey, last_seen: u64, seq: u64) {
    let st = dir.join("status");
    std::fs::create_dir_all(&st).unwrap();
    update_json(&st.join("keys.json"), |m| {
        m.insert(uuid.into(), json!({"key": key_to_b64(&key.verifying_key()), "tether_id": null, "label": "t", "enrolled": last_seen}));
    });
    let entry = json!({"host": "PC-1", "agent": "0.1.0", "overall": "crit", "counts": {"ok":31,"warn":0,"crit":1,"unknown":0,"setup":0}, "products": [], "faults": []});
    update_json(&st.join("seen.json"), |m| {
        m.insert(uuid.into(), json!({"last_seen": last_seen, "seq": seq, "next": 300, "entry": entry.to_string()}));
    });
    std::fs::write(st.join(format!("{uuid}.json")), full(uuid, seq, "crit")).unwrap();
}

/// A roster member (published with the admin key).
fn member(s: &Srv, admin: &SigningKey) -> SigningKey {
    let m = generate_key();
    let roster = json!({"members": [{"name": "M", "key": key_to_b64(&m.verifying_key()), "write": false, "audit": false}]});
    assert_eq!(s.admin(admin, "PUT", "/api/admin/team", roster.to_string().as_bytes()).status, 204);
    m
}

fn team_get(s: &Srv, m: &SigningKey, path: &str) -> Reply {
    s.admin(m, "GET", path, b"")
}

#[test]
fn enroll_pins_uuid_and_tether_id() {
    let (s, admin) = server();
    // A token issued for 123456789 pins that ID; any other token only lets a device claim one.
    let bound = |id: &str| status::make_token("t", now() + 3600, Some(id), None, false, &admin).unwrap().0;
    let tok = bound("123456789");
    let any = status::make_token("t", now() + 3600, None, None, false, &admin).unwrap().0;
    let (a, b) = (generate_key(), generate_key());
    assert_eq!(enroll(&s, &a, "dev-0001", Some("123456789"), &tok).status, 200);
    let r = enroll(&s, &a, "dev-0001", Some("123456789"), &tok);
    assert_eq!(r.status, 200, "idempotent");
    assert_eq!(r.json(), json!({"releases": 0, "next": 300}));
    let r = enroll(&s, &b, "dev-0001", None, &any);
    assert_eq!((r.status, r.text()), (409, "uuid is enrolled with another key".into()));
    let r = enroll(&s, &b, "dev-0002", Some("123456789"), &bound("123456789"));
    assert_eq!((r.status, r.text()), (409, "tether_id is enrolled with another key".into()));
    assert_eq!(enroll(&s, &b, "dev-0003", None, &status::make_token("t", now() - 1, None, None, false, &admin).unwrap().0).status, 401);
    let r = enroll(&s, &b, "dev-0003", None, &status::make_token("t", now() + 60, None, None, false, &generate_key()).unwrap().0);
    assert_eq!((r.status, r.text()), (401, "enrollment token: signed by an untrusted key".into()));
    // A report cannot move the pin; the summary shows the pinned ID.
    let r = post(&s, &a, "dev-0001", &full("dev-0001", 1, "crit").replace("123456789", "999999999"));
    assert_eq!((r.status, r.text()), (409, "tether_id is pinned to another value".into()));
    assert_eq!(post(&s, &a, "dev-0001", &full("dev-0001", 2, "crit")).status, 200);
    let m = member(&s, &admin);
    let sum = team_get(&s, &m, "/api/team/status").json();
    assert_eq!(sum["hosts"][0]["tether_id"], "123456789");
    // Self-release, then a new key may enrol (a token issued for one ID stays used); the admin
    // route removes any pin.
    assert_eq!(signed(&s, &a, "dev-0001", "DELETE", "/api/status", b"").status, 204);
    assert!(!s.dir.join("status/dev-0001.json").exists());
    assert_eq!(enroll(&s, &b, "dev-0001", Some("123456789"), &tok).text(), "token already used");
    assert_eq!(enroll(&s, &b, "dev-0001", Some("123456789"), &bound("123456789")).status, 200);
    assert_eq!(s.admin(&admin, "DELETE", "/api/admin/status/dev-0001", b"").status, 204);
    assert_eq!(s.admin(&admin, "DELETE", "/api/admin/status/dev-0001", b"").status, 404);
    // Gone at once from the per-host route; the cached summary drops it within 30 s.
    assert_eq!(team_get(&s, &m, "/api/team/status/dev-0001").status, 404);
}

#[test]
fn malformed_uuids_and_odd_tether_ids_never_reach_a_path() {
    let (s, admin) = server();
    let tok = status::make_token("t", now() + 3600, None, None, false, &admin).unwrap().0;
    let a = generate_key();
    for bad in ["../../etc", "dev_0001", "short", "dev-0001/../x"] {
        let r = enroll(&s, &a, bad, None, &tok);
        assert_eq!((r.status, r.text()), (400, "bad uuid".into()), "{bad}");
        let r = post(&s, &a, bad, &heartbeat("dev-0001", 1));
        assert_eq!((r.status, r.text()), (401, "malformed signature header".into()), "{bad}");
        assert_eq!(s.admin(&admin, "DELETE", &format!("/api/admin/status/{bad}"), b"").status, 404);
    }
    // A tether_id with path characters is only a map key and a stored value.
    let odd = "../..\\x/y";
    let tok = status::make_token("t", now() + 3600, Some(odd), None, false, &admin).unwrap().0;
    assert_eq!(enroll(&s, &a, "dev-0001", Some(odd), &tok).status, 200);
    assert_eq!(post(&s, &a, "dev-0001", &full("dev-0001", 1, "ok").replace("123456789", odd.replace('\\', "\\\\").as_str())).status, 200);
    let mut names: Vec<String> = std::fs::read_dir(s.dir.join("status")).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    names.sort();
    assert_eq!(names, ["dev-0001.history.jsonl", "dev-0001.json", "keys.jsonl"]);
    let m = member(&s, &admin);
    assert_eq!(team_get(&s, &m, "/api/team/status").json()["hosts"][0]["tether_id"], odd);
    let r = enroll(&s, &generate_key(), "dev-0002", Some(odd), &status::make_token("t", now() + 3600, Some(odd), None, false, &admin).unwrap().0);
    assert_eq!((r.status, r.text()), (409, "tether_id is enrolled with another key".into()));
}

#[test]
fn reports_heartbeats_seq_and_history() {
    let (s, admin) = server();
    let tok = status::make_token("t", now() + 3600, None, None, false, &admin).unwrap().0;
    let a = generate_key();
    assert_eq!(enroll(&s, &a, "dev-0001", Some("123456789"), &tok).status, 200);
    let r = post(&s, &a, "dev-0001", &heartbeat("dev-0001", 1));
    assert_eq!((r.status, r.text()), (409, "send a full report".into()));
    let rep = full("dev-0001", 5, "crit");
    let r = post(&s, &a, "dev-0001", &rep);
    assert_eq!((r.status, r.json()), (200, json!({"releases": 0, "next": 300})));
    assert_eq!(std::fs::read(s.dir.join("status/dev-0001.json")).unwrap(), rep.as_bytes(), "stored byte-exact");
    // Next posts within 60 s are refused by the accepted-post limit: test the rest on a prepped device.
    let r = post(&s, &a, "dev-0001", &heartbeat("dev-0001", 6));
    assert_eq!((r.status, r.text()), (429, "report too frequent".into()));
    drop(s);

    let dir = new_dir();
    let b = generate_key();
    prep_device(&dir, "dev-0002", &b, now() - 61, 5);
    let before = std::fs::read(dir.join("status/dev-0002.json")).unwrap();
    let s = start_in(dir, &admin);
    let r = post(&s, &b, "dev-0002", &heartbeat("dev-0002", 5));
    assert_eq!((r.status, r.text()), (409, "seq must increase".into()));
    assert_eq!(post(&s, &b, "dev-0002", &heartbeat("dev-0002", 6)).status, 200);
    assert_eq!(std::fs::read(s.dir.join("status/dev-0002.json")).unwrap(), before, "heartbeat leaves the report");

    let m = member(&s, &admin);
    let r = team_get(&s, &m, "/api/team/status/dev-0002");
    assert_eq!(r.status, 200);
    // The report comes back as the bytes sent (body keys not reordered).
    let raw = r.text();
    assert!(raw.contains(r#""report":{"v":1,"uuid":"dev-0002""#) && raw.contains(r#""body":{"zeta":1,"alpha":{"y":2,"b":3}"#), "{raw}");
    let v = r.json();
    assert_eq!(v["uuid"], "dev-0002");
    assert_eq!(v["history"], json!([]));
    assert_eq!(v["stale"], false);
    assert_eq!(team_get(&s, &m, "/api/team/status/dev-9999").status, 404);
    assert_eq!(team_get(&s, &m, "/api/team/status/..%2f..").status, 404);

    // Refusals on another prepped device: none counts as accepted.
    drop(s);
    let dir = new_dir();
    prep_device(&dir, "dev-0003", &b, now() - 61, 5);
    let s = start_in(dir, &admin);
    let r = post(&s, &b, "dev-0003", &full("dev-0004", 6, "ok"));
    assert_eq!(r.status, 400, "uuid in body differs from the header: {}", r.text());
    assert_eq!(post(&s, &b, "dev-0003", &full("dev-0003", 6, "ok").replace("\"v\":1", "\"v\":1,\"x\":1")).status, 400);
    let r = post(&s, &b, "dev-0003", &heartbeat("dev-0003", 6).replace("\"faults\"", "\"body\":{},\"faults\""));
    assert_eq!(r.status, 400, "part of a full report");
    assert_eq!(post(&s, &b, "dev-0003", &"x".repeat((64 << 10) + 1)).status, 413);
    let r = post(&s, &b, "dev-0003", &full("dev-0003", 6, "ok").replace(r#"{"zeta":1,"alpha":{"y":2,"b":3},  "checks":[]}"#, "[1]"));
    assert_eq!(r.status, 400, "body not an object");
    // Pinned without a Tether ID: the report carries none either.
    let rep = full("dev-0003", 6, "warn").replace(r#""tether_id":"123456789","#, "");
    assert_eq!(post(&s, &b, "dev-0003", &rep).status, 200);
    let v = team_get(&s, &member(&s, &admin), "/api/team/status/dev-0003").json();
    let h = v["history"].as_array().unwrap();
    assert_eq!(h.len(), 1);
    assert_eq!((h[0]["id"].clone(), h[0]["from"].clone(), h[0]["to"].clone()), (json!("app/db-service"), json!("ok"), json!("crit")));
    assert!(h[0]["at"].as_u64().unwrap() >= now() - 10);
    assert_eq!(v["report"], serde_json::from_str::<Value>(&rep).unwrap());
    assert_eq!(v["last_seen"].as_u64().unwrap() / 10, now() / 10);
}

#[test]
fn device_and_member_signatures_stay_apart() {
    let (s, admin) = server();
    let tok = status::make_token("t", now() + 3600, None, None, false, &admin).unwrap().0;
    let a = generate_key();
    assert_eq!(enroll(&s, &a, "dev-0001", Some("123456789"), &tok).status, 200);
    let m = member(&s, &admin);
    let body = full("dev-0001", 1, "ok");
    let sig = reqauth::sign_request_for(&m, RELAY, "POST", "/api/status", body.as_bytes(), now());
    let r = s.req("POST", "/api/status", &[(reqauth::HEADER, &sig)], body.as_bytes());
    assert_eq!((r.status, r.text()), (401, "unsupported signature version".into()));
    let r = s.req("POST", "/api/status", &[], body.as_bytes());
    assert_eq!((r.status, r.text()), (401, "missing signature".into()));
    // The device key opens neither member nor admin routes.
    for p in ["/api/team/status", "/api/admin/status"] {
        let sig = reqauth::sign_device_request(&a, "dev-0001", RELAY, "GET", p, b"", now());
        assert_eq!(s.req("GET", p, &[(reqauth::HEADER, &sig)], b"").status, 401, "{p}");
    }
    // Another key with the device's uuid; unknown uuid; an accepted post sent again (the
    // per-uuid limit answers before the signature is looked at).
    let r = post(&s, &generate_key(), "dev-0001", &body);
    assert_eq!((r.status, r.text()), (401, "signature does not match".into()));
    let r = post(&s, &a, "dev-0009", &body);
    assert_eq!((r.status, r.text()), (401, "unknown device".into()));
    let sig = reqauth::sign_device_request(&a, "dev-0001", RELAY, "POST", "/api/status", body.as_bytes(), now());
    assert_eq!(s.req("POST", "/api/status", &[(reqauth::HEADER, &sig)], body.as_bytes()).status, 200);
    let r = s.req("POST", "/api/status", &[(reqauth::HEADER, &sig)], body.as_bytes());
    assert_eq!(r.status, 429, "accepted-post limit first: {}", r.text());
}

#[test]
fn stale_is_computed_on_read() {
    let admin = generate_key();
    let dir = new_dir();
    let (a, b) = (generate_key(), generate_key());
    prep_device(&dir, "dev-0001", &a, now() - 901, 5);
    prep_device(&dir, "dev-0002", &b, now() - 899, 5);
    let s = start_in(dir, &admin);
    let m = member(&s, &admin);
    let r = team_get(&s, &m, "/api/team/status");
    assert_eq!(r.status, 200);
    let etag = r.header("ETag").unwrap();
    assert_eq!(etag.len(), 18, "{etag}");
    let v = r.json();
    let hosts = v["hosts"].as_array().unwrap();
    assert_eq!(hosts.len(), 2);
    assert_eq!((hosts[0]["uuid"].clone(), hosts[0]["stale"].clone()), (json!("dev-0001"), json!(true)));
    assert_eq!((hosts[1]["uuid"].clone(), hosts[1]["stale"].clone()), (json!("dev-0002"), json!(false)));
    assert_eq!((hosts[0]["overall"].clone(), hosts[0]["next"].clone(), hosts[0]["host"].clone()), (json!("crit"), json!(300), json!("PC-1")));
    let sig = reqauth::sign_request(&m, "GET", "/api/team/status", b"", now());
    let r = s.req("GET", "/api/team/status", &[(reqauth::HEADER, &sig), ("If-None-Match", &etag)], b"");
    assert_eq!(r.status, 304);
}

#[test]
fn unreadable_keys_lock_the_status_store_only() {
    let admin = generate_key();
    let dir = new_dir();
    std::fs::create_dir_all(dir.join("status")).unwrap();
    let corrupt = b"{\"dev-0001\": {\"key\": tru".to_vec();
    std::fs::write(dir.join("status/keys.json"), &corrupt).unwrap();
    let s = start_in(dir, &admin);
    let tok = status::make_token("t", now() + 3600, None, None, false, &admin).unwrap().0;
    let a = generate_key();
    let locked = |r: Reply| {
        assert_eq!((r.status, r.text()), (503, "status store unavailable".into()));
        assert_eq!(r.header("Retry-After").as_deref(), Some("3600"));
    };
    locked(enroll(&s, &a, "dev-0001", None, &tok));
    locked(post(&s, &a, "dev-0001", &full("dev-0001", 1, "ok")));
    locked(signed(&s, &a, "dev-0001", "DELETE", "/api/status", b""));
    locked(s.admin(&admin, "DELETE", "/api/admin/status/dev-0001", b""));
    let m = member(&s, &admin);
    locked(team_get(&s, &m, "/api/team/status"));
    locked(team_get(&s, &m, "/api/team/status/dev-0001"));
    assert_eq!(std::fs::read(s.dir.join("status/keys.json")).unwrap(), corrupt, "untouched");
    let names: Vec<_> = std::fs::read_dir(s.dir.join("status")).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(names, ["keys.json"]);
    // The rest of the relay works.
    assert_eq!(s.req("GET", "/api/policy", &[], b"").text(), "no policy published");
    let hb = r#"{"uuid":"dev-0001","id":"111222333","profile":"technician","version":"0.5.21","os":"windows 10.0.26300","serial":3}"#;
    assert_eq!(s.req("POST", "/api/heartbeat", &[], hb.as_bytes()).status, 200);
}

#[test]
fn enrollment_limits_and_tether_id_rules() {
    // No admin keys: no enrollment.
    let s = spawn(new_dir(), "");
    let r = enroll(&s, &generate_key(), "dev-0001", None, "x");
    assert_eq!((r.status, r.text()), (503, "status enrollment needs ADMIN_PUB_KEY".into()));
    drop(s);

    let (s, admin) = server();
    let tok = status::make_token("t", now() + 3600, None, None, false, &admin).unwrap().0;
    let a = generate_key();
    // Idempotent re-enrollment adds a claim once (not a Tether ID); another one is refused.
    assert_eq!(enroll(&s, &a, "dev-0001", None, &tok).status, 200);
    assert_eq!(enroll(&s, &a, "dev-0001", Some("123456789"), &tok).status, 200);
    let m = member(&s, &admin);
    let h = team_get(&s, &m, "/api/team/status/dev-0001").json();
    assert_eq!((&h["tether_id"], &h["claimed_id"]), (&Value::Null, &json!("123456789")));
    let r = enroll(&s, &a, "dev-0001", Some("555"), &tok);
    assert_eq!((r.status, r.text()), (409, "tether_id is pinned to another value".into()));

    // 30 refused enrolments from one IP (refused tokens, and unsigned ones, which count the same),
    // then new enrollments from it wait; the same key again is free.
    let ip = "203.0.113.20";
    let from_ip = |dev: &SigningKey, uuid: &str, token: &str| {
        let b = json!({"uuid": uuid, "pubkey": key_to_b64(&dev.verifying_key()), "token": token}).to_string();
        let sig = reqauth::sign_device_request(dev, uuid, RELAY, "POST", "/api/status/enroll", b.as_bytes(), now());
        s.req("POST", "/api/status/enroll", &[("X-Forwarded-For", ip), (reqauth::HEADER, &sig)], b.as_bytes())
    };
    let expired = status::make_token("t", now() - 1, None, None, false, &admin).unwrap().0;
    for i in 0..30 {
        let uuid = format!("dev-1{i:03}");
        let r = if i % 2 == 0 {
            from_ip(&a, &uuid, &expired)
        } else {
            let b = json!({"uuid": uuid, "pubkey": key_to_b64(&a.verifying_key()), "token": tok});
            s.req("POST", "/api/status/enroll", &[("X-Forwarded-For", ip)], b.to_string().as_bytes())
        };
        assert_eq!(r.status, 401, "attempt {i}");
    }
    let r = from_ip(&a, "dev-2000", &tok);
    assert_eq!((r.status, r.text(), r.header("Retry-After")), (429, "too many requests".into(), Some("60".into())));
    assert_eq!(from_ip(&a, "dev-0001", &tok).status, 200, "same uuid, same key");
    assert_eq!(enroll(&s, &a, "dev-2000", None, &tok).status, 200, "other IPs unaffected");
}

#[test]
fn refused_and_failed_posts_are_limited() {
    let admin = generate_key();
    let dir = new_dir();
    let a = generate_key();
    prep_device(&dir, "dev-0001", &a, now() - 61, 5);
    let s = start_in(dir, &admin);
    // Not UTF-8 inside a body string.
    let mut bad = full("dev-0001", 6, "ok").replace(r#""tether_id":"123456789","#, "").replace(r#""zeta":1"#, r#""zeta":"?""#).into_bytes();
    let q = bad.iter().position(|&b| b == b'?').unwrap();
    bad[q] = 0xFF;
    let r = signed(&s, &a, "dev-0001", "POST", "/api/status", &bad);
    assert_eq!((r.status, r.text()), (400, "report is not UTF-8".into()));
    // Correctly signed but refused posts: 10 per uuid and minute.
    for i in 0..9 {
        assert_eq!(post(&s, &a, "dev-0001", &heartbeat("dev-0001", 5)).status, 409, "attempt {i}");
    }
    let r = post(&s, &a, "dev-0001", &heartbeat("dev-0001", 7));
    assert_eq!((r.status, r.text(), r.header("Retry-After")), (429, "too many refused reports".into(), Some("60".into())));

    // Failed signatures: 60 per IP and minute.
    let ip = [("X-Forwarded-For", "203.0.113.9")];
    let stranger = generate_key();
    let forged = || {
        let body = heartbeat("dev-0009", 1);
        let sig = reqauth::sign_device_request(&stranger, "dev-0009", RELAY, "POST", "/api/status", body.as_bytes(), now());
        s.req("POST", "/api/status", &[(reqauth::HEADER, &sig), ip[0]], body.as_bytes())
    };
    for _ in 0..60 {
        assert_eq!(forged().text(), "unknown device");
    }
    let r = forged();
    assert_eq!((r.status, r.text()), (429, "too many failed requests".into()));
}

#[test]
fn device_delete_is_not_replayable() {
    let (s, admin) = server();
    let tok = status::make_token("t", now() + 3600, None, None, false, &admin).unwrap().0;
    let a = generate_key();
    assert_eq!(enroll(&s, &a, "dev-0001", None, &tok).status, 200);
    let sig = reqauth::sign_device_request(&a, "dev-0001", RELAY, "DELETE", "/api/status", b"", now());
    assert_eq!(s.req("DELETE", "/api/status", &[(reqauth::HEADER, &sig)], b"").status, 204);
    assert_eq!(enroll(&s, &a, "dev-0001", None, &tok).status, 200);
    let r = s.req("DELETE", "/api/status", &[(reqauth::HEADER, &sig)], b"");
    assert_eq!((r.status, r.text()), (401, "replayed request".into()));
}

/// The pins `keys.jsonl` holds, replayed.
fn live_pins(dir: &Path) -> Vec<String> {
    let mut pins = std::collections::BTreeSet::new();
    for l in std::fs::read_to_string(dir.join("status/keys.jsonl")).unwrap().lines() {
        let v: Value = serde_json::from_str(l).unwrap();
        let u = v["uuid"].as_str().unwrap().to_owned();
        match v["op"].as_str() {
            Some("pin" | "set") => pins.insert(u),
            Some("unpin") => pins.remove(&u),
            _ => false, // `used`
        };
    }
    pins.into_iter().collect()
}

/// Background work: polls for up to 10 s.
fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..200 {
        if done() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("timed out waiting for {what}");
}

#[test]
fn keys_log_survives_a_torn_line_and_locks_on_a_bad_one() {
    let admin = generate_key();
    let a = generate_key();
    let pin = |u: &str| json!({"op": "pin", "uuid": u, "key": key_to_b64(&a.verifying_key()), "tether_id": null, "label": "t", "enrolled": now()}).to_string();
    // A torn last line (a crash during an append) is dropped and cut off; the rest loads.
    let dir = new_dir();
    std::fs::create_dir_all(dir.join("status")).unwrap();
    let good = format!("{}\n{}\n{}\n", pin("dev-0001"), json!({"op": "unpin", "uuid": "dev-0001"}), pin("dev-0003"));
    std::fs::write(dir.join("status/keys.jsonl"), format!("{good}{}", &pin("dev-0002")[..20])).unwrap();
    let s = start_in(dir, &admin);
    assert_eq!(std::fs::read_to_string(s.dir.join("status/keys.jsonl")).unwrap(), good);
    let tok = status::make_token("t", now() + 3600, None, None, false, &admin).unwrap().0;
    assert_eq!(enroll(&s, &a, "dev-0003", None, &tok).status, 200, "pinned: the same key again");
    assert_eq!(enroll(&s, &a, "dev-0001", None, &tok).status, 200);
    assert_eq!(live_pins(&s.dir), ["dev-0001", "dev-0003"]);
    drop(s);

    // Any other unreadable line: locked, nothing written.
    let dir = new_dir();
    std::fs::create_dir_all(dir.join("status")).unwrap();
    let bad = format!("{}\nnot json\n{}\n", pin("dev-0001"), pin("dev-0003"));
    std::fs::write(dir.join("status/keys.jsonl"), &bad).unwrap();
    let s = start_in(dir, &admin);
    let r = enroll(&s, &a, "dev-0004", None, &tok);
    assert_eq!((r.status, r.text()), (503, "status store unavailable".into()));
    assert_eq!(std::fs::read_to_string(s.dir.join("status/keys.jsonl")).unwrap(), bad);
    let names: Vec<_> = std::fs::read_dir(s.dir.join("status")).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(names, ["keys.jsonl"]);
}

#[test]
fn summary_rebuilds_after_30_s_and_keeps_an_unchanged_etag() {
    let admin = generate_key();
    let dir = new_dir();
    let (a, b) = (generate_key(), generate_key());
    prep_device(&dir, "dev-0001", &a, now() - 61, 5);
    prep_device(&dir, "dev-0002", &b, now() - 61, 5);
    let s = start_in(dir, &admin);
    let m = member(&s, &admin);
    let get = || {
        let r = team_get(&s, &m, "/api/team/status");
        assert_eq!(r.status, 200);
        (r.header("ETag").unwrap(), r.json())
    };
    let (e1, v1) = get();
    let rep = full("dev-0002", 6, "warn").replace(r#""tether_id":"123456789","#, "");
    assert_eq!(post(&s, &b, "dev-0002", &rep).status, 200);
    assert_eq!(get().0, e1, "cached");
    std::thread::sleep(std::time::Duration::from_secs(31));
    let (e2, v2) = get();
    assert_ne!(e2, e1);
    assert_eq!(v2["hosts"][1]["overall"], "warn");
    assert!(v2["built"].as_u64() > v1["built"].as_u64());
    // Rebuilt again with nothing changed: the same ETag.
    std::thread::sleep(std::time::Duration::from_secs(31));
    let (e3, v3) = get();
    assert!(v3["built"].as_u64() > v2["built"].as_u64(), "rebuilt");
    assert_eq!(e3, e2);
}

#[test]
fn removals_do_not_rebuild_the_summary_and_the_keys_log_is_compacted() {
    let (s, admin) = server();
    let tok = status::make_token("t", now() + 3600, None, None, false, &admin).unwrap().0;
    let a = generate_key();
    assert_eq!(enroll(&s, &a, "dev-0001", None, &tok).status, 200);
    assert_eq!(post(&s, &a, "dev-0001", &full("dev-0001", 1, "ok").replace(r#""tether_id":"123456789","#, "")).status, 200);
    let m = member(&s, &admin);
    let r = team_get(&s, &m, "/api/team/status");
    let (e1, built) = (r.header("ETag"), r.json()["built"].clone());
    for i in 0..40 {
        let u = format!("dev-1{i:03}");
        let k = generate_key();
        let b = json!({"uuid": u, "pubkey": key_to_b64(&k.verifying_key()), "token": tok}).to_string();
        let sig = reqauth::sign_device_request(&k, &u, RELAY, "POST", "/api/status/enroll", b.as_bytes(), now());
        let ip = format!("203.0.113.{i}"); // 30 new pins per IP and minute
        assert_eq!(s.req("POST", "/api/status/enroll", &[("X-Forwarded-For", &ip), (reqauth::HEADER, &sig)], b.as_bytes()).status, 200);
        assert_eq!(s.admin(&admin, "DELETE", &format!("/api/admin/status/{u}"), b"").status, 204);
    }
    let r = team_get(&s, &m, "/api/team/status");
    assert_eq!((r.header("ETag"), r.json()["built"].clone()), (e1, built), "no rebuild");
    // 81 lines for one live pin: compacted in the background.
    let lines = || std::fs::read_to_string(s.dir.join("status/keys.jsonl")).unwrap().lines().count();
    wait_for("keys.jsonl compaction", || lines() < 40);
    assert_eq!(live_pins(&s.dir), ["dev-0001"]);
    assert_eq!(enroll(&s, &a, "dev-0002", None, &tok).status, 200);
    assert_eq!(live_pins(&s.dir), ["dev-0001", "dev-0002"]);
}

#[test]
fn a_long_history_is_trimmed_after_the_append() {
    let admin = generate_key();
    let dir = new_dir();
    let a = generate_key();
    prep_device(&dir, "dev-0001", &a, now() - 61, 5);
    let t = now();
    let line = |i: u64, at: u64| format!(r#"{{"id":"check/{i:040}","from":"ok","to":"warn","time":"2026-11-23T08:00:00Z","at":{at}}}"#) + "\n";
    // 500 lines older than 90 days, 2500 recent: about 380 KB.
    let old: String = (0..3000).map(|i| line(i, if i < 500 { t - 91 * 86_400 } else { t - 3000 + i })).collect();
    std::fs::write(dir.join("status/dev-0001.history.jsonl"), &old).unwrap();
    let s = start_in(dir, &admin);
    let rep = full("dev-0001", 6, "crit").replace(r#""tether_id":"123456789","#, "");
    assert_eq!(post(&s, &a, "dev-0001", &rep).status, 200);
    let path = s.dir.join("status/dev-0001.history.jsonl");
    wait_for("history trim", || std::fs::read_to_string(&path).unwrap().lines().count() == 2000);
    let kept = std::fs::read_to_string(&path).unwrap();
    // The newest 2000: 1999 of the recent lines, then the change just posted.
    assert_eq!(kept.lines().next().unwrap().to_owned() + "\n", line(1001, t - 3000 + 1001));
    assert!(kept.lines().last().unwrap().contains(r#""id":"app/db-service""#), "{}", kept.lines().last().unwrap());
}

#[test]
fn summary_entries_keep_8_products_and_10_faults() {
    let admin = generate_key();
    let dir = new_dir();
    let a = generate_key();
    prep_device(&dir, "dev-0001", &a, now() - 61, 5);
    let s = start_in(dir, &admin);
    let products: Vec<Value> = (0..9).map(|i| json!({"name": format!("App {i}"), "version": "1.0"})).collect();
    let faults: Vec<Value> = (0..12).map(|i| json!({"id": format!("app/check-{i}"), "state": "warn"})).collect();
    let mut rep: Value = serde_json::from_str(&full("dev-0001", 6, "warn").replace(r#""tether_id":"123456789","#, "")).unwrap();
    (rep["products"], rep["faults"]) = (json!(products), json!(faults));
    assert_eq!(post(&s, &a, "dev-0001", &rep.to_string()).status, 200);
    let m = member(&s, &admin);
    let h = team_get(&s, &m, "/api/team/status").json()["hosts"][0].clone();
    assert_eq!((h["products"].as_array().unwrap().len(), h["more_products"].clone()), (8, json!(1)));
    assert_eq!((h["faults"].as_array().unwrap().len(), h["more_faults"].clone()), (10, json!(2)));
    assert_eq!(h["faults"][0]["id"], "app/check-0", "the first ones, as sent");
    let r = team_get(&s, &m, "/api/team/status/dev-0001").json();
    assert_eq!(r["report"]["products"].as_array().unwrap().len(), 9, "the report keeps them all");
}

/// Name, bytes and mtime of every file in `status/`.
fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>, SystemTime)> {
    let mut v: Vec<_> = std::fs::read_dir(dir.join("status"))
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            (e.file_name().into_string().unwrap(), std::fs::read(e.path()).unwrap(), e.metadata().unwrap().modified().unwrap())
        })
        .collect();
    v.sort();
    v
}

#[test]
fn heartbeat_writes_no_file() {
    let admin = generate_key();
    let dir = new_dir();
    let a = generate_key();
    prep_device(&dir, "dev-0001", &a, now() - 61, 5);
    let mut s = start_in(dir, &admin);
    let before = snapshot(&s.dir);
    assert_eq!(post(&s, &a, "dev-0001", &heartbeat("dev-0001", 6)).status, 200);
    assert_eq!(snapshot(&s.dir), before, "a heartbeat touches no file");
    let st = Command::new("kill").args(["-TERM", &s.child.id().to_string()]).status().unwrap();
    assert!(st.success());
    let _ = s.child.wait();
    let seen: Value = serde_json::from_slice(&std::fs::read(s.dir.join("status/seen.json")).unwrap()).unwrap();
    assert_eq!(seen["dev-0001"]["seq"], 6, "flushed at shutdown");
}

#[test]
fn forged_posts_do_not_rate_limit_a_device() {
    let admin = generate_key();
    let dir = new_dir();
    let a = generate_key();
    prep_device(&dir, "dev-0001", &a, now() - 120, 5);
    let s = start_in(dir, &admin);
    let from = |key: &SigningKey, uuid: &str, ip: &str, body: &[u8]| {
        let sig = reqauth::sign_device_request(key, uuid, RELAY, "POST", "/api/status", body, now());
        s.req("POST", "/api/status", &[(reqauth::HEADER, &sig), ("X-Forwarded-For", ip)], body)
    };
    let stranger = generate_key();
    for i in 0..70 {
        let r = from(&stranger, "dev-0001", "203.0.113.9", heartbeat("dev-0001", 7).as_bytes());
        let want = if i < 60 { (401, "signature does not match") } else { (429, "too many failed requests") };
        assert_eq!((r.status, r.text().as_str()), want, "forged post {i}");
    }
    let r = from(&a, "dev-0001", "198.51.100.4", heartbeat("dev-0001", 6).as_bytes());
    assert_eq!((r.status, r.text()), (200, r#"{"releases":0,"next":300}"#.into()), "the device's own limit is untouched");
    let r = from(&a, "dev-0001", "198.51.100.4", heartbeat("dev-0001", 7).as_bytes());
    assert_eq!((r.status, r.text(), r.header("Retry-After")), (429, "report too frequent".into(), Some("60".into())));
    // Size before anything else, then the key lookup before any parsing.
    let r = from(&stranger, "dev-0009", "198.51.100.5", &vec![b'x'; 65537]);
    assert_eq!((r.status, r.text()), (413, "body too large".into()));
    let r = from(&stranger, "dev-0009", "198.51.100.5", b"not json");
    assert_eq!((r.status, r.text()), (401, "unknown device".into()));
}

#[test]
fn status_off_and_device_cap() {
    let admin = generate_key();
    let admin_env = key_to_b64(&admin.verifying_key());
    let s = spawn_env(new_dir(), &admin_env, &[("STATUS", "off")]);
    let tok = status::make_token("t", now() + 3600, None, None, false, &admin).unwrap().0;
    let a = generate_key();
    let m = member(&s, &admin);
    for r in [
        enroll(&s, &a, "dev-0001", None, &tok),
        post(&s, &a, "dev-0001", &full("dev-0001", 1, "ok")),
        signed(&s, &a, "dev-0001", "DELETE", "/api/status", b""),
        team_get(&s, &m, "/api/team/status"),
    ] {
        assert_eq!((r.status, r.text(), r.header("Retry-After")), (503, "status reporting is off".into(), Some("3600".into())));
    }
    assert!(!s.dir.join("status/keys.json").exists());
    drop(s);

    let s = spawn_env(new_dir(), &admin_env, &[("STATUS_MAX_DEVICES", "2")]);
    assert_eq!(enroll(&s, &generate_key(), "dev-0001", None, &tok).status, 200);
    assert_eq!(enroll(&s, &generate_key(), "dev-0002", None, &tok).status, 200);
    let r = enroll(&s, &generate_key(), "dev-0003", None, &tok);
    assert_eq!((r.status, r.text()), (507, "too many devices (STATUS_MAX_DEVICES)".into()));
    drop(s);

    // `next` from the enrolled count and STATUS_TARGET_RPS (read from an idempotent enrollment).
    for (pins, next) in [(3, 300), (4000, 3600)] {
        let dir = new_dir();
        prep_device(&dir, "dev-0001", &a, now() - 61, 5);
        update_json(&dir.join("status/keys.json"), |m| {
            for i in 1..pins {
                m.insert(format!("dev-{i:06}"), json!({"key": key_to_b64(&a.verifying_key()), "tether_id": null, "label": "t", "enrolled": now()}));
            }
        });
        let s = spawn_env(dir, &admin_env, &[("STATUS_TARGET_RPS", "1")]);
        let r = enroll(&s, &a, "dev-0001", None, &tok);
        assert_eq!((r.status, r.json()["next"].clone()), (200, json!(next)), "{pins} pins");
    }
}

fn metric(s: &Srv, series: &str) -> Option<f64> {
    let page = s.req("GET", "/metrics", &[], b"").text();
    page.lines().find_map(|l| l.strip_prefix(series)?.strip_prefix(' ')?.parse().ok())
}

#[test]
fn summary_is_cached_with_etag_and_metrics() {
    let admin = generate_key();
    let dir = new_dir();
    let (a, b) = (generate_key(), generate_key());
    prep_device(&dir, "dev-0001", &a, now() - 61, 5);
    prep_device(&dir, "dev-0002", &b, now() - 61, 5);
    update_json(&dir.join("status/seen.json"), |m| {
        let e = m["dev-0002"]["entry"].as_str().unwrap().replace(r#""overall":"crit""#, r#""overall":"ok""#);
        m.get_mut("dev-0002").unwrap()["entry"] = e.into();
    });
    let s = start_in(dir, &admin);
    let m = member(&s, &admin);
    let r = team_get(&s, &m, "/api/team/status");
    assert_eq!(r.status, 200);
    let e1 = r.header("ETag").unwrap();
    assert_eq!(r.json()["hosts"][1]["overall"], "ok");
    let sig = reqauth::sign_request(&m, "GET", "/api/team/status", b"", now());
    assert_eq!(s.req("GET", "/api/team/status", &[(reqauth::HEADER, &sig), ("If-None-Match", &e1)], b"").status, 304);
    assert_eq!(metric(&s, r#"tether_api_status_hosts{state="crit"}"#), Some(1.));
    assert_eq!(metric(&s, r#"tether_api_status_hosts{state="ok"}"#), Some(1.));
    assert_eq!(metric(&s, r#"tether_api_status_hosts{state="stale"}"#), Some(0.));

    let rep = full("dev-0002", 6, "warn").replace(r#""tether_id":"123456789","#, "");
    assert_eq!(post(&s, &b, "dev-0002", &rep).status, 200);
    let r = team_get(&s, &m, "/api/team/status");
    assert_eq!((r.status, r.header("ETag")), (200, Some(e1)), "cached for 30 s");
    assert_eq!(r.json()["hosts"][1]["overall"], "ok");
    // The gauges are counted at scrape time.
    assert_eq!(metric(&s, r#"tether_api_status_hosts{state="warn"}"#), Some(1.));
    assert_eq!(metric(&s, r#"tether_api_status_hosts{state="ok"}"#), Some(0.));
    assert_eq!(metric(&s, r#"tether_api_requests_total{endpoint="status",code="200"}"#), Some(1.));
}

#[test]
fn pins_expire_after_180_days() {
    let admin = generate_key();
    let dir = new_dir();
    let (a, b) = (generate_key(), generate_key());
    prep_device(&dir, "dev-0001", &a, now() - 181 * 86_400, 5);
    prep_device(&dir, "dev-0002", &b, now() - 179 * 86_400, 5);
    std::fs::write(dir.join("status/dev-0001.history.jsonl"), "{}\n").unwrap();
    let s = start_in(dir, &admin);
    let mut names: Vec<String> = std::fs::read_dir(s.dir.join("status")).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    names.sort();
    // keys.json (the earlier format) migrated to keys.jsonl, then the expired pin unpinned there.
    assert_eq!(names, ["dev-0002.json", "keys.json.migrated", "keys.jsonl", "seen.json"]);
    assert_eq!(live_pins(&s.dir), ["dev-0002"]);
    let r = post(&s, &a, "dev-0001", &heartbeat("dev-0001", 6));
    assert_eq!((r.status, r.text()), (401, "unknown device".into()));
}

/// A signed `monitor.json` (or, with other `key`/`file`, any index) naming one release.
fn monitor_doc(admin: &SigningKey, channel: &str, serial: u64, key: &str, file: &str, version: &str) -> Vec<u8> {
    let entry = json!({
        "version": version, "file": file, "size": 3, "sha256": "a".repeat(64), "signature": "U0lH",
        "urls": [format!("https://{{relay}}/download/{channel}/{key}/{file}")],
        "rollout": {"start": 0, "steps": [[0, 100]]}
    });
    let payload = json!({"type": "tether-releases/1", "channel": channel, "serial": serial, "issued": 1, "expires": 2_000_000_000u64, "releases": {key: entry}});
    serde_json::to_vec(&tether_trust::envelope::sign(&serde_json::to_vec(&payload).unwrap(), admin)).unwrap()
}

/// The `releases` hint a status post answers (a new device each time: one post per minute).
fn status_releases(s: &Srv, admin: &SigningKey, uuid: &str) -> u64 {
    let dev = generate_key();
    let tok = status::make_token("t", now() + 3600, None, None, false, admin).unwrap().0;
    assert_eq!(enroll(s, &dev, uuid, None, &tok).status, 200);
    let r = post(s, &dev, uuid, &full(uuid, 1, "ok").replace(r#""tether_id":"123456789","#, ""));
    assert_eq!(r.status, 200, "{}", r.text());
    r.json()["releases"].as_u64().unwrap()
}

/// The second release index: own app names, served like releases.json, does not move Tether's
/// release generation, its serials are the status answers' `releases` hint, guarded and pruned.
#[test]
fn monitor_index_upload_serve_and_prune() {
    let (s, k) = server();
    let mon = ("monitor/windows-x86_64", "tether-monitor-0.1.0.exe", "0.1.0");
    let tech = ("technician/windows-x86_64", "Tether_0.5.24_x64-setup.exe", "0.5.24");
    let put = |p: &str, b: &[u8]| s.admin(&k, "PUT", &format!("/api/admin/release/{p}"), b);

    let r = put("stable/releases.json", &monitor_doc(&k, "stable", 1, mon.0, mon.1, mon.2));
    assert_eq!(r.status, 400);
    assert!(r.text().contains("bad release key"), "{}", r.text());
    assert_eq!(put("stable/monitor.json", &monitor_doc(&k, "stable", 1, tech.0, tech.1, tech.2)).status, 400);
    let doc3 = monitor_doc(&k, "stable", 3, mon.0, mon.1, mon.2);
    assert_eq!(put("stable/monitor.json", &doc3).status, 204);
    assert_eq!(put("stable/monitor.json", &doc3).status, 204, "the same document again");
    assert_eq!(put("stable/monitor.json", &monitor_doc(&k, "stable", 2, mon.0, mon.1, mon.2)).status, 409);

    let r = s.req("GET", "/download/stable/monitor.json", &[], b"");
    assert_eq!((r.status, r.header("ETag"), r.header("Cache-Control")), (200, Some("\"3\"".into()), Some("no-cache".into())));
    assert_eq!(r.body, doc3);
    assert_eq!(s.req("GET", "/download/stable/monitor.json", &[("If-None-Match", "\"3\"")], b"").status, 304);
    assert_eq!(s.req("GET", "/download/test/monitor.json", &[], b"").status, 404);

    let hb = r#"{"uuid":"hb-000001","id":"123456789","profile":"technician","version":"0.3.0","os":"windows 10.0.26300","serial":12}"#;
    let r = s.req("POST", "/api/heartbeat", &[], hb.as_bytes());
    assert_eq!((r.status, r.json()), (200, json!({"releases": 0})), "Tether's G did not move");
    assert_eq!(status_releases(&s, &k, "dev-0001"), 3);
    assert_eq!(put("test/monitor.json", &monitor_doc(&k, "test", 5, mon.0, mon.1, mon.2)).status, 204);
    assert_eq!(status_releases(&s, &k, "dev-0002"), 8);

    // Prune: the monitor folder is protected by monitor.json.
    let dir = s.dir.join("download/stable/monitor/windows-x86_64");
    std::fs::create_dir_all(&dir).unwrap();
    let old = SystemTime::now() - std::time::Duration::from_secs(2 * 86_400);
    for v in ["0.0.7", "0.0.8", "0.0.9", "0.1.0"] {
        for f in [format!("tether-monitor-{v}.exe"), format!("tether-monitor-{v}.exe.sig")] {
            std::fs::write(dir.join(&f), b"x").unwrap();
            std::fs::File::options().write(true).open(dir.join(&f)).unwrap().set_modified(old).unwrap();
        }
    }
    let plan = s.admin(&k, "POST", "/api/admin/releases/prune", br#"{"dry_run":true,"keep":2}"#).json();
    let g = plan["groups"].as_array().unwrap().iter().find(|g| g["dir"] == "monitor/windows-x86_64").unwrap_or_else(|| panic!("{plan}"));
    assert_eq!(g["published"], "0.1.0");
    let vs = |l: &Value| l.as_array().unwrap().iter().map(|v| (v["version"].as_str().unwrap().to_owned(), v["why"].clone())).collect::<Vec<_>>();
    assert_eq!(vs(&g["keep"]), [("0.1.0".into(), json!(["index"])), ("0.0.9".into(), json!(["previous"])), ("0.0.8".into(), json!(["previous"]))]);
    assert_eq!(vs(&g["drop"]).into_iter().map(|v| v.0).collect::<Vec<_>>(), ["0.0.7"]);
    let r = s.admin(&k, "DELETE", "/api/admin/release/stable/monitor/windows-x86_64/tether-monitor-0.1.0.exe", b"");
    assert_eq!(r.status, 409);
    assert!(r.text().contains("published release: "), "{}", r.text());

    let p = "/api/admin/release/stable/monitor.json";
    assert_eq!(s.admin(&k, "DELETE", p, b"").status, 409);
    assert_eq!(s.admin(&k, "DELETE", &format!("{p}?force=1"), b"").status, 204);
    assert_eq!(s.req("GET", "/download/stable/monitor.json", &[], b"").status, 404);
    assert_eq!(status_releases(&s, &k, "dev-0003"), 5);
}

// ---- Revocable tokens, verified Tether IDs, removal by the team.

/// Two roster members: `W` with `write`, `R` without.
fn writer_and_reader(s: &Srv, admin: &SigningKey) -> (SigningKey, SigningKey) {
    let (w, r) = (generate_key(), generate_key());
    let roster = json!({"members": [
        {"name": "W", "key": key_to_b64(&w.verifying_key()), "write": true, "audit": false},
        {"name": "R", "key": key_to_b64(&r.verifying_key()), "write": false, "audit": false}]});
    assert_eq!(s.admin(admin, "PUT", "/api/admin/team", roster.to_string().as_bytes()).status, 204);
    (w, r)
}

fn revoke(s: &Srv, admin: &SigningKey, id: &str, exp: u64) -> Reply {
    s.admin(admin, "POST", "/api/admin/status/revoke", json!({"id": id, "exp": exp}).to_string().as_bytes())
}

/// Stops `s` (SIGTERM, as systemd does) and starts the relay again on its folder.
fn restart(mut s: Srv, admin: &SigningKey) -> Srv {
    assert!(Command::new("kill").args(["-TERM", &s.child.id().to_string()]).status().unwrap().success());
    let _ = s.child.wait();
    let dir = s.dir.clone();
    std::mem::forget(s); // its Drop would remove the folder
    start_in(dir, admin)
}

#[test]
fn revoked_tokens_are_refused_across_a_restart() {
    let (s, admin) = server();
    let (tok, id) = status::make_token("t", now() + 3600, None, None, false, &admin).unwrap();
    let (other, _) = status::make_token("t", now() + 3600, None, None, false, &admin).unwrap();
    let a = generate_key();
    assert_eq!(enroll(&s, &a, "dev-0001", None, &tok).status, 200);
    assert_eq!(revoke(&s, &admin, &id, now() + 3600).status, 204);
    assert_eq!(revoke(&s, &admin, &id, now() + 3600).status, 204, "idempotent");
    let r = enroll(&s, &generate_key(), "dev-0002", None, &tok);
    assert_eq!((r.status, r.text()), (401, "token revoked".into()));
    assert_eq!(enroll(&s, &a, "dev-0001", None, &tok).status, 401, "also for a device it enrolled");
    assert_eq!(enroll(&s, &generate_key(), "dev-0003", None, &other).status, 200, "an unrevoked token is accepted");
    assert_eq!(s.admin(&admin, "GET", "/api/admin/status/devices", b"").json()["devices"][0]["uuid"], "dev-0001", "revoking does not unpin");
    let listed = s.admin(&admin, "GET", "/api/admin/status/revoked", b"").json();
    assert!(listed[&id].as_u64().unwrap() > now(), "{listed}");
    assert_eq!(revoke(&s, &admin, "short", now() + 60).status, 400);
    let r = revoke(&s, &admin, "abcdefghABCDEFGH", now() - 1);
    assert_eq!((r.status, r.text()), (400, "exp has passed".into()), "never a silent no-op");
    // On disk before the answer; still refused after a restart.
    let file: Value = serde_json::from_slice(&std::fs::read(s.dir.join("status/revoked.json")).unwrap()).unwrap();
    assert!(file[&id].is_u64() && file.get("abcdefghABCDEFGH").is_none(), "{file}");
    let s = restart(s, &admin);
    let r = enroll(&s, &generate_key(), "dev-0004", None, &tok);
    assert_eq!((r.status, r.text()), (401, "token revoked".into()));
}

#[test]
fn expired_revocations_are_dropped_and_a_bad_file_locks() {
    let admin = generate_key();
    let dir = new_dir();
    std::fs::create_dir_all(dir.join("status")).unwrap();
    let live = "liveliveLIVELIVE";
    let exp = now() + 600;
    std::fs::write(dir.join("status/revoked.json"), json!({"oldoldOLDOLDoldo": now() - 10, live: exp}).to_string()).unwrap();
    let s = start_in(dir, &admin);
    assert_eq!(s.admin(&admin, "GET", "/api/admin/status/revoked", b"").json(), json!({live: exp}));
    // The next write (a revocation) leaves the expired one out of the file too.
    assert_eq!(revoke(&s, &admin, "nextnextNEXTNEXT", now() + 60).status, 204);
    let file: Value = serde_json::from_slice(&std::fs::read(s.dir.join("status/revoked.json")).unwrap()).unwrap();
    assert_eq!(file.as_object().unwrap().keys().collect::<Vec<_>>(), [live, "nextnextNEXTNEXT"], "{file}");
    drop(s);

    // Unreadable: locked (fail closed), nothing written.
    let dir = new_dir();
    std::fs::create_dir_all(dir.join("status")).unwrap();
    std::fs::write(dir.join("status/revoked.json"), "{not json").unwrap();
    let s = start_in(dir, &admin);
    let tok = status::make_token("t", now() + 3600, None, None, false, &admin).unwrap().0;
    let r = enroll(&s, &generate_key(), "dev-0001", None, &tok);
    assert_eq!((r.status, r.text()), (503, "status store unavailable".into()));
    assert_eq!(revoke(&s, &admin, "nextnextNEXTNEXT", now() + 60).status, 503);
    assert_eq!(std::fs::read_to_string(s.dir.join("status/revoked.json")).unwrap(), "{not json");
}

#[test]
fn a_bound_token_pins_its_id_for_one_device() {
    let (s, admin) = server();
    let (tok, id) = status::make_token("front desk", now() + 3600, Some("123456789"), None, false, &admin).unwrap();
    let (a, b) = (generate_key(), generate_key());
    let (w, _) = writer_and_reader(&s, &admin);
    let r = enroll(&s, &a, "dev-0001", Some("555"), &tok);
    assert_eq!((r.status, r.text()), (409, "token is issued for another id".into()));
    assert_eq!(enroll(&s, &a, "dev-0001", None, &tok).status, 200);
    let h = team_get(&s, &w, "/api/team/status/dev-0001").json();
    assert_eq!((&h["tether_id"], &h["claimed_id"], &h["linked_by"]), (&json!("123456789"), &Value::Null, &Value::Null));
    assert!(h["linked_at"].as_u64().unwrap() >= now() - 5 && h["enrolled"].is_u64(), "{h}");
    assert_eq!(enroll(&s, &a, "dev-0001", Some("123456789"), &tok).status, 200, "the same device again");
    for t in [None, Some("123456789")] {
        let r = enroll(&s, &b, "dev-0002", t, &tok);
        assert_eq!((r.status, r.text()), (409, "token already used".into()));
    }
    let list = s.admin(&admin, "GET", &format!("/api/admin/status/devices?token={id}"), b"").json();
    assert_eq!((list["devices"].as_array().unwrap().len(), &list["devices"][0]["tether_id"]), (1, &json!("123456789")));
    // Used is kept across a restart (a `used` line in keys.jsonl).
    let s = restart(s, &admin);
    assert_eq!(enroll(&s, &b, "dev-0002", None, &tok).text(), "token already used");
    assert_eq!(enroll(&s, &a, "dev-0001", None, &tok).status, 200);
}

#[test]
fn an_unbound_token_only_claims_an_id() {
    let (s, admin) = server();
    let tok = status::make_token("t", now() + 3600, None, None, false, &admin).unwrap().0;
    let (a, b) = (generate_key(), generate_key());
    assert_eq!(enroll(&s, &a, "dev-0001", Some("123456789"), &tok).status, 200);
    assert_eq!(enroll(&s, &b, "dev-0002", Some("123456789"), &tok).status, 200, "a claim pins nothing");
    assert_eq!(post(&s, &a, "dev-0001", &full("dev-0001", 1, "crit")).status, 200, "a report may name its claim");
    let m = member(&s, &admin);
    let host = team_get(&s, &m, "/api/team/status").json()["hosts"][0].clone();
    assert_eq!((&host["tether_id"], &host["claimed_id"]), (&Value::Null, &json!("123456789")), "{host}");
    let r = post(&s, &b, "dev-0002", &full("dev-0002", 1, "crit").replace("123456789", "555"));
    assert_eq!((r.status, r.text()), (409, "tether_id is pinned to another value".into()));
}

#[test]
fn members_with_write_link_and_unlink() {
    let (s, admin) = server();
    let tok = status::make_token("t", now() + 3600, None, None, false, &admin).unwrap().0;
    let (a, b) = (generate_key(), generate_key());
    assert_eq!(enroll(&s, &a, "dev-0001", Some("123456789"), &tok).status, 200);
    assert_eq!(enroll(&s, &b, "dev-0002", Some("123456789"), &tok).status, 200);
    let (w, r) = writer_and_reader(&s, &admin);
    let link = |m: &SigningKey, uuid: &str, id: &str| s.admin(m, "POST", &format!("/api/team/status/{uuid}/link"), json!({"tether_id": id}).to_string().as_bytes());
    assert_eq!(link(&r, "dev-0001", "123456789").status, 403);
    let x = link(&w, "dev-0001", "999999999");
    assert_eq!((x.status, x.text()), (409, "tether_id is not the one this device claims".into()));
    assert_eq!(link(&w, "dev-9999", "123456789").status, 404);
    assert_eq!(link(&w, "dev-0001", "m:dev-0001").status, 400);
    assert_eq!(link(&w, "dev-0001", "123456789").status, 204);
    assert_eq!(link(&w, "dev-0001", "123456789").status, 204, "the same link again");
    let h = team_get(&s, &r, "/api/team/status/dev-0001").json();
    assert_eq!((&h["tether_id"], &h["claimed_id"], &h["linked_by"]), (&json!("123456789"), &Value::Null, &json!("W")));
    assert!(h["linked_at"].as_u64().unwrap() >= now() - 5);
    // The summary shows it at once (rebuilt for the link).
    assert_eq!(post(&s, &a, "dev-0001", &full("dev-0001", 1, "crit")).status, 200);
    assert_eq!(team_get(&s, &r, "/api/team/status").json()["hosts"][0]["tether_id"], "123456789");
    let x = link(&w, "dev-0002", "123456789");
    assert_eq!((x.status, x.text()), (409, "tether_id is pinned to another device".into()));
    // Unlink removes the link and the claim; then the other device may be linked.
    assert_eq!(s.admin(&r, "DELETE", "/api/team/status/dev-0001/link", b"").status, 403);
    assert_eq!(s.admin(&w, "DELETE", "/api/team/status/dev-0001/link", b"").status, 204);
    let h = team_get(&s, &r, "/api/team/status/dev-0001").json();
    assert_eq!((&h["tether_id"], &h["claimed_id"], &h["linked_by"], &h["no_claim"]), (&Value::Null, &Value::Null, &Value::Null, &json!(true)));
    assert_eq!(s.admin(&w, "DELETE", "/api/team/status/dev-0001/link", b"").status, 204, "again: no change");
    assert_eq!(link(&w, "dev-0001", "123456789").status, 409, "no claim left to confirm");
    assert_eq!(link(&w, "dev-0002", "123456789").status, 204);
    // The link of dev-0002 survives a restart.
    let s = restart(s, &admin);
    let h = team_get(&s, &r, "/api/team/status/dev-0002").json();
    assert_eq!((&h["tether_id"], &h["linked_by"]), (&json!("123456789"), &json!("W")));
}

#[test]
fn a_device_cannot_change_its_link() {
    let (s, admin) = server();
    let tok = status::make_token("t", now() + 3600, None, None, false, &admin).unwrap().0;
    let a = generate_key();
    assert_eq!(enroll(&s, &a, "dev-0001", Some("123456789"), &tok).status, 200);
    let (w, _) = writer_and_reader(&s, &admin);
    assert_eq!(s.admin(&w, "POST", "/api/team/status/dev-0001/link", br#"{"tether_id":"123456789"}"#).status, 204);
    let r = post(&s, &a, "dev-0001", &full("dev-0001", 1, "crit").replace("123456789", "555"));
    assert_eq!((r.status, r.text()), (409, "tether_id is pinned to another value".into()));
    let r = enroll(&s, &a, "dev-0001", Some("555"), &tok);
    assert_eq!((r.status, r.text()), (409, "tether_id is pinned to another value".into()));
    // Its own key does not sign team routes.
    for (m, p) in [("POST", "/api/team/status/dev-0001/link"), ("DELETE", "/api/team/status/dev-0001/link"), ("DELETE", "/api/team/status/dev-0001")] {
        let body: &[u8] = if m == "POST" { br#"{"tether_id":"555"}"# } else { b"" };
        assert_eq!(signed(&s, &a, "dev-0001", m, p, body).status, 401, "{m} {p}");
    }
    assert_eq!(team_get(&s, &w, "/api/team/status/dev-0001").json()["tether_id"], "123456789");
}

#[test]
fn team_delete_needs_write() {
    let (s, admin) = server();
    let tok = status::make_token("t", now() + 3600, None, None, false, &admin).unwrap().0;
    let a = generate_key();
    assert_eq!(enroll(&s, &a, "dev-0001", Some("123456789"), &tok).status, 200);
    assert_eq!(post(&s, &a, "dev-0001", &full("dev-0001", 1, "crit")).status, 200);
    let (w, r) = writer_and_reader(&s, &admin);
    assert_eq!(s.admin(&r, "DELETE", "/api/team/status/dev-0001", b"").status, 403);
    assert_eq!(team_get(&s, &r, "/api/team/status/dev-0001").status, 200);
    assert_eq!(s.admin(&w, "DELETE", "/api/team/status/dev-0001", b"").status, 204);
    assert_eq!(team_get(&s, &r, "/api/team/status/dev-0001").status, 404);
    assert!(!s.dir.join("status/dev-0001.json").exists());
    assert_eq!(s.admin(&w, "DELETE", "/api/team/status/dev-0001", b"").status, 404);
    assert_eq!(s.admin(&w, "DELETE", "/api/team/status/x/../../keys", b"").status, 404);
    let r = post(&s, &a, "dev-0001", &full("dev-0001", 2, "crit"));
    assert_eq!((r.status, r.text()), (401, "unknown device".into()));
}

#[test]
fn the_admin_device_list_pages_and_filters_by_token() {
    let admin = generate_key();
    let dir = new_dir();
    std::fs::create_dir_all(dir.join("status")).unwrap();
    let k = key_to_b64(&generate_key().verifying_key());
    let (ta, tb) = ("tokenAtokenAtokenA", "tokenBtokenBtokenB");
    let lines: String = (0..1005)
        .map(|i| json!({"op": "set", "uuid": format!("dev-{i:04}"), "key": k, "tether_id": null, "label": "t", "enrolled": now(), "token": if i % 2 == 0 { ta } else { tb }}).to_string() + "\n")
        .collect();
    std::fs::write(dir.join("status/keys.jsonl"), lines).unwrap();
    let s = start_in(dir, &admin);
    let get = |q: &str| {
        let r = s.admin(&admin, "GET", &format!("/api/admin/status/devices{q}"), b"");
        assert_eq!(r.status, 200, "{q}: {}", r.text());
        r.json()
    };
    let uuids = |v: &Value| v["devices"].as_array().unwrap().iter().map(|d| d["uuid"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
    let p1 = get("");
    assert_eq!((uuids(&p1).len(), uuids(&p1)[0].as_str(), &p1["next"]), (1000, "dev-0000", &json!("dev-0999")));
    assert_eq!((&p1["devices"][0]["token"], &p1["devices"][0]["host"], &p1["devices"][0]["last_seen"]), (&json!(ta), &Value::Null, &Value::Null));
    let p2 = get("?after=dev-0999");
    assert_eq!((uuids(&p2), &p2["next"]), (["dev-1000", "dev-1001", "dev-1002", "dev-1003", "dev-1004"].map(String::from).to_vec(), &Value::Null));
    // A filter looks at no more than one page of pins per request: short pages, `next` moves on.
    let a = get(&format!("?token={ta}"));
    assert_eq!((uuids(&a).len(), &a["next"]), (500, &json!("dev-0999")), "token A on the first 1000 pins");
    let a2 = get(&format!("?token={ta}&after=dev-0999"));
    assert_eq!((uuids(&a2).len(), &a2["next"]), (3, &Value::Null), "503 of the 1005 have token A");
    assert!(uuids(&a).iter().chain(&uuids(&a2)).all(|u| u[4..].parse::<u32>().unwrap() % 2 == 0));
    assert_eq!(uuids(&get(&format!("?token={tb}&after=dev-0500")))[0], "dev-0501");
    // A filter that matches nothing: empty pages, still paged.
    let none = get("?token=nobodynobodynobody");
    assert_eq!((uuids(&none).len(), &none["next"]), (0, &json!("dev-0999")));
    let none = get("?token=nobodynobodynobody&after=dev-0999");
    assert_eq!((uuids(&none).len(), &none["next"]), (0, &Value::Null));
    for bad in ["?token=x", "?after=../x", "?limit=5"] {
        assert_eq!(s.admin(&admin, "GET", &format!("/api/admin/status/devices{bad}"), b"").status, 400, "{bad}");
    }
}

#[test]
fn new_routes_refuse_other_credentials() {
    let (s, admin) = server();
    let tok = status::make_token("t", now() + 3600, None, None, false, &admin).unwrap().0;
    let dev = generate_key();
    assert_eq!(enroll(&s, &dev, "dev-0001", Some("123456789"), &tok).status, 200);
    let (w, _) = writer_and_reader(&s, &admin);
    let rv = json!({"id": "abcdefghABCDEFGH", "exp": now() + 60}).to_string();
    let admin_routes: [(&str, &str, &[u8]); 3] = [("POST", "/api/admin/status/revoke", rv.as_bytes()), ("GET", "/api/admin/status/revoked", b""), ("GET", "/api/admin/status/devices", b"")];
    for (m, p, body) in admin_routes {
        assert_eq!(s.admin(&w, m, p, body).status, 401, "member key on {m} {p}");
        assert_eq!(signed(&s, &dev, "dev-0001", m, p, body).status, 401, "device key on {m} {p}");
    }
    let link: &[u8] = br#"{"tether_id":"123456789"}"#;
    let team_routes: [(&str, &str, &[u8]); 3] = [("POST", "/api/team/status/dev-0001/link", link), ("DELETE", "/api/team/status/dev-0001/link", b""), ("DELETE", "/api/team/status/dev-0001", b"")];
    for (m, p, body) in team_routes {
        assert_eq!(s.admin(&admin, m, p, body).status, 401, "admin key on {m} {p}");
        assert_eq!(signed(&s, &dev, "dev-0001", m, p, body).status, 401, "device key on {m} {p}");
    }
    // Nothing changed: the claim is still only a claim.
    let h = team_get(&s, &w, "/api/team/status/dev-0001").json();
    assert_eq!((&h["tether_id"], &h["claimed_id"]), (&Value::Null, &json!("123456789")));
    assert_eq!(s.admin(&admin, "GET", "/api/admin/status/revoked", b"").json(), json!({}));
}

#[test]
fn unlink_dismisses_the_claim_until_a_member_allows_one() {
    let (s, admin) = server();
    let (w, r) = writer_and_reader(&s, &admin);
    // A token issued for the ID: Unlink holds, the same token enrolling again changes nothing.
    let bound = status::make_token("t", now() + 3600, Some("123456789"), None, false, &admin).unwrap().0;
    let any = status::make_token("t", now() + 3600, None, None, false, &admin).unwrap().0;
    let a = generate_key();
    assert_eq!(enroll(&s, &a, "dev-0001", None, &bound).status, 200);
    assert_eq!(team_get(&s, &r, "/api/team/status/dev-0001").json()["tether_id"], "123456789");
    assert_eq!(s.admin(&w, "DELETE", "/api/team/status/dev-0001/link", b"").status, 204);
    for tok in [&bound, &any] {
        assert_eq!(enroll(&s, &a, "dev-0001", Some("123456789"), tok).status, 200);
        let h = team_get(&s, &r, "/api/team/status/dev-0001").json();
        assert_eq!((&h["tether_id"], &h["claimed_id"], &h["no_claim"]), (&Value::Null, &Value::Null, &json!(true)), "still unlinked");
    }
    // Reports naming the ID are accepted and the field ignored.
    assert_eq!(post(&s, &a, "dev-0001", &full("dev-0001", 1, "crit")).status, 200);
    assert_eq!(team_get(&s, &r, "/api/team/status/dev-0001").json()["claimed_id"], Value::Null);
    // Allow a new claim (write only): the next enrolment or report names the claim again.
    assert_eq!(s.admin(&r, "POST", "/api/team/status/dev-0001/allow-claim", b"").status, 403);
    assert_eq!(s.admin(&w, "POST", "/api/team/status/dev-0001/allow-claim", b"").status, 204);
    assert_eq!(enroll(&s, &a, "dev-0001", Some("123456789"), &any).status, 200);
    let h = team_get(&s, &r, "/api/team/status/dev-0001").json();
    assert_eq!((&h["tether_id"], &h["claimed_id"], &h["no_claim"]), (&Value::Null, &json!("123456789"), &json!(false)));
    // An unbound device, dismissed, then allowed: a report restores the claim; across a restart.
    let b = generate_key();
    assert_eq!(enroll(&s, &b, "dev-0002", Some("222222222"), &any).status, 200);
    assert_eq!(s.admin(&w, "DELETE", "/api/team/status/dev-0002/link", b"").status, 204);
    assert_eq!(s.admin(&w, "POST", "/api/team/status/dev-0002/allow-claim", b"").status, 204);
    assert_eq!(post(&s, &b, "dev-0002", &full("dev-0002", 1, "ok").replace("123456789", "222222222")).status, 200);
    assert_eq!(team_get(&s, &r, "/api/team/status/dev-0002").json()["claimed_id"], "222222222");
    let s = restart(s, &admin);
    assert_eq!(team_get(&s, &r, "/api/team/status/dev-0002").json()["claimed_id"], "222222222");
    assert_eq!(team_get(&s, &r, "/api/team/status/dev-0001").json()["claimed_id"], "123456789");
}

#[test]
fn legacy_pins_bring_only_claims() {
    let admin = generate_key();
    let a = generate_key();
    let m = |s: &Srv| writer_and_reader(s, &admin).1;
    // A `pin` line from before verified IDs.
    let dir = new_dir();
    std::fs::create_dir_all(dir.join("status")).unwrap();
    let line = json!({"op": "pin", "uuid": "dev-0001", "key": key_to_b64(&a.verifying_key()), "tether_id": "123456789", "label": "t", "enrolled": now()});
    std::fs::write(dir.join("status/keys.jsonl"), format!("{line}\n")).unwrap();
    let s = start_in(dir, &admin);
    let h = team_get(&s, &m(&s), "/api/team/status/dev-0001").json();
    assert_eq!((&h["tether_id"], &h["claimed_id"]), (&Value::Null, &json!("123456789")));
    // A bound token for that ID can still go to another device: nothing verified it here.
    let bound = status::make_token("t", now() + 3600, Some("123456789"), None, false, &admin).unwrap().0;
    assert_eq!(enroll(&s, &generate_key(), "dev-0002", None, &bound).status, 200);
    drop(s);
    // The earlier keys.json, migrated.
    let dir = new_dir();
    std::fs::create_dir_all(dir.join("status")).unwrap();
    let old = json!({"dev-0001": {"key": key_to_b64(&a.verifying_key()), "tether_id": "123456789", "label": "t", "enrolled": now()}});
    std::fs::write(dir.join("status/keys.json"), old.to_string()).unwrap();
    let s = start_in(dir, &admin);
    let h = team_get(&s, &m(&s), "/api/team/status/dev-0001").json();
    assert_eq!((&h["tether_id"], &h["claimed_id"]), (&Value::Null, &json!("123456789")));
    assert!(std::fs::read_to_string(s.dir.join("status/keys.jsonl")).unwrap().contains(r#""claimed_id":"123456789""#));
}

#[test]
fn used_tokens_survive_a_compaction() {
    let admin = generate_key();
    let dir = new_dir();
    std::fs::create_dir_all(dir.join("status")).unwrap();
    let (bound, id) = status::make_token("t", now() + 3600, Some("123456789"), None, false, &admin).unwrap();
    let k = key_to_b64(&generate_key().verifying_key());
    // Many lines for one pin, then the token's use: well past the compaction threshold.
    let mut lines: String = (0..100).map(|_| json!({"op": "set", "uuid": "dev-0001", "key": k, "tether_id": null, "label": "t", "enrolled": now()}).to_string() + "\n").collect();
    lines += &(json!({"op": "used", "token": id, "uuid": "dev-0001", "exp": now() + 3600}).to_string() + "\n");
    std::fs::write(dir.join("status/keys.jsonl"), lines).unwrap();
    let s = start_in(dir, &admin);
    let any = status::make_token("t", now() + 3600, None, None, false, &admin).unwrap().0;
    assert_eq!(enroll(&s, &generate_key(), "dev-0002", None, &any).status, 200, "an append starts the compaction");
    let count = || std::fs::read_to_string(s.dir.join("status/keys.jsonl")).unwrap().lines().count();
    wait_for("compaction", || count() < 10);
    assert!(std::fs::read_to_string(s.dir.join("status/keys.jsonl")).unwrap().contains(r#""op":"used""#));
    let s = restart(s, &admin);
    assert_eq!(enroll(&s, &generate_key(), "dev-0003", None, &bound).text(), "token already used");
}

#[test]
fn revocations_are_capped() {
    let admin = generate_key();
    let dir = new_dir();
    std::fs::create_dir_all(dir.join("status")).unwrap();
    let full: serde_json::Map<String, Value> = (0..10_000).map(|i| (format!("tok{i:013}"), json!(now() + 600))).collect();
    std::fs::write(dir.join("status/revoked.json"), Value::Object(full).to_string()).unwrap();
    let s = start_in(dir, &admin);
    let r = revoke(&s, &admin, "onemoreONEMOREone", now() + 60);
    assert_eq!((r.status, r.text()), (507, "too many revoked tokens".into()));
    assert_eq!(revoke(&s, &admin, "tok0000000000007", now() + 900).status, 204, "a known one may be extended");
}

#[test]
fn the_periodic_tick_drops_expired_revocations() {
    let admin = generate_key();
    let dir = new_dir();
    std::fs::create_dir_all(dir.join("status")).unwrap();
    std::fs::write(dir.join("status/revoked.json"), json!({"soonsoonSOONSOON": now() + 3, "laterlaterLATERla": now() + 3600}).to_string()).unwrap();
    let s = start_in(dir, &admin);
    // The writer's tick runs once a minute.
    let read = || serde_json::from_slice::<Value>(&std::fs::read(s.dir.join("status/revoked.json")).unwrap()).unwrap();
    for _ in 0..150 {
        if read().get("soonsoonSOONSOON").is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    assert_eq!(read().as_object().unwrap().keys().collect::<Vec<_>>(), ["laterlaterLATERla"]);
}

// ---- Relay load-protection, round 2.

#[test]
fn an_unwritable_status_folder_locks_only_the_status_store() {
    use std::os::unix::fs::PermissionsExt;
    let admin = generate_key();
    let dir = new_dir();
    let st = dir.join("status");
    std::fs::create_dir_all(&st).unwrap();
    // A migration is due (keys.json), but the folder cannot be written (read-only, or a full disk).
    let old = json!({"dev-0001": {"key": key_to_b64(&generate_key().verifying_key()), "tether_id": null, "label": "t", "enrolled": now()}});
    std::fs::write(st.join("keys.json"), old.to_string()).unwrap();
    std::fs::set_permissions(&st, std::fs::Permissions::from_mode(0o500)).unwrap();
    let s = start_in(dir, &admin);
    let tok = status::make_token("t", now() + 3600, None, None, false, &admin).unwrap().0;
    let r = enroll(&s, &generate_key(), "dev-0002", None, &tok);
    assert_eq!((r.status, r.text()), (503, "status store unavailable".into()));
    let hb = r#"{"uuid":"hb-000001","id":"123456789","profile":"technician","version":"0.6.3","os":"windows","serial":1}"#;
    assert_eq!(s.req("POST", "/api/heartbeat", &[], hb.as_bytes()).status, 200, "the relay's other work goes on");
    assert_eq!(s.req("GET", "/api/policy", &[], b"").status, 404, "no policy published, not a 5xx");
    std::fs::set_permissions(s.dir.join("status"), std::fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn a_signed_request_is_not_replayable_after_a_restart() {
    let (s, admin) = server();
    let m = member(&s, &admin);
    let sig = reqauth::sign_request(&m, "GET", "/api/team/me", b"", now());
    assert_eq!(s.req("GET", "/api/team/me", &[(reqauth::HEADER, &sig)], b"").status, 200);
    let asig = reqauth::sign_request(&admin, "GET", "/api/admin/team", b"", now());
    assert_eq!(s.req("GET", "/api/admin/team", &[(reqauth::HEADER, &asig)], b"").status, 200);
    let s = restart(s, &admin);
    let r = s.req("GET", "/api/team/me", &[(reqauth::HEADER, &sig)], b"");
    assert_eq!((r.status, r.text()), (401, "replayed request".into()));
    let r = s.req("GET", "/api/admin/team", &[(reqauth::HEADER, &asig)], b"");
    assert_eq!((r.status, r.text()), (401, "replayed request".into()));
}

#[test]
fn a_dismissed_device_dropping_its_own_pin_gets_no_claim_back() {
    let (s, admin) = server();
    let (w, r) = writer_and_reader(&s, &admin);
    let any = status::make_token("t", now() + 3600, None, None, false, &admin).unwrap().0;
    let a = generate_key();
    assert_eq!(enroll(&s, &a, "dev-0001", Some("123456789"), &any).status, 200);
    assert_eq!(s.admin(&w, "DELETE", "/api/team/status/dev-0001/link", b"").status, 204);
    // The device releases its own pin and enrols again (also under another uuid) naming a claim.
    assert_eq!(signed(&s, &a, "dev-0001", "DELETE", "/api/status", b"").status, 204);
    for uuid in ["dev-0002", "dev-0003"] {
        assert_eq!(enroll(&s, &a, uuid, Some("555555555"), &any).status, 200);
        let h = team_get(&s, &r, &format!("/api/team/status/{uuid}")).json();
        assert_eq!((&h["claimed_id"], &h["no_claim"]), (&Value::Null, &json!(true)), "{uuid}");
        assert_eq!(signed(&s, &a, uuid, "DELETE", "/api/status", b"").status, 204);
    }
    // Kept across a restart (a `dismissed` line in keys.jsonl).
    let s = restart(s, &admin);
    let (w, r) = writer_and_reader(&s, &admin);
    assert_eq!(enroll(&s, &a, "dev-0004", Some("555555555"), &any).status, 200);
    let h = team_get(&s, &r, "/api/team/status/dev-0004").json();
    assert_eq!((&h["claimed_id"], &h["no_claim"]), (&Value::Null, &json!(true)));
    // A new key with the same token is a new device: it may claim (the boundary the docs state).
    assert_eq!(enroll(&s, &generate_key(), "dev-0005", Some("555555555"), &any).status, 200);
    assert_eq!(team_get(&s, &r, "/api/team/status/dev-0005").json()["claimed_id"], "555555555");
    // Allow a new claim, as before, lets the dismissed device claim again.
    assert_eq!(s.admin(&w, "POST", "/api/team/status/dev-0004/allow-claim", b"").status, 204);
    assert_eq!(enroll(&s, &a, "dev-0004", Some("555555555"), &any).status, 200);
    assert_eq!(team_get(&s, &r, "/api/team/status/dev-0004").json()["claimed_id"], "555555555");
    // A token issued for the ID, on its first use, links a dismissed key that dropped its pin.
    let b = generate_key();
    assert_eq!(enroll(&s, &b, "dev-0006", Some("777777777"), &any).status, 200);
    assert_eq!(s.admin(&w, "DELETE", "/api/team/status/dev-0006/link", b"").status, 204);
    assert_eq!(signed(&s, &b, "dev-0006", "DELETE", "/api/status", b"").status, 204);
    let bound = status::make_token("t", now() + 3600, Some("777777777"), None, false, &admin).unwrap().0;
    assert_eq!(enroll(&s, &b, "dev-0006", None, &bound).status, 200);
    let h = team_get(&s, &r, "/api/team/status/dev-0006").json();
    assert_eq!((&h["tether_id"], &h["no_claim"]), (&json!("777777777"), &json!(false)));
    // An admin or a member removing the pin leaves no such record (the device is gone for good).
    let c = generate_key();
    assert_eq!(enroll(&s, &c, "dev-0007", Some("888888888"), &any).status, 200);
    assert_eq!(s.admin(&w, "DELETE", "/api/team/status/dev-0007/link", b"").status, 204);
    assert_eq!(s.admin(&w, "DELETE", "/api/team/status/dev-0007", b"").status, 204);
    assert_eq!(enroll(&s, &c, "dev-0007", Some("888888888"), &any).status, 200);
    assert_eq!(team_get(&s, &r, "/api/team/status/dev-0007").json()["claimed_id"], "888888888");
}

#[test]
fn a_report_claims_only_when_it_is_accepted() {
    let (s, admin) = server();
    let r = writer_and_reader(&s, &admin).1;
    let any = status::make_token("t", now() + 3600, None, None, false, &admin).unwrap().0;
    let a = generate_key();
    assert_eq!(enroll(&s, &a, "dev-0001", None, &any).status, 200);
    let hb = heartbeat("dev-0001", 1).replace(r#""host""#, r#""tether_id":"123456789","host""#);
    assert_eq!(post(&s, &a, "dev-0001", &hb).text(), "send a full report");
    assert_eq!(team_get(&s, &r, "/api/team/status/dev-0001").json()["claimed_id"], Value::Null, "a refused post claims nothing");
    assert_eq!(post(&s, &a, "dev-0001", &full("dev-0001", 2, "ok")).status, 200);
    assert_eq!(team_get(&s, &r, "/api/team/status/dev-0001").json()["claimed_id"], "123456789");
    assert!(std::fs::read_to_string(s.dir.join("status/keys.jsonl")).unwrap().contains(r#""claimed_id":"123456789""#));
}

// ---- Final review fixes.

#[test]
fn a_dismissed_key_claims_nothing_under_another_uuid() {
    let (s, admin) = server();
    let (w, r) = writer_and_reader(&s, &admin);
    let any = status::make_token("t", now() + 3600, None, None, false, &admin).unwrap().0;
    let a = generate_key();
    assert_eq!(enroll(&s, &a, "dev-0001", Some("123456789"), &any).status, 200);
    assert_eq!(s.admin(&w, "DELETE", "/api/team/status/dev-0001/link", b"").status, 204);
    // The dismissed pin stays on dev-0001; the same key enrols dev-0002 naming a claim.
    assert_eq!(enroll(&s, &a, "dev-0002", Some("555555555"), &any).status, 200);
    let h = team_get(&s, &r, "/api/team/status/dev-0002").json();
    assert_eq!((&h["claimed_id"], &h["no_claim"]), (&Value::Null, &json!(true)));
    // Nor does a report naming one; the summary entry says it is dismissed.
    assert_eq!(post(&s, &a, "dev-0002", &full("dev-0002", 1, "ok").replace("123456789", "555555555")).status, 200);
    assert_eq!(team_get(&s, &r, "/api/team/status/dev-0002").json()["claimed_id"], Value::Null);
    let sum = team_get(&s, &r, "/api/team/status").json();
    assert_eq!((&sum["hosts"][0]["uuid"], &sum["hosts"][0]["no_claim"]), (&json!("dev-0002"), &json!(true)));
    // Across a restart.
    let s = restart(s, &admin);
    let (w, r) = writer_and_reader(&s, &admin);
    assert_eq!(enroll(&s, &a, "dev-0003", Some("555555555"), &any).status, 200);
    assert_eq!(team_get(&s, &r, "/api/team/status/dev-0003").json()["no_claim"], json!(true));
    // Allowing a new claim on every pin of that key ends it.
    for u in ["dev-0001", "dev-0002", "dev-0003"] {
        assert_eq!(s.admin(&w, "POST", &format!("/api/team/status/{u}/allow-claim"), b"").status, 204);
    }
    assert_eq!(enroll(&s, &a, "dev-0004", Some("555555555"), &any).status, 200);
    assert_eq!(team_get(&s, &r, "/api/team/status/dev-0004").json()["claimed_id"], "555555555");
}

#[test]
fn dismissed_keys_are_capped_also_when_loaded() {
    let admin = generate_key();
    let dir = new_dir();
    std::fs::create_dir_all(dir.join("status")).unwrap();
    // 10 001 dismissed keys, one more than the cap: the oldest goes at start.
    let (old, newer) = (generate_key(), generate_key());
    let line = |k: &str, at: u64| json!({"op": "dismissed", "key": k, "at": at}).to_string() + "\n";
    let mut lines = line(&key_to_b64(&old.verifying_key()), now() - 2000);
    lines += &line(&key_to_b64(&newer.verifying_key()), now() - 1500);
    lines += &(0..9_999).map(|i| line(&format!("fake{i:08}"), now() - 1000 + i % 500)).collect::<String>();
    std::fs::write(dir.join("status/keys.jsonl"), lines).unwrap();
    let s = start_in(dir, &admin);
    let (w, r) = writer_and_reader(&s, &admin);
    let any = status::make_token("t", now() + 3600, None, None, false, &admin).unwrap().0;
    assert_eq!(enroll(&s, &old, "dev-0001", Some("111111111"), &any).status, 200);
    assert_eq!(team_get(&s, &r, "/api/team/status/dev-0001").json()["claimed_id"], "111111111", "dropped at load");
    // A new tombstone at the cap drops the oldest left, with a `forgotten` line (kept after a restart).
    let c = generate_key();
    assert_eq!(enroll(&s, &c, "dev-0002", Some("222222222"), &any).status, 200);
    assert_eq!(s.admin(&w, "DELETE", "/api/team/status/dev-0002/link", b"").status, 204);
    assert_eq!(signed(&s, &c, "dev-0002", "DELETE", "/api/status", b"").status, 204);
    // Replaying the file (compacted meanwhile or not) gives the capped set without both dropped keys.
    let mut set = std::collections::HashSet::new();
    for l in std::fs::read_to_string(s.dir.join("status/keys.jsonl")).unwrap().lines() {
        let v: Value = serde_json::from_str(l).unwrap();
        match v["op"].as_str() {
            Some("dismissed") => set.insert(v["key"].as_str().unwrap().to_owned()),
            Some("forgotten") => set.remove(v["key"].as_str().unwrap()),
            _ => false,
        };
    }
    assert!(set.len() == 10_000 && !set.contains(&key_to_b64(&old.verifying_key())) && !set.contains(&key_to_b64(&newer.verifying_key())), "{}", set.len());
    let s = restart(s, &admin);
    let r = writer_and_reader(&s, &admin).1;
    assert_eq!(enroll(&s, &newer, "dev-0003", Some("333333333"), &any).status, 200);
    assert_eq!(team_get(&s, &r, "/api/team/status/dev-0003").json()["claimed_id"], "333333333");
}

#[test]
fn a_report_that_is_not_stored_claims_nothing() {
    let (s, admin) = server();
    let r = writer_and_reader(&s, &admin).1;
    let any = status::make_token("t", now() + 3600, None, None, false, &admin).unwrap().0;
    let a = generate_key();
    assert_eq!(enroll(&s, &a, "dev-0001", None, &any).status, 200);
    // The report cannot be renamed into place (a folder in its way).
    std::fs::create_dir_all(s.dir.join("status/dev-0001.json/x")).unwrap();
    assert_eq!(post(&s, &a, "dev-0001", &full("dev-0001", 1, "ok")).status, 500);
    assert_eq!(team_get(&s, &r, "/api/team/status/dev-0001").json()["claimed_id"], Value::Null);
    assert!(!std::fs::read_to_string(s.dir.join("status/keys.jsonl")).unwrap().contains("claimed_id"));
    std::fs::remove_dir_all(s.dir.join("status/dev-0001.json")).unwrap();
}

#[test]
fn the_monitor_index_expiry_is_a_metric() {
    let (s, k) = server();
    let doc = monitor_doc(&k, "stable", 1, "monitor/windows-x86_64", "tether-monitor-0.1.0.exe", "0.1.0");
    assert_eq!(s.admin(&k, "PUT", "/api/admin/release/stable/monitor.json", &doc).status, 204);
    assert_eq!(metric(&s, r#"tether_api_release_index_expires_timestamp{channel="stable",index="monitor"}"#), Some(2_000_000_000.));
    assert_eq!(metric(&s, r#"tether_api_release_index_expires_timestamp{channel="stable"}"#), None, "the Tether series are not moved");
    let s = restart(s, &k);
    assert_eq!(metric(&s, r#"tether_api_release_index_expires_timestamp{channel="stable",index="monitor"}"#), Some(2_000_000_000.));
}

impl Srv {
    /// What the relay wrote to stderr so far.
    fn journal(&self) -> String {
        std::fs::read_to_string(&self.err).unwrap()
    }
}

/// A token that sets `customer` on the device it enrols.
fn customer_token(admin: &SigningKey, customer: Option<&str>) -> (String, String) {
    status::make_token("t", now() + 3600, None, customer, false, admin).unwrap()
}

#[test]
fn enrol_sets_customer() {
    let (s, admin) = server();
    let (tok, _) = customer_token(&admin, Some("4711"));
    let a = generate_key();
    assert_eq!(enroll(&s, &a, "dev-0001", None, &tok).status, 200);
    assert_eq!(post(&s, &a, "dev-0001", &full("dev-0001", 1, "crit")).status, 200);
    let m = member(&s, &admin);
    assert_eq!(team_get(&s, &m, "/api/team/status").json()["hosts"][0]["customer"], "4711");
    assert_eq!(team_get(&s, &m, "/api/team/status/dev-0001").json()["customer"], "4711");
    assert_eq!(s.admin(&admin, "GET", "/api/admin/status/devices", b"").json()["devices"][0]["customer"], "4711");
}

#[test]
fn token_without_customer_changes_nothing() {
    let (s, admin) = server();
    let a = generate_key();
    assert_eq!(enroll(&s, &a, "dev-0001", None, &customer_token(&admin, Some("4711")).0).status, 200);
    assert_eq!(enroll(&s, &a, "dev-0001", None, &customer_token(&admin, None).0).status, 200);
    let m = member(&s, &admin);
    assert_eq!(team_get(&s, &m, "/api/team/status/dev-0001").json()["customer"], "4711");
    assert!(!s.journal().contains("customer of"), "{}", s.journal());
}

#[test]
fn re_enrol_replaces_customer() {
    let (s, admin) = server();
    let a = generate_key();
    assert_eq!(enroll(&s, &a, "dev-0001", None, &customer_token(&admin, Some("4711")).0).status, 200);
    let (tok, id) = customer_token(&admin, Some("0815"));
    assert_eq!(enroll_signed(&s, &a, &a, "dev-0001", &tok).status, 200);
    let m = member(&s, &admin);
    assert_eq!(team_get(&s, &m, "/api/team/status/dev-0001").json()["customer"], "0815");
    assert_eq!(s.admin(&admin, "GET", "/api/admin/status/devices", b"").json()["devices"][0]["customer"], "0815");
    let line = format!("status: customer of dev-0001 set to 0815 by token {id}");
    assert_eq!(s.journal().matches(&line).count(), 1, "{}", s.journal());
    // The same token again changes nothing and logs nothing more.
    assert_eq!(enroll(&s, &a, "dev-0001", None, &tok).status, 200);
    assert_eq!(s.journal().matches("customer of").count(), 1, "{}", s.journal());
}

fn enroll_body(dev: &SigningKey, uuid: &str, token: &str) -> Vec<u8> {
    json!({"uuid": uuid, "pubkey": key_to_b64(&dev.verifying_key()), "token": token}).to_string().into_bytes()
}

/// An enrolment of `dev` signed with `signer`.
fn enroll_signed(s: &Srv, signer: &SigningKey, dev: &SigningKey, uuid: &str, token: &str) -> Reply {
    signed(s, signer, uuid, "POST", "/api/status/enroll", &enroll_body(dev, uuid, token))
}

const NEEDS_SIG: &str = "enrolment needs the device's signature";

fn enroll_unsigned(s: &Srv, dev: &SigningKey, uuid: &str, tether_id: Option<&str>, token: &str) -> Reply {
    let mut b = json!({"uuid": uuid, "pubkey": key_to_b64(&dev.verifying_key()), "token": token});
    if let Some(t) = tether_id {
        b["tether_id"] = t.into();
    }
    s.req("POST", "/api/status/enroll", &[], b.to_string().as_bytes())
}

#[test]
fn unsigned_enrolment_is_refused() {
    let (s, admin) = server();
    let a = generate_key();
    // A first enrolment: no pin.
    let r = enroll_unsigned(&s, &a, "dev-0001", None, &customer_token(&admin, Some("4711")).0);
    assert_eq!((r.status, r.text().as_str()), (401, NEEDS_SIG));
    assert_eq!(s.admin(&admin, "GET", "/api/admin/status/devices", b"").json()["devices"].as_array().unwrap().len(), 0);
    assert!(!s.dir.join("status/keys.jsonl").exists() || live_pins(&s.dir).is_empty());
    // A re-enrolment: without a customer change, with one, with a first customer, with a claim.
    assert_eq!(enroll(&s, &a, "dev-0001", None, &customer_token(&admin, Some("4711")).0).status, 200);
    let b = generate_key();
    assert_eq!(enroll(&s, &b, "dev-0002", None, &customer_token(&admin, None).0).status, 200);
    for (dev, uuid, customer, claim) in [(&a, "dev-0001", Some("4711"), None), (&a, "dev-0001", None, None), (&a, "dev-0001", Some("0815"), None), (&b, "dev-0002", Some("0815"), None), (&a, "dev-0001", None, Some("123456789"))] {
        let r = enroll_unsigned(&s, dev, uuid, claim, &customer_token(&admin, customer).0);
        assert_eq!((r.status, r.text().as_str()), (401, NEEDS_SIG), "{uuid} {customer:?}");
    }
    let m = member(&s, &admin);
    let h = team_get(&s, &m, "/api/team/status/dev-0001").json();
    assert_eq!((&h["customer"], &h["claimed_id"]), (&json!("4711"), &Value::Null));
    assert_eq!(team_get(&s, &m, "/api/team/status/dev-0002").json()["customer"], Value::Null);
    assert_eq!(live_pins(&s.dir), ["dev-0001", "dev-0002"]);
    assert!(!s.journal().contains("customer of"), "{}", s.journal());
}

#[test]
fn signed_customer_change_is_accepted() {
    let (s, admin) = server();
    let a = generate_key();
    assert_eq!(enroll(&s, &a, "dev-0001", None, &customer_token(&admin, Some("4711")).0).status, 200);
    let (tok, id) = customer_token(&admin, Some("0815"));
    let r = enroll_signed(&s, &a, &a, "dev-0001", &tok);
    assert_eq!(r.status, 200, "{}", r.text());
    let m = member(&s, &admin);
    assert_eq!(team_get(&s, &m, "/api/team/status/dev-0001").json()["customer"], "0815");
    assert!(s.journal().contains(&format!("status: customer of dev-0001 set to 0815 by token {id}")), "{}", s.journal());
}

#[test]
fn customer_change_signed_by_another_key_is_refused() {
    let (s, admin) = server();
    let (a, b) = (generate_key(), generate_key());
    assert_eq!(enroll(&s, &a, "dev-0001", None, &customer_token(&admin, Some("4711")).0).status, 200);
    assert_eq!(enroll(&s, &b, "dev-0002", None, &customer_token(&admin, None).0).status, 200);
    let r = enroll_signed(&s, &b, &a, "dev-0001", &customer_token(&admin, Some("0815")).0);
    assert_eq!((r.status, r.text().as_str()), (401, "signature does not match"));
    // A header naming another uuid than the body.
    let r = signed(&s, &b, "dev-0002", "POST", "/api/status/enroll", &enroll_body(&a, "dev-0001", &customer_token(&admin, Some("0815")).0));
    assert_eq!((r.status, r.text().as_str()), (401, "signature names another uuid"));
    let m = member(&s, &admin);
    assert_eq!(team_get(&s, &m, "/api/team/status/dev-0001").json()["customer"], "4711");
}

#[test]
fn replayed_signed_customer_change_is_refused() {
    let (s, admin) = server();
    let a = generate_key();
    assert_eq!(enroll(&s, &a, "dev-0001", None, &customer_token(&admin, Some("4711")).0).status, 200);
    let body = enroll_body(&a, "dev-0001", &customer_token(&admin, Some("0815")).0);
    let sig = reqauth::sign_device_request(&a, "dev-0001", RELAY, "POST", "/api/status/enroll", &body, now());
    assert_eq!(s.req("POST", "/api/status/enroll", &[(reqauth::HEADER, &sig)], &body).status, 200);
    let other = enroll_body(&a, "dev-0001", &customer_token(&admin, Some("1234")).0);
    let r = s.req("POST", "/api/status/enroll", &[(reqauth::HEADER, &sig)], &other);
    assert_eq!((r.status, r.text().as_str()), (401, "signature does not match"));
    // The very same request again: its nonce is used.
    let r = s.req("POST", "/api/status/enroll", &[(reqauth::HEADER, &sig)], &body);
    assert_eq!((r.status, r.text().as_str()), (401, "replayed request"));
    let m = member(&s, &admin);
    assert_eq!(team_get(&s, &m, "/api/team/status/dev-0001").json()["customer"], "0815");
}

#[test]
fn re_enrol_without_customer_change_works() {
    let (s, admin) = server();
    let a = generate_key();
    assert_eq!(enroll(&s, &a, "dev-0001", None, &customer_token(&admin, Some("4711")).0).status, 200);
    assert_eq!(enroll(&s, &a, "dev-0001", None, &customer_token(&admin, None).0).status, 200);
    assert_eq!(enroll(&s, &a, "dev-0001", None, &customer_token(&admin, Some("4711")).0).status, 200);
    // A signature by another key is refused even so.
    let r = enroll_signed(&s, &generate_key(), &a, "dev-0001", &customer_token(&admin, None).0);
    assert_eq!((r.status, r.text().as_str()), (401, "signature does not match"));
    // A new device may sign its first enrolment with its own key.
    let b = generate_key();
    assert_eq!(enroll_signed(&s, &b, &b, "dev-0002", &customer_token(&admin, Some("0815")).0).status, 200);
}

#[test]
fn signed_first_enrolment_is_accepted() {
    let (s, admin) = server();
    let a = generate_key();
    let r = enroll_signed(&s, &a, &a, "dev-0001", &customer_token(&admin, None).0);
    assert_eq!(r.status, 200, "{}", r.text());
    let devices = s.admin(&admin, "GET", "/api/admin/status/devices", b"").json()["devices"].clone();
    assert_eq!(devices.as_array().unwrap().len(), 1);
    assert_eq!(devices[0]["uuid"], "dev-0001");
}

#[test]
fn first_enrolment_needs_the_key_of_the_request() {
    let (s, admin) = server();
    let (a, b) = (generate_key(), generate_key());
    // The body carries key A, the signature is made with key B: knowing A is not enough.
    let r = enroll_signed(&s, &b, &a, "dev-0001", &customer_token(&admin, Some("4711")).0);
    assert_eq!((r.status, r.text().as_str()), (401, "signature does not match"));
    assert_eq!(s.admin(&admin, "GET", "/api/admin/status/devices", b"").json()["devices"].as_array().unwrap().len(), 0);
    assert!(!s.dir.join("status/keys.jsonl").exists() || live_pins(&s.dir).is_empty());
}

#[test]
fn a_device_cannot_name_its_customer() {
    let (s, admin) = server();
    let a = generate_key();
    let tok = customer_token(&admin, None).0;
    let mut b = json!({"uuid": "dev-0001", "pubkey": key_to_b64(&a.verifying_key()), "token": tok, "customer": "x"});
    assert_eq!(signed(&s, &a, "dev-0001", "POST", "/api/status/enroll", b.to_string().as_bytes()).status, 400);
    b.as_object_mut().unwrap().remove("customer");
    assert_eq!(signed(&s, &a, "dev-0001", "POST", "/api/status/enroll", b.to_string().as_bytes()).status, 200);
    let r = post(&s, &a, "dev-0001", &full("dev-0001", 1, "ok").replacen(r#""v":1,"#, r#""v":1,"customer":"x","#, 1));
    assert_eq!(r.status, 400, "{}", r.text());
    let m = member(&s, &admin);
    assert_eq!(team_get(&s, &m, "/api/team/status/dev-0001").json()["customer"], Value::Null);
}

#[test]
fn old_key_lines_load_without_customer() {
    let admin = generate_key();
    let a = generate_key();
    let dir = new_dir();
    std::fs::create_dir_all(dir.join("status")).unwrap();
    let line = json!({"op": "set", "uuid": "dev-0001", "key": key_to_b64(&a.verifying_key()), "tether_id": null, "label": "t", "enrolled": now()});
    std::fs::write(dir.join("status/keys.jsonl"), format!("{line}\n")).unwrap();
    let s = start_in(dir, &admin);
    assert_eq!(post(&s, &a, "dev-0001", &full("dev-0001", 1, "ok")).status, 200);
    let m = member(&s, &admin);
    let sum = team_get(&s, &m, "/api/team/status").json();
    assert_eq!(sum["hosts"][0]["customer"], Value::Null);
    assert!(sum["hosts"][0].as_object().unwrap().contains_key("customer"));
    // A new pin without a customer writes no `customer` key.
    assert_eq!(enroll(&s, &generate_key(), "dev-0002", None, &customer_token(&admin, None).0).status, 200);
    assert!(!std::fs::read_to_string(s.dir.join("status/keys.jsonl")).unwrap().contains("customer"));
}

#[test]
fn customer_survives_link_unlink_and_allow_claim() {
    let (s, admin) = server();
    let (w, r) = writer_and_reader(&s, &admin);
    let a = generate_key();
    assert_eq!(enroll(&s, &a, "dev-0001", Some("123456789"), &customer_token(&admin, Some("4711")).0).status, 200);
    let customer = || team_get(&s, &r, "/api/team/status/dev-0001").json()["customer"].clone();
    assert_eq!(customer(), "4711");
    assert_eq!(s.admin(&w, "POST", "/api/team/status/dev-0001/link", br#"{"tether_id":"123456789"}"#).status, 204);
    assert_eq!(team_get(&s, &r, "/api/team/status/dev-0001").json()["tether_id"], "123456789");
    assert_eq!(customer(), "4711", "after link");
    assert_eq!(s.admin(&w, "DELETE", "/api/team/status/dev-0001/link", b"").status, 204);
    assert_eq!(customer(), "4711", "after unlink");
    assert_eq!(s.admin(&w, "POST", "/api/team/status/dev-0001/allow-claim", b"").status, 204);
    assert_eq!(customer(), "4711", "after allow-claim");
}

#[test]
fn customer_survives_restart() {
    let (s, admin) = server();
    let a = generate_key();
    assert_eq!(enroll(&s, &a, "dev-0001", None, &customer_token(&admin, Some("4711")).0).status, 200);
    assert_eq!(post(&s, &a, "dev-0001", &full("dev-0001", 1, "ok")).status, 200);
    let s = restart(s, &admin);
    let m = member(&s, &admin);
    assert_eq!(team_get(&s, &m, "/api/team/status").json()["hosts"][0]["customer"], "4711");
    assert!(std::fs::read_to_string(s.dir.join("status/keys.jsonl")).unwrap().contains(r#""customer":"4711""#));
}

// ---- The software list (SBOM): stored per device byte for byte, served to team members.

const PUT_SBOM: &str = "/api/status/sbom";

fn sha16(b: &[u8]) -> String {
    reqauth::body_digest(b)[..16].to_owned()
}

/// A document of exactly `len` bytes (at least 40): a JSON object the relay does not read.
fn sbom_doc(len: usize, fill: char) -> Vec<u8> {
    let head = r#"{"bomFormat":"CycloneDX",  "x":""#;
    format!("{head}{}\"}}", fill.to_string().repeat(len - head.len() - 2)).into_bytes()
}

fn sbom_put(s: &Srv, dev: &SigningKey, uuid: &str, body: &[u8]) -> Reply {
    signed(s, dev, uuid, "PUT", PUT_SBOM, body)
}

/// `dev-0001`..`dev-<n>` pinned with a full report (their keys in order), `prep` on the folder,
/// then the relay started with the software list on, a budget no test reaches unless `env` sets
/// one, and `env`.
fn sbom_server(n: usize, env: &[(&str, &str)], prep: impl FnOnce(&Path)) -> (Srv, SigningKey, Vec<SigningKey>) {
    let (admin, dir) = (generate_key(), new_dir());
    let keys: Vec<SigningKey> = (1..=n).map(|_| generate_key()).collect();
    for (i, k) in keys.iter().enumerate() {
        prep_device(&dir, &format!("dev-{:04}", i + 1), k, now() - 120, 5);
    }
    prep(&dir);
    let mut e = vec![("STATUS_SBOM", "on"), ("STATUS_SBOM_RPS", "1000")];
    e.extend_from_slice(env);
    (spawn_env(dir, &key_to_b64(&admin.verifying_key()), &e), admin, keys)
}

/// A stored document with its record in `seen.json` (accepted at `at`, also the file's mtime, as
/// the relay leaves them), before the relay starts.
fn prep_sbom(dir: &Path, uuid: &str, doc: &[u8], at: u64) {
    let p = dir.join(format!("status/{uuid}.sbom.json"));
    std::fs::write(&p, doc).unwrap();
    std::fs::File::options().write(true).open(&p).unwrap().set_modified(UNIX_EPOCH + std::time::Duration::from_secs(at)).unwrap();
    update_json(&dir.join("status/seen.json"), |m| {
        m.get_mut(uuid).unwrap()["sbom"] = json!({"at": at, "size": doc.len(), "sha": sha16(doc)});
    });
}

fn sbom_file(s: &Srv, uuid: &str) -> Option<Vec<u8>> {
    std::fs::read(s.dir.join(format!("status/{uuid}.sbom.json"))).ok()
}

/// A member's GET of `uuid`'s document, with `If-None-Match` if given.
fn sbom_get(s: &Srv, m: &SigningKey, uuid: &str, inm: Option<&str>) -> Reply {
    let p = format!("/api/team/status/{uuid}/sbom");
    let sig = reqauth::sign_request(m, "GET", &p, b"", now());
    let mut h = vec![(reqauth::HEADER, sig.as_str())];
    h.extend(inm.map(|e| ("If-None-Match", e)));
    s.req("GET", &p, &h, b"")
}

/// A request whose head declares `declared` bytes of body and sends `body`, then closes its side.
fn send_declared(s: &Srv, method: &str, path: &str, headers: &[(&str, &str)], declared: usize, body: &[u8]) -> Reply {
    let mut c = TcpStream::connect(&s.addr).unwrap();
    c.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: t\r\nConnection: close\r\nContent-Length: {declared}\r\n");
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    c.write_all(head.as_bytes()).unwrap();
    let _ = c.write_all(body);
    let _ = c.shutdown(std::net::Shutdown::Write);
    let mut raw = Vec::new();
    let r = c.read_to_end(&mut raw);
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap_or_else(|| panic!("no response head to {declared} declared, {} sent: {r:?}", body.len()));
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let status = head.lines().next().unwrap().split_whitespace().nth(1).unwrap().parse().unwrap();
    Reply { status, body: raw[split + 4..].to_vec(), head }
}

/// Stops `s` (SIGTERM, or SIGKILL with `hard`) and starts the relay again on its folder with `env`.
fn restart_env(mut s: Srv, admin: &SigningKey, env: &[(&str, &str)], hard: bool) -> Srv {
    assert!(Command::new("kill").args([if hard { "-KILL" } else { "-TERM" }, &s.child.id().to_string()]).status().unwrap().success());
    let _ = s.child.wait();
    let dir = s.dir.clone();
    std::mem::forget(s); // its Drop would remove the folder
    spawn_env(dir, &key_to_b64(&admin.verifying_key()), env)
}

#[test]
fn sbom_is_stored_byte_for_byte_and_served_to_members() {
    let (s, admin, k) = sbom_server(2, &[], |_| {});
    let m = member(&s, &admin);
    let doc = br#"{ "bomFormat":"CycloneDX","specVersion":"1.6",  "components":[{"name":"Example \t App"}] }"#;
    let r = sbom_put(&s, &k[0], "dev-0001", doc);
    assert_eq!((r.status, r.text()), (204, String::new()));
    assert_eq!(sbom_file(&s, "dev-0001").unwrap(), doc);
    let r = sbom_get(&s, &m, "dev-0001", None);
    let tag = format!("\"{}\"", sha16(doc));
    assert_eq!((r.status, r.body.as_slice(), r.header("Content-Type"), r.header("ETag")), (200, &doc[..], Some("application/vnd.cyclonedx+json".into()), Some(tag.clone())));
    let r = sbom_get(&s, &m, "dev-0001", Some(&tag));
    assert_eq!((r.status, r.header("ETag")), (304, Some(tag.clone())));
    assert_eq!(sbom_get(&s, &m, "dev-0002", None).status, 404, "none stored");
    assert_eq!(sbom_get(&s, &m, "dev-9999", None).status, 404);
    // The summary entry and the status answer.
    let h = team_get(&s, &m, "/api/team/status").json()["hosts"].clone();
    assert_eq!(h[0]["sbom"]["sha"], sha16(doc));
    assert!(h[0]["sbom"]["at"].as_u64().unwrap() >= now() - 5);
    assert!(h[1].get("sbom").is_none(), "no document, no field");
    let r = post(&s, &k[1], "dev-0002", &heartbeat("dev-0002", 6));
    assert_eq!(r.json(), json!({"releases": 0, "next": 300, "sbom": true}));
    // One accepted upload per device and hour.
    let r = sbom_put(&s, &k[0], "dev-0001", doc);
    let wait: u64 = r.header("Retry-After").unwrap().parse().unwrap();
    assert_eq!((r.status, r.text()), (429, "upload too frequent".into()));
    assert!((3590..=3600).contains(&wait), "{wait}");
    assert_eq!(metric(&s, "tether_api_status_sbom_files"), Some(1.));
    assert_eq!(metric(&s, "tether_api_status_sbom_bytes"), Some(doc.len() as f64));
    // Credentials stay apart: a device key on the team route, a member key on the device routes.
    assert_eq!(signed(&s, &k[0], "dev-0001", "GET", "/api/team/status/dev-0001/sbom", b"").status, 401);
    assert_eq!(s.admin(&m, "PUT", PUT_SBOM, doc).status, 401);
    assert_eq!(s.admin(&m, "DELETE", PUT_SBOM, b"").status, 401);
    assert_eq!(sbom_file(&s, "dev-0001").unwrap(), doc);
    // The record survives a restart, and with it the hourly limit.
    let s = restart_env(s, &admin, &[("STATUS_SBOM", "on")], false);
    let m = member(&s, &admin);
    assert_eq!(sbom_get(&s, &m, "dev-0001", None).header("ETag"), Some(tag));
    assert_eq!(team_get(&s, &m, "/api/team/status").json()["hosts"][0]["sbom"]["sha"], sha16(doc));
    assert_eq!(sbom_put(&s, &k[0], "dev-0001", doc).status, 429);
    assert_eq!(metric(&s, "tether_api_status_sbom_bytes"), Some(doc.len() as f64));
}

#[test]
fn sbom_is_off_by_default_and_status_off_wins() {
    let admin = generate_key();
    let dir = new_dir();
    let a = generate_key();
    prep_device(&dir, "dev-0001", &a, now() - 120, 5);
    prep_sbom(&dir, "dev-0001", b"{}", now() - 7200);
    let s = start_in(dir, &admin);
    let m = member(&s, &admin);
    for r in [sbom_put(&s, &a, "dev-0001", b"{}"), sbom_get(&s, &m, "dev-0001", None)] {
        assert_eq!(r.status, 404);
    }
    assert_eq!(post(&s, &a, "dev-0001", &heartbeat("dev-0001", 6)).json(), json!({"releases": 0, "next": 300}));
    assert!(team_get(&s, &m, "/api/team/status").json()["hosts"][0].get("sbom").is_none(), "a document from before is not shown");
    assert!(sbom_file(&s, "dev-0001").is_some(), "nor removed");
    drop(s);
    let s = spawn_env(new_dir(), &key_to_b64(&admin.verifying_key()), &[("STATUS", "off"), ("STATUS_SBOM", "on")]);
    let m = member(&s, &admin);
    for r in [sbom_put(&s, &a, "dev-0001", b"{}"), signed(&s, &a, "dev-0001", "DELETE", PUT_SBOM, b""), sbom_get(&s, &m, "dev-0001", None)] {
        assert_eq!((r.status, r.text()), (503, "status reporting is off".into()));
    }
}

/// A device's own DELETE is honoured while the feature is off, so a list stored while it was on
/// does not come back when it is switched on again; PUT and GET stay 404.
#[test]
fn sbom_delete_is_honoured_while_the_feature_is_off() {
    let (s, admin, k) = sbom_server(2, &[], |_| {});
    let doc = sbom_doc(500, 'z');
    assert_eq!(sbom_put(&s, &k[0], "dev-0001", &doc).status, 204);
    assert_eq!(sbom_put(&s, &k[1], "dev-0002", &doc).status, 204);
    let s = restart_env(s, &admin, &[], false);
    let m = member(&s, &admin);
    assert_eq!(sbom_put(&s, &k[0], "dev-0001", &doc).status, 404);
    assert_eq!(sbom_get(&s, &m, "dev-0001", None).status, 404);
    let r = signed(&s, &k[0], "dev-0001", "DELETE", PUT_SBOM, b"");
    assert_eq!((r.status, r.text()), (204, String::new()));
    assert!(sbom_file(&s, "dev-0001").is_none());
    assert_eq!(signed(&s, &k[0], "dev-0001", "DELETE", PUT_SBOM, b"").status, 204, "also when none is left");
    assert_eq!(signed(&s, &generate_key(), "dev-0002", "DELETE", PUT_SBOM, b"").status, 401);
    assert_eq!(sbom_file(&s, "dev-0002").unwrap(), doc, "another device's list is kept");
    let s = restart_env(s, &admin, &[("STATUS_SBOM", "on")], false);
    let m = member(&s, &admin);
    let hosts = team_get(&s, &m, "/api/team/status").json()["hosts"].clone();
    let host = |u: &str| hosts.as_array().unwrap().iter().find(|h| h["uuid"] == u).unwrap().clone();
    assert!(host("dev-0001").get("sbom").is_none(), "{hosts}");
    assert_eq!(host("dev-0002")["sbom"]["sha"], sha16(&doc));
    assert_eq!(sbom_get(&s, &m, "dev-0001", None).status, 404);
    assert_eq!(metric(&s, "tether_api_status_sbom_files"), Some(1.));
}

#[test]
fn sbom_body_rules() {
    let (s, _admin, k) = sbom_server(12, &[], |_| {});
    // Not an object, or not JSON: 400 and nothing stored (each from its own device: a refusal
    // after a valid signature uses the device's hour).
    for (i, body) in [&b"[1,2]"[..], b"\"x\"", b"not json", b"{\"a\":1} {}", b"{\"a\":1", b""].into_iter().enumerate() {
        let u = format!("dev-{:04}", i + 7);
        let r = sbom_put(&s, &k[i + 6], &u, body);
        assert_eq!((r.status, r.text()), (400, "not a JSON object".into()), "{}", String::from_utf8_lossy(body));
        assert!(sbom_file(&s, &u).is_none());
    }
    assert_eq!(sbom_put(&s, &k[0], "dev-0001", b"{}").status, 204);
    // 512 KiB of valid JSON that is no software list: stored.
    let big = sbom_doc(512 << 10, 'a');
    assert_eq!(sbom_put(&s, &k[1], "dev-0002", &big).status, 204);
    assert_eq!(sbom_file(&s, "dev-0002").unwrap(), big);
    // One byte over: 413 from Content-Length, before the body is read (none is sent).
    let sig = reqauth::sign_device_request(&k[2], "dev-0003", RELAY, "PUT", PUT_SBOM, b"", now());
    let r = send_declared(&s, "PUT", PUT_SBOM, &[(reqauth::HEADER, &sig)], (512 << 10) + 1, b"");
    assert_eq!((r.status, r.text()), (413, "body too large".into()));
    // Deep nesting: walked without recursion (bounded by the body), stored; unbalanced: 400.
    let deep = format!("{{\"a\":{}{}}}", "[".repeat(200_000), "]".repeat(200_000));
    assert_eq!(sbom_put(&s, &k[2], "dev-0003", deep.as_bytes()).status, 204);
    let open = format!("{{\"a\":{}}}", "[".repeat(200_000));
    assert_eq!(sbom_put(&s, &k[3], "dev-0004", open.as_bytes()).status, 400);
    // Content-Length that lies: more sent than declared, only the declared bytes are read.
    let doc = sbom_doc(100, 'b');
    let sig = reqauth::sign_device_request(&k[4], "dev-0005", RELAY, "PUT", PUT_SBOM, &doc, now());
    let mut more = doc.clone();
    more.extend_from_slice(b"GARBAGE{[");
    let r = send_declared(&s, "PUT", PUT_SBOM, &[(reqauth::HEADER, &sig)], doc.len(), &more);
    assert_eq!(r.status, 204);
    assert_eq!(sbom_file(&s, "dev-0005").unwrap(), doc);
    // Fewer sent than declared: the read fails, nothing stored. (Up to 1 KiB declared, tiny_http
    // reads the body itself and drops such a request unanswered.)
    let doc = sbom_doc(5000, 'b');
    let sig = reqauth::sign_device_request(&k[5], "dev-0006", RELAY, "PUT", PUT_SBOM, &doc, now());
    let r = send_declared(&s, "PUT", PUT_SBOM, &[(reqauth::HEADER, &sig)], doc.len() + 50, &doc);
    assert_eq!((r.status, r.text()), (400, "request body incomplete".into()));
    assert!(sbom_file(&s, "dev-0006").is_none());
    assert_eq!(sbom_put(&s, &k[5], "dev-0006", &doc).status, 204, "the hour not used up");
}

#[test]
fn sbom_forged_uploads_do_not_use_up_the_hour_and_the_budget_holds() {
    let (s, _admin, k) = sbom_server(2, &[], |_| {});
    let doc = sbom_doc(200, 'c');
    let stranger = generate_key();
    // Unsigned, signed by another key, and signed by dev-0002 with a header naming dev-0001.
    assert_eq!(s.req("PUT", PUT_SBOM, &[], &doc).status, 401);
    let r = sbom_put(&s, &stranger, "dev-0001", &doc);
    assert_eq!((r.status, r.text()), (401, "signature does not match".into()));
    assert_eq!(sbom_put(&s, &k[1], "dev-0001", &doc).status, 401);
    let r = sbom_put(&s, &stranger, "dev-0099", &doc);
    assert_eq!((r.status, r.text()), (401, "unknown device".into()));
    assert!(sbom_file(&s, "dev-0001").is_none());
    assert_eq!(sbom_put(&s, &k[0], "dev-0001", &doc).status, 204, "the device's own allowance is untouched");
    // A signed request refused for its body is not taken again within the hour, a replay
    // included (stopped before the signature is looked at).
    let sig = reqauth::sign_device_request(&k[1], "dev-0002", RELAY, "PUT", PUT_SBOM, b"[1]", now());
    assert_eq!(s.req("PUT", PUT_SBOM, &[(reqauth::HEADER, &sig)], b"[1]").status, 400);
    let r = s.req("PUT", PUT_SBOM, &[(reqauth::HEADER, &sig)], b"[1]");
    assert_eq!((r.status, r.text()), (429, "upload too frequent".into()));
    drop(s);
    // The global budget: one upload per second, another within that second is refused.
    let (s, _admin, k) = sbom_server(3, &[("STATUS_SBOM_RPS", "1")], |_| {});
    let codes: Vec<(u16, String, Option<String>)> = (0..3)
        .map(|i| {
            let r = sbom_put(&s, &k[i], &format!("dev-{:04}", i + 1), &doc);
            (r.status, r.text(), r.header("Retry-After"))
        })
        .collect();
    assert!(codes.iter().filter(|c| c.0 == 204).count() <= 2, "{codes:?}");
    assert!(codes.contains(&(429, "busy".into(), Some("60".into()))), "{codes:?}");
}

#[test]
fn sbom_total_cap() {
    // 1 MiB in all; dev-0001 holds 700 KB, dev-0003 100 KB, from before.
    let (big, small) = (sbom_doc(700_000, 'd'), sbom_doc(100_000, 'e'));
    let (s, _admin, k) = sbom_server(4, &[("STATUS_SBOM_MAX_KB", "1024"), ("STATUS_SBOM_MAX_TOTAL_MB", "1")], |d| {
        prep_sbom(d, "dev-0001", &big, now() - 7200);
        prep_sbom(d, "dev-0003", &small, now() - 7200);
    });
    let r = sbom_put(&s, &k[1], "dev-0002", &sbom_doc(300_000, 'f'));
    assert_eq!((r.status, r.text()), (507, "sbom store full (STATUS_SBOM_MAX_TOTAL_MB)".into()));
    assert!(sbom_file(&s, "dev-0002").is_none());
    // A larger replacement that does not fit is refused; the previous document stays.
    assert_eq!(sbom_put(&s, &k[2], "dev-0003", &sbom_doc(400_000, 'g')).status, 507);
    assert_eq!(sbom_file(&s, "dev-0003").unwrap(), small);
    // One that is not larger is taken; then there is room for another device. dev-0002's 507
    // used its hour.
    let smaller = sbom_doc(500_000, 'h');
    assert_eq!(sbom_put(&s, &k[0], "dev-0001", &smaller).status, 204);
    assert_eq!(sbom_file(&s, "dev-0001").unwrap(), smaller);
    assert_eq!(metric(&s, "tether_api_status_sbom_bytes"), Some(600_000.));
    assert_eq!(sbom_put(&s, &k[1], "dev-0002", &sbom_doc(300_000, 'f')).status, 429);
    assert_eq!(sbom_put(&s, &k[3], "dev-0004", &sbom_doc(300_000, 'f')).status, 204);
    assert_eq!(metric(&s, "tether_api_status_sbom_files"), Some(3.));
    assert_eq!(metric(&s, "tether_api_status_sbom_bytes"), Some(900_000.));
}

#[test]
fn sbom_delete_and_lifetime() {
    let doc = sbom_doc(1000, 'i');
    let (w, r) = (generate_key(), generate_key());
    let (s, admin, k) = sbom_server(7, &[], |d| {
        for i in 1..=7 {
            prep_sbom(d, &format!("dev-{i:04}"), &doc, now() - 7200);
        }
        // dev-0005 linked, dev-0006 claiming an ID; dev-0007 not seen for 181 days.
        let old = now() - 181 * 86_400;
        update_json(&d.join("status/keys.json"), |m| {
            m.get_mut("dev-0005").unwrap()["tether_id"] = "123456789".into();
            m.get_mut("dev-0005").unwrap()["linked_by"] = "W".into();
            m.get_mut("dev-0006").unwrap()["tether_id"] = "987654321".into();
            m.get_mut("dev-0007").unwrap()["enrolled"] = old.into();
        });
        update_json(&d.join("status/seen.json"), |m| m.get_mut("dev-0007").unwrap()["last_seen"] = old.into());
    });
    assert!(sbom_file(&s, "dev-0007").is_none(), "expired with its pin");
    let roster = json!({"members": [{"name": "W", "key": key_to_b64(&w.verifying_key()), "write": true, "audit": false},
        {"name": "R", "key": key_to_b64(&r.verifying_key()), "write": false, "audit": false}]});
    assert_eq!(s.admin(&admin, "PUT", "/api/admin/team", roster.to_string().as_bytes()).status, 204);
    // The device replaces, then deletes it: 204, also when none is left. Deleting frees no upload.
    assert_eq!(sbom_put(&s, &k[0], "dev-0001", b"{}").status, 204);
    assert_eq!(signed(&s, &k[0], "dev-0001", "DELETE", PUT_SBOM, b"").status, 204);
    assert!(sbom_file(&s, "dev-0001").is_none());
    assert_eq!(sbom_get(&s, &r, "dev-0001", None).status, 404);
    assert_eq!(signed(&s, &k[0], "dev-0001", "DELETE", PUT_SBOM, b"").status, 204);
    assert_eq!(sbom_put(&s, &k[0], "dev-0001", b"{}").status, 429);
    assert_eq!(signed(&s, &generate_key(), "dev-0001", "DELETE", PUT_SBOM, b"").status, 401);
    // Removal of the pin by the device, an admin and a member takes the file along.
    assert_eq!(signed(&s, &k[1], "dev-0002", "DELETE", "/api/status", b"").status, 204);
    assert_eq!(s.admin(&admin, "DELETE", "/api/admin/status/dev-0003", b"").status, 204);
    assert_eq!(s.admin(&w, "DELETE", "/api/team/status/dev-0004", b"").status, 204);
    for u in ["dev-0002", "dev-0003", "dev-0004"] {
        assert!(sbom_file(&s, u).is_none(), "{u}");
    }
    // Unlink and dismiss keep it.
    assert_eq!(s.admin(&w, "DELETE", "/api/team/status/dev-0005/link", b"").status, 204);
    assert_eq!(s.admin(&w, "DELETE", "/api/team/status/dev-0006/link", b"").status, 204);
    for u in ["dev-0005", "dev-0006"] {
        assert_eq!(sbom_file(&s, u).unwrap(), doc, "{u}");
        assert_eq!(sbom_get(&s, &r, u, None).status, 200, "{u}");
    }
    assert_eq!(metric(&s, "tether_api_status_sbom_files"), Some(2.));
    assert_eq!(metric(&s, "tether_api_status_sbom_bytes"), Some(2000.));
}

#[test]
fn sbom_record_is_rebuilt_after_a_hard_kill() {
    let (doc, old, new) = (sbom_doc(5000, 'j'), sbom_doc(3000, 'k'), sbom_doc(4000, 'l'));
    // dev-0002 has a record but its file is gone: the record is dropped. dev-0003 has a list
    // from two hours ago and replaces it before the kill.
    let (s, admin, k) = sbom_server(3, &[], |d| {
        prep_sbom(d, "dev-0002", b"{}", now() - 7200);
        std::fs::remove_file(d.join("status/dev-0002.sbom.json")).unwrap();
        prep_sbom(d, "dev-0003", &old, now() - 7200);
    });
    assert_eq!(sbom_put(&s, &k[0], "dev-0001", &doc).status, 204);
    assert_eq!(sbom_put(&s, &k[2], "dev-0003", &new).status, 204);
    // seen.json is written at most every 5 minutes: after SIGKILL it holds the records from before.
    let s = restart_env(s, &admin, &[("STATUS_SBOM", "on")], true);
    let seen: Value = serde_json::from_slice(&std::fs::read(s.dir.join("status/seen.json")).unwrap()).unwrap();
    assert!(seen["dev-0001"].get("sbom").is_none());
    assert_eq!(seen["dev-0003"]["sbom"]["sha"], sha16(&old), "the stale record");
    let m = member(&s, &admin);
    let new_tag = Some(format!("\"{}\"", sha16(&new)));
    wait_for("the records rebuilt from the files", || sbom_get(&s, &m, "dev-0001", None).status == 200 && sbom_get(&s, &m, "dev-0003", None).header("ETag") == new_tag);
    assert_eq!(sbom_get(&s, &m, "dev-0001", None).header("ETag"), Some(format!("\"{}\"", sha16(&doc))));
    let r = sbom_get(&s, &m, "dev-0003", Some(&format!("\"{}\"", sha16(&old))));
    assert_eq!((r.status, r.body), (200, new.clone()), "the old tag no longer matches");
    let h = team_get(&s, &m, "/api/team/status").json()["hosts"].clone();
    assert_eq!((&h[0]["sbom"]["sha"], &h[2]["sbom"]["sha"]), (&json!(sha16(&doc)), &json!(sha16(&new))));
    assert!(h[1].get("sbom").is_none());
    assert_eq!(sbom_get(&s, &m, "dev-0002", None).status, 404);
    assert_eq!(metric(&s, "tether_api_status_sbom_files"), Some(2.));
    assert_eq!(metric(&s, "tether_api_status_sbom_bytes"), Some(9000.));
}

#[test]
fn sbom_budget_is_not_held_by_requests_without_the_key() {
    // Room for exactly the three honest uploads below, should they fall into one second.
    let (s, _admin, k) = sbom_server(3, &[("STATUS_SBOM_RPS", "3")], |_| {});
    let doc = sbom_doc(200, 'm');
    let stranger = generate_key();
    let from = |key: &SigningKey, uuid: &str, ip: &str| {
        let sig = reqauth::sign_device_request(key, uuid, RELAY, "PUT", PUT_SBOM, &doc, now());
        s.req("PUT", PUT_SBOM, &[(reqauth::HEADER, &sig), ("X-Forwarded-For", ip)], &doc)
    };
    // An unknown uuid, from many addresses: refused before the budget.
    for i in 0..200 {
        let r = from(&stranger, "dev-0099", &format!("198.18.{}.{}", i / 250, i % 250));
        assert_eq!((r.status, r.text()), (401, "unknown device".into()), "{i}");
    }
    assert_eq!(from(&k[0], "dev-0001", "192.0.2.1").status, 204, "the budget is free");
    // A pinned uuid with a bad signature, from one address: each attempt gives its slot back,
    // and after 60 failures the address is cut off before the budget.
    for i in 0..200 {
        let r = from(&stranger, "dev-0002", "203.0.113.9");
        let want = if i < 60 { (401, "signature does not match") } else { (429, "too many failed requests") };
        assert_eq!((r.status, r.text().as_str()), want, "forged upload {i}");
    }
    assert_eq!(from(&k[2], "dev-0003", "192.0.2.3").status, 204, "another device still uploads");
    assert_eq!(from(&k[1], "dev-0002", "192.0.2.2").status, 204, "the victim's hour is untouched");
}

#[test]
fn sbom_put_needs_a_content_length() {
    let (s, _admin, k) = sbom_server(1, &[], |_| {});
    let sig = reqauth::sign_device_request(&k[0], "dev-0001", RELAY, "PUT", PUT_SBOM, b"{}", now());
    let mut c = TcpStream::connect(&s.addr).unwrap();
    c.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
    let head = format!("PUT {PUT_SBOM} HTTP/1.1\r\nHost: t\r\nConnection: close\r\nTransfer-Encoding: chunked\r\n{}: {sig}\r\n\r\n2\r\n{{}}\r\n0\r\n\r\n", reqauth::HEADER);
    c.write_all(head.as_bytes()).unwrap();
    let mut raw = Vec::new();
    let _ = c.read_to_end(&mut raw);
    let raw = String::from_utf8_lossy(&raw);
    assert!(raw.starts_with("HTTP/1.1 411"), "{raw}");
    assert!(sbom_file(&s, "dev-0001").is_none());
    assert_eq!(sbom_put(&s, &k[0], "dev-0001", b"{}").status, 204, "nothing used up");
}

#[test]
fn sbom_bodies_that_do_not_arrive_cost_the_sender_not_the_budget() {
    // One upload per second for all devices: a held slot would show at once.
    let (s, _admin, k) = sbom_server(2, &[("STATUS_SBOM_RPS", "1")], |_| {});
    let stranger = generate_key();
    // Heads naming a pinned uuid, 5000 bytes declared (over tiny_http's own 1 KiB read), 10 sent.
    for i in 0..200 {
        let sig = reqauth::sign_device_request(&stranger, "dev-0001", RELAY, "PUT", PUT_SBOM, b"", now());
        let r = send_declared(&s, "PUT", PUT_SBOM, &[(reqauth::HEADER, &sig), ("X-Forwarded-For", "203.0.113.7")], 5000, b"{\"a\":\"xxx\"");
        let want = if i < 60 { (400, "request body incomplete") } else { (429, "too many failed requests") };
        assert_eq!((r.status, r.text().as_str()), want, "short body {i}");
    }
    let doc = sbom_doc(300, 'n');
    let sig = reqauth::sign_device_request(&k[1], "dev-0002", RELAY, "PUT", PUT_SBOM, &doc, now());
    assert_eq!(s.req("PUT", PUT_SBOM, &[(reqauth::HEADER, &sig), ("X-Forwarded-For", "192.0.2.2")], &doc).status, 204, "the budget is free");
    let sig = reqauth::sign_device_request(&k[0], "dev-0001", RELAY, "PUT", PUT_SBOM, &doc, now());
    let r = s.req("PUT", PUT_SBOM, &[(reqauth::HEADER, &sig), ("X-Forwarded-For", "192.0.2.1")], &doc);
    assert!(matches!(r.status, 204 | 429) && r.text() != "upload too frequent", "the named device's hour is untouched: {} {}", r.status, r.text());
}

#[test]
fn sbom_a_refused_upload_uses_the_device_hour() {
    let (s, _admin, k) = sbom_server(2, &[("STATUS_SBOM_RPS", "2")], |_| {});
    assert_eq!(sbom_put(&s, &k[0], "dev-0001", b"not json").status, 400);
    let r = sbom_put(&s, &k[0], "dev-0001", b"still not json");
    assert_eq!((r.status, r.text()), (429, "upload too frequent".into()));
    assert!(r.header("Retry-After").unwrap().parse::<u64>().unwrap() > 3500);
    assert_eq!(sbom_put(&s, &k[1], "dev-0002", b"{}").status, 204, "another device gets the budget");
}

#[test]
fn sbom_scan_puts_back_touched_mtimes_and_does_not_hold_up_revocations() {
    const N: usize = 200;
    let (s, admin, _k) = sbom_server(N, &[], |d| {
        for i in 1..=N {
            let u = format!("dev-{i:04}");
            prep_sbom(d, &u, format!("{{\"n\":{i}}}").as_bytes(), now() - 7200);
            // A restore that did not keep mtimes.
            let p = d.join(format!("status/{u}.sbom.json"));
            std::fs::File::options().write(true).open(&p).unwrap().set_modified(SystemTime::now()).unwrap();
        }
    });
    let scans = |s: &Srv| std::fs::read_to_string(&s.err).unwrap().lines().filter(|l| l.contains("software lists checked")).map(str::to_owned).collect::<Vec<_>>();
    // The start-up scan reads 200 files with a pause each (seconds); a revocation does not wait for it.
    let r = revoke(&s, &admin, "abcdefghABCDEFGH", now() + 3600);
    assert_eq!(r.status, 204);
    assert!(scans(&s).is_empty(), "answered while the scan runs");
    wait_for("the scan", || !scans(&s).is_empty());
    assert_eq!(scans(&s), [format!("status: software lists checked: {N} files, {N} read again, 0 records rebuilt")]);
    // The files have the recorded times back.
    let seen: Value = serde_json::from_slice(&std::fs::read(s.dir.join("status/seen.json")).unwrap()).unwrap();
    for i in 1..=N {
        let u = format!("dev-{i:04}");
        let m = std::fs::metadata(s.dir.join(format!("status/{u}.sbom.json"))).unwrap().modified().unwrap();
        assert_eq!(m.duration_since(UNIX_EPOCH).unwrap().as_secs(), seen[&u]["sbom"]["at"].as_u64().unwrap(), "{u}");
    }
    // The next scan reads nothing.
    let s = restart_env(s, &admin, &[("STATUS_SBOM", "on")], false);
    wait_for("the second scan", || scans(&s).len() == 2);
    assert_eq!(scans(&s)[1], format!("status: software lists checked: {N} files, 0 read again, 0 records rebuilt"));
}

#[test]
fn sbom_settings_must_be_numbers() {
    let dir = new_dir();
    let err = dir.with_extension("err");
    let st = Command::new(env!("CARGO_BIN_EXE_tether-api"))
        .args(["--data", dir.to_str().unwrap(), "--listen", "127.0.0.1:0"])
        .env("STATUS_SBOM_MAX_KB", "lots")
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&err).unwrap())
        .status()
        .unwrap();
    assert!(!st.success());
    assert!(std::fs::read_to_string(&err).unwrap().contains("STATUS_SBOM_MAX_KB: not a number"));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&err);
}

// ---- Enrolment with a Tether proof: a grant (a token with `tether_proof`), the ID's key from hbbs.

const TID: &str = "123456789";

/// A stub hbbs health port: the bytes `answer` gives for the request's path are sent back as they
/// are. Its URL and the number of requests it got.
fn stub_hbbs(answer: impl Fn(&str) -> Vec<u8> + Send + Sync + 'static) -> (String, Arc<AtomicU32>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let (answer, hits) = (Arc::new(answer), Arc::new(AtomicU32::new(0)));
    let h = hits.clone();
    std::thread::spawn(move || {
        for mut c in l.incoming().flatten() {
            let (answer, h) = (answer.clone(), h.clone());
            std::thread::spawn(move || {
                let mut buf = [0u8; 1024];
                let n = c.read(&mut buf).unwrap_or(0);
                h.fetch_add(1, Ordering::SeqCst);
                let req = String::from_utf8_lossy(&buf[..n]).into_owned();
                let _ = c.write_all(&answer(req.split_whitespace().nth(1).unwrap_or("")));
            });
        }
    });
    (url, hits)
}

fn http(status: &str, body: &str) -> Vec<u8> {
    format!("HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).into_bytes()
}

/// As hbbs answers: `tether`'s key for `id`, 404 for any other ID.
fn hbbs_knowing(id: &str, tether: &SigningKey) -> (String, Arc<AtomicU32>) {
    let (path, key) = (format!("/peer-pk/{id}"), key_to_b64(&tether.verifying_key()));
    stub_hbbs(move |p| if p == path { http("200 OK", &format!("{key}\n")) } else { http("404 Not Found", "not found\n") })
}

/// A URL nobody listens on.
fn hbbs_down() -> String {
    format!("http://{}", TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap())
}

fn proof_server(admin: &SigningKey, hbbs: &str) -> Srv {
    spawn_env(new_dir(), &key_to_b64(&admin.verifying_key()), &[("HBBS_PEER_URL", hbbs)])
}

fn grant(admin: &SigningKey, customer: Option<&str>) -> (String, String) {
    status::make_token("grant", now() + 3600, None, customer, true, admin).unwrap()
}

/// What Tether on the machine signs with its identity key `tether`.
fn proof(tether: &SigningKey, id: &str, uuid: &str, dev: &SigningKey) -> String {
    status::sign_agent_link(&tether.to_bytes(), id, uuid, &key_to_b64(&dev.verifying_key()))
}

fn enroll_proof(s: &Srv, dev: &SigningKey, uuid: &str, id: Option<&str>, token: &str, proof: Option<&str>) -> Reply {
    let mut b = json!({"uuid": uuid, "pubkey": key_to_b64(&dev.verifying_key()), "token": token});
    if let Some(t) = id {
        b["tether_id"] = t.into();
    }
    if let Some(p) = proof {
        b["tether_proof"] = p.into();
    }
    signed(s, dev, uuid, "POST", "/api/status/enroll", b.to_string().as_bytes())
}

#[test]
fn a_grant_with_a_proof_links_the_id_hbbs_holds() {
    let (admin, tether) = (generate_key(), generate_key());
    let (url, hits) = hbbs_knowing(TID, &tether);
    let s = proof_server(&admin, &url);
    let (g, gid) = grant(&admin, Some("cust-1"));
    let a = generate_key();
    let p = proof(&tether, TID, "dev-0001", &a);
    let r = enroll_proof(&s, &a, "dev-0001", Some(TID), &g, Some(&p));
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json(), json!({"releases": 0, "next": 300}));
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    let r = enroll_proof(&s, &a, "dev-0001", Some(TID), &g, Some(&p));
    assert_eq!((r.status, r.json()), (200, json!({"releases": 0, "next": 300})), "idempotent");
    let (w, _) = writer_and_reader(&s, &admin);
    let h = team_get(&s, &w, "/api/team/status/dev-0001").json();
    assert_eq!((&h["tether_id"], &h["claimed_id"], &h["linked_by"], &h["linked_token"], &h["linked_proof"], &h["customer"]), (&json!(TID), &Value::Null, &Value::Null, &json!(gid), &json!(true), &json!("cust-1")), "{h}");
    assert!(h["linked_at"].as_u64().unwrap() >= now() - 5);
    assert_eq!(post(&s, &a, "dev-0001", &full("dev-0001", 1, "ok")).status, 200);
    let host = team_get(&s, &w, "/api/team/status").json()["hosts"][0].clone();
    assert_eq!((&host["tether_id"], &host["linked_proof"]), (&json!(TID), &json!(true)), "{host}");
    // The journal names the link once, never the grant or the proof.
    let log = std::fs::read_to_string(&s.err).unwrap();
    assert_eq!(log.matches(&format!("status: dev-0001 linked to {TID} by Tether proof (token {gid})")).count(), 1, "{log}");
    assert!(!log.contains(&g) && !log.contains(&p), "{log}");
    // Not single-use, but the ID is held: another uuid with a valid proof for it is refused.
    let b = generate_key();
    let r = enroll_proof(&s, &b, "dev-0002", Some(TID), &g, Some(&proof(&tether, TID, "dev-0002", &b)));
    assert_eq!((r.status, r.text()), (409, "tether_id is enrolled with another key".into()));
    // keys.jsonl carries no `linked_proof` field (a relay of 0.6 locks its store on an unknown
    // field): the grant is stored as `linked_token` `proof:<grant id>`, every line in 0.6's form.
    let keys = std::fs::read_to_string(s.dir.join("status/keys.jsonl")).unwrap();
    assert!(!keys.contains("linked_proof") && keys.contains(&format!("\"linked_token\":\"proof:{gid}\"")), "{keys}");
    for l in keys.lines() {
        serde_json::from_str::<OldKeyLine>(l).unwrap_or_else(|e| panic!("{l}: {e}"));
    }
    // After a restart the link reads exactly as before it.
    let s = restart(s, &admin);
    let h = team_get(&s, &w, "/api/team/status/dev-0001").json();
    assert_eq!((&h["tether_id"], &h["linked_token"], &h["linked_proof"]), (&json!(TID), &json!(gid), &json!(true)), "{h}");
    let host = team_get(&s, &w, "/api/team/status").json()["hosts"][0].clone();
    assert_eq!((&host["tether_id"], &host["linked_proof"]), (&json!(TID), &json!(true)), "{host}");
    let d = s.admin(&admin, "GET", "/api/admin/status/devices", b"").json()["devices"][0].clone();
    assert_eq!(d["linked_token"], json!(gid), "{d}");
}

/// `keys.jsonl` lines as a relay of 0.6 parses them (its `KeyLine`, unknown fields refused).
#[allow(dead_code)]
#[derive(serde::Deserialize)]
#[serde(tag = "op", rename_all = "lowercase", deny_unknown_fields)]
enum OldKeyLine {
    Pin { uuid: String, key: String, tether_id: Option<String>, label: String, enrolled: u64 },
    Unpin { uuid: String },
    Set {
        uuid: String,
        key: String,
        tether_id: Option<String>,
        label: String,
        enrolled: u64,
        #[serde(default)]
        token: Option<String>,
        #[serde(default)]
        claimed_id: Option<String>,
        #[serde(default)]
        linked_by: Option<String>,
        #[serde(default)]
        linked_at: Option<u64>,
        #[serde(default)]
        linked_token: Option<String>,
        #[serde(default)]
        no_claim: bool,
        #[serde(default)]
        customer: Option<String>,
    },
    Used { token: String, uuid: String, exp: u64 },
    Dismissed { key: String, at: u64 },
    Forgotten { key: String },
}

#[test]
fn stored_proof_links_and_hand_edited_ones() {
    let admin = generate_key();
    let dir = new_dir();
    std::fs::create_dir_all(dir.join("status")).unwrap();
    let pk = || key_to_b64(&generate_key().verifying_key());
    let set = |uuid: &str, tid: &str, lt: &str| json!({"op": "set", "uuid": uuid, "key": pk(), "tether_id": tid, "label": "t", "enrolled": now(), "linked_at": now(), "linked_token": lt}).to_string() + "\n";
    // A proof link, a token's link, and hand-edited ones: `proof:` alone, a bad id after it.
    let lines = [set("dev-0001", "111111111", "proof:abcdefghABCDEFGH"), set("dev-0002", "222222222", "abcdefghABCDEFGH"), set("dev-0003", "333333333", "proof:"), set("dev-0004", "444444444", "proof:bad-id!"), set("dev-0005", "555555555", "proof:short")];
    for l in &lines {
        serde_json::from_str::<OldKeyLine>(l.trim()).unwrap();
    }
    std::fs::write(dir.join("status/keys.jsonl"), lines.concat()).unwrap();
    let s = start_in(dir, &admin);
    let (w, _) = writer_and_reader(&s, &admin);
    let link = |u: &str| {
        let h = team_get(&s, &w, &format!("/api/team/status/{u}")).json();
        (h["linked_token"].clone(), h["linked_proof"].clone())
    };
    assert_eq!(link("dev-0001"), (json!("abcdefghABCDEFGH"), json!(true)));
    assert_eq!(link("dev-0002"), (json!("abcdefghABCDEFGH"), Value::Null));
    // Not a proof link: the text stays as written (as any hand-edited value), the store open.
    assert_eq!(link("dev-0003"), (json!("proof:"), Value::Null));
    assert_eq!(link("dev-0004"), (json!("proof:bad-id!"), Value::Null));
    assert_eq!(link("dev-0005"), (json!("proof:short"), Value::Null));
    let tok = status::make_token("t", now() + 3600, None, None, false, &admin).unwrap().0;
    assert_eq!(enroll(&s, &generate_key(), "dev-0009", None, &tok).status, 200, "store not locked");
}

#[test]
fn a_token_link_is_unchanged_across_a_restart() {
    let (s, admin) = server();
    let (tok, id) = status::make_token("t", now() + 3600, Some(TID), None, false, &admin).unwrap();
    assert_eq!(enroll(&s, &generate_key(), "dev-0001", Some(TID), &tok).status, 200);
    let keys = std::fs::read_to_string(s.dir.join("status/keys.jsonl")).unwrap();
    assert!(keys.contains(&format!("\"linked_token\":\"{id}\"")), "{keys}");
    let s = restart(s, &admin);
    let (w, _) = writer_and_reader(&s, &admin);
    let h = team_get(&s, &w, "/api/team/status/dev-0001").json();
    assert_eq!((&h["tether_id"], &h["linked_token"], &h["linked_proof"], &h["linked_by"]), (&json!(TID), &json!(id), &Value::Null, &Value::Null), "{h}");
}

#[test]
fn a_dismissed_key_gets_no_link_by_proof_under_a_new_uuid() {
    // The rule for dismissed keys: a key whose link a member dismissed (a live `no_claim` pin, or
    // one it released itself) claims nothing under any uuid; a grant with a valid proof does not
    // link it either (200, a new pin with `no_claim`, no Tether ID).
    let (admin, tether) = (generate_key(), generate_key());
    let s = proof_server(&admin, &hbbs_knowing(TID, &tether).0);
    let (w, r) = writer_and_reader(&s, &admin);
    let any = status::make_token("t", now() + 3600, None, None, false, &admin).unwrap().0;
    let (g, _) = grant(&admin, None);
    let a = generate_key();
    assert_eq!(enroll(&s, &a, "dev-0001", Some(TID), &any).status, 200);
    assert_eq!(s.admin(&w, "DELETE", "/api/team/status/dev-0001/link", b"").status, 204);
    let unlinked = |u: &str| {
        let h = team_get(&s, &r, &format!("/api/team/status/{u}")).json();
        assert_eq!((&h["tether_id"], &h["claimed_id"], &h["no_claim"], &h["linked_proof"]), (&Value::Null, &Value::Null, &json!(true), &Value::Null), "{u} {h}");
    };
    // The dismissed pin is live on dev-0001.
    assert_eq!(enroll_proof(&s, &a, "dev-0002", Some(TID), &g, Some(&proof(&tether, TID, "dev-0002", &a))).status, 200);
    unlinked("dev-0002");
    // Released by the device itself.
    assert_eq!(signed(&s, &a, "dev-0001", "DELETE", "/api/status", b"").status, 204);
    assert_eq!(signed(&s, &a, "dev-0002", "DELETE", "/api/status", b"").status, 204);
    assert_eq!(enroll_proof(&s, &a, "dev-0003", Some(TID), &g, Some(&proof(&tether, TID, "dev-0003", &a))).status, 200);
    unlinked("dev-0003");
    assert!(!std::fs::read_to_string(&s.err).unwrap().contains("by Tether proof"));
}

#[test]
fn unknown_ids_count_against_the_ip() {
    let (admin, tether) = (generate_key(), generate_key());
    let (url, hits) = hbbs_knowing(TID, &tether);
    let s = proof_server(&admin, &url);
    let (g, _) = grant(&admin, None);
    let a = generate_key();
    let ip = "203.0.113.31";
    let from_ip = |id: &str| {
        let b = json!({"uuid": "dev-0001", "pubkey": key_to_b64(&a.verifying_key()), "token": g, "tether_id": id, "tether_proof": proof(&tether, id, "dev-0001", &a)}).to_string();
        let sig = reqauth::sign_device_request(&a, "dev-0001", RELAY, "POST", "/api/status/enroll", b.as_bytes(), now());
        s.req("POST", "/api/status/enroll", &[("X-Forwarded-For", ip), (reqauth::HEADER, &sig)], b.as_bytes())
    };
    for i in 0..30 {
        let r = from_ip(&format!("{:09}", 100_000_000 + i));
        assert_eq!((r.status, r.text()), (409, "tether id unknown to this relay".into()), "attempt {i}");
    }
    let r = from_ip(TID);
    assert_eq!((r.status, r.text()), (429, "too many requests".into()));
    assert_eq!(hits.load(Ordering::SeqCst), 30, "hbbs not asked once the IP is over its limit");
}

/// A listener whose accept queue is full: a connect to it gets no answer (Linux drops the SYN).
fn hbbs_full() -> (String, TcpListener, Vec<TcpStream>) {
    use std::os::unix::io::AsRawFd;
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    assert_eq!(unsafe { libc::listen(l.as_raw_fd(), 0) }, 0);
    let a = l.local_addr().unwrap();
    let held: Vec<_> = (0..2).filter_map(|_| TcpStream::connect_timeout(&a, Duration::from_millis(300)).ok()).collect();
    assert!(TcpStream::connect_timeout(&a, Duration::from_millis(300)).is_err(), "the queue is full");
    (format!("http://{a}"), l, held)
}

#[test]
fn a_dead_first_hbbs_leaves_the_next_its_turn() {
    let (admin, tether) = (generate_key(), generate_key());
    let (good, _) = hbbs_knowing(TID, &tether);
    let (full, _l, _held) = hbbs_full();
    // Accepts and never answers.
    let (mute, _) = stub_hbbs(|_| {
        std::thread::sleep(Duration::from_secs(5));
        Vec::new()
    });
    let (g, _) = grant(&admin, None);
    let a = generate_key();
    for first in [full, mute] {
        let s = proof_server(&admin, &format!("{first},{good}"));
        let t0 = Instant::now();
        let r = enroll_proof(&s, &a, "dev-0001", Some(TID), &g, Some(&proof(&tether, TID, "dev-0001", &a)));
        assert_eq!(r.status, 200, "{first}: {}", r.text());
        assert!(t0.elapsed() < Duration::from_millis(1800), "{first}: {:?}", t0.elapsed());
    }
}

#[test]
fn hbbs_lookups_are_capped() {
    let (admin, tether) = (generate_key(), generate_key());
    let (path, key) = (format!("/peer-pk/{TID}"), key_to_b64(&tether.verifying_key()));
    let n = AtomicU32::new(0);
    // The first four lookups hang past the 2 s; later ones are answered.
    let (url, hits) = stub_hbbs(move |p| {
        if n.fetch_add(1, Ordering::SeqCst) < 4 {
            std::thread::sleep(Duration::from_secs(3));
            return Vec::new();
        }
        if p == path { http("200 OK", &format!("{key}\n")) } else { http("404 Not Found", "not found\n") }
    });
    let dir = new_dir();
    let b = generate_key();
    prep_device(&dir, "dev-0009", &b, now() - 61, 5);
    let s = spawn_env(dir, &key_to_b64(&admin.verifying_key()), &[("HBBS_PEER_URL", &url)]);
    let (g, _) = grant(&admin, None);
    let keys: Vec<_> = (0..6).map(|_| generate_key()).collect();
    let try_enrol = |i: usize| enroll_proof(&s, &keys[i], &format!("dev-000{i}"), Some(TID), &g, Some(&proof(&tether, TID, &format!("dev-000{i}"), &keys[i])));
    std::thread::scope(|sc| {
        let hung: Vec<_> = (0..4).map(|i| sc.spawn(move || try_enrol(i))).collect();
        while hits.load(Ordering::SeqCst) < 4 {
            std::thread::sleep(Duration::from_millis(20));
        }
        let t0 = Instant::now();
        let r = try_enrol(4);
        assert_eq!((r.status, r.text()), (503, "tether proof unavailable".into()));
        assert!(t0.elapsed() < Duration::from_millis(200), "refused in {:?}", t0.elapsed());
        assert_eq!(hits.load(Ordering::SeqCst), 4, "no lookup started");
        let t0 = Instant::now();
        assert_eq!(post(&s, &b, "dev-0009", &heartbeat("dev-0009", 6)).status, 200);
        assert!(t0.elapsed() < Duration::from_millis(200), "heartbeat took {:?}", t0.elapsed());
        for h in hung {
            assert_eq!(h.join().unwrap().status, 503);
        }
    });
    // The hung lookups' threads end with their 2 s; then lookups run again.
    std::thread::sleep(Duration::from_millis(300));
    let r = try_enrol(5);
    assert_eq!(r.status, 200, "{}", r.text());
}

#[test]
fn a_keys_line_with_an_unknown_field_locks_the_store() {
    // What a relay of 0.6 (this parser) does with a `set` line carrying `linked_proof`.
    let admin = generate_key();
    let dir = new_dir();
    std::fs::create_dir_all(dir.join("status")).unwrap();
    let line = json!({"op": "set", "uuid": "dev-0001", "key": key_to_b64(&generate_key().verifying_key()), "tether_id": TID, "label": "t", "enrolled": now(), "linked_at": now(), "linked_token": "abcdefghABCDEFGH", "linked_proof": true});
    std::fs::write(dir.join("status/keys.jsonl"), format!("{line}\n")).unwrap();
    let s = start_in(dir, &admin);
    let tok = status::make_token("t", now() + 3600, None, None, false, &admin).unwrap().0;
    let r = enroll(&s, &generate_key(), "dev-0002", None, &tok);
    assert_eq!((r.status, r.text()), (503, "status store unavailable".into()));
}

#[test]
fn proof_enrolment_refusals() {
    let (admin, tether) = (generate_key(), generate_key());
    let (url, hits) = hbbs_knowing(TID, &tether);
    let s = proof_server(&admin, &url);
    let (g, gid) = grant(&admin, None);
    let a = generate_key();
    let p = proof(&tether, TID, "dev-0001", &a);
    for (id, pr) in [(Some(TID), None), (None, Some(p.as_str())), (None, None)] {
        let r = enroll_proof(&s, &a, "dev-0001", id, &g, pr);
        assert_eq!((r.status, r.text()), (400, "tether proof required".into()), "{id:?} {pr:?}");
    }
    // Only an ID of hbbs's rule goes into the lookup's path.
    for bad in ["12345", "12345678901234567", "../pubkey", "1234567%2F", "12345678/", "123456789?x=1", "1234567.9", "../..\\x/y"] {
        let r = enroll_proof(&s, &a, "dev-0001", Some(bad), &g, Some(&proof(&tether, bad, "dev-0001", &a)));
        assert_eq!((r.status, r.text()), (400, "bad tether_id".into()), "{bad}");
    }
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    // A proof for another uuid, by another key, for another device key, or not a signature.
    let other = generate_key();
    for wrong in [proof(&tether, TID, "dev-0002", &a), proof(&other, TID, "dev-0001", &a), proof(&tether, TID, "dev-0001", &other), "AAAA".into(), "not base64".into()] {
        let r = enroll_proof(&s, &a, "dev-0001", Some(TID), &g, Some(&wrong));
        assert_eq!((r.status, r.text()), (401, "tether proof does not match".into()), "{wrong}");
    }
    // An ID hbbs does not know.
    let r = enroll_proof(&s, &a, "dev-0001", Some("987654321"), &g, Some(&proof(&tether, "987654321", "dev-0001", &a)));
    assert_eq!((r.status, r.text()), (409, "tether id unknown to this relay".into()));
    // A revoked grant is refused before hbbs is asked.
    let before = hits.load(Ordering::SeqCst);
    assert_eq!(revoke(&s, &admin, &gid, now() + 3600).status, 204);
    let r = enroll_proof(&s, &a, "dev-0001", Some(TID), &g, Some(&p));
    assert_eq!((r.status, r.text()), (401, "token revoked".into()));
    assert_eq!(hits.load(Ordering::SeqCst), before);
    assert_eq!(s.admin(&admin, "GET", "/api/admin/status/devices", b"").json()["devices"], json!([]), "nothing pinned");
}

#[test]
fn wrong_proofs_count_against_the_ip() {
    let (admin, tether) = (generate_key(), generate_key());
    let s = proof_server(&admin, &hbbs_knowing(TID, &tether).0);
    let (g, _) = grant(&admin, None);
    let a = generate_key();
    let ip = "203.0.113.30";
    let from_ip = |uuid: &str, p: &str| {
        let b = json!({"uuid": uuid, "pubkey": key_to_b64(&a.verifying_key()), "token": g, "tether_id": TID, "tether_proof": p}).to_string();
        let sig = reqauth::sign_device_request(&a, uuid, RELAY, "POST", "/api/status/enroll", b.as_bytes(), now());
        s.req("POST", "/api/status/enroll", &[("X-Forwarded-For", ip), (reqauth::HEADER, &sig)], b.as_bytes())
    };
    for i in 0..30 {
        let r = from_ip("dev-0001", &proof(&tether, TID, "dev-0002", &a));
        assert_eq!((r.status, r.text()), (401, "tether proof does not match".into()), "attempt {i}");
    }
    let r = from_ip("dev-0001", &proof(&tether, TID, "dev-0001", &a));
    assert_eq!((r.status, r.text()), (429, "too many requests".into()));
    assert_eq!(enroll_proof(&s, &a, "dev-0001", Some(TID), &g, Some(&proof(&tether, TID, "dev-0001", &a))).status, 200, "other IPs unaffected");
}

#[test]
fn proof_enrolment_without_a_usable_hbbs() {
    let (admin, tether) = (generate_key(), generate_key());
    let (g, _) = grant(&admin, None);
    let a = generate_key();
    let p = proof(&tether, TID, "dev-0001", &a);
    let try_with = |hbbs: &str| {
        let s = proof_server(&admin, hbbs);
        let r = enroll_proof(&s, &a, "dev-0001", Some(TID), &g, Some(&p));
        (r.status, r.text())
    };
    assert_eq!(try_with("off"), (503, "tether proof needs HBBS_PEER_URL".into()));
    let down = hbbs_down();
    assert_eq!(try_with(&down), (503, "tether proof unavailable".into()));
    let key = key_to_b64(&tether.verifying_key());
    let junk: Vec<Vec<u8>> = vec![
        http("200 OK", ""),
        http("200 OK", "\n"),
        http("200 OK", &key), // no line end
        http("200 OK", &format!("{key}\n\n")),
        http("200 OK", &format!(" {key}\n")),
        http("200 OK", &format!("{key}\n{}", "x".repeat(200))), // over 256 bytes
        http("200 OK", "not base64!\n"),
        http("200 OK", &format!("{}AA==\n", "A".repeat(40))), // 31 bytes
        http("200 OK", &format!("{}\n", "A".repeat(44))),      // 33 bytes
        format!("HTTP/1.1 302 Found\r\nLocation: {}/peer-pk/{TID}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", hbbs_knowing(TID, &tether).0).into_bytes(),
        http("500 Internal Server Error", &format!("{key}\n")),
        format!("HTTP/1.1 200 OK\r\n\r\n{key}\n").into_bytes(), // no length
        b"garbage".to_vec(),
        Vec::new(),
    ];
    for j in junk {
        let shown = String::from_utf8_lossy(&j).into_owned();
        let (url, _) = stub_hbbs(move |_| j.clone());
        assert_eq!(try_with(&url), (503, "tether proof unavailable".into()), "{shown}");
    }
    // Several hbbs, tried in order: a down one, then one that answers.
    let (url, _) = hbbs_knowing(TID, &tether);
    assert_eq!(try_with(&format!("{down}, {url}/")).0, 200);
    // One that does not know the ID, then one that does.
    let (unknown, _) = hbbs_knowing("555555555", &tether);
    assert_eq!(try_with(&format!("{unknown},{url}")).0, 200);
}

#[test]
fn hbbs_peer_url_must_be_plain_http_urls() {
    let msgs = [
        ("https://127.0.0.1:21122", "https is not supported: hbbs's health port is plain HTTP"),
        ("HTTPS://h:1", "https is not supported"),
        ("127.0.0.1:21122", "not an http:// URL"),
        ("http://", "expected http://<host>:<port>"),
        ("http://h:1/path", "no path, query or user name"),
        ("http://u:p@h:1", "no path, query or user name"),
        ("http://u@h:1", "no path, query or user name"),
        ("http://h:1?x", "no path, query or user name"),
        ("http://h:1#x", "no path, query or user name"),
        ("http://h:1,off", "not an http:// URL"),
        ("http://::1:21122", "an IPv6 address must be in brackets, as http://[::1]:21122"),
        ("http://[::1:21122", "an IPv6 address must be in brackets"),
        ("http://[nothost]:1", "expected http://<host>:<port>"),
        ("http://h_x:1", "expected http://<host>:<port>"),
    ];
    for (bad, msg) in msgs {
        let dir = new_dir();
        let err = dir.with_extension("err");
        let st = Command::new(env!("CARGO_BIN_EXE_tether-api"))
            .args(["--data", dir.to_str().unwrap(), "--listen", "127.0.0.1:0"])
            .env("HBBS_PEER_URL", bad)
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&err).unwrap())
            .status()
            .unwrap();
        assert!(!st.success(), "{bad}");
        let e = std::fs::read_to_string(&err).unwrap();
        assert!(e.contains("HBBS_PEER_URL:") && e.contains(msg), "{bad}: {e}");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&err);
    }
    // A bracketed IPv6 address, a host name, several URLs: accepted.
    let _s = proof_server(&generate_key(), "http://[::1]:21122, http://hbbs.internal:21122/,http://10.0.0.5:21122");
}

#[test]
fn a_proof_respects_an_unlink_and_other_tokens_ignore_it() {
    let (admin, tether) = (generate_key(), generate_key());
    let (url, hits) = hbbs_knowing(TID, &tether);
    let s = proof_server(&admin, &url);
    let tok = status::make_token("t", now() + 3600, None, None, false, &admin).unwrap().0;
    let a = generate_key();
    let p = proof(&tether, TID, "dev-0001", &a);
    // An ordinary token: the proof is ignored, the ID only a claim, hbbs not asked.
    assert_eq!(enroll_proof(&s, &a, "dev-0001", Some(TID), &tok, Some(&p)).status, 200);
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    let (w, _) = writer_and_reader(&s, &admin);
    let h = team_get(&s, &w, "/api/team/status/dev-0001").json();
    assert_eq!((&h["tether_id"], &h["claimed_id"], &h["linked_proof"]), (&Value::Null, &json!(TID), &Value::Null), "{h}");
    // A member's Unlink holds against a grant with a valid proof.
    assert_eq!(s.admin(&w, "DELETE", "/api/team/status/dev-0001/link", b"").status, 204);
    let (g, _) = grant(&admin, None);
    assert_eq!(enroll_proof(&s, &a, "dev-0001", Some(TID), &g, Some(&p)).status, 200);
    let h = team_get(&s, &w, "/api/team/status/dev-0001").json();
    assert_eq!((&h["tether_id"], &h["claimed_id"], &h["no_claim"], &h["linked_proof"]), (&Value::Null, &Value::Null, &json!(true), &Value::Null), "{h}");
    let log = std::fs::read_to_string(&s.err).unwrap();
    assert!(!log.contains("by Tether proof"), "{log}");
}

#[test]
fn the_hbbs_lookup_holds_no_lock_and_is_checked_again_after() {
    let (admin, tether) = (generate_key(), generate_key());
    let (key, path) = (key_to_b64(&tether.verifying_key()), format!("/peer-pk/{TID}"));
    let known = ["/peer-pk/abcdef_-12".to_owned(), path];
    let (url, _) = stub_hbbs(move |p| {
        std::thread::sleep(Duration::from_millis(1500));
        if known.iter().any(|k| k == p) { http("200 OK", &format!("{key}\n")) } else { http("404 Not Found", "not found\n") }
    });
    let dir = new_dir();
    let b = generate_key();
    prep_device(&dir, "dev-0009", &b, now() - 61, 5);
    let s = spawn_env(dir, &key_to_b64(&admin.verifying_key()), &[("HBBS_PEER_URL", &url)]);
    let a = generate_key();
    let (g, gid) = grant(&admin, None);
    std::thread::scope(|sc| {
        let e = sc.spawn(|| enroll_proof(&s, &a, "dev-0001", Some(TID), &g, Some(&proof(&tether, TID, "dev-0001", &a))));
        std::thread::sleep(Duration::from_millis(300));
        let t0 = Instant::now();
        assert_eq!(post(&s, &b, "dev-0009", &heartbeat("dev-0009", 6)).status, 200);
        assert!(t0.elapsed() < Duration::from_millis(200), "heartbeat took {:?}", t0.elapsed());
        // Revoked while hbbs was asked.
        assert_eq!(revoke(&s, &admin, &gid, now() + 3600).status, 204);
        let r = e.join().unwrap();
        assert_eq!((r.status, r.text()), (401, "token revoked".into()));
    });
    // The ID taken by another device (a token issued for it) while hbbs was asked.
    let (g, _) = grant(&admin, None);
    std::thread::scope(|sc| {
        let e = sc.spawn(|| enroll_proof(&s, &a, "dev-0001", Some(TID), &g, Some(&proof(&tether, TID, "dev-0001", &a))));
        std::thread::sleep(Duration::from_millis(300));
        let bound = status::make_token("t", now() + 3600, Some(TID), None, false, &admin).unwrap().0;
        assert_eq!(enroll(&s, &generate_key(), "dev-0002", Some(TID), &bound).status, 200);
        let r = e.join().unwrap();
        assert_eq!((r.status, r.text()), (409, "tether_id is enrolled with another key".into()));
    });
    // The pin changed while hbbs was asked: the uuid enrolled first with another key.
    std::thread::scope(|sc| {
        let e = sc.spawn(|| enroll_proof(&s, &a, "dev-0003", Some("abcdef_-12"), &g, Some(&proof(&tether, "abcdef_-12", "dev-0003", &a))));
        std::thread::sleep(Duration::from_millis(300));
        let tok = status::make_token("t", now() + 3600, None, None, false, &admin).unwrap().0;
        assert_eq!(enroll(&s, &generate_key(), "dev-0003", None, &tok).status, 200);
        let r = e.join().unwrap();
        assert_eq!((r.status, r.text()), (409, "uuid is enrolled with another key".into()));
    });
    drop(s);
    // hbbs answering after the 2 s: unavailable, in about 2 s.
    let (slow, _) = stub_hbbs(|_| {
        std::thread::sleep(Duration::from_secs(5));
        Vec::new()
    });
    let s = proof_server(&admin, &slow);
    let t0 = Instant::now();
    let r = enroll_proof(&s, &a, "dev-0001", Some(TID), &g, Some(&proof(&tether, TID, "dev-0001", &a)));
    assert_eq!((r.status, r.text()), (503, "tether proof unavailable".into()));
    assert!(t0.elapsed() < Duration::from_millis(2800), "{:?}", t0.elapsed());
}

// ---- Permissions of a signed team document on the status routes (`sbom`, `customers`).

/// Publishes a signed team document `serial` with `members` (name, key, extra fields replacing
/// the required `sbom`, `agent` true and `customers` `["*"]`).
fn publish_doc(s: &Srv, admin: &SigningKey, serial: u64, members: &[(&str, &SigningKey, Value)]) {
    let m: Vec<Value> = members
        .iter()
        .map(|(n, k, extra)| {
            let mut e = json!({"name": n, "key": key_to_b64(&k.verifying_key()), "sbom": true, "agent": true, "customers": ["*"]});
            for (f, v) in extra.as_object().unwrap() {
                e[f] = v.clone();
            }
            e
        })
        .collect();
    let doc = serde_json::to_vec(&json!({"serial": serial, "members": m})).unwrap();
    let up = json!({"team": tether_trust::envelope::sign(&doc, admin)});
    assert_eq!(s.admin(admin, "PUT", "/api/admin/team", up.to_string().as_bytes()).status, 204);
}

/// A member request with an optional `If-None-Match`.
fn member_req(s: &Srv, m: &SigningKey, method: &str, path: &str, inm: Option<&str>, body: &[u8]) -> Reply {
    let sig = reqauth::sign_request(m, method, path, body, now());
    let mut h = vec![(reqauth::HEADER, sig.as_str())];
    h.extend(inm.map(|e| ("If-None-Match", e)));
    s.req(method, path, &h, body)
}

fn summary_hosts(r: &Reply) -> Vec<String> {
    r.json()["hosts"].as_array().unwrap().iter().map(|h| h["uuid"].as_str().unwrap().to_owned()).collect()
}

/// dev-0001 (customer 4711), dev-0002 (4812), dev-0003 (none), each with a software list, the
/// software list on.
fn scoped_server() -> (Srv, SigningKey) {
    let (s, admin, _) = sbom_server(3, &[], |dir| {
        update_json(&dir.join("status/keys.json"), |m| {
            m.get_mut("dev-0001").unwrap()["customer"] = "4711".into();
            m.get_mut("dev-0002").unwrap()["customer"] = "4812".into();
            // Claims (a legacy tether_id), so link and unlink have something to act on.
            for u in ["dev-0001", "dev-0002"] {
                m.get_mut(u).unwrap()["tether_id"] = "123456789".into();
            }
        });
        for u in ["dev-0001", "dev-0002", "dev-0003"] {
            prep_sbom(dir, u, br#"{"bomFormat":"CycloneDX"}"#, now() - 7200);
        }
    });
    (s, admin)
}

#[test]
fn customers_limit_every_status_route_and_out_of_scope_is_unknown() {
    let (s, admin) = scoped_server();
    let (all, part, nosbom, writer) = (generate_key(), generate_key(), generate_key(), generate_key());
    publish_doc(&s, &admin, 1, &[("All", &all, json!({})), ("Part", &part, json!({"customers": ["47*"]})), ("NoSbom", &nosbom, json!({"sbom": false})), ("Writer", &writer, json!({"write": true, "customers": ["47*", "0815"]}))]);

    let r = team_get(&s, &all, "/api/team/status");
    assert_eq!(summary_hosts(&r), ["dev-0001", "dev-0002", "dev-0003"]);
    assert!(r.json()["hosts"].as_array().unwrap().iter().all(|h| h["sbom"]["sha"].is_string()));
    assert_eq!(summary_hosts(&team_get(&s, &part, "/api/team/status")), ["dev-0001"]);
    // Without `sbom`: every host, no software list offered.
    let r = team_get(&s, &nosbom, "/api/team/status");
    assert_eq!(summary_hosts(&r), ["dev-0001", "dev-0002", "dev-0003"]);
    assert!(r.json()["hosts"].as_array().unwrap().iter().all(|h| h.get("sbom").is_none()), "{}", r.text());
    for u in ["dev-0001", "dev-9999"] {
        let r = team_get(&s, &nosbom, &format!("/api/team/status/{u}/sbom"));
        assert_eq!((r.status, r.text()), (403, "this member may not read software lists".into()));
    }
    assert_eq!(team_get(&s, &nosbom, "/api/team/status/dev-0003").status, 200, "reports stay readable");

    // Report and history, software list: a device outside the scope answers as an unknown one.
    assert_eq!(team_get(&s, &part, "/api/team/status/dev-0001").status, 200);
    assert_eq!(team_get(&s, &part, "/api/team/status/dev-0001/sbom").status, 200);
    for u in ["dev-0002", "dev-0003"] {
        for p in ["/api/team/status/{u}", "/api/team/status/{u}/sbom"] {
            let (out, unknown) = (team_get(&s, &part, &p.replace("{u}", u)), team_get(&s, &part, &p.replace("{u}", "dev-9999")));
            assert_eq!((out.status, &out.body, out.head.lines().count()), (unknown.status, &unknown.body, unknown.head.lines().count()), "{p} {u}");
            assert_eq!(out.status, 404);
        }
    }
    // Device actions (with `write`): the same; inside the scope they work.
    let link = br#"{"tether_id":"123456789"}"#;
    for (method, p, body) in [("POST", "/api/team/status/{u}/link", &link[..]), ("DELETE", "/api/team/status/{u}/link", b""), ("POST", "/api/team/status/{u}/allow-claim", b""), ("DELETE", "/api/team/status/{u}", b"")] {
        for u in ["dev-0002", "dev-0003"] {
            let out = member_req(&s, &writer, method, &p.replace("{u}", u), None, body);
            let unknown = member_req(&s, &writer, method, &p.replace("{u}", "dev-9999"), None, body);
            assert_eq!((out.status, &out.body), (unknown.status, &unknown.body), "{method} {p} {u}");
            assert_eq!(out.status, 404);
        }
    }
    assert_eq!(member_req(&s, &writer, "POST", "/api/team/status/dev-0001/link", None, link).status, 204);
    assert_eq!(member_req(&s, &writer, "DELETE", "/api/team/status/dev-0001/link", None, b"").status, 204);
    assert_eq!(member_req(&s, &writer, "POST", "/api/team/status/dev-0001/allow-claim", None, b"").status, 204);
    assert_eq!(member_req(&s, &part, "DELETE", "/api/team/status/dev-0001", None, b"").status, 403, "write stays what it is");
    assert_eq!(member_req(&s, &writer, "DELETE", "/api/team/status/dev-0001", None, b"").status, 204);
    // Nothing outside the scope was touched.
    let r = team_get(&s, &all, "/api/team/status/dev-0002").json();
    assert_eq!((r["claimed_id"].as_str(), r["no_claim"].as_bool()), (Some("123456789"), Some(false)));
    assert_eq!(team_get(&s, &all, "/api/team/status/dev-0003").status, 200);
}

#[test]
fn an_empty_customer_list_sees_nothing() {
    let (s, admin) = scoped_server();
    let (none, all) = (generate_key(), generate_key());
    publish_doc(&s, &admin, 1, &[("None", &none, json!({"customers": [], "write": true, "audit": true})), ("All", &all, json!({"audit": true}))]);
    assert_eq!(team_get(&s, &none, "/api/team/status").json()["hosts"], json!([]));
    let link = br#"{"tether_id":"123456789"}"#;
    for u in ["dev-0001", "dev-0002", "dev-0003"] {
        for (method, p, body) in [("GET", "/api/team/status/{u}", &b""[..]), ("GET", "/api/team/status/{u}/sbom", b""), ("POST", "/api/team/status/{u}/link", link), ("DELETE", "/api/team/status/{u}/link", b""), ("POST", "/api/team/status/{u}/allow-claim", b""), ("DELETE", "/api/team/status/{u}", b"")] {
            let out = member_req(&s, &none, method, &p.replace("{u}", u), None, body);
            let unknown = member_req(&s, &none, method, &p.replace("{u}", "dev-9999"), None, body);
            assert_eq!((out.status, &out.body), (unknown.status, &unknown.body), "{method} {p} {u}");
            assert_eq!(out.status, 404, "{method} {p} {u}");
        }
    }
    // The session audit too (also of a machine without a health agent).
    assert_eq!(s.req("POST", "/api/audit/conn", &[("X-Forwarded-For", "203.0.113.50")], conn_new(1, "n-1", "444555666").as_bytes()).status, 204);
    assert_eq!(team_get(&s, &all, "/api/team/audit?device=444555666").json().as_array().unwrap().len(), 1);
    assert_eq!(team_get(&s, &none, "/api/team/audit?device=444555666").json(), json!([]));
}

#[test]
fn a_software_list_out_of_scope_is_unknown_also_with_its_etag() {
    let (s, admin) = scoped_server();
    let (all, part) = (generate_key(), generate_key());
    publish_doc(&s, &admin, 1, &[("All", &all, json!({})), ("Part", &part, json!({"customers": ["47*"]}))]);
    let etag = team_get(&s, &all, "/api/team/status/dev-0002/sbom").header("ETag").unwrap();
    assert_eq!(member_req(&s, &all, "GET", "/api/team/status/dev-0002/sbom", Some(&etag), b"").status, 304);
    let out = member_req(&s, &part, "GET", "/api/team/status/dev-0002/sbom", Some(&etag), b"");
    let unknown = member_req(&s, &part, "GET", "/api/team/status/dev-9999/sbom", Some(&etag), b"");
    assert_eq!((out.status, &out.body), (404, &unknown.body));
    assert_eq!(member_req(&s, &part, "GET", "/api/team/status/dev-0001/sbom", Some(&etag), b"").status, 304, "in scope");
}

/// A session audit `new` record of connection `conn` to Tether ID `id`.
fn conn_new(conn: i64, nonce: &str, id: &str) -> String {
    format!(r#"{{"action":"new","conn_id":{conn},"id":"{id}","ip":"198.51.100.20","nonce":"{nonce}","session_id":0,"uuid":"QUJDLTEyMw=="}}"#)
}

#[test]
fn the_session_audit_follows_the_customer_scope() {
    let (s, admin) = scoped_server();
    let (all, part, other) = (generate_key(), generate_key(), generate_key());
    publish_doc(&s, &admin, 1, &[("All", &all, json!({"audit": true, "write": true})), ("Part", &part, json!({"audit": true, "customers": ["47*"]})), ("Other", &other, json!({"audit": true, "customers": ["4812"]}))]);
    // dev-0001 (4711) verified as 123456789; 444555666 has no health agent (no pin, no customer).
    assert_eq!(member_req(&s, &all, "POST", "/api/team/status/dev-0001/link", None, br#"{"tether_id":"123456789"}"#).status, 204);
    let ip = [("X-Forwarded-For", "203.0.113.50")];
    for (i, id) in [(1, "123456789"), (2, "123456789"), (3, "444555666")] {
        assert_eq!(s.req("POST", "/api/audit/conn", &ip, conn_new(i, &format!("n-{i}"), id).as_bytes()).status, 204);
    }
    let n = |m: &SigningKey, d: &str| team_get(&s, m, &format!("/api/team/audit?device={d}")).json().as_array().unwrap().len();
    assert_eq!((n(&all, "123456789"), n(&all, "444555666")), (2, 1));
    // In scope: by the Tether ID or by the device's uuid.
    assert_eq!((n(&part, "123456789"), n(&part, "dev-0001")), (2, 2));
    // Out of scope, or no pin at all: exactly as an unknown device.
    for (m, d) in [(&part, "444555666"), (&other, "123456789"), (&other, "dev-0001"), (&other, "444555666"), (&part, "dev-0002")] {
        let (out, unknown) = (team_get(&s, m, &format!("/api/team/audit?device={d}")), team_get(&s, m, "/api/team/audit?device=999999999"));
        assert_eq!((out.status, &out.body, out.head.lines().count()), (unknown.status, &unknown.body, unknown.head.lines().count()), "{d}");
        assert_eq!(out.body, b"[]", "{d}");
    }
}

#[test]
fn a_new_customer_leaves_the_old_scope_at_once() {
    let (s, admin) = server();
    let a = generate_key();
    assert_eq!(enroll(&s, &a, "dev-0001", None, &customer_token(&admin, Some("4711")).0).status, 200);
    assert_eq!(post(&s, &a, "dev-0001", &full("dev-0001", 1, "crit")).status, 200);
    let p = generate_key();
    publish_doc(&s, &admin, 1, &[("P", &p, json!({"customers": ["4711"]}))]);
    assert_eq!(summary_hosts(&team_get(&s, &p, "/api/team/status")), ["dev-0001"]);
    // Re-enrolled for another customer: not in 4711's cached summary any more, within its 30 s.
    assert_eq!(enroll_signed(&s, &a, &a, "dev-0001", &customer_token(&admin, Some("0815")).0).status, 200);
    assert_eq!(summary_hosts(&team_get(&s, &p, "/api/team/status")), Vec::<String>::new());
}

#[test]
fn a_link_does_not_tell_of_a_holder_out_of_scope() {
    // dev-0001 (4711) and dev-0002 (4812) both claim 123456789; dev-0002 is linked to it.
    let (s, admin) = scoped_server();
    let (all, part) = (generate_key(), generate_key());
    publish_doc(&s, &admin, 1, &[("All", &all, json!({"write": true})), ("Part", &part, json!({"write": true, "customers": ["47*"]}))]);
    let link = br#"{"tether_id":"123456789"}"#;
    assert_eq!(member_req(&s, &all, "POST", "/api/team/status/dev-0002/link", None, link).status, 204);
    let r = member_req(&s, &part, "POST", "/api/team/status/dev-0001/link", None, link);
    assert_eq!((r.status, r.text()), (409, "tether_id is not the one this device claims".into()));
    let r = member_req(&s, &all, "POST", "/api/team/status/dev-0001/link", None, link);
    assert_eq!((r.status, r.text()), (409, "tether_id is pinned to another device".into()), "a holder in scope is named");
}

#[test]
fn each_scope_gets_its_own_cached_summary() {
    let (s, admin) = scoped_server();
    let (a, b, c) = (generate_key(), generate_key(), generate_key());
    // c's scope is a's, written another way: the same summary.
    publish_doc(&s, &admin, 1, &[("A", &a, json!({"customers": ["47*"]})), ("B", &b, json!({})), ("C", &c, json!({"customers": ["4711", "47*", "47*"]}))]);
    let summary = |m: &SigningKey, inm: Option<&str>| member_req(&s, m, "GET", "/api/team/status", inm, b"");
    let ra = summary(&a, None);
    let ea = ra.header("ETag").unwrap();
    assert_eq!(summary_hosts(&ra), ["dev-0001"]);
    let rb = summary(&b, None);
    let eb = rb.header("ETag").unwrap();
    assert_eq!(summary_hosts(&rb), ["dev-0001", "dev-0002", "dev-0003"]);
    assert_ne!(ea, eb);
    // Back to back, each gets its own; another scope's tag is no match.
    let r = summary(&a, Some(&eb));
    assert_eq!((r.status, summary_hosts(&r)), (200, vec!["dev-0001".to_owned()]));
    assert_eq!(summary(&a, Some(&ea)).status, 304);
    assert_eq!(summary(&b, Some(&eb)).status, 304);
    assert_eq!(summary(&b, Some(&ea)).status, 200);
    assert_eq!(summary(&c, Some(&ea)).status, 304);
    // a's scope changes with a new document: the next answer is the new scope's.
    publish_doc(&s, &admin, 2, &[("A", &a, json!({"customers": ["4812"]})), ("B", &b, json!({}))]);
    let r = summary(&a, Some(&ea));
    assert_eq!((r.status, summary_hosts(&r)), (200, vec!["dev-0002".to_owned()]));
    assert_eq!(summary(&c, None).status, 401, "removed");

    // At most 32 summaries are kept; the oldest scope's file goes.
    let keys: Vec<SigningKey> = (0..33).map(|_| generate_key()).collect();
    let names: Vec<String> = (0..33).map(|i| format!("S{i}")).collect();
    let members: Vec<(&str, &SigningKey, Value)> = keys.iter().zip(&names).enumerate().map(|(i, (k, n))| (n.as_str(), k, json!({"customers": [format!("c{i}")]}))).collect();
    publish_doc(&s, &admin, 3, &members);
    for k in &keys {
        assert_eq!(summary(k, None).json()["hosts"], json!([]));
    }
    let files = std::fs::read_dir(s.dir.join("status")).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("summary")).count();
    assert_eq!(files, 32);
}

#[test]
fn a_plain_roster_member_gets_the_summary_of_before() {
    // Same file, same bytes as before signed documents existed: summary.json, hosts with their
    // software list, a member of the plain roster sees every device.
    let (s, admin) = scoped_server();
    let m = member(&s, &admin);
    let r = team_get(&s, &m, "/api/team/status");
    assert_eq!(summary_hosts(&r), ["dev-0001", "dev-0002", "dev-0003"]);
    assert_eq!(r.body, std::fs::read(s.dir.join("status/summary.json")).unwrap());
    assert!(r.json()["hosts"][2]["sbom"]["sha"].is_string());
    assert_eq!(team_get(&s, &m, "/api/team/me").json(), json!({"name": "M", "write": false, "audit": false}));
}

/// The team audit log as the admin reads it (`?<q>`).
fn team_audit(s: &Srv, admin: &SigningKey, q: &str) -> Value {
    let r = s.admin(admin, "GET", &format!("/api/admin/team-audit{q}"), b"");
    assert_eq!(r.status, 200, "{}", r.text());
    r.json()
}

fn team_audit_files(s: &Srv) -> String {
    let mut all = String::new();
    for e in std::fs::read_dir(s.dir.join("team-audit")).unwrap().flatten() {
        if e.path().extension().is_some_and(|x| x == "jsonl") {
            all.push_str(&std::fs::read_to_string(e.path()).unwrap());
        }
    }
    all
}

#[test]
fn the_team_audit_log_records_every_access_and_nothing_it_returned() {
    let (s, admin) = scoped_server();
    let (all, part, nosbom) = (generate_key(), generate_key(), generate_key());
    publish_doc(&s, &admin, 1, &[("All", &all, json!({"write": true, "audit": true})), ("Part", &part, json!({"customers": ["47*"], "write": true})), ("NoSbom", &nosbom, json!({"sbom": false}))]);
    let kid = |k: &SigningKey| tether_trust::team::key_id(&k.verifying_key());
    // (who, event, uuid asked, status): what each request must have left.
    let mut want: Vec<(String, &str, Option<&str>, u16)> = Vec::new();
    let mut go = |who: &str, k: &SigningKey, method: &str, path: &str, body: &[u8], event: &'static str, uuid: Option<&'static str>| {
        let r = s.admin(k, method, path, body);
        want.push((who.to_owned(), event, uuid, r.status));
        r
    };
    // The summary: one record per member and hour, however often it is read.
    assert_eq!(go("All", &all, "GET", "/api/team/status", b"", "summary", None).status, 200);
    assert_eq!(team_get(&s, &all, "/api/team/status").status, 200);
    assert_eq!(go("All", &all, "GET", "/api/team/status/dev-0001", b"", "report", Some("dev-0001")).status, 200);
    assert_eq!(go("All", &all, "GET", "/api/team/status/dev-0001/sbom", b"", "software-list", Some("dev-0001")).status, 200);
    go("All", &all, "POST", "/api/team/status/dev-0001/link", br#"{"tether_id":"123456789"}"#, "link", Some("dev-0001"));
    assert_eq!(go("All", &all, "DELETE", "/api/team/status/dev-0002/link", b"", "unlink", Some("dev-0002")).status, 204);
    assert_eq!(go("All", &all, "POST", "/api/team/status/dev-0002/allow-claim", b"", "allow-claim", Some("dev-0002")).status, 204);
    assert_eq!(go("All", &all, "GET", "/api/team/audit?device=dev-0001", b"", "session-audit-read", Some("dev-0001")).status, 200);
    // Refused: a missing permission (403) and a device outside the scope (404, as an unknown one).
    assert_eq!(go("NoSbom", &nosbom, "GET", "/api/team/status/dev-0001/sbom", b"", "software-list", Some("dev-0001")).status, 403);
    assert_eq!(go("Part", &part, "GET", "/api/team/status/dev-0002", b"", "report", Some("dev-0002")).status, 404);
    assert_eq!(go("Part", &part, "GET", "/api/team/status/dev-9999", b"", "report", Some("dev-9999")).status, 404);
    assert_eq!(go("Part", &part, "DELETE", "/api/team/status/dev-0002", b"", "remove", Some("dev-0002")).status, 404);
    assert_eq!(go("All", &all, "DELETE", "/api/team/status/dev-0003", b"", "remove", Some("dev-0003")).status, 204);
    // The admin's changes.
    let (_, tok) = customer_token(&admin, None);
    let body = json!({"id": tok, "exp": now() + 3600}).to_string();
    assert_eq!(go("admin", &admin, "POST", "/api/admin/status/revoke", body.as_bytes(), "revoke", None).status, 204);
    assert_eq!(go("admin", &admin, "DELETE", "/api/admin/status/dev-0002", b"", "remove", Some("dev-0002")).status, 204);
    // Not recorded: what is not a device's data.
    assert_eq!(team_get(&s, &all, "/api/team/me").status, 200);

    let v = team_audit(&s, &admin, "");
    assert_eq!(v["problems"], json!([]));
    let head = v["head"].as_str().unwrap();
    assert!(head.len() == 64 && head != "0".repeat(64));
    let recs = v["records"].as_array().unwrap();
    let got: Vec<(String, &str, Option<&str>, u16)> = recs.iter().map(|r| (r["actor"]["name"].as_str().unwrap().to_owned(), r["event"].as_str().unwrap(), r["target"]["uuid"].as_str(), r["result"].as_u64().unwrap() as u16)).collect();
    assert_eq!(got, want);
    for r in recs {
        let keys: Vec<&str> = r.as_object().unwrap().keys().map(|k| k.as_str()).collect();
        assert!(keys.iter().all(|k| ["ts", "event", "actor", "ip", "target", "detail", "result", "prev"].contains(k)), "{r}");
        assert_eq!(r["actor"].as_object().unwrap().len(), 2, "{r}");
        assert!(r["target"].as_object().is_none_or(|t| t.keys().all(|k| ["uuid", "tether_id", "customer"].contains(&k.as_str()))), "{r}");
    }
    // A read in scope names the device fully (its pin's Tether ID once linked); a refused one only what was asked.
    assert_eq!(recs[1]["actor"], json!({"key_fingerprint": kid(&all), "name": "All"}));
    assert_eq!((recs[1]["target"].clone(), recs[1]["ip"].clone(), recs[1].get("detail")), (json!({"uuid": "dev-0001", "customer": "4711"}), json!("127.0.0.1"), None));
    assert_eq!((recs[3]["result"].as_u64(), recs[3]["detail"].clone()), (Some(204), json!({"tether_id": "123456789"})));
    assert_eq!(recs[7]["result"].as_u64(), Some(403));
    assert_eq!(recs[0]["detail"], json!({"reads": 1}));
    for (i, u) in [(7, "dev-0001"), (8, "dev-0002"), (9, "dev-9999"), (10, "dev-0002")] {
        assert_eq!(recs[i]["target"], json!({"uuid": u}), "{}", recs[i]);
    }
    assert_eq!(recs[12]["detail"], json!({"token": tok}));
    assert_eq!(recs[12]["actor"], json!({"key_fingerprint": kid(&admin), "name": "admin"}));
    // Never what was returned: no host name, product, check, software list or report content.
    let log = team_audit_files(&s);
    for secret in ["PC-1", "Example App", "app/db-service", "CycloneDX", "bomFormat", "zeta", "counts", "hosts"] {
        assert!(!log.contains(secret), "{secret} in the team audit log");
    }
    // The read itself is recorded, with its filters; members cannot read the log.
    let v = team_audit(&s, &admin, "?event=team-audit-read&actor=admin");
    let reads = v["records"].as_array().unwrap();
    assert_eq!((reads.len(), reads[0]["result"].as_u64(), reads[0].get("detail")), (1, Some(200), None));
    let v = team_audit(&s, &admin, "?event=team-audit-read");
    assert_eq!(v["records"][1]["detail"], json!({"actor": "admin", "event": "team-audit-read"}));
    assert_eq!(team_get(&s, &all, "/api/admin/team-audit").status, 401);
    // Filters by device, customer; pages.
    assert_eq!(team_audit(&s, &admin, "?device=dev-0003")["records"].as_array().unwrap().len(), 1);
    assert!(team_audit(&s, &admin, "?customer=4711")["records"].as_array().unwrap().iter().all(|r| r["target"]["uuid"] == "dev-0001"));
    let p = team_audit(&s, &admin, "?limit=3");
    assert_eq!(p["records"].as_array().unwrap().len(), 3);
    let p2 = team_audit(&s, &admin, &format!("?limit=3&cursor={}", p["next"].as_str().unwrap()));
    assert_eq!(p2["records"][0], recs[3]);
    assert_eq!(s.admin(&admin, "GET", "/api/admin/team-audit?limit=0", b"").status, 400);
    // Stored privately; every line also in the journal.
    use std::os::unix::fs::PermissionsExt;
    let dir = s.dir.join("team-audit");
    assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
    for e in std::fs::read_dir(&dir).unwrap().flatten() {
        assert_eq!(e.metadata().unwrap().permissions().mode() & 0o777, 0o600, "{:?}", e.path());
    }
    assert!(s.journal().contains(&format!("team-audit: {}", log.lines().next().unwrap())));
}

#[test]
fn members_report_their_own_events_marked_as_self_reported() {
    let (s, admin) = server();
    // A plain roster (no signed document): events name the roster member.
    let m = member(&s, &admin);
    let post = |b: Value| s.admin(&m, "POST", "/api/team/audit-event", b.to_string().as_bytes());
    assert_eq!(post(json!({"event": "export", "what": "hosts", "rows": 120})).status, 204);
    assert_eq!(post(json!({"event": "session-start", "tether_id": "123456789"})).status, 204);
    assert_eq!(post(json!({"event": "agent-request", "tether_id": "123456789", "action": "install", "outcome": "done"})).status, 204);
    for bad in [json!({"event": "delete-everything"}), json!({"event": "export", "what": "hosts", "rows": 1, "names": ["PC-1"]})] {
        assert_eq!(post(bad).status, 400);
    }
    assert_eq!(s.req("POST", "/api/team/audit-event", &[], br#"{"event":"export"}"#).status, 401);
    assert_eq!(team_get(&s, &m, "/api/team/status").status, 200);
    let v = team_audit(&s, &admin, "");
    let recs = v["records"].as_array().unwrap();
    let got: Vec<(&str, bool, &str)> = recs.iter().map(|r| (r["event"].as_str().unwrap(), r["self_reported"] == true, r["actor"]["name"].as_str().unwrap())).collect();
    assert_eq!(got, [("export", true, "M"), ("session-start", true, "M"), ("agent-request", true, "M"), ("summary", false, "M")]);
    assert_eq!((recs[0]["detail"].clone(), recs[0]["result"].as_u64()), (json!({"what": "hosts", "rows": 120}), Some(204)));
    assert_eq!(recs[2]["target"], json!({"tether_id": "123456789"}));
    assert!(!team_audit_files(&s).contains("PC-1"));
    // 30 per minute and member key (5 used above, refused ones included).
    let codes: Vec<u16> = (0..30).map(|_| post(json!({"event": "session-start", "tether_id": "123456789"})).status).collect();
    assert_eq!((codes.iter().filter(|c| **c == 204).count(), codes.last().copied()), (25, Some(429)));
}

#[test]
fn the_team_audit_chain_continues_across_a_restart() {
    let (s, admin) = server();
    let m = member(&s, &admin);
    assert_eq!(team_get(&s, &m, "/api/team/status").status, 200);
    let head = team_audit(&s, &admin, "")["head"].as_str().unwrap().to_owned();
    let s = restart(s, &admin);
    assert_eq!(s.admin(&m, "POST", "/api/team/audit-event", br#"{"event":"session-start","tether_id":"123456789"}"#).status, 204);
    let v = team_audit(&s, &admin, "");
    assert_eq!(v["problems"], json!([]));
    let recs = v["records"].as_array().unwrap();
    // The first read follows the head it returned (the summary); the event after the restart follows the read.
    assert_eq!(recs.iter().map(|r| r["event"].as_str().unwrap()).collect::<Vec<_>>(), ["summary", "team-audit-read", "session-start"]);
    assert_eq!(recs[1]["prev"], head);
}

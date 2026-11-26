//! End-to-end tests: start the real binary on a random port with a temp data dir.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tether_trust::{custom, envelope, generate_key, key_to_b64, reqauth, SigningKey};

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
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

fn start(admin_keys: &str) -> Srv {
    start_env(admin_keys, &[], |_| {})
}

/// With extra environment, after `prep` populated the fresh data dir.
fn start_env(admin_keys: &str, env: &[(&str, &str)], prep: impl FnOnce(&std::path::Path)) -> Srv {
    let dir = std::env::temp_dir().join(format!("tether-api-test-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    prep(&dir);
    let mut child = Command::new(env!("CARGO_BIN_EXE_tether-api"))
        .args(["--data", dir.to_str().unwrap(), "--listen", "127.0.0.1:0"])
        .env("ADMIN_PUB_KEY", admin_keys)
        .envs(env.iter().copied())
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
        // The server may answer (413/404) and close before reading an oversized body.
        let _ = s.write_all(body);
        let mut raw = Vec::new();
        let _ = s.read_to_end(&mut raw);
        let split = raw.windows(4).position(|w| w == b"\r\n\r\n").expect("response head");
        let head = String::from_utf8_lossy(&raw[..split]).into_owned();
        let mut lines = head.lines();
        let status = lines.next().unwrap().split_whitespace().nth(1).unwrap().parse().unwrap();
        let headers = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned())).collect();
        Reply { status, headers, body: raw[split + 4..].to_vec() }
    }

    fn admin_at(&self, key: &SigningKey, method: &str, path: &str, body: &[u8], ts: u64) -> Reply {
        let sig = reqauth::sign_request(key, method, path, body, ts);
        self.req(method, path, &[(reqauth::HEADER, &sig)], body)
    }

    fn admin(&self, key: &SigningKey, method: &str, path: &str, body: &[u8]) -> Reply {
        self.admin_at(key, method, path, body, now())
    }
}

fn admin_server() -> (Srv, SigningKey) {
    let k = generate_key();
    // A second trusted key checks the multi-key path.
    let other = generate_key();
    (start(&format!("{},{}", key_to_b64(&other.verifying_key()), key_to_b64(&k.verifying_key()))), k)
}

fn policy_doc(key: &SigningKey, payload: &str) -> Vec<u8> {
    serde_json::to_vec(&envelope::sign(payload.as_bytes(), key)).unwrap()
}

#[test]
fn admin_auth() {
    let (s, k) = admin_server();
    assert_eq!(s.admin(&k, "GET", "/api/admin/status", b"").status, 200);
    let st = s.admin(&k, "GET", "/api/admin/status", b"").json();
    assert_eq!(st["version"], "0.3.0");
    assert!(st["disk_free"].as_u64().unwrap() > 0);
    assert_eq!(st["devices"], 0);
    assert!(st["policy_serial"].is_null());

    // Unsigned, wrong key, tampered body, replayed, skewed.
    assert_eq!(s.req("GET", "/api/admin/status", &[], b"").status, 401);
    assert_eq!(s.admin(&generate_key(), "GET", "/api/admin/status", b"").status, 401);
    let sig = reqauth::sign_request(&k, "PUT", "/api/admin/download-page", b"<p>a</p>", now());
    let r = s.req("PUT", "/api/admin/download-page", &[(reqauth::HEADER, &sig)], b"<p>b</p>");
    assert_eq!(r.status, 401);
    let sig = reqauth::sign_request(&k, "GET", "/api/admin/devices", b"", now());
    assert_eq!(s.req("GET", "/api/admin/devices", &[(reqauth::HEADER, &sig)], b"").status, 200);
    let r = s.req("GET", "/api/admin/devices", &[(reqauth::HEADER, &sig)], b"");
    assert_eq!(r.status, 401);
    assert!(String::from_utf8_lossy(&r.body).contains("replayed"));
    assert_eq!(s.admin_at(&k, "GET", "/api/admin/status", b"", now() - 1000).status, 401);
    assert_eq!(s.admin_at(&k, "GET", "/api/admin/status", b"", now() + 1000).status, 401);
    // Signature covers the query string.
    let sig = reqauth::sign_request(&k, "GET", "/api/admin/status", b"", now());
    assert_eq!(s.req("GET", "/api/admin/status?x=1", &[(reqauth::HEADER, &sig)], b"").status, 401);

    // Without ADMIN_PUB_KEY, admin is off but public endpoints work.
    let open = start("");
    assert_eq!(open.admin(&k, "GET", "/api/admin/status", b"").status, 503);
    assert_eq!(open.req("GET", "/api/policy", &[], b"").status, 404);
}

/// v2 signatures name the relay they are for (`RELAY_HOST`); v1 passes until `TETHER_SIG_V1=off`.
#[test]
fn signature_v2_is_bound_to_this_relay() {
    let k = generate_key();
    let pk = key_to_b64(&k.verifying_key());
    let send = |s: &Srv, sig: &str, path: &str| {
        let r = s.req("GET", path, &[(reqauth::HEADER, sig)], b"");
        (r.status, String::from_utf8_lossy(&r.body).into_owned())
    };
    let v2 = |relay: &str, path: &str| reqauth::sign_request_for(&k, relay, "GET", path, b"", now());
    let v1 = |path: &str| reqauth::sign_request(&k, "GET", path, b"", now());
    const STATUS: &str = "/api/admin/status";

    let s = start_env(&pk, &[("RELAY_HOST", "Relay-A.example.com, alias.example.com:443")], |_| {});
    assert_eq!(send(&s, &v2("https://relay-a.example.com", STATUS), STATUS).0, 200);
    assert_eq!(send(&s, &v2("https://alias.example.com", STATUS), STATUS).0, 200);
    assert_eq!(send(&s, &v2("https://relay-b.example.com", STATUS), STATUS), (401, "request signed for another relay".to_owned()));
    // What relay B saw cannot be passed off as made for this relay, nor as v1.
    let seen = v2("https://relay-b.example.com", STATUS);
    assert_eq!(send(&s, &seen.replace("relay-b", "relay-a"), STATUS), (401, "signature does not match".to_owned()));
    assert_eq!(send(&s, &seen.replace("v2 host=relay-b.example.com ", "v1 "), STATUS), (401, "signature does not match".to_owned()));
    assert_eq!(send(&s, &v1(STATUS), STATUS).0, 200, "v1 is accepted by default");
    // Team members: the same rules.
    let roster = format!(r#"{{"members":[{{"name":"Anna","key":"{pk}","write":false,"audit":false}}]}}"#);
    assert_eq!(s.admin(&k, "PUT", "/api/admin/team", roster.as_bytes()).status, 204);
    assert_eq!(send(&s, &v2("relay-a.example.com", "/api/team/me"), "/api/team/me").0, 200);
    assert_eq!(send(&s, &v2("relay-b.example.com", "/api/team/me"), "/api/team/me"), (401, "request signed for another relay".to_owned()));

    let strict = start_env(&pk, &[("RELAY_HOST", "relay-a.example.com"), ("TETHER_SIG_V1", "off")], |_| {});
    assert_eq!(send(&strict, &v2("relay-a.example.com", STATUS), STATUS).0, 200);
    let (code, why) = send(&strict, &v1(STATUS), STATUS);
    assert!(code == 401 && why.contains("switched off"), "{code} {why}");

    // A relay that does not know its own name: no v2, v1 as before.
    let unnamed = start(&pk);
    let (code, why) = send(&unnamed, &v2("relay-a.example.com", STATUS), STATUS);
    assert!(code == 401 && why.contains("RELAY_HOST"), "{code} {why}");
    assert_eq!(send(&unnamed, &v1(STATUS), STATUS).0, 200);
}

#[test]
fn policy_publish_and_fetch() {
    let (s, k) = admin_server();
    assert_eq!(s.req("GET", "/api/policy", &[], b"").status, 404);

    let v5 = policy_doc(&k, r#"{"serial":5,"issued_at":100}"#);
    assert_eq!(s.admin(&k, "PUT", "/api/admin/policy", &v5).status, 204);
    let r = s.req("GET", "/api/policy", &[], b"");
    assert_eq!(r.status, 200);
    assert_eq!(r.body, v5);
    assert_eq!(r.header("ETag"), Some("\"5\""));
    let r = s.req("GET", "/api/policy", &[("If-None-Match", "\"5\"")], b"");
    assert_eq!(r.status, 304);
    assert!(r.body.is_empty());
    assert_eq!(s.req("GET", "/api/policy", &[("If-None-Match", "\"4\"")], b"").status, 200);

    // Same or lower serial: 409.
    assert_eq!(s.admin(&k, "PUT", "/api/admin/policy", &policy_doc(&k, r#"{"serial":5,"issued_at":1}"#)).status, 409);
    assert_eq!(s.admin(&k, "PUT", "/api/admin/policy", &policy_doc(&k, r#"{"serial":3,"issued_at":1}"#)).status, 409);
    // Invalid bundle, garbage, untrusted signer: 400.
    let bad = policy_doc(&k, r#"{"serial":9,"issued_at":1,"profiles":{"technician":{"no.such":{"state":"on"}}}}"#);
    assert_eq!(s.admin(&k, "PUT", "/api/admin/policy", &bad).status, 400);
    assert_eq!(s.admin(&k, "PUT", "/api/admin/policy", b"not json").status, 400);
    let foreign = policy_doc(&generate_key(), r#"{"serial":9,"issued_at":1}"#);
    assert_eq!(s.admin(&k, "PUT", "/api/admin/policy", &foreign).status, 400);

    let v6 = policy_doc(&k, r#"{"serial":6,"issued_at":200}"#);
    assert_eq!(s.admin(&k, "PUT", "/api/admin/policy", &v6).status, 204);
    let h = s.admin(&k, "GET", "/api/admin/policy-history", b"").json();
    assert_eq!(h, serde_json::json!([{"serial":6,"issued_at":200},{"serial":5,"issued_at":100}]));
    assert_eq!(s.admin(&k, "GET", "/api/admin/status", b"").json()["policy_serial"], 6);
    // Any stored version can be fetched back exactly as signed (restore from another admin PC).
    assert_eq!(s.admin(&k, "GET", "/api/admin/policy-history/5", b"").body, v5);
    assert_eq!(s.admin(&k, "GET", "/api/admin/policy-history/999", b"").status, 404);
    assert_eq!(s.admin(&k, "GET", "/api/admin/policy-history/..%2fpolicy", b"").status, 404);
    assert_eq!(s.req("GET", "/api/policy", &[("If-None-Match", "\"5\"")], b"").status, 200);
}

#[test]
fn policy_history_keeps_last_50() {
    let (s, k) = admin_server();
    for serial in 1..=52 {
        let doc = policy_doc(&k, &format!(r#"{{"serial":{serial},"issued_at":{serial}}}"#));
        assert_eq!(s.admin(&k, "PUT", "/api/admin/policy", &doc).status, 204);
    }
    let h = s.admin(&k, "GET", "/api/admin/policy-history", b"").json();
    let h = h.as_array().unwrap();
    assert_eq!(h.len(), 50);
    assert_eq!(h[0]["serial"], 52);
    assert_eq!(h[49]["serial"], 3);
}

fn keys_doc(key: &SigningKey, serial: u64, keys: &[&str]) -> Vec<u8> {
    let keys: Vec<String> = keys.iter().map(|k| format!(r#"{{"key":"{k}"}}"#)).collect();
    policy_doc(key, &format!(r#"{{"serial":{serial},"issued_at":1,"keys":[{}]}}"#, keys.join(",")))
}

#[test]
fn server_key_list_publish_fetch_and_heartbeat() {
    let (s, k) = admin_server();
    let (a, b) = (key_to_b64(&generate_key().verifying_key()), key_to_b64(&generate_key().verifying_key()));
    assert_eq!(s.req("GET", "/api/server-keys", &[], b"").status, 404);
    // Public endpoint is read-only; publishing needs a signed admin request.
    assert_eq!(s.req("PUT", "/api/admin/server-keys", &[], &keys_doc(&k, 1, &[&a])).status, 401);
    let v1 = keys_doc(&k, 1, &[&a, &b]);
    assert_eq!(s.admin(&k, "PUT", "/api/admin/server-keys", &v1).status, 204);
    let r = s.req("GET", "/api/server-keys", &[], b"");
    assert_eq!((r.status, r.header("ETag")), (200, Some("\"1\"")));
    assert_eq!(r.body, v1, "served exactly as signed");
    assert_eq!(s.req("GET", "/api/server-keys", &[("If-None-Match", "\"1\"")], b"").status, 304);
    // Same or lower serial, a list signed by another key, or an invalid list: refused.
    assert_eq!(s.admin(&k, "PUT", "/api/admin/server-keys", &keys_doc(&k, 1, &[&b])).status, 409);
    assert_eq!(s.admin(&k, "PUT", "/api/admin/server-keys", &keys_doc(&generate_key(), 5, &[&b])).status, 400);
    assert_eq!(s.admin(&k, "PUT", "/api/admin/server-keys", &keys_doc(&k, 5, &["bogus"])).status, 400);
    assert_eq!(s.admin(&k, "PUT", "/api/admin/server-keys", &keys_doc(&k, 5, &[])).status, 400);
    assert_eq!(s.admin(&k, "PUT", "/api/admin/server-keys", &vec![b' '; (16 << 10) + 1]).status, 413);
    // A policy document is not a key list (and the other way round).
    assert_eq!(s.admin(&k, "PUT", "/api/admin/server-keys", &policy_doc(&k, r#"{"serial":7,"issued_at":1}"#)).status, 400);
    assert_eq!(s.admin(&k, "PUT", "/api/admin/policy", &keys_doc(&k, 7, &[&a])).status, 400);
    assert_eq!(s.admin(&k, "PUT", "/api/admin/server-keys", &keys_doc(&k, 2, &[&b])).status, 204);
    assert_eq!(s.admin(&k, "GET", "/api/admin/status", b"").json()["server_keys_serial"], 2);
    assert_eq!(s.req("GET", "/api/policy", &[], b"").status, 404, "the policy is untouched");

    // Heartbeats report the applied list; older clients leave the field out.
    let new = hb("keys-0001").replace("\"serial\":12", "\"serial\":12,\"server_keys_serial\":2");
    assert_eq!(s.req("POST", "/api/heartbeat", &[], new.as_bytes()).status, 200);
    assert_eq!(s.req("POST", "/api/heartbeat", &[], hb("keys-0002").as_bytes()).status, 200);
    let devs = s.admin(&k, "GET", "/api/admin/devices", b"").json();
    let by = |u: &str| devs.as_array().unwrap().iter().find(|d| d["uuid"] == u).unwrap()["server_keys_serial"].clone();
    assert_eq!((by("keys-0001"), by("keys-0002")), (serde_json::json!(2), serde_json::json!(0)));
}

fn hb(uuid: &str) -> String {
    format!(r#"{{"uuid":"{uuid}","id":"123456789","profile":"technician","version":"0.3.0","os":"windows 10.0.26300","serial":12}}"#)
}

#[test]
fn heartbeat() {
    let (s, k) = admin_server();
    let ip = [("X-Forwarded-For", "203.0.113.7, 10.0.0.1")];
    assert_eq!(s.req("POST", "/api/heartbeat", &ip, hb("aaaa-1111").as_bytes()).status, 200);
    assert_eq!(s.req("POST", "/api/heartbeat", &ip, hb("aaaa-1111").as_bytes()).status, 429);

    let ip2 = [("X-Forwarded-For", "198.51.100.1")];
    assert_eq!(s.req("POST", "/api/heartbeat", &ip2, hb("short").as_bytes()).status, 400);
    assert_eq!(s.req("POST", "/api/heartbeat", &ip2, hb("bad_uuid!x").as_bytes()).status, 400);
    let extra = hb("bbbb-2222").replace("\"serial\":12", "\"serial\":12,\"x\":1");
    assert_eq!(s.req("POST", "/api/heartbeat", &ip2, extra.as_bytes()).status, 400);
    let profile = hb("bbbb-2222").replace("technician", "root");
    assert_eq!(s.req("POST", "/api/heartbeat", &ip2, profile.as_bytes()).status, 400);
    let big = hb("bbbb-2222").replace("windows", &"w".repeat(3000));
    assert_eq!(s.req("POST", "/api/heartbeat", &ip2, big.as_bytes()).status, 413);

    let devs = s.admin(&k, "GET", "/api/admin/devices", b"").json();
    assert_eq!(devs.as_array().unwrap().len(), 1);
    assert_eq!(devs[0]["uuid"], "aaaa-1111");
    assert_eq!(devs[0]["ip"], "203.0.113.7");
    assert_eq!(devs[0]["serial"], 12);
    assert!(devs[0]["last_seen"].as_u64().unwrap() >= now() - 5);

    // 30 per minute per IP, the 31st is refused.
    let ip3 = [("X-Forwarded-For", "192.0.2.9")];
    for i in 0..30 {
        assert_eq!(s.req("POST", "/api/heartbeat", &ip3, hb(&format!("dev-{i:04}")).as_bytes()).status, 200, "post {i}");
    }
    let r = s.req("POST", "/api/heartbeat", &ip3, hb("dev-9999").as_bytes());
    assert_eq!((r.status, r.body.as_slice()), (429, &b"too many requests"[..]));
    // A known device behind that IP is not refused for the IP's count, only by its own interval.
    let r = s.req("POST", "/api/heartbeat", &ip3, hb("aaaa-1111").as_bytes());
    assert_eq!((r.status, r.body.as_slice()), (429, &b"heartbeat too frequent"[..]));
    assert_eq!(s.admin(&k, "GET", "/api/admin/status", b"").json()["devices"], 31);
}

#[test]
fn diag_ticket_flow() {
    let (s, k) = admin_server();
    assert_eq!(s.req("POST", "/api/diag/ABCDEFGH", &[], b"PK").status, 404);

    let r = s.admin(&k, "POST", "/api/admin/diag-ticket", br#"{"note":"printer issue"}"#);
    assert_eq!(r.status, 200);
    let t = r.json();
    let ticket = t["ticket"].as_str().unwrap().to_owned();
    assert_eq!(ticket.len(), 8);
    assert!(ticket.bytes().all(|b| b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789".contains(&b)));
    let exp = t["expires"].as_u64().unwrap();
    assert!(exp >= now() + 86_400 - 5 && exp <= now() + 86_400);

    // Oversized upload is refused and leaves the ticket open.
    let big = vec![0u8; (2 << 20) + 1];
    assert_eq!(s.req("POST", &format!("/api/diag/{ticket}"), &[("Content-Type", "application/zip")], &big).status, 413);
    let zip = b"PK\x03\x04 fake zip".to_vec();
    // A client that goes away before its Content-Length arrived does not use up the ticket.
    let mut c = TcpStream::connect(&s.addr).unwrap();
    c.write_all(format!("POST /api/diag/{ticket} HTTP/1.1\r\nHost: t\r\nContent-Length: 1000\r\n\r\nPK").as_bytes()).unwrap();
    c.shutdown(std::net::Shutdown::Write).unwrap();
    // tiny_http may drop the half-closed connection without an answer; the ticket is what counts.
    let _ = c.read_to_end(&mut Vec::new());
    std::thread::sleep(Duration::from_millis(300));
    assert!(s.admin(&k, "GET", "/api/admin/diag", b"").json()[0]["uploaded"].is_null());
    assert_eq!(s.req("POST", &format!("/api/diag/{ticket}"), &[("Content-Type", "application/zip")], &zip).status, 204);
    let used = s.req("POST", &format!("/api/diag/{ticket}"), &[], &zip);
    assert_eq!((used.status, used.body.as_slice()), (404, &b"ticket not open"[..]), "used: the apps stop here");
    assert_eq!(s.req("POST", "/api/diag/ABCDEFGH", &[], b"PK").body, b"not found", "unknown: the apps try the next relay");

    let list = s.admin(&k, "GET", "/api/admin/diag", b"").json();
    assert_eq!(list[0]["ticket"], ticket.as_str());
    assert_eq!(list[0]["note"], "printer issue");
    assert_eq!(list[0]["size"], zip.len());
    assert!(list[0]["uploaded"].as_u64().is_some());

    let r = s.admin(&k, "GET", &format!("/api/admin/diag/{ticket}"), b"");
    assert_eq!(r.status, 200);
    assert_eq!(r.header("Content-Type"), Some("application/zip"));
    assert_eq!(r.body, zip);
    assert_eq!(s.admin(&k, "GET", "/api/admin/diag/ZZZZZZZZ", b"").status, 404);
    assert_eq!(s.admin(&k, "GET", "/api/admin/diag/..%2Ftickets", b"").status, 404);

    let long = format!(r#"{{"note":"{}"}}"#, "n".repeat(257));
    assert_eq!(s.admin(&k, "POST", "/api/admin/diag-ticket", long.as_bytes()).status, 413);
    assert_eq!(s.admin(&k, "POST", "/api/admin/diag-ticket", br#"{"note":"a","x":1}"#).status, 400);

    // Same ticket on another relay (multi-relay setups): given ID accepted once, validated.
    let r = s.admin(&k, "POST", "/api/admin/diag-ticket", br#"{"note":"b","ticket":"K7Q2M9XD"}"#);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.json()["ticket"], "K7Q2M9XD");
    assert_eq!(s.admin(&k, "POST", "/api/admin/diag-ticket", br#"{"note":"b","ticket":"K7Q2M9XD"}"#).status, 409);
    assert_eq!(s.admin(&k, "POST", "/api/admin/diag-ticket", br#"{"note":"b","ticket":"../x/abc"}"#).status, 400);
    assert_eq!(s.req("POST", "/api/diag/K7Q2M9XD", &[("Content-Type", "application/zip")], &zip).status, 204);
}

/// Close (archive) and delete: states, uploads refused after close, archived listing, the zip
/// removed by delete, idempotency, auth and replay.
#[test]
fn diag_close_archive_delete() {
    let (s, k) = admin_server();
    let zip = b"PK\x03\x04 fake zip".to_vec();
    let up = |t: &str| s.req("POST", &format!("/api/diag/{t}"), &[("Content-Type", "application/zip")], &zip).status;
    let open = |note: &str| s.admin(&k, "POST", "/api/admin/diag-ticket", format!(r#"{{"note":"{note}"}}"#).as_bytes()).json()["ticket"].as_str().unwrap().to_owned();
    let (a, b, c) = (open("uploaded then closed"), open("closed before upload"), open("deleted"));
    assert_eq!(up(&a), 204);
    assert_eq!(up(&c), 204);
    let zip_of = |t: &str| s.dir.join("diag").join(format!("{t}.zip"));
    assert!(zip_of(&a).exists() && zip_of(&c).exists());

    let state = |list: &serde_json::Value, t: &str| list.as_array().unwrap().iter().find(|x| x["ticket"] == t).map(|x| x["state"].as_str().unwrap().to_owned());
    let active = s.admin(&k, "GET", "/api/admin/diag", b"").json();
    assert_eq!(state(&active, &a).as_deref(), Some("uploaded"));
    assert_eq!(state(&active, &b).as_deref(), Some("open"));

    // Unsigned, wrong key and replayed closes are refused and change nothing.
    let path = format!("/api/admin/diag/{b}/close");
    assert_eq!(s.req("POST", &path, &[], b"").status, 401);
    assert_eq!(s.admin(&generate_key(), "POST", &path, b"").status, 401);
    let sig = reqauth::sign_request(&k, "POST", &path, b"", now());
    assert_eq!(s.req("POST", &path, &[(reqauth::HEADER, &sig)], b"").status, 204);
    assert_eq!(s.req("POST", &path, &[(reqauth::HEADER, &sig)], b"").status, 401, "replay");
    // Closed: no upload (404 like an expired code), closing again is a no-op.
    assert_eq!(up(&b), 404);
    assert_eq!(s.req("POST", &format!("/api/diag/{b}"), &[], &zip).body, b"ticket not open");
    assert_eq!(s.admin(&k, "POST", &path, b"").status, 204);
    assert_eq!(s.admin(&k, "POST", &format!("/api/admin/diag/{a}/close"), b"").status, 204);
    assert_eq!(s.admin(&k, "POST", "/api/admin/diag/ZZZZZZZZ/close", b"").status, 404);

    let active = s.admin(&k, "GET", "/api/admin/diag", b"").json();
    assert_eq!(state(&active, &a), None);
    assert_eq!(state(&active, &c).as_deref(), Some("uploaded"));
    let archived = s.admin(&k, "GET", "/api/admin/diag?archived=1", b"").json();
    assert_eq!(archived.as_array().unwrap().len(), 2);
    assert_eq!(state(&archived, &a).as_deref(), Some("closed"));
    let ea = archived.as_array().unwrap().iter().find(|x| x["ticket"] == a.as_str()).unwrap();
    assert_eq!(ea["note"], "uploaded then closed");
    assert_eq!(ea["size"], zip.len());
    assert!(ea["closed"].as_u64().is_some());
    assert_eq!(s.admin(&k, "GET", "/api/admin/diag?x=1", b"").status, 400);
    // An archived upload stays downloadable.
    assert_eq!(s.admin(&k, "GET", &format!("/api/admin/diag/{a}"), b"").body, zip);

    // Delete: entry and zip gone for good, in any state; again 404.
    for t in [&a, &b, &c] {
        assert_eq!(s.admin(&k, "DELETE", &format!("/api/admin/diag/{t}"), b"").status, 204);
        assert_eq!(s.admin(&k, "DELETE", &format!("/api/admin/diag/{t}"), b"").status, 404);
        assert!(!zip_of(t).exists());
        assert_eq!(s.admin(&k, "GET", &format!("/api/admin/diag/{t}"), b"").status, 404);
    }
    assert_eq!(s.req("DELETE", "/api/admin/diag/ZZZZZZZZ", &[], b"").status, 401);
    assert_eq!(s.admin(&k, "DELETE", "/api/admin/diag/..%2Ftickets.json", b"").status, 404);
    assert!(s.dir.join("diag/tickets.json").exists());
    assert_eq!(s.admin(&k, "GET", "/api/admin/diag", b"").json(), serde_json::json!([]));
    assert_eq!(s.admin(&k, "GET", "/api/admin/diag?archived=1", b"").json(), serde_json::json!([]));
}

/// Expiry: an unused ticket expires after 24 h (no upload, still listed until 7 days); a closed
/// ticket is kept 90 days after closing, its zip too; tickets past their keep time are removed.
#[test]
fn diag_expiry_and_retention() {
    let k = generate_key();
    let t = now();
    let d = 86_400;
    let tickets = serde_json::json!([
        {"ticket": "AAAAAAAA", "note": "", "created": t - 2 * d, "uploaded": null, "size": null},
        {"ticket": "BBBBBBBB", "note": "", "created": t - 8 * d, "uploaded": null, "size": null},
        {"ticket": "CCCCCCCC", "note": "", "created": t - 30 * d, "uploaded": t - 30 * d, "size": 2, "closed": t - 20 * d},
        {"ticket": "DDDDDDDD", "note": "", "created": t - 100 * d, "uploaded": t - 100 * d, "size": 2, "closed": t - 91 * d},
        {"ticket": "EEEEEEEE", "note": "", "created": t - 9 * d, "uploaded": t - 8 * d, "size": 2}
    ]);
    let s = start_env(&key_to_b64(&k.verifying_key()), &[], |dir| {
        std::fs::create_dir_all(dir.join("diag")).unwrap();
        std::fs::write(dir.join("diag/tickets.json"), tickets.to_string()).unwrap();
        for z in ["CCCCCCCC", "DDDDDDDD", "EEEEEEEE"] {
            std::fs::write(dir.join(format!("diag/{z}.zip")), b"PK").unwrap();
        }
    });
    let zip = b"PK".to_vec();
    let r = s.req("POST", "/api/diag/AAAAAAAA", &[("Content-Type", "application/zip")], &zip);
    assert_eq!((r.status, r.body.as_slice()), (404, &b"ticket not open"[..]), "expired");
    // Retention runs before the server listens.
    let ids = |v: serde_json::Value| v.as_array().unwrap().iter().map(|x| format!("{}:{}", x["ticket"].as_str().unwrap(), x["state"].as_str().unwrap())).collect::<Vec<_>>();
    assert_eq!(ids(s.admin(&k, "GET", "/api/admin/diag", b"").json()), ["AAAAAAAA:expired"]);
    assert_eq!(ids(s.admin(&k, "GET", "/api/admin/diag?archived=1", b"").json()), ["CCCCCCCC:closed"]);
    assert!(s.dir.join("diag/CCCCCCCC.zip").exists());
    assert!(!s.dir.join("diag/DDDDDDDD.zip").exists());
    assert!(!s.dir.join("diag/EEEEEEEE.zip").exists());
    assert_eq!(s.admin(&k, "GET", "/api/admin/diag/CCCCCCCC", b"").body, zip);
}

#[test]
fn releases() {
    let (s, k) = admin_server();
    for bad in [
        "/api/admin/release/stable/../x",
        "/api/admin/release/stable/a/../../x",
        "/api/admin/release/stable/./x",
        "/api/admin/release/Stable/x",
        "/api/admin/release/stable/%2e%2e/x",
        "/api/admin/release/stable/a//b",
        "/api/admin/release/stable/",
        "/api/admin/release/stable",
        "/api/admin/release/../download/x",
        "/api/admin/release/stable/a%20b",
    ] {
        assert_eq!(s.admin(&k, "PUT", bad, b"x").status, 400, "{bad}");
    }

    let p = "/api/admin/release/stable/technician/windows-x86_64/latest.json";
    // Unsigned upload is refused and leaves nothing behind.
    assert_eq!(s.req("PUT", p, &[], b"{}").status, 401);
    assert!(!s.dir.join("download/stable").exists());
    assert_eq!(s.admin(&k, "PUT", p, br#"{"version":"0.3.1"}"#).status, 204);
    let f = s.dir.join("download/stable/technician/windows-x86_64/latest.json");
    assert_eq!(std::fs::read(&f).unwrap(), br#"{"version":"0.3.1"}"#);
    let exe = vec![7u8; 300_000];
    assert_eq!(s.admin(&k, "PUT", "/api/admin/release/stable/technician/windows-x86_64/Tether_0.3.1_x64-setup.exe", &exe).status, 204);

    let list = s.admin(&k, "GET", "/api/admin/releases", b"").json();
    assert_eq!(list.as_array().unwrap().len(), 2);
    assert_eq!(list[0]["channel"], "stable");
    assert_eq!(list[0]["path"], "technician/windows-x86_64/Tether_0.3.1_x64-setup.exe");
    assert_eq!(list[0]["size"], 300_000);

    // latest.json is what devices read: only with force (docs/release-cleanup.md §4.3).
    assert_eq!(s.admin(&k, "DELETE", p, b"").status, 409);
    let forced = format!("{p}?force=1");
    assert_eq!(s.admin(&k, "DELETE", &forced, b"").status, 204);
    assert!(!f.exists());
    assert_eq!(s.admin(&k, "DELETE", &forced, b"").status, 404);

    assert_eq!(s.admin(&k, "PUT", "/api/admin/download-page", b"<h1>Tether</h1>").status, 204);
    assert_eq!(std::fs::read(s.dir.join("download/index.html")).unwrap(), b"<h1>Tether</h1>");
    // index.html is not a release.
    assert_eq!(s.admin(&k, "GET", "/api/admin/releases", b"").json().as_array().unwrap().len(), 1);
}

#[test]
fn size_limits() {
    let (s, k) = admin_server();
    let big_policy = vec![b' '; (256 << 10) + 1];
    assert_eq!(s.admin(&k, "PUT", "/api/admin/policy", &big_policy).status, 413);
    let big_page = vec![b'a'; (512 << 10) + 1];
    assert_eq!(s.admin(&k, "PUT", "/api/admin/download-page", &big_page).status, 413);
    assert_eq!(s.admin(&k, "PUT", "/api/admin/download-page", &big_page[1..]).status, 204);
    let big_hb = vec![b' '; 2049];
    assert_eq!(s.req("POST", "/api/heartbeat", &[], &big_hb).status, 413);
    // Server still healthy after refusing bodies it did not read.
    assert_eq!(s.admin(&k, "GET", "/api/admin/status", b"").status, 200);
}

fn custom_body(key: &SigningKey, id: &str, name: &str, token: &str, expires: u64) -> Vec<u8> {
    let p = custom::CustomProfile { id: id.into(), name: name.into(), accent: "#c8102e".into(), issued_at: 1, ..Default::default() };
    let doc = envelope::sign(&serde_json::to_vec(&p).unwrap(), key);
    serde_json::to_vec(&serde_json::json!({"profile": doc, "token": token, "expires": expires, "channel": "stable"})).unwrap()
}

#[test]
fn custom_apps() {
    let (s, k) = admin_server();
    let tok = custom::new_token();
    let exp = now() + 3600;
    let path = "/api/admin/custom/acme-1";
    let body = custom_body(&k, "acme-1", "ACME \"GmbH\"/../x\r\n", &tok, exp);
    // Name with quotes/CR is refused by validate(); a plain one is fine.
    assert_eq!(s.admin(&k, "PUT", path, &body).status, 400);
    let body = custom_body(&k, "acme-1", "ACME \"GmbH\" / Ä", &tok, exp);

    // Auth and validation.
    assert_eq!(s.req("PUT", path, &[], &body).status, 401);
    assert_eq!(s.admin(&generate_key(), "PUT", path, &body).status, 401);
    assert_eq!(s.admin(&k, "PUT", "/api/admin/custom/other", &body).status, 400, "id mismatch");
    assert_eq!(s.admin(&k, "PUT", path, &custom_body(&generate_key(), "acme-1", "A", &tok, exp)).status, 400, "untrusted signer");
    assert_eq!(s.admin(&k, "PUT", path, &custom_body(&k, "acme-1", "A", "short", exp)).status, 400, "token shape");
    assert_eq!(s.admin(&k, "PUT", path, &custom_body(&k, "acme-1", "A", &tok, now() - 1)).status, 400, "expired");
    assert_eq!(s.admin(&k, "PUT", path, &custom_body(&k, "acme-1", "A", &tok, now() + 367 * 86_400)).status, 400, "too far");
    assert_eq!(s.admin(&k, "PUT", "/api/admin/custom/..%2fx", &body).status, 400);
    assert_eq!(s.admin(&k, "PUT", path, &vec![b' '; (300 << 10) + 1]).status, 413);
    assert_eq!(s.admin(&k, "PUT", path, &body).status, 204);
    let f = s.dir.join("custom/acme-1.json");
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o777, 0o600);
    }
    // Another app cannot reuse the token.
    assert_eq!(s.admin(&k, "PUT", "/api/admin/custom/b", &custom_body(&k, "b", "B", &tok, exp)).status, 409);

    // No release yet: 404. Then publish one and download.
    let q = format!("/q/{tok}");
    assert_eq!(s.req("GET", &q, &[], b"").status, 404);
    let exe = vec![0x4du8; 200_000];
    let rel = "/api/admin/release/stable/customer/windows-x86_64";
    assert_eq!(s.admin(&k, "PUT", &format!("{rel}/TetherQS-0.4.1.exe"), &exe).status, 204);
    let latest = br#"{"version":"0.4.1","platforms":{"windows-x86_64":{"signature":"x","url":"https://r/download/stable/customer/windows-x86_64/TetherQS-0.4.1.exe"}}}"#;
    assert_eq!(s.admin(&k, "PUT", &format!("{rel}/latest.json"), latest).status, 204);
    let r = s.req("GET", &q, &[], b"");
    assert_eq!(r.status, 200);
    assert_eq!(r.header("Cache-Control"), Some("no-store"));
    assert_eq!(r.header("Content-Disposition"), Some("attachment; filename=\"ACME GmbH-Support.exe\""));
    assert_eq!(r.header("Content-Length").unwrap().parse::<usize>().unwrap(), r.body.len());
    let (plain, doc) = custom::split(&r.body);
    assert_eq!(plain, exe);
    let p = custom::open(doc.unwrap(), &[k.verifying_key()]).unwrap();
    assert_eq!((p.id.as_str(), p.accent.as_str()), ("acme-1", "#c8102e"));
    // Behind Caddy (X-Tether-Accel): no body, the path of the same bytes for Caddy to send.
    let r = s.req("GET", &q, &[("X-Tether-Accel", "1")], b"");
    assert_eq!((r.status, r.body.len()), (204, 0));
    assert_eq!(r.header("Content-Disposition"), Some("attachment; filename=\"ACME GmbH-Support.exe\""));
    let rel = r.header("X-Accel-Redirect").unwrap().to_owned();
    assert!(rel.starts_with("/qcache/acme-1--") && rel.ends_with("--TetherQS-0.4.1.exe"), "{rel}");
    let cached = s.dir.join(rel.trim_start_matches('/'));
    assert_eq!(std::fs::read(&cached).unwrap(), s.req("GET", &q, &[], b"").body);
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&cached).unwrap().permissions().mode() & 0o777, 0o640, "readable by Caddy's group");
    }

    let list = s.admin(&k, "GET", "/api/admin/custom", b"").json();
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["id"], "acme-1");
    assert_eq!(list[0]["token"], tok.as_str());
    assert_eq!(list[0]["expires"], exp);
    assert_eq!(list[0]["channel"], "stable");
    assert_eq!(list[0]["downloads"], 3);
    assert!(list[0]["last_download"].as_u64().is_some());
    assert!(list[0].get("profile").is_none());

    // macOS: the same link serves the TetherQS zip with the .app renamed to carry the mac id.
    let mac_id = custom::mac_id(&tok);
    let bundle = format!("ACME GmbH-Support [{mac_id}].app");
    assert_eq!(s.req("GET", &format!("{q}/mac"), &[], b"").status, 404, "no macOS release yet");
    let zip = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/TetherQS-mini.zip")).unwrap();
    let mrel = "/api/admin/release/stable/customer/darwin-universal";
    assert_eq!(s.admin(&k, "PUT", &format!("{mrel}/TetherQS-0.4.1-universal.zip"), &zip).status, 204);
    let mlatest = br#"{"version":"0.4.1","platforms":{"darwin-universal":{"signature":"x","url":"https://r/download/stable/customer/darwin-universal/TetherQS-0.4.1-universal.zip"}}}"#;
    assert_eq!(s.admin(&k, "PUT", &format!("{mrel}/latest.json"), mlatest).status, 204);
    let r = s.req("GET", &format!("{q}/mac"), &[], b"");
    assert_eq!(r.status, 200);
    assert_eq!(r.header("Content-Type"), Some("application/zip"));
    assert_eq!(r.header("Content-Disposition"), Some("attachment; filename=\"ACME GmbH-Support.zip\""));
    assert_eq!(r.header("Content-Length").unwrap().parse::<usize>().unwrap(), r.body.len());
    // 4 entries, each name in the local and the central header grows by the same amount.
    assert_eq!(r.body.len(), zip.len() + 8 * (bundle.len() - "TetherQS.app".len()));
    let count = |hay: &[u8], needle: &[u8]| hay.windows(needle.len()).filter(|w| *w == needle).count();
    assert_eq!(count(&r.body, format!("{bundle}/Contents/MacOS/TetherQS").as_bytes()), 2);
    assert_eq!(count(&r.body, b"TetherQS.app"), 0);
    let mac_zip = r.body;
    // A Mac browser on the plain link gets the zip, everything else (Windows, curl) the exe.
    let mac_ua = ("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15");
    assert_eq!(s.req("GET", &q, &[mac_ua], b"").body, mac_zip);
    let iphone = ("User-Agent", "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15");
    assert_eq!(s.req("GET", &q, &[iphone], b"").header("Content-Type"), Some("application/vnd.microsoft.portable-executable"));
    assert_eq!(s.req("GET", &format!("{q}/windows"), &[mac_ua], b"").header("Content-Type"), Some("application/vnd.microsoft.portable-executable"));
    assert_eq!(s.req("GET", &format!("{q}/linux"), &[], b"").status, 404);
    assert_eq!(s.req("GET", &format!("{q}/mac/x"), &[], b"").status, 404);
    // Behind Caddy: the prepared zip, next to (not instead of) the prepared exe.
    let r = s.req("GET", &format!("{q}/mac"), &[("X-Tether-Accel", "1")], b"");
    assert_eq!((r.status, r.header("Content-Type")), (204, Some("application/zip")));
    let mcached = r.header("X-Accel-Redirect").unwrap().to_owned();
    assert!(mcached.starts_with("/qcache/acme-1--") && mcached.ends_with("--TetherQS-0.4.1-universal.zip"), "{mcached}");
    assert_eq!(std::fs::read(s.dir.join(mcached.trim_start_matches('/'))).unwrap(), mac_zip);
    assert_eq!(s.req("GET", &q, &[("X-Tether-Accel", "1")], b"").header("X-Accel-Redirect"), Some(rel.as_str()));
    assert!(cached.exists() && s.dir.join(mcached.trim_start_matches('/')).exists());
    assert_eq!(s.admin(&k, "GET", "/api/admin/custom", b"").json()[0]["downloads"], 9);

    // The profile for TetherQS on macOS, by mac id (not the token).
    let r = s.req("GET", &format!("/q/m/{mac_id}"), &[], b"");
    assert_eq!((r.status, r.header("Cache-Control")), (200, Some("no-store")));
    let p = custom::open(&r.body, &[k.verifying_key()]).unwrap();
    assert_eq!(p.id, "acme-1");
    assert_eq!(s.req("GET", &format!("/q/m/{}", custom::mac_id(&custom::new_token())), &[], b"").status, 404);
    assert_eq!(s.req("GET", "/q/m/..%2f..%2fpolicy.json", &[], b"").status, 404);
    assert_eq!(s.req("GET", &format!("/q/m/{tok}"), &[], b"").status, 404, "the token is not a mac id");
    assert_eq!(s.admin(&k, "GET", "/api/admin/custom", b"").json()[0]["downloads"], 9, "profile fetches are not downloads");

    // Unknown / malformed tokens: 404.
    assert_eq!(s.req("GET", &format!("/q/{}", custom::new_token()), &[], b"").status, 404);
    assert_eq!(s.req("GET", "/q/..%2f..%2fpolicy.json", &[], b"").status, 404);
    assert_eq!(s.req("GET", &format!("/q/{}", &tok[..42]), &[], b"").status, 404);

    // Replacing with a new token: the old link stops working, counters restart.
    let tok2 = custom::new_token();
    assert_eq!(s.admin(&k, "PUT", path, &custom_body(&k, "acme-1", "ACME", &tok2, exp)).status, 204);
    assert!(!cached.exists(), "prepared copy of the old profile kept");
    assert_eq!(s.req("GET", &q, &[], b"").status, 404);
    assert_eq!(s.req("GET", &format!("/q/m/{mac_id}"), &[], b"").status, 404, "old mac id");
    assert_eq!(s.req("GET", &format!("/q/m/{}", custom::mac_id(&tok2)), &[], b"").status, 200);
    assert_eq!(s.req("GET", &format!("/q/{tok2}"), &[], b"").status, 200);
    assert_eq!(s.admin(&k, "GET", "/api/admin/custom", b"").json()[0]["downloads"], 1);

    // Revoke.
    assert_eq!(s.admin(&k, "DELETE", path, b"").status, 204);
    assert!(!f.exists());
    assert_eq!(s.req("GET", &format!("/q/{tok2}"), &[], b"").status, 404);
    assert_eq!(s.admin(&k, "DELETE", path, b"").status, 404);
    assert_eq!(s.admin(&k, "GET", "/api/admin/custom", b"").json(), serde_json::json!([]));
}

#[test]
fn custom_app_expiry_counters_persist_and_rate_limit() {
    let k = generate_key();
    let keys = key_to_b64(&k.verifying_key());
    let mut s = start(&keys);
    let tok = custom::new_token();
    assert_eq!(s.admin(&k, "PUT", "/api/admin/custom/x", &custom_body(&k, "x", "X", &tok, now() + 2)).status, 204);
    let rel = "/api/admin/release/stable/customer/windows-x86_64";
    assert_eq!(s.admin(&k, "PUT", &format!("{rel}/TetherQS-1.exe"), b"MZ").status, 204);
    assert_eq!(s.admin(&k, "PUT", &format!("{rel}/latest.json"), br#"{"platforms":{"windows-x86_64":{"url":"https://r/TetherQS-1.exe"}}}"#).status, 204);
    let q = format!("/q/{tok}");
    assert_eq!(s.req("GET", &q, &[], b"").status, 200);

    // Download counters survive a restart.
    unsafe { libc::kill(s.child.id() as i32, libc::SIGTERM) };
    s.child.wait().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tether-api"))
        .args(["--data", s.dir.to_str().unwrap(), "--listen", "127.0.0.1:0"])
        .env("ADMIN_PUB_KEY", &keys)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    s.addr = line.trim().strip_prefix("listening on ").unwrap().to_owned();
    s.child = child;
    assert_eq!(s.admin(&k, "GET", "/api/admin/custom", b"").json()[0]["downloads"], 1);

    // Expired: same 404 as unknown, entry still listed for the admin.
    std::thread::sleep(std::time::Duration::from_secs(3));
    assert_eq!(s.req("GET", &q, &[], b"").status, 404);
    assert_eq!(s.req("GET", &format!("{q}/mac"), &[], b"").status, 404);
    assert_eq!(s.admin(&k, "GET", "/api/admin/custom", b"").json().as_array().unwrap().len(), 1);
    // The profile stays available: a copy downloaded before expiry still gets its branding.
    assert_eq!(s.req("GET", &format!("/q/m/{}", custom::mac_id(&tok)), &[], b"").status, 200);

    // 30 per minute per IP (unknown tokens count too), other IPs unaffected.
    let ip = [("X-Forwarded-For", "192.0.2.50")];
    for i in 0..30 {
        assert_eq!(s.req("GET", &q, &ip, b"").status, 404, "req {i}");
    }
    assert_eq!(s.req("GET", &q, &ip, b"").status, 429);
    assert_eq!(s.req("GET", &q, &[("X-Forwarded-For", "192.0.2.51")], b"").status, 404);
}

#[test]
fn state_survives_restart() {
    let k = generate_key();
    let keys = key_to_b64(&k.verifying_key());
    let mut s = start(&keys);
    let doc = policy_doc(&k, r#"{"serial":4,"issued_at":1}"#);
    assert_eq!(s.admin(&k, "PUT", "/api/admin/policy", &doc).status, 204);
    let list = keys_doc(&k, 3, &[&keys]);
    assert_eq!(s.admin(&k, "PUT", "/api/admin/server-keys", &list).status, 204);
    let sig = reqauth::sign_request(&k, "GET", "/api/admin/status", b"", now());
    assert_eq!(s.req("GET", "/api/admin/status", &[(reqauth::HEADER, &sig)], b"").status, 200);
    assert_eq!(s.req("POST", "/api/heartbeat", &[], hb("persist-01").as_bytes()).status, 200);

    // SIGTERM flushes devices; restart on the same data dir.
    unsafe { libc::kill(s.child.id() as i32, libc::SIGTERM) };
    s.child.wait().unwrap();
    let dir = s.dir.clone();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tether-api"))
        .args(["--data", dir.to_str().unwrap(), "--listen", "127.0.0.1:0"])
        .env("ADMIN_PUB_KEY", &keys)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    s.addr = line.trim().strip_prefix("listening on ").unwrap().to_owned();
    s.child = child;

    let r = s.req("GET", "/api/policy", &[], b"");
    assert_eq!(r.header("ETag"), Some("\"4\""));
    // Nonces are persisted: the earlier request cannot be replayed after a restart.
    assert_eq!(s.req("GET", "/api/admin/status", &[(reqauth::HEADER, &sig)], b"").status, 401);
    let st = s.admin(&k, "GET", "/api/admin/status", b"").json();
    assert_eq!(st["devices"], 1);
    assert_eq!(st["policy_serial"], 4);
    assert_eq!(st["server_keys_serial"], 3);
    assert_eq!(s.req("GET", "/api/server-keys", &[], b"").body, list);
}

// ---- session audit log (POST /api/audit/*, GET /api/admin/audit) ----

/// The records vendor/rustdesk/src/server/connection.rs posts, as it builds them.
fn conn_new(conn: i64, nonce: &str) -> String {
    format!(r#"{{"action":"new","conn_id":{conn},"id":"111222333","ip":"198.51.100.20","nonce":"{nonce}","session_id":0,"uuid":"QUJDLTEyMw=="}}"#)
}
fn conn_login(conn: i64, nonce: &str) -> String {
    format!(r#"{{"conn_id":{conn},"id":"111222333","nonce":"{nonce}","peer":["987654321","Tech Anna"],"primary_auth":3,"session_id":18446744073709551615,"type":0,"uuid":"QUJDLTEyMw=="}}"#)
}
fn conn_close(conn: i64, nonce: &str) -> String {
    format!(r#"{{"action":"close","conn_id":{conn},"id":"111222333","nonce":"{nonce}","session_id":42,"uuid":"QUJDLTEyMw=="}}"#)
}
fn file_post(conn: i64, nonce: &str) -> String {
    let info = serde_json::json!({"ip": "198.51.100.20", "name": "Tech Anna", "num": 12, "files": [["report.pdf", 1234], ["b.txt", 5]]}).to_string();
    serde_json::json!({"id": "111222333", "uuid": "QUJDLTEyMw==", "peer_id": "987654321", "conn_id": conn, "type": 1,
        "path": r"C:\Users\x\Desktop", "is_file": false, "info": info, "nonce": nonce})
    .to_string()
}

#[test]
fn audit_log() {
    let (s, k) = admin_server();
    let ip = [("X-Forwarded-For", "203.0.113.50")];
    let post = |kind: &str, body: &str| s.req("POST", &format!("/api/audit/{kind}"), &ip, body.as_bytes());
    for (kind, body) in [("conn", conn_new(7, "n-1")), ("conn", conn_login(7, "n-2")), ("file", file_post(7, "n-3")), ("conn", conn_close(7, "n-4"))] {
        let r = post(kind, &body);
        assert_eq!((r.status, r.body.len()), (204, 0), "{kind} {body}");
    }
    let alarm = r#"{"conn_id":8,"id":"111222333","info":"{\"ip\":\"192.0.2.1\"}","nonce":"n-5","typ":0,"uuid":"QUJDLTEyMw=="}"#;
    assert_eq!(post("alarm", alarm).status, 204);
    // A retry of a stored record (same nonce) is accepted and not stored twice.
    assert_eq!(post("conn", &conn_close(7, "n-4")).status, 204);

    // Rejected: unknown field, wrong action, bad ip, bad id, oversized, bad type, unknown kind, GET.
    assert_eq!(post("conn", &conn_new(9, "x-1").replace(r#""action""#, r#""x":1,"action""#)).status, 400);
    assert_eq!(post("conn", &conn_new(9, "x-2").replace(r#""new""#, r#""delete""#)).status, 400);
    assert_eq!(post("conn", &conn_new(9, "x-3").replace("198.51.100.20", "not-an-ip")).status, 400);
    assert_eq!(post("conn", &conn_new(9, "x-4").replace("111222333", "1 2")).status, 400);
    assert_eq!(post("file", &file_post(9, "x-5").replace("report.pdf", &"r".repeat(9000))).status, 413);
    assert_eq!(post("file", &file_post(9, "x-6").replace(r#""type":1"#, r#""type":5"#)).status, 400);
    assert_eq!(post("nope", &conn_new(9, "x-7")).status, 404);
    assert_eq!(s.req("GET", "/api/audit/conn", &ip, b"").status, 404);

    // Newest first, server timestamps, poster IP from X-Forwarded-For.
    assert_eq!(s.req("GET", "/api/admin/audit", &[], b"").status, 401);
    let all = s.admin(&k, "GET", "/api/admin/audit", b"").json();
    let all = all.as_array().unwrap();
    let actions: Vec<_> = all.iter().map(|r| r["action"].as_str().unwrap()).collect();
    assert_eq!(actions, ["alarm", "disconnect", "receive", "login", "connect"]);
    let (close, file, login, open) = (&all[1], &all[2], &all[3], &all[4]);
    assert!(all.iter().all(|r| r["ts"].as_u64().unwrap() >= now() - 10 && r["src"] == "203.0.113.50" && r["id"] == "111222333"));
    assert_eq!((open["kind"].as_str(), open["ip"].as_str(), open["conn_id"].as_i64()), (Some("conn"), Some("198.51.100.20"), Some(7)));
    assert_eq!((login["peer_id"].as_str(), login["peer_name"].as_str(), login["conn_type"].as_str()), (Some("987654321"), Some("Tech Anna"), Some("remote")));
    assert_eq!((login["primary_auth"].as_str(), login["session_id"].as_str()), (Some("permanent_password"), Some("18446744073709551615")));
    assert_eq!((file["kind"].as_str(), file["path"].as_str(), file["num"].as_u64()), (Some("file"), Some(r"C:\Users\x\Desktop"), Some(12)));
    assert_eq!(file["files"], serde_json::json!([["report.pdf", 1234], ["b.txt", 5]]));
    assert_eq!(close["uuid"], "QUJDLTEyMw==");

    // Filters.
    let q = |p: &str| s.admin(&k, "GET", &format!("/api/admin/audit?{p}"), b"").json().as_array().unwrap().len();
    assert_eq!(q("limit=2"), 2);
    assert_eq!(q("device=111222333"), 5);
    assert_eq!(q("device=QUJDLTEyMw%3D%3D"), 5);
    assert_eq!(q("device=000"), 0);
    assert_eq!(q(&format!("since={}", now() + 60)), 0);
    assert_eq!(s.admin(&k, "GET", "/api/admin/audit?limit=0", b"").status, 400);
    assert_eq!(s.admin(&k, "GET", "/api/admin/audit?limit=5001", b"").status, 400);
    assert_eq!(s.admin(&k, "GET", "/api/admin/audit?x=1", b"").status, 400);

    // One JSON line per record under audit/<UTC day>.jsonl, private to the service.
    let files: Vec<_> = std::fs::read_dir(s.dir.join("audit")).unwrap().flatten().collect();
    assert_eq!(files.len(), 1);
    assert!(files[0].file_name().to_str().unwrap().ends_with(".jsonl"));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(files[0].metadata().unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::read_to_string(files[0].path()).unwrap().lines().count(), 5);

    // Devices are not touched.
    assert_eq!(s.admin(&k, "GET", "/api/admin/status", b"").json()["devices"], 0);
}

#[test]
fn audit_rate_limits() {
    let (s, k) = admin_server();
    // 60 per minute per device, then 429 (the client retries 429 later).
    let ip = [("X-Forwarded-For", "203.0.113.60")];
    for i in 0..60 {
        assert_eq!(s.req("POST", "/api/audit/conn", &ip, conn_close(i, &format!("d-{i}")).as_bytes()).status, 204, "post {i}");
    }
    assert_eq!(s.req("POST", "/api/audit/conn", &ip, conn_close(99, "d-x").as_bytes()).status, 429);
    // 120 per minute per IP across devices (the 61 above count too).
    let ip2 = [("X-Forwarded-For", "203.0.113.61")];
    for i in 0..120 {
        let body = conn_close(i, &format!("e-{i}")).replace("QUJDLTEyMw==", &format!("dev{i}"));
        assert_eq!(s.req("POST", "/api/audit/conn", &ip2, body.as_bytes()).status, 204, "post {i}");
    }
    assert_eq!(s.req("POST", "/api/audit/conn", &ip2, conn_close(1, "e-x").replace("QUJDLTEyMw==", "other").as_bytes()).status, 429);
    assert_eq!(s.admin(&k, "GET", "/api/admin/audit?limit=5000", b"").json().as_array().unwrap().len(), 180);
}

/// UTC `YYYY-MM-DD` of `now - days`.
fn utc_day(days: u64) -> String {
    let out = Command::new("date").args(["-u", "-d", &format!("@{}", now() - days * 86_400), "+%F"]).output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

#[test]
fn audit_retention_and_cap() {
    let k = generate_key();
    let keys = key_to_b64(&k.verifying_key());
    let (old, recent) = (utc_day(40), utc_day(3));
    let line = |days: u64| format!("{{\"ts\":{},\"id\":\"1\",\"action\":\"connect\"}}\n", now() - days * 86_400);
    let s = start_env(&keys, &[("AUDIT_RETENTION_DAYS", "30")], |dir| {
        std::fs::create_dir_all(dir.join("audit")).unwrap();
        std::fs::write(dir.join(format!("audit/{old}.jsonl")), line(40)).unwrap();
        std::fs::write(dir.join(format!("audit/{recent}.jsonl")), line(3)).unwrap();
    });
    // Startup housekeeping drops the day past retention, keeps the other.
    assert!(!s.dir.join(format!("audit/{old}.jsonl")).exists());
    assert!(s.dir.join(format!("audit/{recent}.jsonl")).exists());
    assert_eq!(s.admin(&k, "GET", "/api/admin/audit", b"").json().as_array().unwrap().len(), 1);
    let since = s.admin(&k, "GET", &format!("/api/admin/audit?since={}", now() - 86_400), b"").json();
    assert_eq!(since.as_array().unwrap().len(), 0);

    // Size cap: older days go first; today alone over the cap is refused (507) rather than grown.
    let s = start_env(&keys, &[("AUDIT_MAX_MB", "0")], |dir| {
        std::fs::create_dir_all(dir.join("audit")).unwrap();
        std::fs::write(dir.join(format!("audit/{recent}.jsonl")), line(3)).unwrap();
    });
    assert_eq!(s.req("POST", "/api/audit/conn", &[], conn_close(1, "c-1").as_bytes()).status, 507);
    assert!(!s.dir.join(format!("audit/{recent}.jsonl")).exists());

    // Retention 0: accepted (no client noise) but not stored.
    let s = start_env(&keys, &[("AUDIT_RETENTION_DAYS", "0")], |_| {});
    assert_eq!(s.req("POST", "/api/audit/conn", &[], conn_close(1, "z-1").as_bytes()).status, 204);
    assert_eq!(s.admin(&k, "GET", "/api/admin/audit", b"").json().as_array().unwrap().len(), 0);
}

/// A client that sends the head and part of a heartbeat body, then stalls.
fn stalled_client(s: &Srv) -> TcpStream {
    let mut c = TcpStream::connect(&s.addr).unwrap();
    c.write_all(b"POST /api/heartbeat HTTP/1.1\r\nHost: t\r\nContent-Length: 2000\r\n\r\n").unwrap();
    c.write_all(&[b' '; 1500]).unwrap();
    c
}

#[test]
fn stalled_client_does_not_block_others() {
    let s = start("");
    let stalled = stalled_client(&s);
    std::thread::sleep(std::time::Duration::from_millis(300)); // the server takes it first
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::scope(|sc| {
        sc.spawn(|| tx.send(s.req("GET", "/api/policy", &[], b"").status));
        let got = rx.recv_timeout(std::time::Duration::from_secs(5));
        drop(stalled); // unblocks a serial server, so the scope can end either way
        assert_eq!(got.ok(), Some(404), "a stalled client blocked the API");
    });
}

#[test]
fn stalled_body_read_times_out() {
    let s = start("");
    let mut c = stalled_client(&s);
    c.set_read_timeout(Some(std::time::Duration::from_secs(60))).unwrap();
    let t0 = std::time::Instant::now();
    let mut raw = Vec::new();
    let mut buf = [0u8; 512];
    while !raw.windows(2).any(|w| w == b"\r\n") {
        match c.read(&mut buf) {
            Ok(n) if n > 0 => raw.extend_from_slice(&buf[..n]),
            _ => break,
        }
    }
    let took = t0.elapsed().as_secs();
    assert!(raw.starts_with(b"HTTP/1.1 400"), "no 400 after {took} s: {:?}", String::from_utf8_lossy(&raw));
    assert!((20..50).contains(&took), "{took} s");
}

fn index_doc(key: &SigningKey, channel: &str, serial: u64) -> Vec<u8> {
    index_doc_issued(key, channel, serial, 1)
}

fn index_doc_issued(key: &SigningKey, channel: &str, serial: u64, issued: u64) -> Vec<u8> {
    let entry = serde_json::json!({
        "version": "0.5.24", "file": "Tether_0.5.24_x64-setup.exe", "size": 3, "sha256": "a".repeat(64), "signature": "U0lH",
        "urls": ["https://{relay}/download/stable/technician/windows-x86_64/Tether_0.5.24_x64-setup.exe"],
        "rollout": {"start": 0, "steps": [[0, 100]]}
    });
    let payload = serde_json::json!({"type": "tether-releases/1", "channel": channel, "serial": serial, "issued": issued, "expires": 2_000_000_000u64,
        "releases": {"technician/windows-x86_64": entry}});
    serde_json::to_vec(&envelope::sign(&serde_json::to_vec(&payload).unwrap(), key)).unwrap()
}

fn generation(s: &Srv, uuid: &str) -> u64 {
    let r = s.req("POST", "/api/heartbeat", &[("X-Real-IP", uuid)], hb(uuid).as_bytes());
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    r.json()["releases"].as_u64().unwrap()
}

/// Release generation G, the heartbeat hint and the signed release index (docs/update-design.md).
#[test]
fn release_generation_heartbeat_hint_and_index() {
    let (s, k) = admin_server();
    assert_eq!(generation(&s, "gen-0001"), 0);
    let rel = "/api/admin/release/stable/technician/windows-x86_64";
    // An artifact does not change G; a manifest does.
    assert_eq!(s.admin(&k, "PUT", &format!("{rel}/Tether_0.5.24_x64-setup.exe"), b"exe").status, 204);
    assert_eq!(generation(&s, "gen-0002"), 0);
    assert_eq!(s.admin(&k, "PUT", &format!("{rel}/latest.json"), br#"{"version":"0.5.24"}"#).status, 204);
    assert_eq!(generation(&s, "gen-0003"), 1);
    assert_eq!(std::fs::read_to_string(s.dir.join("release-generation")).unwrap().trim(), "1");

    // The index: signed by a trusted admin key, for the channel of its path, serials only go up.
    let p = "/api/admin/release/stable/releases.json";
    assert_eq!(s.admin(&k, "PUT", p, &index_doc(&generate_key(), "stable", 5)).status, 400, "foreign key");
    assert_eq!(s.admin(&k, "PUT", p, &index_doc(&k, "test", 5)).status, 400, "other channel");
    assert_eq!(s.admin(&k, "PUT", p, br#"{"version":"0.5.24"}"#).status, 400, "unsigned");
    assert!(!s.dir.join("download/stable/releases.json").exists());
    assert_eq!(generation(&s, "gen-0004"), 1, "refused uploads do not bump G");
    let doc5 = index_doc(&k, "stable", 5);
    assert_eq!(s.admin(&k, "PUT", p, &doc5).status, 204);
    assert_eq!(s.admin(&k, "PUT", p, &doc5).status, 204, "the same document again (publish to missing relays)");
    assert_eq!(s.admin(&k, "PUT", p, &index_doc(&k, "stable", 4)).status, 409);
    assert_eq!(s.admin(&k, "PUT", p, &index_doc_issued(&k, "stable", 5, 2)).status, 409, "same serial, other bytes");
    assert_eq!(generation(&s, "gen-0005"), 3);

    // Served with the serial as ETag (equal on every relay), 304 on If-None-Match, no-cache.
    let r = s.req("GET", "/download/stable/releases.json", &[], b"");
    assert_eq!((r.status, r.header("ETag"), r.header("Cache-Control")), (200, Some("\"5\""), Some("no-cache")));
    assert_eq!(r.body, doc5);
    assert_eq!(s.req("GET", "/download/stable/releases.json", &[("If-None-Match", "\"5\"")], b"").status, 304);
    assert_eq!(s.req("GET", "/download/test/releases.json", &[], b"").status, 404);
    assert_eq!(s.admin(&k, "PUT", p, &index_doc(&k, "stable", 6)).status, 204);
    assert_eq!(s.req("GET", "/download/stable/releases.json", &[("If-None-Match", "\"5\"")], b"").status, 200);
    let status = s.admin(&k, "GET", "/api/admin/status", b"").json();
    assert_eq!((status["release_generation"].as_u64(), status["release_indexes"]["stable"]["serial"].as_u64()), (Some(4), Some(6)));

    // releases-v2.json (xz copies, new builds): the same rules, its own serial, v1 untouched.
    let p2 = "/api/admin/release/stable/releases-v2.json";
    assert_eq!(s.req("GET", "/download/stable/releases-v2.json", &[], b"").status, 404);
    assert_eq!(s.admin(&k, "PUT", p2, &index_doc(&generate_key(), "stable", 6)).status, 400, "foreign key");
    let doc6 = index_doc(&k, "stable", 6);
    assert_eq!(s.admin(&k, "PUT", p2, &doc6).status, 204);
    assert_eq!(s.admin(&k, "PUT", p2, &index_doc(&k, "stable", 5)).status, 409);
    let r = s.req("GET", "/download/stable/releases-v2.json", &[], b"");
    assert_eq!((r.status, r.header("ETag"), r.body.clone()), (200, Some("\"6\""), doc6));
    assert_eq!(s.req("GET", "/download/stable/releases.json", &[], b"").header("ETag"), Some("\"6\""));
    assert_eq!(generation(&s, "gen-0007"), 5, "v2 bumps G too");
    // Deleted on disk behind tether-api's back (scripts/release-index-v1.sh): 404 at once, also
    // for a client that sends the old ETag.
    let f2 = s.dir.join("download/stable/releases-v2.json");
    std::fs::rename(&f2, s.dir.join("v2.bak")).unwrap();
    assert_eq!(s.req("GET", "/download/stable/releases-v2.json", &[("If-None-Match", "\"6\"")], b"").status, 404);
    std::fs::rename(s.dir.join("v2.bak"), &f2).unwrap();
    assert_eq!(s.req("GET", "/download/stable/releases-v2.json", &[("If-None-Match", "\"6\"")], b"").status, 304);
    assert_eq!(s.admin(&k, "DELETE", p2, b"").status, 409, "v2 only with force");
    assert_eq!(s.admin(&k, "DELETE", &format!("{p2}?force=1"), b"").status, 204);
    assert_eq!(s.req("GET", "/download/stable/releases-v2.json", &[], b"").status, 404);
    assert_eq!(s.req("GET", "/download/stable/releases.json", &[], b"").status, 200, "v1 stays");
    assert_eq!(generation(&s, "gen-0008"), 6);

    // Deleting the index bumps G and stops serving it.
    assert_eq!(s.admin(&k, "DELETE", p, b"").status, 409, "the index only with force");
    assert_eq!(s.admin(&k, "DELETE", &format!("{p}?force=1"), b"").status, 204);
    assert_eq!(s.req("GET", "/download/stable/releases.json", &[], b"").status, 404);
    assert_eq!(generation(&s, "gen-0009"), 7);
    let m = s.req("GET", "/metrics", &[], b"");
    let text = String::from_utf8_lossy(&m.body);
    assert!(text.contains("tether_api_release_generation 7"), "{text}");
    assert!(text.contains("tether_api_requests_total{endpoint=\"release_index\",code=\"304\"} 1"));
}

/// New heartbeat fields, validated; old clients take any 2xx (technician: `is_success()`,
/// TetherQS ignores the body), so the 200 with a JSON body is compatible.
#[test]
fn heartbeat_update_fields_and_old_clients() {
    let (s, _) = admin_server();
    let with = |uuid: &str, extra: &str| format!(r#"{{"uuid":"{uuid}","id":"1","profile":"customer","version":"0.5.23","os":"w","serial":1{extra}}}"#);
    for (i, (extra, code)) in [
        (r#","release_serial":5,"update_state":"staged","update_version":"0.5.24""#, 200),
        (r#","update_state":"waiting-rollout""#, 200),
        (r#","update_state":"pwned""#, 400),
        (r#","update_version":"../1""#, 400),
        (r#","update_state":"staged","update_version":"0.5.24""#, 200),
    ]
    .iter()
    .enumerate()
    {
        let r = s.req("POST", "/api/heartbeat", &[("X-Real-IP", "10.0.0.9")], with(&format!("hbnew-{i:03}"), extra).as_bytes());
        assert_eq!(r.status, *code, "{extra}");
        if *code == 200 {
            // What a 0.5.23 client does with the answer: success, body irrelevant.
            assert!((200..300).contains(&r.status));
            assert!(r.json()["releases"].is_u64());
        }
    }
    let m = String::from_utf8_lossy(&s.req("GET", "/metrics", &[], b"").body).into_owned();
    assert!(m.contains("tether_api_release_downloads_total{profile=\"customer\",version=\"0.5.24\"} 2"), "{m}");
    assert!(m.contains("tether_api_devices_update_state{state=\"staged\"} 2"));
    assert!(m.contains("tether_api_devices_by_version{profile=\"customer\",version=\"0.5.23\"} 3"));
}

/// G survives a restart (read from `release-generation`).
#[test]
fn release_generation_persists() {
    let k = generate_key();
    let s = start_env(&key_to_b64(&k.verifying_key()), &[], |d| std::fs::write(d.join("release-generation"), "41\n").unwrap());
    assert_eq!(generation(&s, "persist-g1"), 41);
}

/// Sends the head of a `method` request with `len` bytes of declared body and one byte of it.
fn partial_upload(s: &Srv, method: &str, path: &str, headers: &[(&str, &str)], len: u64) -> TcpStream {
    let mut c = TcpStream::connect(&s.addr).unwrap();
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: t\r\nContent-Length: {len}\r\n");
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    c.write_all(head.as_bytes()).unwrap();
    c.write_all(b"x").unwrap();
    c
}

/// The response head on `c`, if one arrives within `wait`.
fn head_of(c: &mut TcpStream, wait: Duration) -> Option<String> {
    c.set_read_timeout(Some(wait)).unwrap();
    let mut raw = Vec::new();
    let mut buf = [0u8; 1024];
    while !raw.windows(4).any(|w| w == b"\r\n\r\n") {
        match c.read(&mut buf) {
            Ok(n) if n > 0 => raw.extend_from_slice(&buf[..n]),
            _ => return None,
        }
    }
    Some(String::from_utf8_lossy(&raw).into_owned())
}

/// Release and diagnostics uploads that send slowly hold at most MAX_UPLOADS (4) workers; the
/// rest are refused at once, and so is an upload that fails the checks possible before its body.
#[test]
fn slow_uploads_leave_workers_for_heartbeats() {
    let (s, k) = admin_server();
    let p = "/api/admin/release/stable/technician/windows-x86_64/big.exe";
    let sig = reqauth::sign_request(&k, "PUT", p, b"", now());
    // Paced: 40 at the same instant can exceed MAX_WORKERS (32) on a slow machine, which answers a plain 503 "busy".
    let mut conns: Vec<TcpStream> = (0..40)
        .map(|_| {
            std::thread::sleep(Duration::from_millis(10));
            partial_upload(&s, "PUT", p, &[(reqauth::HEADER, &sig)], 100_000_000)
        })
        .collect();
    std::thread::sleep(Duration::from_millis(500));
    let refused: Vec<String> = conns.iter_mut().filter_map(|c| head_of(c, Duration::from_millis(1500))).collect();
    assert_eq!(refused.len(), 36, "{refused:?}");
    assert!(refused.iter().all(|h| h.starts_with("HTTP/1.1 503") && h.contains("Retry-After: 30")), "{refused:?}");
    for i in 0..10 {
        let r = s.req("POST", "/api/heartbeat", &[("X-Forwarded-For", "192.0.2.77")], hb(&format!("slow-{i:04}")).as_bytes());
        assert_eq!(r.status, 200, "heartbeat {i} while uploads stall");
    }
    // Refused before the body is read, while all upload slots are taken.
    let t0 = Instant::now();
    let stale = reqauth::sign_request(&k, "PUT", p, b"", now() - 1000);
    for (method, path, h, want) in [
        ("PUT", p, vec![], "HTTP/1.1 401"),
        ("PUT", p, vec![(reqauth::HEADER, stale.as_str())], "HTTP/1.1 401"),
        ("PUT", "/api/admin/release/stable/../x", vec![(reqauth::HEADER, sig.as_str())], "HTTP/1.1 400"),
        ("POST", "/api/diag/AAAAAAAA", vec![], "HTTP/1.1 404"),
    ] {
        let mut c = partial_upload(&s, method, path, &h, 100_000_000);
        let got = head_of(&mut c, Duration::from_secs(3)).unwrap_or_default();
        assert!(got.starts_with(want), "{method} {path}: {got}");
    }
    assert!(t0.elapsed() < Duration::from_secs(3));
    drop(conns);
}

/// An upload that trickles (1 byte a second) is cut off once it averages under 16 KiB/s after
/// 30 s, instead of holding its worker until the 200 MiB deadline (about 55 min).
#[test]
fn trickled_upload_is_cut_off() {
    let (s, k) = admin_server();
    let p = "/api/admin/release/stable/technician/windows-x86_64/big.exe";
    let sig = reqauth::sign_request(&k, "PUT", p, b"", now());
    let mut c = partial_upload(&s, "PUT", p, &[(reqauth::HEADER, &sig)], 10_000_000);
    let t0 = Instant::now();
    let head = loop {
        if let Some(h) = head_of(&mut c, Duration::from_secs(1)) {
            break h;
        }
        let _ = c.write_all(b"x");
        assert!(t0.elapsed() < Duration::from_secs(50), "trickle not cut off");
    };
    let took = t0.elapsed().as_secs();
    assert!(head.starts_with("HTTP/1.1 408"), "{head}");
    assert!((29..45).contains(&took), "{took} s");
}

/// tiny_http allocated the whole remaining Content-Length to discard an unread body: one request
/// with a huge one aborted the process.
#[test]
fn huge_content_length_is_refused_without_a_crash() {
    let (s, k) = admin_server();
    let p = "/api/admin/release/stable/technician/windows-x86_64/big.exe";
    let sig = reqauth::sign_request(&k, "PUT", p, b"", now());
    for (method, path, h, want) in [
        ("POST", "/api/heartbeat", vec![], "HTTP/1.1 413"),
        ("POST", "/api/diag/AAAAAAAA", vec![], "HTTP/1.1 404"),
        ("PUT", p, vec![(reqauth::HEADER, sig.as_str())], "HTTP/1.1 413"),
    ] {
        let mut c = partial_upload(&s, method, path, &h, 100_000_000_000_000);
        let got = head_of(&mut c, Duration::from_secs(5)).unwrap_or_default();
        assert!(got.starts_with(want), "{method} {path}: {got}");
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(s.admin(&k, "GET", "/api/admin/status", b"").status, 200, "server gone after {method} {path}");
    }
}

/// `devices.json` with `n` devices (newest first by index).
fn devices_json(n: usize) -> String {
    let t = now();
    let v: Vec<serde_json::Value> = (0..n)
        .map(|i| {
            serde_json::json!({"uuid": format!("{i:08x}-1111-2222-3333-444455556666"), "id": format!("{}", 100_000_000 + i), "profile": "customer",
                "version": "0.5.25", "os": "windows 10.0.26100 x86_64", "serial": 12, "server_keys_serial": 3, "release_serial": 7,
                "update_state": "idle", "update_version": "", "ip": format!("198.51.{}.{}", i / 256 % 256, i % 256), "last_seen": t - i as u64})
        })
        .collect();
    serde_json::to_string(&v).unwrap()
}

fn sigterm_and_wait(s: &mut Srv) {
    let st = Command::new("kill").args(["-TERM", &s.child.id().to_string()]).status().unwrap();
    assert!(st.success());
    let _ = s.child.wait();
}

/// Team access for one member (`key`), via the admin API.
fn add_member(s: &Srv, admin: &SigningKey, key: &SigningKey) {
    let roster = serde_json::json!({"members": [{"name": "M", "key": key_to_b64(&key.verifying_key()), "write": false, "audit": true}]});
    assert_eq!(s.admin(admin, "PUT", "/api/admin/team", roster.to_string().as_bytes()).status, 204);
}

/// Device lists and the devices.json flush are written from the map (no copy of every device);
/// same results as before.
#[test]
fn many_devices_lists_and_flush() {
    let k = generate_key();
    let n = 5000;
    let mut s = start_env(&key_to_b64(&k.verifying_key()), &[], |d| std::fs::write(d.join("devices.json"), devices_json(n)).unwrap());
    let member = generate_key();
    add_member(&s, &k, &member);
    let all = s.admin(&k, "GET", "/api/admin/devices", b"");
    assert_eq!(all.header("Content-Type"), Some("application/json"));
    let all = all.json();
    assert_eq!(all.as_array().unwrap().len(), n);
    assert_eq!(all[0]["uuid"], "00000000-1111-2222-3333-444455556666", "newest first");
    assert_eq!(all[n - 1]["ip"], format!("198.51.{}.{}", (n - 1) / 256 % 256, (n - 1) % 256));
    let r = s.admin(&member, "GET", "/api/team/devices", b"");
    let team = r.json();
    assert_eq!(team.as_array().unwrap().len(), n);
    assert_eq!(team[0], serde_json::json!({"uuid": all[0]["uuid"], "id": all[0]["id"], "profile": "customer", "version": "0.5.25",
        "os": "windows 10.0.26100 x86_64", "last_seen": all[0]["last_seen"]}));
    assert!(!String::from_utf8_lossy(&r.body).contains("198.51."), "IPs in the member list");
    // Flushed on shutdown with the new device.
    assert_eq!(s.req("POST", "/api/heartbeat", &[("X-Forwarded-For", "192.0.2.5")], hb("flushed-0001").as_bytes()).status, 200);
    sigterm_and_wait(&mut s);
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(s.dir.join("devices.json")).unwrap()).unwrap();
    let saved = saved.as_array().unwrap();
    assert_eq!(saved.len(), n + 1);
    assert!(saved.iter().any(|d| d["uuid"] == "flushed-0001" && d["ip"] == "192.0.2.5"));
}

/// "<peak RSS>/<RSS> KiB" of a process.
fn vm_hwm_kib(pid: u32) -> String {
    let st = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    let kib = |k: &str| st.lines().find_map(|l| l.strip_prefix(k)).unwrap().trim().trim_end_matches(" kB").trim().to_owned();
    format!("{}/{}", kib("VmHWM:"), kib("VmRSS:"))
}

/// Peak RSS at MAX_DEVICES (docs/scaling.md). Run by hand:
/// `cargo test --release --test api -- --ignored --nocapture many_devices_memory`, with
/// TETHER_API_BIN=<other build> to measure another binary. Takes about 70 s (waits for the
/// periodic flush).
#[test]
#[ignore]
fn many_devices_memory() {
    let k = generate_key();
    let n = 50_000;
    let bin = std::env::var("TETHER_API_BIN").unwrap_or_else(|_| env!("CARGO_BIN_EXE_tether-api").to_owned());
    let dir = std::env::temp_dir().join(format!("tether-api-mem-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("devices.json"), devices_json(n)).unwrap();
    let mut child = Command::new(bin)
        .args(["--data", dir.to_str().unwrap(), "--listen", "127.0.0.1:0"])
        .env("ADMIN_PUB_KEY", key_to_b64(&k.verifying_key()))
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let addr = line.trim().strip_prefix("listening on ").unwrap().to_owned();
    let pid = child.id();
    let s = Srv { child, addr, dir };
    let member = generate_key();
    add_member(&s, &k, &member);
    let start = vm_hwm_kib(pid);
    // Status only: older builds send large lists chunked, which this client does not decode.
    assert_eq!(s.admin(&k, "GET", "/api/admin/devices", b"").status, 200);
    let admin_list = vm_hwm_kib(pid);
    std::thread::scope(|sc| {
        for _ in 0..16 {
            sc.spawn(|| assert_eq!(s.admin(&member, "GET", "/api/team/devices", b"").status, 200));
        }
    });
    let team_lists = vm_hwm_kib(pid);
    let before = std::fs::metadata(s.dir.join("devices.json")).unwrap().modified().unwrap();
    assert_eq!(s.req("POST", "/api/heartbeat", &[("X-Forwarded-For", "192.0.2.6")], hb("memtest-0001").as_bytes()).status, 200);
    let t0 = Instant::now();
    while std::fs::metadata(s.dir.join("devices.json")).unwrap().modified().unwrap() == before {
        assert!(t0.elapsed() < Duration::from_secs(90), "no flush");
        std::thread::sleep(Duration::from_millis(200));
    }
    std::thread::sleep(Duration::from_millis(500));
    let flush = vm_hwm_kib(pid);
    println!("devices={n} peak/current RSS KiB: after start {start}, after admin list {admin_list}, after 16 concurrent team lists {team_lists}, after flush {flush}");
}

/// mkfifo: a "file" whose open blocks until a writer comes, to hold a handler inside its file work.
fn fifo(p: &std::path::Path) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    assert!(Command::new("mkfifo").arg(p).status().unwrap().success());
}

/// Lets the reader blocked on `p` go on (it sees an empty file).
fn release_fifo(p: &std::path::Path) {
    // Non-blocking: without a reader (the handler never got there) this fails instead of hanging.
    use std::os::unix::fs::OpenOptionsExt;
    let _ = std::fs::OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open(p);
}

/// The audit read and the preparation of a `/q/` copy run without the state lock: a heartbeat is
/// answered while either is stuck in its file work.
#[test]
fn file_work_runs_without_the_state_lock() {
    let (s, k) = admin_server();
    let tok = custom::new_token();
    assert_eq!(s.admin(&k, "PUT", "/api/admin/custom/lock-1", &custom_body(&k, "lock-1", "Lock", &tok, now() + 3600)).status, 204);
    let dir = s.dir.join("download/stable/customer/windows-x86_64");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("latest.json"), br#"{"version":"0.5.25","platforms":{"windows-x86_64":{"signature":"x","url":"https://r/download/stable/customer/windows-x86_64/TetherQS-0.5.25.exe"}}}"#).unwrap();
    let audit_fifo = s.dir.join("audit/2099-01-01.jsonl");
    let release_fifo_path = dir.join("TetherQS-0.5.25.exe");
    fifo(&audit_fifo);
    fifo(&release_fifo_path);
    let q = format!("/q/{tok}");
    std::thread::scope(|sc| {
        let audit = sc.spawn(|| s.admin(&k, "GET", "/api/admin/audit?device=dev-1", b"").status);
        let download = sc.spawn(|| s.req("GET", &q, &[("X-Tether-Accel", "1")], b"").status);
        std::thread::sleep(Duration::from_millis(500));
        let (tx, rx) = std::sync::mpsc::channel();
        let s = &s;
        sc.spawn(move || tx.send(s.req("POST", "/api/heartbeat", &[("X-Forwarded-For", "192.0.2.8")], hb("lockfree-01").as_bytes()).status));
        let got = rx.recv_timeout(Duration::from_secs(5));
        release_fifo(&audit_fifo);
        release_fifo(&release_fifo_path);
        assert_eq!(got.ok(), Some(200), "heartbeat blocked by file work under the lock");
        assert_eq!(audit.join().unwrap(), 200);
        assert_eq!(download.join().unwrap(), 404, "a FIFO is not a release file");
    });
}

/// Custom-app profiles are not kept in memory: the macOS profile and the exe trailer come from
/// `custom/<id>.json`, and saving the download counter keeps the profile in it.
#[test]
fn custom_profile_is_read_from_disk() {
    let (s, k) = admin_server();
    let tok = custom::new_token();
    assert_eq!(s.admin(&k, "PUT", "/api/admin/custom/disk-1", &custom_body(&k, "disk-1", "Disk", &tok, now() + 3600)).status, 204);
    let f = s.dir.join("custom/disk-1.json");
    let mut doc: serde_json::Value = serde_json::from_slice(&std::fs::read(&f).unwrap()).unwrap();
    doc["profile"] = serde_json::to_value(envelope::sign(br#"{"changed":true}"#, &k)).unwrap();
    std::fs::write(&f, doc.to_string()).unwrap();
    let r = s.req("GET", &format!("/q/m/{}", custom::mac_id(&tok)), &[], b"");
    assert_eq!((r.status, r.json()), (200, doc["profile"].clone()));

    let exe = vec![0x4du8; 1000];
    let rel = "/api/admin/release/stable/customer/windows-x86_64";
    assert_eq!(s.admin(&k, "PUT", &format!("{rel}/TetherQS-0.5.25.exe"), &exe).status, 204);
    let latest = br#"{"version":"0.5.25","platforms":{"windows-x86_64":{"signature":"x","url":"https://r/download/stable/customer/windows-x86_64/TetherQS-0.5.25.exe"}}}"#;
    assert_eq!(s.admin(&k, "PUT", &format!("{rel}/latest.json"), latest).status, 204);
    let r = s.req("GET", &format!("/q/{tok}"), &[], b"");
    assert_eq!(r.status, 200);
    assert_eq!(custom::split(&r.body).1.unwrap(), serde_json::to_vec(&doc["profile"]).unwrap());
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&f).unwrap()).unwrap();
    assert_eq!((saved["downloads"].as_u64(), &saved["profile"]), (Some(1), &doc["profile"]));
}

/// Concurrent first downloads behind the proxy prepare the copy at the same time: every answer
/// names one complete copy.
#[test]
fn concurrent_first_q_downloads() {
    let (s, k) = admin_server();
    let tok = custom::new_token();
    assert_eq!(s.admin(&k, "PUT", "/api/admin/custom/race-1", &custom_body(&k, "race-1", "Race", &tok, now() + 3600)).status, 204);
    let exe: Vec<u8> = (0..3_000_000u32).map(|i| i as u8).collect();
    let rel = "/api/admin/release/stable/customer/windows-x86_64";
    assert_eq!(s.admin(&k, "PUT", &format!("{rel}/TetherQS-0.5.25.exe"), &exe).status, 204);
    let latest = br#"{"version":"0.5.25","platforms":{"windows-x86_64":{"signature":"x","url":"https://r/download/stable/customer/windows-x86_64/TetherQS-0.5.25.exe"}}}"#;
    assert_eq!(s.admin(&k, "PUT", &format!("{rel}/latest.json"), latest).status, 204);
    let q = format!("/q/{tok}");
    let paths: Vec<String> = std::thread::scope(|sc| {
        let hs: Vec<_> = (0..8).map(|_| sc.spawn(|| s.req("GET", &q, &[("X-Tether-Accel", "1")], b""))).collect();
        hs.into_iter()
            .map(|h| {
                let r = h.join().unwrap();
                assert_eq!(r.status, 204);
                r.header("X-Accel-Redirect").unwrap().to_owned()
            })
            .collect()
    });
    assert!(paths.iter().all(|p| *p == paths[0]), "{paths:?}");
    let want = s.req("GET", &q, &[], b"").body;
    assert_eq!(std::fs::read(s.dir.join(paths[0].trim_start_matches('/'))).unwrap(), want);
    assert_eq!(std::fs::read_dir(s.dir.join("qcache")).unwrap().count(), 1);
    assert_eq!(s.admin(&k, "GET", "/api/admin/custom", b"").json()[0]["downloads"], 9);
}

/// The in-memory index serial changes only once the new index is in place: after a failed rename
/// the old one is still served, with its serial.
#[test]
fn failed_index_rename_keeps_the_serial() {
    let (s, k) = admin_server();
    let p = "/api/admin/release/stable/releases.json";
    assert_eq!(s.admin(&k, "PUT", p, &index_doc(&k, "stable", 1)).status, 204);
    let f = s.dir.join("download/stable/releases.json");
    let old = std::fs::read(&f).unwrap();
    // A non-empty directory where the index goes: the rename fails.
    std::fs::remove_file(&f).unwrap();
    std::fs::create_dir_all(f.join("x")).unwrap();
    assert_eq!(s.admin(&k, "PUT", p, &index_doc(&k, "stable", 2)).status, 500);
    assert_eq!(s.admin(&k, "GET", "/api/admin/status", b"").json()["release_indexes"]["stable"]["serial"], 1);
    std::fs::remove_dir_all(&f).unwrap();
    std::fs::write(&f, &old).unwrap();
    assert_eq!(s.req("GET", "/download/stable/releases.json", &[], b"").header("ETag"), Some("\"1\""));
    assert_eq!(s.admin(&k, "PUT", p, &index_doc(&k, "stable", 2)).status, 204);
    assert_eq!(s.admin(&k, "GET", "/api/admin/status", b"").json()["release_indexes"]["stable"]["serial"], 2);
}

/// GET and DELETE routes take no body, and nothing upgrades: such a request is refused at once
/// instead of holding one of the 32 workers until its body arrived or timed out.
#[test]
fn unexpected_bodies_do_not_hold_workers() {
    let s = start("");
    let mut held = Vec::new();
    for i in 0..33 {
        let mut c = match i {
            0 => partial_upload(&s, "POST", "/api/heartbeat", &[("Connection", "Upgrade")], 2000),
            1 => partial_upload(&s, "DELETE", "/api/admin/custom/x", &[], 2000),
            _ => partial_upload(&s, "GET", "/api/policy", &[], 2000),
        };
        let got = head_of(&mut c, Duration::from_secs(3)).unwrap_or_default();
        assert!(got.starts_with("HTTP/1.1 400") && got.contains("Connection: close"), "request {i}: {got}");
        held.push(c);
    }
    assert_eq!(s.req("GET", "/api/policy", &[], b"").status, 404);
}

/// With all workers taken, the `503 busy` answer is written by the thread that takes every
/// request: it must not wait for the refused request's body.
#[test]
fn busy_answer_does_not_wait_for_the_body() {
    let s = start("");
    let _stalled: Vec<TcpStream> = (0..32).map(|_| stalled_client(&s)).collect();
    std::thread::sleep(Duration::from_millis(500));
    let mut one_more = stalled_client(&s);
    let got = head_of(&mut one_more, Duration::from_secs(5)).unwrap_or_default();
    assert!(got.starts_with("HTTP/1.1 503"), "{got}");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::scope(|sc| {
        sc.spawn(|| tx.send(s.req("GET", "/api/policy", &[], b"").status));
        let got = rx.recv_timeout(Duration::from_secs(5));
        drop(one_more); // unblocks a waiting server, so the scope can end either way
        assert_eq!(got.ok(), Some(503), "a stalled request blocked the accept loop");
    });
}

/// tiny_http reads a body of up to 1024 bytes itself, before tether-api has the request: a client
/// that stops within it is dropped after 30 s. So is an idle kept-alive connection, without a
/// stray answer (a proxy would take that for the response to its next request).
#[test]
fn stalled_small_body_and_idle_keep_alive_are_dropped() {
    let s = start("");
    let mut idle = TcpStream::connect(&s.addr).unwrap();
    for _ in 0..2 {
        idle.write_all(b"GET /api/policy HTTP/1.1\r\nHost: t\r\n\r\n").unwrap();
        assert!(head_of(&mut idle, Duration::from_secs(5)).unwrap_or_default().contains("HTTP/1.1 404"));
        std::thread::sleep(Duration::from_secs(1));
    }

    let mut c = TcpStream::connect(&s.addr).unwrap();
    c.write_all(b"POST /api/heartbeat HTTP/1.1\r\nHost: t\r\nContent-Length: 1000\r\n\r\n").unwrap();
    c.write_all(&[b' '; 999]).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    let t0 = Instant::now();
    let got = c.read(&mut [0u8; 64]);
    let took = t0.elapsed().as_secs();
    assert!(!matches!(got, Ok(1..)) && (25..45).contains(&took), "{got:?} after {took} s");

    idle.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut rest = Vec::new();
    let end = idle.read_to_end(&mut rest);
    assert!(end.is_ok() && !rest.windows(5).any(|w| w == b"HTTP/"), "{end:?} {:?}", String::from_utf8_lossy(&rest));
}

/// The signature's timestamp must be current when a release upload begins, not when its body
/// has arrived (a 200 MiB upload on a slow line takes longer than the allowed clock skew).
#[test]
fn slow_upload_is_checked_against_its_start() {
    let (s, k) = admin_server();
    let p = "/api/admin/release/stable/technician/windows-x86_64/slow.bin";
    // More than tiny_http reads before tether-api sees the request (1024 bytes).
    let body = [b'x'; 2000];
    let sig = reqauth::sign_request(&k, "PUT", p, &body, now() - reqauth::MAX_SKEW_SECS + 2);
    let mut c = partial_upload(&s, "PUT", p, &[(reqauth::HEADER, &sig)], 2000);
    std::thread::sleep(Duration::from_secs(4));
    c.write_all(&body[1..]).unwrap();
    let got = head_of(&mut c, Duration::from_secs(5)).unwrap_or_default();
    assert!(got.starts_with("HTTP/1.1 204"), "{got}");
}

/// A release upload gets its slot on a signature header nobody has verified yet: such uploads
/// must not take the slots of diagnostics uploads, which need an open ticket.
#[test]
fn unverified_release_uploads_do_not_block_diagnostics() {
    let (s, k) = admin_server();
    let ticket = s.admin(&k, "POST", "/api/admin/diag-ticket", br#"{"note":""}"#).json()["ticket"].as_str().unwrap().to_owned();
    let p = "/api/admin/release/stable/technician/windows-x86_64/big.exe";
    let fake = format!("v1 ts={} nonce=n sig=s", now());
    let held: Vec<TcpStream> = (0..4).map(|_| partial_upload(&s, "PUT", p, &[(reqauth::HEADER, &fake)], 100_000_000)).collect();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(s.req("POST", &format!("/api/diag/{ticket}"), &[], b"PK\x03\x04 fake zip").status, 204);
    drop(held);
}

/// One IPv6 client has a whole /64: the per-IP limits count the /64, not each address.
#[test]
fn ipv6_clients_are_limited_per_64() {
    let s = start("");
    let from = |ip: String, uuid: String| s.req("POST", "/api/heartbeat", &[("X-Forwarded-For", &ip)], hb(&uuid).as_bytes()).status;
    for i in 1..=30 {
        assert_eq!(from(format!("2001:db8:1:2::{i:x}"), format!("v6-dev-{i:04}")), 200, "heartbeat {i}");
    }
    assert_eq!(from("2001:db8:1:2:ffff::1".into(), "v6-dev-0031".into()), 429);
    assert_eq!(from("2001:db8:1:3::1".into(), "v6-dev-0032".into()), 200);
    // An IPv4 client in the mapped form is that IPv4 address, not one bucket for all of them.
    for i in 1..=30 {
        assert_eq!(from("::ffff:198.51.100.1".into(), format!("v4-dev-{i:04}")), 200, "mapped heartbeat {i}");
    }
    assert_eq!(from("198.51.100.1".into(), "v4-dev-0031".into()), 429);
    assert_eq!(from("::ffff:198.51.100.2".into(), "v4-dev-0032".into()), 200);
}

/// A release upload's nonce is remembered for as long as a replay that began in time could
/// still be sending its body (about 54 min for 200 MiB), not just the 601 s of other requests;
/// a refused replay does not shorten that.
#[test]
fn release_upload_nonce_outlives_a_slow_replay() {
    let (s, k) = admin_server();
    let p = "/api/admin/release/stable/technician/windows-x86_64/a.bin";
    let sig = reqauth::sign_request(&k, "PUT", p, b"bytes", now());
    let put = || s.req("PUT", p, &[(reqauth::HEADER, &sig)], b"bytes").status;
    assert_eq!(put(), 204);
    assert_eq!(s.admin(&k, "GET", "/api/admin/status", b"").status, 200);
    assert_eq!(put(), 401);
    let mut seen: Vec<u64> = stored_nonces(&s.dir).iter().map(|(_, t)| t.saturating_sub(now())).collect();
    seen.sort_unstable();
    // The status request's nonce: now. The upload's: the body deadline (3260 s) and the skew later.
    assert!(seen.len() == 2 && seen[0] < 5 && (3500..3600).contains(&seen[1]), "{seen:?}");
}

/// A session history read that would exceed the byte budget is cut and says so.
#[test]
fn audit_read_is_cut_with_a_header() {
    let k = generate_key();
    let s = start_env(&key_to_b64(&k.verifying_key()), &[], |d| {
        std::fs::create_dir_all(d.join("audit")).unwrap();
        let line = serde_json::json!({"ts": now(), "kind": "file", "id": "1", "uuid": "u", "path": "x".repeat(7000)}).to_string() + "\n";
        std::fs::write(d.join("audit").join(format!("{}.jsonl", utc_day(0))), line.repeat(1500)).unwrap();
    });
    let r = s.admin(&k, "GET", "/api/admin/audit?limit=5000", b"");
    assert_eq!((r.status, r.header("X-Audit-Truncated")), (200, Some("1")));
    let n = r.json().as_array().unwrap().len();
    assert!((1000..1500).contains(&n), "{n}");
    let r = s.admin(&k, "GET", "/api/admin/audit?limit=100", b"");
    assert_eq!((r.json().as_array().unwrap().len(), r.header("X-Audit-Truncated")), (100, None));
}

/// A nonce list that cannot be saved (disk full) must not fail every admin request, the deletes
/// that would free space included; replays are still refused from memory.
#[test]
fn unsaved_nonces_do_not_stop_admin_requests() {
    let k = generate_key();
    let s = start_env(&key_to_b64(&k.verifying_key()), &[], |d| ["nonces.json", "nonces.log"].iter().for_each(|f| std::fs::create_dir(d.join(f)).unwrap()));
    let sig = reqauth::sign_request(&k, "GET", "/api/admin/status", b"", now());
    assert_eq!(s.req("GET", "/api/admin/status", &[(reqauth::HEADER, &sig)], b"").status, 200);
    assert_eq!(s.req("GET", "/api/admin/status", &[(reqauth::HEADER, &sig)], b"").status, 401);
}

/// Connections that arrive in a burst are all served: tiny_http queued a new connection behind a
/// thread it had already woken for an earlier one, where it waited for some connection to end.
#[test]
fn burst_of_connections_is_served() {
    let s = start("");
    let mut conns: Vec<TcpStream> = (0..40).map(|_| TcpStream::connect(&s.addr).unwrap()).collect();
    for (i, c) in conns.iter_mut().enumerate() {
        c.write_all(b"GET /api/policy HTTP/1.1\r\nHost: t\r\n\r\n").unwrap();
        let got = head_of(c, Duration::from_secs(3)).unwrap_or_default();
        assert!(got.starts_with("HTTP/1.1 404"), "connection {i}: {got}");
    }
}

/// The team audit log as an admin reads it.
fn team_audit_records(s: &Srv, k: &SigningKey) -> Vec<serde_json::Value> {
    let r = s.admin(k, "GET", "/api/admin/team-audit?limit=5000", b"");
    assert_eq!(r.status, 200);
    r.json()["records"].as_array().unwrap().clone()
}

#[test]
fn every_admin_change_is_in_the_team_audit_log() {
    let (s, k) = admin_server();
    let kid = tether_trust::team::key_id(&k.verifying_key());
    let rel = "/api/admin/release/stable/technician/windows-x86_64";
    let tok = custom::new_token();
    let member = generate_key();
    let roster = serde_json::json!({"members": [{"name": "M", "key": key_to_b64(&member.verifying_key()), "write": false, "audit": true}]}).to_string();
    let team = serde_json::json!({"team": envelope::sign(&serde_json::to_vec(&serde_json::json!({"serial": 3, "members": []})).unwrap(), &k)}).to_string();
    // (method, path, body, event, detail, status)
    let steps: Vec<(&str, String, Vec<u8>, &str, serde_json::Value, u16)> = vec![
        ("PUT", "/api/admin/policy".into(), policy_doc(&k, r#"{"serial":7,"issued_at":1}"#), "policy-publish", serde_json::json!({"serial": 7}), 204),
        ("PUT", "/api/admin/policy".into(), policy_doc(&k, r#"{"serial":6,"issued_at":1}"#), "policy-publish", serde_json::json!({"serial": 6}), 409),
        ("PUT", "/api/admin/server-keys".into(), keys_doc(&k, 2, &[&key_to_b64(&generate_key().verifying_key())]), "server-keys-publish", serde_json::json!({"serial": 2}), 204),
        ("PUT", "/api/admin/team".into(), roster.into_bytes(), "team-publish", serde_json::json!({"plain_roster": true, "members": 1}), 204),
        ("PUT", "/api/admin/team".into(), team.into_bytes(), "team-publish", serde_json::json!({"serial": 3}), 204),
        ("PUT", "/api/admin/download-page".into(), b"<p>x</p>".to_vec(), "download-page-publish", serde_json::json!({"bytes": 8}), 204),
        ("POST", "/api/admin/diag-ticket".into(), br#"{"note":"n","ticket":"K7Q2M9XD"}"#.to_vec(), "diag-ticket", serde_json::json!({"ticket": "K7Q2M9XD"}), 200),
        ("POST", "/api/admin/diag/K7Q2M9XD/close".into(), Vec::new(), "diag-close", serde_json::json!({"ticket": "K7Q2M9XD"}), 204),
        ("DELETE", "/api/admin/diag/K7Q2M9XD".into(), Vec::new(), "diag-delete", serde_json::json!({"ticket": "K7Q2M9XD"}), 204),
        ("PUT", "/api/admin/custom/acme-1".into(), custom_body(&k, "acme-1", "ACME", &tok, 2_000_000_000), "custom-app-publish", serde_json::json!({"id": "acme-1", "channel": "stable", "expires": 2_000_000_000u64}), 400),
        ("DELETE", "/api/admin/custom/acme-1".into(), Vec::new(), "custom-app-delete", serde_json::json!({"id": "acme-1"}), 404),
        ("PUT", format!("{rel}/Tether_0.5.24_x64-setup.exe"), b"exe".to_vec(), "release-upload", serde_json::json!({"path": "stable/technician/windows-x86_64/Tether_0.5.24_x64-setup.exe"}), 204),
        ("PUT", "/api/admin/release/stable/releases.json".into(), index_doc(&k, "stable", 5), "release-index-publish", serde_json::json!({"channel": "stable", "file": "releases.json", "serial": 5, "expires": 2_000_000_000u64}), 204),
        ("DELETE", format!("{rel}/Tether_0.5.24_x64-setup.exe"), Vec::new(), "release-delete", serde_json::json!({"path": "stable/technician/windows-x86_64/Tether_0.5.24_x64-setup.exe"}), 409),
        ("POST", "/api/admin/releases/prune".into(), br#"{"dry_run":true}"#.to_vec(), "release-prune", serde_json::json!({"dry_run": true}), 200),
    ];
    for (m, p, b, _, _, code) in &steps {
        assert_eq!(s.admin(&k, m, p, b).status, *code, "{m} {p}");
    }
    // Refused before the signature: nobody to name, nothing recorded.
    assert_eq!(s.req("PUT", "/api/admin/policy", &[], &policy_doc(&k, r#"{"serial":9,"issued_at":1}"#)).status, 401);
    let recs = team_audit_records(&s, &k);
    let got: Vec<(&str, serde_json::Value, u64)> = recs.iter().map(|r| (r["event"].as_str().unwrap(), r["detail"].clone(), r["result"].as_u64().unwrap())).collect();
    let want: Vec<(&str, serde_json::Value, u64)> = steps.iter().map(|(_, _, _, e, d, c)| (*e, d.clone(), *c as u64)).collect();
    assert_eq!(got, want);
    assert!(recs.iter().all(|r| r["actor"] == serde_json::json!({"key_fingerprint": kid, "name": "admin"})));
    // Never a document's contents: the page, the roster's names and keys, the custom link.
    let log: String = std::fs::read_dir(s.dir.join("team-audit")).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".jsonl")).map(|e| std::fs::read_to_string(e.path()).unwrap()).collect();
    for secret in ["<p>", &key_to_b64(&member.verifying_key()), &tok, "\"note\""] {
        assert!(!log.contains(secret), "{secret} in the log");
    }
}

/// The nonces on disk: `nonces.json` (the last full write) and the lines appended to `nonces.log` since.
fn stored_nonces(dir: &std::path::Path) -> Vec<(String, u64)> {
    let mut v: Vec<(String, u64)> = std::fs::read(dir.join("nonces.json")).ok().map(|b| serde_json::from_slice::<std::collections::HashMap<String, u64>>(&b).unwrap().into_iter().collect()).unwrap_or_default();
    v.extend(std::fs::read_to_string(dir.join("nonces.log")).unwrap_or_default().lines().map(|l| serde_json::from_str::<(String, u64)>(l).unwrap()));
    v
}

/// A signed request appends its nonce to `nonces.log` (synced before the answer) instead of
/// rewriting the whole nonce map in `nonces.json`; a restart reads both.
#[test]
fn nonces_are_appended_not_rewritten() {
    let (s, k) = admin_server();
    let sigs: Vec<String> = (0..3).map(|_| reqauth::sign_request(&k, "GET", "/api/admin/status", b"", now())).collect();
    for sig in &sigs {
        assert_eq!(s.req("GET", "/api/admin/status", &[(reqauth::HEADER, sig)], b"").status, 200);
    }
    assert!(!s.dir.join("nonces.json").exists(), "the whole map is not rewritten per request");
    assert_eq!(stored_nonces(&s.dir).len(), 3);
    for sig in &sigs {
        assert_eq!(s.req("GET", "/api/admin/status", &[(reqauth::HEADER, sig)], b"").status, 401, "replay");
    }
}

/// Past `NONCE_LOG_MAX` lines the whole map goes to `nonces.json` and the log starts again; a
/// restart reads both.
#[test]
fn the_nonce_log_is_compacted_into_nonces_json() {
    let k = generate_key();
    let keys = key_to_b64(&k.verifying_key());
    let s = start_env(&keys, &[("TETHER_TEST_NONCE_LOG_MAX", "2")], |_| {});
    let sigs: Vec<String> = (0..4).map(|_| reqauth::sign_request(&k, "GET", "/api/admin/status", b"", now())).collect();
    let get = |s: &Srv, sig: &String| s.req("GET", "/api/admin/status", &[(reqauth::HEADER, sig)], b"").status;
    for sig in &sigs[..3] {
        assert_eq!(get(&s, sig), 200);
    }
    assert!(!s.dir.join("nonces.log").exists());
    assert_eq!(stored_nonces(&s.dir).len(), 3, "all three in nonces.json");
    assert_eq!(get(&s, &sigs[3]), 200);
    assert_eq!(std::fs::read_to_string(s.dir.join("nonces.log")).unwrap().lines().count(), 1);
    // Another relay on a copy of both files (as after a restart).
    let s2 = start_env(&keys, &[], |d| {
        for f in ["nonces.json", "nonces.log"] {
            std::fs::copy(s.dir.join(f), d.join(f)).unwrap();
        }
    });
    for sig in &sigs {
        assert_eq!(get(&s2, sig), 401, "replay after a restart");
    }
}

/// The nonce map has a hard bound: when it is full of nonces still inside the replay window, a
/// new signed request is refused `503` with `Retry-After` rather than the map growing.
#[test]
fn the_nonce_map_is_bounded() {
    let k = generate_key();
    let s = start_env(&key_to_b64(&k.verifying_key()), &[("TETHER_TEST_NONCES_MAX", "5")], |_| {});
    for i in 0..5 {
        assert_eq!(s.admin(&k, "GET", "/api/admin/status", b"").status, 200, "request {i}");
    }
    let r = s.admin(&k, "GET", "/api/admin/status", b"");
    assert_eq!((r.status, r.header("Retry-After")), (503, Some("60")));
    assert_eq!(stored_nonces(&s.dir).len(), 5);
}

/// `GET /api/admin/devices` (heartbeats with addresses) is recorded, folded per actor and hour.
#[test]
fn the_admin_device_list_is_recorded() {
    let (s, k) = admin_server();
    for _ in 0..2 {
        assert_eq!(s.admin(&k, "GET", "/api/admin/devices", b"").status, 200);
    }
    let recs: Vec<serde_json::Value> = team_audit_records(&s, &k).into_iter().filter(|r| r["event"] == "device-list").collect();
    assert_eq!(recs.len(), 1, "{recs:?}");
    assert_eq!(recs[0]["actor"]["name"], "admin");
}

/// A release proposal's document is at most 64 KiB; the list keeps each document as stored and
/// names a file that cannot be read by its id (it counts toward the limit), which DELETE removes.
#[test]
fn proposals_list_raw_documents_and_unreadable_files() {
    let (s, k) = admin_server();
    let put = |doc: serde_json::Value| s.admin(&k, "PUT", "/api/admin/proposals/release", serde_json::json!({"note": "n", "doc": doc}).to_string().as_bytes());
    assert_eq!(put(serde_json::json!({"files": "x".repeat(70_000)})).status, 400);
    let r = put(serde_json::json!({"serial": 3, "files": [{"name": "a.exe", "sha256": "ab"}], "z": 1.50}));
    assert_eq!(r.status, 200);
    let id = r.json()["id"].as_str().unwrap().to_owned();
    std::fs::write(s.dir.join("proposals/release/0123456789abcdef.json"), b"{not json").unwrap();
    let list = s.admin(&k, "GET", "/api/admin/proposals/release", b"").json();
    let p = list["proposals"].as_array().unwrap();
    assert_eq!(p.len(), 2, "{list}");
    assert_eq!(p[0], serde_json::json!({"id": "0123456789abcdef", "unreadable": true}));
    assert_eq!((p[1]["id"].as_str(), p[1]["doc"]["files"][0]["name"].as_str()), (Some(id.as_str()), Some("a.exe")));
    assert_eq!(s.admin(&k, "DELETE", "/api/admin/proposals/release/0123456789abcdef", b"").status, 204);
    assert_eq!(s.admin(&k, "DELETE", "/api/admin/proposals/release/0123456789abcdef", b"").status, 404);
    assert_eq!(s.admin(&k, "GET", "/api/admin/proposals/release", b"").json()["proposals"].as_array().unwrap().len(), 1);
}

/// One proposal list and one team-audit read at a time relay-wide: another is `429` with `Retry-After`.
#[test]
fn one_proposal_list_and_one_team_audit_read_at_a_time() {
    let k = generate_key();
    let s = start_env(&key_to_b64(&k.verifying_key()), &[("TETHER_TEST_ADMIN_READ_PAUSE", "1500")], |_| {});
    for (path, retry) in [("/api/admin/proposals/policy", "1"), ("/api/admin/team-audit?limit=10", "2")] {
        std::thread::scope(|sc| {
            let first = sc.spawn(|| s.admin(&k, "GET", path, b"").status);
            std::thread::sleep(Duration::from_millis(500));
            let r = s.admin(&k, "GET", path, b"");
            assert_eq!((r.status, r.header("Retry-After")), (429, Some(retry)), "{path}");
            assert_eq!(first.join().unwrap(), 200, "{path}");
        });
        assert_eq!(s.admin(&k, "GET", path, b"").status, 200, "{path} after the first");
    }
}

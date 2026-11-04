//! End-to-end tests: start the real binary on a random port with a temp data dir.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

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
    let dir = std::env::temp_dir().join(format!("tether-api-test-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&dir);
    let mut child = Command::new(env!("CARGO_BIN_EXE_tether-api"))
        .args(["--data", dir.to_str().unwrap(), "--listen", "127.0.0.1:0"])
        .env("ADMIN_PUB_KEY", admin_keys)
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

fn hb(uuid: &str) -> String {
    format!(r#"{{"uuid":"{uuid}","id":"123456789","profile":"technician","version":"0.3.0","os":"windows 10.0.26300","serial":12}}"#)
}

#[test]
fn heartbeat() {
    let (s, k) = admin_server();
    let ip = [("X-Forwarded-For", "203.0.113.7, 10.0.0.1")];
    assert_eq!(s.req("POST", "/api/heartbeat", &ip, hb("aaaa-1111").as_bytes()).status, 204);
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
        assert_eq!(s.req("POST", "/api/heartbeat", &ip3, hb(&format!("dev-{i:04}")).as_bytes()).status, 204, "post {i}");
    }
    assert_eq!(s.req("POST", "/api/heartbeat", &ip3, hb("dev-9999").as_bytes()).status, 429);
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
    assert_eq!(s.req("POST", &format!("/api/diag/{ticket}"), &[("Content-Type", "application/zip")], &zip).status, 204);
    assert_eq!(s.req("POST", &format!("/api/diag/{ticket}"), &[], &zip).status, 404);

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

    assert_eq!(s.admin(&k, "DELETE", p, b"").status, 204);
    assert!(!f.exists());
    assert_eq!(s.admin(&k, "DELETE", p, b"").status, 404);

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

    let list = s.admin(&k, "GET", "/api/admin/custom", b"").json();
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["id"], "acme-1");
    assert_eq!(list[0]["token"], tok.as_str());
    assert_eq!(list[0]["expires"], exp);
    assert_eq!(list[0]["channel"], "stable");
    assert_eq!(list[0]["downloads"], 1);
    assert!(list[0]["last_download"].as_u64().is_some());
    assert!(list[0].get("profile").is_none());

    // Unknown / malformed tokens: 404.
    assert_eq!(s.req("GET", &format!("/q/{}", custom::new_token()), &[], b"").status, 404);
    assert_eq!(s.req("GET", "/q/..%2f..%2fpolicy.json", &[], b"").status, 404);
    assert_eq!(s.req("GET", &format!("/q/{}", &tok[..42]), &[], b"").status, 404);

    // Replacing with a new token: the old link stops working, counters restart.
    let tok2 = custom::new_token();
    assert_eq!(s.admin(&k, "PUT", path, &custom_body(&k, "acme-1", "ACME", &tok2, exp)).status, 204);
    assert_eq!(s.req("GET", &q, &[], b"").status, 404);
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
    assert_eq!(s.admin(&k, "GET", "/api/admin/custom", b"").json().as_array().unwrap().len(), 1);

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
    let sig = reqauth::sign_request(&k, "GET", "/api/admin/status", b"", now());
    assert_eq!(s.req("GET", "/api/admin/status", &[(reqauth::HEADER, &sig)], b"").status, 200);
    assert_eq!(s.req("POST", "/api/heartbeat", &[], hb("persist-01").as_bytes()).status, 204);

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
}

//! Release clean-up end to end (docs/release-cleanup.md): `POST /api/admin/releases/prune` (dry
//! run, execute, idempotent, a publish between plan and execute) and the `DELETE` guard. Same
//! harness as tests/api.rs.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tether_trust::{envelope, generate_key, key_to_b64, reqauth, SigningKey};

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

fn start(admin: &SigningKey) -> Srv {
    let dir = std::env::temp_dir().join(format!("tether-api-prune-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tether-api"))
        .args(["--data", dir.to_str().unwrap(), "--listen", "127.0.0.1:0"])
        .env("ADMIN_PUB_KEY", key_to_b64(&admin.verifying_key()))
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let addr = line.trim().strip_prefix("listening on ").expect("listen line").to_owned();
    Srv { child, addr, dir }
}

impl Srv {
    fn req(&self, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> (u16, Vec<u8>) {
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
        let status = String::from_utf8_lossy(&raw[..split]).split_whitespace().nth(1).unwrap().parse().unwrap();
        (status, raw[split + 4..].to_vec())
    }

    fn admin(&self, k: &SigningKey, method: &str, path: &str, body: &[u8]) -> (u16, Vec<u8>) {
        let sig = reqauth::sign_request(k, method, path, body, now());
        self.req(method, path, &[(reqauth::HEADER, &sig)], body)
    }

    fn put(&self, k: &SigningKey, path: &str, body: &[u8]) {
        let (st, b) = self.admin(k, "PUT", &format!("/api/admin/release/{path}"), body);
        assert_eq!(st, 204, "{path}: {}", String::from_utf8_lossy(&b));
    }

    fn prune(&self, k: &SigningKey, body: Value) -> Value {
        let (st, b) = self.admin(k, "POST", "/api/admin/releases/prune", body.to_string().as_bytes());
        assert_eq!(st, 200, "{}", String::from_utf8_lossy(&b));
        serde_json::from_slice(&b).unwrap()
    }

    fn exists(&self, rel: &str) -> bool {
        self.dir.join("download").join(rel).exists()
    }

    /// Every release file two days old, so the 24 h grace (K7) does not apply.
    fn age(&self) {
        let old = SystemTime::now() - Duration::from_secs(2 * 86_400);
        let mut stack = vec![self.dir.join("download")];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                if e.file_type().unwrap().is_dir() {
                    stack.push(e.path());
                } else {
                    std::fs::File::options().write(true).open(e.path()).unwrap().set_modified(old).unwrap();
                }
            }
        }
    }
}

const QS: &str = "stable/customer/windows-x86_64";

fn qs_files(v: &str) -> [String; 3] {
    [format!("TetherQS-{v}.exe"), format!("TetherQS-{v}.exe.sig"), format!("TetherQS-{v}.exe.gz")]
}

/// A signed index naming `version` for customer/windows-x86_64.
fn index(k: &SigningKey, serial: u64, version: &str, halted: bool) -> Vec<u8> {
    let entry = json!({
        "version": version, "file": format!("TetherQS-{version}.exe"), "size": 3, "sha256": "a".repeat(64), "signature": "U0lH",
        "urls": [format!("https://{{relay}}/download/{QS}/TetherQS-{version}.exe")], "halted": halted,
        "rollout": {"start": 0, "steps": [[0, 1], [3600, 100]]}
    });
    let payload = json!({"type": "tether-releases/1", "channel": "stable", "serial": serial, "issued": 1, "expires": 2_000_000_000u64,
        "releases": {"customer/windows-x86_64": entry}});
    serde_json::to_vec(&envelope::sign(&serde_json::to_vec(&payload).unwrap(), k)).unwrap()
}

fn versions(list: &Value) -> Vec<String> {
    list.as_array().unwrap().iter().map(|v| v["version"].as_str().unwrap().to_owned()).collect()
}

fn group<'a>(plan: &'a Value, dir: &str) -> &'a Value {
    plan["groups"].as_array().unwrap().iter().find(|g| g["dir"] == dir).unwrap_or_else(|| panic!("no group {dir}: {plan}"))
}

fn del(vs: &[&str]) -> Vec<Value> {
    vs.iter().map(|v| json!({"channel": "stable", "dir": "customer/windows-x86_64", "version": v})).collect()
}

#[test]
fn prune_plan_execute_idempotent_and_race() {
    let k = generate_key();
    let s = start(&k);
    for v in ["0.5.1", "0.5.2", "0.5.3", "0.5.4", "0.5.5", "0.5.6"] {
        for f in qs_files(v) {
            s.put(&k, &format!("{QS}/{f}"), v.as_bytes());
        }
    }
    s.put(&k, &format!("{QS}/notes.txt"), b"not a release");
    s.put(&k, &format!("{QS}/latest.json"), br#"{"version":"0.5.4"}"#);
    s.put(&k, "stable/releases.json", &index(&k, 1, "0.5.5", false));
    // The download page still links 0.5.3 (not re-rendered after a publish).
    let (st, _) = s.admin(&k, "PUT", "/api/admin/download-page", format!("<a href=\"/download/{QS}/TetherQS-0.5.3.exe\">").as_bytes());
    assert_eq!(st, 204);
    // Another app on its own: only latest.json publishes it.
    for v in ["1.0.0", "1.1.0"] {
        s.put(&k, &format!("stable/technician/windows-x86_64/Tether_{v}_x64-setup.exe"), b"x");
    }
    s.put(&k, "stable/technician/windows-x86_64/latest.json", br#"{"version":"1.1.0"}"#);

    // Everything is new: the 24 h grace keeps it all.
    let plan = s.prune(&k, json!({"dry_run": true, "keep": 0}));
    assert_eq!(plan["drop_files"], 0, "{plan}");
    assert!(group(&plan, "customer/windows-x86_64")["keep"].as_array().unwrap().iter().all(|v| v["why"].as_array().unwrap().iter().any(|w| w == "recent")));
    s.age();

    // Default keep (2): published 0.5.5, newer 0.5.6, previous 0.5.4 (also latest.json) and 0.5.3.
    let plan = s.prune(&k, json!({"dry_run": true}));
    assert_eq!(plan["keep_older"], 2);
    let g = group(&plan, "customer/windows-x86_64");
    assert_eq!((g["published"].as_str(), g["index_serial"].as_u64()), (Some("0.5.5"), Some(1)));
    assert_eq!(versions(&g["keep"]), ["0.5.6", "0.5.5", "0.5.4", "0.5.3"]);
    assert_eq!(g["keep"][0]["why"], json!(["newer"]));
    assert_eq!(g["keep"][1]["why"], json!(["index"]));
    assert_eq!(g["keep"][2]["why"], json!(["latest.json", "previous"]));
    assert_eq!(versions(&g["drop"]), ["0.5.2", "0.5.1"]);
    assert_eq!(g["drop"][0]["files"].as_array().unwrap().len(), 3);
    assert_eq!((plan["drop_files"].as_u64(), plan["drop_bytes"].as_u64()), (Some(6), Some(30)));
    // keep 0: the page link and a pin still protect 0.5.3 and 0.5.2.
    let plan = s.prune(&k, json!({"dry_run": true, "keep": 0, "pin": del(&["0.5.2"])}));
    let g = group(&plan, "customer/windows-x86_64");
    assert_eq!(versions(&g["drop"]), ["0.5.1"]);
    assert_eq!(g["keep"].as_array().unwrap().iter().find(|v| v["version"] == "0.5.3").unwrap()["why"], json!(["download page"]));
    assert_eq!(g["keep"].as_array().unwrap().iter().find(|v| v["version"] == "0.5.2").unwrap()["why"], json!(["pinned"]));
    assert_eq!(versions(&group(&plan, "technician/windows-x86_64")["drop"]), ["1.0.0"]);

    // Refused: unsigned, replayed, keep out of range, execute without a list, unknown fields.
    let body = json!({"dry_run": true}).to_string();
    assert_eq!(s.req("POST", "/api/admin/releases/prune", &[], body.as_bytes()).0, 401);
    let sig = reqauth::sign_request(&k, "POST", "/api/admin/releases/prune", body.as_bytes(), now());
    assert_eq!(s.req("POST", "/api/admin/releases/prune", &[(reqauth::HEADER, &sig)], body.as_bytes()).0, 200);
    assert_eq!(s.req("POST", "/api/admin/releases/prune", &[(reqauth::HEADER, &sig)], body.as_bytes()).0, 401, "replayed");
    for bad in [json!({"dry_run": true, "keep": 21}), json!({"dry_run": false}), json!({"dry_run": true, "all": true}), json!({"keep": 1})] {
        assert_eq!(s.admin(&k, "POST", "/api/admin/releases/prune", bad.to_string().as_bytes()).0, 400, "{bad}");
    }
    assert_eq!(s.admin(&k, "POST", "/api/admin/releases/prune", &vec![b' '; (64 << 10) + 1]).0, 413);
    assert_eq!(s.admin(&k, "GET", "/api/admin/releases/prune", b"").0, 404);

    // Execute: the two dropped versions go; the published one named in the list stays.
    let r = s.prune(&k, json!({"dry_run": false, "delete": del(&["0.5.2", "0.5.1", "0.5.5"])}));
    assert_eq!(versions(&r["deleted"]), ["0.5.2", "0.5.1"]);
    assert_eq!((versions(&r["kept"]), r["kept"][0]["why"].clone()), (vec!["0.5.5".to_owned()], json!(["index"])));
    assert_eq!(r["freed_bytes"], 30);
    for f in qs_files("0.5.1").iter().chain(&qs_files("0.5.2")) {
        assert!(!s.exists(&format!("{QS}/{f}")), "{f}");
    }
    for f in ["notes.txt", "latest.json", "TetherQS-0.5.3.exe.gz", "TetherQS-0.5.5.exe"] {
        assert!(s.exists(&format!("{QS}/{f}")), "{f}");
    }
    assert!(s.exists("stable/releases.json") && s.dir.join("download/index.html").exists());
    // Again: idempotent.
    let r = s.prune(&k, json!({"dry_run": false, "delete": del(&["0.5.2", "0.5.1"])}));
    assert_eq!((versions(&r["gone"]), r["deleted"].as_array().unwrap().len(), r["freed_bytes"].as_u64()), (vec!["0.5.2".to_owned(), "0.5.1".to_owned()], 0, Some(0)));

    // Race: 0.5.3 is in the keep-0 plan (the page no longer links it), then a publish puts it in
    // the index (halted); the execute recomputes and keeps it.
    assert_eq!(s.admin(&k, "PUT", "/api/admin/download-page", b"<p>none</p>").0, 204);
    let plan = s.prune(&k, json!({"dry_run": true, "keep": 0}));
    assert!(versions(&group(&plan, "customer/windows-x86_64")["drop"]).contains(&"0.5.3".to_owned()));
    s.put(&k, "stable/releases.json", &index(&k, 2, "0.5.3", true));
    let r = s.prune(&k, json!({"dry_run": false, "keep": 0, "delete": del(&["0.5.3"])}));
    assert_eq!(r["kept"][0]["why"], json!(["index"]), "{r}");
    assert!(s.exists(&format!("{QS}/TetherQS-0.5.3.exe")));

    // Log, status and metrics.
    let log = std::fs::read_to_string(s.dir.join("release-prune.jsonl")).unwrap();
    assert_eq!(log.lines().count(), 3);
    assert!(log.lines().next().unwrap().contains("\"freed_bytes\":30"));
    let (_, st) = s.admin(&k, "GET", "/api/admin/status", b"");
    let st: Value = serde_json::from_slice(&st).unwrap();
    assert_eq!(st["last_prune"]["bytes"], 0, "{st}");
    let (_, m) = s.req("GET", "/metrics", &[], b"");
    let m = String::from_utf8_lossy(&m);
    for line in [
        "tether_api_release_prune_runs_total 3",
        "tether_api_release_prune_freed_bytes_total 30",
        "tether_api_release_versions{channel=\"stable\",app=\"customer\",platform=\"windows-x86_64\"} 4",
        "tether_api_release_versions{channel=\"stable\",app=\"technician\",platform=\"windows-x86_64\"} 2",
    ] {
        assert!(m.contains(line), "{line}\n{m}");
    }
    assert!(m.contains("tether_api_release_bytes{channel=\"stable\"}") && !m.contains("tether_api_release_prune_last_timestamp 0\n"));
}

#[test]
fn delete_guard_and_force() {
    let k = generate_key();
    let s = start(&k);
    for v in ["0.5.3", "0.5.4", "0.5.5"] {
        for f in qs_files(v) {
            s.put(&k, &format!("{QS}/{f}"), b"x");
        }
    }
    s.put(&k, &format!("{QS}/latest.json"), br#"{"version":"0.5.4"}"#);
    s.put(&k, "stable/releases.json", &index(&k, 1, "0.5.5", false));
    let d = |p: &str| s.admin(&k, "DELETE", &format!("/api/admin/release/{p}"), b"");
    for p in [format!("{QS}/latest.json"), "stable/releases.json".into(), format!("{QS}/TetherQS-0.5.5.exe"), format!("{QS}/TetherQS-0.5.5.exe.gz"), format!("{QS}/TetherQS-0.5.4.exe.sig")] {
        let (st, b) = d(&p);
        assert_eq!(st, 409, "{p}");
        assert!(String::from_utf8_lossy(&b).contains("published release"), "{p}");
        assert!(s.exists(&p));
    }
    // An old version, and anything with force; a forced delete is logged.
    assert_eq!(d(&format!("{QS}/TetherQS-0.5.3.exe")).0, 204);
    assert_eq!(d(&format!("{QS}/TetherQS-0.5.4.exe.sig?force=1")).0, 204);
    assert!(!s.exists(&format!("{QS}/TetherQS-0.5.4.exe.sig")));
    let log = std::fs::read_to_string(s.dir.join("release-prune.jsonl")).unwrap();
    assert!(log.contains("\"forced_delete\":\"stable/customer/windows-x86_64/TetherQS-0.5.4.exe.sig\""), "{log}");
    // A forced request is signed with its query: dropping it is a different request.
    let sig = reqauth::sign_request(&k, "DELETE", &format!("/api/admin/release/{QS}/TetherQS-0.5.5.exe?force=1"), b"", now());
    assert_eq!(s.req("DELETE", &format!("/api/admin/release/{QS}/TetherQS-0.5.5.exe"), &[(reqauth::HEADER, &sig)], b"").0, 401);
    assert!(s.exists(&format!("{QS}/TetherQS-0.5.5.exe")));
}

/// A signed index `file` (monitor.json or a Tether index) naming `version` under `key`.
fn named_index(k: &SigningKey, serial: u64, key: &str, file: &str, version: &str) -> Vec<u8> {
    let entry = json!({
        "version": version, "file": file, "size": 1, "sha256": "a".repeat(64), "signature": "U0lH",
        "urls": [format!("https://{{relay}}/download/stable/{key}/{file}")], "rollout": {"start": 0, "steps": [[0, 100]]}
    });
    let payload = json!({"type": "tether-releases/1", "channel": "stable", "serial": serial, "issued": 1, "expires": 2_000_000_000u64, "releases": {key: entry}});
    serde_json::to_vec(&envelope::sign(&serde_json::to_vec(&payload).unwrap(), k)).unwrap()
}

/// Monitor keys stay out of releases-v2.json; the packs folder is pruned by what monitor.json names.
#[test]
fn monitor_packs_prune_and_v2_refusal() {
    let k = generate_key();
    let s = start(&k);
    let (st, b) = s.admin(&k, "PUT", "/api/admin/release/stable/releases-v2.json", &named_index(&k, 1, "monitor/windows-x86_64", "tether-monitor-0.1.0.exe", "0.1.0"));
    assert_eq!(st, 400);
    assert!(String::from_utf8_lossy(&b).contains("bad release key"));
    let (st, _) = s.admin(&k, "PUT", "/api/admin/release/stable/releases-v2.json", &named_index(&k, 1, "monitor-packs/any", "monitor-packs-2026.11.23.json", "2026.11.23"));
    assert_eq!(st, 400);

    let packs = "stable/monitor-packs/any";
    for v in ["2026.11.20", "2026.11.21", "2026.11.22", "2026.11.23"] {
        s.put(&k, &format!("{packs}/monitor-packs-{v}.json"), b"{}");
        s.put(&k, &format!("{packs}/monitor-packs-{v}.json.sig"), b"S");
    }
    s.put(&k, "stable/monitor.json", &named_index(&k, 1, "monitor-packs/any", "monitor-packs-2026.11.22.json", "2026.11.22"));
    s.age();
    let plan = s.prune(&k, json!({"dry_run": true, "keep": 1}));
    let g = group(&plan, "monitor-packs/any");
    assert_eq!((g["published"].clone(), g["index_serial"].clone()), (json!("2026.11.22"), json!(1)));
    assert_eq!(versions(&g["keep"]), ["2026.11.23", "2026.11.22", "2026.11.21"]);
    assert_eq!(versions(&g["drop"]), ["2026.11.20"]);
    let out = s.prune(&k, json!({"dry_run": false, "keep": 1, "delete": [{"channel": "stable", "dir": "monitor-packs/any", "version": "2026.11.20"}]}));
    let mut files: Vec<String> = out["deleted"][0]["files"].as_array().unwrap().iter().map(|f| f.as_str().unwrap().to_owned()).collect();
    files.sort();
    assert_eq!(files, ["monitor-packs-2026.11.20.json", "monitor-packs-2026.11.20.json.sig"]);
    assert!(!s.exists(&format!("{packs}/monitor-packs-2026.11.20.json")) && s.exists(&format!("{packs}/monitor-packs-2026.11.21.json")));
    let (st, b) = s.admin(&k, "DELETE", &format!("/api/admin/release/{packs}/monitor-packs-2026.11.22.json.sig"), b"");
    assert_eq!(st, 409, "{}", String::from_utf8_lossy(&b));
}

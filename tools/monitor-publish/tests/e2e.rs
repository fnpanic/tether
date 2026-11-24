//! End to end against a local tether-api (ignored by default; needs a Linux tether-api build):
//!   TETHER_API_BIN=…/tether-api cargo test --test e2e -- --ignored --nocapture
//! Throwaway admin and updater keys in a throwaway Tether Manage key file; publishes a `monitor`
//! artifact and a packs file with the tool, then reads `monitor.json` as a device would.

#[allow(dead_code)]
#[path = "../../../apps/manage/src-tauri/src/keys.rs"]
mod keys;

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use tether_trust::release::{self, Candidate, Index, MONITOR_APPS, TETHER_APPS};
use tether_trust::{envelope, reqauth, status};

const PASS: &str = "throwaway e2e passphrase";

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

fn tool(dir: &std::path::Path, args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_monitor-publish")).args(args).env("MANAGE_KEYS", dir.join("keys.json")).env("MANAGE_PASSPHRASE", PASS).output().unwrap();
    (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

#[test]
#[ignore]
fn publish_to_a_local_relay() {
    let bin = std::env::var("TETHER_API_BIN").expect("TETHER_API_BIN: path of a tether-api build");
    let dir = std::env::temp_dir().join(format!("monitor-publish-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("data")).unwrap();
    let k = keys::Keys::generate().unwrap();
    std::fs::write(dir.join("keys.json"), k.seal(PASS).unwrap()).unwrap();

    let spawn = |data: &std::path::Path| {
        std::fs::create_dir_all(data).unwrap();
        let mut relay = Command::new(&bin)
            .args(["--data", data.to_str().unwrap(), "--listen", "127.0.0.1:0"])
            .env("ADMIN_PUB_KEY", k.admin_pub())
            .env("RELAY_HOST", "127.0.0.1")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(relay.stdout.take().unwrap()).read_line(&mut line).unwrap();
        let base = format!("http://{}", line.trim().strip_prefix("listening on ").unwrap());
        (relay, base)
    };
    let (mut relay, base) = spawn(&dir.join("data"));
    // A second relay that misses the first publish (down, or left out).
    let (mut relay_b, base_b) = spawn(&dir.join("data-b"));
    let c = reqwest::blocking::Client::new();

    // A Tether releases.json already there (what installed Tether clients read).
    let tether = serde_json::json!({"type": "tether-releases/1", "channel": "stable", "serial": 4, "issued": 1, "expires": 2_000_000_000u64, "releases": {"technician/windows-x86_64": {
        "version": "0.6.3", "file": "Tether_0.6.3_x64-setup.exe", "size": 3, "sha256": "a".repeat(64), "signature": "U0lH",
        "urls": ["https://{relay}/download/stable/technician/windows-x86_64/Tether_0.6.3_x64-setup.exe"], "rollout": {"start": 0, "steps": [[0, 100]]}}}});
    let tether_doc = serde_json::to_vec(&envelope::sign(&serde_json::to_vec(&tether).unwrap(), &k.admin)).unwrap();
    let p = "/api/admin/release/stable/releases.json";
    let sig = reqauth::sign_request_for(&k.admin, &base, "PUT", p, &tether_doc, now());
    assert_eq!(c.put(format!("{base}{p}")).header(reqauth::HEADER, sig).body(tether_doc.clone()).send().unwrap().status(), 204);

    // The artifacts with their updater signatures, as a build writes them.
    let files = [("monitor", "0.1.0", "tether-monitor-0.1.0.exe", b"MZ dummy agent".to_vec()), ("monitor-packs", "2026.11.23", "monitor-packs-2026.11.23.json", br#"{"packs":{}}"#.to_vec())];
    for (_, v, name, data) in &files {
        std::fs::write(dir.join(name), data).unwrap();
        std::fs::write(dir.join(format!("{name}.sig")), k.sign_update(data, name, v).unwrap()).unwrap();
    }
    for (app, v, name, _) in &files {
        let (ok, out, err) = tool(&dir, &["index", "--channel", "stable", "--app", app, "--version", v, "--file", dir.join(name).to_str().unwrap(), "--rollout", "now", "--relay", &base]);
        print!("{out}{err}");
        assert!(ok, "{out}{err}");
    }
    // No .sig: refused before anything is sent.
    std::fs::write(dir.join("tether-monitor-0.2.0.exe"), b"x").unwrap();
    let (ok, _, err) = tool(&dir, &["index", "--channel", "stable", "--app", "monitor", "--version", "0.2.0", "--file", dir.join("tether-monitor-0.2.0.exe").to_str().unwrap(), "--relay", &base]);
    assert!(!ok && err.contains(".sig"), "{err}");
    print!("{err}");

    // As a device: the index verifies with the monitor apps, every artifact with the updater key.
    let raw = c.get(format!("{base}/download/stable/monitor.json")).send().unwrap().bytes().unwrap();
    let ix = Index::open_for(&raw, &[k.admin.verifying_key()], "stable", MONITOR_APPS).unwrap();
    assert_eq!((ix.serial, ix.releases.keys().cloned().collect::<Vec<_>>()), (2, vec!["monitor-packs/any".to_owned(), "monitor/windows-x86_64".to_owned()]));
    println!("monitor.json: serial {}, releases {:?}", ix.serial, ix.releases.keys().collect::<Vec<_>>());
    for (key, e) in &ix.releases {
        let cand = Candidate::from_entry(e, &base, &ix);
        // Caddy serves `download/` from tether-api's data folder as it is; tether-api itself
        // serves only the index files there.
        let file = cand.urls[0].strip_prefix(&format!("{base}/")).unwrap();
        let data = std::fs::read(dir.join("data").join(file)).unwrap();
        cand.accept(data, false, &k.updater_pub(), |_, _, _| Err("no packed copy".into())).unwrap();
        assert!(release::eligible(e, release::bucket("dev-0001", key.split('/').next().unwrap(), &e.version), now(), ""));
        println!("{key} {}: {} verified (size, sha256, updater signature, file and version)", e.version, cand.urls[0]);
    }

    // A partial publish is repaired by the same command: relay B gets the current index unchanged
    // (rollout start kept), relay A is not written again.
    let both = format!("{base},{base_b}");
    let mon = |v: &str, more: &[&str]| {
        let f = dir.join(format!("tether-monitor-{v}.exe"));
        let mut args = vec!["index", "--channel", "stable", "--app", "monitor", "--version", v, "--file", f.to_str().unwrap(), "--rollout", "now", "--relay", &both];
        args.extend(more);
        tool(&dir, &args)
    };
    // B lacks the packs file the current index names: the agent file goes there, the index not.
    let (ok, out, err) = mon("0.1.0", &[]);
    print!("{out}{err}");
    assert!(!ok && out.contains(&format!("{base_b}: FAILED: index not written")) && out.contains("monitor-packs-2026.11.23.json") && !out.contains(&format!("{base}:")), "{out}{err}");
    // Re-running the packs command repairs B (both files there now); A is not written again.
    let f = dir.join("monitor-packs-2026.11.23.json");
    let (ok, out, err) = tool(&dir, &["index", "--channel", "stable", "--app", "monitor-packs", "--version", "2026.11.23", "--file", f.to_str().unwrap(), "--relay", &both]);
    print!("{out}{err}");
    assert!(ok && out.contains(&format!("{base_b}: published")) && !out.contains(&format!("{base}:")), "{out}{err}");
    let raw_b = c.get(format!("{base_b}/download/stable/monitor.json")).send().unwrap().bytes().unwrap();
    assert_eq!(raw_b, raw, "relay B serves the same signed index (rollout start kept)");
    let (ok, out, _) = mon("0.1.0", &[]);
    print!("{out}");
    assert!(ok && out.contains("every relay already publishes"), "{out}");
    // The same version with other bytes, and a lower version: refused, nothing sent.
    for (v, data, want) in [("0.1.0", &b"MZ rebuilt agent"[..], "other bytes"), ("0.0.9", &b"MZ old agent"[..], "not newer")] {
        let name = format!("tether-monitor-{v}.exe");
        std::fs::write(dir.join(&name), data).unwrap();
        std::fs::write(dir.join(format!("{name}.sig")), k.sign_update(data, &name, v).unwrap()).unwrap();
        let (ok, _, err) = mon(v, &[]);
        print!("{err}");
        assert!(!ok && err.contains(want), "{v}: {err}");
    }
    assert_eq!(c.get(format!("{base}/download/stable/monitor.json")).send().unwrap().bytes().unwrap(), raw);

    // refresh: the same releases signed again, serial + 1 and a new expiry, on every relay.
    let before = Index::open_for(&raw, &[k.admin.verifying_key()], "stable", MONITOR_APPS).unwrap();
    let (ok, out, err) = tool(&dir, &["refresh", "--channel", "stable", "--relay", &both]);
    print!("{out}{err}");
    assert!(ok && out.contains(&format!("{base}: refreshed")) && out.contains(&format!("{base_b}: refreshed")) && out.contains("now expires"), "{out}{err}");
    for b in [&base, &base_b] {
        let raw = c.get(format!("{b}/download/stable/monitor.json")).send().unwrap().bytes().unwrap();
        let after = Index::open_for(&raw, &[k.admin.verifying_key()], "stable", MONITOR_APPS).unwrap();
        assert_eq!((after.serial, &after.releases, after.issued), (before.serial + 1, &before.releases, before.issued));
        assert!(after.expires >= now() + release::EXPIRY_SECS - 60, "a new expiry");
    }
    let floor: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("monitor-serials.json")).unwrap()).unwrap();
    assert_eq!(floor["stable"], before.serial + 1);
    // A relay without the index (lost, or rebuilt): nothing to refresh, and no publish below the floor.
    let (mut relay_c, base_c) = spawn(&dir.join("data-c"));
    let (ok, _, err) = tool(&dir, &["refresh", "--channel", "stable", "--relay", &base_c]);
    assert!(!ok && err.contains("rolled back"), "{err}");
    let (ok, _, err) = tool(&dir, &["refresh", "--channel", "test", "--relay", &base_c]);
    assert!(!ok && err.contains("nothing is published"), "{err}");
    let f = dir.join("monitor-packs-2026.11.23.json");
    let (ok, out, err) = tool(&dir, &["index", "--channel", "stable", "--app", "monitor-packs", "--version", "2026.11.23", "--file", f.to_str().unwrap(), "--relay", &base_c]);
    print!("{out}{err}");
    assert!(!ok && err.contains("rolled back") && !out.contains("published"), "{out}{err}");
    assert_eq!(c.get(format!("{base_c}/download/stable/monitor.json")).send().unwrap().status(), 404);
    let _ = relay_c.kill();
    let _ = relay_c.wait();

    // releases.json readers are unaffected: same document, Tether apps only, G not moved.
    let raw = c.get(format!("{base}/download/stable/releases.json")).send().unwrap().bytes().unwrap();
    assert_eq!(raw.as_ref(), tether_doc.as_slice());
    let t = Index::open_for(&raw, &[k.admin.verifying_key()], "stable", TETHER_APPS).unwrap();
    assert!(t.releases.keys().all(|k| !k.starts_with("monitor")));
    let hb = r#"{"uuid":"hb-000001","id":"123456789","profile":"technician","version":"0.6.3","os":"windows","serial":1}"#;
    let g: serde_json::Value = serde_json::from_slice(&c.post(format!("{base}/api/heartbeat")).body(hb).send().unwrap().bytes().unwrap()).unwrap();
    assert_eq!(g["releases"], 1, "only the releases.json upload moved G");
    println!("releases.json: unchanged, serial {}, keys {:?}, heartbeat releases {}", t.serial, t.releases.keys().collect::<Vec<_>>(), g["releases"]);

    // A token from the tool enrols a device, whose answer carries the monitor serials as hint.
    let (ok, out, err) = tool(&dir, &["token", "--label", "e2e", "--days", "1"]);
    assert!(ok, "{err}");
    let tok = out.trim();
    assert!(status::verify_token(tok, &[k.admin.verifying_key()], now()).is_ok());
    let dev = tether_trust::generate_key();
    let body = serde_json::json!({"uuid": "dev-e2e-0001", "pubkey": tether_trust::key_to_b64(&dev.verifying_key()), "token": tok});
    let r = c.post(format!("{base}/api/status/enroll")).body(body.to_string()).send().unwrap();
    assert_eq!(r.status(), 200);
    let a: serde_json::Value = serde_json::from_slice(&r.bytes().unwrap()).unwrap();
    assert_eq!(a["releases"], 3, "monitor.json serial 3 after the refresh");
    println!("token: enrolment accepted, answer {a}");

    // A token issued for one Tether ID: journaled without the token, enrols one device with that ID.
    let enrol = |uuid: &str, token: &str| {
        let dev = tether_trust::generate_key();
        let body = serde_json::json!({"uuid": uuid, "pubkey": tether_trust::key_to_b64(&dev.verifying_key()), "token": token});
        let r = c.post(format!("{base}/api/status/enroll")).body(body.to_string()).send().unwrap();
        (r.status().as_u16(), r.text().unwrap())
    };
    let (ok, out, err) = tool(&dir, &["token", "--label", "front desk", "--days", "1", "--tether-id", "123456789"]);
    assert!(ok, "{err}");
    let bound = out.trim().to_owned();
    let bound_id = status::verify_token(&bound, &[k.admin.verifying_key()], now()).unwrap().id;
    assert!(err.contains(&bound_id) && err.contains("123456789"), "{err}");
    let journal = std::fs::read_to_string(dir.join("monitor-tokens.json")).unwrap();
    assert!(journal.contains(&bound_id) && !journal.contains(&bound) && !journal.contains(tok), "the journal never holds a token");
    assert_eq!(enrol("dev-e2e-0002", &bound).0, 200);
    assert_eq!(enrol("dev-e2e-0003", &bound), (409, "token already used".to_owned()));
    // devices: all, and those of one token (admin route, table output).
    let (ok, out, err) = tool(&dir, &["devices", "--relay", &base]);
    print!("{out}");
    assert!(ok && out.contains("2 devices") && out.contains("dev-e2e-0001") && out.contains("dev-e2e-0002"), "{out}{err}");
    let (ok, out, err) = tool(&dir, &["devices", "--token", &bound_id, "--relay", &base]);
    assert!(ok && out.contains("1 devices") && out.contains("dev-e2e-0002") && out.contains("123456789") && !out.contains("dev-e2e-0001"), "{out}{err}");
    // revoke: the expiry from the journal, every relay; the device stays enrolled.
    let first_id = status::verify_token(tok, &[k.admin.verifying_key()], now()).unwrap().id;
    let (ok, out, err) = tool(&dir, &["revoke", "--id", &first_id, "--relay", &both]);
    print!("{out}{err}");
    assert!(ok && out.contains(&format!("{base}: revoked")) && out.contains(&format!("{base_b}: revoked")) && err.contains("stay enrolled"), "{out}{err}");
    assert!(out.contains("revoked on 2 of 2 relays"), "{out}");
    assert_eq!(enrol("dev-e2e-0004", tok), (401, "token revoked".to_owned()));
    let (ok, out, _) = tool(&dir, &["tokens"]);
    print!("{out}");
    let line = |id: &str| out.lines().find(|l| l.starts_with(id)).unwrap_or_default().to_owned();
    assert!(ok && line(&first_id).contains("revoked") && line(&bound_id).contains("valid") && line(&bound_id).contains("123456789"), "{out}");
    let (ok, out, _) = tool(&dir, &["devices", "--token", &first_id, "--relay", &base]);
    assert!(ok && out.contains("dev-e2e-0001"), "revoking does not unpin: {out}");
    // remove: every relay, one line each; non-zero when one fails.
    let (ok, out, err) = tool(&dir, &["remove", "--uuid", "dev-e2e-0002", "--relay", &both]);
    print!("{out}{err}");
    assert!(ok && out.contains(&format!("{base}: removed")) && out.contains(&format!("{base_b}: not enrolled there")), "{out}{err}");
    let (ok, out, _) = tool(&dir, &["devices", "--token", &bound_id, "--relay", &base]);
    assert!(ok && out.contains("0 devices"), "{out}");
    assert_eq!(enrol("dev-e2e-0002", &bound), (409, "token already used".to_owned()), "a used token stays used after the removal");
    // Revoke from the token itself (a file), one relay down: partly revoked, exit 1.
    std::fs::write(dir.join("bound.token"), &bound).unwrap();
    let (ok, out, err) = tool(&dir, &["revoke", "--token-file", dir.join("bound.token").to_str().unwrap(), "--relay", &format!("{base},http://127.0.0.1:9")]);
    print!("{out}{err}");
    assert!(!ok && out.contains(&format!("{base}: revoked")) && out.contains("revoked on 1 of 2 relays"), "{out}{err}");
    assert_eq!(enrol("dev-e2e-0009", &bound), (401, "token revoked".to_owned()));
    let (_, out, _) = tool(&dir, &["tokens"]);
    print!("{out}");
    assert!(out.lines().any(|l| l.starts_with(&bound_id) && l.contains("partly revoked")), "{out}");
    let (ok, out, err) = tool(&dir, &["remove", "--uuid", "dev-e2e-0001", "--relay", &format!("{base},http://127.0.0.1:9")]);
    print!("{out}{err}");
    assert!(!ok && out.contains(&format!("{base}: removed")) && out.contains("http://127.0.0.1:9: FAILED") && err.contains("1 of 2 relays failed"), "{out}{err}");

    for r in [&mut relay, &mut relay_b] {
        let _ = r.kill();
        let _ = r.wait();
    }
    let _ = std::fs::remove_dir_all(&dir);
}

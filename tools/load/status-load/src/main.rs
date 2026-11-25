//! Load generator for tether-api's device status store (driven by `tools/load/status-run.sh`).
//!
//!   status-load keygen <file>                      a throwaway admin key; prints its public key
//!   status-load prep <data-dir> devices=N enroll=E [customers=C sbom=K members=M scoped=S key=<file>]
//!                                                  pins devices E..N (status/keys.json, which
//!                                                  the relay migrates) with a last full report
//!                                                  in seen.json (customer c<i % C>; devices
//!                                                  below K with a 60 KiB software list of 200
//!                                                  programs); with key=, also the signed team
//!                                                  document (serial 1) of `run` as team/team.json,
//!                                                  so it is in force at start (software search)
//!   status-load run addr=<host:port> key=<file> pid=<tether-api pid> devices=N enroll=E
//!       seconds=600 warmup=60 baseline=60 probe_ms=50 workers=16 full_pct=5
//!       new_per_s=2 new_secs=300 summary_s=30 metrics_s=15 sbom_rps=0
//!       members=0 scoped=0 customers=0 sbom=0 plain_pct=10 member_s=300 open_s=300 sw_s=900
//!       member_workers=32 junk=0 search_s=0 doc_serial=1 [--any-address]
//!
//! With `search_s` > 0 the load member searches installed software (`GET
//! /api/team/status/software`, terms in turn) every `search_s` seconds from the start of the
//! warm-up and prints each answer (time, hosts, more, pending, partial); `doc_serial` 2 when `prep`
//! wrote the document with serial 1.
//!
//! For test relays only: `run` refuses an `addr` that is not loopback or private unless
//! `--any-address` is given. With `members` > 0 it publishes a signed team document with that many
//! simulated desktops (see `Members`) and runs them beside the device load; `junk` > 0 sends that
//! many requests at once every second from one address with no-`kid` signatures of unknown keys.
//!
//! `run` publishes a policy, enrols devices 0..E through `POST /api/status/enroll` with an
//! admin-signed token and publishes a roster with one throwaway member. Then it probes
//! `POST /api/heartbeat` and `GET /api/policy` (one request every `probe_ms` each): `warmup`
//! seconds unmeasured, then the RSS reference, then `baseline` seconds measured alone, then
//! `seconds` measured while
//!   - all N devices post at the pace the relay answers (`next`), first posts spread uniformly
//!     over it, `full_pct` % full reports (8 KiB body), the rest heartbeats of about 300 bytes;
//!   - `new_per_s` new devices enrol through the API per second for the first `new_secs`;
//!   - the member fetches `GET /api/team/status` every `summary_s` and `/metrics` is scraped
//!     every `metrics_s`;
//!   - with `sbom_rps` > 0 (the relay needs `STATUS_SBOM=on` and the same `STATUS_SBOM_RPS`): one
//!     thread uploads a 60 KiB software list (`PUT /api/status/sbom`, device-signed) for device 0,
//!     1, 2, … at `sbom_rps` per second, each device once, until the devices or the run are over.
//! Device entries are realistic: 4 products and 3 faults (about 1 KB in the summary), every
//! 20th device 8 products and 10 faults. It prints the numbers and PASS or FAIL (exit 0 or 1).
//!
//! Device i's key is derived from i, so `prep` and `run` agree without sharing a file. Every
//! request is a new loopback connection with `X-Forwarded-For`, as from the relay's proxy.

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tether_trust::{envelope, generate_key, key_to_b64, reqauth, status, SigningKey};

const RELAY: &str = "https://relay.test";

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
}

/// Simulated desktop j's key (the load member: `usize::MAX`), derived from j as the device keys.
fn member_key(j: usize) -> SigningKey {
    let mut seed = *b"tether-load-status-member-key-00";
    seed[..8].copy_from_slice(&(j as u64).to_le_bytes());
    SigningKey::from_bytes(&seed)
}

fn device_key(i: usize) -> SigningKey {
    let mut seed = *b"tether-load-status-device-key-00";
    seed[..8].copy_from_slice(&(i as u64).to_le_bytes());
    SigningKey::from_bytes(&seed)
}

/// Device i's uuid, shaped as the agent's (random-looking, so the summary compresses as a real one).
fn uuid(i: usize) -> String {
    let h = reqauth::body_digest(&(i as u64).to_le_bytes());
    format!("{}-{}-4{}-a{}-{}", &h[..8], &h[8..12], &h[13..16], &h[17..20], &h[20..32])
}

/// Device i's host name.
fn host(i: usize) -> String {
    format!("WS-{}", reqauth::body_digest(&(i as u64).to_be_bytes())[..8].to_uppercase())
}

/// Every second device is linked to a Tether ID (9 digits, unique).
fn tether_id(i: usize) -> Option<String> {
    (i % 2 == 0).then(|| format!("{}", 100_000_000 + (i as u64 * 7919) % 900_000_000))
}

fn ip(net: u8, i: usize) -> String {
    format!("10.{net}.{}.{}", (i >> 8) & 255, i & 255)
}

/// xorshift64*: offsets and the full-report share, no dependency needed.
struct Rng(u64);

impl Rng {
    fn unit(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 11) as f64 / (1u64 << 53) as f64
    }
}

struct Reply {
    status: u16,
    body: String,
    head: String,
    /// Body bytes as received (a gzip body is not text).
    bytes: usize,
    took: Duration,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.head.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case(name).then(|| v.trim())
        })
    }
}

/// One request on a new connection (`Connection: close`), timed from connect to the last byte.
fn req(addr: &str, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> io::Result<Reply> {
    let start = Instant::now();
    let mut s = TcpStream::connect(addr)?;
    s.set_read_timeout(Some(Duration::from_secs(30)))?;
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: relay.test\r\nConnection: close\r\nContent-Length: {}\r\n", body.len());
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes())?;
    s.write_all(body)?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw)?;
    let took = start.elapsed();
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or_else(|| io::Error::other("no response head"))?;
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let status = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).ok_or_else(|| io::Error::other("no status"))?;
    let body = &raw[split + 4..];
    Ok(Reply { status, body: String::from_utf8_lossy(body).into_owned(), head, bytes: body.len(), took })
}

/// `--self-check` on `run`: also print how late this generator itself woke up (its own pauses).
static SELF_CHECK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn args(list: &[String]) -> BTreeMap<String, String> {
    list.iter().filter_map(|a| a.split_once('=')).map(|(k, v)| (k.to_owned(), v.to_owned())).collect()
}

/// A test relay only: `addr` (host:port) must resolve to loopback or private addresses only
/// (10/8, 172.16/12, 192.168/16, 100.64/10, fc00::/7, fe80::/10), unless `--any-address`.
fn test_relay(addr: &str) -> Result<(), String> {
    use std::net::{IpAddr, ToSocketAddrs};
    let ips: Vec<IpAddr> = addr.to_socket_addrs().map_err(|e| format!("{addr}: {e}"))?.map(|a| a.ip()).collect();
    let private = |ip: &IpAddr| match ip {
        IpAddr::V4(v) => v.is_loopback() || v.is_private() || (v.octets()[0] == 100 && v.octets()[1] & 0xc0 == 64),
        IpAddr::V6(v) => v.is_loopback() || (v.segments()[0] & 0xfe00) == 0xfc00 || (v.segments()[0] & 0xffc0) == 0xfe80 || v.to_ipv4_mapped().is_some_and(|m| m.is_loopback() || m.is_private()),
    };
    match ips.iter().find(|ip| !private(ip)) {
        None if !ips.is_empty() => Ok(()),
        None => Err(format!("{addr}: no address")),
        Some(ip) => Err(format!("{addr} is {ip}, not a loopback or private address: this tool is for test relays only (--any-address to run it anyway)")),
    }
}

fn num(a: &BTreeMap<String, String>, k: &str, d: u64) -> u64 {
    a.get(k).map_or(d, |v| v.parse().unwrap_or_else(|_| panic!("{k}: not a number")))
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match argv.first().map(String::as_str) {
        Some("keygen") if argv.len() == 2 => {
            let k = generate_key();
            std::fs::write(&argv[1], k.to_bytes().iter().map(|b| format!("{b:02x}")).collect::<String>()).unwrap();
            println!("{}", key_to_b64(&k.verifying_key()));
        }
        Some("prep") if argv.len() >= 2 => {
            let a = args(&argv[2..]);
            prep(Path::new(&argv[1]), num(&a, "devices", 20_000) as usize, num(&a, "enroll", 100) as usize, num(&a, "customers", 0) as usize, num(&a, "sbom", 0) as usize);
            if let Some(k) = a.get("key") {
                let sim = Members::new(&a, num(&a, "devices", 20_000) as usize);
                let doc = sim.team_doc(&member_key(usize::MAX), &read_key(k), 1);
                std::fs::create_dir_all(Path::new(&argv[1]).join("team")).unwrap();
                std::fs::write(Path::new(&argv[1]).join("team/team.json"), doc).unwrap();
            }
        }
        Some("run") => {
            SELF_CHECK.store(argv.iter().any(|a| a == "--self-check"), Ordering::Relaxed);
            let addr = args(&argv[1..]).get("addr").cloned().unwrap_or_default();
            if let Err(e) = test_relay(&addr).or_else(|e| if argv.iter().any(|a| a == "--any-address") { Ok(()) } else { Err(e) }) {
                eprintln!("status-load: {e}");
                std::process::exit(2);
            }
            std::process::exit(if run(&args(&argv[1..])) { 0 } else { 1 })
        }
        _ => {
            eprintln!("usage: status-load keygen <file> | prep <data-dir> devices=N enroll=E | run addr=… key=… pid=… [--self-check] (see the source)");
            std::process::exit(2);
        }
    }
}

/// Device i's products and faults: 4 and 3, every 20th device 8 and 10.
fn products_faults(i: usize) -> (Value, Value) {
    let (np, nf) = if i % 20 == 0 { (8, 10) } else { (4, 3) };
    let products: Vec<Value> = (0..np)
        .map(|k| json!({"name": format!("Example Practice Management Server {k}"), "version": format!("12.{}.{}.{}", i % 4, 4000 + (i + k) % 900, i % 100), "role": if k == 0 { "server" } else { "client" }}))
        .collect();
    let faults: Vec<Value> =
        (0..nf).map(|k| json!({"id": format!("example-product-{k}/database-backup-age"), "state": if k == 0 { "crit" } else { "warn" }, "since": format!("2026-11-{:02}T{:02}:{:02}:00Z", 1 + (i + k) % 22, i % 24, (i * 7 + k) % 60)})).collect();
    (json!(products), json!(faults))
}

const COUNTS: &str = r#"{"ok":31,"warn":2,"crit":1,"unknown":0,"setup":0}"#;

/// Device i's customer reference with `customers` > 0.
fn customer(i: usize, customers: usize) -> Option<String> {
    (customers > 0).then(|| format!("c{:04}", i % customers))
}

/// Devices `enroll..devices`: pinned (with a customer reference with `customers` > 0), with a last
/// full report seen within the last `next` seconds; devices `enroll..sbom` also with a stored software list.
fn prep(data: &Path, devices: usize, enroll: usize, customers: usize, sbom: usize) {
    let dir = data.join("status");
    std::fs::create_dir_all(&dir).unwrap();
    let t = now();
    let (mut keys, mut seen) = (serde_json::Map::new(), serde_json::Map::new());
    let counts: status::Counts = serde_json::from_str(COUNTS).unwrap();
    for i in enroll..devices {
        let (products, faults) = products_faults(i);
        // Byte for byte the entry the relay makes from this device's report (its field order), so
        // a device's first post in the run does not change the summary.
        let (products, faults): (Vec<status::Product>, Vec<status::Fault>) = (serde_json::from_value(products).unwrap(), serde_json::from_value(faults).unwrap());
        let entry = format!(r#"{{"host":{},"agent":"0.1.0","overall":"crit","counts":{},"products":{},"faults":{}}}"#, json!(host(i)), serde_json::to_string(&counts).unwrap(), serde_json::to_string(&products).unwrap(), serde_json::to_string(&faults).unwrap());
        let mut pin = json!({"key": key_to_b64(&device_key(i).verifying_key()), "tether_id": tether_id(i), "label": "load", "enrolled": t});
        if let Some(c) = customer(i, customers) {
            pin["customer"] = c.into();
        }
        keys.insert(uuid(i), pin);
        // As in a fleet at the relay's pace: `next` as it answers (10 posts/s), the last post at any
        // point of that interval (with next 300 for all, every device not posting within 13 minutes
        // of the start would turn stale during the run).
        let next = (devices as u64).div_ceil(10).clamp(300, 3600);
        let mut s = json!({"last_seen": t - 60 - (i as u64 * 7919) % next, "seq": 1, "next": next, "entry": entry});
        if i < sbom {
            let doc = sbom_doc(i);
            let p = dir.join(format!("{}.sbom.json", uuid(i)));
            std::fs::write(&p, &doc).unwrap();
            // The relay's start-up scan trusts a record whose file has its size and mtime.
            std::fs::File::options().write(true).open(&p).unwrap().set_modified(UNIX_EPOCH + Duration::from_secs(t - 3600)).unwrap();
            s["sbom"] = json!({"at": t - 3600, "size": doc.len(), "sha": &reqauth::body_digest(doc.as_bytes())[..16]});
        }
        seen.insert(uuid(i), s);
    }
    std::fs::write(dir.join("keys.json"), serde_json::to_vec(&keys).unwrap()).unwrap();
    std::fs::write(dir.join("seen.json"), serde_json::to_vec(&seen).unwrap()).unwrap();
}

/// One search every `every_s` seconds by the load member until `stop`, terms in turn (common
/// programs on most hosts, a pool program on a few, a version prefix, one nobody has); prints each
/// answer with the relay's RSS and peak RSS at that moment.
fn search_loop(addr: &str, member: &SigningKey, second: &SigningKey, every_s: u64, stop: &AtomicBool, pid: u64) -> Samples {
    // Eight terms at `every_s` 10: a term comes again after 80 s, past the 60 s cache.
    const TERMS: [(&str, &str); 8] = [("chrome", ""), ("example application 1234", ""), ("firefox", "11"), ("acrobat", ""), ("nobody-has-this", ""), ("7-zip", ""), ("microsoft edge", "1"), ("example vendor 7", "")];
    let (start, mut out, mut n) = (Instant::now(), Vec::new(), 0usize);
    while !stop.load(Ordering::Relaxed) {
        let next = Instant::now() + Duration::from_secs(every_s);
        let (q, v) = TERMS[n % TERMS.len()];
        // Two members in turn: one search every 10 s stays under 6 a minute per member key.
        let member = if n % 2 == 0 { member } else { second };
        let enc = |s: &str| s.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect::<String>();
        let mut p = format!("/api/team/status/software?q={}", enc(q));
        if !v.is_empty() {
            p.push_str(&format!("&v={}", enc(v)));
        }
        let sig = reqauth::sign_request_for_kid(member, RELAY, "GET", &p, b"", now());
        let r = req(addr, "GET", &p, &[(reqauth::HEADER, &sig), ("X-Forwarded-For", "10.250.0.1")], b"");
        let hwm = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default().lines().find_map(|l| l.strip_prefix("VmHWM:")?.split_whitespace().next()?.parse::<u64>().ok()).unwrap_or(0);
        match &r {
            Ok(r) => {
                let a: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
                println!(
                    "  search t={:>4} s q={q:?} v={v:?}: {} in {:.0} ms, {} hosts, more {}, pending {}, partial {}, {} bytes; relay RSS {:.1} MiB, peak {:.1} MiB",
                    start.elapsed().as_secs(), r.status, r.took.as_secs_f64() * 1000., a["hosts"].as_array().map_or(0, Vec::len), a["more"], a["pending"], a["partial"], r.bytes, rss_kib(pid) as f64 / 1024., hwm as f64 / 1024.
                );
            }
            Err(e) => println!("  search t={} s: {e}", start.elapsed().as_secs()),
        }
        out.push(sample(r));
        n += 1;
        while Instant::now() < next && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    out
}

fn rss_kib(pid: u64) -> u64 {
    let s = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
    s.lines().find_map(|l| l.strip_prefix("VmRSS:")?.split_whitespace().next()?.parse().ok()).unwrap_or(0)
}

fn heartbeat(i: usize, seq: u64) -> String {
    format!(
        r#"{{"v":1,"uuid":"{}","host":"{h}","seq":{seq},"time":"2026-11-23T08:20:00Z","next":300,"agent":"0.1.0","packs":{{"system":9}},"overall":"crit","counts":{COUNTS},"faults":{}}}"#,
        uuid(i),
        products_faults(i).1,
        h = host(i)
    )
}

fn full(i: usize, seq: u64) -> String {
    let hb = heartbeat(i, seq);
    format!(
        r#"{},"products":{},"changes":[{{"id":"example-product-0/database-backup-age","from":"warn","to":"crit","time":"2026-11-23T08:19:00Z"}}],"body":{{"checks":[],"pad":"{}"}}}}"#,
        &hb[..hb.len() - 1],
        products_faults(i).0,
        "x".repeat(8192)
    )
}

/// Programs every list has a share of: (name, publisher), the version varies by host.
const COMMON: [(&str, &str); 6] = [("Google Chrome", "Google LLC"), ("Mozilla Firefox", "Mozilla"), ("7-Zip", "Igor Pavlov"), ("Adobe Acrobat Reader", "Adobe Inc."), ("Microsoft Edge", "Microsoft Corporation"), ("Example Practice Suite", "Example Medical Software GmbH")];

/// A CycloneDX-shaped software list of host i of about 60 KiB: 200 programs (the operating system,
/// some common ones on most hosts, the rest from a pool of 3 000), each with a `purl` and
/// properties, as an agent's list carries them, and the hardware identity.
fn sbom_doc(i: usize) -> String {
    let mut comps = vec![r#"{"type":"operating-system","name":"Windows 11 Pro","version":"10.0.26300"}"#.to_owned()];
    let pad = "x".repeat(30);
    let mut k = 0;
    while comps.len() < 200 {
        let (name, publisher, version) = match COMMON.get(k).filter(|_| (i + k) % 5 != 0) {
            Some((n, p)) => (n.to_string(), p.to_string(), format!("{}.0.{}.{}", 100 + (i + k) % 30, i % 7000, k)),
            None if k < COMMON.len() => (format!("Example Tool {}", (i + k) % 3000), format!("Example Vendor {}", k % 40), format!("{}.{}", k % 9, i % 97)),
            None => {
                let p = (i * 7 + k * 13) % 3000;
                (format!("Example Application {p}"), format!("Example Software Vendor {}", p % 40), format!("{}.{}.{}", p % 7, (i + k) % 31, k % 997))
            }
        };
        comps.push(format!(r#"{{"type":"application","name":"{name}","version":"{version}","publisher":"{publisher}","purl":"pkg:generic/{}@{version}","properties":[{{"name":"tether:install.location","value":"C:\\Program Files\\{name}\\{pad}"}}]}}"#, name.replace(' ', "-")));
        k += 1;
    }
    format!(r#"{{"bomFormat":"CycloneDX","specVersion":"1.6","version":1,"metadata":{{"component":{{"type":"device","name":"{}","properties":[{{"name":"tether:hw.serial","value":"SN-{i:08}"}}]}}}},"components":[{}]}}"#, host(i), comps.join(","))
}

fn read_key(path: &str) -> SigningKey {
    let hex = std::fs::read_to_string(path).unwrap();
    let seed: Vec<u8> = (0..32).map(|i| u8::from_str_radix(&hex.trim()[2 * i..2 * i + 2], 16).unwrap()).collect();
    SigningKey::from_bytes(&seed.try_into().unwrap())
}

#[cfg(test)]
mod tests {
    #[test]
    fn sbom_doc_is_a_json_object_of_about_60_kib_with_200_programs() {
        for i in [0, 1, 19_999] {
            let d = super::sbom_doc(i);
            assert!((50 * 1024..=70 * 1024).contains(&d.len()), "{}", d.len());
            let v: serde_json::Value = serde_json::from_str(&d).unwrap();
            assert_eq!(v["components"].as_array().unwrap().len(), 200);
        }
    }

    #[test]
    fn only_test_relays_are_loaded() {
        for ok in ["127.0.0.1:8080", "[::1]:80", "10.1.2.3:443", "172.16.0.1:1", "192.168.1.1:1", "100.64.0.1:1", "[fd00::1]:1", "localhost:1"] {
            assert!(super::test_relay(ok).is_ok(), "{ok}");
        }
        for bad in ["8.8.8.8:443", "172.32.0.1:1", "[2001:db8::1]:1", "1.1.1.1:80"] {
            assert!(super::test_relay(bad).unwrap_err().contains("--any-address"), "{bad}");
        }
        assert!(super::test_relay("no-port").is_err());
    }
}

/// (status, latency in ms) of one route.
type Samples = Vec<(u16, f64)>;

fn sample(r: io::Result<Reply>) -> (u16, f64) {
    match r {
        Ok(r) => (r.status, r.took.as_secs_f64() * 1000.),
        Err(_) => (0, f64::NAN),
    }
}

/// Runs `f` every `every` until `until` (open loop: a slow answer does not delay the schedule
/// beyond itself).
fn every(every: Duration, until: Instant, mut f: impl FnMut(usize) -> (u16, f64)) -> Samples {
    let (mut out, mut at, mut n) = (Vec::new(), Instant::now(), 0);
    while at < until {
        out.push(f(n));
        n += 1;
        at += every;
        if let Some(d) = at.checked_duration_since(Instant::now()) {
            std::thread::sleep(d);
        }
    }
    out
}

fn pct(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    sorted[((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len()) - 1]
}

struct Stats {
    n: usize,
    codes: BTreeMap<u16, usize>,
    p50: f64,
    p99: f64,
    p999: f64,
    max: f64,
}

fn stats(s: &Samples) -> Stats {
    let mut codes = BTreeMap::new();
    for (c, _) in s {
        *codes.entry(*c).or_default() += 1;
    }
    let mut ms: Vec<f64> = s.iter().map(|x| x.1).filter(|x| x.is_finite()).collect();
    ms.sort_by(f64::total_cmp);
    Stats { n: s.len(), codes, p50: pct(&ms, 0.5), p99: pct(&ms, 0.99), p999: pct(&ms, 0.999), max: ms.last().copied().unwrap_or(f64::NAN) }
}

fn show(name: &str, s: &Stats) {
    println!(
        "{name:<24} n={:<6} p50={:>7.2}  p99={:>7.2}  p99.9={:>7.2}  max={:>8.2} ms  codes={:?}",
        s.n, s.p50, s.p99, s.p999, s.max, s.codes
    );
}

/// Both probes for `secs`: (heartbeat, policy). Heartbeat uuids rotate over a pool that reuses
/// one only after 75 s (one heartbeat per uuid per 60 s). Also prints how late a 1 ms sleep in
/// this process woke up (pauses of the machine itself, not of tether-api).
fn probes(addr: &str, secs: u64, gap: Duration, pool: usize, next_uuid: &Arc<AtomicUsize>) -> (Samples, Samples) {
    let until = Instant::now() + Duration::from_secs(secs);
    let a = addr.to_owned();
    let p = std::thread::spawn(move || every(gap, until, |_| sample(req(&a, "GET", "/api/policy", &[], b""))));
    let jitter = std::thread::spawn(move || {
        let (mut max, mut over) = (0f64, 0);
        while Instant::now() < until {
            let t = Instant::now();
            std::thread::sleep(Duration::from_millis(1));
            let late = t.elapsed().as_secs_f64() * 1000. - 1.;
            max = max.max(late);
            over += usize::from(late > 20.);
        }
        (max, over)
    });
    let h = every(gap, until, |_| {
        let n = next_uuid.fetch_add(1, Ordering::Relaxed) % pool;
        let hb = format!(r#"{{"uuid":"probe-{n:08}","id":"{}","profile":"technician","version":"0.5.21","os":"windows 10.0.26300","serial":3}}"#, 100_000_000 + n);
        sample(req(addr, "POST", "/api/heartbeat", &[("X-Forwarded-For", &ip(100, n))], hb.as_bytes()))
    });
    let (max, over) = jitter.join().unwrap();
    if SELF_CHECK.load(Ordering::Relaxed) {
        println!("  self-check: a 1 ms sleep in the generator woke up to {max:.1} ms late, {over} times more than 20 ms late");
    }
    (h, p.join().unwrap())
}

fn admin_req(addr: &str, key: &SigningKey, method: &str, path: &str, body: &[u8]) -> Reply {
    let sig = reqauth::sign_request(key, method, path, body, now());
    req(addr, method, path, &[(reqauth::HEADER, &sig)], body).unwrap_or_else(|e| panic!("{method} {path}: {e}"))
}

fn enrol(addr: &str, token: &str, i: usize, net: u8) -> io::Result<Reply> {
    let key = device_key(i);
    let b = json!({"uuid": uuid(i), "pubkey": key_to_b64(&key.verifying_key()), "token": token}).to_string();
    let sig = reqauth::sign_device_request(&key, &uuid(i), RELAY, "POST", "/api/status/enroll", b.as_bytes(), now());
    req(addr, "POST", "/api/status/enroll", &[("X-Forwarded-For", &ip(net, i)), (reqauth::HEADER, &sig)], b.as_bytes())
}

/// Simulated technician desktops (`members=M`): each its own roster key (requests signed with
/// `kid`, from its own address), every `member_s` the summary with `If-None-Match` (and
/// `Accept-Encoding: gzip`, but `plain_pct` % of them as an old desktop without it), every `open_s`
/// a host's report and every `sw_s` a software list (devices below `sbom`) of a host in its scope.
/// The first `scoped` members see one customer each (`customers` references, device i has
/// `c<i % customers>`), the rest every customer.
struct Members {
    keys: Vec<SigningKey>,
    scoped: usize,
    customers: usize,
    devices: usize,
    sbom: usize,
    /// Devices enrolled through the API have no stored list (`prep` writes them from here on).
    sbom_from: usize,
    plain_pct: u64,
    every: [u64; 3],
}

/// Request kinds of a member.
const KINDS: [&str; 3] = ["summary", "host open", "software list"];

#[derive(Default)]
struct Tally {
    samples: [Samples; 3],
    bytes: [u64; 3],
    /// Summary answers: (200 gzip, 200 plain, 304), and the body bytes of the 200s each.
    summary: [u64; 3],
    summary_bytes: [u64; 2],
}

impl Members {
    fn new(a: &BTreeMap<String, String>, devices: usize) -> Members {
        let m = num(a, "members", 0) as usize;
        Members {
            keys: (0..m).map(member_key).collect(),
            scoped: (num(a, "scoped", 0) as usize).min(m),
            customers: num(a, "customers", 0) as usize,
            devices,
            sbom: num(a, "sbom", 0) as usize,
            sbom_from: num(a, "enroll", 100) as usize,
            plain_pct: num(a, "plain_pct", 10),
            every: [num(a, "member_s", 300), num(a, "open_s", 300), num(a, "sw_s", 900)],
        }
    }

    /// Member j's one customer (None: all of them).
    fn customer(&self, j: usize) -> Option<usize> {
        (j < self.scoped && self.customers > 0).then(|| j % self.customers)
    }

    fn team_doc(&self, load: &SigningKey, admin: &SigningKey, serial: u64) -> Vec<u8> {
        let entry = |name: String, k: &SigningKey, customers: Value| json!({"name": name, "key": key_to_b64(&k.verifying_key()), "write": false, "audit": false, "sbom": true, "agent": true, "customers": customers, "service": false});
        // The load member and a second one for the software searches (`search_loop`).
        let mut members = vec![entry("load".into(), load, json!(["*"])), entry("load-search".into(), &member_key(usize::MAX - 1), json!(["*"]))];
        members.extend(self.keys.iter().enumerate().map(|(j, k)| entry(format!("tech-{j:04}"), k, self.customer(j).map_or(json!(["*"]), |c| json!([format!("c{c:04}")])))));
        let doc = json!({"serial": serial, "members": members});
        serde_json::to_vec(&json!({"team": envelope::sign(&serde_json::to_vec(&doc).unwrap(), admin)})).unwrap()
    }

    /// Every request of the run: (ms from the start, member, kind), each kind at a random offset
    /// within its period.
    fn schedule(&self, secs: u64, rng: &mut Rng) -> Vec<(u64, usize, usize)> {
        let mut ev = Vec::new();
        for j in 0..self.keys.len() {
            for (kind, every) in self.every.iter().enumerate() {
                let mut at = (rng.unit() * (every * 1000) as f64) as u64;
                while at < secs * 1000 {
                    ev.push((at, j, kind));
                    at += every * 1000;
                }
            }
        }
        ev.sort_unstable();
        ev
    }

    /// A device in member j's scope (below `below`), if any.
    /// A device in member j's scope in `from..below`, if any.
    fn device(&self, j: usize, from: usize, below: usize, rng: &mut Rng) -> Option<usize> {
        let (c, step) = self.customer(j).map_or((0, 1), |c| (c, self.customers));
        let first = if from <= c { c } else { c + (from - c).div_ceil(step) * step };
        let n = below.saturating_sub(first).div_ceil(step);
        (n > 0).then(|| first + step * ((rng.unit() * n as f64) as usize % n))
    }

    fn request(&self, addr: &str, j: usize, kind: usize, etag: &Mutex<Vec<Option<String>>>, rng: &mut Rng, tally: &Mutex<Tally>) {
        let path = match kind {
            0 => "/api/team/status".to_owned(),
            1 => match self.device(j, 0, self.devices, rng) {
                Some(d) => format!("/api/team/status/{}", uuid(d)),
                None => return,
            },
            _ => match self.device(j, self.sbom_from, self.sbom, rng) {
                Some(d) => format!("/api/team/status/{}/sbom", uuid(d)),
                None => return,
            },
        };
        let sig = reqauth::sign_request_for_kid(&self.keys[j], RELAY, "GET", &path, b"", now());
        let from = format!("10.{}.{}.{}", 30 + (j >> 16), (j >> 8) & 255, j & 255);
        let gzip = (j as u64 * 37 % 100) >= self.plain_pct;
        let inm = if kind == 0 { etag.lock().unwrap()[j].clone() } else { None };
        let mut h: Vec<(&str, &str)> = vec![(reqauth::HEADER, &sig), ("X-Forwarded-For", &from)];
        if kind == 0 && gzip {
            h.push(("Accept-Encoding", "gzip"));
        }
        if let Some(e) = &inm {
            h.push(("If-None-Match", e));
        }
        let r = req(addr, "GET", &path, &h, b"");
        let mut t = tally.lock().unwrap();
        match &r {
            Ok(r) => {
                t.bytes[kind] += r.bytes as u64;
                if kind == 0 {
                    match (r.status, r.header("Content-Encoding")) {
                        (304, _) => t.summary[2] += 1,
                        (200, Some("gzip")) => (t.summary[0], t.summary_bytes[0]) = (t.summary[0] + 1, t.summary_bytes[0] + r.bytes as u64),
                        (200, _) => (t.summary[1], t.summary_bytes[1]) = (t.summary[1] + 1, t.summary_bytes[1] + r.bytes as u64),
                        _ => {}
                    }
                    if r.status == 200 {
                        etag.lock().unwrap()[j] = r.header("ETag").map(str::to_owned);
                    }
                }
            }
            Err(_) => {}
        }
        t.samples[kind].push(sample(r));
    }
}

fn run(a: &BTreeMap<String, String>) -> bool {
    let addr = a.get("addr").expect("addr=").clone();
    let pid = num(a, "pid", 0);
    let (devices, enroll) = (num(a, "devices", 20_000) as usize, num(a, "enroll", 100) as usize);
    let (secs, warmup, base) = (num(a, "seconds", 600), num(a, "warmup", 60), num(a, "baseline", 60));
    let gap = Duration::from_millis(num(a, "probe_ms", 50));
    let (workers, full_pct) = (num(a, "workers", 16) as usize, num(a, "full_pct", 5) as f64);
    let (new_per_s, new_secs) = (num(a, "new_per_s", 2), num(a, "new_secs", 300));
    let (summary_s, metrics_s) = (num(a, "summary_s", 30), num(a, "metrics_s", 15));
    let sbom_rps = num(a, "sbom_rps", 0);
    let admin = read_key(a.get("key").expect("key="));
    let search_s = num(a, "search_s", 0);

    // A published policy, so GET /api/policy reads and sends the file as real clients get it.
    let doc = serde_json::to_vec(&envelope::sign(format!(r#"{{"serial":1,"issued_at":{}}}"#, now()).as_bytes(), &admin)).unwrap();
    let r = admin_req(&addr, &admin, "PUT", "/api/admin/policy", &doc);
    assert_eq!(r.status, 204, "publish policy: {}", r.body);
    // A throwaway roster member for the team summary; with members > 0 a signed team document
    // with it and the simulated desktops (`scoped` of them limited to one customer each).
    let member = member_key(usize::MAX);
    let sim = Members::new(a, devices);
    let r = if sim.keys.is_empty() && search_s == 0 {
        let roster = json!({"members": [{"name": "load", "key": key_to_b64(&member.verifying_key()), "write": false, "audit": false}]});
        admin_req(&addr, &admin, "PUT", "/api/admin/team", roster.to_string().as_bytes())
    } else {
        admin_req(&addr, &admin, "PUT", "/api/admin/team", &sim.team_doc(&member, &admin, num(a, "doc_serial", 1)))
    };
    assert_eq!(r.status, 204, "publish roster: {}", r.body);
    // Software searches from here on (the start-up scan is still building sidecars).
    let search_until = Arc::new(AtomicBool::new(false));
    let searches = (search_s > 0).then(|| {
        let (addr, member, stop) = (addr.clone(), member.clone(), search_until.clone());
        std::thread::spawn(move || search_loop(&addr, &member, &member_key(usize::MAX - 1), search_s, &stop, pid))
    });

    // Enrolment through the API for devices 0..enroll (one IP each: 30 new pins per IP and
    // minute), then an idempotent one of the last device: its answer is the pace for everyone.
    let token = status::make_token("load", now() + 7200, None, None, false, &admin).unwrap().0;
    let t0 = Instant::now();
    for i in 0..enroll.min(devices) {
        let r = enrol(&addr, &token, i, 200).expect("enroll");
        assert_eq!(r.status, 200, "enroll {}: {}", uuid(i), r.body);
    }
    let r = enrol(&addr, &token, devices - 1, 200).unwrap();
    let next = serde_json::from_str::<Value>(&r.body).unwrap()["next"].as_u64().unwrap();
    println!("devices {devices} ({enroll} enrolled through the API in {:.1} s), next {next} s, posts about {:.1}/s", t0.elapsed().as_secs_f64(), devices as f64 / next as f64);
    let rss_enrolled = rss_kib(pid);

    let pool = (75_000 / gap.as_millis().max(1) as usize).max(100);
    let next_uuid = Arc::new(AtomicUsize::new(0));
    println!("warm-up: {warmup} s of probes (not measured)");
    let _ = probes(&addr, warmup, gap, pool, &next_uuid);
    let rss_ref = rss_kib(pid);
    println!("tether-api RSS: {:.1} MiB after enrolment, {:.1} MiB after the warm-up (the reference)", rss_enrolled as f64 / 1024., rss_ref as f64 / 1024.);
    println!("baseline: {base} s of probes alone");
    let (hb_base, pol_base) = probes(&addr, base, gap, pool, &next_uuid);

    // Every post of the run, at the device's offset and then every `next` seconds.
    let mut rng = Rng(now_ms() | 1);
    let mut events: Vec<(u64, usize)> = Vec::new();
    for i in 0..devices {
        let mut at = (rng.unit() * (next * 1000) as f64) as u64;
        while at < secs * 1000 {
            events.push((at, i));
            at += next * 1000;
        }
    }
    events.sort_unstable();
    let fulls: Vec<bool> = events.iter().map(|_| rng.unit() * 100. < full_pct).collect();
    // Devices enrolled through the API have no report yet: their first post is a full one.
    let need_full: Arc<Vec<AtomicBool>> = Arc::new((0..devices).map(|i| AtomicBool::new(i < enroll)).collect());
    println!("run: {secs} s, {} status posts scheduled, {} new enrolments, summary every {summary_s} s, metrics every {metrics_s} s", events.len(), new_per_s * new_secs);
    let posts: Arc<Mutex<Samples>> = Arc::new(Mutex::new(Vec::new()));
    let refusals: Arc<Mutex<BTreeMap<String, usize>>> = Arc::new(Mutex::new(BTreeMap::new()));
    let (tx, rx) = mpsc::channel::<(usize, bool)>();
    let rx = Arc::new(Mutex::new(rx));
    let pool_threads: Vec<_> = (0..workers)
        .map(|_| {
            let (rx, posts, refusals, need_full, addr) = (rx.clone(), posts.clone(), refusals.clone(), need_full.clone(), addr.clone());
            std::thread::spawn(move || loop {
                let Ok((i, want_full)) = rx.lock().unwrap().recv() else { break };
                let is_full = want_full || need_full[i].load(Ordering::Relaxed);
                let body = if is_full { full(i, now_ms()) } else { heartbeat(i, now_ms()) };
                let sig = reqauth::sign_device_request(&device_key(i), &uuid(i), RELAY, "POST", "/api/status", body.as_bytes(), now());
                let r = req(&addr, "POST", "/api/status", &[(reqauth::HEADER, &sig), ("X-Forwarded-For", &ip(50 + (i >> 16) as u8, i))], body.as_bytes());
                let s = match r {
                    Ok(r) => {
                        if r.status == 200 && is_full {
                            need_full[i].store(false, Ordering::Relaxed);
                        } else if r.status != 200 {
                            *refusals.lock().unwrap().entry(format!("{} {}", r.status, r.body)).or_default() += 1;
                        }
                        (r.status, r.took.as_secs_f64() * 1000.)
                    }
                    Err(e) => {
                        *refusals.lock().unwrap().entry(format!("error {e}")).or_default() += 1;
                        (0, f64::NAN)
                    }
                };
                posts.lock().unwrap().push(s);
            })
        })
        .collect();
    let start = Instant::now();
    let until = start + Duration::from_secs(secs);
    let peak = Arc::new(AtomicUsize::new(rss_ref as usize));
    let sampler = {
        let peak = peak.clone();
        std::thread::spawn(move || {
            let mut series = Vec::new();
            while Instant::now() < until {
                let r = rss_kib(pid);
                peak.fetch_max(r as usize, Ordering::Relaxed);
                if start.elapsed().as_secs() >= 60 * series.len() as u64 {
                    series.push(format!("{:.1}", r as f64 / 1024.));
                }
                std::thread::sleep(Duration::from_secs(1));
            }
            series
        })
    };
    let dispatcher = std::thread::spawn(move || {
        for ((at, i), f) in events.into_iter().zip(fulls) {
            if let Some(d) = (start + Duration::from_millis(at)).checked_duration_since(Instant::now()) {
                std::thread::sleep(d);
            }
            let _ = tx.send((i, f));
        }
    });
    let new_enrols = {
        let (addr, token) = (addr.clone(), token.clone());
        let stop = start + Duration::from_secs(new_secs.min(secs));
        std::thread::spawn(move || every(Duration::from_millis(1000 / new_per_s.max(1)), stop, |n| sample(enrol(&addr, &token, devices + n, 201))))
    };
    let summaries = {
        let (addr, member) = (addr.clone(), member.clone());
        std::thread::spawn(move || {
            every(Duration::from_secs(summary_s), until, |_| {
                let sig = reqauth::sign_request(&member, "GET", "/api/team/status", b"", now());
                sample(req(&addr, "GET", "/api/team/status", &[(reqauth::HEADER, &sig)], b""))
            })
        })
    };
    let scrapes = {
        let addr = addr.clone();
        std::thread::spawn(move || every(Duration::from_secs(metrics_s), until, |_| sample(req(&addr, "GET", "/metrics", &[], b""))))
    };
    // The simulated desktops: their schedule through `member_workers` threads (open loop).
    let sim = Arc::new(sim);
    let tally = Arc::new(Mutex::new(Tally::default()));
    let member_threads = {
        let events = sim.schedule(secs, &mut rng);
        println!("members: {} ({} limited to one of {} customers, {} % without gzip), {} requests scheduled", sim.keys.len(), sim.scoped, sim.customers, sim.plain_pct, events.len());
        let (mtx, mrx) = mpsc::channel::<(usize, usize)>();
        let mrx = Arc::new(Mutex::new(mrx));
        let etags = Arc::new(Mutex::new(vec![None; sim.keys.len()]));
        let mut ts: Vec<_> = (0..num(a, "member_workers", 32))
            .map(|w| {
                let (mrx, sim, tally, etags, addr) = (mrx.clone(), sim.clone(), tally.clone(), etags.clone(), addr.clone());
                std::thread::spawn(move || {
                    let mut rng = Rng((now_ms() ^ (w + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15)) | 1);
                    loop {
                        let Ok((j, kind)) = mrx.lock().unwrap().recv() else { break };
                        sim.request(&addr, j, kind, &etags, &mut rng, &tally);
                    }
                })
            })
            .collect();
        ts.push(std::thread::spawn(move || {
            for (at, j, kind) in events {
                if let Some(d) = (start + Duration::from_millis(at)).checked_duration_since(Instant::now()) {
                    std::thread::sleep(d);
                }
                let _ = mtx.send((j, kind));
            }
        }));
        ts
    };
    // `junk` requests at once every second from one address, member-signed by unknown keys without
    // `kid` (each tries every roster key until the address reaches the failure limit).
    let junk_n = num(a, "junk", 0) as usize;
    let junk = (junk_n > 0).then(|| {
        let addr = addr.clone();
        std::thread::spawn(move || {
            let mut out: Samples = Vec::new();
            while Instant::now() < until {
                let next = Instant::now() + Duration::from_secs(1);
                let hs: Vec<_> = (0..junk_n)
                    .map(|_| {
                        let addr = addr.clone();
                        std::thread::spawn(move || {
                            let sig = reqauth::sign_request_for(&generate_key(), RELAY, "GET", "/api/team/me", b"", now());
                            sample(req(&addr, "GET", "/api/team/me", &[(reqauth::HEADER, &sig), ("X-Forwarded-For", "198.51.100.66")], b""))
                        })
                    })
                    .collect();
                out.extend(hs.into_iter().map(|h| h.join().unwrap()));
                if let Some(d) = next.checked_duration_since(Instant::now()) {
                    std::thread::sleep(d);
                }
            }
            out
        })
    });
    // One 60 KiB list per device, in device order, at the relay's global budget, until every device
    // has sent one or the run is over.
    let sbom_refused: Arc<Mutex<BTreeMap<String, usize>>> = Arc::new(Mutex::new(BTreeMap::new()));
    let sboms = (sbom_rps > 0).then(|| {
        let (addr, refused) = (addr.clone(), sbom_refused.clone());
        let stop = start + Duration::from_micros((devices as u64 * 1_000_000 / sbom_rps).min(secs * 1_000_000));
        std::thread::spawn(move || {
            every(Duration::from_micros(1_000_000 / sbom_rps), stop, |n| {
                let body = sbom_doc(n);
                let sig = reqauth::sign_device_request(&device_key(n), &uuid(n), RELAY, "PUT", "/api/status/sbom", body.as_bytes(), now());
                let r = req(&addr, "PUT", "/api/status/sbom", &[(reqauth::HEADER, &sig), ("X-Forwarded-For", &ip(150 + (n >> 16) as u8, n))], body.as_bytes());
                if let Ok(r) = &r {
                    if r.status != 204 {
                        *refused.lock().unwrap().entry(format!("{} {}", r.status, r.body)).or_default() += 1;
                    }
                }
                sample(r)
            })
        })
    });
    let (hb_run, pol_run) = probes(&addr, secs, gap, pool, &next_uuid);
    dispatcher.join().unwrap();
    for t in pool_threads {
        t.join().unwrap();
    }
    let (new_enrols, summaries, scrapes) = (new_enrols.join().unwrap(), summaries.join().unwrap(), scrapes.join().unwrap());
    let sboms: Samples = sboms.map(|t| t.join().unwrap()).unwrap_or_default();
    for t in member_threads {
        t.join().unwrap();
    }
    let junk: Samples = junk.map(|t| t.join().unwrap()).unwrap_or_default();
    search_until.store(true, Ordering::Relaxed);
    let searches: Samples = searches.map(|t| t.join().unwrap()).unwrap_or_default();
    let series = sampler.join().unwrap();
    let rss_end = rss_kib(pid);
    let peak = peak.load(Ordering::Relaxed).max(rss_end as usize) as u64;

    let (hb_b, pol_b, hb_r, pol_r) = (stats(&hb_base), stats(&pol_base), stats(&hb_run), stats(&pol_run));
    let posts = stats(&posts.lock().unwrap());
    let (new_s, sum_s, met_s) = (stats(&new_enrols), stats(&summaries), stats(&scrapes));
    let sbom_s = stats(&sboms);
    println!();
    show("heartbeat baseline", &hb_b);
    show("heartbeat during run", &hb_r);
    show("policy baseline", &pol_b);
    show("policy during run", &pol_r);
    show("status posts", &posts);
    show("new enrolments", &new_s);
    show("team summary", &sum_s);
    show("metrics scrapes", &met_s);
    if sbom_rps > 0 {
        let ok = sbom_s.codes.get(&204).copied().unwrap_or(0);
        println!("software list uploads: {} sent at {sbom_rps}/s, {ok} accepted (204), {} refused or failed", sbom_s.n, sbom_s.n - ok);
        show("software list uploads", &sbom_s);
        for (k, n) in sbom_refused.lock().unwrap().iter() {
            println!("  refused software list upload x{n}: {}", k.chars().take(120).collect::<String>());
        }
    }
    for (k, n) in refusals.lock().unwrap().iter() {
        println!("  refused or failed status post x{n}: {}", k.chars().take(120).collect::<String>());
    }
    let t = tally.lock().unwrap();
    let member_stats: Vec<Stats> = t.samples.iter().map(stats).collect();
    if !sim.keys.is_empty() {
        let per_s = |n: u64| n as f64 / secs as f64;
        for (k, s) in KINDS.iter().zip(&member_stats) {
            show(&format!("member {k}"), s);
            println!("  {:.2} requests/s, {:.0} bytes/s out", per_s(s.n as u64), per_s(t.bytes[KINDS.iter().position(|x| x == k).unwrap()]));
        }
        let [gz, plain, nm] = t.summary;
        let all = (gz + plain + nm).max(1);
        println!("summary answers: {gz} 200 gzip ({:.0} bytes avg), {plain} 200 plain ({:.0} bytes avg), {nm} 304: 304 share {:.1} %", t.summary_bytes[0] as f64 / gz.max(1) as f64, t.summary_bytes[1] as f64 / plain.max(1) as f64, nm as f64 * 100. / all as f64);
    }
    if !junk.is_empty() {
        show("junk without kid", &stats(&junk));
    }
    let search_stats = stats(&searches);
    if search_s > 0 {
        show("software searches", &search_stats);
    }
    // The full summary once plain and once gzip (sizes), and the relay's counters.
    let sig = |p: &str| reqauth::sign_request(&member, "GET", p, b"", now());
    if let (Ok(p), Ok(g)) = (req(&addr, "GET", "/api/team/status", &[(reqauth::HEADER, &sig("/api/team/status"))], b""), req(&addr, "GET", "/api/team/status", &[(reqauth::HEADER, &sig("/api/team/status")), ("Accept-Encoding", "gzip")], b"")) {
        println!("full summary: {} bytes plain, {} bytes gzip ({:.1} x), {} hosts", p.bytes, g.bytes, p.bytes as f64 / g.bytes.max(1) as f64, serde_json::from_str::<Value>(&p.body).ok().and_then(|v| v["hosts"].as_array().map(Vec::len)).unwrap_or(0));
    }
    if let Ok(m) = req(&addr, "GET", "/metrics", &[], b"") {
        for l in m.body.lines().filter(|l| ["tether_api_status_summary_", "tether_api_status_software_", "tether_api_team_audit_", "tether_api_busy_rejected_total"].iter().any(|p| l.starts_with(p))) {
            println!("  metrics: {l}");
        }
        let codes: BTreeMap<String, u64> = m.body.lines().filter_map(|l| l.strip_prefix("tether_api_requests_total{")).filter_map(|l| {
            let (labels, n) = l.split_once("} ")?;
            Some((labels.to_owned(), n.trim().parse().ok()?))
        }).collect();
        for (k, n) in codes {
            println!("  metrics: requests {k}: {n}");
        }
    }
    let mib = |k: u64| k as f64 / 1024.;
    println!("tether-api RSS: reference {:.1} MiB, end {:.1} MiB, peak {:.1} MiB", mib(rss_ref), mib(rss_end), mib(peak));
    println!("tether-api RSS each minute of the run (MiB): {}", series.join(" "));

    let r429 = posts.codes.get(&429).copied().unwrap_or(0);
    let all = [&hb_b, &hb_r, &pol_b, &pol_r, &posts, &new_s, &sum_s, &met_s, &sbom_s];
    let mut checks = Vec::new();
    for (name, b, r) in [("heartbeat", &hb_b, &hb_r), ("policy", &pol_b, &pol_r)] {
        checks.push((format!("{name} p99 {:.2} ms <= 1.2 x {:.2} + 2 ms", r.p99, b.p99), r.p99 <= 1.2 * b.p99 + 2.));
        checks.push((format!("{name} p99.9 {:.2} ms <= 1.5 x {:.2} + 5 ms", r.p999, b.p999), r.p999 <= 1.5 * b.p999 + 5.));
        checks.push((format!("{name} slowest {:.2} ms <= 50 ms", r.max), r.max <= 50.));
    }
    checks.extend([
        ("no 5xx (also no 503 busy) and no connection errors on any route".to_owned(), all.iter().all(|s| s.codes.keys().all(|c| (1..500).contains(c)))),
        (format!("status 429: {r429} of {} posts <= 0.1 %", posts.n), r429 as f64 <= posts.n as f64 * 0.001),
        ("every status post accepted or 429".to_owned(), posts.codes.keys().all(|c| matches!(c, 200 | 429))),
        ("every probe, enrolment, summary and scrape answered 200".to_owned(), [&hb_b, &hb_r, &pol_b, &pol_r, &new_s, &sum_s, &met_s].iter().all(|s| s.codes.keys().all(|c| *c == 200))),
        ("every software list upload accepted (204) or 429".to_owned(), sbom_s.codes.keys().all(|c| matches!(c, 204 | 429))),
        ("every member request answered 200 or 304 (no 429, no 5xx, no connection error)".to_owned(), member_stats.iter().all(|s| s.codes.keys().all(|c| matches!(c, 200 | 304)))),
        ("every software search answered 200".to_owned(), search_stats.codes.keys().all(|c| *c == 200)),
        (format!("peak RSS {:.1} MiB < 48 MiB", mib(peak)), peak < 48 * 1024),
        (format!("RSS at the end within 10 % of the reference ({:.1} vs {:.1} MiB)", mib(rss_end), mib(rss_ref)), (rss_end as f64 - rss_ref as f64).abs() <= rss_ref as f64 * 0.1),
    ]);
    println!();
    for (what, ok) in &checks {
        println!("{} {what}", if *ok { "ok  " } else { "FAIL" });
    }
    let pass = checks.iter().all(|c| c.1);
    println!("{}", if pass { "PASS" } else { "FAIL" });
    pass
}

//! Load generator for tether-api's device status store (driven by `tools/load/status-run.sh`).
//!
//!   status-load keygen <file>                      a throwaway admin key; prints its public key
//!   status-load prep <data-dir> devices=N enroll=E pins devices E..N (status/keys.json, which
//!                                                  the relay migrates) with a last full report
//!                                                  in seen.json
//!   status-load run addr=<host:port> key=<file> pid=<tether-api pid> devices=N enroll=E
//!       seconds=600 warmup=60 baseline=60 probe_ms=50 workers=16 full_pct=5
//!       new_per_s=2 new_secs=300 summary_s=30 metrics_s=15
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
//!     every `metrics_s`.
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

fn device_key(i: usize) -> SigningKey {
    let mut seed = *b"tether-load-status-device-key-00";
    seed[..8].copy_from_slice(&(i as u64).to_le_bytes());
    SigningKey::from_bytes(&seed)
}

fn uuid(i: usize) -> String {
    format!("load-{i:08}")
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
    took: Duration,
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
    let text = String::from_utf8_lossy(&raw);
    let (head, body) = text.split_once("\r\n\r\n").ok_or_else(|| io::Error::other("no response head"))?;
    let status = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).ok_or_else(|| io::Error::other("no status"))?;
    Ok(Reply { status, body: body.to_owned(), took })
}

/// `--self-check` on `run`: also print how late this generator itself woke up (its own pauses).
static SELF_CHECK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn args(list: &[String]) -> BTreeMap<String, String> {
    list.iter().filter_map(|a| a.split_once('=')).map(|(k, v)| (k.to_owned(), v.to_owned())).collect()
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
            prep(Path::new(&argv[1]), num(&a, "devices", 20_000) as usize, num(&a, "enroll", 100) as usize);
        }
        Some("run") => {
            SELF_CHECK.store(argv.iter().any(|a| a == "--self-check"), Ordering::Relaxed);
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
        .map(|k| json!({"name": format!("Example Practice Management Server {k}"), "version": "12.3.4567.89", "role": if k == 0 { "server" } else { "client" }}))
        .collect();
    let faults: Vec<Value> =
        (0..nf).map(|k| json!({"id": format!("example-product-{k}/database-backup-age"), "state": if k == 0 { "crit" } else { "warn" }, "since": "2026-11-23T07:55:00Z"})).collect();
    (json!(products), json!(faults))
}

const COUNTS: &str = r#"{"ok":31,"warn":2,"crit":1,"unknown":0,"setup":0}"#;

/// Devices `enroll..devices`: pinned, with a last full report seen two minutes ago.
fn prep(data: &Path, devices: usize, enroll: usize) {
    let dir = data.join("status");
    std::fs::create_dir_all(&dir).unwrap();
    let t = now();
    let (mut keys, mut seen) = (serde_json::Map::new(), serde_json::Map::new());
    let counts: Value = serde_json::from_str(COUNTS).unwrap();
    for i in enroll..devices {
        let (products, faults) = products_faults(i);
        let entry = json!({"host": format!("PC-LOAD-{i:06}"), "agent": "0.1.0", "overall": "crit", "counts": counts, "products": products, "faults": faults}).to_string();
        keys.insert(uuid(i), json!({"key": key_to_b64(&device_key(i).verifying_key()), "tether_id": null, "label": "load", "enrolled": t}));
        seen.insert(uuid(i), json!({"last_seen": t - 120, "seq": 1, "next": 300, "entry": entry}));
    }
    std::fs::write(dir.join("keys.json"), serde_json::to_vec(&keys).unwrap()).unwrap();
    std::fs::write(dir.join("seen.json"), serde_json::to_vec(&seen).unwrap()).unwrap();
}

fn rss_kib(pid: u64) -> u64 {
    let s = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
    s.lines().find_map(|l| l.strip_prefix("VmRSS:")?.split_whitespace().next()?.parse().ok()).unwrap_or(0)
}

fn heartbeat(i: usize, seq: u64) -> String {
    format!(
        r#"{{"v":1,"uuid":"{}","host":"PC-LOAD-{i:06}","seq":{seq},"time":"2026-11-23T08:20:00Z","next":300,"agent":"0.1.0","packs":{{"system":9}},"overall":"crit","counts":{COUNTS},"faults":{}}}"#,
        uuid(i),
        products_faults(i).1
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
    let b = json!({"uuid": uuid(i), "pubkey": key_to_b64(&device_key(i).verifying_key()), "token": token}).to_string();
    req(addr, "POST", "/api/status/enroll", &[("X-Forwarded-For", &ip(net, i))], b.as_bytes())
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
    let hex = std::fs::read_to_string(a.get("key").expect("key=")).unwrap();
    let seed: Vec<u8> = (0..32).map(|i| u8::from_str_radix(&hex.trim()[2 * i..2 * i + 2], 16).unwrap()).collect();
    let admin = SigningKey::from_bytes(&seed.try_into().unwrap());

    // A published policy, so GET /api/policy reads and sends the file as real clients get it.
    let doc = serde_json::to_vec(&envelope::sign(format!(r#"{{"serial":1,"issued_at":{}}}"#, now()).as_bytes(), &admin)).unwrap();
    let r = admin_req(&addr, &admin, "PUT", "/api/admin/policy", &doc);
    assert_eq!(r.status, 204, "publish policy: {}", r.body);
    // A throwaway roster member for the team summary.
    let member = generate_key();
    let roster = json!({"members": [{"name": "load", "key": key_to_b64(&member.verifying_key()), "write": false, "audit": false}]});
    let r = admin_req(&addr, &admin, "PUT", "/api/admin/team", roster.to_string().as_bytes());
    assert_eq!(r.status, 204, "publish roster: {}", r.body);

    // Enrolment through the API for devices 0..enroll (one IP each: 30 new pins per IP and
    // minute), then an idempotent one of the last device: its answer is the pace for everyone.
    let token = status::make_token("load", now() + 7200, None, None, &admin).unwrap().0;
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
        let addr = addr.clone();
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
    let (hb_run, pol_run) = probes(&addr, secs, gap, pool, &next_uuid);
    dispatcher.join().unwrap();
    for t in pool_threads {
        t.join().unwrap();
    }
    let (new_enrols, summaries, scrapes) = (new_enrols.join().unwrap(), summaries.join().unwrap(), scrapes.join().unwrap());
    let series = sampler.join().unwrap();
    let rss_end = rss_kib(pid);
    let peak = peak.load(Ordering::Relaxed).max(rss_end as usize) as u64;

    let (hb_b, pol_b, hb_r, pol_r) = (stats(&hb_base), stats(&pol_base), stats(&hb_run), stats(&pol_run));
    let posts = stats(&posts.lock().unwrap());
    let (new_s, sum_s, met_s) = (stats(&new_enrols), stats(&summaries), stats(&scrapes));
    println!();
    show("heartbeat baseline", &hb_b);
    show("heartbeat during run", &hb_r);
    show("policy baseline", &pol_b);
    show("policy during run", &pol_r);
    show("status posts", &posts);
    show("new enrolments", &new_s);
    show("team summary", &sum_s);
    show("metrics scrapes", &met_s);
    for (k, n) in refusals.lock().unwrap().iter() {
        println!("  refused or failed status post x{n}: {}", k.chars().take(120).collect::<String>());
    }
    let mib = |k: u64| k as f64 / 1024.;
    println!("tether-api RSS: reference {:.1} MiB, end {:.1} MiB, peak {:.1} MiB", mib(rss_ref), mib(rss_end), mib(peak));
    println!("tether-api RSS each minute of the run (MiB): {}", series.join(" "));

    let r429 = posts.codes.get(&429).copied().unwrap_or(0);
    let all = [&hb_b, &hb_r, &pol_b, &pol_r, &posts, &new_s, &sum_s, &met_s];
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

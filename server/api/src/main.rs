//! Tether relay API. Wire contract: `docs/api.md`. Flat files under the data dir, no database.
//!
//! Layout: `policy.json`, `policy-history/<serial>.json`, `devices.json`, `nonces.json`,
//! `diag/tickets.json` + `diag/<ticket>.zip`, `download/…` (served by Caddy), `tmp/`.

use std::collections::HashMap;
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tether_trust::{envelope, policy::Bundle, reqauth, VerifyingKey};
use tiny_http::{Header, Request, Response, Server};

const DAY: u64 = 86_400;
const MAX_DEVICES: usize = 10_000;
const DEVICE_TTL: u64 = 180 * DAY;
const HB_UUID_INTERVAL: u64 = 60;
const HB_IP_PER_MIN: u32 = 30;
const HISTORY_KEEP: usize = 50;
const TICKET_OPEN: u64 = DAY;
const DIAG_KEEP: u64 = 7 * DAY;
const MIN_FREE: u64 = 2 << 30;
const TICKET_ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

const LIMIT_SMALL: u64 = 4096;
const LIMIT_HEARTBEAT: u64 = 2048;
const LIMIT_POLICY: u64 = 256 << 10;
const LIMIT_PAGE: u64 = 512 << 10;
const LIMIT_DIAG: u64 = 2 << 20;
const LIMIT_RELEASE: u64 = 200 << 20;

static STOP: AtomicBool = AtomicBool::new(false);

type Resp = Response<Box<dyn Read + Send>>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Heartbeat {
    uuid: String,
    id: String,
    profile: String,
    version: String,
    os: String,
    serial: u64,
}

#[derive(Serialize, Deserialize, Clone)]
struct Device {
    uuid: String,
    id: String,
    profile: String,
    version: String,
    os: String,
    serial: u64,
    ip: String,
    last_seen: u64,
}

#[derive(Serialize, Deserialize, Clone)]
struct Ticket {
    ticket: String,
    note: String,
    created: u64,
    uploaded: Option<u64>,
    size: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TicketRequest {
    note: String,
}

/// Just the header fields of a stored bundle (history listing, startup).
#[derive(Deserialize)]
struct BundleMeta {
    serial: u64,
    issued_at: u64,
}

struct App {
    data: PathBuf,
    keys: Vec<VerifyingKey>,
    started: u64,
    devices: HashMap<String, Device>,
    devices_dirty: bool,
    ip_hits: HashMap<String, (u64, u32)>,
    nonces: HashMap<String, u64>,
    tickets: Vec<Ticket>,
    policy_serial: Option<u64>,
    tmp_seq: u64,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn hdr(k: &str, v: &str) -> Header {
    Header::from_bytes(k.as_bytes(), v.as_bytes()).expect("valid header")
}

fn text(code: u16, msg: &str) -> Resp {
    Response::from_string(msg)
        .with_status_code(code)
        .with_header(hdr("Content-Type", "text/plain; charset=utf-8"))
        .boxed()
}

fn empty(code: u16) -> Resp {
    Response::empty(code).boxed()
}

fn json<T: Serialize>(v: &T) -> Resp {
    Response::from_data(serde_json::to_vec(v).unwrap_or_default())
        .with_header(hdr("Content-Type", "application/json"))
        .boxed()
}

fn not_found() -> Resp {
    text(404, "not found")
}

fn too_large() -> Resp {
    text(413, "body too large")
}

fn internal(e: impl std::fmt::Display) -> Resp {
    eprintln!("error: {e}");
    text(500, "internal error")
}

fn header(rq: &Request, name: &str) -> Option<String> {
    rq.headers().iter().find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name)).map(|h| h.value.as_str().to_owned())
}

/// Caddy sets X-Forwarded-For; trust it only from the local proxy.
fn client_ip(rq: &Request) -> String {
    let peer = rq.remote_addr().map(|a| a.ip());
    if peer.is_some_and(|ip| ip.is_loopback()) {
        if let Some(xff) = header(rq, "X-Forwarded-For") {
            let first = xff.split(',').next().unwrap_or("").trim();
            if !first.is_empty() && first.len() <= 64 {
                return first.to_owned();
            }
        }
    }
    peer.map(|ip| ip.to_string()).unwrap_or_default()
}

fn read_body(rq: &mut Request, limit: u64) -> Result<Vec<u8>, Resp> {
    if rq.body_length().is_some_and(|n| n as u64 > limit) {
        return Err(too_large());
    }
    let mut buf = Vec::new();
    rq.as_reader().take(limit + 1).read_to_end(&mut buf).map_err(|_| text(400, "could not read body"))?;
    if buf.len() as u64 > limit {
        return Err(too_large());
    }
    Ok(buf)
}

fn disk_free(p: &Path) -> u64 {
    let Ok(c) = CString::new(p.as_os_str().as_bytes()) else { return 0 };
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return 0;
    }
    s.f_bavail as u64 * s.f_frsize as u64
}

/// `verify_request` needs the body as one slice; mapping the temp file keeps a 200 MiB upload out
/// of anonymous memory (MemoryMax=64M; page cache is reclaimable). The file is private to us.
fn with_mapped<T>(path: &Path, f: impl FnOnce(&[u8]) -> T) -> io::Result<T> {
    let file = File::open(path)?;
    let len = file.metadata()?.len() as usize;
    if len == 0 {
        return Ok(f(&[]));
    }
    unsafe {
        let p = libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ, libc::MAP_PRIVATE, file.as_raw_fd(), 0);
        if p == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        libc::madvise(p, len, libc::MADV_SEQUENTIAL); // lets the kernel drop pages behind the hash
        let r = f(std::slice::from_raw_parts(p as *const u8, len));
        libc::munmap(p, len);
        Ok(r)
    }
}

fn mkdirs(p: &Path, mode: u32) -> io::Result<()> {
    fs::DirBuilder::new().recursive(true).mode(mode).create(p)
}

fn random_bytes(buf: &mut [u8]) -> io::Result<()> {
    File::open("/dev/urandom")?.read_exact(buf)
}

fn valid_channel(s: &str) -> bool {
    (1..=32).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn valid_segment(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 255
        && s != "."
        && s != ".."
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// `<channel>/<path>` → absolute path under `download/`, or 400.
fn release_path(data: &Path, rel: &str) -> Result<PathBuf, Resp> {
    let bad = || text(400, "invalid release path");
    let (channel, path) = rel.split_once('/').ok_or_else(bad)?;
    if !valid_channel(channel) || !path.split('/').all(valid_segment) {
        return Err(bad());
    }
    Ok(data.join("download").join(channel).join(path))
}

fn valid_heartbeat(hb: &Heartbeat) -> Result<(), String> {
    let graphic = |s: &str| s.bytes().all(|b| b.is_ascii_graphic());
    if !(8..=64).contains(&hb.uuid.len()) || !hb.uuid.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err("bad uuid".into());
    }
    if hb.id.len() > 32 || !graphic(&hb.id) {
        return Err("bad id".into());
    }
    if hb.profile != "technician" && hb.profile != "customer" {
        return Err("bad profile".into());
    }
    if hb.version.is_empty() || hb.version.len() > 32 || !graphic(&hb.version) {
        return Err("bad version".into());
    }
    if hb.os.len() > 128 || hb.os.chars().any(char::is_control) {
        return Err("bad os".into());
    }
    Ok(())
}

/// Serial and issue time of a stored signed policy, verified against the key it names (so history
/// stays readable after an admin key rotation; it was checked against trusted keys on upload).
fn stored_meta(raw: &[u8]) -> Option<BundleMeta> {
    let doc: envelope::Signed = serde_json::from_slice(raw).ok()?;
    let key = tether_trust::key_from_b64(&doc.key).ok()?;
    serde_json::from_slice(&envelope::open(&doc, &[key]).ok()?).ok()
}

fn load_json<T: for<'de> Deserialize<'de> + Default>(p: &Path) -> T {
    match fs::read(p) {
        Ok(b) => serde_json::from_slice(&b).unwrap_or_else(|e| {
            eprintln!("warning: ignoring unreadable {}: {e}", p.display());
            T::default()
        }),
        Err(_) => T::default(),
    }
}

impl App {
    fn open(data: PathBuf, keys: Vec<VerifyingKey>) -> io::Result<Self> {
        mkdirs(&data, 0o750)?;
        mkdirs(&data.join("download"), 0o750)?;
        for d in ["policy-history", "diag", "tmp"] {
            mkdirs(&data.join(d), 0o700)?;
        }
        for e in fs::read_dir(data.join("tmp"))? {
            let _ = fs::remove_file(e?.path());
        }
        let devices: Vec<Device> = load_json(&data.join("devices.json"));
        let policy_serial = fs::read(data.join("policy.json")).ok().and_then(|b| stored_meta(&b)).map(|m| m.serial);
        Ok(App {
            devices: devices.into_iter().map(|d| (d.uuid.clone(), d)).collect(),
            devices_dirty: false,
            ip_hits: HashMap::new(),
            nonces: load_json(&data.join("nonces.json")),
            tickets: load_json(&data.join("diag/tickets.json")),
            policy_serial,
            started: now(),
            tmp_seq: 0,
            keys,
            data,
        })
    }

    fn tmp_path(&mut self) -> PathBuf {
        self.tmp_seq += 1;
        self.data.join("tmp").join(format!("{}-{}", std::process::id(), self.tmp_seq))
    }

    fn write_atomic(&mut self, dest: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
        let tmp = self.tmp_path();
        let r = (|| {
            let mut f = OpenOptions::new().write(true).create_new(true).mode(mode).open(&tmp)?;
            f.write_all(bytes)?;
            f.sync_all()?;
            fs::rename(&tmp, dest)
        })();
        if r.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        r
    }

    fn save<T: Serialize>(&mut self, name: &str, v: &T) -> io::Result<()> {
        let bytes = serde_json::to_vec(v).map_err(io::Error::other)?;
        self.write_atomic(&self.data.join(name), &bytes, 0o600)
    }

    /// Streams the body to a new temp file (mode `mode`), enforcing `limit`.
    fn body_to_file(&mut self, rq: &mut Request, limit: u64, mode: u32) -> Result<(PathBuf, u64), Resp> {
        if rq.body_length().is_some_and(|n| n as u64 > limit) {
            return Err(too_large());
        }
        let tmp = self.tmp_path();
        let r = (|| -> io::Result<u64> {
            let mut f = OpenOptions::new().write(true).create_new(true).mode(mode).open(&tmp)?;
            let n = io::copy(&mut rq.as_reader().take(limit + 1), &mut f)?;
            f.sync_all()?;
            Ok(n)
        })();
        match r {
            Ok(n) if n <= limit => Ok((tmp, n)),
            Ok(_) => {
                let _ = fs::remove_file(&tmp);
                Err(too_large())
            }
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                Err(internal(format!("upload failed: {e}")))
            }
        }
    }

    fn check_space(&self, incoming: u64) -> Result<(), Resp> {
        if disk_free(&self.data).saturating_sub(incoming) < MIN_FREE {
            return Err(text(507, "insufficient storage"));
        }
        Ok(())
    }

    fn auth(&mut self, rq: &Request, method: &str, url: &str, body: &[u8]) -> Result<(), Resp> {
        let sig = header(rq, reqauth::HEADER).ok_or_else(|| text(401, "missing signature"))?;
        let t = now();
        let mut err = String::new();
        for key in &self.keys {
            let nonces = &mut self.nonces;
            let unused = |n: &str| nonces.insert(n.to_owned(), t).is_none();
            match reqauth::verify_request(&sig, key, method, url, body, t, unused) {
                Ok(()) => {
                    // Persisted so a restart does not reopen the replay window.
                    let nonces = self.nonces.clone();
                    self.save("nonces.json", &nonces).map_err(internal)?;
                    return Ok(());
                }
                // A mismatch under one key says nothing; keep the more specific reason.
                Err(e) if err.is_empty() || e != "signature does not match" => err = e,
                Err(_) => {}
            }
        }
        Err(text(401, &err))
    }

    fn handle(&mut self, mut rq: Request) {
        let resp = self.route(&mut rq).unwrap_or_else(|e| e);
        let _ = rq.respond(resp);
    }

    fn route(&mut self, rq: &mut Request) -> Result<Resp, Resp> {
        let url = rq.url().to_owned();
        let path = url.split('?').next().unwrap_or("");
        let method = rq.method().as_str().to_owned();
        match (method.as_str(), path) {
            ("GET", "/api/policy") => self.get_policy(rq),
            ("POST", "/api/heartbeat") => self.heartbeat(rq),
            ("POST", p) if p.starts_with("/api/diag/") => self.diag_upload(rq, &p["/api/diag/".len()..]),
            (m, p) if p.starts_with("/api/admin/") => self.admin(rq, m, &url, &p["/api/admin/".len()..]),
            _ => Err(not_found()),
        }
    }

    fn get_policy(&self, rq: &Request) -> Result<Resp, Resp> {
        let serial = self.policy_serial.ok_or_else(|| text(404, "no policy published"))?;
        let etag = format!("\"{serial}\"");
        if let Some(inm) = header(rq, "If-None-Match") {
            if inm.split(',').map(|t| t.trim().trim_start_matches("W/")).any(|t| t == etag || t == "*") {
                return Ok(empty(304).with_header(hdr("ETag", &etag)));
            }
        }
        let doc = fs::read(self.data.join("policy.json")).map_err(internal)?;
        Ok(Response::from_data(doc)
            .with_header(hdr("Content-Type", "application/json"))
            .with_header(hdr("ETag", &etag))
            .boxed())
    }

    fn heartbeat(&mut self, rq: &mut Request) -> Result<Resp, Resp> {
        let t = now();
        let hits = self.ip_hits.entry(client_ip(rq)).or_insert((t, 0));
        if t >= hits.0 + 60 {
            *hits = (t, 0);
        }
        hits.1 += 1;
        if hits.1 > HB_IP_PER_MIN {
            return Err(text(429, "too many requests"));
        }
        let body = read_body(rq, LIMIT_HEARTBEAT)?;
        let hb: Heartbeat = serde_json::from_slice(&body).map_err(|e| text(400, &e.to_string()))?;
        valid_heartbeat(&hb).map_err(|e| text(400, &e))?;
        if self.devices.get(&hb.uuid).is_some_and(|d| t < d.last_seen + HB_UUID_INTERVAL) {
            return Err(text(429, "too many requests"));
        }
        if !self.devices.contains_key(&hb.uuid) && self.devices.len() >= MAX_DEVICES {
            if let Some(oldest) = self.devices.values().min_by_key(|d| d.last_seen).map(|d| d.uuid.clone()) {
                self.devices.remove(&oldest);
            }
        }
        let ip = client_ip(rq);
        let d = Device { uuid: hb.uuid, id: hb.id, profile: hb.profile, version: hb.version, os: hb.os, serial: hb.serial, ip, last_seen: t };
        self.devices.insert(d.uuid.clone(), d);
        self.devices_dirty = true;
        Ok(empty(204))
    }

    fn diag_upload(&mut self, rq: &mut Request, ticket: &str) -> Result<Resp, Resp> {
        let t = now();
        let idx = self
            .tickets
            .iter()
            .position(|k| k.ticket == ticket && k.uploaded.is_none() && t < k.created + TICKET_OPEN)
            .ok_or_else(not_found)?;
        self.check_space(rq.body_length().map_or(LIMIT_DIAG, |n| n as u64))?;
        let (tmp, size) = self.body_to_file(rq, LIMIT_DIAG, 0o600)?;
        let dest = self.data.join("diag").join(format!("{ticket}.zip"));
        if let Err(e) = fs::rename(&tmp, dest) {
            let _ = fs::remove_file(&tmp);
            return Err(internal(e));
        }
        self.tickets[idx].uploaded = Some(t);
        self.tickets[idx].size = Some(size);
        let tickets = self.tickets.clone();
        self.save("diag/tickets.json", &tickets).map_err(internal)?;
        Ok(empty(204))
    }

    fn admin(&mut self, rq: &mut Request, method: &str, url: &str, rest: &str) -> Result<Resp, Resp> {
        if self.keys.is_empty() {
            return Err(text(503, "admin API disabled: ADMIN_PUB_KEY not set"));
        }
        if let Some(rel) = rest.strip_prefix("release/") {
            let dest = release_path(&self.data, rel)?;
            return match method {
                "PUT" => self.put_release(rq, url, &dest),
                "DELETE" => {
                    let body = read_body(rq, LIMIT_SMALL)?;
                    self.auth(rq, method, url, &body)?;
                    self.delete_release(&dest)
                }
                _ => Err(not_found()),
            };
        }
        let limit = match (method, rest) {
            ("PUT", "policy") => LIMIT_POLICY,
            ("PUT", "download-page") => LIMIT_PAGE,
            ("POST", "diag-ticket") => LIMIT_SMALL,
            ("GET", "policy-history" | "devices" | "diag" | "releases" | "status") => LIMIT_SMALL,
            ("GET", r) if r.starts_with("diag/") => LIMIT_SMALL,
            _ => return Err(not_found()),
        };
        let body = read_body(rq, limit)?;
        self.auth(rq, method, url, &body)?;
        match (method, rest) {
            ("PUT", "policy") => self.put_policy(&body),
            ("PUT", "download-page") => {
                self.check_space(body.len() as u64)?;
                let dest = self.data.join("download/index.html");
                self.write_atomic(&dest, &body, 0o640).map_err(internal)?;
                Ok(empty(204))
            }
            ("POST", "diag-ticket") => self.new_ticket(&body),
            ("GET", "policy-history") => self.policy_history(),
            ("GET", "devices") => {
                let mut v: Vec<&Device> = self.devices.values().collect();
                v.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
                Ok(json(&v))
            }
            ("GET", "diag") => Ok(json(&self.tickets)),
            ("GET", "releases") => self.list_releases(),
            ("GET", "status") => Ok(json(&serde_json::json!({
                "version": env!("CARGO_PKG_VERSION"),
                "disk_free": disk_free(&self.data),
                "devices": self.devices.len(),
                "policy_serial": self.policy_serial,
                "uptime": now().saturating_sub(self.started),
            }))),
            (_, r) => {
                let ticket = r.strip_prefix("diag/").unwrap_or("");
                if !self.tickets.iter().any(|k| k.ticket == ticket && k.uploaded.is_some()) {
                    return Err(not_found());
                }
                let f = File::open(self.data.join("diag").join(format!("{ticket}.zip"))).map_err(|_| not_found())?;
                Ok(Response::from_file(f).with_header(hdr("Content-Type", "application/zip")).boxed())
            }
        }
    }

    fn put_policy(&mut self, body: &[u8]) -> Result<Resp, Resp> {
        let payload = envelope::open_json(body, &self.keys).map_err(|e| text(400, &e))?;
        let bundle = Bundle::from_json(&payload).map_err(|e| text(400, &e))?;
        if self.policy_serial.is_some_and(|s| bundle.serial <= s) {
            return Err(text(409, "serial must be greater than the current policy's"));
        }
        self.check_space(body.len() as u64)?;
        let hist = self.data.join("policy-history");
        self.write_atomic(&hist.join(format!("{}.json", bundle.serial)), body, 0o600).map_err(internal)?;
        self.write_atomic(&self.data.join("policy.json"), body, 0o600).map_err(internal)?;
        self.policy_serial = Some(bundle.serial);
        let mut serials = history_serials(&hist);
        serials.sort_unstable_by(|a, b| b.cmp(a));
        for s in serials.iter().skip(HISTORY_KEEP) {
            let _ = fs::remove_file(hist.join(format!("{s}.json")));
        }
        Ok(empty(204))
    }

    fn policy_history(&self) -> Result<Resp, Resp> {
        let hist = self.data.join("policy-history");
        let mut out: Vec<_> = history_serials(&hist)
            .into_iter()
            .filter_map(|s| fs::read(hist.join(format!("{s}.json"))).ok().and_then(|b| stored_meta(&b)))
            .map(|m| serde_json::json!({"serial": m.serial, "issued_at": m.issued_at}))
            .collect();
        out.sort_by_key(|v| std::cmp::Reverse(v["serial"].as_u64()));
        Ok(json(&out))
    }

    fn new_ticket(&mut self, body: &[u8]) -> Result<Resp, Resp> {
        let req: TicketRequest = serde_json::from_slice(body).map_err(|e| text(400, &e.to_string()))?;
        if req.note.chars().count() > 256 {
            return Err(text(413, "note longer than 256 characters"));
        }
        let ticket = loop {
            let mut b = [0u8; 8];
            random_bytes(&mut b).map_err(internal)?;
            let t: String = b.iter().map(|x| TICKET_ALPHABET[(x & 31) as usize] as char).collect();
            if !self.tickets.iter().any(|k| k.ticket == t) {
                break t;
            }
        };
        let created = now();
        self.tickets.push(Ticket { ticket: ticket.clone(), note: req.note, created, uploaded: None, size: None });
        let tickets = self.tickets.clone();
        self.save("diag/tickets.json", &tickets).map_err(internal)?;
        Ok(json(&serde_json::json!({"ticket": ticket, "expires": created + TICKET_OPEN})))
    }

    fn put_release(&mut self, rq: &mut Request, url: &str, dest: &Path) -> Result<Resp, Resp> {
        if rq.body_length().is_some_and(|n| n as u64 > LIMIT_RELEASE) {
            return Err(too_large());
        }
        self.check_space(rq.body_length().map_or(LIMIT_RELEASE, |n| n as u64))?;
        let (tmp, _) = self.body_to_file(rq, LIMIT_RELEASE, 0o640)?;
        let r = with_mapped(&tmp, |body| self.auth(rq, "PUT", url, body))
            .map_err(internal)
            .and_then(|r| r)
            .and_then(|()| {
                mkdirs(dest.parent().expect("under download/"), 0o750).map_err(internal)?;
                fs::rename(&tmp, dest).map_err(internal)
            });
        if r.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        r.map(|()| empty(204))
    }

    fn delete_release(&self, dest: &Path) -> Result<Resp, Resp> {
        if !dest.is_file() {
            return Err(not_found());
        }
        fs::remove_file(dest).map_err(internal)?;
        let root = self.data.join("download");
        let mut dir = dest.parent();
        while let Some(d) = dir.filter(|d| *d != root) {
            if fs::remove_dir(d).is_err() {
                break; // not empty
            }
            dir = d.parent();
        }
        Ok(empty(204))
    }

    fn list_releases(&self) -> Result<Resp, Resp> {
        let root = self.data.join("download");
        let mut out = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            for e in fs::read_dir(&dir).map_err(internal)?.flatten() {
                let (p, Ok(meta)) = (e.path(), e.metadata()) else { continue };
                if meta.is_dir() {
                    stack.push(p);
                    continue;
                }
                let rel = p.strip_prefix(&root).unwrap_or(&p).to_string_lossy().into_owned();
                let Some((channel, path)) = rel.split_once('/') else { continue }; // index.html
                let modified = meta.modified().ok().and_then(|m| m.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
                out.push(serde_json::json!({"channel": channel, "path": path, "size": meta.len(), "modified": modified}));
            }
        }
        out.sort_by(|a, b| (a["channel"].as_str(), a["path"].as_str()).cmp(&(b["channel"].as_str(), b["path"].as_str())));
        Ok(json(&out))
    }

    /// Periodic housekeeping: expiry, rate-limit and nonce pruning, device flush.
    fn maintain(&mut self) {
        let t = now();
        let before = self.devices.len();
        self.devices.retain(|_, d| d.last_seen + DEVICE_TTL > t);
        self.devices_dirty |= self.devices.len() != before;
        self.ip_hits.retain(|_, (start, _)| t < *start + 60);
        let n = self.nonces.len();
        self.nonces.retain(|_, seen| t < *seen + reqauth::NONCE_TTL_SECS);
        if self.nonces.len() != n {
            let nonces = self.nonces.clone();
            let _ = self.save("nonces.json", &nonces).map_err(internal);
        }
        let diag = self.data.join("diag");
        let n = self.tickets.len();
        self.tickets.retain(|k| {
            let keep = t < k.uploaded.unwrap_or(k.created) + DIAG_KEEP;
            if !keep {
                let _ = fs::remove_file(diag.join(format!("{}.zip", k.ticket)));
            }
            keep
        });
        if self.tickets.len() != n {
            let tickets = self.tickets.clone();
            let _ = self.save("diag/tickets.json", &tickets).map_err(internal);
        }
        self.flush_devices();
    }

    fn flush_devices(&mut self) {
        if self.devices_dirty {
            let v: Vec<Device> = self.devices.values().cloned().collect();
            if self.save("devices.json", &v).map_err(internal).is_ok() {
                self.devices_dirty = false;
            }
        }
    }
}

fn history_serials(hist: &Path) -> Vec<u64> {
    fs::read_dir(hist)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| e.file_name().to_str()?.strip_suffix(".json")?.parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

fn main() {
    let mut data = PathBuf::from("/var/lib/tether-api");
    let mut listen = "127.0.0.1:21120".to_owned();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match (a.as_str(), args.next()) {
            ("--data", Some(v)) => data = v.into(),
            ("--listen", Some(v)) => listen = v,
            _ => {
                eprintln!("usage: tether-api [--data <dir>] [--listen <addr:port>]");
                std::process::exit(2);
            }
        }
    }
    let keys: Vec<VerifyingKey> = match std::env::var("ADMIN_PUB_KEY")
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.trim().is_empty())
        .map(tether_trust::key_from_b64)
        .collect()
    {
        Ok(k) => k,
        Err(e) => {
            eprintln!("ADMIN_PUB_KEY: {e}");
            std::process::exit(1);
        }
    };
    if keys.is_empty() {
        eprintln!("warning: ADMIN_PUB_KEY is empty; admin endpoints are disabled");
    }
    let mut app = App::open(data, keys).unwrap_or_else(|e| {
        eprintln!("data dir: {e}");
        std::process::exit(1);
    });
    let server = Server::http(&listen).unwrap_or_else(|e| {
        eprintln!("listen {listen}: {e}");
        std::process::exit(1);
    });
    match server.server_addr().to_ip() {
        Some(a) => println!("listening on {a}"),
        None => println!("listening"),
    }
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
    }
    let mut last_maint = now();
    app.maintain();
    while !STOP.load(Ordering::SeqCst) {
        match server.recv_timeout(Duration::from_secs(1)) {
            Ok(Some(rq)) => app.handle(rq),
            Ok(None) => {}
            Err(e) => eprintln!("accept: {e}"),
        }
        if now() >= last_maint + 60 {
            app.maintain();
            last_maint = now();
        }
    }
    app.flush_devices();
}

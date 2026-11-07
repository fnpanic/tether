//! Tether relay API. Wire contract: `docs/api.md`. Flat files under the data dir, no database.
//!
//! Layout: `policy.json`, `policy-history/<serial>.json`, `server-keys.json` (signed server key
//! list, docs/key-rotation.md), `devices.json`, `nonces.json`,
//! `diag/tickets.json` + `diag/<ticket>.zip`, `custom/<id>.json` (custom TetherQS apps),
//! `audit/<YYYY-MM-DD>.jsonl` (session audit log, `audit.rs`), `team/roster.json` + `team/tags.json`
//! (technician access, `team.rs`), `download/…` (served by Caddy), `tmp/`.

mod audit;
mod metrics;
mod team;

use std::collections::HashMap;
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::mem::ManuallyDrop;
use std::net::{SocketAddr, TcpStream};
use std::os::unix::io::FromRawFd;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tether_trust::{custom, envelope, policy::Bundle, reqauth, serverkeys::KeyList, VerifyingKey};
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
const LIMIT_SERVER_KEYS: u64 = 16 << 10;
const LIMIT_PAGE: u64 = 512 << 10;
const LIMIT_DIAG: u64 = 2 << 20;
const LIMIT_RELEASE: u64 = 200 << 20;
const LIMIT_CUSTOM: u64 = 300 << 10;
const CUSTOM_MAX_DAYS: u64 = 366;
const Q_IP_PER_MIN: u32 = 30;

/// Requests served at once; more get 503. Each holds at most one in-memory body (LIMIT_PAGE).
const MAX_WORKERS: usize = 32;
/// Longest wait for one read or write on a client connection.
const IO_TIMEOUT: Duration = Duration::from_secs(30);

static STOP: AtomicBool = AtomicBool::new(false);
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

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
    /// Serial of the server key list the device applied (absent: none).
    #[serde(default)]
    server_keys_serial: u64,
}

#[derive(Serialize, Deserialize, Clone)]
struct Device {
    uuid: String,
    id: String,
    profile: String,
    version: String,
    os: String,
    serial: u64,
    #[serde(default)]
    server_keys_serial: u64,
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
    /// Open this ticket ID instead of a random one: Tether Manage opens the ticket it got from
    /// the first relay on the others too, so the upload works on whichever relay the client uses.
    #[serde(default)]
    ticket: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CustomRequest {
    profile: serde_json::Value,
    token: String,
    expires: u64,
    channel: String,
}

/// `custom/<id>.json`. The signed profile is kept as sent; TetherQS verifies it itself.
#[derive(Serialize, Deserialize, Clone)]
struct CustomApp {
    id: String,
    name: String,
    token: String,
    expires: u64,
    channel: String,
    created: u64,
    downloads: u64,
    last_download: Option<u64>,
    profile: serde_json::Value,
}

/// Just the header fields of a stored bundle (history listing, startup).
#[derive(Deserialize)]
struct BundleMeta {
    serial: u64,
    issued_at: u64,
}

/// A request body, read before the state lock is taken: a slow client holds only its own worker.
enum Body {
    Mem(Vec<u8>),
    /// The large uploads (release, diagnostics), streamed to a temp file that is removed on drop
    /// unless the handler renamed it into place.
    File(PathBuf, u64),
}

impl Body {
    fn bytes(&self, limit: u64) -> Result<&[u8], Resp> {
        match self {
            Body::Mem(b) if b.len() as u64 <= limit => Ok(b),
            _ => Err(too_large()),
        }
    }

    fn file(&self) -> Result<(&Path, u64), Resp> {
        match self {
            Body::File(p, n) => Ok((p, *n)),
            Body::Mem(_) => Err(internal("upload not streamed to a file")),
        }
    }
}

impl Drop for Body {
    fn drop(&mut self) {
        if let Body::File(p, _) = self {
            let _ = fs::remove_file(p);
        }
    }
}

/// Fails a body read once its whole-body deadline has passed (IO_TIMEOUT bounds each read).
struct Deadline<R>(R, Instant);

impl<R: Read> Read for Deadline<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if Instant::now() > self.1 {
            return Err(io::ErrorKind::TimedOut.into());
        }
        self.0.read(buf)
    }
}

/// Reading `limit` bytes may take a minute plus 64 KiB/s (200 MiB: about 55 min).
fn body_deadline(limit: u64) -> Instant {
    Instant::now() + Duration::from_secs(60 + limit / (64 << 10))
}

/// tiny_http does not expose a request's socket: find it by its peer address among our fds to
/// set (or with `None` clear) its read and write timeouts.
fn set_io_timeouts(peer: SocketAddr, t: Option<Duration>) {
    let Ok(fds) = fs::read_dir("/proc/self/fd") else { return };
    for fd in fds.flatten().filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok()) {
        // Borrowed, never closed here; getpeername fails on anything that is not a TCP connection.
        let s = ManuallyDrop::new(unsafe { TcpStream::from_raw_fd(fd) });
        if s.peer_addr().is_ok_and(|a| a == peer) {
            let _ = s.set_read_timeout(t);
            let _ = s.set_write_timeout(t);
        }
    }
}

struct App {
    data: PathBuf,
    keys: Vec<VerifyingKey>,
    started: u64,
    devices: HashMap<String, Device>,
    devices_dirty: bool,
    ip_hits: HashMap<String, (u64, u32)>,
    q_hits: HashMap<String, (u64, u32)>,
    audit_ip_hits: HashMap<String, (u64, u32)>,
    audit_dev_hits: HashMap<String, (u64, u32)>,
    audit: audit::Log,
    custom: Vec<CustomApp>,
    nonces: HashMap<String, u64>,
    tickets: Vec<Ticket>,
    policy_serial: Option<u64>,
    server_keys_serial: Option<u64>,
    team: team::Team,
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
    Deadline(rq.as_reader(), body_deadline(limit))
        .take(limit + 1)
        .read_to_end(&mut buf)
        .map_err(|e| {
            metrics::body_error(&e);
            text(400, "could not read body")
        })?;
    if buf.len() as u64 > limit {
        return Err(too_large());
    }
    Ok(buf)
}

fn tmp_path(data: &Path) -> PathBuf {
    data.join("tmp").join(format!("{}-{}", std::process::id(), TMP_SEQ.fetch_add(1, Ordering::SeqCst) + 1))
}

fn check_space(data: &Path, incoming: u64) -> Result<(), Resp> {
    if disk_free(data).saturating_sub(incoming) < MIN_FREE {
        return Err(text(507, "insufficient storage"));
    }
    Ok(())
}

/// Streams the body to a new temp file (mode `mode`), enforcing `limit`.
fn body_to_file(rq: &mut Request, data: &Path, limit: u64, mode: u32) -> Result<Body, Resp> {
    if rq.body_length().is_some_and(|n| n as u64 > limit) {
        return Err(too_large());
    }
    check_space(data, rq.body_length().map_or(limit, |n| n as u64))?;
    let tmp = tmp_path(data);
    let r = (|| -> io::Result<u64> {
        let mut f = OpenOptions::new().write(true).create_new(true).mode(mode).open(&tmp)?;
        let n = io::copy(&mut Deadline(rq.as_reader(), body_deadline(limit)).take(limit + 1), &mut f)?;
        f.sync_all()?;
        Ok(n)
    })();
    let body = Body::File(tmp, *r.as_ref().unwrap_or(&0));
    match r {
        Ok(n) if n <= limit => Ok(body),
        Ok(_) => Err(too_large()),
        Err(e) => {
            metrics::body_error(&e);
            Err(internal(format!("upload failed: {e}")))
        }
    }
}

/// Reads the body a route takes, before any state is locked: the large uploads to a temp file,
/// everything else into memory up to the largest in-memory limit (LIMIT_PAGE); each handler
/// still applies its own limit.
fn read_request_body(rq: &mut Request, data: &Path) -> Result<Body, Resp> {
    let path = rq.url().split('?').next().unwrap_or("").to_owned();
    match rq.method().as_str() {
        "PUT" if path.starts_with("/api/admin/release/") => body_to_file(rq, data, LIMIT_RELEASE, 0o640),
        "POST" if path.starts_with("/api/diag/") => body_to_file(rq, data, LIMIT_DIAG, 0o600),
        _ => read_body(rq, LIMIT_PAGE).map(Body::Mem),
    }
}

fn lock(app: &Mutex<App>) -> MutexGuard<'_, App> {
    app.lock().unwrap_or_else(|e| e.into_inner()) // a panicked request leaves the state usable
}

/// One request on its own worker thread. Only routing holds the state lock; the body is read
/// before it and the response written after it, each read and write bounded by IO_TIMEOUT.
fn serve(app: &Mutex<App>, data: &Path, mut rq: Request) {
    let (start, endpoint) = (Instant::now(), metrics::endpoint(rq.method().as_str(), rq.url()));
    let peer = rq.remote_addr().copied();
    if endpoint == metrics::METRICS {
        let page = match peer {
            Some(p) if p.ip().is_loopback() => lock(app).metrics_now(),
            _ => None,
        };
        let _ = rq.respond(match page {
            Some(now) => text(200, &metrics::render(&now, data)).with_header(hdr("Content-Type", "text/plain; version=0.0.4")),
            None => not_found(),
        });
        return;
    }
    if let Some(p) = peer {
        set_io_timeouts(p, Some(IO_TIMEOUT));
    }
    let resp = match read_request_body(&mut rq, data) {
        Ok(body) => lock(app).route(&mut rq, &body).unwrap_or_else(|e| e),
        Err(e) => e,
    };
    let code = resp.status_code().0;
    let _ = rq.respond(resp);
    if let Some(p) = peer {
        set_io_timeouts(p, None); // an idle keep-alive connection from the proxy may wait
    }
    metrics::observe(endpoint, code, start.elapsed());
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

/// Count a request from `ip` in the current minute; false when over `limit`.
fn rate_ok(hits: &mut HashMap<String, (u64, u32)>, ip: String, t: u64, limit: u32) -> bool {
    let h = hits.entry(ip).or_insert((t, 0));
    if t >= h.0 + 60 {
        *h = (t, 0);
    }
    h.1 += 1;
    h.1 <= limit
}

fn valid_custom_id(s: &str) -> bool {
    (1..=40).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `<Name>-Support.exe`, name reduced to `[A-Za-z0-9 _.-]` (safe inside the quoted header value).
fn download_name(name: &str) -> String {
    let n: String = name.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '.' | '-')).collect();
    let n = n.trim_matches(|c| c == ' ' || c == '.');
    format!("{}-Support.exe", if n.is_empty() { "TetherQS" } else { n })
}

/// Log-safe token prefix (full tokens are never logged).
fn token_hint(t: &str) -> &str {
    &t[..t.len().min(6)]
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
    fn open(data: PathBuf, keys: Vec<VerifyingKey>, audit_days: u64, audit_max: u64) -> io::Result<Self> {
        mkdirs(&data, 0o750)?;
        mkdirs(&data.join("download"), 0o750)?;
        for d in ["policy-history", "diag", "custom", "audit", "tmp"] {
            mkdirs(&data.join(d), 0o700)?;
        }
        for e in fs::read_dir(data.join("tmp"))? {
            let _ = fs::remove_file(e?.path());
        }
        let devices: Vec<Device> = load_json(&data.join("devices.json"));
        let policy_serial = fs::read(data.join("policy.json")).ok().and_then(|b| stored_meta(&b)).map(|m| m.serial);
        let server_keys_serial = fs::read(data.join("server-keys.json")).ok().and_then(|b| stored_meta(&b)).map(|m| m.serial);
        let mut custom: Vec<CustomApp> = Vec::new();
        for e in fs::read_dir(data.join("custom"))?.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "json") {
                match fs::read(&p).map_err(|e| e.to_string()).and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string())) {
                    Ok(c) => custom.push(c),
                    Err(e) => eprintln!("warning: ignoring unreadable {}: {e}", p.display()),
                }
            }
        }
        Ok(App {
            q_hits: HashMap::new(),
            audit_ip_hits: HashMap::new(),
            audit_dev_hits: HashMap::new(),
            audit: audit::Log::open(data.join("audit"), audit_days, audit_max)?,
            team: team::Team::open(&data)?,
            custom,
            devices: devices.into_iter().map(|d| (d.uuid.clone(), d)).collect(),
            devices_dirty: false,
            ip_hits: HashMap::new(),
            nonces: load_json(&data.join("nonces.json")),
            tickets: load_json(&data.join("diag/tickets.json")),
            policy_serial,
            server_keys_serial,
            started: now(),
            keys,
            data,
        })
    }

    fn write_atomic(&mut self, dest: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
        let tmp = tmp_path(&self.data);
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

    fn check_space(&self, incoming: u64) -> Result<(), Resp> {
        check_space(&self.data, incoming)
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

    fn route(&mut self, rq: &mut Request, body: &Body) -> Result<Resp, Resp> {
        let url = rq.url().to_owned();
        let path = url.split('?').next().unwrap_or("");
        let method = rq.method().as_str().to_owned();
        match (method.as_str(), path) {
            ("GET", "/api/policy") => self.get_signed(rq, self.policy_serial, "policy.json", "no policy published"),
            ("GET", "/api/server-keys") => self.get_signed(rq, self.server_keys_serial, "server-keys.json", "no server key list published"),
            ("POST", "/api/heartbeat") => self.heartbeat(rq, body),
            ("POST", "/api/audit/conn") => self.audit_post(rq, body, "conn"),
            ("POST", "/api/audit/file") => self.audit_post(rq, body, "file"),
            ("POST", "/api/audit/alarm") => self.audit_post(rq, body, "alarm"),
            ("POST", p) if p.starts_with("/api/diag/") => self.diag_upload(body, &p["/api/diag/".len()..]),
            ("GET", p) if p.starts_with("/q/") => self.custom_download(rq, &p["/q/".len()..]),
            (m, p) if p.starts_with("/api/team/") => self.team_route(rq, body, m, &url, &p["/api/team/".len()..]),
            (m, p) if p.starts_with("/api/admin/") => self.admin(rq, body, m, &url, &p["/api/admin/".len()..]),
            _ => Err(not_found()),
        }
    }

    /// A stored signed document (policy, server key list) with `ETag: "<serial>"`.
    fn get_signed(&self, rq: &Request, serial: Option<u64>, file: &str, missing: &str) -> Result<Resp, Resp> {
        let serial = serial.ok_or_else(|| text(404, missing))?;
        let etag = format!("\"{serial}\"");
        if let Some(inm) = header(rq, "If-None-Match") {
            if inm.split(',').map(|t| t.trim().trim_start_matches("W/")).any(|t| t == etag || t == "*") {
                return Ok(empty(304).with_header(hdr("ETag", &etag)));
            }
        }
        let doc = fs::read(self.data.join(file)).map_err(internal)?;
        Ok(Response::from_data(doc)
            .with_header(hdr("Content-Type", "application/json"))
            .with_header(hdr("ETag", &etag))
            .boxed())
    }

    fn heartbeat(&mut self, rq: &mut Request, body: &Body) -> Result<Resp, Resp> {
        let t = now();
        if !rate_ok(&mut self.ip_hits, client_ip(rq), t, HB_IP_PER_MIN) {
            return Err(text(429, "too many requests"));
        }
        let body = body.bytes(LIMIT_HEARTBEAT)?;
        let hb: Heartbeat = serde_json::from_slice(body).map_err(|e| text(400, &e.to_string()))?;
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
        let d = Device {
            uuid: hb.uuid,
            id: hb.id,
            profile: hb.profile,
            version: hb.version,
            os: hb.os,
            serial: hb.serial,
            server_keys_serial: hb.server_keys_serial,
            ip,
            last_seen: t,
        };
        self.devices.insert(d.uuid.clone(), d);
        self.devices_dirty = true;
        Ok(empty(204))
    }

    /// `POST /api/audit/<kind>` from a host (unauthenticated, see audit.rs). Success is an empty
    /// 2xx body: the client retries anything else except a 4xx other than 408/429.
    fn audit_post(&mut self, rq: &mut Request, body: &Body, kind: &str) -> Result<Resp, Resp> {
        let t = now();
        let src = client_ip(rq);
        if !rate_ok(&mut self.audit_ip_hits, src.clone(), t, audit::IP_PER_MIN) {
            return Err(text(429, "too many requests"));
        }
        let body = body.bytes(audit::LIMIT)?;
        let (uuid, nonce, mut rec) = audit::parse(kind, body).map_err(|e| text(400, &e))?;
        if !rate_ok(&mut self.audit_dev_hits, uuid, t, audit::DEVICE_PER_MIN) {
            return Err(text(429, "too many requests"));
        }
        if self.audit.seen(&nonce) {
            return Ok(empty(204));
        }
        self.check_space(audit::LIMIT)?;
        rec.insert("ts".into(), t.into());
        rec.insert("src".into(), src.into());
        match self.audit.append(nonce, &rec, t) {
            Ok(()) => Ok(empty(204)),
            Err(None) => Err(text(507, "audit log full")),
            Err(Some(e)) => Err(internal(format!("audit log: {e}"))),
        }
    }

    /// `GET /api/admin/audit?since=<unix>&limit=<n>&device=<id or uuid>`.
    fn audit_read(&self, url: &str) -> Result<Resp, Resp> {
        let (mut since, mut limit, mut device) = (0, 1000, String::new());
        for kv in url.split_once('?').map_or("", |(_, q)| q).split('&').filter(|s| !s.is_empty()) {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            let bad = || text(400, &format!("bad {k}"));
            match k {
                "since" => since = v.parse().map_err(|_| bad())?,
                "limit" => limit = v.parse::<usize>().ok().filter(|n| (1..=audit::READ_MAX).contains(n)).ok_or_else(bad)?,
                "device" => device = audit::unescape(v),
                _ => return Err(text(400, &format!("unknown parameter {k}"))),
            }
        }
        Ok(json(&self.audit.read(since, limit, &device)))
    }

    fn diag_upload(&mut self, body: &Body, ticket: &str) -> Result<Resp, Resp> {
        let t = now();
        let idx = self
            .tickets
            .iter()
            .position(|k| k.ticket == ticket && k.uploaded.is_none() && t < k.created + TICKET_OPEN)
            .ok_or_else(not_found)?;
        let (tmp, size) = body.file()?;
        fs::rename(tmp, self.data.join("diag").join(format!("{ticket}.zip"))).map_err(internal)?;
        self.tickets[idx].uploaded = Some(t);
        self.tickets[idx].size = Some(size);
        let tickets = self.tickets.clone();
        self.save("diag/tickets.json", &tickets).map_err(internal)?;
        Ok(empty(204))
    }

    fn admin(&mut self, rq: &mut Request, body: &Body, method: &str, url: &str, rest: &str) -> Result<Resp, Resp> {
        if self.keys.is_empty() {
            return Err(text(503, "admin API disabled: ADMIN_PUB_KEY not set"));
        }
        if let Some(rel) = rest.strip_prefix("release/") {
            let dest = release_path(&self.data, rel)?;
            return match method {
                "PUT" => self.put_release(rq, body, url, &dest),
                "DELETE" => {
                    self.auth(rq, method, url, body.bytes(LIMIT_SMALL)?)?;
                    self.delete_release(&dest)
                }
                _ => Err(not_found()),
            };
        }
        let limit = match (method, rest) {
            ("PUT", "policy") => LIMIT_POLICY,
            ("PUT", "server-keys") => LIMIT_SERVER_KEYS,
            ("PUT", "download-page") => LIMIT_PAGE,
            ("PUT", "team") => team::LIMIT_ROSTER,
            ("POST", "diag-ticket") => LIMIT_SMALL,
            ("PUT", r) if r.starts_with("custom/") => LIMIT_CUSTOM,
            ("DELETE", r) if r.starts_with("custom/") => LIMIT_SMALL,
            ("GET", "policy-history" | "devices" | "diag" | "releases" | "status" | "custom" | "audit" | "team") => LIMIT_SMALL,
            ("GET", r) if r.starts_with("diag/") || r.starts_with("policy-history/") => LIMIT_SMALL,
            _ => return Err(not_found()),
        };
        let body = body.bytes(limit)?;
        self.auth(rq, method, url, body)?;
        match (method, rest) {
            ("PUT", "policy") => self.put_policy(body),
            ("PUT", "server-keys") => self.put_server_keys(body),
            ("PUT", "team") => self.put_team(body),
            ("GET", "team") => self.get_team(),
            ("PUT", "download-page") => {
                self.check_space(body.len() as u64)?;
                let dest = self.data.join("download/index.html");
                self.write_atomic(&dest, body, 0o640).map_err(internal)?;
                Ok(empty(204))
            }
            ("POST", "diag-ticket") => self.new_ticket(body),
            ("PUT", r) if r.starts_with("custom/") => self.put_custom(&r["custom/".len()..], body),
            ("DELETE", r) if r.starts_with("custom/") => self.delete_custom(&r["custom/".len()..]),
            ("GET", "custom") => {
                let list: Vec<_> = self
                    .custom
                    .iter()
                    .map(|c| {
                        serde_json::json!({"id": c.id, "name": c.name, "token": c.token, "expires": c.expires, "channel": c.channel,
                            "created": c.created, "downloads": c.downloads, "last_download": c.last_download})
                    })
                    .collect();
                Ok(json(&list))
            }
            ("GET", "policy-history") => self.policy_history(),
            ("GET", "devices") => {
                let mut v: Vec<&Device> = self.devices.values().collect();
                v.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
                Ok(json(&v))
            }
            ("GET", "diag") => Ok(json(&self.tickets)),
            ("GET", "audit") => self.audit_read(url),
            ("GET", "releases") => self.list_releases(),
            ("GET", r) if r.starts_with("policy-history/") => {
                // The signed document exactly as published, so another admin PC can restore it.
                let serial: u64 = r["policy-history/".len()..].parse().map_err(|_| not_found())?;
                let doc = fs::read(self.data.join("policy-history").join(format!("{serial}.json"))).map_err(|_| not_found())?;
                Ok(Response::from_data(doc).with_header(hdr("Content-Type", "application/json")).boxed())
            }
            ("GET", "status") => Ok(json(&serde_json::json!({
                "version": env!("CARGO_PKG_VERSION"),
                "disk_free": disk_free(&self.data),
                "devices": self.devices.len(),
                "policy_serial": self.policy_serial,
                "server_keys_serial": self.server_keys_serial,
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

    /// The server key list devices trust (docs/key-rotation.md). Verified and validated like a
    /// policy; serials only go up.
    fn put_server_keys(&mut self, body: &[u8]) -> Result<Resp, Resp> {
        let list = KeyList::open(body, &self.keys).map_err(|e| text(400, &e))?;
        if self.server_keys_serial.is_some_and(|s| list.serial <= s) {
            return Err(text(409, "serial must be greater than the current server key list's"));
        }
        self.check_space(body.len() as u64)?;
        self.write_atomic(&self.data.join("server-keys.json"), body, 0o600).map_err(internal)?;
        self.server_keys_serial = Some(list.serial);
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
        let ticket = match req.ticket {
            Some(t) => {
                if t.len() != 8 || !t.bytes().all(|c| TICKET_ALPHABET.contains(&c)) {
                    return Err(text(400, "ticket must be 8 characters of the ticket alphabet"));
                }
                if self.tickets.iter().any(|k| k.ticket == t) {
                    return Err(text(409, "ticket exists"));
                }
                t
            }
            None => loop {
                let mut b = [0u8; 8];
                random_bytes(&mut b).map_err(internal)?;
                let t: String = b.iter().map(|x| TICKET_ALPHABET[(x & 31) as usize] as char).collect();
                if !self.tickets.iter().any(|k| k.ticket == t) {
                    break t;
                }
            },
        };
        let created = now();
        self.tickets.push(Ticket { ticket: ticket.clone(), note: req.note, created, uploaded: None, size: None });
        let tickets = self.tickets.clone();
        self.save("diag/tickets.json", &tickets).map_err(internal)?;
        Ok(json(&serde_json::json!({"ticket": ticket, "expires": created + TICKET_OPEN})))
    }

    fn put_custom(&mut self, id: &str, body: &[u8]) -> Result<Resp, Resp> {
        let req: CustomRequest = serde_json::from_slice(body).map_err(|e| text(400, &e.to_string()))?;
        let doc = serde_json::to_vec(&req.profile).map_err(internal)?;
        let p = custom::open(&doc, &self.keys).map_err(|e| text(400, &e))?;
        if p.id != id || !valid_custom_id(id) {
            return Err(text(400, "profile id does not match the path"));
        }
        if !custom::valid_token(&req.token) {
            return Err(text(400, "token must be 43 characters base64url"));
        }
        let t = now();
        if req.expires <= t || req.expires > t + CUSTOM_MAX_DAYS * DAY {
            return Err(text(400, "expires must be in the future, at most 366 days ahead"));
        }
        if !valid_channel(&req.channel) {
            return Err(text(400, "invalid channel"));
        }
        if self.custom.iter().any(|c| c.token == req.token && c.id != id) {
            return Err(text(409, "token is used by another custom app"));
        }
        let old = self.custom.iter().position(|c| c.id == id);
        // The same link (token) keeps its counters; a new token is a new link.
        let (created, downloads, last_download) = match old.map(|i| &self.custom[i]) {
            Some(o) if o.token == req.token => (o.created, o.downloads, o.last_download),
            _ => (t, 0, None),
        };
        let entry = CustomApp { id: id.to_owned(), name: p.name, token: req.token, expires: req.expires, channel: req.channel, created, downloads, last_download, profile: req.profile };
        self.check_space(body.len() as u64)?;
        self.save(&format!("custom/{id}.json"), &entry).map_err(internal)?;
        match old {
            Some(i) => self.custom[i] = entry,
            None => self.custom.push(entry),
        }
        Ok(empty(204))
    }

    fn delete_custom(&mut self, id: &str) -> Result<Resp, Resp> {
        let i = self.custom.iter().position(|c| c.id == id && valid_custom_id(id)).ok_or_else(not_found)?;
        fs::remove_file(self.data.join("custom").join(format!("{id}.json"))).map_err(internal)?;
        self.custom.remove(i);
        Ok(empty(204))
    }

    /// `GET /q/<token>`: the channel's current TetherQS.exe plus the signed profile trailer.
    /// Unknown, revoked and expired tokens get the same 404.
    fn custom_download(&mut self, rq: &Request, token: &str) -> Result<Resp, Resp> {
        let t = now();
        if !rate_ok(&mut self.q_hits, client_ip(rq), t, Q_IP_PER_MIN) {
            return Err(text(429, "too many requests"));
        }
        if !custom::valid_token(token) {
            return Err(not_found());
        }
        let i = self.custom.iter().position(|c| c.token == token && t < c.expires).ok_or_else(not_found)?;
        let c = &self.custom[i];
        let dir = self.data.join("download").join(&c.channel).join("customer/windows-x86_64");
        let exe = (|| {
            let m: serde_json::Value = serde_json::from_slice(&fs::read(dir.join("latest.json")).ok()?).ok()?;
            let file = m["platforms"]["windows-x86_64"]["url"].as_str()?.rsplit('/').next().filter(|f| valid_segment(f))?;
            let f = File::open(dir.join(file)).ok()?;
            let len = f.metadata().ok().filter(|m| m.is_file())?.len();
            Some((f, len))
        })();
        let Some((f, len)) = exe else {
            eprintln!("custom app {} (token {}…): no Windows TetherQS release in channel {}", c.id, token_hint(token), c.channel);
            return Err(not_found());
        };
        let trailer = custom::trailer(&serde_json::to_vec(&c.profile).map_err(internal)?);
        let size = len as usize + trailer.len();
        let disposition = format!("attachment; filename=\"{}\"", download_name(&c.name));
        let body: Box<dyn Read + Send> = Box::new(f.take(len).chain(io::Cursor::new(trailer)));
        let resp = Response::new(
            tiny_http::StatusCode(200),
            vec![
                hdr("Content-Type", "application/vnd.microsoft.portable-executable"),
                hdr("Content-Disposition", &disposition),
                hdr("Cache-Control", "no-store"),
                hdr("X-Content-Type-Options", "nosniff"),
            ],
            body,
            Some(size),
            None,
        )
        .with_chunked_threshold(usize::MAX); // tiny_http would switch to chunked above 32 KiB
        let c = &mut self.custom[i];
        c.downloads += 1;
        c.last_download = Some(t);
        let entry = c.clone();
        if let Err(e) = self.save(&format!("custom/{}.json", entry.id), &entry) {
            eprintln!("custom app {}: could not save the download count: {e}", entry.id);
        }
        Ok(resp)
    }

    fn put_release(&mut self, rq: &mut Request, body: &Body, url: &str, dest: &Path) -> Result<Resp, Resp> {
        let (tmp, _) = body.file()?;
        with_mapped(tmp, |body| self.auth(rq, "PUT", url, body)).map_err(internal)??;
        mkdirs(dest.parent().expect("under download/"), 0o750).map_err(internal)?;
        fs::rename(tmp, dest).map_err(internal)?;
        Ok(empty(204))
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

    /// Values for `/metrics` (`metrics.rs`).
    fn metrics_now(&self) -> Option<metrics::Now> {
        let t = now();
        Some(metrics::Now {
            max_workers: MAX_WORKERS,
            devices: self.devices.len(),
            devices_24h: self.devices.values().filter(|d| d.last_seen + DAY > t).count(),
        })
    }

    /// Periodic housekeeping: expiry, rate-limit and nonce pruning, device flush.
    fn maintain(&mut self) {
        let t = now();
        let before = self.devices.len();
        self.devices.retain(|_, d| d.last_seen + DEVICE_TTL > t);
        self.devices_dirty |= self.devices.len() != before;
        self.ip_hits.retain(|_, (start, _)| t < *start + 60);
        self.q_hits.retain(|_, (start, _)| t < *start + 60);
        self.audit_ip_hits.retain(|_, (start, _)| t < *start + 60);
        self.audit_dev_hits.retain(|_, (start, _)| t < *start + 60);
        self.audit.prune(t, 0);
        self.team.maintain(t);
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
    // AUDIT_RETENTION_DAYS (default 90, 0 = do not store) and AUDIT_MAX_MB (default 50), api.env.
    let env_num = |k: &str, d: u64| match std::env::var(k) {
        Ok(v) if !v.trim().is_empty() => v.trim().parse().unwrap_or_else(|_| {
            eprintln!("{k}: not a number");
            std::process::exit(1);
        }),
        _ => d,
    };
    let (audit_days, audit_max) = (env_num("AUDIT_RETENTION_DAYS", 90), env_num("AUDIT_MAX_MB", 50).saturating_mul(1 << 20));
    let mut app = App::open(data.clone(), keys, audit_days, audit_max).unwrap_or_else(|e| {
        eprintln!("data dir: {e}");
        std::process::exit(1);
    });
    let server = Server::http(&listen).unwrap_or_else(|e| {
        eprintln!("listen {listen}: {e}");
        std::process::exit(1);
    });
    app.maintain(); // before announcing readiness: retention applies from the first request
    match server.server_addr().to_ip() {
        Some(a) => println!("listening on {a}"),
        None => println!("listening"),
    }
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
    }
    let app = Arc::new(Mutex::new(app));
    let data = Arc::new(data);
    let busy = Arc::new(AtomicUsize::new(0));
    let mut last_maint = now();
    while !STOP.load(Ordering::SeqCst) {
        match server.recv_timeout(Duration::from_secs(1)) {
            Ok(Some(rq)) if busy.load(Ordering::SeqCst) >= MAX_WORKERS => {
                let _ = rq.respond(text(503, "busy"));
                metrics::BUSY_REJECTED.fetch_add(1, Ordering::Relaxed);
            }
            Ok(Some(rq)) => {
                busy.fetch_add(1, Ordering::SeqCst);
                let (app, data, busy) = (app.clone(), data.clone(), busy.clone());
                std::thread::spawn(move || {
                    metrics::ACTIVE.fetch_add(1, Ordering::Relaxed);
                    serve(&app, &data, rq);
                    metrics::ACTIVE.fetch_sub(1, Ordering::Relaxed);
                    busy.fetch_sub(1, Ordering::SeqCst);
                });
            }
            Ok(None) => {}
            Err(e) => eprintln!("accept: {e}"),
        }
        if now() >= last_maint + 60 {
            lock(&app).maintain();
            last_maint = now();
        }
    }
    // Let running requests finish (each read and write is bounded by IO_TIMEOUT).
    let t = Instant::now();
    while busy.load(Ordering::SeqCst) > 0 && t.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(50));
    }
    lock(&app).flush_devices();
}

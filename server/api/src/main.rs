//! Tether relay API. Wire contract: `docs/api.md`. Flat files under the data dir, no database.
//!
//! Layout: `policy.json`, `policy-history/<serial>.json`, `server-keys.json` (signed server key
//! list, docs/key-rotation.md), `devices.json`, `nonces.json`,
//! `diag/tickets.json` + `diag/<ticket>.zip`, `custom/<id>.json` (custom TetherQS apps),
//! `audit/<YYYY-MM-DD>.jsonl` (session audit log, `audit.rs`), `team/roster.json` + `team/tags.json`
//! (technician access, `team.rs`), `download/…` (served by Caddy; `download/<channel>/releases.json`,
//! the signed release index, by this server with `ETag: "<serial>"`), `release-generation` (the
//! release generation G, bumped on every index or manifest change, read by hbbs for its push hint;
//! docs/update-design.md), `tmp/`.

mod audit;
mod maczip;
mod metrics;
mod prune;
mod team;

use std::collections::HashMap;
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Seek, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::mem::ManuallyDrop;
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::os::unix::io::FromRawFd;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tether_trust::{custom, envelope, policy::Bundle, release, reqauth, serverkeys::KeyList, VerifyingKey};
use tiny_http::{Header, Request, Response, Server};

const DAY: u64 = 86_400;
/// Devices kept (oldest seen dropped first). 10 000 users plus reinstalls within DEVICE_TTL need
/// headroom; 50 000 is ~7 MB of devices.json and ~30 MB of memory, 40 MB at the peak while
/// loading (measured, docs/scaling.md; MemoryMax=64M).
const MAX_DEVICES: usize = 50_000;
const DEVICE_TTL: u64 = 180 * DAY;
const HB_UUID_INTERVAL: u64 = 60;
const HB_IP_PER_MIN: u32 = 30;
const HISTORY_KEEP: usize = 50;
const TICKET_OPEN: u64 = DAY;
const DIAG_KEEP: u64 = 7 * DAY;
/// Closed (archived) tickets, with their upload, are kept this long after closing.
const DIAG_ARCHIVE_KEEP: u64 = 90 * DAY;
const MIN_FREE: u64 = 2 << 30;
/// 404 body for a ticket this relay has but that takes no upload (docs/api.md).
const TICKET_NOT_OPEN: &str = "ticket not open";
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
/// Large uploads (release, diagnostics) read at once; more get 503 with Retry-After, so slow
/// uploads never hold more than these of the MAX_WORKERS (heartbeats keep the rest).
const MAX_UPLOADS: usize = 4;
/// A body read that averages under MIN_RATE bytes/s once MIN_RATE_AFTER has passed is aborted.
const MIN_RATE: u64 = 16 << 10;
const MIN_RATE_AFTER: Duration = Duration::from_secs(30);

static STOP: AtomicBool = AtomicBool::new(false);
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);
static UPLOADS: AtomicUsize = AtomicUsize::new(0);

type Resp = Response<Box<dyn Read + Send>>;
type Later = Box<dyn FnOnce(&Mutex<App>) -> Result<Resp, Resp> + Send>;

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
    /// Release index serial the device accepted (0: none). Sent only to relays that answered a
    /// heartbeat with `{"releases": G}` (older relays reject unknown fields).
    #[serde(default)]
    release_serial: u64,
    /// One of [`UPDATE_STATES`].
    #[serde(default)]
    update_state: String,
    /// The version being downloaded / staged / waited for.
    #[serde(default)]
    update_version: String,
}

/// `update_state` values (docs/api.md "Heartbeat").
const UPDATE_STATES: [&str; 9] = ["", "idle", "waiting-rollout", "downloading", "staged", "waiting-idle", "failed", "stale", "halted"];

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
    #[serde(default)]
    release_serial: u64,
    #[serde(default)]
    update_state: String,
    #[serde(default)]
    update_version: String,
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
    /// Closed by an admin: archived, no more uploads (docs/api.md "Diagnostics tickets").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    closed: Option<u64>,
}

impl Ticket {
    /// open → uploaded → closed; open → expired (24 h without an upload) → closed or removed.
    fn state(&self, t: u64) -> &'static str {
        match (self.closed, self.uploaded) {
            (Some(_), _) => "closed",
            (None, Some(_)) => "uploaded",
            (None, None) if t < self.created + TICKET_OPEN => "open",
            _ => "expired",
        }
    }

    /// Removed (with its zip) after this time.
    fn keep_until(&self) -> u64 {
        match self.closed {
            Some(c) => c + DIAG_ARCHIVE_KEEP,
            None => self.uploaded.unwrap_or(self.created) + DIAG_KEEP,
        }
    }
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

/// A custom app as kept in memory (and listed): everything but its profile, which is up to
/// LIMIT_CUSTOM and read from `custom/<id>.json` when a download or the macOS app needs it.
#[derive(Serialize, Deserialize, Clone)]
struct CustomMeta {
    id: String,
    name: String,
    token: String,
    expires: u64,
    channel: String,
    created: u64,
    downloads: u64,
    last_download: Option<u64>,
}

/// `custom/<id>.json`. The signed profile is kept as sent; TetherQS verifies it itself.
#[derive(Serialize, Deserialize)]
struct CustomApp {
    #[serde(flatten)]
    meta: CustomMeta,
    profile: serde_json::Value,
}

fn read_custom(data: &Path, id: &str) -> io::Result<CustomApp> {
    serde_json::from_slice(&fs::read(data.join("custom").join(format!("{id}.json")))?).map_err(io::Error::other)
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
    /// unless the handler renamed it into place. `nonce`: of the admin signature already
    /// verified over the file (release uploads), still to be checked for replay.
    File { path: PathBuf, len: u64, nonce: Option<String> },
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
            Body::File { path, len, .. } => Ok((path, *len)),
            Body::Mem(_) => Err(internal("upload not streamed to a file")),
        }
    }
}

impl Drop for Body {
    fn drop(&mut self) {
        if let Body::File { path, .. } = self {
            let _ = fs::remove_file(path);
        }
    }
}

/// Fails a body read once its whole-body deadline has passed (a minute plus 64 KiB/s of the
/// limit; 200 MiB: about 55 min) or, after MIN_RATE_AFTER, while it averages under MIN_RATE: a
/// trickle would otherwise hold its worker until the deadline. IO_TIMEOUT bounds each read.
struct Deadline<R> {
    r: R,
    start: Instant,
    end: Instant,
    n: u64,
}

impl<R: Read> Deadline<R> {
    fn new(r: R, limit: u64) -> Self {
        let start = Instant::now();
        Deadline { r, start, end: start + Duration::from_secs(60 + limit / (64 << 10)), n: 0 }
    }
}

impl<R: Read> Read for Deadline<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let took = self.start.elapsed();
        if Instant::now() > self.end || (took > MIN_RATE_AFTER && self.n < MIN_RATE * took.as_secs()) {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let n = self.r.read(buf)?;
        self.n += n as u64;
        Ok(n)
    }
}

/// One of the MAX_UPLOADS upload slots, released on drop.
struct UploadSlot;

impl UploadSlot {
    fn take() -> Option<Self> {
        if UPLOADS.fetch_add(1, Ordering::SeqCst) < MAX_UPLOADS {
            return Some(UploadSlot);
        }
        UPLOADS.fetch_sub(1, Ordering::SeqCst);
        None
    }
}

impl Drop for UploadSlot {
    fn drop(&mut self) {
        UPLOADS.fetch_sub(1, Ordering::SeqCst);
    }
}

/// tiny_http does not expose a request's socket: find it by its peer address among our fds.
fn with_socket(peer: SocketAddr, f: impl Fn(&TcpStream)) {
    let Ok(fds) = fs::read_dir("/proc/self/fd") else { return };
    for fd in fds.flatten().filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok()) {
        // Borrowed, never closed here; getpeername fails on anything that is not a TCP connection.
        let s = ManuallyDrop::new(unsafe { TcpStream::from_raw_fd(fd) });
        if s.peer_addr().is_ok_and(|a| a == peer) {
            f(&s);
        }
    }
}

/// Sets (or with `None` clears) the read and write timeouts of a request's socket.
fn set_io_timeouts(peer: SocketAddr, t: Option<Duration>) {
    with_socket(peer, |s| {
        let _ = s.set_read_timeout(t);
        let _ = s.set_write_timeout(t);
    });
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
    custom: Vec<CustomMeta>,
    /// Set by a handler (`App::defer`) whose file work runs after the state lock is released.
    later: Option<Later>,
    nonces: HashMap<String, u64>,
    tickets: Vec<Ticket>,
    policy_serial: Option<u64>,
    server_keys_serial: Option<u64>,
    team: team::Team,
    /// Release generation G (`release-generation`).
    release_gen: u64,
    /// Per channel: serial and expiry of the stored `releases.json`.
    indexes: HashMap<String, (u64, u64)>,
    /// The same for `releases-v2.json` (release::INDEX_V2_FILE: with xz copies, for new builds).
    indexes_v2: HashMap<String, (u64, u64)>,
    /// Devices that reported a newly staged update, by (profile, version), since start.
    downloads: std::collections::BTreeMap<(String, String), u64>,
}

/// `/download/<channel>/releases.json` (or `releases-v2.json`) → (channel, file name).
fn release_index_channel(p: &str) -> Option<(&str, &str)> {
    let (ch, rest) = p.strip_prefix("/download/")?.split_once('/')?;
    (is_index_file(rest) && valid_channel(ch)).then_some((ch, rest))
}

fn is_index_file(name: &str) -> bool {
    name == release::INDEX_FILE || name == release::INDEX_V2_FILE
}

/// Distinct (profile, version) series in the download counter; heartbeats are unauthenticated.
const MAX_DOWNLOAD_SERIES: usize = 64;

/// Serial and expiry of a stored release index (verified against the key it names: it was
/// checked against the trusted keys on upload).
fn index_meta(raw: &[u8]) -> Option<(u64, u64)> {
    let doc: envelope::Signed = serde_json::from_slice(raw).ok()?;
    let key = tether_trust::key_from_b64(&doc.key).ok()?;
    let i = release::Index::from_json(&envelope::open(&doc, &[key]).ok()?).ok()?;
    Some((i.serial, i.expires))
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

fn admin_disabled() -> Resp {
    text(503, "admin API disabled: ADMIN_PUB_KEY not set")
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
    Deadline::new(rq.as_reader(), limit)
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
    let expect = rq.body_length().map(|n| n as u64);
    let tmp = tmp_path(data);
    let r = (|| -> io::Result<u64> {
        let mut f = OpenOptions::new().write(true).create_new(true).mode(mode).open(&tmp)?;
        let n = io::copy(&mut Deadline::new(rq.as_reader(), limit).take(limit + 1), &mut f)?;
        f.sync_all()?;
        Ok(n)
    })();
    let body = Body::File { path: tmp, len: *r.as_ref().unwrap_or(&0), nonce: None };
    match r {
        // A client gone before its Content-Length arrived: not an upload (a diagnostics ticket
        // would be used up by a truncated zip).
        Ok(n) if expect.is_some_and(|e| e != n) => Err(text(400, "request body incomplete")),
        Ok(n) if n <= limit => Ok(body),
        Ok(_) => Err(too_large()),
        Err(e) => {
            metrics::body_error(&e);
            match e.kind() {
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => Err(text(408, "request body too slow")),
                _ => Err(internal(format!("upload failed: {e}"))),
            }
        }
    }
}

/// The admin signature of a request under any of `keys`. Needs no state, so a large body is
/// checked before the state lock; the returned nonce still goes through `App::use_nonce`.
fn verify_sig(keys: &[VerifyingKey], rq: &Request, method: &str, url: &str, body: &[u8], t: u64) -> Result<String, Resp> {
    let sig = header(rq, reqauth::HEADER).ok_or_else(|| text(401, "missing signature"))?;
    let mut err = String::new();
    for key in keys {
        let mut nonce = String::new();
        match reqauth::verify_request(&sig, key, method, url, body, t, |n| {
            nonce = n.to_owned();
            true
        }) {
            Ok(()) => return Ok(nonce),
            // A mismatch under one key says nothing; keep the more specific reason.
            Err(e) if err.is_empty() || e != "signature does not match" => err = e,
            Err(_) => {}
        }
    }
    Err(text(401, &err))
}

/// Reads the body a route takes, before any state is locked: the large uploads to a temp file,
/// everything else into memory up to the largest in-memory limit (LIMIT_PAGE); each handler
/// still applies its own limit. An upload is first checked as far as it can be without its body
/// and needs one of the MAX_UPLOADS slots; a release upload's signature is verified here.
fn read_request_body(app: &Mutex<App>, rq: &mut Request, data: &Path) -> Result<Body, Resp> {
    let url = rq.url().to_owned();
    let path = url.split('?').next().unwrap_or("");
    let (limit, mode, keys) = match (rq.method().as_str(), path) {
        ("PUT", p) if p.starts_with("/api/admin/release/") => (LIMIT_RELEASE, 0o640, Some(lock(app).release_precheck(rq, p)?)),
        ("POST", p) if p.starts_with("/api/diag/") => {
            lock(app).open_ticket(&p["/api/diag/".len()..], now())?;
            (LIMIT_DIAG, 0o600, None)
        }
        _ => return read_body(rq, LIMIT_PAGE).map(Body::Mem),
    };
    let _slot = UploadSlot::take().ok_or_else(|| text(503, "too many uploads at once, retry later").with_header(hdr("Retry-After", "30")))?;
    let mut body = body_to_file(rq, data, limit, mode)?;
    if let (Some(keys), Body::File { path, nonce, .. }) = (keys, &mut body) {
        *nonce = Some(with_mapped(path, |b| verify_sig(&keys, rq, "PUT", &url, b, now())).map_err(internal)??);
    }
    Ok(body)
}

fn lock(app: &Mutex<App>) -> MutexGuard<'_, App> {
    app.lock().unwrap_or_else(|e| e.into_inner()) // a panicked request leaves the state usable
}

/// One request on its own worker thread. Only routing holds the state lock; the body is read
/// before it, file work a handler deferred (`App::defer`) and the response written after it,
/// each read and write bounded by IO_TIMEOUT.
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
    let resp = match read_request_body(app, &mut rq, data) {
        Ok(body) => {
            let (r, later) = {
                let mut a = lock(app);
                let r = a.route(&mut rq, &body);
                (r, a.later.take())
            };
            match (r, later) {
                (Ok(_), Some(f)) => f(app),
                (r, _) => r,
            }
            .unwrap_or_else(|e| e)
        }
        Err(e) => {
            // Refused before (all of) the body was read: tiny_http would read the rest after the
            // response, holding this worker for as long as a slow client keeps sending.
            if let Some(p) = peer {
                with_socket(p, |s| {
                    let _ = s.shutdown(Shutdown::Read);
                });
            }
            e.with_header(hdr("Connection", "close"))
        }
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

/// `<Name>-Support` (+ `.exe`/`.zip`, and the macOS bundle name), name reduced to
/// `[A-Za-z0-9 _.-]` (safe inside the quoted header value; no `[` for `custom::mac_id_from_bundle`).
fn download_base(name: &str) -> String {
    let n: String = name.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '.' | '-')).collect();
    let n = n.trim_matches(|c| c == ' ' || c == '.');
    format!("{}-Support", if n.is_empty() { "TetherQS" } else { n })
}

/// A `/q/` download streamed by tether-api itself (no proxy in front that follows X-Accel-Redirect).
fn download(body: impl Read + Send + 'static, len: u64, ctype: &str, disposition: &str) -> Resp {
    Response::new(
        tiny_http::StatusCode(200),
        vec![
            hdr("Content-Type", ctype),
            hdr("Content-Disposition", disposition),
            hdr("Cache-Control", "no-store"),
            hdr("X-Content-Type-Options", "nosniff"),
        ],
        Box::new(body) as Box<dyn Read + Send>,
        Some(len as usize),
        None,
    )
    .with_chunked_threshold(usize::MAX) // tiny_http would switch to chunked above 32 KiB
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
    if !UPDATE_STATES.contains(&hb.update_state.as_str()) {
        return Err("bad update_state".into());
    }
    if !hb.update_version.is_empty() && !release::valid_version(&hb.update_version) {
        return Err("bad update_version".into());
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
    // From a reader: devices.json at MAX_DEVICES is ~7 MB, not read into memory first.
    match File::open(p) {
        Ok(f) => serde_json::from_reader(BufReader::new(f)).unwrap_or_else(|e| {
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
        let release_gen = fs::read_to_string(data.join("release-generation")).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
        let (mut indexes, mut indexes_v2) = (HashMap::new(), HashMap::new());
        for e in fs::read_dir(data.join("download"))?.flatten() {
            let ch = e.file_name().to_string_lossy().into_owned();
            if let Some(m) = fs::read(e.path().join(release::INDEX_FILE)).ok().and_then(|b| index_meta(&b)) {
                indexes.insert(ch.clone(), m);
            }
            if let Some(m) = fs::read(e.path().join(release::INDEX_V2_FILE)).ok().and_then(|b| index_meta(&b)) {
                indexes_v2.insert(ch, m);
            }
        }
        let mut custom: Vec<CustomMeta> = Vec::new();
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
            later: None,
            devices: devices.into_iter().map(|d| (d.uuid.clone(), d)).collect(),
            devices_dirty: false,
            ip_hits: HashMap::new(),
            nonces: load_json(&data.join("nonces.json")),
            tickets: load_json(&data.join("diag/tickets.json")),
            policy_serial,
            server_keys_serial,
            release_gen,
            indexes,
            indexes_v2,
            downloads: Default::default(),
            started: now(),
            keys,
            data,
        })
    }

    /// Writes `dest` through a temp file, fsync and rename; `write` streams into it.
    fn write_atomic_with(&self, dest: &Path, mode: u32, write: impl FnOnce(&mut BufWriter<&File>) -> io::Result<()>) -> io::Result<()> {
        let tmp = tmp_path(&self.data);
        let r = (|| {
            let f = OpenOptions::new().write(true).create_new(true).mode(mode).open(&tmp)?;
            let mut w = BufWriter::new(&f);
            write(&mut w)?;
            w.flush()?;
            drop(w);
            f.sync_all()?;
            fs::rename(&tmp, dest)
        })();
        if r.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        r
    }

    fn write_atomic(&self, dest: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
        self.write_atomic_with(dest, mode, |w| w.write_all(bytes))
    }

    fn save<T: Serialize + ?Sized>(&self, name: &str, v: &T) -> io::Result<()> {
        self.write_atomic_with(&self.data.join(name), 0o600, |w| serde_json::to_writer(w, v).map_err(io::Error::other))
    }

    /// A JSON response serialized into an (already unlinked) temp file instead of a buffer: a
    /// device list at MAX_DEVICES is ~7 MB, and MAX_WORKERS of them must fit in MemoryMax.
    fn json_file(&self, write: impl FnOnce(&mut BufWriter<&File>) -> serde_json::Result<()>) -> Result<Resp, Resp> {
        let tmp = tmp_path(&self.data);
        let mut f = OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).open(&tmp).map_err(internal)?;
        let _ = fs::remove_file(&tmp);
        let mut w = BufWriter::new(&f);
        write(&mut w).map_err(internal)?;
        w.flush().map_err(internal)?;
        drop(w);
        f.rewind().map_err(internal)?;
        Ok(Response::from_file(f).with_header(hdr("Content-Type", "application/json")).with_chunked_threshold(usize::MAX).boxed())
    }

    fn check_space(&self, incoming: u64) -> Result<(), Resp> {
        check_space(&self.data, incoming)
    }

    fn auth(&mut self, rq: &Request, method: &str, url: &str, body: &[u8]) -> Result<(), Resp> {
        let nonce = verify_sig(&self.keys, rq, method, url, body, now())?;
        self.use_nonce(nonce)
    }

    /// Replay check of a verified signature's nonce.
    fn use_nonce(&mut self, nonce: String) -> Result<(), Resp> {
        if self.nonces.insert(nonce, now()).is_some() {
            return Err(text(401, "replayed request"));
        }
        // Persisted so a restart does not reopen the replay window.
        self.save("nonces.json", &self.nonces).map_err(internal)
    }

    /// The checks a release upload must pass before its body is read (the signature covers the
    /// body, so it is verified after): the admin API is on, the path is valid, a signature with a
    /// current timestamp is there. The trusted keys, for that verification.
    fn release_precheck(&self, rq: &Request, path: &str) -> Result<Vec<VerifyingKey>, Resp> {
        if self.keys.is_empty() {
            return Err(admin_disabled());
        }
        release_path(&self.data, &path["/api/admin/release/".len()..])?;
        let sig = header(rq, reqauth::HEADER).ok_or_else(|| text(401, "missing signature"))?;
        let ts = sig.split_whitespace().find_map(|p| p.strip_prefix("ts=")?.parse::<u64>().ok());
        if !ts.is_some_and(|ts| now().abs_diff(ts) <= reqauth::MAX_SKEW_SECS) {
            return Err(text(401, "request timestamp outside allowed clock skew"));
        }
        Ok(self.keys.clone())
    }

    /// Index of the open diagnostics ticket `ticket`, or 404: body `ticket not open` when this
    /// relay has it but it is used, expired or closed (the apps then stop instead of trying the
    /// next relay, which may still have the code open), `not found` when unknown.
    fn open_ticket(&self, ticket: &str, t: u64) -> Result<usize, Resp> {
        match self.tickets.iter().position(|k| k.ticket == ticket) {
            Some(i) if self.tickets[i].state(t) == "open" => Ok(i),
            Some(_) => Err(text(404, TICKET_NOT_OPEN)),
            None => Err(not_found()),
        }
    }

    /// Lets `f` make the response after the state lock is released (file work that needs no
    /// state, or takes the lock again itself); the response returned here is replaced by f's.
    fn defer(&mut self, f: impl FnOnce(&Mutex<App>) -> Result<Resp, Resp> + Send + 'static) -> Result<Resp, Resp> {
        self.later = Some(Box::new(f));
        Ok(empty(500))
    }

    fn route(&mut self, rq: &mut Request, body: &Body) -> Result<Resp, Resp> {
        let url = rq.url().to_owned();
        let path = url.split('?').next().unwrap_or("");
        let method = rq.method().as_str().to_owned();
        match (method.as_str(), path) {
            ("GET", "/api/policy") => self.get_signed(rq, self.policy_serial, "policy.json", "no policy published"),
            ("GET", "/api/server-keys") => self.get_signed(rq, self.server_keys_serial, "server-keys.json", "no server key list published"),
            ("GET", p) if release_index_channel(p).is_some() => {
                let (ch, name) = release_index_channel(p).unwrap_or_default();
                let serial = self.index_map(name).get(ch).map(|m| m.0);
                let r = self.get_signed(rq, serial, &format!("download/{ch}/{name}"), "no release index published")?;
                Ok(r.with_header(hdr("Cache-Control", "no-cache")))
            }
            ("POST", "/api/heartbeat") => self.heartbeat(rq, body),
            ("POST", "/api/audit/conn") => self.audit_post(rq, body, "conn"),
            ("POST", "/api/audit/file") => self.audit_post(rq, body, "file"),
            ("POST", "/api/audit/alarm") => self.audit_post(rq, body, "alarm"),
            ("POST", p) if p.starts_with("/api/diag/") => self.diag_upload(body, &p["/api/diag/".len()..]),
            ("GET", p) if p.starts_with("/q/") => self.q(rq, &p["/q/".len()..]),
            (m, p) if p.starts_with("/api/team/") => self.team_route(rq, body, m, &url, &p["/api/team/".len()..]),
            (m, p) if p.starts_with("/api/admin/") => self.admin(rq, body, m, &url, &p["/api/admin/".len()..]),
            _ => Err(not_found()),
        }
    }

    /// A stored signed document (policy, server key list) with `ETag: "<serial>"`.
    fn get_signed(&self, rq: &Request, serial: Option<u64>, file: &str, missing: &str) -> Result<Resp, Resp> {
        let serial = serial.ok_or_else(|| text(404, missing))?;
        // Deleted on disk (scripts/release-index-v1.sh removes releases-v2.json): gone at once,
        // whatever the serial cached at start says.
        if !self.data.join(file).is_file() {
            return Err(text(404, missing));
        }
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
        let body = body.bytes(LIMIT_HEARTBEAT)?;
        let hb: Heartbeat = serde_json::from_slice(body).map_err(|e| text(400, &e.to_string()))?;
        valid_heartbeat(&hb).map_err(|e| text(400, &e))?;
        // Known devices are limited per uuid only: a site behind one NAT IP has hundreds of them.
        // The per-IP limit is for new uuids, which would push real devices out of the list.
        match self.devices.get(&hb.uuid) {
            Some(d) if t < d.last_seen + HB_UUID_INTERVAL => return Err(text(429, "heartbeat too frequent")),
            Some(_) => {}
            None if !rate_ok(&mut self.ip_hits, client_ip(rq), t, HB_IP_PER_MIN) => return Err(text(429, "too many requests")),
            None => {}
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
            release_serial: hb.release_serial,
            update_state: hb.update_state,
            update_version: hb.update_version,
            ip,
            last_seen: t,
        };
        // "Downloads by version": a device reporting a staged update it did not report before.
        let staged = |s: &str| matches!(s, "staged" | "waiting-idle");
        let prev = self.devices.get(&d.uuid).filter(|p| staged(&p.update_state)).map(|p| p.update_version.clone());
        if staged(&d.update_state) && !d.update_version.is_empty() && prev.as_deref() != Some(&d.update_version) {
            let k = (d.profile.clone(), d.update_version.clone());
            if self.downloads.contains_key(&k) || self.downloads.len() < MAX_DOWNLOAD_SERIES {
                *self.downloads.entry(k).or_default() += 1;
            }
        }
        self.devices.insert(d.uuid.clone(), d);
        self.devices_dirty = true;
        // The release generation: a device that sees it change checks for updates (≤ 15 min).
        Ok(json(&serde_json::json!({ "releases": self.release_gen })))
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
    fn audit_read(&mut self, url: &str) -> Result<Resp, Resp> {
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
        let dir = self.audit.dir.clone();
        self.defer(move |_| Ok(json(&audit::read(&dir, since, limit, &device))))
    }

    fn diag_upload(&mut self, body: &Body, ticket: &str) -> Result<Resp, Resp> {
        let t = now();
        let idx = self.open_ticket(ticket, t)?;
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
            return Err(admin_disabled());
        }
        if let Some(rel) = rest.strip_prefix("release/") {
            let dest = release_path(&self.data, rel)?;
            return match method {
                "PUT" => self.put_release(body, &dest),
                "DELETE" => {
                    self.auth(rq, method, url, body.bytes(LIMIT_SMALL)?)?;
                    self.delete_guarded(&dest, url)
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
            ("POST", "releases/prune") => prune::LIMIT_PRUNE,
            ("PUT", r) if r.starts_with("custom/") => LIMIT_CUSTOM,
            ("DELETE", r) if r.starts_with("custom/") => LIMIT_SMALL,
            ("GET", "policy-history" | "devices" | "diag" | "releases" | "status" | "custom" | "audit" | "team") => LIMIT_SMALL,
            ("GET", r) if r.starts_with("diag/") || r.starts_with("policy-history/") => LIMIT_SMALL,
            ("POST", r) if r.starts_with("diag/") && r.ends_with("/close") => LIMIT_SMALL,
            ("DELETE", r) if r.starts_with("diag/") => LIMIT_SMALL,
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
            ("POST", "releases/prune") => self.prune(body),
            ("PUT", r) if r.starts_with("custom/") => self.put_custom(&r["custom/".len()..], body),
            ("DELETE", r) if r.starts_with("custom/") => self.delete_custom(&r["custom/".len()..]),
            ("GET", "custom") => Ok(json(&self.custom)),
            ("GET", "policy-history") => self.policy_history(),
            ("GET", "devices") => {
                let mut v: Vec<&Device> = self.devices.values().collect();
                v.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
                self.json_file(|w| serde_json::to_writer(w, &v))
            }
            ("GET", "diag") => self.list_tickets(url),
            ("POST", r) if r.starts_with("diag/") => self.close_ticket(r["diag/".len()..].strip_suffix("/close").unwrap_or_default()),
            ("DELETE", r) if r.starts_with("diag/") => self.delete_ticket(&r["diag/".len()..]),
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
                "release_generation": self.release_gen,
                "release_indexes": self.indexes.iter().map(|(c, (s, e))| (c.clone(), serde_json::json!({"serial": s, "expires": e}))).collect::<serde_json::Map<_, _>>(),
                "uptime": now().saturating_sub(self.started),
                "last_prune": prune::last(&self.data),
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
        self.tickets.push(Ticket { ticket: ticket.clone(), note: req.note, created, uploaded: None, size: None, closed: None });
        let tickets = self.tickets.clone();
        self.save("diag/tickets.json", &tickets).map_err(internal)?;
        Ok(json(&serde_json::json!({"ticket": ticket, "expires": created + TICKET_OPEN})))
    }

    /// Active tickets (open, uploaded, expired), or with `?archived=1` the closed ones; each with
    /// its `state`.
    fn list_tickets(&self, url: &str) -> Result<Resp, Resp> {
        let archived = match url.split_once('?').map(|q| q.1) {
            None => false,
            Some("archived=1") => true,
            Some(q) => return Err(text(400, &format!("unknown parameter {q}"))),
        };
        let t = now();
        let v: Vec<serde_json::Value> = self
            .tickets
            .iter()
            .filter(|k| k.closed.is_some() == archived)
            .map(|k| {
                let mut v = serde_json::to_value(k).unwrap_or_default();
                v["state"] = k.state(t).into();
                v
            })
            .collect();
        Ok(json(&v))
    }

    fn save_tickets(&self) -> Result<(), Resp> {
        self.save("diag/tickets.json", &self.tickets).map_err(internal)
    }

    /// Archive: no more uploads; the entry and its zip stay DIAG_ARCHIVE_KEEP. Closing again is a no-op.
    fn close_ticket(&mut self, ticket: &str) -> Result<Resp, Resp> {
        let k = self.tickets.iter_mut().find(|k| k.ticket == ticket).ok_or_else(not_found)?;
        if k.closed.is_none() {
            k.closed = Some(now());
            self.save_tickets()?;
            eprintln!("diag ticket {ticket} closed");
        }
        Ok(empty(204))
    }

    /// Removes the ticket and its zip for good, in any state.
    fn delete_ticket(&mut self, ticket: &str) -> Result<Resp, Resp> {
        let i = self.tickets.iter().position(|k| k.ticket == ticket).ok_or_else(not_found)?;
        let k = self.tickets.remove(i);
        if let Err(e) = self.save_tickets() {
            self.tickets.insert(i, k);
            return Err(e);
        }
        // The ticket is a validated entry of the list, so this path stays inside diag/.
        let _ = fs::remove_file(self.data.join("diag").join(format!("{ticket}.zip")));
        eprintln!("diag ticket {ticket} deleted");
        Ok(empty(204))
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
        let meta = CustomMeta { id: id.to_owned(), name: p.name, token: req.token, expires: req.expires, channel: req.channel, created, downloads, last_download };
        self.check_space(body.len() as u64)?;
        self.save(&format!("custom/{id}.json"), &CustomApp { meta: meta.clone(), profile: req.profile }).map_err(internal)?;
        qcache_drop(&self.data, id, "", "");
        match old {
            Some(i) => self.custom[i] = meta,
            None => self.custom.push(meta),
        }
        Ok(empty(204))
    }

    fn delete_custom(&mut self, id: &str) -> Result<Resp, Resp> {
        let i = self.custom.iter().position(|c| c.id == id && valid_custom_id(id)).ok_or_else(not_found)?;
        fs::remove_file(self.data.join("custom").join(format!("{id}.json"))).map_err(internal)?;
        qcache_drop(&self.data, id, "", "");
        self.custom.remove(i);
        Ok(empty(204))
    }

    /// `GET /q/<token>` (Windows exe, or the macOS zip for a Mac browser), `/q/<token>/windows`,
    /// `/q/<token>/mac`, and `/q/m/<mac id>` (the signed profile of a macOS custom app). One rate
    /// limit for all of them.
    fn q(&mut self, rq: &Request, rest: &str) -> Result<Resp, Resp> {
        if !rate_ok(&mut self.q_hits, client_ip(rq), now(), Q_IP_PER_MIN) {
            return Err(text(429, "too many requests"));
        }
        if let Some(id) = rest.strip_prefix("m/") {
            return self.custom_profile(id);
        }
        let (token, mac) = match rest.split_once('/') {
            // Mac browsers (Safari, Chrome, Firefox, Edge) all say "Macintosh"; iPhones say "like Mac OS X".
            None => (rest, header(rq, "User-Agent").is_some_and(|ua| ua.contains("Macintosh"))),
            Some((t, "windows")) => (t, false),
            Some((t, "mac")) => (t, true),
            _ => return Err(not_found()),
        };
        self.custom_download(rq, token, mac)
    }

    /// The signed profile of the custom app whose token gives this mac id (`custom::mac_id`).
    /// Also after the link expired: copies downloaded before keep working, as on Windows; a
    /// deleted app answers 404.
    fn custom_profile(&self, id: &str) -> Result<Resp, Resp> {
        if !custom::valid_mac_id(id) {
            return Err(not_found());
        }
        let c = self.custom.iter().find(|c| custom::mac_id(&c.token) == id).ok_or_else(not_found)?;
        let app = read_custom(&self.data, &c.id).map_err(internal)?;
        Ok(json(&app.profile).with_header(hdr("Cache-Control", "no-store")))
    }

    /// The channel's current TetherQS.exe plus the signed profile trailer, or for macOS its
    /// TetherQS zip with the `.app` named `<Name>-Support [<mac id>].app`. Unknown, revoked and
    /// expired tokens get the same 404. Reading the release and preparing the copy (up to 200 MB,
    /// with fsync) run after the state lock is released.
    fn custom_download(&mut self, rq: &Request, token: &str, mac: bool) -> Result<Resp, Resp> {
        let t = now();
        if !custom::valid_token(token) {
            return Err(not_found());
        }
        let c = self.custom.iter().find(|c| c.token == token && t < c.expires).ok_or_else(not_found)?.clone();
        let (data, token) = (self.data.clone(), token.to_owned());
        let accel = header(rq, "X-Tether-Accel").as_deref() == Some("1");
        self.defer(move |app| {
            let resp = custom_file(&data, &c, &token, mac, accel)?;
            lock(app).count_download(&c.id, &token, t);
            Ok(resp)
        })
    }

    /// Counts a `/q/` download in memory and in `custom/<id>.json`, unless the link changed since.
    fn count_download(&mut self, id: &str, token: &str, t: u64) {
        let Some(c) = self.custom.iter_mut().find(|c| c.id == id && c.token == token) else { return };
        c.downloads += 1;
        c.last_download = Some(t);
        let meta = c.clone();
        let r = read_custom(&self.data, id).and_then(|f| self.save(&format!("custom/{id}.json"), &CustomApp { meta, profile: f.profile }));
        if let Err(e) = r {
            eprintln!("custom app {id}: could not save the download count: {e}");
        }
    }

    /// The signature over the body was verified before the state lock (`read_request_body`).
    fn put_release(&mut self, body: &Body, dest: &Path) -> Result<Resp, Resp> {
        let Body::File { path: tmp, nonce: Some(nonce), .. } = body else { return Err(internal("release upload not verified")) };
        self.use_nonce(nonce.clone())?;
        let index = self.index_channel(dest);
        let mut new_index = None;
        if let Some(ch) = &index {
            // The signed release index: verified like a policy, serials only go up. The same
            // document again (Manage's "publish to missing relays") is accepted unchanged.
            let raw = fs::read(tmp).map_err(internal)?;
            if raw.len() > release::MAX_INDEX {
                return Err(too_large());
            }
            let ix = release::Index::open(&raw, &self.keys, &ch.0).map_err(|e| text(400, &e))?;
            if let Some((cur, _)) = self.index_map(&ch.1).get(&ch.0) {
                if ix.serial < *cur || (ix.serial == *cur && fs::read(dest).ok().as_deref() != Some(&raw[..])) {
                    return Err(text(409, "serial must be greater than the current release index's"));
                }
            }
            new_index = Some((ch.clone(), (ix.serial, ix.expires)));
        }
        mkdirs(dest.parent().expect("under download/"), 0o750).map_err(internal)?;
        fs::rename(tmp, dest).map_err(internal)?;
        // Only now: after a failed rename the old index is still what is served.
        if let Some(((ch, name), m)) = new_index {
            self.index_map_mut(&name).insert(ch, m);
        }
        self.bump_generation(dest);
        Ok(empty(204))
    }

    fn index_map(&self, name: &str) -> &HashMap<String, (u64, u64)> {
        if name == release::INDEX_V2_FILE { &self.indexes_v2 } else { &self.indexes }
    }

    fn index_map_mut(&mut self, name: &str) -> &mut HashMap<String, (u64, u64)> {
        if name == release::INDEX_V2_FILE { &mut self.indexes_v2 } else { &mut self.indexes }
    }

    /// `download/<channel>/releases.json` (or `releases-v2.json`) → (channel, file name).
    fn index_channel(&self, dest: &Path) -> Option<(String, String)> {
        let rel = dest.strip_prefix(self.data.join("download")).ok()?;
        let mut it = rel.iter();
        let ch = it.next()?.to_str()?.to_owned();
        let name = it.next()?.to_str()?.to_owned();
        (is_index_file(&name) && it.next().is_none()).then_some((ch, name))
    }

    /// Every change of an index or a `latest.json` bumps G, written for hbbs (`release-generation`,
    /// group-readable: hbbs's user is in group tether-api, see install.sh).
    fn bump_generation(&mut self, dest: &Path) {
        if !dest.file_name().and_then(|n| n.to_str()).is_some_and(|n| is_index_file(n) || n == "latest.json") {
            return;
        }
        self.release_gen += 1;
        let (gen, path) = (self.release_gen, self.data.join("release-generation"));
        if let Err(e) = self.write_atomic(&path, format!("{gen}\n").as_bytes(), 0o644) {
            eprintln!("release-generation: {e}");
        }
    }

    fn delete_release(&mut self, dest: &Path) -> Result<Resp, Resp> {
        if !dest.is_file() {
            return Err(not_found());
        }
        fs::remove_file(dest).map_err(internal)?;
        if let Some((ch, name)) = self.index_channel(dest) {
            self.index_map_mut(&name).remove(&ch);
        }
        self.bump_generation(dest);
        self.remove_empty_dirs(dest.parent().expect("under download/"));
        Ok(empty(204))
    }

    /// Removes `dir` and its parents up to `download/` while they are empty.
    fn remove_empty_dirs(&self, dir: &Path) {
        let root = self.data.join("download");
        let mut dir = Some(dir);
        while let Some(d) = dir.filter(|d| *d != root) {
            if fs::remove_dir(d).is_err() {
                break; // not empty
            }
            dir = d.parent();
        }
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
        let active: Vec<&Device> = self.devices.values().filter(|d| d.last_seen + DAY > t).collect();
        let mut by_version: HashMap<(String, String), u64> = HashMap::new();
        let mut states: HashMap<String, u64> = HashMap::new();
        for d in &active {
            *by_version.entry((d.profile.clone(), d.version.clone())).or_default() += 1;
            let s = if d.update_state.is_empty() { "unknown" } else { d.update_state.as_str() };
            *states.entry(s.to_owned()).or_default() += 1;
        }
        Some(metrics::Now {
            max_workers: MAX_WORKERS,
            devices: self.devices.len(),
            devices_24h: active.len(),
            release_generation: self.release_gen,
            indexes: self.indexes.iter().map(|(c, (s, e))| (c.clone(), *s, *e)).collect(),
            by_version: metrics::top(by_version, metrics::MAX_VERSION_SERIES),
            update_states: states.into_iter().collect(),
            downloads: self.downloads.iter().map(|(k, v)| (k.clone(), *v)).collect(),
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
            let keep = t < k.keep_until();
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
            // Straight from the map into the file: no copy of up to MAX_DEVICES devices.
            let write = |w: &mut BufWriter<&File>| serde::Serializer::collect_seq(&mut serde_json::Serializer::new(w), self.devices.values()).map_err(io::Error::other);
            if self.write_atomic_with(&self.data.join("devices.json"), 0o600, write).map_err(internal).is_ok() {
                self.devices_dirty = false;
            }
        }
    }
}


/// The `/q/` download of custom app `c` (no state lock held: reads the release and the profile,
/// may prepare a copy).
fn custom_file(data: &Path, c: &CustomMeta, token: &str, mac: bool, accel: bool) -> Result<Resp, Resp> {
    let (platform, ctype, ext) = if mac {
        ("darwin-universal", "application/zip", "zip")
    } else {
        ("windows-x86_64", "application/vnd.microsoft.portable-executable", "exe")
    };
    let dir = data.join("download").join(&c.channel).join("customer").join(platform);
    let exe = (|| {
        let m: serde_json::Value = serde_json::from_slice(&fs::read(dir.join("latest.json")).ok()?).ok()?;
        let file = m["platforms"][platform]["url"].as_str()?.rsplit('/').next().filter(|f| valid_segment(f))?.to_owned();
        let f = File::open(dir.join(&file)).ok()?;
        let meta = f.metadata().ok().filter(|m| m.is_file())?;
        Some((f, meta, file))
    })();
    let Some((mut f, meta, file)) = exe else {
        eprintln!("custom app {} (token {}…): no {platform} TetherQS release in channel {}", c.id, token_hint(token), c.channel);
        return Err(not_found());
    };
    // Deleted since the lookup: the same 404 as an unknown token.
    let doc = serde_json::to_vec(&read_custom(data, &c.id).map_err(|_| not_found())?.profile).map_err(internal)?;
    // The prepared copy is named after what it is made of (any SHA-256 would do), so a copy built
    // from a profile or token that changed meanwhile is never taken for the current one.
    let key = custom::mac_id(&format!("{token}\n{}", String::from_utf8_lossy(&doc)));
    let base = download_base(&c.name);
    let disposition = format!("attachment; filename=\"{base}.{ext}\"");
    let id = &c.id;
    // Behind the Caddyfile's handle_response (it sets X-Tether-Accel): answer with the path of a
    // prepared copy and let Caddy send it, so a slow download does not hold a worker.
    let accel_resp = |rel: &str| empty(204).with_header(hdr("X-Accel-Redirect", rel)).with_header(hdr("Content-Type", ctype)).with_header(hdr("Content-Disposition", &disposition));
    // Always prepared on disk, streamed file to file (tether-api runs with MemoryMax=64M): the zip
    // with its bundle renamed, or the exe with the trailer (custom::attach_plan; inside the
    // Authenticode table if the exe is signed, which changes two 4-byte size fields).
    let rel = if mac {
        let bundle = custom::mac_bundle_name(&base, &custom::mac_id(token));
        qcache(data, id, &key, &file, &meta, |out| maczip::rename_app(&mut f, out, &bundle).map(drop))
    } else {
        qcache(data, id, &key, &file, &meta, |out| {
            let (patches, tail) = custom::attach_plan(&mut f, &doc)?;
            f.rewind()?;
            io::copy(&mut f, out)?;
            for (at, b) in patches {
                out.seek(io::SeekFrom::Start(at))?;
                out.write_all(&b)?;
            }
            out.seek(io::SeekFrom::End(0))?;
            out.write_all(&tail)
        })
    }
    .map_err(|e| {
        eprintln!("custom app {id}: could not prepare the download from {file}: {e}");
        not_found()
    })?;
    if accel {
        return Ok(accel_resp(&rel));
    }
    let f = File::open(data.join(rel.trim_start_matches('/'))).map_err(internal)?;
    let len = f.metadata().map_err(internal)?.len();
    Ok(download(f, len, ctype, &disposition))
}

/// `qcache/<id>--<key>--<release file>`: the release with this app's trailer (exe) or renamed
/// bundle (zip), as Caddy serves it for `/q/` (path relative to the data dir). Rebuilt when the
/// release is newer; other copies of the app's platform are
/// removed, and every profile or token change drops them all. Concurrent first downloads each
/// write their own temp file and rename it into place. The directory is not under `download/`,
/// so it is reachable only through the X-Accel-Redirect of a valid token.
fn qcache(data: &Path, id: &str, key: &str, file: &str, meta: &fs::Metadata, write: impl FnOnce(&mut File) -> io::Result<()>) -> io::Result<String> {
    let dir = data.join("qcache");
    mkdirs(&dir, 0o750)?;
    let name = format!("{id}--{key}--{file}");
    let dest = dir.join(&name);
    let fresh = fs::metadata(&dest).is_ok_and(|m| m.modified().ok() >= meta.modified().ok());
    if !fresh {
        // shortcut: a copy that finishes after its app was deleted stays on disk (unreachable: no
        // token names it) until an app with that id prepares one; sweep qcache/ in maintain() if
        // deletes during first downloads become common.
        qcache_drop(data, id, Path::new(file).extension().and_then(|e| e.to_str()).unwrap_or(""), &name);
        let tmp = tmp_path(data);
        let r = (|| {
            let mut f = OpenOptions::new().write(true).create_new(true).mode(0o640).open(&tmp)?;
            write(&mut f)?;
            f.sync_all()?;
            fs::rename(&tmp, &dest)
        })();
        if r.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        r?;
    }
    Ok(format!("/qcache/{name}"))
}

/// Removes the prepared `/q/` copies of an app (its profile or token changed, or it was
/// revoked), or with `ext` only those of one platform (`exe`, `zip`), except `keep`.
fn qcache_drop(data: &Path, id: &str, ext: &str, keep: &str) {
    let prefix = format!("{id}--");
    for e in fs::read_dir(data.join("qcache")).into_iter().flatten().flatten() {
        if e.file_name().to_str().is_some_and(|n| n.starts_with(&prefix) && n.ends_with(ext) && n != keep) {
            let _ = fs::remove_file(e.path());
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

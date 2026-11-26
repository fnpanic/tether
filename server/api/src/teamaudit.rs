//! Team audit log (design 12.5): every access of support staff to the practices' data on this
//! relay, reads and refused attempts included: who (name and key id), when, from which address,
//! the event, which device (uuid, Tether ID, customer) and the HTTP result. Never what was
//! returned: no report, software list, summary content or search hit. docs/api.md "Team audit log".
//!
//! `team-audit/<YYYY-MM-DD>.jsonl` (UTC day, 0600), one JSON line per record, fields in a fixed
//! order, each with `prev`: the SHA-256 (lower-case hex) of the line before it without its newline
//! (the first line of a day: the last line of the day before; the very first line: 64 zeros).
//! One writer thread takes records from a bounded queue ([`Log::submit`] never blocks: when the
//! queue is full the record is dropped and counted, and a `dropped` record in the chain says how
//! many); each line is appended with one write, the file synced every second, the line copied to
//! the journal (`team-audit: <line>`) and the head (hash of the last line) kept in `team-audit/head`
//! and in the journal every 10 minutes. A restart continues the chain from the newest line; when
//! the files no longer reach the stored head, a `chain-gap` record says so.
//!
//! What the chain gives: an edited, removed or inserted line, a removed day or a cut file shows as a
//! break to anyone reading the files ([`read`]), unless the whole tail after the change was
//! rewritten; whoever controls the relay can do that, which only a head kept elsewhere (Tether
//! Manage, the journal in a SIEM) reveals.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tether_trust::status as st;

use crate::audit::date;

/// Records waiting for the writer at most (each a boxed `Rec`, a few hundred bytes).
pub const QUEUE: usize = 10_000;
pub const ZERO: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const DAY: u64 = 86_400;
const SYNC_EVERY: Duration = Duration::from_secs(1);
/// A `dropped` record and its journal line at most this often (seconds).
const DROPPED_EVERY: u64 = 10;
const HEAD_EVERY: u64 = 600;
const PRUNE_EVERY: u64 = 3600;
/// A record is well under 2 KiB; of a longer line only this much is looked at (its hash is whole).
const LINE_MAX: usize = 16 << 10;
/// Bytes of the previous day read to find its last line.
const TAIL: u64 = 64 << 10;
/// Free text in a record (names, filters, uuids as asked), in bytes.
const TEXT_MAX: usize = 128;
pub const READ_MAX: usize = 5000;
/// Record bytes in one answer at most (as the session audit, `audit.rs`).
const READ_BYTES: usize = 8 << 20;
/// Bytes of log one answer reads at most; the cursor continues from there.
const SCAN_BYTES: u64 = 256 << 20;
const PROBLEMS_MAX: usize = 100;
/// Member-reported events per member key and minute.
pub const POSTED_PER_MIN: u32 = 30;

/// Counted since start (`/metrics`).
#[derive(Default)]
pub struct Counters {
    pub written: AtomicU64,
    /// Dropped because the queue was full.
    pub queue_full: AtomicU64,
    /// Dropped because today's file alone reached TEAM_AUDIT_MAX_MB.
    pub log_full: AtomicU64,
    /// Dropped because the line could not be written (disk full, folder not writable).
    pub write_failed: AtomicU64,
    /// Failed syncs of the day file.
    pub write_errors: AtomicU64,
}

#[derive(Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub struct Actor {
    /// `tether_trust::team::key_id` of the key that signed the request.
    pub key_fingerprint: String,
    pub name: String,
    /// A signed-in person (oidc.rs): object id and user name; absent for a roster key (and on
    /// lines written before, so their hashes stay).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upn: Option<String>,
}

#[derive(Serialize, Default, Clone, Debug, PartialEq)]
pub struct Target {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tether_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub customer: Option<String>,
}

impl Target {
    fn is_empty(&self) -> bool {
        self.uuid.is_none() && self.tether_id.is_none() && self.customer.is_none()
    }

    /// A device as a request names it, nothing looked up: digits only are a Tether ID, other
    /// uuid-shaped text a uuid.
    pub fn asked(device: &str) -> Target {
        if st::valid_uuid(device) && !device.bytes().all(|b| b.is_ascii_digit()) {
            Target { uuid: Some(device.to_owned()), ..Default::default() }
        } else {
            Target { tether_id: Some(text(device)), ..Default::default() }
        }
    }
}

/// One record; serialised in this field order (the line is what is hashed).
#[derive(Serialize, Debug)]
pub struct Rec {
    pub ts: u64,
    pub event: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<Actor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    #[serde(skip_serializing_if = "Target::is_empty")]
    pub target: Target,
    #[serde(skip_serializing_if = "Value::is_null")]
    pub detail: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<u16>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub self_reported: bool,
    /// Set by the writer.
    pub prev: String,
}

impl Rec {
    pub fn new(event: &'static str, actor: Option<Actor>, ip: Option<String>, target: Target, detail: Value) -> Rec {
        Rec { ts: crate::now(), event, actor, ip, target, detail, result: None, self_reported: false, prev: String::new() }
    }

    /// The HTTP status the request got. A refused one keeps only what the request named: the
    /// device's Tether ID and customer are left out, so the record confirms nothing about it.
    pub fn answered(mut self, code: u16) -> Rec {
        if !(200..400).contains(&code) {
            self.target.tether_id = self.target.tether_id.filter(|_| self.target.uuid.is_none());
            self.target.customer = None;
        }
        self.result = Some(code);
        self
    }
}

/// Reads folded into one record per actor, event and hour (`summary`, `device-list`,
/// `status-device-list`): the first read of an hour is recorded at once with `{"reads": n}` (the
/// reads since the record before, this one included); reads after it are written by the hourly
/// maintenance ([`Folded::due`]) or at a clean stop. Entries unseen for a day are dropped.
#[derive(Default)]
pub struct Folded(std::collections::HashMap<(String, &'static str), Fold>);

struct Fold {
    /// When the last record was written.
    at: u64,
    /// Reads since then, not recorded yet.
    reads: u64,
    last: u64,
    actor: Actor,
    ip: Option<String>,
}

impl Folded {
    /// A read by `actor`: its record when one is due now (the first read of an hour).
    pub fn read(&mut self, event: &'static str, actor: Actor, ip: Option<String>, t: u64) -> Option<Rec> {
        let f = self.0.entry((actor.key_fingerprint.clone(), event)).or_insert_with(|| Fold { at: 0, reads: 0, last: t, actor: actor.clone(), ip: None });
        (f.reads, f.last, f.actor, f.ip) = (f.reads + 1, t, actor, ip);
        (t >= f.at + 3600).then(|| Self::take(event, f, t))
    }

    fn take(event: &'static str, f: &mut Fold, t: u64) -> Rec {
        let n = std::mem::take(&mut f.reads);
        f.at = t;
        Rec::new(event, Some(f.actor.clone()), f.ip.clone(), Target::default(), json!({"reads": n}))
    }

    /// The records of reads not recorded for an hour (`all`: every pending one, at a stop); drops
    /// entries without reads unseen for a day.
    pub fn due(&mut self, t: u64, all: bool) -> Vec<Rec> {
        let mut out = Vec::new();
        for ((_, event), f) in self.0.iter_mut() {
            if f.reads > 0 && (all || t >= f.at + 3600) {
                out.push(Self::take(event, f, t));
            }
        }
        self.0.retain(|_, f| f.reads > 0 || t < f.last + DAY);
        out
    }
}

/// At most TEXT_MAX bytes of `s` (on a character boundary).
pub fn text(s: &str) -> String {
    let mut n = s.len().min(TEXT_MAX);
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    s[..n].to_owned()
}

fn hex(h: Sha256) -> String {
    format!("{:x}", h.finalize())
}

fn hash(line: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(line);
    hex(h)
}

enum Msg {
    Rec(Box<Rec>),
    Flush(mpsc::Sender<()>),
}

/// The handle requests use; the writer thread owns the files.
#[derive(Clone)]
pub struct Log {
    tx: SyncSender<Msg>,
    head: Arc<Mutex<String>>,
    pub counters: Arc<Counters>,
    pub dir: PathBuf,
}

impl Log {
    /// Opens `dir` (created 0700), continues its chain and starts the writer. `journal`: copy
    /// every line to stderr (off in unit tests).
    pub fn start(dir: PathBuf, retention_days: u64, max_bytes: u64, journal: bool) -> io::Result<Log> {
        crate::mkdirs(&dir, 0o700)?;
        // At start the relay refuses to run without a folder it can write (while running it fails open).
        let probe = dir.join("probe.tmp");
        OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&probe)?.write_all(b"x")?;
        fs::remove_file(&probe)?;
        let counters = Arc::new(Counters::default());
        let w = Writer::open(dir.clone(), retention_days, max_bytes, journal, counters.clone(), crate::now())?;
        let head = Arc::new(Mutex::new(w.prev.clone()));
        let (tx, rx) = mpsc::sync_channel(QUEUE);
        let h = head.clone();
        std::thread::Builder::new().name("team-audit".into()).spawn(move || w.run(rx, &h))?;
        Ok(Log { tx, head, counters, dir })
    }

    /// Queues `r`; never waits: a full queue drops it (counted, and reported in the chain).
    pub fn submit(&self, r: Rec) {
        if self.tx.try_send(Msg::Rec(Box::new(r))).is_err() {
            self.counters.queue_full.fetch_add(1, Relaxed);
        }
    }

    /// Waits up to `wait` until everything queued before is written and synced (not when the
    /// queue is full).
    pub fn flush(&self, wait: Duration) -> bool {
        let (t, r) = mpsc::channel();
        self.tx.try_send(Msg::Flush(t)).is_ok() && r.recv_timeout(wait).is_ok()
    }

    /// Hash of the last line written (64 zeros: none yet).
    pub fn head(&self) -> String {
        self.head.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

struct Writer {
    dir: PathBuf,
    retention_days: u64,
    max_bytes: u64,
    journal: bool,
    counters: Arc<Counters>,
    /// Hash of the last line.
    prev: String,
    /// Day of the file being written; never goes back (a record of a second before midnight
    /// that comes late goes into the new day, so files stay in chain order).
    day: String,
    file: Option<File>,
    /// Bytes of all day files.
    total: u64,
    dirty: bool,
    head_saved: String,
    /// The drop counters as of the last `dropped` record, and when its window began.
    seen: (u64, u64, u64),
    since: u64,
    last_dropped: u64,
    /// When a failed write was last copied to the journal.
    last_write_err: u64,
    /// The day on which today's file alone reached the size cap: no folder scan per record that day.
    cap_day: Option<String>,
    /// Appends one line (a failing one in tests); a failed append is cut back to the length before.
    put: fn(&File, &[u8]) -> io::Result<()>,
    last_head: u64,
    last_prune: u64,
}

impl Writer {
    fn open(dir: PathBuf, retention_days: u64, max_bytes: u64, journal: bool, counters: Arc<Counters>, t: u64) -> io::Result<Writer> {
        let files = files(&dir);
        let newest = files.iter().rev().find(|(_, n)| *n > 0).map(|(d, _)| d.clone());
        let prev = newest.as_ref().and_then(|d| tail_hash(&dir.join(format!("{d}.jsonl")))).unwrap_or_else(|| ZERO.to_owned());
        let stored = fs::read_to_string(dir.join("head")).ok().map(|s| s.trim().to_owned()).filter(|s| s.len() == 64);
        let mut w = Writer {
            total: files.iter().map(|(_, n)| n).sum(),
            day: newest.clone().unwrap_or_default(),
            dir,
            retention_days,
            max_bytes,
            journal,
            counters,
            head_saved: stored.clone().unwrap_or_default(),
            prev,
            file: None,
            dirty: false,
            seen: (0, 0, 0),
            since: t,
            last_dropped: 0,
            last_write_err: 0,
            cap_day: None,
            put: |mut f, b| f.write_all(b),
            last_head: 0,
            last_prune: t,
        };
        w.prune(t, 0);
        // The stored head must be a line of the newest file or the line its first line follows:
        // otherwise lines were removed (or files lost) since it was written.
        if let Some(h) = stored.filter(|h| h != &w.prev) {
            let reached = newest.is_some_and(|d| reaches(&w.dir.join(format!("{d}.jsonl")), &h));
            if !reached {
                eprintln!("team-audit: the log does not reach its last head {h} (now {}): lines or files were removed", w.prev);
                let detail = json!({"expected": h, "found": w.prev});
                w.write(Rec { ts: t, ..Rec::new("chain-gap", None, None, Target::default(), detail) }, t, true);
            }
        }
        Ok(w)
    }

    fn run(mut self, rx: Receiver<Msg>, head: &Mutex<String>) {
        let mut synced = Instant::now();
        loop {
            match rx.recv_timeout(SYNC_EVERY) {
                Ok(Msg::Rec(r)) => {
                    self.write(*r, crate::now(), false);
                }
                Ok(Msg::Flush(done)) => {
                    self.housekeep(crate::now(), true);
                    *head.lock().unwrap_or_else(|e| e.into_inner()) = self.prev.clone();
                    let _ = done.send(());
                    synced = Instant::now();
                    continue;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    self.housekeep(crate::now(), true);
                    return;
                }
            }
            *head.lock().unwrap_or_else(|e| e.into_inner()) = self.prev.clone();
            if synced.elapsed() >= SYNC_EVERY {
                self.housekeep(crate::now(), false);
                *head.lock().unwrap_or_else(|e| e.into_inner()) = self.prev.clone();
                synced = Instant::now();
            }
        }
    }

    /// Appends `rec` to its day's file. `force`: also over the size cap (the writer's own small
    /// `dropped` and `chain-gap` records, at most one per DROPPED_EVERY).
    fn write(&mut self, mut rec: Rec, t: u64, force: bool) -> bool {
        let day = date(rec.ts).max(self.day.clone());
        if day != self.day {
            self.file = None;
            self.day = day;
            self.cap_day = None;
            self.prune(t, 0);
        }
        rec.prev = self.prev.clone();
        let Ok(mut line) = serde_json::to_vec(&rec) else { return false };
        let n = line.len() as u64 + 1;
        if !force && self.total + n > self.max_bytes {
            if self.cap_day.as_ref() != Some(&self.day) {
                self.prune(t, n);
            }
            if self.total + n > self.max_bytes {
                // Only today's file is left (never removed): no folder scan again until the hourly prune.
                self.cap_day = Some(self.day.clone());
                self.counters.log_full.fetch_add(1, Relaxed);
                return false;
            }
        }
        let h = hash(&line);
        line.push(b'\n');
        let r = (|| {
            if self.file.is_none() {
                let p = self.dir.join(format!("{}.jsonl", self.day));
                let mut f = OpenOptions::new().read(true).append(true).create(true).mode(0o600).open(p)?;
                // A line cut by a crash ends here; the next one starts on its own line (and
                // chains to the cut one, whose hash is its bytes as they are).
                if ends_unterminated(&mut f)? {
                    f.write_all(b"\n")?;
                    self.total += 1;
                    eprintln!("team-audit: {}.jsonl ended in a cut line (a crash while writing): the next record starts on a new line", self.day);
                }
                self.file = Some(f);
            }
            let put = self.put;
            self.file.as_ref().map_or(Ok(()), |f| crate::status::append_whole(f, &line, put))
        })();
        if let Err(e) = r {
            // Counted as dropped (reported in the next `dropped` record); the request is answered
            // anyway (fails open, docs/api.md).
            self.counters.write_failed.fetch_add(1, Relaxed);
            if t >= self.last_write_err + DROPPED_EVERY {
                self.last_write_err = t;
                eprintln!("error: team-audit: a record could not be written: {e}");
            }
            self.file = None;
            return false;
        }
        self.prev = h;
        self.total += n;
        self.dirty = true;
        self.counters.written.fetch_add(1, Relaxed);
        if self.journal {
            eprintln!("team-audit: {}", String::from_utf8_lossy(&line[..line.len() - 1]));
        }
        true
    }

    /// Every second and on a flush: the `dropped` record when records were dropped, the sync, the
    /// head file; the head into the journal, retention.
    fn housekeep(&mut self, t: u64, now: bool) {
        let c = &self.counters;
        let seen = (c.queue_full.load(Relaxed), c.log_full.load(Relaxed), c.write_failed.load(Relaxed));
        if seen != self.seen && (now || t >= self.last_dropped + DROPPED_EVERY) {
            let (q, f, w) = (seen.0 - self.seen.0, seen.1 - self.seen.1, seen.2 - self.seen.2);
            self.last_dropped = t;
            eprintln!("team-audit: {} records dropped since {} ({q} with the queue full, {f} with the log full, {w} not written)", q + f + w, self.since);
            let detail = json!({"since": self.since, "queue_full": q, "log_full": f, "write_failed": w});
            // When this one cannot be written either, its counts go into the next one.
            if self.write(Rec { ts: t, ..Rec::new("dropped", None, None, Target::default(), detail) }, t, true) {
                (self.seen, self.since) = (seen, t);
            }
        }
        if self.dirty {
            if let Some(f) = &self.file {
                if let Err(e) = f.sync_data() {
                    self.counters.write_errors.fetch_add(1, Relaxed);
                    eprintln!("error: team-audit: sync: {e}");
                }
            }
            self.dirty = false;
        }
        if self.head_saved != self.prev {
            let tmp = self.dir.join("head.tmp");
            let r = OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).and_then(|mut f| f.write_all(self.prev.as_bytes())).and_then(|_| fs::rename(&tmp, self.dir.join("head")));
            match r {
                Ok(()) => self.head_saved = self.prev.clone(),
                Err(e) => eprintln!("error: team-audit: head: {e}"),
            }
        }
        if t >= self.last_head + HEAD_EVERY {
            self.last_head = t;
            if self.journal {
                eprintln!("team-audit: head {}", self.prev);
            }
        }
        if t >= self.last_prune + PRUNE_EVERY {
            self.last_prune = t;
            self.cap_day = None;
            self.prune(t, 0);
        }
    }

    /// Retention and size cap: whole days, oldest first, never the day being written.
    fn prune(&mut self, t: u64, need: u64) {
        let cutoff = date(t.saturating_sub(self.retention_days.saturating_mul(DAY)));
        let files = files(&self.dir);
        let mut total: u64 = files.iter().map(|(_, n)| n).sum();
        for (day, n) in files {
            if day != self.day && (day < cutoff || total + need > self.max_bytes) && fs::remove_file(self.dir.join(format!("{day}.jsonl"))).is_ok() {
                total -= n;
                eprintln!("team-audit: {day} removed ({})", if day < cutoff { "older than TEAM_AUDIT_RETENTION_DAYS" } else { "TEAM_AUDIT_MAX_MB reached" });
            }
        }
        self.total = total;
    }
}

fn ends_unterminated(f: &mut File) -> io::Result<bool> {
    let len = f.metadata()?.len();
    if len == 0 {
        return Ok(false);
    }
    let mut b = [0u8];
    f.seek(SeekFrom::Start(len - 1))?;
    f.read_exact(&mut b)?;
    Ok(b[0] != b'\n')
}

/// `(day, size)` of every day file, oldest first.
fn files(dir: &Path) -> Vec<(String, u64)> {
    let mut v: Vec<(String, u64)> = fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let day = e.file_name().to_str()?.strip_suffix(".jsonl")?.to_owned();
                    (day.len() == 10 && day.bytes().all(|b| b.is_ascii_digit() || b == b'-')).then_some(())?;
                    Some((day, e.metadata().ok()?.len()))
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// One line as [`next_line`] read it.
struct Line {
    hash: String,
    /// Ended with a newline (else: the end of the file came first).
    complete: bool,
    /// Bytes, without the newline.
    len: u64,
}

/// The next line of `r`: its hash (of all its bytes), and its first LINE_MAX bytes in `buf`.
fn next_line(r: &mut impl BufRead, buf: &mut Vec<u8>) -> io::Result<Option<Line>> {
    buf.clear();
    let (mut h, mut len) = (Sha256::new(), 0u64);
    loop {
        let (used, nl) = {
            let chunk = r.fill_buf()?;
            if chunk.is_empty() {
                return Ok((len > 0).then(|| Line { hash: hex(h), complete: false, len }));
            }
            let end = chunk.iter().position(|&b| b == b'\n');
            let part = &chunk[..end.unwrap_or(chunk.len())];
            h.update(part);
            let room = LINE_MAX.saturating_sub(buf.len());
            buf.extend_from_slice(&part[..part.len().min(room)]);
            len += part.len() as u64;
            (part.len() + end.is_some() as usize, end.is_some())
        };
        r.consume(used);
        if nl {
            return Ok(Some(Line { hash: hex(h), complete: true, len }));
        }
    }
}

/// Hash of the last line of `p` (read from its last TAIL bytes).
fn tail_hash(p: &Path) -> Option<String> {
    let mut f = File::open(p).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(TAIL))).ok()?;
    let mut b = Vec::new();
    f.read_to_end(&mut b).ok()?;
    let b = b.strip_suffix(b"\n").unwrap_or(&b);
    let last = b.rsplit(|&c| c == b'\n').next()?;
    (!last.is_empty()).then(|| hash(last))
}

/// True if `head` is the hash of a line of `p` or the `prev` of its first line.
fn reaches(p: &Path, head: &str) -> bool {
    let Ok(f) = File::open(p) else { return false };
    let (mut r, mut buf, mut first) = (BufReader::new(f), Vec::new(), true);
    while let Ok(Some(l)) = next_line(&mut r, &mut buf) {
        if l.hash == head || (first && serde_json::from_slice::<Value>(&buf).is_ok_and(|v| v["prev"] == head)) {
            return true;
        }
        first = false;
    }
    false
}

/// Where an answer stopped: the next line's day file and offset, and the hash of the line before it.
#[derive(Debug, PartialEq)]
pub struct Cursor {
    day: String,
    off: u64,
    prev: String,
}

impl Cursor {
    fn parse(s: &str) -> Option<Cursor> {
        let mut p = s.splitn(3, ':');
        let (day, off, prev) = (p.next()?, p.next()?.parse().ok()?, p.next()?);
        let ok = day.len() == 10 && day.bytes().all(|b| b.is_ascii_digit() || b == b'-') && prev.len() == 64 && prev.bytes().all(|b| b.is_ascii_hexdigit());
        ok.then(|| Cursor { day: day.into(), off, prev: prev.to_ascii_lowercase() })
    }
}

/// `GET /api/admin/team-audit` parameters.
#[derive(Debug)]
pub struct Query {
    pub since: u64,
    pub until: Option<u64>,
    pub actor: Option<String>,
    pub device: Option<String>,
    pub customer: Option<String>,
    pub event: Option<String>,
    pub limit: usize,
    pub cursor: Option<Cursor>,
}

impl Query {
    pub fn parse(url: &str) -> Result<Query, String> {
        let mut q = Query { since: 0, until: None, actor: None, device: None, customer: None, event: None, limit: 1000, cursor: None };
        for kv in url.split_once('?').map_or("", |(_, q)| q).split('&').filter(|s| !s.is_empty()) {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            let v = crate::audit::unescape(v);
            let bad = || format!("bad {k}");
            match k {
                "since" => q.since = v.parse().map_err(|_| bad())?,
                "until" => q.until = Some(v.parse().map_err(|_| bad())?),
                "limit" => q.limit = v.parse::<usize>().ok().filter(|n| (1..=READ_MAX).contains(n)).ok_or_else(bad)?,
                "cursor" => q.cursor = Some(Cursor::parse(&v).ok_or_else(bad)?),
                "actor" | "device" | "customer" | "event" if v.is_empty() || v.len() > TEXT_MAX => return Err(bad()),
                "actor" => q.actor = Some(v),
                "device" => q.device = Some(v),
                "customer" => q.customer = Some(v),
                "event" => q.event = Some(v),
                _ => return Err(format!("unknown parameter {k}")),
            }
        }
        Ok(q)
    }

    /// The filters as given (for the reader's own record).
    pub fn detail(&self) -> Value {
        let mut d = serde_json::Map::new();
        for (k, v) in [("actor", &self.actor), ("device", &self.device), ("customer", &self.customer), ("event", &self.event)] {
            if let Some(v) = v {
                d.insert(k.into(), v.as_str().into());
            }
        }
        if self.since > 0 {
            d.insert("since".into(), self.since.into());
        }
        if let Some(u) = self.until {
            d.insert("until".into(), u.into());
        }
        if d.is_empty() { Value::Null } else { Value::Object(d) }
    }

    fn matches(&self, v: &Value) -> bool {
        let ts = v["ts"].as_u64().unwrap_or(0);
        let is = |f: &Option<String>, x: &Value| f.as_deref().is_none_or(|f| x.as_str() == Some(f));
        let any = |f: &Option<String>, a: &Value, b: &Value| f.as_deref().is_none_or(|f| a.as_str() == Some(f) || b.as_str() == Some(f));
        ts >= self.since
            && self.until.is_none_or(|u| ts <= u)
            && (any(&self.actor, &v["actor"]["name"], &v["actor"]["key_fingerprint"]) || any(&self.actor, &v["actor"]["oid"], &v["actor"]["upn"]))
            && any(&self.device, &v["target"]["uuid"], &v["target"]["tether_id"])
            && is(&self.customer, &v["target"]["customer"])
            && is(&self.event, &v["event"])
    }
}

/// Writes the answer of `q` over the log in `dir` to `out`, oldest first, read line by line:
/// `{"head","records":[…],"next":<cursor>|null,"problems":[{"day","at","problem"}]}`. Every line
/// read (not only those that match) is checked against the chain; `problems`: `prev-mismatch` (the
/// line before this one was changed or removed, or this one inserted), `unreadable`, `truncated`
/// (no newline), `starts-after-removed-day` (the oldest line read follows a line no longer kept:
/// retention or the size cap), `head-not-found` (the newest lines are not the ones the writer
/// last wrote: removed or changed). Stops at `limit` records, READ_BYTES of records or SCAN_BYTES
/// read, with `next` to continue.
pub fn read(dir: &Path, q: &Query, head: &str, out: &mut impl Write) -> io::Result<()> {
    let files = files(dir);
    let (start, mut expect) = match &q.cursor {
        // A cursor into a day removed since (retention, the size cap): what follows starts after it.
        Some(c) => (c.day.clone(), files.iter().any(|(d, _)| *d == c.day).then(|| c.prev.clone())),
        None => {
            let start = date(q.since);
            let before = files.iter().rev().find(|(d, n)| *d < start && *n > 0);
            (start, before.and_then(|(d, _)| tail_hash(&dir.join(format!("{d}.jsonl")))))
        }
    };
    let end = q.until.map(date);
    let newest = files.last().map(|(d, _)| d.clone());
    write!(out, "{{\"head\":\"{head}\",\"records\":[")?;
    let (mut n, mut bytes, mut scanned) = (0, 0, 0u64);
    let mut problems = Vec::new();
    let mut next = None;
    let (mut head_seen, mut read_to_end) = (head == ZERO, false);
    let mut buf = Vec::new();
    let mut problem = |day: &str, at: u64, what: &str| {
        if problems.len() < PROBLEMS_MAX {
            problems.push(json!({"day": day, "at": at, "problem": what}));
        }
    };
    'files: for (day, _) in files.iter().filter(|(d, _)| *d >= start && end.as_ref().is_none_or(|e| d <= e)) {
        let Ok(f) = File::open(dir.join(format!("{day}.jsonl"))) else { continue }; // removed meanwhile
        let mut r = BufReader::new(f);
        let mut pos = 0;
        if let Some(c) = q.cursor.as_ref().filter(|c| c.day == *day) {
            pos = c.off;
            r.seek(SeekFrom::Start(pos))?;
        }
        loop {
            if n >= q.limit || bytes >= READ_BYTES || scanned >= SCAN_BYTES {
                // Only if there is more to read.
                if r.fill_buf()?.is_empty() && Some(day) == newest.as_ref() {
                    read_to_end = true;
                } else {
                    let (d, off) = if r.fill_buf()?.is_empty() { (files.iter().find(|(d, _)| d > day).map(|(d, _)| d.clone()), 0) } else { (Some(day.clone()), pos) };
                    next = d.map(|d| format!("{d}:{off}:{}", expect.as_deref().unwrap_or(ZERO)));
                }
                break 'files;
            }
            let Some(l) = next_line(&mut r, &mut buf)? else { break };
            let at = pos;
            pos += l.len + l.complete as u64;
            scanned += l.len + 1;
            if !l.complete {
                problem(day, at, "truncated");
            }
            match serde_json::from_slice::<Value>(&buf).ok().filter(|_| l.len as usize <= LINE_MAX) {
                None => problem(day, at, "unreadable"),
                Some(v) => {
                    let prev = v["prev"].as_str().unwrap_or("");
                    match &expect {
                        Some(e) if e != prev => problem(day, at, "prev-mismatch"),
                        None if prev != ZERO => problem(day, at, "starts-after-removed-day"),
                        _ => {}
                    }
                    if q.matches(&v) {
                        if bytes + buf.len() > READ_BYTES {
                            // The next answer starts with this line.
                            next = Some(format!("{day}:{at}:{}", expect.as_deref().unwrap_or(ZERO)));
                            break 'files;
                        }
                        if n > 0 {
                            out.write_all(b",")?;
                        }
                        out.write_all(&buf)?;
                        n += 1;
                        bytes += buf.len();
                    }
                }
            }
            head_seen |= l.hash == head;
            expect = Some(l.hash);
        }
        read_to_end |= Some(day) == newest.as_ref();
    }
    if read_to_end && !head_seen && end.is_none() {
        problem(newest.as_deref().unwrap_or(""), 0, "head-not-found");
    }
    write!(out, "],\"next\":{},\"problems\":{}}}", serde_json::to_string(&next)?, Value::Array(problems))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Posted {
    event: String,
    uuid: Option<String>,
    tether_id: Option<String>,
    what: Option<String>,
    rows: Option<u64>,
    action: Option<String>,
    outcome: Option<String>,
    relay: Option<String>,
}

/// `POST /api/team/audit-event`: what a member's desktop reports it did (`export` with `what`
/// `hosts` | `failing` | `sbom` and `rows`; `session-start` with `tether_id`; `agent-request` with
/// `tether_id`, `action` `install` | `remove` and optionally `outcome`, `[a-z-]{1,32}`); `uuid`
/// optional for each; `token-used` with `relay` (the host that answered `409 token-used` to this
/// member's sign-in: someone else used the person's token there first). Event, target and detail of
/// its record.
pub fn posted(body: &[u8]) -> Result<(&'static str, Target, Value), String> {
    let p: Posted = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    if p.uuid.as_deref().is_some_and(|u| !st::valid_uuid(u)) {
        return Err("bad uuid".into());
    }
    if p.tether_id.as_deref().is_some_and(|t| !st::valid_tether_id(t)) {
        return Err("bad tether_id".into());
    }
    let one_of = |what: &str, v: &Option<String>, set: &[&'static str]| -> Result<&'static str, String> {
        let v = v.as_deref().ok_or_else(|| format!("{what} required"))?;
        set.iter().copied().find(|s| *s == v).ok_or_else(|| format!("bad {what}"))
    };
    let unexpected = |ok: bool, what: &str| if ok { Ok(()) } else { Err(format!("unexpected {what}")) };
    let (event, detail) = match p.event.as_str() {
        "export" => {
            unexpected(p.action.is_none() && p.outcome.is_none() && p.relay.is_none(), "action, outcome or relay")?;
            let what = one_of("what", &p.what, &["hosts", "failing", "sbom"])?;
            let rows = p.rows.filter(|r| *r <= 10_000_000).ok_or("rows required (at most 10000000)")?;
            ("export", json!({"what": what, "rows": rows}))
        }
        "session-start" => {
            unexpected(p.what.is_none() && p.rows.is_none() && p.action.is_none() && p.outcome.is_none() && p.relay.is_none(), "field")?;
            p.tether_id.as_ref().ok_or("tether_id required")?;
            ("session-start", Value::Null)
        }
        "token-used" => {
            unexpected(p.what.is_none() && p.rows.is_none() && p.action.is_none() && p.outcome.is_none() && p.tether_id.is_none() && p.uuid.is_none(), "field")?;
            let r = p.relay.as_deref().ok_or("relay required")?;
            if r.is_empty() || r.len() > 253 || !r.bytes().all(|b| b.is_ascii_alphanumeric() || b"-.:[]".contains(&b)) {
                return Err("bad relay".into());
            }
            ("token-used", json!({"relay": r.to_ascii_lowercase()}))
        }
        "agent-request" => {
            unexpected(p.what.is_none() && p.rows.is_none() && p.relay.is_none(), "what, rows or relay")?;
            p.tether_id.as_ref().ok_or("tether_id required")?;
            let action = one_of("action", &p.action, &["install", "remove"])?;
            let mut d = json!({"action": action});
            if let Some(o) = &p.outcome {
                if o.is_empty() || o.len() > 32 || !o.bytes().all(|b| b.is_ascii_lowercase() || b == b'-') {
                    return Err("bad outcome".into());
                }
                d["outcome"] = o.as_str().into();
            }
            ("agent-request", d)
        }
        _ => return Err("unknown event".into()),
    };
    Ok((event, Target { uuid: p.uuid, tether_id: p.tether_id, customer: None }, detail))
}

/// Tests that measure the process's RSS hold this, and so do tests that allocate many MB.
#[cfg(test)]
pub(crate) static RSS_TEST: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    static SEQ: AtomicU32 = AtomicU32::new(0);
    const T: u64 = 1_794_009_599; // 2026-11-06 23:59:59

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("tether-team-audit-{}-{}", std::process::id(), SEQ.fetch_add(1, Relaxed)));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn writer(dir: &Path, days: u64, max: u64, t: u64) -> Writer {
        Writer::open(dir.to_owned(), days, max, false, Arc::new(Counters::default()), t).unwrap()
    }

    fn rec(ts: u64, name: &str) -> Rec {
        let actor = Actor { key_fingerprint: "0123456789abcdef".into(), name: name.into(), oid: None, upn: None };
        Rec { ts, ..Rec::new("report", Some(actor), Some("192.0.2.1".into()), Target { uuid: Some("dev-0001".into()), tether_id: Some("123456789".into()), customer: Some("4711".into()) }, Value::Null) }.answered(200)
    }

    fn answer(dir: &Path, q: &str, head: &str) -> Value {
        let mut out = Vec::new();
        read(dir, &Query::parse(q).unwrap(), head, &mut out).unwrap();
        serde_json::from_slice(&out).unwrap()
    }

    fn problems(v: &Value) -> Vec<(String, u64, String)> {
        v["problems"].as_array().unwrap().iter().map(|p| (p["day"].as_str().unwrap().into(), p["at"].as_u64().unwrap(), p["problem"].as_str().unwrap().into())).collect()
    }

    fn day_file(dir: &Path, t: u64) -> PathBuf {
        dir.join(format!("{}.jsonl", date(t)))
    }

    #[test]
    fn a_record_has_exactly_its_fields_in_order() {
        let dir = tmp();
        let mut w = writer(&dir, 90, 1 << 30, T);
        w.write(rec(T, "Anna"), T, false);
        let line = fs::read_to_string(day_file(&dir, T)).unwrap();
        assert_eq!(line, format!("{{\"ts\":{T},\"event\":\"report\",\"actor\":{{\"key_fingerprint\":\"0123456789abcdef\",\"name\":\"Anna\"}},\"ip\":\"192.0.2.1\",\"target\":{{\"uuid\":\"dev-0001\",\"tether_id\":\"123456789\",\"customer\":\"4711\"}},\"result\":200,\"prev\":\"{ZERO}\"}}\n"));
        // Refused: only what the request named.
        let r = rec(T, "Anna").answered(404);
        assert_eq!((r.target.uuid.as_deref(), r.target.tether_id, r.target.customer, r.result), (Some("dev-0001"), None, None, Some(404)));
        let r = Rec::new("session-audit-read", None, None, Target::asked("123456789"), Value::Null).answered(403);
        assert_eq!(r.target.tether_id.as_deref(), Some("123456789"));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(day_file(&dir, T)).unwrap().permissions().mode() & 0o777, 0o600);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_chain_verifies_and_shows_edits_cuts_and_removed_days() {
        let dir = tmp();
        let mut w = writer(&dir, 90, 1 << 30, T);
        for (i, ts) in [T - 10, T - 5, T, T + 1, T + 2].into_iter().enumerate() {
            w.write(rec(ts, &format!("m{i}")), ts, false);
        }
        let head = w.prev.clone();
        let (d1, d2) = (date(T), date(T + 1));
        let v = answer(&dir, "/x", &head);
        assert_eq!((v["records"].as_array().unwrap().len(), problems(&v)), (5, vec![]));
        // The first line of the second day chains to the last of the first.
        let first2: Value = serde_json::from_str(fs::read_to_string(day_file(&dir, T + 1)).unwrap().lines().next().unwrap()).unwrap();
        assert_eq!(first2["prev"], tail_hash(&day_file(&dir, T)).unwrap());
        // An edited line: the line after it no longer follows.
        let orig = fs::read_to_string(day_file(&dir, T)).unwrap();
        let second_at = orig.find('\n').unwrap() as u64 + 1;
        let third_at = orig[second_at as usize..].find('\n').unwrap() as u64 + second_at + 1;
        fs::write(day_file(&dir, T), orig.replacen("\"m1\"", "\"mX\"", 1)).unwrap();
        assert_eq!(problems(&answer(&dir, "/x", &head)), vec![(d1.clone(), third_at, "prev-mismatch".into())]);
        // A removed line: the same.
        let lines: Vec<&str> = orig.lines().collect();
        fs::write(day_file(&dir, T), format!("{}\n{}\n", lines[0], lines[2])).unwrap();
        assert_eq!(problems(&answer(&dir, "/x", &head)), vec![(d1.clone(), second_at, "prev-mismatch".into())]);
        // The end of a day cut off: the next day's first line shows it.
        fs::write(day_file(&dir, T), format!("{}\n{}\n", lines[0], lines[1])).unwrap();
        assert_eq!(problems(&answer(&dir, "/x", &head)), vec![(d2.clone(), 0, "prev-mismatch".into())]);
        // A cut last line (crash): reported, the rest still chains.
        fs::write(day_file(&dir, T), format!("{}\n{}\n{}", lines[0], lines[1], &lines[2][..20])).unwrap();
        let p = problems(&answer(&dir, "/x", &head));
        assert_eq!(p[..2].to_vec(), vec![(d1.clone(), third_at, "truncated".into()), (d1.clone(), third_at, "unreadable".into())]);
        // A removed day (retention): the rest verifies, starting after it.
        fs::remove_file(day_file(&dir, T)).unwrap();
        assert_eq!(problems(&answer(&dir, "/x", &head)), vec![(d2.clone(), 0, "starts-after-removed-day".into())]);
        // The newest lines removed: only a head known elsewhere shows it.
        let day2 = fs::read_to_string(day_file(&dir, T + 1)).unwrap();
        fs::write(day_file(&dir, T + 1), day2.lines().next().unwrap().to_owned() + "\n").unwrap();
        assert_eq!(problems(&answer(&dir, "/x", &head)), vec![(d2.clone(), 0, "starts-after-removed-day".into()), (d2.clone(), 0, "head-not-found".into())]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_restart_continues_the_chain_and_records_a_gap() {
        let dir = tmp();
        let mut w = writer(&dir, 90, 1 << 30, T);
        w.write(rec(T - 5, "a"), T, false);
        w.write(rec(T - 4, "b"), T, false);
        w.housekeep(T, true);
        let head = w.prev.clone();
        assert_eq!(fs::read_to_string(dir.join("head")).unwrap(), head);
        drop(w);
        let mut w = writer(&dir, 90, 1 << 30, T);
        assert_eq!(w.prev, head);
        w.write(rec(T - 3, "c"), T, false);
        w.housekeep(T, true);
        let v = answer(&dir, "/x", &w.prev);
        assert_eq!((v["records"].as_array().unwrap().len(), problems(&v)), (3, vec![]));
        drop(w);
        // The last line removed while the relay was down: the restart says so, in the chain.
        let s = fs::read_to_string(day_file(&dir, T)).unwrap();
        let cut: String = s.lines().take(2).map(|l| format!("{l}\n")).collect();
        fs::write(day_file(&dir, T), &cut).unwrap();
        let mut w = writer(&dir, 90, 1 << 30, T);
        let last: Value = serde_json::from_str(fs::read_to_string(day_file(&dir, T)).unwrap().lines().last().unwrap()).unwrap();
        assert_eq!((last["event"].as_str(), last["detail"]["found"].as_str()), (Some("chain-gap"), Some(tail_hash_of(&cut).as_str())));
        assert!(problems(&answer(&dir, "/x", &w.prev)).is_empty());
        w.housekeep(T, true);
        drop(w);
        // A crash in the middle of a line: the next line starts on its own.
        let mut f = OpenOptions::new().append(true).open(day_file(&dir, T)).unwrap();
        f.write_all(b"{\"ts\":1").unwrap();
        let mut w = writer(&dir, 90, 1 << 30, T);
        w.write(rec(T, "d"), T, false);
        let s = fs::read_to_string(day_file(&dir, T)).unwrap();
        assert!(s.contains("{\"ts\":1\n{\"ts\""), "{s}");
        let p = problems(&answer(&dir, "/x", &w.prev));
        assert_eq!(p.iter().map(|p| p.2.as_str()).collect::<Vec<_>>(), ["unreadable"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    fn tail_hash_of(s: &str) -> String {
        hash(s.lines().last().unwrap().as_bytes())
    }

    #[test]
    fn retention_and_the_size_cap_remove_whole_old_days_never_today() {
        let dir = tmp();
        for d in [100, 91, 5, 1] {
            fs::write(dir.join(format!("{}.jsonl", date(T - d * DAY))), vec![b'x'; 1000]).unwrap();
        }
        let mut w = writer(&dir, 90, 1 << 30, T);
        let left = |dir: &Path| files(dir).into_iter().map(|(d, _)| d).collect::<Vec<_>>();
        assert_eq!(left(&dir), [date(T - 5 * DAY), date(T - DAY)], "older than 90 days");
        // Cap 2200 bytes: writing today removes the oldest day first.
        w.max_bytes = 2200;
        w.write(rec(T, "a"), T, false);
        assert_eq!(left(&dir), [date(T - DAY), date(T)]);
        w.write(rec(T, "b"), T, false);
        // Today alone over the cap: records are dropped and counted, today's file stays.
        w.max_bytes = 1;
        let size = fs::metadata(day_file(&dir, T)).unwrap().len();
        w.write(rec(T, "c"), T, false);
        assert_eq!((left(&dir), fs::metadata(day_file(&dir, T)).unwrap().len(), w.counters.log_full.load(Relaxed)), (vec![date(T)], size, 1));
        // The writer's own note of it goes in.
        w.housekeep(T, true);
        let last: Value = serde_json::from_str(fs::read_to_string(day_file(&dir, T)).unwrap().lines().last().unwrap()).unwrap();
        assert_eq!((last["event"].as_str(), last["detail"].clone()), (Some("dropped"), json!({"since": T, "queue_full": 0, "log_full": 1, "write_failed": 0})));
        // At the cap the folder is not scanned again for each dropped record that day.
        assert_eq!(w.cap_day.as_deref(), Some(date(T).as_str()));
        fs::remove_dir_all(&dir).unwrap();
    }

    /// A write that fails halfway (disk full): the file is cut back to its length before, the
    /// record counted as dropped and reported, and the chain still verifies.
    #[test]
    fn a_failed_write_is_cut_back_counted_and_reported() {
        let dir = tmp();
        let mut w = writer(&dir, 90, 1 << 30, T - 100);
        assert!(w.write(rec(T - 50, "a"), T - 50, false));
        let before = fs::read(day_file(&dir, T)).unwrap();
        w.put = |mut f, b| {
            f.write_all(&b[..b.len() / 2])?;
            Err(io::Error::other("no space left on device"))
        };
        assert!(!w.write(rec(T - 40, "b"), T - 40, false));
        assert!(!w.write(rec(T - 39, "c"), T - 39, false));
        assert_eq!(fs::read(day_file(&dir, T)).unwrap(), before, "no torn line stays");
        assert_eq!(w.counters.write_failed.load(Relaxed), 2);
        // The dropped record cannot be written either: its counts go into the next one.
        w.housekeep(T - 30, true);
        assert_eq!(fs::read(day_file(&dir, T)).unwrap(), before);
        w.put = |mut f, b| f.write_all(b);
        w.housekeep(T - 20, true);
        let last: Value = serde_json::from_str(fs::read_to_string(day_file(&dir, T)).unwrap().lines().last().unwrap()).unwrap();
        assert_eq!((last["event"].as_str(), last["detail"].clone()), (Some("dropped"), json!({"since": T - 100, "queue_full": 0, "log_full": 0, "write_failed": 3})));
        assert!(w.write(rec(T - 10, "d"), T - 10, false));
        assert!(problems(&answer(&dir, "/x", &w.prev)).is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn folded_reads_are_written_each_hour_with_their_count() {
        let mut f = Folded::default();
        let actor = |n: &str| Actor { key_fingerprint: format!("{n:0>16}"), name: n.into(), oid: None, upn: None };
        let reads = |r: &Rec| r.detail["reads"].as_u64().unwrap();
        // The first read is recorded at once; the next ones of the hour are counted.
        assert_eq!(f.read("summary", actor("a"), None, T).map(|r| reads(&r)), Some(1));
        for i in 1..=4 {
            assert!(f.read("summary", actor("a"), Some("192.0.2.9".into()), T + i * 60).is_none());
        }
        assert!(f.read("device-list", actor("a"), None, T + 300).is_some(), "another event, its own record");
        assert!(f.due(T + 3599, false).is_empty(), "not an hour yet");
        // The hour is over: maintenance writes the 4 reads, with the last read's address.
        let d = f.due(T + 3600, false);
        assert_eq!(d.iter().map(|r| (r.event, reads(r), r.ip.clone())).collect::<Vec<_>>(), [("summary", 4, Some("192.0.2.9".to_owned()))]);
        // Reads in the next hour: counted, and at a stop written whatever the hour.
        assert!(f.read("summary", actor("a"), None, T + 3700).is_none());
        assert!(f.read("summary", actor("a"), None, T + 3800).is_none());
        assert_eq!(f.due(T + 3900, true).iter().map(reads).collect::<Vec<_>>(), [2]);
        // An actor unseen for a day is forgotten; its next read is recorded at once.
        assert!(f.due(T + 3800 + DAY, false).is_empty());
        assert!(f.0.is_empty());
        assert_eq!(f.read("summary", actor("a"), None, T + 3800 + DAY).map(|r| reads(&r)), Some(1));
    }

    #[test]
    fn a_cursor_into_a_removed_day_starts_after_it() {
        let dir = tmp();
        let mut w = writer(&dir, 90, 1 << 30, T);
        for ts in [T - 3, T - 2, T - 1, T + 1, T + 2] {
            w.write(rec(ts, "a"), ts, false);
        }
        let a = answer(&dir, "/x?limit=1", &w.prev);
        let c = a["next"].as_str().unwrap().to_owned();
        assert!(c.starts_with(&date(T)));
        fs::remove_file(day_file(&dir, T)).unwrap();
        let b = answer(&dir, &format!("/x?cursor={c}"), &w.prev);
        assert_eq!((b["records"].as_array().unwrap().len(), problems(&b)), (2, vec![(date(T + 1), 0, "starts-after-removed-day".to_owned())]));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_reader_filters_and_pages_with_a_cursor() {
        let dir = tmp();
        let mut w = writer(&dir, 90, 1 << 30, T);
        for i in 0..6u64 {
            let mut r = rec(T - 2 + i, if i % 2 == 0 { "Anna" } else { "Ben" });
            r.target.customer = Some(if i < 3 { "4711" } else { "0815" }.into());
            w.write(r, T, false);
        }
        let head = w.prev.clone();
        let names = |v: &Value| v["records"].as_array().unwrap().iter().map(|r| r["ts"].as_u64().unwrap() - (T - 2)).collect::<Vec<_>>();
        assert_eq!(names(&answer(&dir, "/x?actor=Anna", &head)), [0, 2, 4]);
        assert_eq!(names(&answer(&dir, "/x?actor=0123456789abcdef&customer=0815", &head)), [3, 4, 5]);
        assert_eq!(names(&answer(&dir, &format!("/x?since={}&until={}", T - 1, T + 1), &head)), [1, 2, 3]);
        assert_eq!(names(&answer(&dir, "/x?device=123456789&event=report", &head)).len(), 6);
        assert_eq!(names(&answer(&dir, "/x?event=summary", &head)).len(), 0);
        // Pages of 4 across the two days, the chain checked across the page boundary.
        let a = answer(&dir, "/x?limit=4", &head);
        assert_eq!(names(&a), [0, 1, 2, 3]);
        let c = a["next"].as_str().unwrap();
        let b = answer(&dir, &format!("/x?limit=4&cursor={c}"), &head);
        assert_eq!((names(&b), b["next"].clone(), problems(&b)), (vec![4, 5], Value::Null, vec![]));
        let c = c.replacen(':', ":x", 1);
        assert!(Query::parse(&format!("/x?cursor={c}")).is_err());
        for bad in ["/x?limit=0", "/x?limit=5001", "/x?since=a", "/x?nope=1", "/x?actor="] {
            assert!(Query::parse(bad).is_err(), "{bad}");
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn member_posted_events_are_a_closed_set() {
        let ok = |b: Value| posted(b.to_string().as_bytes());
        assert_eq!(ok(json!({"event": "export", "what": "hosts", "rows": 12})).unwrap().2, json!({"what": "hosts", "rows": 12}));
        assert_eq!(ok(json!({"event": "session-start", "tether_id": "123456789"})).unwrap().1.tether_id.as_deref(), Some("123456789"));
        assert_eq!(ok(json!({"event": "agent-request", "tether_id": "123456789", "action": "install", "outcome": "not-allowed"})).unwrap().2, json!({"action": "install", "outcome": "not-allowed"}));
        assert_eq!(ok(json!({"event": "token-used", "relay": "Relay2.example.com"})).unwrap(), ("token-used", Target { uuid: None, tether_id: None, customer: None }, json!({"relay": "relay2.example.com"})));
        for bad in [json!({"event": "token-used"}), json!({"event": "token-used", "relay": "a b"}), json!({"event": "token-used", "relay": "x", "tether_id": "123456789"}), json!({"event": "export", "what": "hosts", "rows": 1, "relay": "x"})] {
            assert!(ok(bad.clone()).is_err(), "{bad}");
        }
        for bad in [
            json!({"event": "login"}),
            json!({"event": "export", "what": "all", "rows": 1}),
            json!({"event": "export", "what": "hosts"}),
            json!({"event": "export", "what": "hosts", "rows": 1, "hosts": ["PC-1"]}),
            json!({"event": "session-start"}),
            json!({"event": "session-start", "tether_id": "123456789", "rows": 1}),
            json!({"event": "agent-request", "tether_id": "123456789", "action": "run"}),
            json!({"event": "agent-request", "tether_id": "123456789", "action": "install", "outcome": "x".repeat(33)}),
            json!({"event": "agent-request", "tether_id": "123456789", "action": "install", "outcome": "Done!"}),
            json!({"event": "export", "what": "sbom", "rows": 1, "uuid": "../x"}),
        ] {
            assert!(ok(bad.clone()).is_err(), "{bad}");
        }
    }

    fn rss_kb() -> u64 {
        let s = fs::read_to_string("/proc/self/status").unwrap();
        s.lines().find_map(|l| l.strip_prefix("VmRSS:")?.split_whitespace().next()?.parse().ok()).unwrap()
    }

    /// 50 000 records at once: submitting never waits, memory stays flat (the queue holds at
    /// most QUEUE), every record is written or counted as dropped, and the chain verifies.
    #[test]
    fn a_burst_of_50000_records_never_blocks_and_keeps_memory_flat() {
        let _alone = RSS_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tmp();
        let log = Log::start(dir.clone(), 90, 2048 << 20, false).unwrap();
        let base = rss_kb();
        let peak = Arc::new(AtomicU64::new(base));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let sampler = {
            let (peak, stop) = (peak.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(Relaxed) {
                    peak.fetch_max(rss_kb(), Relaxed);
                    std::thread::sleep(Duration::from_millis(2));
                }
            })
        };
        let slowest = std::thread::scope(|sc| {
            let hs: Vec<_> = (0..4)
                .map(|_| {
                    sc.spawn(|| {
                        let mut slowest = Duration::ZERO;
                        for i in 0..12_500 {
                            let t = Instant::now();
                            log.submit(rec(crate::now(), &format!("member {i}")));
                            slowest = slowest.max(t.elapsed());
                        }
                        slowest
                    })
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).max().unwrap()
        });
        // A flush is not queued while the queue is full.
        let t = Instant::now();
        while !log.flush(Duration::from_secs(30)) {
            assert!(t.elapsed() < Duration::from_secs(60));
            std::thread::sleep(Duration::from_millis(10));
        }
        let drained = t.elapsed();
        stop.store(true, Relaxed);
        sampler.join().unwrap();
        let growth = peak.load(Relaxed).saturating_sub(base);
        let (written, dropped) = (log.counters.written.load(Relaxed), log.counters.queue_full.load(Relaxed));
        let mut lines = 0u64;
        let mut reported = 0u64;
        let mut size = 0u64;
        for (d, n) in files(&dir) {
            size += n;
            for l in fs::read_to_string(dir.join(format!("{d}.jsonl"))).unwrap().lines() {
                let v: Value = serde_json::from_str(l).unwrap();
                if v["event"] == "dropped" {
                    reported += v["detail"]["queue_full"].as_u64().unwrap();
                } else {
                    lines += 1;
                }
            }
        }
        println!("team-audit burst: 50000 submitted, {lines} written, {dropped} dropped (reported {reported}); slowest submit {slowest:?}, queue drained {drained:?} after the last; RSS {base} kB, peak +{growth} kB; {} bytes per record", size / written.max(1));
        assert_eq!((lines + reported, lines + dropped), (50_000, 50_000));
        assert!(written >= lines);
        assert!(slowest < Duration::from_millis(50), "{slowest:?}");
        assert!(growth < 24 << 10, "{growth} kB");
        let mut q = Query::parse("/x?limit=5000").unwrap();
        let mut pages = 0;
        loop {
            let mut out = Vec::new();
            read(&dir, &q, &log.head(), &mut out).unwrap();
            let v: Value = serde_json::from_slice(&out).unwrap();
            assert!(problems(&v).is_empty(), "{:?}", problems(&v));
            pages += 1;
            match v["next"].as_str() {
                Some(c) => q.cursor = Cursor::parse(c),
                None => break,
            }
        }
        assert!(pages >= (lines / 5000) as usize);
        fs::remove_dir_all(&dir).unwrap();
    }
}

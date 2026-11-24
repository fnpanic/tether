//! Device status: devices enrol with an admin-signed token (`POST /api/status/enroll`, pins the
//! uuid to the device key; a Tether ID only from a token issued for it, else it is a claim), then
//! post device-signed reports and heartbeats (`POST /api/status`, `tether_trust::status::Report`);
//! roster members read a summary of all hosts and one host's last report with its history
//! (`/api/team/status[/<uuid>]`), and members with `write` confirm a claim, withdraw a link or
//! remove a device. Admins revoke tokens and list the pins. docs/api.md.
//!
//! Files (`status/`, private): `keys.jsonl` (the pins: one `set` line per enrolment, link or
//! unlink, `used` per first use of a token issued for one Tether ID, `unpin` per removal; earlier
//! `pin` lines are read; appended and synced, compacted in the background), `revoked.json` (revoked
//! token id → exp, written by the status writer; unreadable, it locks the store), `<uuid>.json` (the last full
//! report, byte-exact as received), `<uuid>.history.jsonl` (one line per state change, appended),
//! `seen.json` {uuid: last post}, `summary.json` (the team summary, rebuilt at most every 30 s),
//! `<uuid>.sbom.json` (the device's software list with `STATUS_SBOM=on`, byte-exact as received;
//! its record, time, size and hash, is kept with the device's entry in `seen.json`).
//!
//! The relay also carries session signalling and Tether's updates and has one state lock, so
//! nothing here does work in proportion to the number of devices while holding it: heartbeats
//! touch no file, a full report is written and synced before the lock is taken again, and
//! `seen.json`, the summary, the compaction of `keys.jsonl` and history trimming are written from
//! a snapshot or a file handed over under the lock, by the status writer thread or after the
//! lock is released. The relay sets the devices' pace (`next`), the cheap checks answer before
//! the signature and the parser run, and `STATUS=off` turns all of it off.

use std::borrow::Borrow;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::ops::Bound;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::de::{Deserializer, IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tether_trust::status::{self as st, Answer, Change, Counts, Fault, Product, Report};
use tether_trust::reqauth;
use tiny_http::Request;

use crate::{empty, header, hdr, json, load_json, lock, mkdirs, not_found, now, rate_ok, text, App, Body, Resp, DAY};

pub const LIMIT_STATUS: u64 = st::REPORT_MAX as u64;
/// The team summary is rebuilt at most this often (and not sooner after a failed build).
const SUMMARY_SECS: u64 = 30;
/// `seen.json` is written at most this often (stale needs at least 900 s; a hard kill loses at
/// most this much of last-seen data).
const SEEN_SECS: u64 = 300;
const HISTORY_LINES: usize = 2000;
/// A history file above this size is trimmed in the background right after the append.
const HISTORY_TRIM_BYTES: u64 = 256 << 10;
/// Products and faults kept in a device's summary entry (in memory); the report has them all.
const SUMMARY_PRODUCTS: usize = 8;
const SUMMARY_FAULTS: usize = 10;
/// `keys.jsonl` is compacted when it has more lines than this many times the live pins.
const KEYS_LOG_FACTOR: usize = 2;
const KEYS_LOG_MIN: usize = 32;
/// The daily history pass: pause between files (a few files per second).
const TRIM_PAUSE: Duration = Duration::from_millis(250);

/// Failed background writes since start (`tether_api_status_write_errors`).
pub static WRITE_ERRORS: AtomicU64 = AtomicU64::new(0);
static WRITE_ERROR_LOGGED: AtomicU64 = AtomicU64::new(0);

/// A failed status write: counted, logged at most once a minute.
fn write_error(what: &str, e: impl fmt::Display) {
    WRITE_ERRORS.fetch_add(1, Ordering::Relaxed);
    let t = now();
    let last = WRITE_ERROR_LOGGED.load(Ordering::Relaxed);
    if t >= last + 60 && WRITE_ERROR_LOGGED.compare_exchange(last, t, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
        eprintln!("error: status: {what}: {e} (further errors within a minute are only counted)");
    }
}
/// Accepted posts per uuid: one per this many seconds.
const POST_INTERVAL: u64 = 60;
/// Failed device signatures (and unknown uuids) per IP and minute.
const FAIL_PER_MIN: u32 = 60;
const ENROLL_PER_MIN: u32 = 30;
/// Correctly signed but refused posts (400/409) per uuid and minute.
const REFUSED_PER_MIN: u32 = 10;
const HISTORY_DAYS: u64 = 90;
/// Accepted software lists per uuid: one per this many seconds.
const SBOM_INTERVAL: u64 = 3600;

/// Relay settings (api.env): `STATUS=off`, `STATUS_TARGET_RPS` (default 10, at least 1),
/// `STATUS_MAX_DEVICES` (default 20 000, the size load-tested within `MemoryMax=64M`); the
/// software list: `STATUS_SBOM=on` (off by default), `STATUS_SBOM_MAX_KB` (default 512),
/// `STATUS_SBOM_RPS` (accepted for checking per second, all devices; default 2, at least 1),
/// `STATUS_SBOM_MAX_TOTAL_MB` (default 2048).
pub struct Conf {
    pub off: bool,
    pub rps: u64,
    pub max_devices: usize,
    pub sbom: bool,
    /// Bytes.
    pub sbom_max: u64,
    pub sbom_rps: u64,
    /// Bytes.
    pub sbom_total: u64,
}

static CONF: OnceLock<Conf> = OnceLock::new();

/// Set once by `main` from the environment.
pub fn set_conf(c: Conf) {
    let _ = CONF.set(c);
}

pub fn conf() -> &'static Conf {
    CONF.get_or_init(|| Conf { off: false, rps: 10, max_devices: 20_000, sbom: false, sbom_max: 512 << 10, sbom_rps: 2, sbom_total: 2048 << 20 })
}

/// `min(3600, max(300, ceil(enrolled / rps)))`: the relay's budget spread over its devices.
pub fn next_secs(enrolled: usize, rps: u64) -> u64 {
    (enrolled as u64).div_ceil(rps.max(1)).clamp(300, 3600)
}

/// Every status route while `STATUS=off`.
pub(crate) fn off() -> Resp {
    text(503, "status reporting is off").with_header(hdr("Retry-After", "3600"))
}

/// 0 ok (also unknown), 1 warn, 2 crit: index into the `tether_api_status_hosts` gauges.
fn level(overall: &str) -> usize {
    match overall {
        "warn" => 1,
        "crit" => 2,
        _ => 0,
    }
}

/// A device's pinned key (`keys.jsonl`; the earlier `keys.json` was {uuid: pin}).
#[derive(Serialize, Deserialize, Clone, Default, PartialEq)]
struct Pin {
    key: String,
    /// Verified: from a token issued for it, or confirmed by a team member. Never with `claimed_id`.
    tether_id: Option<String>,
    label: String,
    enrolled: u64,
    /// Id of the token it enrolled with (None: pins from before token ids).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    token: Option<String>,
    /// The Tether ID the device named with a token not issued for one; nobody confirmed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    claimed_id: Option<String>,
    /// Who confirmed `tether_id` (roster name; None for a token issued for it) and when.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    linked_by: Option<String>,
    /// When the link was made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    linked_at: Option<u64>,
    /// The token issued for `tether_id` that made the link (None: a member linked it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    linked_token: Option<String>,
    /// A member unlinked it: the device cannot claim an ID again (not by enrolling, not by a
    /// report) until a member allows a new claim.
    #[serde(default, skip_serializing_if = "is_false")]
    no_claim: bool,
    /// The customer reference an enrolment token set (`valid_customer`); a device cannot name it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    customer: Option<String>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// One line of `keys.jsonl`. `pin` lines (from before token ids) are only read; a `set` line
/// carries a pin's whole record (enrolment, link, unlink) and replaces the earlier one.
#[derive(Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase", deny_unknown_fields)]
enum KeyLine {
    Pin {
        uuid: String,
        key: String,
        tether_id: Option<String>,
        label: String,
        enrolled: u64,
    },
    Unpin {
        uuid: String,
    },
    Set {
        uuid: String,
        key: String,
        tether_id: Option<String>,
        label: String,
        enrolled: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        claimed_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        linked_by: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        linked_at: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        linked_token: Option<String>,
        #[serde(default, skip_serializing_if = "is_false")]
        no_claim: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        customer: Option<String>,
    },
    /// A token issued for one Tether ID enrolled `uuid`: it enrols no other device (also after that
    /// pin is removed) until `exp`.
    Used {
        token: String,
        uuid: String,
        exp: u64,
    },
    /// A device whose link a member dismissed released its own pin: its key enrols again only
    /// without a claim (`no_claim`), with a token not issued for a Tether ID.
    Dismissed {
        key: String,
        at: u64,
    },
    /// That record is used up (the key enrolled again, or a token issued for an ID linked it).
    Forgotten {
        key: String,
    },
}

impl KeyLine {
    fn set(uuid: &str, p: &Pin) -> Self {
        let p = p.clone();
        KeyLine::Set { uuid: uuid.to_owned(), key: p.key, tether_id: p.tether_id, label: p.label, enrolled: p.enrolled, token: p.token, claimed_id: p.claimed_id, linked_by: p.linked_by, linked_at: p.linked_at, linked_token: p.linked_token, no_claim: p.no_claim, customer: p.customer }
    }
}

/// A pin from before verified IDs (`pin` lines, `keys.json`): the Tether ID the device named was
/// never checked, so it is only a claim.
fn legacy(mut p: Pin) -> Pin {
    if p.tether_id.is_some() && p.linked_by.is_none() && p.linked_token.is_none() {
        p.claimed_id = p.tether_id.take();
    }
    p
}

/// Tokens issued for one Tether ID that were used: token id → (uuid, the token's `exp`).
type Used = BTreeMap<String, (String, u64)>;
/// Device keys that released their own pin while their link was dismissed: key → when.
type Dismissed = BTreeMap<String, u64>;
/// Dismissed keys kept at most (the oldest go first).
const DISMISSED_MAX: usize = 10_000;

/// Replays `keys.jsonl`: the pins, the used bound tokens, the number of lines, and the length up
/// to the last complete line (a torn last line from a crash is dropped). Any other unreadable line
/// is an error.
#[allow(clippy::type_complexity)]
fn replay_keys(b: &[u8]) -> Result<(BTreeMap<String, Pin>, Used, Dismissed, usize, usize), String> {
    let good = b.iter().rposition(|&c| c == b'\n').map_or(0, |i| i + 1);
    let (mut keys, mut used, mut dismissed) = (BTreeMap::new(), Used::new(), Dismissed::new());
    let mut n = 0;
    for (i, line) in b[..good].split(|&c| c == b'\n').enumerate().filter(|(_, l)| !l.is_empty()) {
        n += 1;
        match serde_json::from_slice(line).map_err(|e| format!("line {}: {e}", i + 1))? {
            KeyLine::Pin { uuid, key, tether_id, label, enrolled } => {
                keys.insert(uuid, legacy(Pin { key, tether_id, label, enrolled, ..Default::default() }));
            }
            KeyLine::Unpin { uuid } => {
                keys.remove(&uuid);
            }
            KeyLine::Set { uuid, key, tether_id, label, enrolled, token, claimed_id, linked_by, linked_at, linked_token, no_claim, customer } => {
                keys.insert(uuid, Pin { key, tether_id, label, enrolled, token, claimed_id, linked_by, linked_at, linked_token, no_claim, customer });
            }
            KeyLine::Used { token, uuid, exp } => {
                used.insert(token, (uuid, exp));
            }
            KeyLine::Dismissed { key, at } => {
                dismissed.insert(key, at);
            }
            KeyLine::Forgotten { key } => {
                dismissed.remove(&key);
            }
        }
    }
    Ok((keys, used, dismissed, n, good))
}

/// The most revoked tokens (not expired) a relay keeps.
const REVOKED_MAX: usize = 10_000;

fn key_lines(lines: &[KeyLine]) -> Vec<u8> {
    let mut out = Vec::new();
    for l in lines {
        let _ = serde_json::to_writer(&mut out, l);
        out.push(b'\n');
    }
    out
}

/// Writes `bytes` to a new file at `tmp` and syncs it.
fn write_synced(tmp: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut f = OpenOptions::new().write(true).create_new(true).mode(0o600).open(tmp)?;
    f.write_all(bytes)?;
    f.sync_all()
}

/// `seen.json`: the last accepted post of a device.
#[derive(Serialize, Deserialize, Clone, Default)]
struct Seen {
    last_seen: u64,
    seq: u64,
    /// The `next` the relay answered that post with (stale is measured against it).
    next: u64,
    /// The summary entry's static part from the last full report, compact JSON object
    /// (host, agent, overall, counts, products, faults; bounded, see [`Entry`]). None: no full
    /// report yet. Shared, so a snapshot copies no entry.
    #[serde(default)]
    entry: Option<Arc<str>>,
    /// `level` of the entry's `overall` (kept so a metrics scrape parses nothing).
    #[serde(skip)]
    level: usize,
    /// The stored software list (`<uuid>.sbom.json`). A device may have one before its first
    /// report: then `last_seen` is 0 and there is no `entry`.
    #[serde(default)]
    sbom: Option<Sbom>,
}

/// A stored software list: when it was accepted, its size and the first 16 hex digits of its
/// SHA-256 (the `ETag` and the summary's `sbom.sha`).
#[derive(Serialize, Deserialize, Clone, PartialEq)]
struct Sbom {
    at: u64,
    size: u64,
    sha: String,
}

impl Seen {
    /// The entry, if it is a usable JSON object.
    fn entry(&self) -> Option<&str> {
        self.entry.as_deref().filter(|e| e.len() > 2 && e.starts_with('{'))
    }
}

/// The summary entry's static part: at most 8 products and 10 faults (the first ones, the agent
/// sends the worst first), with the number left out; the stored report has them all.
#[derive(Serialize)]
struct Entry<'a> {
    host: &'a str,
    agent: &'a str,
    overall: &'a str,
    counts: &'a Counts,
    products: &'a [Product],
    faults: &'a [Fault],
    #[serde(skip_serializing_if = "is_zero")]
    more_products: usize,
    #[serde(skip_serializing_if = "is_zero")]
    more_faults: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// A summary entry is at most this many bytes, whatever a device sends: the report rules count
/// characters, and 20 000 entries stay in memory (and are reloaded from `seen.json` at start).
const ENTRY_MAX: usize = 2048;

/// The summary entry as compact JSON, at most `ENTRY_MAX` bytes: the first 8 products and 10
/// faults, then faults dropped from the end, then products, then `host` and `agent` cut (on
/// character boundaries) until it fits. `more_*` count what was left out, plus `more_p`/`more_f`
/// left out before.
#[allow(clippy::too_many_arguments)]
fn entry_json(host: &str, agent: &str, overall: &str, counts: &Counts, products: &[Product], faults: &[Fault], more_p: usize, more_f: usize) -> String {
    let (mut np, mut nf) = (products.len().min(SUMMARY_PRODUCTS), faults.len().min(SUMMARY_FAULTS));
    let (mut host, mut agent) = (host.to_owned(), agent.to_owned());
    loop {
        let e = Entry { host: &host, agent: &agent, overall, counts, products: &products[..np], faults: &faults[..nf], more_products: more_p + products.len() - np, more_faults: more_f + faults.len() - nf };
        let s = serde_json::to_string(&e).unwrap_or_default();
        if s.len() <= ENTRY_MAX || (host.is_empty() && agent.is_empty() && np == 0 && nf == 0) {
            return s;
        }
        if nf > 0 {
            nf -= 1;
        } else if np > 0 {
            np -= 1;
        } else {
            let half = |s: &mut String| *s = s.chars().take(s.chars().count() / 2).collect();
            if host.len() >= agent.len() {
                half(&mut host);
            } else {
                half(&mut agent);
            }
        }
    }
}

/// An entry loaded from `seen.json` cut to `ENTRY_MAX`; None if it cannot be read or cut.
fn recap_entry(e: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Owned {
        host: String,
        agent: String,
        overall: String,
        counts: Counts,
        #[serde(default)]
        products: Vec<Product>,
        #[serde(default)]
        faults: Vec<Fault>,
        #[serde(default)]
        more_products: usize,
        #[serde(default)]
        more_faults: usize,
    }
    let o: Owned = serde_json::from_str(e).ok()?;
    let s = entry_json(&o.host, &o.agent, &o.overall, &o.counts, &o.products, &o.faults, o.more_products, o.more_faults);
    (s.len() <= ENTRY_MAX).then_some(s)
}

/// Appends `bytes` through `write`; on an error the file is cut back to its length before, so a
/// partly written line (a full disk) is never left for the next append to be glued onto.
fn append_whole(f: &fs::File, bytes: &[u8], write: impl FnOnce(&fs::File, &[u8]) -> io::Result<()>) -> io::Result<()> {
    let len = f.metadata()?.len();
    write(f, bytes).inspect_err(|_| {
        let _ = f.set_len(len);
    })
}

/// A full report's files: its history lines appended to `hist` (if any), then the report renamed
/// from `tmp` to `dest`. On any error the history is cut back to its length before, so it never
/// holds changes of a report that was not stored.
fn append_then_rename(hist: Option<&fs::File>, lines: &[u8], tmp: &Path, dest: &Path) -> io::Result<()> {
    match hist {
        Some(f) => append_whole(f, lines, |mut f, b| {
            f.write_all(b)?;
            fs::rename(tmp, dest)
        }),
        None => fs::rename(tmp, dest),
    }
}

#[derive(Serialize)]
struct HistoryLine<'a> {
    #[serde(flatten)]
    change: &'a Change,
    at: u64,
}

/// A status post as the relay reads it: [`Report`] with `body` checked to be a JSON object and
/// skipped, never built into a value (it is kept only in the raw bytes stored).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Post {
    v: u32,
    uuid: String,
    #[serde(default)]
    tether_id: Option<String>,
    host: String,
    seq: u64,
    time: String,
    next: u32,
    agent: String,
    packs: std::collections::BTreeMap<String, u64>,
    overall: String,
    counts: Counts,
    faults: Vec<Fault>,
    #[serde(default)]
    products: Option<Vec<Product>>,
    #[serde(default)]
    changes: Option<Vec<Change>>,
    #[serde(default)]
    body: Option<Opaque>,
}

struct Opaque;

impl<'de> Deserialize<'de> for Opaque {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Opaque;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Opaque, A::Error> {
                while m.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(Opaque)
            }
        }
        d.deserialize_map(V)
    }
}

impl Post {
    /// The contract's field rules, from [`Report::validate`] (body: an empty object stands in).
    fn validate(self) -> Result<Report, String> {
        let r = Report {
            v: self.v,
            uuid: self.uuid,
            tether_id: self.tether_id,
            host: self.host,
            seq: self.seq,
            time: self.time,
            next: self.next,
            agent: self.agent,
            packs: self.packs,
            overall: self.overall,
            counts: self.counts,
            faults: self.faults,
            products: self.products,
            changes: self.changes,
            body: self.body.map(|_| serde_json::Value::Object(Default::default())),
        };
        r.validate().map(|()| r)
    }
}

/// Work for the status writer thread ([`start_writer`]).
pub(crate) enum Job {
    /// Once a minute from `maintain`: expiry, `seen.json`, daily compaction and history pass.
    Tick,
    /// A history file grew past `HISTORY_TRIM_BYTES`.
    Trim(String),
    /// `keys.jsonl` has many more lines than live pins.
    Compact,
    /// Write `revoked.json` (a snapshot of the map now), then report the result.
    Revoked(mpsc::Sender<io::Result<()>>),
    /// Once at start: software-list records and files matched up ([`sbom_scan`]).
    SbomScan,
}

#[derive(Default)]
pub struct Status {
    /// The data directory.
    data: PathBuf,
    /// Ordered maps: copied out in chunks, each under its own lock hold ([`chunks`]).
    keys: BTreeMap<String, Pin>,
    dev: BTreeMap<Arc<str>, Seen>,
    /// Pinned Tether ID → uuid. Only a map key: a Tether ID never names a file.
    by_tether: HashMap<String, String>,
    /// Used tokens issued for one Tether ID (`keys.jsonl` `used` lines), until they expire.
    used: Used,
    /// Keys of dismissed devices that released their own pin (`keys.jsonl` `dismissed` lines).
    dismissed: Dismissed,
    /// Live pins with `no_claim`, per key: with `dismissed`, the keys that claim nothing under any uuid.
    nc_keys: HashMap<String, u32>,
    /// Revoked token id → its `exp` (`revoked.json`). Shared with the writer's snapshot.
    revoked: Arc<HashMap<String, u64>>,
    /// Device signature nonces, in memory only (a replay after a restart still needs a higher seq).
    nonces: HashMap<String, u64>,
    fail_hits: HashMap<String, (u64, u32)>,
    enroll_hits: HashMap<String, (u64, u32)>,
    refused_hits: HashMap<String, (u64, u32)>,
    seen_dirty: bool,
    /// `keys.jsonl` (or the earlier `keys.json`) or `revoked.json` exists but could not be read:
    /// every status route answers 503 and nothing under `status/` is written (an empty map would
    /// hand every pin to any token holder, or make revoked tokens valid again).
    locked: bool,
    locked_logged: u64,
    /// Time and ETag of `summary.json` (None: never built), and whether a build is running.
    built: Option<(u64, String)>,
    building: bool,
    /// uuids whose full report is being written (outside the lock), and whose history is being
    /// trimmed.
    writing: HashSet<String>,
    trimming: HashSet<String>,
    /// A `seen.json` flush is running in the writer thread.
    flushing: bool,
    /// Lines in `keys.jsonl`; while it is compacted, the lines appended meanwhile.
    log_lines: usize,
    compacting: Option<Vec<u8>>,
    last_compact: u64,
    last_daily: u64,
    /// Enrolment, link or removal answers waiting for their `keys.jsonl` sync.
    keys_pending: usize,
    /// When a summary build last failed (the next one waits `SUMMARY_SECS`).
    summary_failed: u64,
    /// When `seen.json` was last written (every `SEEN_SECS`).
    seen_flushed: u64,
    daily_running: bool,
    writer: Option<mpsc::Sender<Job>>,
    /// Software lists recorded: count and the running sum of their sizes (`STATUS_SBOM_MAX_TOTAL_MB`).
    sbom_files: u64,
    sbom_bytes: u64,
    /// uuid → when its last software list was accepted, or refused after a valid signature
    /// (400, 507 store full), for an hour (a deleted list still counts).
    sbom_last: HashMap<String, u64>,
    /// uuids whose software list is being written (outside the lock).
    sbom_writing: HashSet<String>,
    /// The global budget: (second, lists let through to the signature check in it).
    sbom_budget: (u64, u64),
    /// When the last `Job::SbomScan` was queued (the start-up one counts).
    sbom_scanned: u64,
    /// A scan's thread is running.
    sbom_scanning: bool,
}

impl Status {
    /// The status store, or a locked one: an I/O error here (a full or read-only disk during the
    /// migration, an unreadable file) must never stop the relay's other work.
    pub fn open(data: &Path) -> Self {
        Self::open_store(data).unwrap_or_else(|e| {
            eprintln!("error: status store in {}: {e}; status reporting is unavailable until this is repaired (the rest of the relay runs)", data.join("status").display());
            Status { locked: true, locked_logged: now(), ..Default::default() }
        })
    }

    fn open_store(data: &Path) -> io::Result<Self> {
        let dir = data.join("status");
        mkdirs(&dir, 0o700)?;
        let locked = |p: &Path, e: &dyn fmt::Display| {
            eprintln!("error: {}: {e}; status reporting is unavailable until it is repaired or removed", p.display());
            Ok(Status { locked: true, locked_logged: now(), ..Default::default() })
        };
        let rev_path = dir.join("revoked.json");
        let mut revoked: HashMap<String, u64> = match fs::read(&rev_path) {
            Ok(b) => match serde_json::from_slice(&b) {
                Ok(m) => m,
                Err(e) => return locked(&rev_path, &e),
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => HashMap::new(),
            Err(e) => return locked(&rev_path, &e),
        };
        let t = now();
        revoked.retain(|_, exp| *exp > t); // written again with the next revocation or tick
        let (log, old) = (dir.join("keys.jsonl"), dir.join("keys.json"));
        let (keys, mut used, mut dismissed, log_lines) = match fs::read(&log) {
            Ok(b) => match replay_keys(&b) {
                Ok((k, used, dismissed, n, good)) => {
                    if good < b.len() {
                        eprintln!("warning: {}: incomplete last line dropped", log.display());
                        OpenOptions::new().write(true).open(&log)?.set_len(good as u64)?;
                    }
                    (k, used, dismissed, n)
                }
                Err(e) => return locked(&log, &e),
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => match fs::read(&old) {
                // The earlier format, migrated once.
                Ok(b) => match serde_json::from_slice::<BTreeMap<String, Pin>>(&b) {
                    Ok(k) => {
                        let k: BTreeMap<String, Pin> = k.into_iter().map(|(u, p)| (u, legacy(p))).collect();
                        let lines: Vec<KeyLine> = k.iter().map(|(u, p)| KeyLine::set(u, p)).collect();
                        let tmp = crate::tmp_path(data);
                        write_synced(&tmp, &key_lines(&lines))?;
                        fs::rename(&tmp, &log)?;
                        fs::rename(&old, dir.join("keys.json.migrated"))?;
                        eprintln!("status: {} pins moved from keys.json to keys.jsonl", k.len());
                        let n = k.len();
                        (k, Used::new(), Dismissed::new(), n)
                    }
                    Err(e) => return locked(&old, &e),
                },
                Err(e) if e.kind() == io::ErrorKind::NotFound => (BTreeMap::new(), Used::new(), Dismissed::new(), 0),
                Err(e) => return locked(&old, &e),
            },
            Err(e) => return locked(&log, &e),
        };
        used.retain(|_, (_, exp)| *exp > t);
        dismissed.retain(|_, at| *at + crate::DEVICE_TTL > t);
        // The cap also on load (a file from before it, or written by hand): the oldest go, with a
        // `forgotten` line each (below), so they do not come back when fewer are left.
        let mut dropped = Vec::new();
        if dismissed.len() > DISMISSED_MAX {
            let mut by_age: Vec<(u64, String)> = dismissed.iter().map(|(k, at)| (*at, k.clone())).collect();
            by_age.sort();
            by_age.truncate(by_age.len() - DISMISSED_MAX);
            for (_, key) in by_age {
                dismissed.remove(&key);
                dropped.push(KeyLine::Forgotten { key });
            }
        }
        // seen.json is derived data: unreadable, it is logged (load_json) and rebuilt by the next posts.
        let mut dev: BTreeMap<Arc<str>, Seen> = load_json(&dir.join("seen.json"));
        dev.retain(|u, _| keys.contains_key(&**u)); // a pin removed before seen.json was flushed
        #[derive(Deserialize)]
        struct Overall {
            overall: String,
        }
        for s in dev.values_mut() {
            // Entries from before the byte cap are cut to it (dropped if unreadable: the device
            // gets `409 send a full report`).
            if s.entry.as_ref().is_some_and(|e| e.len() > ENTRY_MAX) {
                s.entry = s.entry.as_deref().and_then(recap_entry).map(Into::into);
            }
            s.level = s.entry().and_then(|e| serde_json::from_str::<Overall>(e).ok()).map_or(0, |o| level(&o.overall));
        }
        let by_tether = keys.iter().filter_map(|(u, p)| Some((p.tether_id.clone()?, u.clone()))).collect();
        let mut nc_keys: HashMap<String, u32> = HashMap::new();
        for p in keys.values().filter(|p| p.no_claim) {
            *nc_keys.entry(p.key.clone()).or_default() += 1;
        }
        let sbom = dev.values().filter_map(|s| s.sbom.as_ref());
        let (sbom_files, sbom_bytes) = (sbom.clone().count() as u64, sbom.map(|r| r.size).sum());
        let mut s = Status { data: data.to_owned(), keys, dev, by_tether, used, dismissed, nc_keys, revoked: Arc::new(revoked), log_lines, sbom_files, sbom_bytes, sbom_scanned: now(), ..Default::default() };
        if !dropped.is_empty() {
            s.keys_append(&dropped)?.sync_data()?;
        }
        // Before serving: the files of pins expired while the relay was down go now.
        let (gone, log) = s.expire(now());
        if let Some(log) = log {
            log.sync_data()?;
        }
        remove_files(data, &gone);
        Ok(s)
    }

    fn next_secs(&self) -> u64 {
        next_secs(self.keys.len(), conf().rps)
    }

    fn send(&self, job: Job) {
        if let Some(w) = &self.writer {
            let _ = w.send(job);
        }
    }

    /// Appends lines to `keys.jsonl`: O(1) per enrolment or removal. The caller syncs the returned
    /// file after releasing the lock and answers after that (a sync can take tens of ms).
    #[must_use = "sync the file"]
    fn keys_append(&mut self, lines: &[KeyLine]) -> io::Result<fs::File> {
        let bytes = key_lines(lines);
        let f = OpenOptions::new().append(true).create(true).mode(0o600).open(self.data.join("status/keys.jsonl"))?;
        append_whole(&f, &bytes, |mut f, b| f.write_all(b))?;
        self.log_lines += lines.len();
        match &mut self.compacting {
            Some(pending) => pending.extend_from_slice(&bytes),
            None if self.writer.is_some() && self.log_lines > KEYS_LOG_FACTOR * self.keys.len().max(KEYS_LOG_MIN) => {
                self.compacting = Some(Vec::new()); // claimed here, so one request is queued
                self.send(Job::Compact);
            }
            None => {}
        }
        Ok(f)
    }

    /// Drops pins not seen for 180 days (`DEVICE_TTL`, as the device store; never seen: since
    /// enrolment) from memory and `keys.jsonl`; their files are for the caller to remove.
    /// The caller syncs the returned `keys.jsonl` after releasing the lock.
    fn expire(&mut self, t: u64) -> (Vec<String>, Option<fs::File>) {
        if self.locked {
            return (Vec::new(), None);
        }
        let dev = &self.dev;
        // `last_seen` is 0 for a device that sent only a software list.
        let gone: Vec<String> = self.keys.iter().filter(|(u, p)| dev.get(u.as_str()).map_or(0, |s| s.last_seen).max(p.enrolled) + crate::DEVICE_TTL <= t).map(|(u, _)| u.clone()).collect();
        if gone.is_empty() {
            return (gone, None);
        }
        let lines: Vec<KeyLine> = gone.iter().map(|u| KeyLine::Unpin { uuid: u.clone() }).collect();
        let log = match crate::check_space(&self.data, (lines.len() * 64) as u64).map_err(|_| io::Error::other("disk nearly full")).and_then(|()| self.keys_append(&lines)) {
            Ok(f) => f,
            Err(e) => {
                write_error("keys.jsonl", e);
                return (Vec::new(), None);
            }
        };
        for u in &gone {
            if let Some(p) = self.take(u) {
                self.forget(u, &p);
            }
        }
        eprintln!("status: {} pins not seen for 180 days removed", gone.len());
        (gone, Some(log))
    }

    /// Puts `pin` in place of `uuid`'s, keeping `nc_keys` in step.
    fn put(&mut self, uuid: &str, pin: Pin) -> Option<Pin> {
        if pin.no_claim {
            *self.nc_keys.entry(pin.key.clone()).or_default() += 1;
        }
        let old = self.keys.insert(uuid.to_owned(), pin);
        if let Some(o) = old.as_ref().filter(|o| o.no_claim) {
            self.nc_drop(&o.key);
        }
        old
    }

    /// Removes `uuid`'s pin, keeping `nc_keys` in step.
    fn take(&mut self, uuid: &str) -> Option<Pin> {
        let p = self.keys.remove(uuid)?;
        if p.no_claim {
            self.nc_drop(&p.key);
        }
        Some(p)
    }

    fn nc_drop(&mut self, key: &str) {
        if let Some(n) = self.nc_keys.get_mut(key) {
            *n -= 1;
            if *n == 0 {
                self.nc_keys.remove(key);
            }
        }
    }

    /// A key whose link a member dismissed: a live pin with `no_claim`, or a device that released
    /// such a pin itself. It claims nothing, under any uuid, until a member allows a new claim.
    fn key_dismissed(&self, key: &str) -> bool {
        self.dismissed.contains_key(key) || self.nc_keys.contains_key(key)
    }

    /// The rest of a pin already removed: its Tether ID and last post (files: the caller).
    fn forget(&mut self, uuid: &str, pin: &Pin) {
        if let Some(i) = &pin.tether_id {
            self.by_tether.remove(i);
        }
        if let Some(s) = self.dev.remove(uuid) {
            if let Some(r) = s.sbom {
                self.sbom_count(&r, false);
            }
            self.seen_dirty = true;
        }
    }

    /// The running totals, with a record added or taken away.
    fn sbom_count(&mut self, r: &Sbom, add: bool) {
        if add {
            (self.sbom_files, self.sbom_bytes) = (self.sbom_files + 1, self.sbom_bytes + r.size);
        } else {
            (self.sbom_files, self.sbom_bytes) = (self.sbom_files.saturating_sub(1), self.sbom_bytes.saturating_sub(r.size));
        }
    }

    /// Gives back a global budget slot taken in second `slot` (a refused signature), if that
    /// second's count is still running.
    fn sbom_slot_back(&mut self, slot: u64) {
        let (sec, n) = &mut self.sbom_budget;
        if *sec == slot {
            *n = n.saturating_sub(1);
        }
    }

    /// Puts or drops `uuid`'s software-list record, keeping the totals in step.
    fn sbom_record(&mut self, uuid: &str, rec: Option<Sbom>) {
        if rec.is_none() && self.dev.get(uuid).is_none_or(|s| s.sbom.is_none()) {
            return;
        }
        let s = self.dev.entry(uuid.into()).or_default();
        let old = std::mem::replace(&mut s.sbom, rec.clone());
        if rec.is_none() && s.last_seen == 0 && s.entry.is_none() {
            self.dev.remove(uuid); // it existed only for the list
        }
        if let Some(o) = &old {
            self.sbom_count(o, false);
        }
        if let Some(r) = &rec {
            self.sbom_count(r, true);
        }
        self.seen_dirty = true;
    }

    fn sbom_size(&self, uuid: &str) -> Option<u64> {
        self.dev.get(uuid).and_then(|s| s.sbom.as_ref()).map(|r| r.size)
    }

    /// Whether a list of `len` bytes from `uuid` fits under `STATUS_SBOM_MAX_TOTAL_MB`; one not
    /// larger than the device's current list always does.
    fn sbom_fits(&self, uuid: &str, len: u64) -> bool {
        let old = self.sbom_size(uuid).unwrap_or(0);
        len <= old || self.sbom_bytes.saturating_sub(old) + len <= conf().sbom_total
    }

    /// 429 while `uuid`'s last accepted list is younger than an hour or one is being written.
    fn sbom_hour(&self, uuid: &str, t: u64) -> Result<(), Resp> {
        let last = self.dev.get(uuid).and_then(|s| s.sbom.as_ref()).map(|r| r.at).max(self.sbom_last.get(uuid).copied());
        let wait = if self.sbom_writing.contains(uuid) { Some(60) } else { last.filter(|&a| t < a + SBOM_INTERVAL).map(|a| a + SBOM_INTERVAL - t) };
        match wait {
            Some(w) => Err(text(429, "upload too frequent").with_header(hdr("Retry-After", &w.to_string()))),
            None => Ok(()),
        }
    }
}

fn sbom_full() -> Resp {
    text(507, "sbom store full (STATUS_SBOM_MAX_TOTAL_MB)")
}

/// The rest of `PUT /api/status/sbom` after the key lookup, without the state lock but for two
/// short holds: the signature and the hash over the body and the JSON-object check; under the
/// lock the nonce, failure count, the hour, the cap and the claim of the write; the file written
/// and synced; under the lock again the rename and the record.
#[allow(clippy::too_many_arguments)]
fn sbom_store(app: &Mutex<App>, uuid: String, sig: String, key: String, ip: String, url: String, raw: Vec<u8>, t: u64, slot: u64) -> Result<Resp, Resp> {
    let digest = reqauth::body_digest(&raw);
    let mut nonce = String::new();
    let verified = tether_trust::key_from_b64(&key).and_then(|k| {
        reqauth::verify_device_request(&sig, &k, crate::sig_relay().hosts, "PUT", &url, &digest, t, |n| {
            nonce = n.to_owned();
            true
        })
    });
    // serde_json skips ignored values with a loop, not by recursion: any depth, bounded by the body.
    let object = verified.is_ok() && serde_json::from_slice::<Opaque>(&raw).is_ok();
    let len = raw.len() as u64;
    let tmp = {
        let mut a = lock(app);
        let s = &mut a.status;
        let fresh = |s: &mut Status| if s.nonces.insert(nonce, t).is_some() { Err("replayed request".to_owned()) } else { Ok(()) };
        if let Err(e) = verified.and_then(|()| fresh(s)) {
            // A forged list costs its sender one of its failures per minute, not the budget.
            s.sbom_slot_back(slot);
            rate_ok(&mut s.fail_hits, ip, t, u32::MAX);
            return Err(text(401, &e));
        }
        // Refused after a valid signature: the device's fault, so it uses the device's hour (its
        // slot stays used): one device cannot keep the global budget busy. A relay-side failure
        // (disk full, a write error) uses nothing of the device's.
        if !object {
            s.sbom_last.insert(uuid, now());
            return Err(text(400, "not a JSON object"));
        }
        s.sbom_hour(&uuid, now())?;
        if !s.sbom_fits(&uuid, len) {
            s.sbom_last.insert(uuid, now());
            return Err(sbom_full());
        }
        a.check_space(len)?;
        a.status.sbom_writing.insert(uuid.clone());
        crate::tmp_path(&a.data)
    };
    // The record's time is the file's mtime (kept by the rename), so the start-up scan can tell a
    // file that a lost record does not describe.
    let wrote = write_synced(&tmp, &raw).and_then(|()| fs::metadata(&tmp)).map(|m| mtime(&m));
    let mut a = lock(app);
    a.status.sbom_writing.remove(&uuid);
    let r = wrote.map_err(crate::internal).and_then(|at| {
        // Removed, or another list took the room, while this one was written.
        if !a.status.keys.contains_key(&uuid) {
            return Err(text(401, "unknown device"));
        }
        if !a.status.sbom_fits(&uuid, len) {
            a.status.sbom_last.insert(uuid.clone(), now());
            return Err(sbom_full());
        }
        fs::rename(&tmp, file(&a.data, &uuid, "sbom.json")).map(|()| at).map_err(crate::internal)
    });
    let at = match r {
        Ok(at) => at,
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
    };
    a.status.sbom_record(&uuid, Some(Sbom { at, size: len, sha: digest[..16].to_owned() }));
    a.status.sbom_last.insert(uuid, at);
    Ok(empty(204))
}

/// Answers `resp` once `keys.jsonl` is synced, after the lock is released.
/// While it waits, `keys_pending` counts it, so an enrolment answered from memory waits too.
fn synced_then(app: &mut App, log: fs::File, resp: Resp) -> Result<Resp, Resp> {
    app.status.keys_pending += 1;
    app.defer(move |app| {
        let r = log.sync_data();
        lock(app).status.keys_pending -= 1;
        match r {
            Ok(()) => Ok(resp),
            Err(e) => {
                write_error("keys.jsonl", &e);
                Err(crate::internal(e))
            }
        }
    })
}

/// Syncs the `keys.jsonl` line of a claim taken from a report, after the lock is released. If
/// that fails the claim is taken back in memory: the device names it again with its next report.
fn claim_synced(app: &Mutex<App>, uuid: &str, (log, id): (fs::File, String)) {
    let Err(e) = log.sync_data() else { return };
    write_error("keys.jsonl", &e);
    let mut a = lock(app);
    let Some(p) = a.status.keys.get_mut(uuid) else { return };
    if p.tether_id.is_none() && p.claimed_id.as_deref() == Some(id.as_str()) {
        p.claimed_id = None;
        a.summary_outdated();
    }
}

fn remove_files(data: &Path, uuids: &[String]) {
    for u in uuids {
        for ext in ["json", "history.jsonl", "sbom.json"] {
            let _ = fs::remove_file(file(data, u, ext));
        }
    }
}

/// A history file's lines from the last 90 days, at most the newest 2000; None if nothing would
/// be dropped. Streams, so an oversized file costs only the lines kept.
fn trim_history(r: impl BufRead, t: u64) -> io::Result<Option<Vec<u8>>> {
    #[derive(Deserialize)]
    struct At {
        at: u64,
    }
    let from = t.saturating_sub(HISTORY_DAYS * DAY);
    let (mut kept, mut total) = (VecDeque::new(), 0);
    for line in r.lines() {
        let line = line?;
        total += 1;
        if serde_json::from_str::<At>(&line).is_ok_and(|a| a.at >= from) {
            if kept.len() == HISTORY_LINES {
                kept.pop_front();
            }
            kept.push_back(line);
        }
    }
    Ok((kept.len() < total).then(|| kept.into_iter().flat_map(|l| (l + "\n").into_bytes()).collect()))
}

/// Writes through and hashes what it writes (the summary's ETag without a copy in memory).
struct Hashing<W: Write> {
    w: W,
    h: Sha256,
}

impl<W: Write> Write for Hashing<W> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        let n = self.w.write(b)?;
        self.h.update(&b[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.w.flush()
    }
}

/// Entries copied per lock hold when a map of the state is written out.
const CHUNK: usize = 1000;

fn dev_map(a: &App) -> &BTreeMap<Arc<str>, Seen> {
    &a.status.dev
}

fn keys_map(a: &App) -> &BTreeMap<String, Pin> {
    &a.status.keys
}

fn used_map(a: &App) -> &Used {
    &a.status.used
}

fn dismissed_map(a: &App) -> &Dismissed {
    &a.status.dismissed
}

/// Walks a map of the state in key order, `CHUNK` entries per lock hold, so no hold is
/// O(devices) and no copy of the whole map is made: `copy` takes what it needs out of an entry
/// (under the lock), `out` writes a chunk (without it). A later chunk sees changes made in between.
fn chunks<K, V, T>(
    app: &Mutex<App>,
    map: fn(&App) -> &BTreeMap<K, V>,
    mut copy: impl FnMut(&App, &K, &V) -> Option<T>,
    mut out: impl FnMut(&mut Vec<T>) -> io::Result<()>,
) -> io::Result<()>
where
    K: Ord + Clone + Borrow<str>,
{
    let (mut after, mut buf): (Option<K>, Vec<T>) = (None, Vec::with_capacity(CHUNK));
    loop {
        let n = {
            let a = lock(app);
            let m = map(&a);
            let it: Box<dyn Iterator<Item = (&K, &V)>> = match &after {
                None => Box::new(m.iter()),
                Some(k) => Box::new(m.range::<str, _>((Bound::Excluded(k.borrow()), Bound::Unbounded))),
            };
            let (mut n, mut last) = (0, None);
            for (k, v) in it.take(CHUNK) {
                n += 1;
                last = Some(k);
                buf.extend(copy(&a, k, v));
            }
            let last = last.cloned();
            after = last;
            n
        };
        out(&mut buf)?;
        buf.clear();
        if n < CHUNK {
            return Ok(());
        }
    }
}

/// Writes the summary to `tmp` (no fsync: it is rebuilt after a restart) and returns its ETag:
/// the first 16 hex characters of SHA-256 over the hosts array.
fn build_summary(app: &Mutex<App>, tmp: &Path, t: u64) -> io::Result<String> {
    let f = OpenOptions::new().write(true).create_new(true).mode(0o600).open(tmp)?;
    let mut w = io::BufWriter::new(&f);
    write!(w, "{{\"built\":{t},\"hosts\":")?;
    let mut h = Hashing { w, h: Sha256::new() };
    h.write_all(b"[")?;
    let mut first = true;
    let copy = |a: &App, u: &Arc<str>, s: &Seen| {
        s.entry()?;
        let p = a.status.keys.get(&**u);
        let sbom = s.sbom.as_ref().filter(|_| conf().sbom).map(|r| (r.at, r.sha.clone()));
        Some((u.clone(), p.and_then(|p| p.tether_id.clone()), p.and_then(|p| p.claimed_id.clone()), p.and_then(|p| p.linked_at), p.is_some_and(|p| p.no_claim), p.and_then(|p| p.customer.clone()), s.last_seen, s.next, s.entry.clone()?, sbom))
    };
    chunks(app, dev_map, copy, |buf| {
        // The head is built by the relay from fixed-size fields (`sbom`: a number and 16 hex
        // digits) and is outside the entry's 2 KiB cap, which bounds only the device-supplied part.
        for (u, tid, claimed, linked_at, no_claim, customer, last_seen, next, entry, sbom) in buf.drain(..) {
            let mut head = serde_json::json!({"uuid": &*u, "tether_id": tid, "claimed_id": claimed, "linked_at": linked_at, "no_claim": no_claim, "customer": customer, "last_seen": last_seen, "next": next, "stale": stale_at(last_seen, next, t)});
            if let Some((at, sha)) = sbom {
                head["sbom"] = serde_json::json!({"at": at, "sha": sha});
            }
            let head = head.to_string();
            if !first {
                h.write_all(b",")?;
            }
            first = false;
            // Both are JSON objects: `{head…` + `,` + `…entry}`.
            h.write_all(&head.as_bytes()[..head.len() - 1])?;
            h.write_all(b",")?;
            h.write_all(&entry.as_bytes()[1..])?;
        }
        Ok(())
    })?;
    h.write_all(b"]")?;
    let digest = h.h.finalize();
    let mut w = h.w;
    w.write_all(b"}")?;
    w.flush()?;
    Ok(format!("\"{}\"", digest.iter().take(8).map(|b| format!("{b:02x}")).collect::<String>()))
}

/// `If-None-Match` names `etag` (weak or strong).
fn etag_matches(inm: Option<&str>, etag: &str) -> bool {
    inm.is_some_and(|inm| inm.split(',').any(|x| x.trim().trim_start_matches("W/") == etag))
}

fn stale(s: &Seen, t: u64) -> bool {
    stale_at(s.last_seen, s.next, t)
}

fn stale_at(last_seen: u64, next: u64, t: u64) -> bool {
    t > last_seen + 3 * next
}

/// `status/<uuid>.json` or `.history.jsonl`; `uuid` passed `valid_uuid`.
fn file(data: &Path, uuid: &str, ext: &str) -> std::path::PathBuf {
    data.join("status").join(format!("{uuid}.{ext}"))
}

impl App {
    fn status_answer(&self) -> Resp {
        json(&Answer { releases: self.status_releases(), next: self.status.next_secs(), sbom: conf().sbom })
    }

    /// 503 while `STATUS=off`, or while `keys.json` is unreadable (`Status::locked`, logged at
    /// most once an hour).
    fn status_open(&mut self) -> Result<(), Resp> {
        if conf().off {
            return Err(off());
        }
        if !self.status.locked {
            return Ok(());
        }
        let t = now();
        if t >= self.status.locked_logged + 3600 {
            eprintln!("error: status/keys.jsonl (or keys.json) or status/revoked.json is unreadable; status reporting is unavailable until it is repaired or removed");
            self.status.locked_logged = t;
        }
        Err(text(503, "status store unavailable").with_header(hdr("Retry-After", "3600")))
    }

    /// `POST /api/status/enroll`: the token is admin-signed, and the request is signed by the device
    /// with the key it enrols.
    pub(crate) fn status_enroll(&mut self, rq: &Request, body: &Body, url: &str) -> Result<Resp, Resp> {
        self.status_open()?;
        let body = body.bytes(st::ENROLL_MAX as u64)?;
        if self.keys.is_empty() {
            return Err(text(503, "status enrollment needs ADMIN_PUB_KEY"));
        }
        let t = now();
        let req: st::EnrollRequest = serde_json::from_slice(body).map_err(|e| text(400, &e.to_string()))?;
        if !st::valid_uuid(&req.uuid) {
            return Err(text(400, "bad uuid"));
        }
        tether_trust::key_from_b64(&req.pubkey).map_err(|_| text(400, "bad pubkey"))?;
        let key = req.pubkey.trim().to_owned();
        if req.tether_id.as_deref().is_some_and(|i| !st::valid_tether_id(i)) {
            return Err(text(400, "bad tether_id"));
        }
        // New enrollments and refused tokens count per IP; the same key again is free.
        let old = self.status.keys.get(&req.uuid).cloned();
        let ip = crate::rate_ip(rq);
        let counted = old.as_ref().is_none_or(|p| p.key != key);
        if counted && self.status.enroll_hits.get(&ip).is_some_and(|h| t < h.0 + 60 && h.1 >= ENROLL_PER_MIN) {
            return Err(text(429, "too many requests").with_header(hdr("Retry-After", "60")));
        }
        let refused = |s: &mut Status, r: Resp| {
            if counted {
                rate_ok(&mut s.enroll_hits, ip.clone(), t, u32::MAX);
            }
            r
        };
        // Every enrolment proves possession of the key it pins (verified after the token). Without a
        // header it is refused before the token is checked, counted as a refused token is.
        if header(rq, reqauth::HEADER).is_none() {
            return Err(refused(&mut self.status, text(401, "enrolment needs the device's signature")));
        }
        let tok = match st::verify_token(&req.token, &self.keys, t) {
            Ok(tok) if !self.status.revoked.contains_key(&tok.id) => tok,
            Ok(_) => return Err(refused(&mut self.status, text(401, "token revoked"))),
            Err(e) => return Err(refused(&mut self.status, text(401, &format!("enrollment token: {e}")))),
        };
        if old.as_ref().is_some_and(|p| p.key != key) {
            return Err(text(409, "uuid is enrolled with another key"));
        }
        // The device's signature, against the key it enrols (for an existing pin: the pinned key).
        let (uuid, sig) = Self::device_header(rq)?;
        if uuid != req.uuid {
            return Err(text(401, "signature names another uuid"));
        }
        self.device_verify_key(rq, &sig, Some(&key), "POST", url, body)?;
        // A token issued for a Tether ID: that ID, for one device. Any other token: what the
        // device names is only a claim, until a team member links it.
        let first_use = tok.tether_id.is_some() && !self.status.used.contains_key(&tok.id);
        let dismissed = self.status.key_dismissed(&key);
        let (tether_id, claim) = match &tok.tether_id {
            Some(b) if req.tether_id.as_ref().is_some_and(|i| i != b) => return Err(text(409, "token is issued for another id")),
            Some(b) => {
                // The same device again is idempotent; another one, or this one after its pin was removed, is not.
                if self.status.used.get(&tok.id).is_some_and(|(u, _)| *u != req.uuid || old.is_none()) {
                    return Err(text(409, "token already used"));
                }
                // Only its first use links: later the same device enrolling again changes nothing
                // (a member may have unlinked it since).
                (first_use.then(|| b.clone()), None)
            }
            // A device whose link a member dismissed claims nothing until a member allows it, also
            // with its key under another uuid.
            None if old.as_ref().is_some_and(|o| o.no_claim) || dismissed => (None, None),
            None => (None, req.tether_id.clone()),
        };
        if let (Some(o), Some(i)) = (&old, tether_id.as_ref().or(claim.as_ref())) {
            // Neither can change by enrolling again: not the verified ID, not the claim.
            if o.tether_id.as_ref().or(o.claimed_id.as_ref()).is_some_and(|p| p != i) {
                return Err(text(409, "tether_id is pinned to another value"));
            }
        }
        if tether_id.as_ref().and_then(|i| self.status.by_tether.get(i)).is_some_and(|u| *u != req.uuid) {
            return Err(text(409, "tether_id is enrolled with another key"));
        }
        if old.is_none() && self.status.keys.len() >= conf().max_devices {
            return Err(text(507, "too many devices (STATUS_MAX_DEVICES)"));
        }
        let mut pin = match old.clone() {
            // The same key again: idempotent; a pin without a Tether ID gets the bound one, or the claim.
            Some(mut p) if p.tether_id.is_none() && tether_id.is_some() => {
                (p.tether_id, p.claimed_id, p.linked_by, p.linked_at, p.linked_token, p.no_claim) = (tether_id.clone(), None, None, Some(t), Some(tok.id.clone()), false);
                p
            }
            Some(mut p) => {
                if p.tether_id.is_none() && !p.no_claim {
                    p.claimed_id = p.claimed_id.or(claim);
                }
                p
            }
            // A key whose link was dismissed (a live pin, or one it released itself): a new pin
            // without a claim (only a token issued for its ID, on that token's first use, links it).
            None if dismissed && tether_id.is_none() => Pin {
                key,
                claimed_id: None,
                no_claim: true,
                label: tok.label,
                enrolled: t,
                token: Some(tok.id.clone()),
                ..Default::default()
            },
            None => Pin {
                key,
                linked_at: tether_id.as_ref().map(|_| t),
                linked_token: tether_id.as_ref().map(|_| tok.id.clone()),
                tether_id: tether_id.clone(),
                claimed_id: claim,
                label: tok.label,
                enrolled: t,
                token: Some(tok.id.clone()),
                linked_by: None,
                no_claim: false,
                customer: None,
            },
        };
        // A token without a customer changes nothing; one with it sets it, also on a pin that has one.
        if tok.customer.is_some() {
            pin.customer = tok.customer.clone();
        }
        let changed = old.as_ref() != Some(&pin);
        // The record of a key that released its pin: carried into a new pin (`no_claim`), or ended
        // by a token issued for its ID.
        let forget = (old.is_none() || first_use) && self.status.dismissed.contains_key(&pin.key);
        if changed || first_use {
            self.check_space(st::ENROLL_MAX as u64)?;
            let mut lines = vec![KeyLine::set(&req.uuid, &pin)];
            if forget {
                // Carried into the pin (`no_claim`), or ended by a token issued for its ID.
                lines.push(KeyLine::Forgotten { key: pin.key.clone() });
            }
            if first_use {
                lines.insert(0, KeyLine::Used { token: tok.id.clone(), uuid: req.uuid.clone(), exp: tok.exp });
            }
            let log = self.status.keys_append(&lines).map_err(crate::internal)?;
            if let (Some(o), Some(c)) = (&old, &pin.customer) {
                if o.customer != pin.customer {
                    eprintln!("status: customer of {} set to {c} by token {}", req.uuid, tok.id);
                }
            }
            self.status.put(&req.uuid, pin.clone());
            if forget {
                self.status.dismissed.remove(&pin.key);
            }
            if first_use {
                self.status.used.insert(tok.id, (req.uuid.clone(), tok.exp));
            }
            if old.is_none() {
                rate_ok(&mut self.status.enroll_hits, ip, t, u32::MAX);
            }
            if let Some(i) = pin.tether_id {
                self.status.by_tether.insert(i, req.uuid);
            }
            let resp = self.status_answer();
            return synced_then(self, log, resp);
        }
        let resp = self.status_answer();
        if self.status.keys_pending > 0 {
            // The pin is in memory but a concurrent enrolment's line may not be synced yet: sync
            // before saying it is enrolled (after the lock is released).
            let path = self.data.join("status/keys.jsonl");
            return self.defer(move |_| match fs::File::open(&path).and_then(|f| f.sync_data()) {
                Ok(()) => Ok(resp),
                Err(e) => Err(crate::internal(e)),
            });
        }
        Ok(resp)
    }

    /// The uuid a device header names, checked before it is used as a key or file name.
    fn device_header(rq: &Request) -> Result<(String, String), Resp> {
        let sig = header(rq, reqauth::HEADER).ok_or_else(|| text(401, "missing signature"))?;
        if sig.split_whitespace().next() != Some(reqauth::DEVICE_VERSION) {
            return Err(text(401, "unsupported signature version"));
        }
        match reqauth::device_uuid(&sig) {
            Some(u) if st::valid_uuid(u) => Ok((u.to_owned(), sig)),
            _ => Err(text(401, "malformed signature header")),
        }
    }

    /// Failure limit per IP, key lookup, signature (nonces in memory).
    fn device_verify(&mut self, rq: &Request, sig: &str, uuid: &str, method: &str, url: &str, body: &[u8]) -> Result<(), Resp> {
        let key = self.status.keys.get(uuid).map(|p| p.key.clone());
        self.device_verify_key(rq, sig, key.as_deref(), method, url, body)
    }

    /// 429 while `ip` has had `FAIL_PER_MIN` failed device signatures within the minute.
    fn fail_limit(&self, ip: &str, t: u64) -> Result<(), Resp> {
        if self.status.fail_hits.get(ip).is_some_and(|h| t < h.0 + 60 && h.1 >= FAIL_PER_MIN) {
            return Err(text(429, "too many failed requests").with_header(hdr("Retry-After", "60")));
        }
        Ok(())
    }

    /// As [`Self::device_verify`], against `key` (base64) instead of the pin's.
    fn device_verify_key(&mut self, rq: &Request, sig: &str, key: Option<&str>, method: &str, url: &str, body: &[u8]) -> Result<(), Resp> {
        let ip = crate::rate_ip(rq);
        let t = now();
        self.fail_limit(&ip, t)?;
        let r = match key.map(tether_trust::key_from_b64) {
            None => Err("unknown device".to_owned()),
            Some(Err(e)) => Err(e),
            Some(Ok(key)) => {
                let nonces = &mut self.status.nonces;
                let unused = |n: &str| !nonces.contains_key(n) && nonces.insert(n.to_owned(), t).is_none();
                reqauth::verify_device_request(sig, &key, crate::sig_relay().hosts, method, url, &reqauth::body_digest(body), t, unused)
            }
        };
        r.map_err(|e| {
            rate_ok(&mut self.status.fail_hits, ip, t, u32::MAX);
            text(401, &e)
        })
    }

    /// `POST /api/status`. Cheap checks first: size, header, accepted-post and refused-post
    /// limits, failure limit, key lookup, signature, and only then the body is parsed.
    pub(crate) fn status_post(&mut self, rq: &Request, body: &Body, url: &str) -> Result<Resp, Resp> {
        self.status_open()?;
        let raw = body.bytes(LIMIT_STATUS)?;
        let (uuid, sig) = Self::device_header(rq)?;
        let t = now();
        if let Some(s) = self.status.dev.get(uuid.as_str()).filter(|s| t < s.last_seen + POST_INTERVAL) {
            let wait = (s.last_seen + POST_INTERVAL - t).to_string();
            return Err(text(429, "report too frequent").with_header(hdr("Retry-After", &wait)));
        }
        if self.status.writing.contains(&uuid) {
            return Err(text(429, "report too frequent").with_header(hdr("Retry-After", "60")));
        }
        // Only correctly signed posts are counted, so forged ones cannot use up a device's allowance.
        if self.status.refused_hits.get(&uuid).is_some_and(|h| t < h.0 + 60 && h.1 >= REFUSED_PER_MIN) {
            return Err(text(429, "too many refused reports").with_header(hdr("Retry-After", "60")));
        }
        self.device_verify(rq, &sig, &uuid, "POST", url, raw)?;
        let r = self.status_accept(&uuid, raw, t);
        if r.as_ref().is_err_and(|e| matches!(e.status_code().0, 400 | 409)) {
            rate_ok(&mut self.status.refused_hits, uuid, t, u32::MAX);
        }
        r
    }

    /// A verified post: parse, validate, pin and seq checks, then store.
    fn status_accept(&mut self, uuid: &str, raw: &[u8], t: u64) -> Result<Resp, Resp> {
        if std::str::from_utf8(raw).is_err() {
            return Err(text(400, "report is not UTF-8"));
        }
        let post: Post = serde_json::from_slice(raw).map_err(|e| text(400, &e.to_string()))?;
        let r = post.validate().map_err(|e| text(400, &e))?;
        if r.uuid != uuid {
            return Err(text(400, "uuid does not match the signature"));
        }
        let pin = &self.status.keys[uuid];
        // A report never changes the Tether ID: it must name the verified one, else the claim; with
        // neither there is nothing it could change (with `no_claim` it is ignored).
        if let (Some(i), Some(p)) = (&r.tether_id, pin.tether_id.as_ref().or(pin.claimed_id.as_ref())) {
            if i != p {
                return Err(text(409, "tether_id is pinned to another value"));
            }
        }
        let prev = self.status.dev.get(uuid);
        if prev.is_some_and(|s| r.seq <= s.seq) {
            return Err(text(409, "seq must increase"));
        }
        if !r.is_full() {
            if !prev.is_some_and(|s| s.entry.is_some()) {
                return Err(text(409, "send a full report"));
            }
            let claim = self.reclaim(uuid, r.tether_id.as_deref());
            let next = self.status.next_secs();
            // In place: a heartbeat copies no entry and leaves no allocation behind.
            if let Some(s) = self.status.dev.get_mut(uuid) {
                (s.last_seen, s.seq, s.next) = (t, r.seq, next);
            }
            self.status.seen_dirty = true;
            let resp = self.status_answer();
            return match claim {
                Some(c) => {
                    let uuid = uuid.to_owned();
                    self.defer(move |app| {
                        claim_synced(app, &uuid, c);
                        Ok(resp)
                    })
                }
                None => Ok(resp),
            };
        }
        self.check_space(LIMIT_STATUS)?;
        let named = r.tether_id.clone();
        let mut lines = Vec::new();
        for c in r.changes.as_deref().unwrap_or_default() {
            serde_json::to_writer(&mut lines, &HistoryLine { change: c, at: t }).map_err(crate::internal)?;
            lines.push(b'\n');
        }
        let entry: Arc<str> = entry_json(&r.host, &r.agent, &r.overall, &r.counts, r.products.as_deref().unwrap_or_default(), &r.faults, 0, 0).into();
        let (seq, lvl) = (r.seq, level(&r.overall));
        // The report is written and synced without the lock; taking it again, the history is
        // appended and the report renamed into place (`status_commit`).
        self.status.writing.insert(uuid.to_owned());
        let (uuid, raw, tmp) = (uuid.to_owned(), raw.to_vec(), crate::tmp_path(&self.data));
        self.defer(move |app| {
            let wrote = write_synced(&tmp, &raw);
            let (resp, claim) = {
                let mut a = lock(app);
                a.status.writing.remove(&uuid);
                match wrote {
                    // The claim only once the report is in place.
                    Ok(()) => match a.status_commit(&uuid, &tmp, &lines, entry, seq, lvl, t) {
                        Ok(resp) => (Ok(resp), a.reclaim(&uuid, named.as_deref())),
                        Err(e) => (Err(e), None),
                    },
                    Err(e) => {
                        let _ = fs::remove_file(&tmp);
                        (Err(crate::internal(e)), None)
                    }
                }
            };
            if let Some(c) = claim {
                claim_synced(app, &uuid, c);
            }
            resp
        })
    }

    /// For an accepted post: a device with neither an ID nor a claim, whose claims are allowed (it
    /// never had one, or a member allowed a new one after dismissing its link, and its key is not
    /// dismissed), gets what the report names as its claim (still only a claim). At most once per
    /// pin; the caller syncs the returned `keys.jsonl` after the lock is released
    /// ([`claim_synced`]). A claim that cannot be written is left for the next report.
    fn reclaim(&mut self, uuid: &str, named: Option<&str>) -> Option<(fs::File, String)> {
        let p = self.status.keys.get(uuid)?;
        let i = named.filter(|i| p.tether_id.is_none() && p.claimed_id.is_none() && !p.no_claim && !i.starts_with("m:") && !self.status.key_dismissed(&p.key))?;
        let p = Pin { claimed_id: Some(i.to_owned()), ..p.clone() };
        self.check_space(st::ENROLL_MAX as u64).ok()?;
        let log = self.status.keys_append(&[KeyLine::set(uuid, &p)]).inspect_err(|e| write_error("keys.jsonl", e)).ok()?;
        self.status.put(uuid, p);
        self.summary_outdated();
        Some((log, i.to_owned()))
    }

    /// A full report whose bytes are synced in `tmp`: history first, then the report; on an error
    /// the history is cut back to where it was.
    #[allow(clippy::too_many_arguments)]
    fn status_commit(&mut self, uuid: &str, tmp: &Path, lines: &[u8], entry: Arc<str>, seq: u64, lvl: usize, t: u64) -> Result<Resp, Resp> {
        // Removed or overtaken while the report was written.
        let refused = if !self.status.keys.contains_key(uuid) {
            Some(text(401, "unknown device"))
        } else if self.status.dev.get(uuid).is_some_and(|s| seq <= s.seq) {
            Some(text(409, "seq must increase"))
        } else {
            None
        };
        if let Some(r) = refused {
            let _ = fs::remove_file(tmp);
            return Err(r);
        }
        let hist = if lines.is_empty() {
            None
        } else {
            let f = OpenOptions::new().append(true).create(true).mode(0o600).open(file(&self.data, uuid, "history.jsonl"));
            let f = f.and_then(|f| f.metadata().map(|m| (f, m.len())));
            match f {
                Ok(h) => Some(h),
                Err(e) => {
                    let _ = fs::remove_file(tmp);
                    return Err(crate::internal(e));
                }
            }
        };
        if let Err(e) = append_then_rename(hist.as_ref().map(|(f, _)| f), lines, tmp, &file(&self.data, uuid, "json")) {
            let _ = fs::remove_file(tmp);
            return Err(crate::internal(e));
        }
        if hist.is_some_and(|(_, len)| len + lines.len() as u64 > HISTORY_TRIM_BYTES) {
            self.status.send(Job::Trim(uuid.to_owned()));
        }
        let next = self.status.next_secs();
        let s = self.status.dev.entry(uuid.into()).or_default();
        (s.last_seen, s.seq, s.next, s.entry, s.level) = (t, seq, next, Some(entry), lvl);
        self.status.seen_dirty = true;
        Ok(self.status_answer())
    }

    /// `DELETE /api/status`: the device releases its own pin.
    pub(crate) fn status_delete(&mut self, rq: &Request, body: &Body, url: &str) -> Result<Resp, Resp> {
        self.status_open()?;
        let raw = body.bytes(crate::LIMIT_SMALL)?;
        let (uuid, sig) = Self::device_header(rq)?;
        self.device_verify(rq, &sig, &uuid, "DELETE", url, raw)?;
        self.admin_status_delete(&uuid, "the device", true)
    }

    /// The software-list routes but a device's DELETE: 503 as every status route, then 404 without
    /// `STATUS_SBOM=on`.
    fn sbom_open(&mut self) -> Result<(), Resp> {
        self.status_open()?;
        if conf().sbom {
            Ok(())
        } else {
            Err(not_found())
        }
    }

    /// `PUT /api/status/sbom` before its body is read (its size is checked before this): the
    /// per-IP failure limit, the pin (an unknown uuid counts as a failure), the device's hour,
    /// then the global budget. Returns the second whose budget slot it took: a refused signature
    /// gives it back. Nothing here is counted for the device.
    pub(crate) fn sbom_precheck(&mut self, rq: &Request) -> Result<u64, Resp> {
        self.sbom_open()?;
        let (uuid, _) = Self::device_header(rq)?;
        let (ip, t) = (crate::rate_ip(rq), now());
        self.fail_limit(&ip, t)?;
        if !self.status.keys.contains_key(&uuid) {
            rate_ok(&mut self.status.fail_hits, ip, t, u32::MAX);
            return Err(text(401, "unknown device"));
        }
        self.status.sbom_hour(&uuid, t)?;
        let (sec, n) = &mut self.status.sbom_budget;
        if *sec != t {
            (*sec, *n) = (t, 0);
        }
        if *n >= conf().sbom_rps {
            return Err(text(429, "busy").with_header(hdr("Retry-After", "60")));
        }
        *n += 1;
        Ok(t)
    }

    /// A software list whose body did not arrive (a short body, a read error): its budget slot
    /// goes back and the sender's IP has one failure more, as for a refused signature.
    pub(crate) fn sbom_unread(&mut self, rq: &Request, slot: u64) {
        self.status.sbom_slot_back(slot);
        rate_ok(&mut self.status.fail_hits, crate::rate_ip(rq), now(), u32::MAX);
    }

    /// `PUT /api/status/sbom`: the failure limit and the key lookup here (the body is moved out,
    /// not copied), the rest after the lock is released ([`sbom_store`]).
    pub(crate) fn status_sbom_put(&mut self, rq: &Request, body: &mut Body, url: &str) -> Result<Resp, Resp> {
        self.sbom_open()?;
        let Body::Sbom(raw, slot) = body else { return Err(crate::internal("software list not read")) };
        let (raw, slot) = (std::mem::take(raw), *slot);
        let (uuid, sig) = Self::device_header(rq)?;
        let (ip, t) = (crate::rate_ip(rq), now());
        if let Err(e) = self.fail_limit(&ip, t) {
            self.status.sbom_slot_back(slot); // the limit tripped since the precheck
            return Err(e);
        }
        let Some(key) = self.status.keys.get(&uuid).map(|p| p.key.clone()) else {
            // Removed since the precheck.
            self.status.sbom_slot_back(slot);
            rate_ok(&mut self.status.fail_hits, ip, t, u32::MAX);
            return Err(text(401, "unknown device"));
        };
        let url = url.to_owned();
        self.defer(move |app| sbom_store(app, uuid, sig, key, ip, url, raw, t, slot))
    }

    /// `DELETE /api/status/sbom`: 204 also when there is none, and also with `STATUS_SBOM` off (a
    /// list stored while it was on must not come back when it is switched on again).
    pub(crate) fn status_sbom_delete(&mut self, rq: &Request, body: &Body, url: &str) -> Result<Resp, Resp> {
        self.status_open()?;
        let raw = body.bytes(crate::LIMIT_SMALL)?;
        let (uuid, sig) = Self::device_header(rq)?;
        self.device_verify(rq, &sig, &uuid, "DELETE", url, raw)?;
        self.status.sbom_record(&uuid, None);
        let _ = fs::remove_file(file(&self.data, &uuid, "sbom.json"));
        Ok(empty(204))
    }

    /// `GET /api/team/status/<uuid>/sbom`: the list as stored, its hash as the ETag. Opened under
    /// the lock: an upload renames a new file over it, this one stays readable and matches its tag.
    pub(crate) fn team_status_sbom(&mut self, rq: &Request, uuid: &str) -> Result<Resp, Resp> {
        self.sbom_open()?;
        let rec = st::valid_uuid(uuid).then(|| self.status.dev.get(uuid)).flatten().and_then(|s| s.sbom.as_ref()).ok_or_else(not_found)?;
        let etag = format!("\"{}\"", rec.sha);
        if etag_matches(header(rq, "If-None-Match").as_deref(), &etag) {
            return Ok(empty(304).with_header(hdr("ETag", &etag)));
        }
        let f = fs::File::open(file(&self.data, uuid, "sbom.json")).map_err(|_| not_found())?;
        Ok(tiny_http::Response::from_file(f)
            .with_header(hdr("Content-Type", "application/vnd.cyclonedx+json"))
            .with_header(hdr("ETag", &etag))
            .with_chunked_threshold(usize::MAX)
            .boxed())
    }

    /// `tether_api_status_sbom_files` and `_bytes`.
    pub(crate) fn status_sbom(&self) -> (u64, u64) {
        (self.status.sbom_files, self.status.sbom_bytes)
    }

    /// `DELETE /api/admin/status/<uuid>` (already admin-authenticated), a team member's
    /// `DELETE /api/team/status/<uuid>` (already checked for `write`) and a device's own release;
    /// `by` names who, for the journal.
    /// `own`: the device itself; when its link was dismissed, its key is remembered (`dismissed`),
    /// so dropping its pin does not give it a new claim.
    pub(crate) fn admin_status_delete(&mut self, uuid: &str, by: &str, own: bool) -> Result<Resp, Resp> {
        self.status_open()?;
        if !st::valid_uuid(uuid) {
            return Err(not_found());
        }
        if !self.status.keys.contains_key(uuid) {
            return Err(not_found());
        }
        self.check_space(st::ENROLL_MAX as u64)?;
        let mut lines = vec![KeyLine::Unpin { uuid: uuid.to_owned() }];
        let tomb = self.status.keys.get(uuid).filter(|p| own && p.no_claim).map(|p| p.key.clone());
        // At the cap the oldest record goes, with its own `forgotten` line (else a restart brings it back).
        let mut oldest = None;
        if let Some(key) = &tomb {
            lines.push(KeyLine::Dismissed { key: key.clone(), at: now() });
            if !self.status.dismissed.contains_key(key) && self.status.dismissed.len() >= DISMISSED_MAX {
                // shortcut: O(dismissed) scan for the oldest, only when the cap is reached.
                oldest = self.status.dismissed.iter().min_by_key(|(_, at)| **at).map(|(k, _)| k.clone());
                lines.extend(oldest.iter().map(|k| KeyLine::Forgotten { key: k.clone() }));
            }
        }
        let log = self.status.keys_append(&lines).map_err(crate::internal)?;
        if let Some(key) = tomb {
            if let Some(old) = oldest {
                self.status.dismissed.remove(&old);
            }
            self.status.dismissed.insert(key, now());
        }
        if let Some(pin) = self.status.take(uuid) {
            self.status.forget(uuid, &pin);
        }
        // Two files, not O(devices). The summary is not rebuilt for this: the host disappears
        // from it within 30 s.
        remove_files(&self.data, &[uuid.to_owned()]);
        eprintln!("status: pin of {uuid} removed by {by}");
        synced_then(self, log, empty(204))
    }

    /// The next summary request rebuilds it (a link changes what the team sees at once).
    fn summary_outdated(&mut self) {
        if let Some((at, _)) = &mut self.status.built {
            *at = 0;
        }
    }

    /// Replaces a pin's record (link, unlink): `keys.jsonl` first, then memory. `forget`: the
    /// record of its key having released a dismissed pin ends too.
    fn pin_update(&mut self, uuid: &str, pin: Pin, forget: bool) -> Result<fs::File, Resp> {
        self.check_space(st::ENROLL_MAX as u64)?;
        let forget = forget && self.status.dismissed.contains_key(&pin.key);
        let mut lines = vec![KeyLine::set(uuid, &pin)];
        if forget {
            lines.push(KeyLine::Forgotten { key: pin.key.clone() });
        }
        let log = self.status.keys_append(&lines).map_err(crate::internal)?;
        if forget {
            self.status.dismissed.remove(&pin.key);
        }
        let old = self.status.put(uuid, pin.clone());
        if let Some(i) = old.and_then(|o| o.tether_id) {
            self.status.by_tether.remove(&i);
        }
        if let Some(i) = pin.tether_id {
            self.status.by_tether.insert(i, uuid.to_owned());
        }
        self.summary_outdated();
        Ok(log)
    }

    /// `POST /api/team/status/<uuid>/link` `{"tether_id"}` from a member with `write` (checked by
    /// the caller): confirms the device's claim, which then is its Tether ID.
    pub(crate) fn team_status_link(&mut self, uuid: &str, body: &[u8], member: &str) -> Result<Resp, Resp> {
        self.status_open()?;
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Link {
            tether_id: String,
        }
        let pin = st::valid_uuid(uuid).then(|| self.status.keys.get(uuid)).flatten().cloned().ok_or_else(not_found)?;
        let id = serde_json::from_slice::<Link>(body).map_err(|e| text(400, &e.to_string()))?.tether_id;
        if !st::valid_tether_id(&id) || id.starts_with("m:") {
            return Err(text(400, "bad tether_id"));
        }
        if pin.tether_id.as_ref() == Some(&id) {
            return Ok(empty(204));
        }
        if pin.claimed_id.as_ref() != Some(&id) {
            return Err(text(409, "tether_id is not the one this device claims"));
        }
        if self.status.by_tether.get(&id).is_some_and(|u| u != uuid) {
            return Err(text(409, "tether_id is pinned to another device"));
        }
        let t = now();
        let next = Pin { tether_id: Some(id.clone()), claimed_id: None, linked_by: Some(member.to_owned()), linked_at: Some(t), linked_token: None, no_claim: false, ..pin };
        let log = self.pin_update(uuid, next, false)?;
        eprintln!("status: {uuid} linked to {id} by team member {member}");
        synced_then(self, log, empty(204))
    }

    /// `DELETE /api/team/status/<uuid>/link` (member with `write`): removes the link and the claim,
    /// and the device cannot claim an ID again (`no_claim`) until a member allows it.
    pub(crate) fn team_status_unlink(&mut self, uuid: &str, member: &str) -> Result<Resp, Resp> {
        self.status_open()?;
        let pin = st::valid_uuid(uuid).then(|| self.status.keys.get(uuid)).flatten().cloned().ok_or_else(not_found)?;
        if pin.no_claim && pin.tether_id.is_none() && pin.claimed_id.is_none() {
            return Ok(empty(204));
        }
        let what = match (&pin.tether_id, &pin.claimed_id) {
            (Some(i), _) => format!("link to {i}"),
            (None, Some(c)) => format!("claim of {c}"),
            (None, None) => "claims".to_owned(),
        };
        let next = Pin { tether_id: None, claimed_id: None, linked_by: None, linked_at: None, linked_token: None, no_claim: true, ..pin };
        let log = self.pin_update(uuid, next, false)?;
        eprintln!("status: {what} of {uuid} dismissed by team member {member}");
        synced_then(self, log, empty(204))
    }

    /// `POST /api/team/status/<uuid>/allow-claim` (member with `write`): clears `no_claim`; the
    /// device's next enrolment or report names its claim again (still only a claim).
    pub(crate) fn team_status_allow_claim(&mut self, uuid: &str, member: &str) -> Result<Resp, Resp> {
        self.status_open()?;
        let pin = st::valid_uuid(uuid).then(|| self.status.keys.get(uuid)).flatten().cloned().ok_or_else(not_found)?;
        if !pin.no_claim {
            return Ok(empty(204));
        }
        let log = self.pin_update(uuid, Pin { no_claim: false, ..pin }, true)?;
        eprintln!("status: {uuid} may claim a Tether ID again, allowed by team member {member}");
        synced_then(self, log, empty(204))
    }

    /// `POST /api/admin/status/revoke` `{"id","exp"}`: the token is refused from now on. Answered
    /// once `revoked.json` (written by the status writer) is synced.
    pub(crate) fn status_revoke(&mut self, body: &[u8]) -> Result<Resp, Resp> {
        self.status_open()?;
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Revoke {
            id: String,
            exp: u64,
        }
        let r: Revoke = serde_json::from_slice(body).map_err(|e| text(400, &e.to_string()))?;
        if !st::valid_token_id(&r.id) {
            return Err(text(400, "bad id"));
        }
        if r.exp <= now() {
            return Err(text(400, "exp has passed"));
        }
        let cur = self.status.revoked.get(&r.id).copied();
        // The map's size: expired entries leave it at the next tick (within a minute).
        if cur.is_none() && self.status.revoked.len() >= REVOKED_MAX {
            return Err(text(507, "too many revoked tokens"));
        }
        if cur.is_none_or(|e| e < r.exp) {
            self.check_space(st::ENROLL_MAX as u64)?;
            Arc::make_mut(&mut self.status.revoked).insert(r.id.clone(), r.exp);
            eprintln!("status: token {} revoked", r.id);
        }
        // Also when it was revoked already: a write that failed before is done now.
        let (tx, rx) = mpsc::channel();
        if let Some(w) = &self.status.writer {
            let _ = w.send(Job::Revoked(tx));
        } else {
            let _ = tx.send(write_revoked(&self.status.data, &self.status.revoked));
        }
        self.defer(move |_| match rx.recv_timeout(Duration::from_secs(30)) {
            Ok(Ok(())) => Ok(empty(204)),
            Ok(Err(e)) => Err(crate::internal(e)),
            Err(_) => Err(crate::internal("revoked.json was not written")),
        })
    }

    /// `GET /api/admin/status/revoked`: {token id: exp} of the revocations not expired.
    pub(crate) fn status_revoked(&mut self) -> Result<Resp, Resp> {
        self.status_open()?;
        let t = now();
        let live: BTreeMap<&String, &u64> = self.status.revoked.iter().filter(|(_, &e)| e > t).collect();
        Ok(json(&live))
    }

    /// `GET /api/admin/status/devices[?token=<id>][&after=<uuid>]`: the pins in uuid order, at
    /// most `CHUNK` per page; copied out of the ordered map (one page per lock hold), sent after.
    pub(crate) fn status_devices(&mut self, url: &str) -> Result<Resp, Resp> {
        self.status_open()?;
        let (mut token, mut after) = (None, None);
        for kv in url.split_once('?').map_or("", |(_, q)| q).split('&').filter(|s| !s.is_empty()) {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            match k {
                "token" if st::valid_token_id(v) => token = Some(v.to_owned()),
                "after" if st::valid_uuid(v) => after = Some(v.to_owned()),
                "token" | "after" => return Err(text(400, &format!("bad {k}"))),
                _ => return Err(text(400, &format!("unknown parameter {k}"))),
            }
        }
        let mut range = self.status.keys.range::<str, _>((after.as_deref().map_or(Bound::Unbounded, Bound::Excluded), Bound::Unbounded));
        let mut page = Vec::new();
        // At most CHUNK pins are looked at per request (matching the filter or not): a page can be
        // short or empty, and `next` is the last uuid looked at while more follow.
        let mut last = None;
        for (u, p) in range.by_ref().take(CHUNK) {
            last = Some(u.clone());
            if token.is_none() || p.token == token {
                let s = self.status.dev.get(u.as_str());
                page.push((u.clone(), p.clone(), s.map(|s| s.last_seen).filter(|&l| l > 0), s.and_then(|s| s.entry.clone())));
            }
        }
        let next = if range.next().is_some() { last } else { None };
        self.defer(move |_| {
            #[derive(Deserialize)]
            struct Host {
                host: String,
            }
            let devices: Vec<serde_json::Value> = page
                .iter()
                .map(|(u, p, last_seen, entry)| {
                    let host = entry.as_deref().and_then(|e| serde_json::from_str::<Host>(e).ok()).map(|h| h.host);
                    serde_json::json!({"uuid": u, "token": p.token, "label": p.label, "tether_id": p.tether_id, "claimed_id": p.claimed_id,
                        "linked_by": p.linked_by, "linked_at": p.linked_at, "linked_token": p.linked_token, "no_claim": p.no_claim, "customer": p.customer,
                        "host": host, "enrolled": p.enrolled, "last_seen": last_seen})
                })
                .collect();
            let body = serde_json::to_vec(&serde_json::json!({"devices": devices, "next": next})).map_err(crate::internal)?;
            // A page can be a few hundred KiB: sent with a length, not chunked (as the other lists).
            Ok(tiny_http::Response::from_data(body).with_header(hdr("Content-Type", "application/json")).with_chunked_threshold(usize::MAX).boxed())
        })
    }

    /// `GET /api/team/status`: every host with a full report. `summary.json` is rebuilt at most
    /// every 30 s (stale computed then) and sent from the file; the ETag hashes the hosts only,
    /// so an unchanged summary keeps its tag.
    ///
    /// Built from a snapshot after the lock is released, by one request at a time; requests
    /// meanwhile get the previous file (the very first build makes them wait for it).
    pub(crate) fn team_status_summary(&mut self, rq: &Request) -> Result<Resp, Resp> {
        self.status_open()?;
        let t = now();
        let inm = header(rq, "If-None-Match");
        let failed_recently = t < self.status.summary_failed + SUMMARY_SECS;
        match &self.status.built {
            Some((at, etag)) if t < at + SUMMARY_SECS || self.status.building || failed_recently => return self.summary_file(&etag.clone(), inm.as_deref()),
            None if failed_recently && !self.status.building => return Err(text(503, "status summary not ready").with_header(hdr("Retry-After", "30"))),
            None if self.status.building => {
                return self.defer(move |app| {
                    for _ in 0..600 {
                        {
                            let a = lock(app);
                            if let Some((_, etag)) = &a.status.built {
                                return a.summary_file(etag, inm.as_deref());
                            }
                            if !a.status.building {
                                break;
                            }
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Err(text(503, "status summary not ready").with_header(hdr("Retry-After", "5")))
                })
            }
            _ => {}
        }
        self.status.building = true;
        let tmp = crate::tmp_path(&self.data);
        self.defer(move |app| {
            let built = build_summary(app, &tmp, t);
            let mut a = lock(app);
            a.status.building = false;
            // Renamed under the lock: a file opened under it always matches the ETag sent with it.
            match built.and_then(|etag| fs::rename(&tmp, a.data.join("status/summary.json")).map(|()| etag)) {
                Ok(etag) => {
                    a.status.built = Some((t, etag.clone()));
                    a.summary_file(&etag, inm.as_deref())
                }
                Err(e) => {
                    let _ = fs::remove_file(&tmp);
                    a.status.summary_failed = now(); // no new attempt for SUMMARY_SECS
                    write_error("summary.json", &e);
                    Err(crate::internal(e))
                }
            }
        })
    }

    /// `summary.json` with its ETag, or 304. Opened under the lock: a rebuild renames a new file
    /// over it, this one stays readable.
    fn summary_file(&self, etag: &str, inm: Option<&str>) -> Result<Resp, Resp> {
        if etag_matches(inm, etag) {
            return Ok(empty(304).with_header(hdr("ETag", etag)));
        }
        let f = fs::File::open(self.data.join("status/summary.json")).map_err(crate::internal)?;
        Ok(tiny_http::Response::from_file(f)
            .with_header(hdr("Content-Type", "application/json"))
            .with_header(hdr("ETag", etag))
            .with_chunked_threshold(usize::MAX)
            .boxed())
    }

    /// `GET /api/team/status/<uuid>`: the last report (raw bytes) and 90 days of history, read
    /// after the state lock is released.
    pub(crate) fn team_status_host(&mut self, uuid: &str) -> Result<Resp, Resp> {
        self.status_open()?;
        if !st::valid_uuid(uuid) {
            return Err(not_found());
        }
        let pin = self.status.keys.get(uuid).ok_or_else(not_found)?;
        let t = now();
        let s = self.status.dev.get(uuid).filter(|s| s.last_seen > 0); // 0: only a software list so far
        let head = serde_json::json!({
            "uuid": uuid,
            "tether_id": pin.tether_id,
            "claimed_id": pin.claimed_id,
            "enrolled": pin.enrolled,
            "linked_by": pin.linked_by,
            "linked_at": pin.linked_at,
            "linked_token": pin.linked_token,
            "no_claim": pin.no_claim,
            "customer": pin.customer,
            "last_seen": s.map(|s| s.last_seen),
            "next": s.map(|s| s.next),
            "stale": s.is_some_and(|s| stale(s, t)),
        })
        .to_string();
        let (report, history) = (file(&self.data, uuid, "json"), file(&self.data, uuid, "history.jsonl"));
        self.defer(move |_| {
            #[derive(Deserialize)]
            struct At {
                at: u64,
            }
            let mut out = head.into_bytes();
            out.pop();
            out.extend_from_slice(b",\"report\":");
            match fs::read(&report) {
                Ok(b) => out.extend_from_slice(&b),
                Err(_) => out.extend_from_slice(b"null"),
            }
            out.extend_from_slice(b",\"history\":[");
            let from = t.saturating_sub(HISTORY_DAYS * DAY);
            let mut first = true;
            if let Ok(f) = fs::File::open(&history) {
                for line in BufReader::new(f).lines().map_while(Result::ok) {
                    if serde_json::from_str::<At>(&line).is_ok_and(|a| a.at >= from) {
                        if !first {
                            out.push(b',');
                        }
                        first = false;
                        out.extend_from_slice(line.as_bytes());
                    }
                }
            }
            out.extend_from_slice(b"]}");
            Ok(tiny_http::Response::from_data(out).with_header(hdr("Content-Type", "application/json")).boxed())
        })
    }

    /// Periodic (on the accept thread, under the lock): in-memory pruning only; the disk work is
    /// the writer thread's (`Job::Tick`).
    pub(crate) fn status_maintain(&mut self, t: u64) {
        self.status.nonces.retain(|_, seen| t < *seen + reqauth::NONCE_TTL_SECS);
        self.status.fail_hits.retain(|_, (start, _)| t < *start + 60);
        self.status.enroll_hits.retain(|_, (start, _)| t < *start + 60);
        self.status.refused_hits.retain(|_, (start, _)| t < *start + 60);
        self.status.sbom_last.retain(|_, at| t < *at + SBOM_INTERVAL);
        // Hourly, in the status writer: records and files matched up, orphans removed. The first
        // run is queued by `start_writer`.
        if t >= self.status.sbom_scanned + SBOM_INTERVAL {
            self.status.sbom_scanned = t;
            self.status.send(Job::SbomScan);
        }
        self.status.send(Job::Tick);
    }

    /// `tether_api_status_hosts`: hosts with a full report by state (stale first, unknown as ok).
    pub(crate) fn status_hosts(&self, t: u64) -> [(&'static str, u64); 4] {
        let mut n = [("ok", 0), ("warn", 0), ("crit", 0), ("stale", 0)];
        for s in self.status.dev.values().filter(|s| s.entry().is_some()) {
            n[if stale(s, t) { 3 } else { s.level }].1 += 1;
        }
        n
    }
}

/// Writes `seen.json` through a temp file (synced, renamed), copied out `CHUNK` devices per lock
/// hold; the caller has cleared `seen_dirty` (a post meanwhile sets it again).
fn write_seen(app: &Mutex<App>, data: &Path) -> io::Result<()> {
    #[derive(Serialize)]
    struct Out<'a> {
        last_seen: u64,
        seq: u64,
        next: u64,
        entry: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        sbom: Option<Sbom>,
    }
    let tmp = crate::tmp_path(data);
    let r = (|| {
        let f = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
        let mut w = io::BufWriter::new(&f);
        w.write_all(b"{")?;
        let mut first = true;
        let copy = |_: &App, u: &Arc<str>, s: &Seen| Some((u.clone(), s.last_seen, s.seq, s.next, s.entry.clone(), s.sbom.clone()));
        chunks(app, dev_map, copy, |buf| {
            for (u, last_seen, seq, next, entry, sbom) in buf.drain(..) {
                if !first {
                    w.write_all(b",")?;
                }
                first = false;
                serde_json::to_writer(&mut w, &*u).map_err(io::Error::other)?;
                w.write_all(b":")?;
                serde_json::to_writer(&mut w, &Out { last_seen, seq, next, entry: entry.as_deref(), sbom }).map_err(io::Error::other)?;
            }
            Ok(())
        })?;
        w.write_all(b"}")?;
        w.flush()?;
        drop(w);
        f.sync_all()?;
        fs::rename(&tmp, data.join("status/seen.json"))
    })();
    if r.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    r
}

/// Writes `revoked.json` from `map` (a snapshot: an `Arc` taken under the lock, written without
/// it), through a synced temp file.
fn write_revoked(data: &Path, map: &HashMap<String, u64>) -> io::Result<()> {
    let tmp = crate::tmp_path(data);
    let r = write_synced(&tmp, &serde_json::to_vec(map).map_err(io::Error::other)?).and_then(|()| fs::rename(&tmp, data.join("status/revoked.json")));
    if r.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    r
}

/// The writer's `revoked.json`: the map as it is now (later revocations queue their own write).
fn revoked_job(app: &Mutex<App>) -> io::Result<()> {
    let (data, map) = {
        let a = lock(app);
        if a.status.locked {
            return Err(io::Error::other("status store unavailable"));
        }
        (a.status.data.clone(), a.status.revoked.clone())
    };
    write_revoked(&data, &map).inspect_err(|e| write_error("revoked.json", e))
}

/// Starts the status writer thread: all of the status store's periodic and bulk disk work.
pub(crate) fn start_writer(app: &Arc<Mutex<App>>) {
    let (tx, rx) = mpsc::channel();
    let _ = tx.send(Job::SbomScan);
    lock(app).status.writer = Some(tx);
    let app = app.clone();
    std::thread::spawn(move || {
        for job in rx {
            match job {
                Job::Tick => tick(&app),
                Job::Trim(uuid) => trim_file(&app, &uuid),
                Job::Compact => compact(&app),
                Job::Revoked(done) => {
                    let _ = done.send(revoked_job(&app));
                }
                // Its own thread (one at a time): a long scan must not hold up a revocation
                // (`status_revoke` waits for this thread) or the `seen.json` flush.
                Job::SbomScan => {
                    if !std::mem::replace(&mut lock(&app).status.sbom_scanning, true) {
                        let app = app.clone();
                        std::thread::spawn(move || {
                            sbom_scan(&app);
                            lock(&app).status.sbom_scanning = false;
                        });
                    }
                }
            }
        }
    });
}

/// Pause after each software list the scan reads: spreads the disk reads (the scan has its own
/// thread, so this delays nothing else).
const SBOM_SCAN_PAUSE: Duration = Duration::from_millis(10);

/// A file's modification time in unix seconds (0 if unknown).
fn mtime(m: &fs::Metadata) -> u64 {
    m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs())
}

/// At start and then hourly, in the background: `seen.json` is written every 5 minutes, so after
/// a hard stop a stored software list may lack its record, or have one from before it was
/// replaced (its size or mtime differs from the record's `size` and `at`): both are rebuilt from
/// the file (size, mtime, hash). A record without a file is dropped; a file whose uuid has no
/// pin is removed. A file with the recorded content but another mtime (a restore, a copy) gets the
/// record's time back, so the next scan does not read it again. One pass over the folder, the
/// records copied out in chunks, one stat per file, then a read and one short lock hold per
/// mismatch. Ends with one journal line (files, read again, records rebuilt).
fn sbom_scan(app: &Mutex<App>) {
    let data = {
        let a = lock(app);
        if a.status.locked {
            return;
        }
        a.data.clone()
    };
    let Ok(dir) = fs::read_dir(data.join("status")) else { return };
    let files: HashSet<String> = dir.flatten().filter_map(|e| e.file_name().into_string().ok()?.strip_suffix(".sbom.json").filter(|u| st::valid_uuid(u)).map(str::to_owned)).collect();
    let mut have: HashMap<Arc<str>, Sbom> = HashMap::new();
    let _ = chunks(app, dev_map, |_, u, s| s.sbom.clone().map(|r| (u.clone(), r)), |buf| {
        have.extend(buf.drain(..));
        Ok(())
    });
    for u in have.keys().filter(|u| !files.contains(&***u)) {
        let mut a = lock(app);
        if !a.status.sbom_writing.contains(&**u) && !file(&data, u, "sbom.json").exists() {
            a.status.sbom_record(u, None);
        }
    }
    let (mut reread, mut rebuilt) = (0, 0);
    for u in &files {
        if crate::STOP.load(Ordering::SeqCst) {
            return;
        }
        let path = file(&data, u, "sbom.json");
        let known = have.get(u.as_str());
        match known {
            // As an accepted upload left it.
            Some(r) if fs::metadata(&path).is_ok_and(|m| m.len() == r.size && mtime(&m) == r.at) => continue,
            Some(_) => {}
            None => {
                let a = lock(app);
                if !a.status.keys.contains_key(u) {
                    let _ = fs::remove_file(&path);
                    continue;
                }
            }
        }
        let read = fs::File::open(&path).and_then(|mut f| {
            let at = mtime(&f.metadata()?);
            let mut b = Vec::new();
            f.read_to_end(&mut b)?;
            Ok((at, b))
        });
        let Ok((at, b)) = read else { continue };
        reread += 1;
        let rec = Sbom { at, size: b.len() as u64, sha: reqauth::body_digest(&b)[..16].to_owned() };
        drop(b);
        {
            let mut a = lock(app);
            // Still pinned, the record as it was looked at, no upload running, the file still there.
            let cur = a.status.dev.get(u.as_str()).and_then(|s| s.sbom.as_ref());
            if a.status.keys.contains_key(u) && cur == known && !a.status.sbom_writing.contains(u) && path.exists() {
                match known {
                    // The same content, only its mtime changed: the file gets the record's time
                    // back (one syscall, under the lock so no upload replaces it meanwhile); if it
                    // cannot, the record takes the file's.
                    Some(k) if k.sha == rec.sha && k.size == rec.size => {
                        let set = fs::File::options().write(true).open(&path).and_then(|f| f.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(k.at)));
                        if set.is_err() {
                            a.status.sbom_record(u, Some(Sbom { at, ..k.clone() }));
                        }
                    }
                    _ => {
                        a.status.sbom_record(u, Some(rec));
                        rebuilt += 1;
                    }
                }
            }
        }
        std::thread::sleep(SBOM_SCAN_PAUSE);
    }
    eprintln!("status: software lists checked: {} files, {reread} read again, {rebuilt} records rebuilt", files.len());
}

fn tick(app: &Arc<Mutex<App>>) {
    let t = now();
    let (gone, log, data, compact_due, daily, revoked_due) = {
        let mut a = lock(app);
        if a.status.locked || crate::STOP.load(Ordering::SeqCst) {
            return;
        }
        let (gone, log) = a.status.expire(t);
        let s = &mut a.status;
        // Revocations and used bound tokens are kept until the token would have expired anyway.
        let revoked_due = s.revoked.values().any(|&e| e <= t);
        if revoked_due {
            Arc::make_mut(&mut s.revoked).retain(|_, e| *e > t);
        }
        let compact_due = s.compacting.is_none() && t >= s.last_compact + DAY;
        if compact_due {
            s.compacting = Some(Vec::new());
            s.used.retain(|_, (_, e)| *e > t); // daily, with the compaction that drops their lines
            s.dismissed.retain(|_, at| *at + crate::DEVICE_TTL > t);
        }
        let daily = !s.daily_running && t >= s.last_daily + DAY;
        if daily {
            (s.daily_running, s.last_daily) = (true, t);
        }
        (gone, log, s.data.clone(), compact_due, daily, revoked_due)
    };
    if let Some(Err(e)) = log.map(|f| f.sync_data()) {
        write_error("keys.jsonl", e);
    }
    remove_files(&data, &gone);
    if revoked_due {
        let _ = revoked_job(app);
    }
    flush_seen(app, false);
    if compact_due {
        compact(app);
    }
    if daily {
        // Its own thread: a few files per second must not hold up the flushes.
        let app = app.clone();
        std::thread::spawn(move || {
            let names: Vec<String> = fs::read_dir(data.join("status")).map(|d| d.flatten().filter_map(|e| e.file_name().into_string().ok()).collect()).unwrap_or_default();
            for u in names.iter().filter_map(|n| n.strip_suffix(".history.jsonl")).filter(|u| st::valid_uuid(u)) {
                if crate::STOP.load(Ordering::SeqCst) {
                    break;
                }
                trim_file(&app, u);
                std::thread::sleep(TRIM_PAUSE);
            }
            lock(&app).status.daily_running = false;
        });
    }
}

/// `seen.json`, written by the writer thread with no lock hold longer than one chunk. `at_exit`:
/// also while stopping (the shutdown flush, once request threads are done).
fn flush_seen(app: &Mutex<App>, at_exit: bool) {
    let data = {
        let mut a = lock(app);
        let s = &a.status;
        let due = at_exit || now() >= s.seen_flushed + SEEN_SECS;
        if s.locked || s.flushing || !s.seen_dirty || !due || (!at_exit && crate::STOP.load(Ordering::SeqCst)) {
            return;
        }
        (a.status.seen_dirty, a.status.flushing, a.status.seen_flushed) = (false, true, now());
        a.status.data.clone()
    };
    let r = write_seen(app, &data);
    let mut a = lock(app);
    a.status.flushing = false;
    if let Err(e) = r {
        a.status.seen_dirty = true;
        write_error("seen.json", e);
    }
}

/// At shutdown: waits for a running flush, then writes `seen.json` once more if needed.
pub(crate) fn flush_at_exit(app: &Mutex<App>) {
    for _ in 0..200 {
        if !lock(app).status.flushing {
            return flush_seen(app, true);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Rewrites `keys.jsonl` with the live pins only, copied out `CHUNK` pins per lock hold and
/// written without the lock; lines appended meanwhile (kept in `compacting` since the caller set
/// it) are added after them before the rename, under the lock, so replaying gives the pins now.
fn compact(app: &Arc<Mutex<App>>) {
    let data = {
        let a = lock(app);
        if a.status.locked {
            return;
        }
        a.status.data.clone()
    };
    let tmp = crate::tmp_path(&data);
    let mut n = 0;
    let wrote = (|| {
        let mut f = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
        chunks(app, keys_map, |_, u, p| Some(KeyLine::set(u, p)), |buf| {
            n += buf.len();
            f.write_all(&key_lines(buf))
        })?;
        let t = now();
        let used = |_: &App, token: &String, (uuid, exp): &(String, u64)| (*exp > t).then(|| KeyLine::Used { token: token.clone(), uuid: uuid.clone(), exp: *exp });
        chunks(app, used_map, used, |buf| {
            n += buf.len();
            f.write_all(&key_lines(buf))
        })?;
        let dismissed = |_: &App, key: &String, at: &u64| (*at + crate::DEVICE_TTL > t).then(|| KeyLine::Dismissed { key: key.clone(), at: *at });
        chunks(app, dismissed_map, dismissed, |buf| {
            n += buf.len();
            f.write_all(&key_lines(buf))
        })?;
        f.sync_all()
    })();
    let mut a = lock(app);
    let pending = a.status.compacting.take().unwrap_or_default();
    let r = wrote.and_then(|()| {
        if !pending.is_empty() {
            let mut f = OpenOptions::new().append(true).open(&tmp)?;
            f.write_all(&pending)?;
            f.sync_data()?;
        }
        fs::rename(&tmp, data.join("status/keys.jsonl"))
    });
    match r {
        Ok(()) => {
            a.status.log_lines = n + pending.iter().filter(|&&c| c == b'\n').count();
            a.status.last_compact = now();
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            write_error("keys.jsonl compaction", e);
        }
    }
}

/// Trims one history file to 90 days and 2000 lines. The lock is held only to hand over the file
/// (its length then: appends happen under the lock) and to swap the new file in by rename, with
/// whatever was appended in between.
fn trim_file(app: &Arc<Mutex<App>>, uuid: &str) {
    let (path, tmp) = {
        let mut a = lock(app);
        if a.status.locked || !a.status.trimming.insert(uuid.to_owned()) {
            return;
        }
        (file(&a.status.data, uuid, "history.jsonl"), crate::tmp_path(&a.status.data))
    };
    let r = (|| -> io::Result<()> {
        let (f, ino, len) = {
            let _a = lock(app);
            let f = fs::File::open(&path)?;
            let m = f.metadata()?;
            (f, m.ino(), m.len())
        };
        let Some(kept) = trim_history(BufReader::new(f.take(len)), now())? else { return Ok(()) };
        write_synced(&tmp, &kept)?;
        let a = lock(app);
        let same = a.status.keys.contains_key(uuid) && fs::metadata(&path).is_ok_and(|m| m.ino() == ino);
        let swapped = same.then(|| -> io::Result<()> {
            let mut tail = Vec::new();
            let mut cur = fs::File::open(&path)?;
            cur.seek(io::SeekFrom::Start(len))?;
            cur.read_to_end(&mut tail)?;
            if !tail.is_empty() {
                let mut t = OpenOptions::new().append(true).open(&tmp)?;
                t.write_all(&tail)?;
                t.sync_data()?;
            }
            fs::rename(&tmp, &path)
        });
        drop(a);
        if !matches!(swapped, Some(Ok(()))) {
            let _ = fs::remove_file(&tmp);
        }
        swapped.unwrap_or(Ok(()))
    })();
    if let Err(e) = r {
        if e.kind() != io::ErrorKind::NotFound {
            write_error("history trim", e);
        }
    }
    lock(app).status.trimming.remove(uuid);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_append_leaves_no_partial_line() {
        let p = std::env::temp_dir().join(format!("tether-append-{}", std::process::id()));
        fs::write(&p, b"{\"op\":\"unpin\",\"uuid\":\"dev-0001\"}\n").unwrap();
        let f = OpenOptions::new().append(true).open(&p).unwrap();
        let before = fs::read(&p).unwrap();
        let r = append_whole(&f, b"{\"op\":\"unpin\",\"uuid\":\"dev-0002\"}\n", |mut f, b| {
            f.write_all(&b[..10])?; // the disk filled up halfway
            Err(io::Error::other("no space left on device"))
        });
        assert!(r.is_err());
        assert_eq!(fs::read(&p).unwrap(), before, "cut back to the last whole line");
        append_whole(&f, b"x\n", |mut f, b| f.write_all(b)).unwrap();
        assert_eq!(fs::read(&p).unwrap().len(), before.len() + 2);
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn a_report_that_cannot_be_renamed_leaves_the_history_as_it_was() {
        let d = std::env::temp_dir().join(format!("tether-history-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        let (hist, tmp, dest) = (d.join("dev-0001.history.jsonl"), d.join("tmp"), d.join("dev-0001.json"));
        fs::write(&hist, b"{\"at\":1}\n").unwrap();
        let f = OpenOptions::new().append(true).open(&hist).unwrap();
        // No report file to rename (as when the rename fails): the appended changes go again.
        assert!(append_then_rename(Some(&f), b"{\"at\":2}\n", &tmp, &dest).is_err());
        assert_eq!(fs::read(&hist).unwrap(), b"{\"at\":1}\n");
        fs::write(&tmp, b"{}").unwrap();
        append_then_rename(Some(&f), b"{\"at\":2}\n", &tmp, &dest).unwrap();
        assert_eq!((fs::read(&hist).unwrap(), fs::read(&dest).unwrap()), (b"{\"at\":1}\n{\"at\":2}\n".to_vec(), b"{}".to_vec()));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn summary_entries_are_capped_in_bytes() {
        // Four-byte characters at every limit of the report rules.
        let wide = |n: usize| "\u{1F600}".repeat(n);
        let products: Vec<Product> = (0..32).map(|_| Product { name: wide(64), version: Some(wide(32)), role: Some("server".into()) }).collect();
        let faults: Vec<Fault> = (0..50).map(|i| Fault { id: format!("check/{i:02}"), state: "crit".into(), since: Some(wide(40)) }).collect();
        let counts = Counts { ok: u32::MAX, warn: u32::MAX, crit: u32::MAX, unknown: u32::MAX, setup: u32::MAX };
        let e = entry_json(&wide(64), "2026.11.23.2", "crit", &counts, &products, &faults, 0, 0);
        assert!(e.len() <= ENTRY_MAX, "{} bytes", e.len());
        let v: serde_json::Value = serde_json::from_str(&e).unwrap();
        let (np, nf) = (v["products"].as_array().unwrap().len(), v["faults"].as_array().unwrap().len());
        assert_eq!((v["more_products"].as_u64().unwrap() as usize + np, v["more_faults"].as_u64().unwrap() as usize + nf), (32, 50));
        assert_eq!(nf, 0, "faults go first");
        // The host alone over the cap is cut on a character boundary.
        let e = entry_json(&wide(600), "0.1.0", "ok", &counts, &[], &[], 0, 0);
        let v: serde_json::Value = serde_json::from_str(&e).unwrap();
        assert!(e.len() <= ENTRY_MAX && v["host"].as_str().unwrap().chars().all(|c| c == '\u{1F600}'));
        // An oversized entry from an older seen.json is cut the same way when loaded.
        let old = serde_json::json!({"host": wide(64), "agent": "0.1.0", "overall": "warn", "counts": {"ok":1,"warn":1,"crit":0,"unknown":0,"setup":0},
            "products": products[..8], "faults": faults[..10], "more_products": 24, "more_faults": 40}).to_string();
        assert!(old.len() > ENTRY_MAX);
        let cut = recap_entry(&old).unwrap();
        let v: serde_json::Value = serde_json::from_str(&cut).unwrap();
        assert!(cut.len() <= ENTRY_MAX && v["more_faults"].as_u64().unwrap() + v["faults"].as_array().unwrap().len() as u64 == 50);
        assert_eq!(recap_entry("not json"), None);
    }

    #[test]
    fn next_follows_the_budget() {
        assert_eq!(next_secs(0, 10), 300);
        assert_eq!(next_secs(2_000, 10), 300);
        assert_eq!(next_secs(20_000, 10), 2_000);
        assert_eq!(next_secs(50_000, 10), 3_600);
        assert_eq!(next_secs(3_001, 10), 301);
        assert_eq!(next_secs(20_000, 1), 3_600);
    }

    #[test]
    fn history_is_trimmed_to_90_days_and_2000_lines() {
        let t = 200 * DAY;
        let line = |at: u64| format!("{{\"id\":\"a\",\"from\":\"ok\",\"to\":\"crit\",\"time\":\"x\",\"at\":{at}}}\n");
        let fresh: String = (0..10).map(|i| line(t - i)).collect();
        assert_eq!(trim_history(fresh.as_bytes(), t).unwrap(), None, "nothing to drop: no rewrite");
        let mixed = format!("{}{}not json\n{}", line(t - 91 * DAY), line(t - 89 * DAY), line(t));
        assert_eq!(trim_history(mixed.as_bytes(), t).unwrap(), Some(format!("{}{}", line(t - 89 * DAY), line(t)).into_bytes()));
        let many: String = (0..2005).map(|i| line(t - 2005 + i)).collect();
        let kept = trim_history(many.as_bytes(), t).unwrap().unwrap();
        assert_eq!(kept, (5..2005).map(|i| line(t - 2005 + i)).collect::<String>().into_bytes());
    }
}

//! Session audit log: the connection, file-transfer and alarm records the host side of every
//! Tether app posts (RustDesk Pro shape, `vendor/rustdesk/src/server/connection.rs`
//! `post_conn_audit` / `post_file_audit` / `post_alarm_audit`). Unauthenticated device telemetry:
//! anything that can reach the relay can post a record, so records are evidence of what a host
//! reported, not proof (SECURITY.md). They feed nothing else in tether-api.
//!
//! Stored as one JSON line per record in `audit/<YYYY-MM-DD>.jsonl` (UTC day of the server
//! timestamp), files older than the retention dropped, oldest days dropped first above the size cap.

use std::collections::{HashMap, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::net::IpAddr;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{json, Map, Value};

pub const LIMIT: u64 = 8 << 10;
pub const IP_PER_MIN: u32 = 120;
pub const DEVICE_PER_MIN: u32 = 60;
pub const READ_MAX: usize = 5000;
const DAY: u64 = 86_400;
/// Longer than the client's retry window (120 s), so a retried post is stored once.
const NONCE_TTL: u64 = 600;
const NONCES_MAX: usize = 50_000;

const CONN_TYPES: [&str; 5] = ["remote", "file_transfer", "port_forward", "view_camera", "terminal"];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Conn {
    id: String,
    uuid: String,
    conn_id: i64,
    #[serde(default)]
    session_id: u64,
    nonce: String,
    action: Option<String>,
    ip: Option<String>,
    peer: Option<(String, String)>,
    #[serde(rename = "type")]
    conn_type: Option<usize>,
    primary_auth: Option<u8>,
    two_factor: Option<u8>,
    conn_audit_ref: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FilePost {
    id: String,
    uuid: String,
    peer_id: String,
    conn_id: i64,
    #[serde(rename = "type")]
    typ: u8,
    path: String,
    is_file: bool,
    /// JSON text of `FileInfo`.
    info: String,
    nonce: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileInfo {
    ip: String,
    name: String,
    num: u64,
    files: Vec<(String, i64)>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Alarm {
    id: String,
    uuid: String,
    typ: u8,
    info: String,
    conn_id: i64,
    nonce: String,
    conn_audit_ref: Option<String>,
}

/// Printable text of at most `max` bytes (no control characters).
fn plain(what: &str, s: &str, max: usize) -> Result<(), String> {
    if s.len() > max || s.chars().any(char::is_control) {
        return Err(format!("bad {what}"));
    }
    Ok(())
}

fn token(what: &str, s: &str, max: usize, extra: &[u8]) -> Result<(), String> {
    if s.is_empty() || s.len() > max || !s.bytes().all(|b| b.is_ascii_alphanumeric() || extra.contains(&b)) {
        return Err(format!("bad {what}"));
    }
    Ok(())
}

fn ip(s: &str) -> Result<(), String> {
    s.parse::<IpAddr>().map(|_| ()).map_err(|_| "bad ip".to_owned())
}

/// Device fields every record has: Tether ID, base64 machine uuid, retry nonce.
fn head(id: &str, uuid: &str, nonce: &str) -> Result<(), String> {
    if id.len() > 32 || !id.bytes().all(|b| b.is_ascii_graphic()) {
        return Err("bad id".into());
    }
    token("uuid", uuid, 128, b"+/=_-")?;
    token("nonce", nonce, 64, b"-")
}

/// A validated record without `ts`/`src` (set by the caller): `(uuid, nonce, record)`.
pub fn parse(kind: &str, body: &[u8]) -> Result<(String, String, Map<String, Value>), String> {
    let e = |e: serde_json::Error| e.to_string();
    let (uuid, nonce, mut r) = match kind {
        "conn" => {
            let c: Conn = serde_json::from_slice(body).map_err(e)?;
            head(&c.id, &c.uuid, &c.nonce)?;
            let action = match (c.action.as_deref(), &c.peer) {
                (Some("new"), None) => "connect",
                (Some("close"), None) => "disconnect",
                (None, Some(_)) => "login",
                _ => return Err("bad action".into()),
            };
            if action == "connect" {
                ip(c.ip.as_deref().ok_or("missing ip")?)?;
            } else if c.ip.is_some() {
                return Err("unexpected ip".into());
            }
            let mut r = json!({"kind": "conn", "action": action, "id": c.id, "uuid": c.uuid, "conn_id": c.conn_id, "session_id": c.session_id.to_string()});
            if let Some(ipv) = c.ip {
                r["ip"] = json!(ipv);
            }
            if let Some((pid, pname)) = c.peer {
                plain("peer id", &pid, 32)?;
                plain("peer name", &pname, 128)?;
                let t = c.conn_type.ok_or("missing type")?;
                r["peer_id"] = json!(pid);
                r["peer_name"] = json!(pname);
                r["conn_type"] = json!(CONN_TYPES.get(t).ok_or("bad type")?);
            } else if c.conn_type.is_some() {
                return Err("unexpected type".into());
            }
            if let Some(a) = c.primary_auth.filter(|a| *a <= 4) {
                r["primary_auth"] = json!(["none", "click", "temporary_password", "permanent_password", "switch_sides"][a as usize]);
            } else if c.primary_auth.is_some() {
                return Err("bad primary_auth".into());
            }
            if let Some(t) = c.two_factor {
                if t > 15 {
                    return Err("bad two_factor".into());
                }
                r["two_factor"] = json!(t);
            }
            if let Some(a) = c.conn_audit_ref {
                plain("conn_audit_ref", &a, 128)?;
                r["ref"] = json!(a);
            }
            (c.uuid, c.nonce, r)
        }
        "file" => {
            let f: FilePost = serde_json::from_slice(body).map_err(e)?;
            head(&f.id, &f.uuid, &f.nonce)?;
            plain("peer id", &f.peer_id, 32)?;
            plain("path", &f.path, 4096)?;
            let action = match f.typ {
                0 => "send",
                1 => "receive",
                _ => return Err("bad type".into()),
            };
            let info: FileInfo = serde_json::from_str(&f.info).map_err(|e| format!("info: {e}"))?;
            // The client sends at most 10 (largest first) plus the total count.
            if info.files.len() > 10 {
                return Err("too many files".into());
            }
            for (name, _) in &info.files {
                plain("file name", name, 1024)?;
            }
            ip(&info.ip)?;
            plain("peer name", &info.name, 128)?;
            let r = json!({"kind": "file", "action": action, "id": f.id, "uuid": f.uuid, "conn_id": f.conn_id, "peer_id": f.peer_id,
                "peer_name": info.name, "ip": info.ip, "path": f.path, "is_file": f.is_file, "num": info.num, "files": info.files});
            (f.uuid, f.nonce, r)
        }
        "alarm" => {
            let a: Alarm = serde_json::from_slice(body).map_err(e)?;
            head(&a.id, &a.uuid, &a.nonce)?;
            if a.typ > 31 {
                return Err("bad typ".into());
            }
            plain("info", &a.info, 1024)?;
            let mut r = json!({"kind": "alarm", "action": "alarm", "id": a.id, "uuid": a.uuid, "conn_id": a.conn_id, "typ": a.typ, "info": a.info});
            if let Some(x) = a.conn_audit_ref {
                plain("conn_audit_ref", &x, 128)?;
                r["ref"] = json!(x);
            }
            (a.uuid, a.nonce, r)
        }
        _ => return Err("unknown audit type".into()),
    };
    r["nonce"] = json!(nonce);
    let Value::Object(map) = r else { unreachable!() };
    Ok((uuid, nonce, map))
}

/// `YYYY-MM-DD` (UTC) of a unix time (days-from-civil inverse, H. Hinnant).
pub fn date(t: u64) -> String {
    let z = (t / DAY) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{:02}-{:02}", yoe + era * 400 + (m <= 2) as i64, m, d)
}

pub struct Log {
    dir: PathBuf,
    /// 0: records are accepted and dropped (audit log off).
    pub retention_days: u64,
    max_bytes: u64,
    total: u64,
    nonces: HashMap<String, u64>,
}

impl Log {
    pub fn open(dir: PathBuf, retention_days: u64, max_bytes: u64) -> io::Result<Self> {
        let mut l = Log { dir, retention_days, max_bytes, total: 0, nonces: HashMap::new() };
        l.total = l.files().iter().map(|(_, n)| n).sum();
        Ok(l)
    }

    /// `(day, size)` of every log file, oldest first.
    fn files(&self) -> Vec<(String, u64)> {
        let mut v: Vec<(String, u64)> = fs::read_dir(&self.dir)
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| Some((e.file_name().to_str()?.strip_suffix(".jsonl")?.to_owned(), e.metadata().ok()?.len())))
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }

    /// Retention and size cap: whole days, oldest first, never `today`.
    pub fn prune(&mut self, t: u64, need: u64) {
        let cutoff = date(t.saturating_sub(self.retention_days.saturating_mul(DAY)));
        let today = date(t);
        let mut total: u64 = 0;
        let files = self.files();
        let mut over = files.iter().map(|(_, n)| n).sum::<u64>() + need;
        for (day, n) in files {
            if day != today && (day < cutoff || over > self.max_bytes) {
                if fs::remove_file(self.dir.join(format!("{day}.jsonl"))).is_ok() {
                    over -= n;
                    continue;
                }
            }
            total += n;
        }
        self.total = total;
        self.nonces.retain(|_, seen| t < *seen + NONCE_TTL);
    }

    /// True if the record with this nonce was stored in the last 10 minutes (a client retry).
    pub fn seen(&self, nonce: &str) -> bool {
        self.nonces.contains_key(nonce)
    }

    /// Appends one record. `Err(None)`: log full (today alone is over the cap).
    pub fn append(&mut self, nonce: String, rec: &Map<String, Value>, t: u64) -> Result<(), Option<io::Error>> {
        if self.retention_days == 0 {
            return Ok(());
        }
        let mut line = serde_json::to_vec(rec).map_err(|e| Some(io::Error::other(e)))?;
        line.push(b'\n');
        if self.total + line.len() as u64 > self.max_bytes {
            self.prune(t, line.len() as u64);
            if self.total + line.len() as u64 > self.max_bytes {
                return Err(None);
            }
        }
        let p = self.dir.join(format!("{}.jsonl", date(t)));
        let mut f = OpenOptions::new().append(true).create(true).mode(0o600).open(p).map_err(Some)?;
        f.write_all(&line).map_err(Some)?;
        self.total += line.len() as u64;
        if self.nonces.len() < NONCES_MAX {
            self.nonces.insert(nonce, t);
        }
        Ok(())
    }

    /// Records with `ts >= since` (and `id` or `uuid` == `device` if given), newest first.
    pub fn read(&self, since: u64, limit: usize, device: &str) -> Vec<Value> {
        let first = date(since);
        let mut out = Vec::new();
        for (day, _) in self.files().into_iter().rev() {
            if day < first || out.len() >= limit {
                break;
            }
            let Ok(f) = fs::File::open(self.dir.join(format!("{day}.jsonl"))) else { continue };
            // Lines are in time order: keep the last `limit` matches of the day.
            let mut keep = VecDeque::new();
            for line in BufReader::new(f).lines().map_while(Result::ok) {
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                if v["ts"].as_u64().unwrap_or(0) < since || (!device.is_empty() && v["id"] != device && v["uuid"] != device) {
                    continue;
                }
                keep.push_back(v);
                if keep.len() > limit - out.len() {
                    keep.pop_front();
                }
            }
            out.extend(keep.into_iter().rev());
        }
        out
    }
}

/// `application/x-www-form-urlencoded` value.
pub fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            // Both digits checked: from_str_radix would also take a leading '+'.
            b'%' => match b.get(i + 1..i + 3).filter(|h| h.iter().all(u8::is_ascii_hexdigit)).and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok()) {
                Some(x) => {
                    out.push(x);
                    i += 2;
                }
                None => out.push(b'%'),
            },
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(951_782_400), "2000-02-29");
        assert_eq!(date(1_794_009_599), "2026-11-06");
    }

    #[test]
    fn unescapes() {
        assert_eq!(unescape("a%2Bb%3D+c"), "a+b= c");
        assert_eq!(unescape("50%"), "50%");
        assert_eq!(unescape("%zz1"), "%zz1");
        assert_eq!(unescape("%+1"), "% 1");
    }
}

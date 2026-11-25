//! Prometheus metrics of tether-api (`GET /metrics`, text format 0.0.4), answered only to
//! loopback peers. Caddy forwards only `/api/*` and `/q/*`, so the page is never public.
//! Labels are fixed sets: endpoint names below and the HTTP status codes this server sends.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::Mutex;
use std::time::Duration;

pub const ENDPOINTS: [&str; 14] = [
    "policy", "server_keys", "heartbeat", "audit_conn", "audit_file", "audit_alarm", "diag_upload", "custom_download", "admin",
    "metrics", "other", "release_index", "status", "team",
];
/// `tether_api_devices_by_version` series at most (largest groups; the rest is `version="other"`).
pub const MAX_VERSION_SERIES: usize = 30;
pub const METRICS: usize = 9;
const OTHER: usize = 10;
const BUCKETS: [f64; 13] = [0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1., 2.5, 5., 10., 30., 120.];

/// Requests being served (worker threads).
pub static ACTIVE: AtomicUsize = AtomicUsize::new(0);
/// Requests answered 503 "busy" because MAX_WORKERS were running.
pub static BUSY_REJECTED: AtomicU64 = AtomicU64::new(0);
/// Request bodies that failed to arrive in time.
pub static BODY_TIMEOUTS: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
struct Hist {
    counts: [u64; BUCKETS.len()],
    sum: f64,
    n: u64,
}

#[derive(Default)]
struct State {
    /// (endpoint, status) -> requests
    requests: BTreeMap<(usize, u16), u64>,
    latency: BTreeMap<usize, Hist>,
}

static STATE: Mutex<State> = Mutex::new(State { requests: BTreeMap::new(), latency: BTreeMap::new() });

/// The endpoint label of a request.
pub fn endpoint(method: &str, url: &str) -> usize {
    let p = url.split('?').next().unwrap_or("");
    match (method, p) {
        ("GET", "/api/policy") => 0,
        ("GET", "/api/server-keys") => 1,
        ("POST", "/api/heartbeat") => 2,
        ("POST", "/api/audit/conn") => 3,
        ("POST", "/api/audit/file") => 4,
        ("POST", "/api/audit/alarm") => 5,
        ("POST", p) if p.starts_with("/api/diag/") => 6,
        ("GET", p) if p.starts_with("/q/") => 7,
        (_, p) if p.starts_with("/api/admin/") => 8,
        ("GET", "/metrics") => METRICS,
        ("GET", p) if p.starts_with("/download/") && p.ends_with("/releases.json") => 11,
        (_, p) if p == "/api/status" || p.starts_with("/api/status/") => 12,
        (_, p) if p.starts_with("/api/team/") => 13,
        _ => OTHER,
    }
}

pub fn observe(endpoint: usize, status: u16, took: Duration) {
    let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    *s.requests.entry((endpoint, status)).or_default() += 1;
    let h = s.latency.entry(endpoint).or_default();
    let secs = took.as_secs_f64();
    for (i, b) in BUCKETS.iter().enumerate() {
        if secs <= *b {
            h.counts[i] += 1;
        }
    }
    h.sum += secs;
    h.n += 1;
}

/// Counts a body read error that was a timeout.
pub fn body_error(e: &std::io::Error) {
    if matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) {
        BODY_TIMEOUTS.fetch_add(1, Relaxed);
    }
}

/// Values the app reads at scrape time.
#[derive(Default)]
pub struct Now {
    pub max_workers: usize,
    pub devices: usize,
    pub devices_24h: usize,
    pub release_generation: u64,
    /// (channel, serial, expires)
    pub indexes: Vec<(String, u64, u64)>,
    /// (channel, expires) of the stored `monitor.json` (the device-status agent's index).
    pub monitor_expires: Vec<(String, u64)>,
    /// ((profile, version), active devices)
    pub by_version: Vec<((String, String), u64)>,
    pub update_states: Vec<(String, u64)>,
    /// ((profile, version), devices that staged it)
    pub downloads: Vec<((String, String), u64)>,
    /// (state, hosts) for ok, warn, crit, stale.
    pub status_hosts: [(&'static str, u64); 4],
    /// Stored software lists: (files, bytes).
    pub status_sbom: (u64, u64),
    /// Team audit records written, dropped with the queue full, with the log full, not written (write failed).
    pub team_audit: [u64; 4],
}

/// The `n` largest groups; the rest summed per profile as version "other" (bounded label sets:
/// versions come from unauthenticated heartbeats).
pub fn top(groups: std::collections::HashMap<(String, String), u64>, n: usize) -> Vec<((String, String), u64)> {
    let mut v: Vec<_> = groups.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut rest: BTreeMap<String, u64> = BTreeMap::new();
    for ((p, _), c) in v.drain(n.min(v.len())..) {
        *rest.entry(p).or_default() += c;
    }
    v.extend(rest.into_iter().map(|(p, c)| ((p, "other".to_owned()), c)));
    v.sort();
    v
}

/// Label value: only `[A-Za-z0-9._-]` (heartbeat fields are validated, this is a second fence).
fn label(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')).take(64).collect()
}

fn header(o: &mut String, name: &str, typ: &str, help: &str) {
    let _ = writeln!(o, "# HELP {name} {help}\n# TYPE {name} {typ}");
}

pub fn render(now: &Now, data_dir: &Path) -> String {
    let mut o = String::new();
    {
        let s = STATE.lock().unwrap_or_else(|e| e.into_inner());
        header(&mut o, "tether_api_requests_total", "counter", "Requests by endpoint and HTTP status.");
        for ((ep, code), n) in &s.requests {
            let _ = writeln!(o, "tether_api_requests_total{{endpoint=\"{}\",code=\"{code}\"}} {n}", ENDPOINTS[*ep]);
        }
        header(&mut o, "tether_api_request_duration_seconds", "histogram", "Time from request to response written, by endpoint.");
        for (ep, h) in &s.latency {
            let ep = ENDPOINTS[*ep];
            for (i, b) in BUCKETS.iter().enumerate() {
                let _ = writeln!(o, "tether_api_request_duration_seconds_bucket{{endpoint=\"{ep}\",le=\"{b}\"}} {}", h.counts[i]);
            }
            let _ = writeln!(o, "tether_api_request_duration_seconds_bucket{{endpoint=\"{ep}\",le=\"+Inf\"}} {}", h.n);
            let _ = writeln!(o, "tether_api_request_duration_seconds_sum{{endpoint=\"{ep}\"}} {}", h.sum);
            let _ = writeln!(o, "tether_api_request_duration_seconds_count{{endpoint=\"{ep}\"}} {}", h.n);
        }
    }
    let du = dir_bytes(data_dir);
    let (free, size) = fs_space(data_dir);
    for (name, typ, help, v) in [
        ("tether_api_workers_active", "gauge", "Requests being served now.", ACTIVE.load(Relaxed) as f64),
        ("tether_api_workers_max", "gauge", "Requests served at once (MAX_WORKERS); more get 503.", now.max_workers as f64),
        ("tether_api_busy_rejected_total", "counter", "Requests answered 503 busy at MAX_WORKERS.", BUSY_REJECTED.load(Relaxed) as f64),
        ("tether_api_body_timeouts_total", "counter", "Request bodies that did not arrive in time.", BODY_TIMEOUTS.load(Relaxed) as f64),
        ("tether_api_status_write_errors", "counter", "Failed background writes of the device status store since start.", crate::status::WRITE_ERRORS.load(Relaxed) as f64),
        ("tether_api_status_summary_builds_total", "counter", "Team summary builds (any member scope, plain file and gzip copy) since start.", crate::status::SUMMARY_BUILDS.load(Relaxed) as f64),
        ("tether_api_status_summary_build_seconds_total", "counter", "Time spent building team summaries since start.", crate::status::SUMMARY_BUILD_US.load(Relaxed) as f64 / 1e6),
        ("tether_api_status_summary_build_seconds_max", "gauge", "The longest team summary build since start.", crate::status::SUMMARY_BUILD_MAX_US.load(Relaxed) as f64 / 1e6),
        ("tether_api_status_software_searches_total", "counter", "Software searches run since start (answers from the cache not counted).", crate::status::SEARCHES.load(Relaxed) as f64),
        ("tether_api_status_software_search_seconds_total", "counter", "Time spent in software searches since start.", crate::status::SEARCH_US.load(Relaxed) as f64 / 1e6),
        ("tether_api_status_software_search_seconds_max", "gauge", "The longest software search since start.", crate::status::SEARCH_MAX_US.load(Relaxed) as f64 / 1e6),
        ("tether_api_status_sbom_files", "gauge", "Software lists stored (status/<uuid>.sbom.json).", now.status_sbom.0 as f64),
        ("tether_api_status_sbom_bytes", "gauge", "Bytes of the software lists stored (counted against STATUS_SBOM_MAX_TOTAL_MB).", now.status_sbom.1 as f64),
        ("tether_api_team_audit_records_total", "counter", "Team audit records written since start.", now.team_audit[0] as f64),
        ("tether_api_team_audit_dropped_total", "counter", "Team audit records dropped since start (queue full, TEAM_AUDIT_MAX_MB reached or a failed write; each drop is also a dropped record in the log).", (now.team_audit[1] + now.team_audit[2] + now.team_audit[3]) as f64),
        ("tether_api_team_audit_write_failed_total", "counter", "Team audit records that could not be written (disk full, folder not writable; counted in the dropped total too).", now.team_audit[3] as f64),
        ("tether_api_devices", "gauge", "Devices in the device list (heartbeat within 180 days).", now.devices as f64),
        ("tether_api_devices_active_24h", "gauge", "Devices with a heartbeat within 24 h.", now.devices_24h as f64),
        ("tether_api_data_bytes", "gauge", "Size of the data directory (releases, audit, diagnostics, state).", du as f64),
        ("tether_api_data_fs_free_bytes", "gauge", "Free space on the data directory's filesystem (for unprivileged users).", free as f64),
        ("tether_api_data_fs_size_bytes", "gauge", "Size of the data directory's filesystem.", size as f64),
    ] {
        header(&mut o, name, typ, help);
        let _ = writeln!(o, "{name} {v}");
    }
    header(&mut o, "tether_api_release_generation", "gauge", "Release generation G (bumped by every release index or latest.json change; hbbs pushes it).");
    let _ = writeln!(o, "tether_api_release_generation {}", now.release_generation);
    header(&mut o, "tether_api_release_index_serial", "gauge", "Serial of the stored signed release index, by channel.");
    for (c, s, _) in &now.indexes {
        let _ = writeln!(o, "tether_api_release_index_serial{{channel=\"{}\"}} {s}", label(c));
    }
    header(&mut o, "tether_api_release_index_expires_timestamp", "gauge", "Expiry (unix seconds) of the stored release index, by channel.");
    for (c, _, e) in &now.indexes {
        let _ = writeln!(o, "tether_api_release_index_expires_timestamp{{channel=\"{}\"}} {e}", label(c));
    }
    for (c, e) in &now.monitor_expires {
        let _ = writeln!(o, "tether_api_release_index_expires_timestamp{{channel=\"{}\",index=\"monitor\"}} {e}", label(c));
    }
    header(&mut o, "tether_api_devices_by_version", "gauge", "Devices with a heartbeat within 24 h, by profile and installed version.");
    for ((p, v), n) in &now.by_version {
        let _ = writeln!(o, "tether_api_devices_by_version{{profile=\"{}\",version=\"{}\"}} {n}", label(p), label(v));
    }
    header(&mut o, "tether_api_devices_update_state", "gauge", "Devices with a heartbeat within 24 h, by reported update state.");
    for (s, n) in &now.update_states {
        let _ = writeln!(o, "tether_api_devices_update_state{{state=\"{}\"}} {n}", label(s));
    }
    header(&mut o, "tether_api_release_downloads_total", "counter", "Devices that reported a newly downloaded (staged) update, by profile and version, since start.");
    for ((p, v), n) in &now.downloads {
        let _ = writeln!(o, "tether_api_release_downloads_total{{profile=\"{}\",version=\"{}\"}} {n}", label(p), label(v));
    }
    header(&mut o, "tether_api_status_hosts", "gauge", "Hosts with a status report, by state (stale: overdue for 3 intervals; unknown counts as ok).");
    for (s, n) in &now.status_hosts {
        let _ = writeln!(o, "tether_api_status_hosts{{state=\"{s}\"}} {n}");
    }
    let (rel_bytes, rel_versions) = crate::prune::usage(data_dir);
    header(&mut o, "tether_api_release_bytes", "gauge", "Bytes of release files below download/<channel>/, by channel.");
    for (c, b) in &rel_bytes {
        let _ = writeln!(o, "tether_api_release_bytes{{channel=\"{}\"}} {b}", label(c));
    }
    header(&mut o, "tether_api_release_versions", "gauge", "Release versions on disk, by channel, app and platform.");
    for (c, dir, n) in &rel_versions {
        let (app, platform) = dir.split_once('/').unwrap_or_default();
        let _ = writeln!(o, "tether_api_release_versions{{channel=\"{}\",app=\"{}\",platform=\"{}\"}} {n}", label(c), label(app), label(platform));
    }
    let last = crate::prune::last(data_dir).and_then(|r| r["time"].as_u64()).unwrap_or(0);
    for (name, typ, help, v) in [
        ("tether_api_release_prune_runs_total", "counter", "Executed release prunes since start.", crate::prune::RUNS.load(Relaxed)),
        ("tether_api_release_prune_freed_bytes_total", "counter", "Bytes deleted by release prunes since start.", crate::prune::FREED.load(Relaxed)),
        ("tether_api_release_prune_last_timestamp", "gauge", "Unix time of the last executed release prune (release-prune.jsonl; 0: none).", last),
    ] {
        header(&mut o, name, typ, help);
        let _ = writeln!(o, "{name} {v}");
    }
    header(&mut o, "tether_build_info", "gauge", "Build version.");
    let _ = writeln!(o, "tether_build_info{{version=\"{}\"}} 1", env!("CARGO_PKG_VERSION"));
    process(&mut o);
    o
}

fn dir_bytes(p: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(p) else { return 0 };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_bytes(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map_or(0, |m| m.len()),
            _ => 0,
        })
        .sum()
}

fn fs_space(p: &Path) -> (u64, u64) {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(p.as_os_str().as_bytes()) else { return (0, 0) };
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return (0, 0);
    }
    (s.f_bavail as u64 * s.f_frsize as u64, s.f_blocks as u64 * s.f_frsize as u64)
}

/// process_* metrics from /proc/self, named like the Prometheus client libraries do.
fn process(o: &mut String) {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let field = |k: &str| status.lines().find_map(|l| l.strip_prefix(k)?.split_whitespace().next()?.parse::<f64>().ok());
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as f64;
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    // Fields after "(comm) ": index 0 is field 3 (state); utime = 14, stime = 15, starttime = 22.
    let f: Vec<f64> = stat.rsplit_once(") ").map_or(vec![], |(_, r)| r.split(' ').map(|x| x.parse().unwrap_or(0.)).collect());
    let at = |n: usize| f.get(n - 3).copied().unwrap_or(0.);
    let btime = std::fs::read_to_string("/proc/stat")
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix("btime ")?.trim().parse::<f64>().ok())
        .unwrap_or(0.);
    let fds = std::fs::read_dir("/proc/self/fd").map_or(0, |d| d.count());
    let max_fds = std::fs::read_to_string("/proc/self/limits")
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix("Max open files")?.split_whitespace().next()?.parse::<f64>().ok())
        .unwrap_or(0.);
    for (name, typ, help, v) in [
        ("process_cpu_seconds_total", "counter", "User and system CPU time spent in seconds.", (at(14) + at(15)) / hz),
        ("process_resident_memory_bytes", "gauge", "Resident memory size in bytes.", field("VmRSS:").unwrap_or(0.) * 1024.),
        ("process_virtual_memory_bytes", "gauge", "Virtual memory size in bytes.", field("VmSize:").unwrap_or(0.) * 1024.),
        ("process_threads", "gauge", "Number of OS threads.", field("Threads:").unwrap_or(0.)),
        ("process_open_fds", "gauge", "Number of open file descriptors.", fds as f64),
        ("process_max_fds", "gauge", "Maximum number of open file descriptors.", max_fds),
        ("process_start_time_seconds", "gauge", "Start time of the process since unix epoch in seconds.", btime + at(22) / hz),
    ] {
        header(o, name, typ, help);
        let _ = writeln!(o, "{name} {v}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(text: &str, series: &str) -> f64 {
        text.lines().find_map(|l| l.strip_prefix(series)?.strip_prefix(' ')?.parse().ok()).unwrap_or(0.)
    }

    #[test]
    fn endpoints_are_a_fixed_set() {
        assert_eq!(ENDPOINTS[endpoint("GET", "/api/policy?x=1")], "policy");
        assert_eq!(ENDPOINTS[endpoint("POST", "/api/diag/ABCD")], "diag_upload");
        assert_eq!(ENDPOINTS[endpoint("GET", "/q/secret-token")], "custom_download");
        assert_eq!(ENDPOINTS[endpoint("PUT", "/api/admin/policy")], "admin");
        assert_eq!(ENDPOINTS[endpoint("GET", "/metrics")], "metrics");
        assert_eq!(ENDPOINTS[endpoint("GET", "/anything/else")], "other");
        assert_eq!(ENDPOINTS[endpoint("GET", "/download/stable/releases.json")], "release_index");
        assert_eq!(ENDPOINTS[endpoint("GET", "/download/stable/technician/windows-x86_64/latest.json")], "other");
        assert_eq!(ENDPOINTS[endpoint("POST", "/api/status")], "status");
        assert_eq!(ENDPOINTS[endpoint("POST", "/api/status/enroll")], "status");
        assert_eq!(ENDPOINTS[endpoint("GET", "/api/statusx")], "other");
        assert_eq!(ENDPOINTS[endpoint("GET", "/api/team/status")], "team");
        assert_eq!(ENDPOINTS[endpoint("DELETE", "/api/admin/status/dev-0001")], "admin");
    }

    #[test]
    fn counters_and_histogram_move() {
        let dir = std::env::temp_dir().join(format!("tether-api-metrics-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f"), [0u8; 100]).unwrap();
        let mut versions = std::collections::HashMap::new();
        for (v, n) in [("0.5.23", 5), ("0.5.24", 9), ("0.5.1", 1), ("x\"}", 1)] {
            versions.insert(("technician".to_owned(), v.to_owned()), n);
        }
        let now = Now {
            max_workers: 32,
            devices: 3,
            devices_24h: 2,
            release_generation: 7,
            indexes: vec![("stable".into(), 57, 1_765_119_600)],
            monitor_expires: vec![],
            by_version: top(versions, 2),
            update_states: vec![("staged".into(), 4)],
            downloads: vec![(("customer".into(), "0.5.24".into()), 3)],
            status_hosts: [("ok", 4), ("warn", 0), ("crit", 1), ("stale", 2)],
            status_sbom: (2, 81_920),
            team_audit: [7, 2, 1, 4],
        };
        let before = render(&now, &dir);
        observe(2, 204, Duration::from_millis(3));
        observe(2, 204, Duration::from_millis(300));
        BUSY_REJECTED.fetch_add(1, Relaxed);
        body_error(&std::io::ErrorKind::TimedOut.into());
        body_error(&std::io::ErrorKind::InvalidData.into());
        let t = render(&now, &dir);
        let d = |s: &str| value(&t, s) - value(&before, s);
        assert_eq!(d("tether_api_requests_total{endpoint=\"heartbeat\",code=\"204\"}"), 2.);
        assert_eq!(d("tether_api_request_duration_seconds_bucket{endpoint=\"heartbeat\",le=\"0.005\"}"), 1.);
        assert_eq!(d("tether_api_request_duration_seconds_bucket{endpoint=\"heartbeat\",le=\"0.5\"}"), 2.);
        assert_eq!(d("tether_api_request_duration_seconds_count{endpoint=\"heartbeat\"}"), 2.);
        assert_eq!(d("tether_api_busy_rejected_total"), 1.);
        assert_eq!(d("tether_api_body_timeouts_total"), 1.);
        assert_eq!(value(&t, "tether_api_workers_max"), 32.);
        assert_eq!(value(&t, "tether_api_devices_active_24h"), 2.);
        assert_eq!(value(&t, "tether_api_data_bytes"), 100.);
        assert!(value(&t, "tether_api_data_fs_size_bytes") > 0.);
        assert!(value(&t, "process_resident_memory_bytes") > 0.);
        assert_eq!(value(&t, "tether_api_release_generation"), 7.);
        assert_eq!(value(&t, "tether_api_release_index_serial{channel=\"stable\"}"), 57.);
        assert_eq!(value(&t, "tether_api_release_index_expires_timestamp{channel=\"stable\"}"), 1_765_119_600.);
        assert_eq!(value(&t, "tether_api_devices_by_version{profile=\"technician\",version=\"0.5.24\"}"), 9.);
        assert_eq!(value(&t, "tether_api_devices_by_version{profile=\"technician\",version=\"other\"}"), 2.);
        assert_eq!(value(&t, "tether_api_devices_update_state{state=\"staged\"}"), 4.);
        assert_eq!(value(&t, "tether_api_release_downloads_total{profile=\"customer\",version=\"0.5.24\"}"), 3.);
        assert_eq!(value(&t, "tether_api_status_hosts{state=\"stale\"}"), 2.);
        assert_eq!((value(&t, "tether_api_status_sbom_files"), value(&t, "tether_api_status_sbom_bytes")), (2., 81_920.));
        assert_eq!((value(&t, "tether_api_team_audit_records_total"), value(&t, "tether_api_team_audit_dropped_total"), value(&t, "tether_api_team_audit_write_failed_total")), (7., 7., 4.));
        assert!(!t.contains("x\"}"), "label values are filtered");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

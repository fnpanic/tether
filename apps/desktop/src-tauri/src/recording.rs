//! Session recordings on the viewer: where the core recorder writes, upload after the session
//! (with a persisted retry queue) and local retention. See `docs/recording.md`.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use librustdesk::ui::api;
use serde::{Deserialize, Serialize};

use super::session_policy as policy;

/// Serialises queue file access between session windows of this process.
static LOCK: Mutex<()> = Mutex::new(());

pub struct Settings {
    /// `none` | `unc` | `https-put`
    pub upload: String,
    pub target: String,
    pub token: String,
    pub keep_local: bool,
    pub max_days: u64,
}

impl Settings {
    pub fn from_policy() -> Self {
        Self {
            upload: policy::get("recording.upload"),
            target: policy::get("recording.upload-target"),
            token: policy::get("recording.upload-token"),
            keep_local: policy::get("recording.keep-local") != "N",
            max_days: policy::get("recording.max-days").trim().parse().unwrap_or(0),
        }
    }
}

/// `recording.folder` with `%VAR%` expanded; default `%USERPROFILE%\Videos\Tether` (macOS `~/Movies/Tether`).
pub fn folder() -> PathBuf {
    let f = policy::get("recording.folder");
    let default = if cfg!(target_os = "macos") { "%HOME%/Movies/Tether" } else { r"%USERPROFILE%\Videos\Tether" };
    let f = if f.trim().is_empty() { default.to_owned() } else { f };
    PathBuf::from(expand_env(&f, |k| std::env::var(k).ok()))
}

/// Points the core recorder (`video-save-directory`) at the policy folder.
pub fn apply_folder() {
    let dir = folder();
    if let Err(e) = fs::create_dir_all(&dir) {
        hbb_common::log::warn!("recording folder {}: {e}", dir.display());
    }
    api::set_local_option("video-save-directory".to_owned(), dir.to_string_lossy().into_owned());
}

fn expand_env(s: &str, var: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(a) = rest.find('%') {
        let Some(b) = rest[a + 1..].find('%') else { break };
        let name = &rest[a + 1..a + 1 + b];
        out.push_str(&rest[..a]);
        match var(name) {
            Some(v) if !name.is_empty() => out.push_str(&v),
            _ => out.push_str(&rest[a..a + b + 2]),
        }
        rest = &rest[a + b + 2..];
    }
    out.push_str(rest);
    out
}

fn queue_file() -> PathBuf {
    hbb_common::config::Config::path("recording-uploads.json")
}

/// Viewer recordings written by the core (`outgoing_<id>_<time>_...webm|mp4`).
fn is_recording(p: &Path) -> bool {
    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
    name.starts_with("outgoing") && (name.ends_with(".webm") || name.ends_with(".mp4"))
}

/// True while another handle (the recorder) still has the file open.
fn in_use(p: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        fs::OpenOptions::new().read(true).share_mode(0).open(p).is_err()
    }
    #[cfg(not(windows))]
    {
        let _ = p;
        false
    }
}

fn modified(p: &Path) -> Option<SystemTime> {
    fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Closed recordings in `dir` written at or after `since`.
pub fn finished_since(dir: &Path, since: SystemTime) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(dir) else { return vec![] };
    rd.flatten()
        .map(|e| e.path())
        .filter(|p| is_recording(p) && modified(p).is_some_and(|m| m >= since) && !in_use(p))
        .collect()
}

/// Persisted between runs so uploads survive a closed window or app restart.
#[derive(Default, Serialize, Deserialize)]
struct State {
    /// Recordings waiting for upload.
    #[serde(default)]
    pending: Vec<PathBuf>,
    /// Recordings closed after this time (ms since epoch) have not been picked up yet.
    #[serde(default)]
    since_ms: Option<u64>,
}

fn load(q: &Path) -> State {
    fs::read(q).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save(q: &Path, st: &State) {
    let res = if st.pending.is_empty() && st.since_ms.is_none() {
        fs::remove_file(q).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) })
    } else {
        serde_json::to_vec(st).map_err(std::io::Error::other).and_then(|b| fs::write(q, b))
    };
    if let Err(e) = res {
        hbb_common::log::warn!("recording upload queue {}: {e}", q.display());
    }
}

fn upload_one(s: &Settings, file: &Path) -> Result<(), String> {
    let name = file.file_name().and_then(|n| n.to_str()).ok_or("bad file name")?;
    if s.target.trim().is_empty() {
        return Err("recording.upload-target is empty".to_owned());
    }
    match s.upload.as_str() {
        "unc" => {
            let dest = Path::new(&s.target).join(name);
            // Copy under a temporary name so the share never shows a half-written file.
            let part = dest.with_file_name(format!("{name}.part"));
            fs::copy(file, &part).map_err(|e| e.to_string())?;
            fs::rename(&part, &dest).map_err(|e| e.to_string())
        }
        "https-put" => {
            let url = format!("{}/{name}", s.target.trim_end_matches('/'));
            api::http_put_file(&url, &s.token, file)
        }
        other => Err(format!("unknown upload mode {other:?}")),
    }
}

/// Adds `new` to the persisted queue and tries every queued upload; failures stay queued.
pub fn run_queue(queue: &Path, s: &Settings, new: &[PathBuf]) {
    let mut st = load(queue);
    let items = &mut st.pending;
    if s.upload == "none" || s.upload.is_empty() {
        items.clear();
    } else {
        for f in new {
            if !items.contains(f) {
                items.push(f.clone());
            }
        }
        items.retain(|f| {
            if !f.exists() {
                return false;
            }
            match upload_one(s, f) {
                Ok(()) => {
                    hbb_common::log::info!("recording uploaded: {}", f.display());
                    if !s.keep_local {
                        if let Err(e) = fs::remove_file(f) {
                            hbb_common::log::warn!("delete {}: {e}", f.display());
                        }
                    }
                    false
                }
                Err(e) => {
                    hbb_common::log::warn!("recording upload failed, will retry: {}: {e}", f.display());
                    true
                }
            }
        });
    }
    save(queue, &st);
}

/// Deletes recordings in `dir` older than `max_days` (0 = keep forever), except queued uploads.
pub fn prune(dir: &Path, queue: &Path, max_days: u64, now: SystemTime) {
    if max_days == 0 {
        return;
    }
    let keep = load(queue).pending;
    let Some(cutoff) = now.checked_sub(Duration::from_secs(max_days * 86_400)) else { return };
    let Ok(rd) = fs::read_dir(dir) else { return };
    for p in rd.flatten().map(|e| e.path()) {
        if is_recording(&p) && !keep.contains(&p) && modified(&p).is_some_and(|m| m < cutoff) {
            if let Err(e) = fs::remove_file(&p) {
                hbb_common::log::warn!("prune {}: {e}", p.display());
            }
        }
    }
}

fn ms(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// A recording is about to start: remember to pick up what gets written from now on.
pub fn started() {
    let _g = LOCK.lock().unwrap();
    let q = queue_file();
    let mut st = load(&q);
    if st.since_ms.is_none() {
        // Slack for clock granularity between this call and the recorder creating its file.
        st.since_ms = Some(ms(SystemTime::now()).saturating_sub(5_000));
        save(&q, &st);
    }
}

/// Picks up recordings closed since `started`, uploads everything queued, prunes old files.
/// Called when a recording stops, when a session window opens and at app start.
pub fn after_session() {
    let _g = LOCK.lock().unwrap();
    let (dir, s, q) = (folder(), Settings::from_policy(), queue_file());
    collect(&dir, &q, SystemTime::now());
    run_queue(&q, &s, &[]);
    prune(&dir, &q, s.max_days, SystemTime::now());
}

/// Moves closed recordings written since `since_ms` into the queue. While another recording is
/// still open the marker moves to `now`: that file's last write (and so its mtime) comes later.
fn collect(dir: &Path, q: &Path, now: SystemTime) {
    let mut st = load(q);
    let Some(since) = st.since_ms else { return };
    for f in finished_since(dir, UNIX_EPOCH + Duration::from_millis(since)) {
        if !st.pending.contains(&f) {
            st.pending.push(f);
        }
    }
    let open = fs::read_dir(dir)
        .map(|rd| rd.flatten().any(|e| is_recording(&e.path()) && in_use(&e.path())))
        .unwrap_or(false);
    st.since_ms = open.then(|| ms(now));
    save(q, &st);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tether-rec-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn settings(upload: &str, target: &Path, keep_local: bool) -> Settings {
        Settings {
            upload: upload.into(),
            target: target.to_string_lossy().into(),
            token: String::new(),
            keep_local,
            max_days: 0,
        }
    }

    #[test]
    fn expands_env_vars() {
        let v = |k: &str| (k == "USERPROFILE").then(|| r"C:\Users\a".to_owned());
        assert_eq!(expand_env(r"%USERPROFILE%\Videos\Tether", v), r"C:\Users\a\Videos\Tether");
        assert_eq!(expand_env(r"%NOPE%\x", v), r"%NOPE%\x");
        assert_eq!(expand_env("50% off", v), "50% off");
    }

    #[test]
    fn copies_to_share_and_deletes_local_when_not_kept() {
        let d = tmp("unc");
        let (local, share) = (d.join("local"), d.join("share"));
        fs::create_dir_all(&local).unwrap();
        fs::create_dir_all(&share).unwrap();
        let f = local.join("outgoing_1_a.webm");
        fs::write(&f, b"video").unwrap();
        let q = d.join("q.json");
        run_queue(&q, &settings("unc", &share, false), &[f.clone()]);
        assert_eq!(fs::read(share.join("outgoing_1_a.webm")).unwrap(), b"video");
        assert!(!f.exists());
        assert!(!q.exists(), "queue is empty after success");
    }

    #[test]
    fn failed_upload_stays_queued_and_is_retried() {
        let d = tmp("retry");
        let f = d.join("outgoing_2_b.webm");
        fs::write(&f, b"v").unwrap();
        let q = d.join("q.json");
        let share = d.join("share-later");
        // Share does not exist yet: the copy fails and the file is persisted in the queue.
        run_queue(&q, &settings("unc", &share, true), &[f.clone()]);
        assert_eq!(load(&q).pending, vec![f.clone()]);
        // Next start: share is back, nothing new, the queued file goes up and the copy is kept.
        fs::create_dir_all(&share).unwrap();
        run_queue(&q, &settings("unc", &share, true), &[]);
        assert!(share.join("outgoing_2_b.webm").exists());
        assert!(f.exists());
        assert!(load(&q).pending.is_empty());
    }

    #[test]
    fn collects_recordings_closed_since_start_once() {
        let d = tmp("collect");
        let q = d.join("q.json");
        let before = d.join("outgoing_before.webm");
        fs::write(&before, b"x").unwrap();
        let start = SystemTime::now() + Duration::from_secs(1);
        save(&q, &State { pending: vec![], since_ms: Some(ms(start)) });
        let rec = d.join("outgoing_rec.webm");
        fs::write(&rec, b"x").unwrap();
        fs::File::options().write(true).open(&rec).unwrap().set_modified(start + Duration::from_secs(1)).unwrap();
        collect(&d, &q, start + Duration::from_secs(2));
        let st = load(&q);
        assert_eq!(st.pending, vec![rec.clone()], "only files written after the start");
        assert_eq!(st.since_ms, None, "nothing left open, marker cleared");
        // A second pass (next window, next app start) finds nothing new.
        save(&q, &State::default());
        collect(&d, &q, SystemTime::now());
        assert!(load(&q).pending.is_empty());
    }

    #[test]
    fn prune_removes_only_old_unqueued_recordings() {
        let d = tmp("prune");
        let (old, queued, other) = (d.join("outgoing_old.webm"), d.join("outgoing_q.webm"), d.join("notes.txt"));
        for f in [&old, &queued, &other] {
            fs::write(f, b"x").unwrap();
        }
        let q = d.join("q.json");
        save(&q, &State { pending: vec![queued.clone()], since_ms: None });
        let later = SystemTime::now() + Duration::from_secs(3 * 86_400);
        prune(&d, &q, 0, later);
        assert!(old.exists(), "max-days 0 keeps everything");
        prune(&d, &q, 2, later);
        assert!(!old.exists());
        assert!(queued.exists(), "pending upload is never pruned");
        assert!(other.exists());
        let fresh = d.join("outgoing_new.webm");
        fs::write(&fresh, b"x").unwrap();
        prune(&d, &q, 2, SystemTime::now());
        assert!(fresh.exists());
    }
}

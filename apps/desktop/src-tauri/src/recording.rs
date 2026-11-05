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

/// Local option holding the folder the user picked in Settings (ignored while policy locks it).
const USER_FOLDER: &str = "tether-recording-folder";

pub fn folder_locked() -> bool {
    crate::policy::effective().is_locked("recording.folder")
}

/// The user's own folder from Settings, if any and allowed.
pub fn user_folder() -> Option<String> {
    let f = api::get_local_option(USER_FOLDER.to_owned());
    (!f.trim().is_empty() && !folder_locked()).then_some(f)
}

/// `""` goes back to the policy/default folder.
pub fn set_user_folder(path: &str) -> Result<(), String> {
    if folder_locked() {
        return Err("locked:recording.folder: Your organisation manages the recording folder.".to_owned());
    }
    api::set_local_option(USER_FOLDER.to_owned(), path.to_owned());
    Ok(())
}

/// The folder picked in Settings, else `recording.folder` with `%VAR%` expanded, else
/// `%USERPROFILE%\Videos\Tether` (macOS `~/Movies/Tether`).
pub fn folder() -> PathBuf {
    if let Some(f) = user_folder() {
        return PathBuf::from(f);
    }
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

/// Viewer recordings: as written by the core (`outgoing_<id>_<time>_display<n>_<codec>.webm|mp4`)
/// or renamed by `nice_name` (`2026-11-05_14-03-22_<peer name>_<id>[_display1].webm`).
fn is_recording(p: &Path) -> bool {
    p.file_name().and_then(|n| n.to_str()).is_some_and(|n| parse_name(n).is_some())
}

fn split_ext(name: &str) -> Option<(&str, &str)> {
    let (stem, ext) = name.rsplit_once('.')?;
    matches!(ext, "webm" | "mp4").then_some((stem, ext))
}

/// `[codec, display, time, id]` of a core name (the id may itself contain `_`).
fn core_parts(stem: &str) -> Option<Vec<&str>> {
    let p: Vec<&str> = stem.strip_prefix("outgoing_")?.rsplitn(4, '_').collect();
    (p.len() == 4 && !p[3].is_empty()).then_some(p)
}

/// `(peer name, peer id)` of a recording file name, `None` for anything else.
pub fn parse_name(name: &str) -> Option<(String, String)> {
    let (stem, _) = split_ext(name)?;
    if stem.starts_with("outgoing_") {
        return Some((String::new(), core_parts(stem)?[3].to_owned()));
    }
    // YYYY-mm-dd_HH-MM-SS_[<name>_]<id>[_display<n>]; the name part never contains `_`.
    let b = stem.as_bytes();
    let stamp = b.len() > 20
        && b[..19].iter().enumerate().all(|(i, c)| match i {
            4 | 7 | 13 | 16 => *c == b'-',
            10 => *c == b'_',
            _ => c.is_ascii_digit(),
        })
        && b[19] == b'_';
    if !stamp {
        return None;
    }
    let mut parts: Vec<&str> = stem[20..].split('_').collect();
    if parts.len() > 1 && parts.last().is_some_and(|p| p.starts_with("display")) {
        parts.pop();
    }
    let id = parts.pop().filter(|i| !i.is_empty())?;
    Some((parts.join("_"), id.to_owned()))
}

/// Peer host name as a file name part: letters, digits, `-` and `.`, at most 40 characters.
fn name_part(s: &str) -> String {
    let t: String = s.chars().take(40).map(|c| if c.is_alphanumeric() || c == '-' || c == '.' { c } else { '-' }).collect();
    t.trim_matches(|c| c == '-' || c == '.').to_owned()
}

/// `outgoing_<id>_<YYYYmmddHHMMSSmmm>_display<n>_<codec>.<ext>` →
/// `YYYY-mm-dd_HH-MM-SS_<peer name>_<id>[_display<n>].<ext>` (display 0 gets no suffix).
fn nice_name(core: &str, peer_name: &str) -> Option<String> {
    let (stem, ext) = split_ext(core)?;
    let p = core_parts(stem)?;
    let (display, t, id) = (p[1], p[2].as_bytes(), p[3]);
    if t.len() < 14 || !t[..14].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let d = |a: usize, b: usize| std::str::from_utf8(&t[a..b]).unwrap_or("");
    let when = format!("{}-{}-{}_{}-{}-{}", d(0, 4), d(4, 6), d(6, 8), d(8, 10), d(10, 12), d(12, 14));
    let name = name_part(peer_name);
    let name = if name.is_empty() { String::new() } else { format!("{name}_") };
    let disp = if display == "display0" { String::new() } else { format!("_{display}") };
    Some(format!("{when}_{name}{id}{disp}.{ext}"))
}

/// Renames a closed core recording to `nice_name` (peer name from the peer's saved info).
/// Keeps the old name when the new one is taken or the rename fails.
fn rename_nice(p: PathBuf) -> PathBuf {
    let Some(old) = p.file_name().and_then(|n| n.to_str()) else { return p };
    let Some((_, id)) = parse_name(old) else { return p };
    let host = hbb_common::config::PeerConfig::load(&id).info.hostname;
    let Some(new) = nice_name(old, &host) else { return p };
    let dest = p.with_file_name(new);
    if dest.exists() || fs::rename(&p, &dest).is_err() {
        return p;
    }
    dest
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
/// Returns the uploaded files and the failed ones with their error.
pub fn run_queue(queue: &Path, s: &Settings, new: &[PathBuf]) -> (Vec<PathBuf>, Vec<(PathBuf, String)>) {
    let (mut ok, mut failed) = (vec![], vec![]);
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
                    ok.push(f.clone());
                    if !s.keep_local {
                        if let Err(e) = fs::remove_file(f) {
                            hbb_common::log::warn!("delete {}: {e}", f.display());
                        }
                    }
                    false
                }
                Err(e) => {
                    hbb_common::log::warn!("recording upload failed, will retry: {}: {e}", f.display());
                    failed.push((f.clone(), e));
                    true
                }
            }
        });
    }
    save(queue, &st);
    (ok, failed)
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
/// Tells every window (`recordings` event): `{ saved: Recording[], upload }` once new files are
/// closed, then `{ uploaded: string[], failed: [name, error][] }` after an upload attempt.
pub fn after_session() {
    let _g = LOCK.lock().unwrap();
    let (dir, s, q) = (folder(), Settings::from_policy(), queue_file());
    let saved: Vec<Recording> = collect(&dir, &q, SystemTime::now()).iter().filter_map(|p| info(p, &[])).collect();
    if !saved.is_empty() {
        let upload = if matches!(s.upload.as_str(), "unc" | "https-put") { s.upload.as_str() } else { "none" };
        crate::app::emit("recordings", serde_json::json!({ "saved": saved, "upload": upload }));
    }
    let (ok, failed) = run_queue(&q, &s, &[]);
    if !ok.is_empty() || !failed.is_empty() {
        let n = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let failed: Vec<(String, String)> = failed.iter().map(|(p, e)| (n(p), e.clone())).collect();
        crate::app::emit("recordings", serde_json::json!({ "uploaded": ok.iter().map(n).collect::<Vec<_>>(), "failed": failed }));
    }
    prune(&dir, &q, s.max_days, SystemTime::now());
}

/// One recording for the UI.
#[derive(Serialize, Clone, Debug)]
pub struct Recording {
    /// File name inside the recordings folder (what the other commands take).
    pub name: String,
    pub peer_id: String,
    pub peer_name: String,
    /// Start (file creation, else last write) in ms since the epoch.
    pub started_ms: u64,
    /// Last write minus creation; 0 when the OS has no creation time.
    pub duration_s: u64,
    pub size: u64,
    /// Still waiting for upload.
    pub pending: bool,
}

fn info(p: &Path, pending: &[PathBuf]) -> Option<Recording> {
    let name = p.file_name()?.to_str()?.to_owned();
    let (peer_name, peer_id) = parse_name(&name)?;
    let m = fs::metadata(p).ok().filter(|m| m.is_file())?;
    let end = m.modified().ok()?;
    let start = m.created().ok().filter(|c| *c <= end).unwrap_or(end);
    Some(Recording {
        name,
        peer_id,
        peer_name,
        started_ms: ms(start),
        duration_s: end.duration_since(start).map(|d| d.as_secs()).unwrap_or(0),
        size: m.len(),
        pending: pending.iter().any(|f| f == p),
    })
}

/// Recordings in `dir`, newest first. Files still being written are left out.
pub fn list(dir: &Path, queue: &Path) -> Vec<Recording> {
    let pending = load(queue).pending;
    let Ok(rd) = fs::read_dir(dir) else { return vec![] };
    let mut v: Vec<Recording> = rd.flatten().map(|e| e.path()).filter(|p| is_recording(p) && !in_use(p)).filter_map(|p| info(&p, &pending)).collect();
    v.sort_by(|a, b| b.started_ms.cmp(&a.started_ms).then(b.name.cmp(&a.name)));
    v
}

pub fn list_current() -> Vec<Recording> {
    list(&folder(), &queue_file())
}

/// A recording's path from a file name the webview sent: a recording file name, directly inside
/// `dir`, that is an existing file. Anything else (separators, `..`, other files, links that lead
/// out of the folder) is refused.
pub fn resolve(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let bad = || "Not a recording in the recordings folder.".to_owned();
    if name.contains(['/', '\\', ':', '\0']) || name.starts_with('.') || parse_name(name).is_none() {
        return Err(bad());
    }
    let p = dir.join(name);
    let (cp, cd) = (p.canonicalize().map_err(|_| bad())?, dir.canonicalize().map_err(|_| bad())?);
    if cp.parent() != Some(cd.as_path()) || !cp.is_file() {
        return Err(bad());
    }
    Ok(p)
}

/// Deletes one recording in `dir` and drops it from the upload queue.
pub fn delete_in(dir: &Path, queue: &Path, name: &str) -> Result<(), String> {
    let p = resolve(dir, name)?;
    fs::remove_file(&p).map_err(|e| e.to_string())?;
    let mut st = load(queue);
    st.pending.retain(|f| f != &p);
    save(queue, &st);
    Ok(())
}

pub fn delete(name: &str) -> Result<(), String> {
    let _g = LOCK.lock().unwrap();
    delete_in(&folder(), &queue_file(), name)
}

/// Plays a recording with the OS default app (`reveal` false), selects it in the file manager
/// (`reveal` true), or opens the folder (`name` None).
pub fn open(name: Option<&str>, reveal: bool) -> Result<(), String> {
    use std::process::Command;
    let dir = folder();
    let tool = if cfg!(windows) { "explorer" } else if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let mut cmd = Command::new(tool);
    match name {
        None => {
            fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            cmd.arg(&dir);
        }
        // explorer parses `/select,` itself: the path goes in the same argument.
        Some(n) if reveal && cfg!(windows) => {
            cmd.arg(format!("/select,{}", resolve(&dir, n)?.display()));
        }
        Some(n) if reveal && cfg!(target_os = "macos") => {
            cmd.arg("-R").arg(resolve(&dir, n)?);
        }
        Some(n) if reveal => {
            resolve(&dir, n)?;
            cmd.arg(&dir);
        }
        // `explorer <file>` / `open <file>` start the default player, like a double-click.
        Some(n) => {
            cmd.arg(resolve(&dir, n)?);
        }
    }
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Moves closed recordings written since `since_ms` into the queue (renamed by `nice_name`) and
/// returns them. While another recording is still open the marker moves to `now`: that file's
/// last write (and so its mtime) comes later.
fn collect(dir: &Path, q: &Path, now: SystemTime) -> Vec<PathBuf> {
    let mut st = load(q);
    let Some(since) = st.since_ms else { return vec![] };
    let mut new = vec![];
    for f in finished_since(dir, UNIX_EPOCH + Duration::from_millis(since)) {
        if !st.pending.contains(&f) {
            let f = rename_nice(f);
            st.pending.push(f.clone());
            new.push(f);
        }
    }
    let open = fs::read_dir(dir)
        .map(|rd| rd.flatten().any(|e| is_recording(&e.path()) && in_use(&e.path())))
        .unwrap_or(false);
    st.since_ms = open.then(|| ms(now));
    save(q, &st);
    new
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
        let before = d.join("outgoing_1_20261105130000000_display0_vp9.webm");
        fs::write(&before, b"x").unwrap();
        let start = SystemTime::now() + Duration::from_secs(1);
        save(&q, &State { pending: vec![], since_ms: Some(ms(start)) });
        let rec = d.join("outgoing_1_20261105140322123_display0_vp9.webm");
        fs::write(&rec, b"x").unwrap();
        fs::File::options().write(true).open(&rec).unwrap().set_modified(start + Duration::from_secs(1)).unwrap();
        collect(&d, &q, start + Duration::from_secs(2));
        let st = load(&q);
        assert_eq!(st.pending, vec![d.join("2026-11-05_14-03-22_1.webm")], "only files written after the start, renamed");
        assert_eq!(st.since_ms, None, "nothing left open, marker cleared");
        // A second pass (next window, next app start) finds nothing new.
        save(&q, &State::default());
        collect(&d, &q, SystemTime::now());
        assert!(load(&q).pending.is_empty());
    }

    #[test]
    fn names_recordings_by_time_peer_name_and_id() {
        let core = "outgoing_184392571_20261105140322123_display0_vp9.webm";
        assert_eq!(nice_name(core, "maria@Design Studio").as_deref(), Some("2026-11-05_14-03-22_maria-Design-Studio_184392571.webm"));
        assert_eq!(
            nice_name("outgoing_184392571_20261105140322123_display1_h264.mp4", "").as_deref(),
            Some("2026-11-05_14-03-22_184392571_display1.mp4"),
            "no name: date, time and ID; other displays keep their number"
        );
        assert_eq!(nice_name("outgoing_x_y.webm", "a"), None);
        assert_eq!(nice_name("2026-11-05_14-03-22_a_1.webm", "a"), None, "already renamed");
        // Round trip, old names and other files.
        assert_eq!(parse_name("2026-11-05_14-03-22_maria-Design-Studio_184392571.webm"), Some(("maria-Design-Studio".into(), "184392571".into())));
        assert_eq!(parse_name("2026-11-05_14-03-22_184392571_display1.mp4"), Some(("".into(), "184392571".into())));
        assert_eq!(parse_name(core), Some(("".into(), "184392571".into())));
        for other in ["notes.txt", "2026-11-05_14-03-22_.webm", "holiday.mp4", "2026-11-05_14-03-22_a_1.mkv", "outgoing_1.webm"] {
            assert_eq!(parse_name(other), None, "{other}");
        }
    }

    #[test]
    fn collect_renames_closed_recordings() {
        let d = tmp("rename");
        let q = d.join("q.json");
        let start = SystemTime::now() - Duration::from_secs(10);
        save(&q, &State { pending: vec![], since_ms: Some(ms(start)) });
        fs::write(d.join("outgoing_999000111_20261105140322123_display0_vp9.webm"), b"x").unwrap();
        let new = collect(&d, &q, SystemTime::now());
        // No saved peer 999000111 here, so no name part.
        assert_eq!(new, vec![d.join("2026-11-05_14-03-22_999000111.webm")]);
        assert!(new[0].exists());
        assert_eq!(load(&q).pending, new);
    }

    #[test]
    fn lists_recordings_newest_first() {
        let d = tmp("list");
        let q = d.join("q.json");
        let (a, b) = (d.join("2026-11-04_09-00-00_pc_1.webm"), d.join("outgoing_2_20261105140322123_display0_vp9.webm"));
        fs::write(&a, b"aaaa").unwrap();
        fs::write(&b, b"bb").unwrap();
        fs::write(d.join("notes.txt"), b"x").unwrap();
        fs::create_dir(d.join("2026-11-04_09-00-00_dir_3.webm")).unwrap();
        save(&q, &State { pending: vec![a.clone()], since_ms: None });
        let old = SystemTime::now() - Duration::from_secs(86_400);
        fs::File::options().write(true).open(&a).unwrap().set_modified(old).unwrap();
        let v = list(&d, &q);
        assert_eq!(v.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), vec![b.file_name().unwrap().to_str().unwrap(), "2026-11-04_09-00-00_pc_1.webm"]);
        let r = &v[1];
        assert_eq!((r.peer_name.as_str(), r.peer_id.as_str(), r.size, r.pending), ("pc", "1", 4, true));
        assert!(!v[0].pending);
    }

    #[test]
    fn resolve_refuses_anything_outside_the_folder() {
        let d = tmp("resolve");
        let dir = d.join("rec");
        fs::create_dir_all(&dir).unwrap();
        let ok = "2026-11-05_14-03-22_pc_1.webm";
        fs::write(dir.join(ok), b"x").unwrap();
        // Same-looking recording one level up: reachable only by traversal.
        fs::write(d.join(ok), b"x").unwrap();
        fs::write(dir.join("notes.txt"), b"x").unwrap();
        assert_eq!(resolve(&dir, ok).unwrap(), dir.join(ok));
        for bad in [
            "",
            "notes.txt",
            "../2026-11-05_14-03-22_pc_1.webm",
            "..\\2026-11-05_14-03-22_pc_1.webm",
            "sub/2026-11-05_14-03-22_pc_1.webm",
            "C:2026-11-05_14-03-22_pc_1.webm",
            "2026-11-05_14-03-22_pc_2.webm", // does not exist
            "..",
        ] {
            assert!(resolve(&dir, bad).is_err(), "{bad:?}");
        }
        let abs = d.join(ok).to_string_lossy().into_owned();
        assert!(resolve(&dir, &abs).is_err(), "absolute path");
    }

    #[test]
    fn delete_removes_file_and_queue_entry_only_inside_folder() {
        let d = tmp("delete");
        let dir = d.join("rec");
        fs::create_dir_all(&dir).unwrap();
        let name = "2026-11-05_14-03-22_pc_1.webm";
        let (f, outside) = (dir.join(name), d.join(name));
        fs::write(&f, b"x").unwrap();
        fs::write(&outside, b"x").unwrap();
        let q = d.join("q.json");
        save(&q, &State { pending: vec![f.clone()], since_ms: None });
        assert!(delete_in(&dir, &q, &format!("../{name}")).is_err());
        assert!(outside.exists());
        delete_in(&dir, &q, name).unwrap();
        assert!(!f.exists());
        assert!(load(&q).pending.is_empty());
    }

    #[test]
    fn prune_removes_only_old_unqueued_recordings() {
        let d = tmp("prune");
        let (old, queued, other, video) = (
            d.join("2026-11-01_10-00-00_pc_1.webm"),
            d.join("outgoing_1_20261101100000000_display0_vp9.webm"),
            d.join("notes.txt"),
            d.join("holiday.mp4"),
        );
        for f in [&old, &queued, &other, &video] {
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
        assert!(video.exists(), "videos that are not recordings stay");
        let fresh = d.join("2026-11-05_10-00-00_pc_1.webm");
        fs::write(&fresh, b"x").unwrap();
        prune(&d, &q, 2, SystemTime::now());
        assert!(fresh.exists());
    }
}

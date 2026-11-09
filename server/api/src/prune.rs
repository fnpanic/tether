//! Release clean-up (docs/release-cleanup.md): `POST /api/admin/releases/prune` (plan or execute),
//! the guard on `DELETE /api/admin/release/…`, the prune log `release-prune.jsonl` and the
//! release figures for `/metrics`. The retention rule itself is `release::prune_plan`.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use super::*;
use tether_trust::release::{DirFile, PlanVersion, Protect, PRUNE_KEEP, PRUNE_KEEP_MAX};

pub const LIMIT_PRUNE: u64 = 64 << 10;
const LOG: &str = "release-prune.jsonl";
/// Records kept in the log (prune runs and forced deletes).
const LOG_KEEP: usize = 100;

pub static RUNS: AtomicU64 = AtomicU64::new(0);
pub static FREED: AtomicU64 = AtomicU64::new(0);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PruneRequest {
    dry_run: bool,
    /// Older versions kept besides the published one; default 2, 0..=20.
    #[serde(default)]
    keep: Option<usize>,
    #[serde(default)]
    pin: Vec<Ver>,
    /// Execute only: the versions to delete (normally the reviewed plan's drop list).
    #[serde(default)]
    delete: Option<Vec<Ver>>,
}

#[derive(Deserialize, Serialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
struct Ver {
    channel: String,
    /// `<app>/<platform>`
    dir: String,
    version: String,
}

/// One folder's plan.
struct Group {
    channel: String,
    dir: String,
    published: Option<String>,
    index_serial: Option<u64>,
    keep: Vec<PlanVersion>,
    drop: Vec<PlanVersion>,
}

fn dirs(p: &Path) -> impl Iterator<Item = (String, PathBuf)> {
    fs::read_dir(p).into_iter().flatten().flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).filter_map(|e| Some((e.file_name().to_str()?.to_owned(), e.path())))
}

/// Every release folder `download/<channel>/<app>/<platform>/` with its files.
pub fn scan(download: &Path) -> BTreeMap<(String, String), Vec<DirFile>> {
    let mut out = BTreeMap::new();
    for (ch, chp) in dirs(download).filter(|(c, _)| valid_channel(c)) {
        for (app, ap) in dirs(&chp) {
            for (plat, pp) in dirs(&ap) {
                let files = fs::read_dir(&pp)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter_map(|e| {
                        let m = e.metadata().ok().filter(|m| m.is_file())?;
                        let modified = m.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs();
                        Some(DirFile { name: e.file_name().to_str()?.to_owned(), size: m.len(), modified })
                    })
                    .collect();
                out.insert((ch.clone(), format!("{app}/{plat}")), files);
            }
        }
    }
    out
}

/// The stored index, verified against the key it names (it was checked against the trusted keys
/// on upload; an index from a rotated-out key still protects what it names). `Err`: unreadable.
fn stored_index(raw: &[u8]) -> Result<release::Index, ()> {
    let doc: envelope::Signed = serde_json::from_slice(raw).map_err(drop)?;
    let key = tether_trust::key_from_b64(&doc.key).map_err(drop)?;
    release::Index::from_json(&envelope::open(&doc, &[key]).map_err(drop)?).map_err(drop)
}

/// What publishes a folder now. `Err`: a `releases.json` or `latest.json` that cannot be read, so
/// what it publishes is unknown and nothing in the folder may go.
fn published(download: &Path, channel: &str, dir: &str) -> Result<(Protect, Option<u64>), ()> {
    let mut p = Protect::default();
    let mut serial = None;
    if let Ok(raw) = fs::read(download.join(channel).join("releases.json")) {
        let ix = stored_index(&raw)?;
        serial = Some(ix.serial);
        p.index = ix.releases.get(dir).map(|e| e.version.clone());
    }
    if let Ok(raw) = fs::read(download.join(channel).join(dir).join("latest.json")) {
        let m: serde_json::Value = serde_json::from_slice(&raw).map_err(drop)?;
        p.latest = Some(m["version"].as_str().filter(|v| release::valid_version(v)).ok_or(())?.to_owned());
    }
    Ok((p, serial))
}

impl App {
    fn plan(&self, keep: usize, pins: &[Ver]) -> Vec<Group> {
        let download = self.data.join("download");
        let page = fs::read_to_string(download.join("index.html")).unwrap_or_default();
        let t = now();
        let mut out = Vec::new();
        for ((channel, dir), files) in scan(&download) {
            let (app, platform) = dir.split_once('/').unwrap_or_default();
            let (mut keep_l, mut drop_l, published_v, serial) = match published(&download, &channel, &dir) {
                Ok((mut p, serial)) => {
                    let linked = |f: &&DirFile| page.contains(&format!("/{channel}/{dir}/{}", f.name));
                    p.page = files.iter().filter(linked).filter_map(|f| release::file_version(app, platform, &f.name)).collect();
                    p.pins = pins.iter().filter(|v| v.channel == channel && v.dir == dir).map(|v| v.version.clone()).collect();
                    let (k, d) = release::prune_plan(app, platform, &files, &p, keep, t);
                    let pv = [&p.index, &p.latest].into_iter().flatten().max_by(|a, b| tether_trust::version::compare(a, b).unwrap_or(std::cmp::Ordering::Equal)).cloned();
                    (k, d, pv, serial)
                }
                Err(()) => {
                    let (mut k, d) = release::prune_plan(app, platform, &files, &Protect::default(), keep, t);
                    k.extend(d);
                    k.iter_mut().for_each(|v| v.why = vec!["unreadable release index or latest.json"]);
                    (k, vec![], None, None)
                }
            };
            keep_l.sort_by(|a, b| tether_trust::version::compare(&b.version, &a.version).unwrap_or(std::cmp::Ordering::Equal));
            drop_l.sort_by(|a, b| tether_trust::version::compare(&b.version, &a.version).unwrap_or(std::cmp::Ordering::Equal));
            if keep_l.is_empty() && drop_l.is_empty() {
                continue;
            }
            out.push(Group { channel, dir, published: published_v, index_serial: serial, keep: keep_l, drop: drop_l });
        }
        out
    }

    /// `POST /api/admin/releases/prune`.
    pub(crate) fn prune(&mut self, body: &[u8]) -> Result<Resp, Resp> {
        let req: PruneRequest = serde_json::from_slice(body).map_err(|e| text(400, &e.to_string()))?;
        let keep = req.keep.unwrap_or(PRUNE_KEEP);
        if keep > PRUNE_KEEP_MAX {
            return Err(text(400, &format!("keep must be 0..={PRUNE_KEEP_MAX}")));
        }
        let groups = self.plan(keep, &req.pin);
        if req.dry_run {
            let drops = || groups.iter().flat_map(|g| &g.drop);
            let (drop_bytes, drop_files) = (drops().map(|v| v.bytes).sum::<u64>(), drops().map(|v| v.files.len()).sum::<usize>());
            let kept_bytes: u64 = groups.iter().flat_map(|g| &g.keep).map(|v| v.bytes).sum();
            let groups: Vec<_> = groups
                .iter()
                .map(|g| serde_json::json!({"channel": g.channel, "dir": g.dir, "published": g.published, "index_serial": g.index_serial, "keep": g.keep, "drop": g.drop}))
                .collect();
            return Ok(json(&serde_json::json!({"keep_older": keep, "groups": groups, "drop_bytes": drop_bytes, "drop_files": drop_files, "kept_bytes": kept_bytes})));
        }
        let delete = req.delete.ok_or_else(|| text(400, "delete: list the versions to delete (from a dry run)"))?;
        let (mut deleted, mut kept, mut gone, mut failed) = (vec![], vec![], vec![], vec![]);
        let (mut freed, mut files_n) = (0u64, 0usize);
        let download = self.data.join("download");
        for d in delete {
            let g = groups.iter().find(|g| g.channel == d.channel && g.dir == d.dir);
            if let Some(v) = g.and_then(|g| g.drop.iter().find(|v| v.version == d.version)) {
                let folder = download.join(&d.channel).join(&d.dir);
                let errs: Vec<String> = v.files.iter().filter_map(|f| fs::remove_file(folder.join(f)).err().map(|e| format!("{f}: {e}"))).collect();
                self.remove_empty_dirs(&folder);
                if errs.is_empty() {
                    freed += v.bytes;
                    files_n += v.files.len();
                    deleted.push(serde_json::json!({"channel": d.channel, "dir": d.dir, "version": d.version, "files": v.files, "bytes": v.bytes}));
                } else {
                    failed.push(serde_json::json!({"channel": d.channel, "dir": d.dir, "version": d.version, "error": errs.join("; ")}));
                }
            } else if let Some(v) = g.and_then(|g| g.keep.iter().find(|v| v.version == d.version)) {
                kept.push(serde_json::json!({"channel": d.channel, "dir": d.dir, "version": d.version, "result": "kept", "why": v.why}));
            } else {
                gone.push(serde_json::json!({"channel": d.channel, "dir": d.dir, "version": d.version, "result": "gone"}));
            }
        }
        RUNS.fetch_add(1, Relaxed);
        FREED.fetch_add(freed, Relaxed);
        eprintln!("release prune: {} versions, {files_n} files, {freed} bytes deleted; {} kept, {} gone, {} failed (keep {keep})", deleted.len(), kept.len(), gone.len(), failed.len());
        let out = serde_json::json!({"deleted": deleted, "kept": kept, "gone": gone, "failed": failed, "freed_bytes": freed});
        let mut rec = out.clone();
        rec["time"] = now().into();
        rec["files"] = files_n.into();
        rec["keep_older"] = keep.into();
        self.log(&rec);
        Ok(json(&out))
    }

    /// `DELETE /api/admin/release/<channel>/<path>`: 409 for `releases.json`, any `latest.json` and
    /// every file of a version the index or `latest.json` publishes, unless `?force=1`.
    pub(crate) fn delete_guarded(&mut self, dest: &Path, url: &str) -> Result<Resp, Resp> {
        let force = url.split_once('?').is_some_and(|(_, q)| q.split('&').any(|kv| kv == "force=1"));
        let download = self.data.join("download");
        let rel = dest.strip_prefix(&download).unwrap_or(dest).to_string_lossy().into_owned();
        if !force {
            if let Some(why) = self.published_file(&download, &rel) {
                return Err(text(409, &format!("published release: {why}; add ?force=1 to delete it anyway")));
            }
        }
        let size = fs::metadata(dest).map_or(0, |m| m.len());
        let r = self.delete_release(dest)?;
        if force {
            eprintln!("release delete (forced): {rel}");
            self.log(&serde_json::json!({"time": now(), "forced_delete": rel, "bytes": size}));
        }
        Ok(r)
    }

    /// Why deleting `<channel>/<path>` would break what is published, if it would.
    fn published_file(&self, download: &Path, rel: &str) -> Option<String> {
        let parts: Vec<&str> = rel.split('/').collect();
        let name = *parts.last()?;
        if name == release::INDEX_FILE || name == release::INDEX_V2_FILE || name == "latest.json" {
            return Some(format!("{rel} is what devices read"));
        }
        let [channel, app, platform, file] = parts[..] else { return None };
        let v = release::file_version(app, platform, file)?;
        let dir = format!("{app}/{platform}");
        let Ok((p, _)) = published(download, channel, &dir) else {
            return Some(format!("the release index or latest.json of {channel}/{dir} cannot be read"));
        };
        let mut why = vec![];
        if p.index.as_deref() == Some(v.as_str()) {
            why.push("the release index");
        }
        if p.latest.as_deref() == Some(v.as_str()) {
            why.push("latest.json");
        }
        (!why.is_empty()).then(|| format!("{channel}/{dir} {v} is in {}", why.join(" and ")))
    }

    /// Appends to `release-prune.jsonl`, keeping the last [`LOG_KEEP`] records.
    fn log(&mut self, rec: &serde_json::Value) {
        let path = self.data.join(LOG);
        let old = fs::read_to_string(&path).unwrap_or_default();
        let mut lines: Vec<&str> = old.lines().filter(|l| !l.trim().is_empty()).collect();
        let new = rec.to_string();
        lines.push(&new);
        let start = lines.len().saturating_sub(LOG_KEEP);
        let body = lines[start..].join("\n") + "\n";
        if let Err(e) = self.write_atomic(&path, body.as_bytes(), 0o600) {
            eprintln!("{LOG}: {e}");
        }
    }
}

/// The last prune run from the log: `{time, files, bytes}` (status, metrics).
pub fn last(data: &Path) -> Option<serde_json::Value> {
    let log = fs::read_to_string(data.join(LOG)).ok()?;
    let r: serde_json::Value = log.lines().rev().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()).find(|r| r.get("freed_bytes").is_some())?;
    Some(serde_json::json!({"time": r["time"], "files": r["files"], "bytes": r["freed_bytes"]}))
}

/// `/metrics`: bytes per channel and versions per release folder.
pub fn usage(data: &Path) -> (BTreeMap<String, u64>, Vec<(String, String, usize)>) {
    let mut bytes: BTreeMap<String, u64> = BTreeMap::new();
    let mut versions = vec![];
    for ((ch, dir), files) in scan(&data.join("download")) {
        *bytes.entry(ch.clone()).or_default() += files.iter().map(|f| f.size).sum::<u64>();
        let (app, platform) = dir.split_once('/').unwrap_or_default();
        let mut vs: Vec<String> = files.iter().filter_map(|f| release::file_version(app, platform, &f.name)).collect();
        vs.sort();
        vs.dedup();
        versions.push((ch, dir, vs.len()));
    }
    (bytes, versions)
}

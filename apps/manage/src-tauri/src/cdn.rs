//! Release downloads on Cloudflare Workers Static Assets (decision D1, docs/updates.md "CDN").
//! Each publish deploys a new version of the Worker `tether-dl` whose assets are the update files
//! of the current and the previous release of every channel/app/platform, served at
//! `<base>/<channel>/<app>/<platform>/<file>`. The CDN is untrusted: clients check every byte
//! against the signed release index and fall back to the relays.
//!
//! API flow (developers.cloudflare.com/workers/static-assets/direct-upload): register the manifest
//! (`assets-upload-session`), upload the buckets of files Cloudflare does not have yet (base64,
//! multipart, one request per bucket, authorised with the session JWT), then upload the script with
//! the completion token. Account API token and account id: Tether Manage › Releases › CDN (sealed in
//! the key vault), or a `cf.env` file (`CLOUDFLARE_API_TOKEN`, `CLOUDFLARE_ACCOUNT_ID`).

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

use crate::api;

pub const API: &str = "https://api.cloudflare.com/client/v4";
pub const DEFAULT_WORKER: &str = "tether-dl";
pub const DEFAULT_BASE: &str = "https://tether-dl.thomas-schend.workers.dev";
/// The script, only run for paths without an asset (wrangler.toml: never `run_worker_first`).
const SCRIPT: &str = include_str!("../../../../server/cloudflare/worker/src/index.js");
const COMPAT_DATE: &str = "2026-10-01";
/// Versions kept per channel/app/platform: the current one and the one before (devices still
/// downloading it, halts).
const KEEP: usize = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    pub hash: String,
    pub size: u64,
    /// `channel/app/platform`
    pub group: String,
    pub version: String,
}

/// What the deployed version holds (`cdn-assets.json` in the Manage data dir): `/path` → asset.
pub type Assets = BTreeMap<String, Asset>;

/// Cloudflare's asset hash: the first 32 hex digits of SHA-256(base64(content) ‖ extension).
pub fn asset_hash(content: &[u8], path: &str) -> String {
    let file = path.rsplit('/').next().unwrap_or(path);
    let ext = file.rsplit_once('.').map_or("", |(_, e)| e);
    let mut h = Sha256::new();
    h.update(B64.encode(content).as_bytes());
    h.update(ext.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>()[..32].to_owned()
}

pub fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "exe" => "application/vnd.microsoft.portable-executable",
        "gz" => "application/gzip",
        "zip" => "application/zip",
        "dmg" => "application/x-apple-diskimage",
        "sig" | "txt" => "text/plain",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
}

/// Refuse a file the Workers Static Assets limit would reject.
pub fn check_size(path: &str, size: u64) -> Result<(), String> {
    if size > tether_trust::release::CDN_MAX_FILE {
        return Err(format!(
            "{path} is {:.1} MiB; the CDN takes at most 25 MiB per file. Publish it compressed or relay-only.",
            size as f64 / (1 << 20) as f64
        ));
    }
    Ok(())
}

/// The next version's assets: `current` plus `new` (`(path, bytes)` of one release), then only the
/// newest [`KEEP`] versions of each group.
pub fn next_assets(current: &Assets, group: &str, version: &str, new: &[(String, &[u8])]) -> Result<Assets, String> {
    let mut out = current.clone();
    for (path, bytes) in new {
        check_size(path, bytes.len() as u64)?;
        out.insert(path.clone(), Asset { hash: asset_hash(bytes, path), size: bytes.len() as u64, group: group.into(), version: version.into() });
    }
    let mut versions: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for a in out.values() {
        let v = versions.entry(a.group.as_str()).or_default();
        if !v.contains(&a.version.as_str()) {
            v.push(&a.version);
        }
    }
    let keep: Vec<(String, String)> = versions
        .into_iter()
        .flat_map(|(g, mut vs)| {
            vs.sort_by(|a, b| tether_trust::version::compare(b, a).unwrap_or(std::cmp::Ordering::Equal));
            vs.into_iter().take(KEEP).map(move |v| (g.to_owned(), v.to_owned()))
        })
        .collect();
    out.retain(|_, a| keep.contains(&(a.group.clone(), a.version.clone())));
    Ok(out)
}

/// A `multipart/form-data` body: `(name, filename, content type, bytes)` parts.
pub fn multipart(parts: &[(&str, Option<&str>, &str, &[u8])]) -> (String, Vec<u8>) {
    let boundary = format!("tether-{}", &asset_hash(format!("{:?}", std::time::SystemTime::now()).as_bytes(), "b")[..16]);
    let mut body = Vec::new();
    for (name, filename, ct, data) in parts {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        let disp = match filename {
            Some(f) => format!("Content-Disposition: form-data; name=\"{name}\"; filename=\"{f}\"\r\n"),
            None => format!("Content-Disposition: form-data; name=\"{name}\"\r\n"),
        };
        body.extend_from_slice(disp.as_bytes());
        body.extend_from_slice(format!("Content-Type: {ct}\r\n\r\n").as_bytes());
        body.extend_from_slice(data);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

#[derive(Clone, Debug)]
pub struct Creds {
    pub account_id: String,
    pub token: String,
    pub worker: String,
}

/// `cf.env` format (`KEY=value` lines, optional quotes, `#` comments): token and account id.
pub fn parse_env(text: &str) -> Result<(String, String), String> {
    let mut m = BTreeMap::new();
    for l in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        if let Some((k, v)) = l.trim_start_matches("export ").split_once('=') {
            m.insert(k.trim().to_owned(), v.trim().trim_matches(['"', '\'']).to_owned());
        }
    }
    let get = |k: &str| m.get(k).filter(|v| !v.is_empty()).cloned().ok_or_else(|| format!("{k} is missing in the file"));
    Ok((get("CLOUDFLARE_API_TOKEN")?, get("CLOUDFLARE_ACCOUNT_ID")?))
}

/// A small API call: Cloudflare must start answering within [`api::ANSWER`] (the client's own
/// limit is the 30 min of a release upload).
async fn call(req: reqwest::RequestBuilder) -> Result<Value, String> {
    let res = match tokio::time::timeout(api::ANSWER, req.send()).await {
        Ok(r) => r.map_err(|e| format!("Cloudflare: {e}"))?,
        Err(_) => return Err(format!("Cloudflare: no answer within {} s", api::ANSWER.as_secs())),
    };
    answer(res).await
}

async fn answer(res: reqwest::Response) -> Result<Value, String> {
    let (status, body) = api::read_all(res).await.map_err(|e| format!("Cloudflare: {e}"))?;
    let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    if !(200..300).contains(&status) || v["success"] == false {
        let msg = v["errors"].as_array().map(|e| e.iter().map(|e| format!("{} {}", e["code"], e["message"].as_str().unwrap_or(""))).collect::<Vec<_>>().join("; "));
        return Err(format!("Cloudflare answered HTTP {status}: {}", msg.filter(|m| !m.is_empty()).unwrap_or_else(|| "no details".into())));
    }
    Ok(v)
}

/// Progress of a deploy: `(step, sent, total)`; bytes only while uploading (else 0, 0).
pub type OnProgress = std::sync::Arc<dyn Fn(&str, u64, u64) + Send + Sync>;

/// Deploy a version with `assets`; `content(path)` gives the bytes of any file Cloudflare asks for.
pub async fn deploy<F, Fut>(client: &reqwest::Client, api: &str, c: &Creds, assets: &Assets, content: F, on: OnProgress) -> Result<(), String>
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<u8>, String>>,
{
    let acct = format!("{api}/accounts/{}", c.account_id);
    on("preparing the upload", 0, 0);
    let manifest: serde_json::Map<String, Value> = assets.iter().map(|(p, a)| (p.clone(), json!({ "hash": a.hash, "size": a.size }))).collect();
    let session = call(client.post(format!("{acct}/workers/scripts/{}/assets-upload-session", c.worker)).bearer_auth(&c.token).header("Content-Type", "application/json").body(serde_json::to_vec(&json!({ "manifest": manifest })).unwrap())).await?;
    let mut jwt = session["result"]["jwt"].as_str().ok_or("Cloudflare sent no upload token")?.to_owned();
    let buckets: Vec<Vec<String>> = serde_json::from_value(session["result"]["buckets"].clone()).unwrap_or_default();
    let by_hash: BTreeMap<&str, &str> = assets.iter().map(|(p, a)| (a.hash.as_str(), p.as_str())).collect();
    if buckets.is_empty() {
        on("files already on Cloudflare", 0, 0);
    }
    for bucket in &buckets {
        let mut files = Vec::new();
        let mut names = Vec::new();
        for h in bucket {
            let path = by_hash.get(h.as_str()).ok_or_else(|| format!("Cloudflare asked for an unknown file {h}"))?;
            let name = path.rsplit('/').next().unwrap_or(path).to_owned();
            on(&format!("reading {name}"), 0, 0);
            let bytes = content(path.to_string()).await?;
            if asset_hash(&bytes, path) != *h {
                return Err(format!("{path} changed since it was published; publish the release again"));
            }
            files.push((h.clone(), content_type(path), B64.encode(bytes)));
            names.push(name);
        }
        let parts: Vec<(&str, Option<&str>, &str, &[u8])> = files.iter().map(|(h, ct, b)| (h.as_str(), Some(h.as_str()), *ct, b.as_bytes())).collect();
        let (ct, body) = multipart(&parts);
        let (len, step) = (body.len() as u64, format!("uploading {}", names.join(", ")));
        on(&step, 0, len);
        // Bounded like a relay upload (api::STALL while sending, api::ANSWER after the last byte).
        let on2 = on.clone();
        let req = client.post(format!("{acct}/workers/assets/upload?base64=true")).bearer_auth(&jwt).header("Content-Type", ct);
        let res = api::upload(req, body, move |n| on2(&step, n, len)).await.map_err(|e| format!("Cloudflare: {e}"))?;
        let r = answer(res).await?;
        if let Some(t) = r["result"]["jwt"].as_str() {
            jwt = t.to_owned(); // the completion token, after the last bucket
        }
    }
    on("activating the new version", 0, 0);
    let meta = json!({
        "main_module": "index.js",
        "compatibility_date": COMPAT_DATE,
        "assets": { "jwt": jwt, "config": { "not_found_handling": "none" } },
    });
    let meta = serde_json::to_vec(&meta).unwrap();
    let (ct, body) = multipart(&[("metadata", None, "application/json", &meta), ("index.js", Some("index.js"), "application/javascript+module", SCRIPT.as_bytes())]);
    call(client.put(format!("{acct}/workers/scripts/{}", c.worker)).bearer_auth(&c.token).header("Content-Type", ct).body(body)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};

    #[test]
    fn hash_matches_the_documented_algorithm() {
        // node -e 'c=require("crypto");console.log(c.createHash("sha256").update(Buffer.from("hello").toString("base64")+"gz").digest("hex").slice(0,32))'
        let expected: String = Sha256::digest(b"aGVsbG8=gz").iter().map(|b| format!("{b:02x}")).collect::<String>()[..32].to_owned();
        assert_eq!(asset_hash(b"hello", "/stable/customer/windows-x86_64/TetherQS-1.exe.gz"), expected);
        assert_eq!(asset_hash(b"hello", "/a/x").len(), 32);
        assert_eq!(content_type("/s/c/w/TetherQS-1.exe.gz"), "application/gzip");
        assert_eq!(content_type("/s/t/w/Tether_1_x64-setup.exe"), "application/vnd.microsoft.portable-executable");
    }

    #[test]
    fn files_over_25_mib_are_refused() {
        assert!(check_size("/a", 25 << 20).is_ok());
        let e = check_size("/stable/customer/windows-x86_64/TetherQS-0.5.24.exe", 34_900_000).unwrap_err();
        assert!(e.contains("25 MiB") && e.contains("TetherQS-0.5.24.exe"), "{e}");
        let big = vec![0u8; (25 << 20) + 1];
        assert!(next_assets(&Assets::new(), "stable/customer/windows-x86_64", "1", &[("/x.exe".into(), &big[..])]).is_err());
    }

    #[test]
    fn keeps_the_current_and_previous_version_per_group() {
        let g = "stable/technician/windows-x86_64";
        let mut a = Assets::new();
        for v in ["0.5.22", "0.5.23", "0.5.24"] {
            let path = format!("/{g}/Tether_{v}_x64-setup.exe");
            a = next_assets(&a, g, v, &[(path, v.as_bytes())]).unwrap();
        }
        a = next_assets(&a, "stable/customer/windows-x86_64", "0.5.20", &[("/q.exe.gz".into(), b"q")]).unwrap();
        let mut versions: Vec<_> = a.values().map(|x| x.version.as_str()).collect();
        versions.sort();
        assert_eq!(versions, ["0.5.20", "0.5.23", "0.5.24"]);
        assert!(a.keys().all(|p| !p.contains("0.5.22")), "{a:?}");
        assert_eq!(a.len(), 3);
    }

    #[test]
    fn env_file_format() {
        let (t, a) = parse_env("# cf\nCLOUDFLARE_API_TOKEN=\"tok\"\nexport CLOUDFLARE_ACCOUNT_ID=acc\nR2_ACCESS_KEY_ID=x\n").unwrap();
        assert_eq!((t.as_str(), a.as_str()), ("tok", "acc"));
        assert!(parse_env("CLOUDFLARE_API_TOKEN=x").unwrap_err().contains("CLOUDFLARE_ACCOUNT_ID"));
    }

    /// A stand-in for api.cloudflare.com that answers the first `answered` calls like
    /// [`deploy_flow_against_a_fake_api`] and then takes the next request without ever answering.
    fn hanging_api(answered: usize, bucket: String) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let api = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for i in 0.. {
                let Ok((s, _)) = listener.accept() else { return };
                let mut r = BufReader::new(s.try_clone().unwrap());
                let (mut line, mut len) = (String::new(), 0usize);
                r.read_line(&mut line).unwrap();
                loop {
                    let mut h = String::new();
                    r.read_line(&mut h).unwrap();
                    if h.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; len];
                r.read_exact(&mut body).unwrap();
                if i >= answered {
                    std::thread::sleep(std::time::Duration::from_secs(30));
                    return;
                }
                let text = json!({"success": true, "result": {"jwt": "upload-jwt", "buckets": [[bucket.clone()]]}}).to_string();
                write!(&s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len()).unwrap();
            }
        });
        api
    }

    /// Cloudflare that stops answering (the first call, or the bucket upload after its last byte):
    /// the deploy fails after api::ANSWER, not after the client's 30 min.
    #[test]
    fn deploy_against_a_silent_api_fails_quickly() {
        let _one = api::tests::SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let file = b"new release".to_vec();
        let hash = asset_hash(&file, "/stable/c/w/n.exe.gz");
        let mut assets = Assets::new();
        assets.insert("/stable/c/w/n.exe.gz".into(), Asset { hash: hash.clone(), size: file.len() as u64, group: "stable/c/w".into(), version: "2".into() });
        let creds = Creds { account_id: "acc".into(), token: "tok".into(), worker: "tether-dl".into() };
        for (answered, expect) in [(0, "no answer within"), (1, "after the upload")] {
            let base = hanging_api(answered, hash.clone());
            let f = file.clone();
            let t = std::time::Instant::now();
            let r = tauri::async_runtime::block_on(deploy(&reqwest::Client::new(), &base, &creds, &assets, move |_| {
                let f = f.clone();
                async move { Ok(f) }
            }, std::sync::Arc::new(|_: &str, _, _| {})));
            let e = r.unwrap_err();
            assert!(e.contains(expect), "{e}");
            assert!(t.elapsed() < api::ANSWER + std::time::Duration::from_secs(2), "{:?}", t.elapsed());
        }
    }

    /// The three API calls against a stand-in for api.cloudflare.com: manifest, one bucket with the
    /// file Cloudflare lacks (base64, field named by hash), then the script with the completion token.
    #[test]
    fn deploy_flow_against_a_fake_api() {
        let _one = api::tests::SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let api = format!("http://{}", listener.local_addr().unwrap());
        let new_file = b"new release".to_vec();
        let new_hash = asset_hash(&new_file, "/stable/c/w/n.exe.gz");
        let h2 = new_hash.clone();
        let server = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for _ in 0..3 {
                let (s, _) = listener.accept().unwrap();
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                let (mut len, mut auth) = (0usize, String::new());
                loop {
                    let mut h = String::new();
                    r.read_line(&mut h).unwrap();
                    if h.trim().is_empty() {
                        break;
                    }
                    let (k, v) = h.split_once(':').unwrap();
                    match k.to_ascii_lowercase().as_str() {
                        "content-length" => len = v.trim().parse().unwrap(),
                        "authorization" => auth = v.trim().to_owned(),
                        _ => {}
                    }
                }
                let mut body = vec![0; len];
                r.read_exact(&mut body).unwrap();
                let answer = if line.contains("assets-upload-session") {
                    json!({"success": true, "result": {"jwt": "upload-jwt", "buckets": [[h2]]}})
                } else if line.contains("assets/upload?base64=true") {
                    json!({"success": true, "result": {"jwt": "completion-jwt"}})
                } else {
                    json!({"success": true, "result": {}})
                };
                let text = answer.to_string();
                write!(&s, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len()).unwrap();
                seen.push((line.trim().to_owned(), auth, String::from_utf8_lossy(&body).into_owned()));
            }
            seen
        });
        let mut assets = Assets::new();
        assets.insert("/stable/c/w/old.exe.gz".into(), Asset { hash: "0".repeat(32), size: 3, group: "stable/c/w".into(), version: "1".into() });
        assets.insert("/stable/c/w/n.exe.gz".into(), Asset { hash: new_hash.clone(), size: new_file.len() as u64, group: "stable/c/w".into(), version: "2".into() });
        let creds = Creds { account_id: "acc".into(), token: "tok".into(), worker: "tether-dl".into() };
        let client = reqwest::Client::new();
        let nf = new_file.clone();
        let steps = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(String, u64, u64)>::new()));
        let rec = steps.clone();
        let on: OnProgress = std::sync::Arc::new(move |s: &str, a, b| rec.lock().unwrap().push((s.to_owned(), a, b)));
        tauri::async_runtime::block_on(deploy(&client, &api, &creds, &assets, move |p| {
            let nf = nf.clone();
            async move { if p.ends_with("n.exe.gz") { Ok(nf) } else { Err(format!("not asked for {p}")) } }
        }, on))
        .unwrap();
        // Progress: prepare, read, upload with bytes up to the body length, activate.
        let steps = steps.lock().unwrap().clone();
        assert_eq!(steps.first().unwrap().0, "preparing the upload");
        assert!(steps.iter().any(|s| s.0 == "reading n.exe.gz"));
        let up: Vec<_> = steps.iter().filter(|s| s.0 == "uploading n.exe.gz").collect();
        assert!(up.len() >= 2 && up[0].1 == 0 && up.last().unwrap().1 == up.last().unwrap().2 && up.last().unwrap().2 > 0, "{up:?}");
        assert_eq!(steps.last().unwrap().0, "activating the new version");
        let seen = server.join().unwrap();
        assert!(seen[0].0.starts_with("POST /accounts/acc/workers/scripts/tether-dl/assets-upload-session"));
        assert_eq!(seen[0].1, "Bearer tok");
        let m: Value = serde_json::from_str(&seen[0].2).unwrap();
        assert_eq!(m["manifest"]["/stable/c/w/n.exe.gz"]["hash"], new_hash.as_str());
        assert!(seen[1].0.starts_with("POST /accounts/acc/workers/assets/upload?base64=true"));
        assert_eq!(seen[1].1, "Bearer upload-jwt");
        assert!(seen[1].2.contains(&format!("name=\"{new_hash}\"")) && seen[1].2.contains(&B64.encode(&new_file)) && seen[1].2.contains("application/gzip"));
        assert!(seen[2].0.starts_with("PUT /accounts/acc/workers/scripts/tether-dl"));
        assert_eq!(seen[2].1, "Bearer tok");
        assert!(seen[2].2.contains("\"jwt\":\"completion-jwt\"") && seen[2].2.contains("not_found_handling") && seen[2].2.contains("export default"));
    }
}

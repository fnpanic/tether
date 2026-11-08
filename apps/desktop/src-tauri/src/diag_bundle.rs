//! The diagnostics zip and its upload, shared by the technician app and TetherQS (TetherQS
//! includes this file with `#[path]`): log collection, config redaction, the zip, the ticket code
//! and the upload to every relay. Only `hbb_common`, `serde_json` and `zip`.

use std::{
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::Value;

const DAY: Duration = Duration::from_secs(24 * 3600);
/// Per log file, newest bytes kept in an export, and the total.
const LOG_TAIL: u64 = 1024 * 1024;
const LOG_TOTAL: u64 = 12 * 1024 * 1024;
/// `POST /api/diag/<ticket>` limit (docs/api.md).
pub const UPLOAD_MAX: usize = 2 * 1024 * 1024;

/// Log files under `root` (`<root>/*.log`, `<root>/<role>/*.log`), changed in the last 24 h, newest first.
pub fn recent_logs(root: &Path) -> Vec<(PathBuf, SystemTime)> {
    let mut out = vec![];
    let mut dirs = vec![root.to_path_buf()];
    for depth in 0..2 {
        let mut next = vec![];
        for d in &dirs {
            for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
                let Ok(m) = e.metadata() else { continue };
                if m.is_dir() && depth == 0 {
                    next.push(e.path());
                } else if m.is_file() && e.path().extension().is_some_and(|x| x == "log") {
                    let t = m.modified().unwrap_or(UNIX_EPOCH);
                    if t.elapsed().map_or(true, |a| a < DAY) {
                        out.push((e.path(), t));
                    }
                }
            }
        }
        dirs = next;
    }
    out.sort_by(|a, b| b.1.cmp(&a.1));
    out
}

pub fn tail(path: &Path, max: u64) -> Vec<u8> {
    let mut buf = vec![];
    if let Ok(mut f) = std::fs::File::open(path) {
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        let _ = f.seek(SeekFrom::Start(len.saturating_sub(max)));
        let _ = f.read_to_end(&mut buf);
    }
    buf
}

/// The last `n` ERROR/WARN lines of the newest log files under `root`, newest first.
pub fn last_errors(root: &Path, n: usize) -> Vec<String> {
    let mut out = vec![];
    for (p, _) in recent_logs(root).iter().take(6) {
        let text = String::from_utf8_lossy(&tail(p, 256 * 1024)).into_owned();
        out.extend(text.lines().filter(|l| l.contains("] ERROR ") || l.contains("] WARN ")).map(|l| l.chars().take(400).collect()));
    }
    // Lines from several files: newest first by their leading timestamp.
    out.sort_by(|a: &String, b| b.cmp(a));
    out.truncate(n);
    out
}

fn sensitive(key: &str) -> bool {
    const WORDS: &[&str] = &["password", "key", "salt", "token", "secret", "pin", "otp", "2fa", "tfa", "trusted", "pk", "sk"];
    let k = key.to_ascii_lowercase();
    k.contains("password") || k.split(['-', '_', '.']).any(|w| WORDS.contains(&w))
}

fn redact(v: &mut hbb_common::toml::Value, sens: bool) {
    use hbb_common::toml::Value as V;
    match v {
        V::Table(t) => t.iter_mut().for_each(|(k, v)| redact(v, sens || sensitive(k))),
        V::Boolean(_) => {}
        V::String(s) if s.is_empty() => {}
        _ if sens => *v = V::String("<redacted>".into()),
        V::Array(a) => a.iter_mut().for_each(|v| redact(v, false)),
        _ => {}
    }
}

/// Config TOML with passwords, keys, salts and tokens replaced. Unparseable files are omitted.
pub fn redact_config(text: &str) -> String {
    match text.parse::<hbb_common::toml::Value>() {
        Ok(mut v) => {
            redact(&mut v, false);
            hbb_common::toml::to_string(&v).unwrap_or_default()
        }
        Err(_) => "# not readable, omitted\n".into(),
    }
}

pub fn build_zip(info: &Value, logs: &[(String, Vec<u8>)], configs: &[(String, String)]) -> Result<Vec<u8>, String> {
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opt = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut add = |name: &str, data: &[u8]| -> Result<(), String> {
        z.start_file(name, opt).map_err(|e| e.to_string())?;
        z.write_all(data).map_err(|e| e.to_string())
    };
    add("diagnostics.json", serde_json::to_string_pretty(info).unwrap_or_default().as_bytes())?;
    for (name, data) in logs {
        add(&format!("logs/{name}"), data)?;
    }
    for (name, text) in configs {
        add(&format!("config/{name}"), redact_config(text).as_bytes())?;
    }
    z.finish().map(|c| c.into_inner()).map_err(|e| e.to_string())
}

/// Diagnostics zip: `diagnostics.json`, last 24 h of the logs under `log_root`, the `*.toml`
/// files of `cfg_dir` redacted (no other files: e.g. TetherQS's handoff.json holds a password).
pub fn bundle(info: &Value, log_root: &Path, cfg_dir: &Path) -> Result<Vec<u8>, String> {
    let mut total = 0;
    let mut logs = vec![];
    for (p, _) in recent_logs(log_root) {
        if total >= LOG_TOTAL {
            break;
        }
        let data = tail(&p, LOG_TAIL.min(LOG_TOTAL - total));
        total += data.len() as u64;
        let name = p.strip_prefix(log_root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
        logs.push((name, data));
    }
    let configs = std::fs::read_dir(cfg_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
        .filter_map(|e| Some((e.file_name().to_string_lossy().into_owned(), std::fs::read_to_string(e.path()).ok()?)))
        .collect::<Vec<_>>();
    build_zip(info, &logs, &configs)
}

/// The bundle for an upload: above the server's limit only `diagnostics.json`.
pub fn for_upload(info: &Value, log_root: &Path, cfg_dir: &Path) -> Result<Vec<u8>, String> {
    let data = bundle(info, log_root, cfg_dir)?;
    if data.len() <= UPLOAD_MAX {
        return Ok(data);
    }
    hbb_common::log::info!("diagnostics {} bytes, sending without logs", data.len());
    build_zip(info, &[], &[])
}

/// Support ticket code: 8 of `[A-HJ-NP-Z2-9]`.
pub fn valid_ticket(t: &str) -> bool {
    t.len() == 8 && t.chars().all(|c| matches!(c, 'A'..='H' | 'J'..='N' | 'P'..='Z' | '2'..='9'))
}

/// The code as typed (spaces, lower case) → the ticket, or the message for the user.
pub fn ticket(input: &str) -> Result<String, String> {
    let t: String = input.chars().filter(|c| !c.is_whitespace() && *c != '-').collect::<String>().to_ascii_uppercase();
    if valid_ticket(&t) {
        Ok(t)
    } else {
        Err("Enter the 8-character code from your support contact.".into())
    }
}

pub const NOT_VALID: &str = "This code is not valid, has expired or was already used. Ask your support contact for a new one.";

/// Upload to each relay in turn until one takes it. A relay answers 404 when the ticket is not
/// open there (Tether Manage opens it on every relay it knows; one may have been down, or the
/// admin's Manage knows fewer relays than this app), so a 404 tries the next relay.
/// `send(relay)`: the HTTP status, or Err when that relay could not be reached.
pub fn upload_each(relays: &[String], mut send: impl FnMut(&str) -> Result<u16, String>) -> Result<(), String> {
    let (mut refused, mut not_found, mut offline) = (None, false, None);
    for r in relays {
        match send(r) {
            Ok(200..=299) => return Ok(()),
            Ok(404) => not_found = true,
            Ok(s) => refused = Some(s),
            Err(e) => offline = Some(e),
        }
    }
    // A relay that has the ticket but refused (busy, too large) says more than a 404 elsewhere.
    match (refused, not_found, offline) {
        (Some(503), ..) => Err("The server is busy. Try again in a minute.".into()),
        (Some(413), ..) => Err("The diagnostics are larger than the server accepts.".into()),
        (Some(s), ..) => Err(format!("The server refused the upload ({s}).")),
        (None, true, _) => Err(NOT_VALID.into()),
        (None, false, Some(e)) => Err(format!("Cannot reach the server: {e}")),
        (None, false, None) => Err("Cannot reach the server: no server in this build.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
id = "123456789"
password = "hunter2-Secret"
salt = "s4ltS4lt"
key_pair = [[11, 22, 33], [44, 55, 66]]
key_confirmed = true

[options]
approve-mode = "both"
enable-keyboard = "Y"
key = "SERVERPUBKEYxyz="
unlock_pin = "9911"
2fa = "TOTPSECRETBASE32"
recording-upload-token = "tok-abc-123"

[socks]
proxy = "http://proxy:3128"
username = "bob"
password = "proxy-pass-77"
"#;

    #[test]
    fn export_contains_no_secrets() {
        let zip = build_zip(&serde_json::json!({}), &[], &[("Tether.toml".into(), SAMPLE.into())]).unwrap();
        let mut a = zip::ZipArchive::new(std::io::Cursor::new(zip)).unwrap();
        let mut text = String::new();
        a.by_name("config/Tether.toml").unwrap().read_to_string(&mut text).unwrap();
        for secret in ["hunter2-Secret", "s4ltS4lt", "11, 22", "SERVERPUBKEYxyz", "9911", "TOTPSECRET", "tok-abc-123", "proxy-pass-77"] {
            assert!(!text.contains(secret), "{secret} leaked:\n{text}");
        }
        // Useful, non-secret values stay.
        for kept in ["123456789", "enable-keyboard", "both", "proxy:3128"] {
            assert!(text.contains(kept), "{kept} missing:\n{text}");
        }
        assert!(a.by_name("diagnostics.json").is_ok());
    }

    #[test]
    fn bundle_takes_logs_and_toml_only() {
        let d = std::env::temp_dir().join(format!("diag-bundle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("log/server")).unwrap();
        std::fs::write(d.join("log/server/a.log"), "[x] ERROR boom\n").unwrap();
        std::fs::write(d.join("log/b.log"), "plain\n").unwrap();
        std::fs::write(d.join("App.toml"), SAMPLE).unwrap();
        std::fs::write(d.join("handoff.json"), r#"{"password":"hand-off-secret"}"#).unwrap();
        let zip = bundle(&serde_json::json!({"v": 1}), &d.join("log"), &d).unwrap();
        let mut a = zip::ZipArchive::new(std::io::Cursor::new(zip)).unwrap();
        let mut names: Vec<String> = (0..a.len()).map(|i| a.by_index(i).unwrap().name().unwrap().into_owned()).collect();
        names.sort();
        assert_eq!(names, ["config/App.toml", "diagnostics.json", "logs/b.log", "logs/server/a.log"]);
        let mut all = String::new();
        for i in 0..a.len() {
            a.by_index(i).unwrap().read_to_string(&mut all).unwrap();
        }
        assert!(!all.contains("hunter2-Secret") && !all.contains("hand-off-secret"));
        assert_eq!(last_errors(&d.join("log"), 5), ["[x] ERROR boom"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn tickets() {
        assert!(valid_ticket("K7Q2M9XD"));
        assert!(!valid_ticket("K7Q2M9X"));
        assert!(!valid_ticket("K7Q2M9XI")); // I is ambiguous
        assert!(!valid_ticket("k7q2m9xd"));
        assert!(!valid_ticket("../../x1"));
        assert_eq!(ticket(" k7q2 m9xd ").unwrap(), "K7Q2M9XD");
        assert_eq!(ticket("K7Q2-M9XD").unwrap(), "K7Q2M9XD");
        assert!(ticket("K7Q2M9X").is_err());
    }

    #[test]
    fn upload_tries_every_relay() {
        let relays = ["a".to_owned(), "b".to_owned()];
        let run = |answers: [Result<u16, &str>; 2]| {
            let mut sent = vec![];
            let r = upload_each(&relays, |h| {
                sent.push(h.to_owned());
                answers[sent.len() - 1].map_err(str::to_owned)
            });
            (r, sent.len())
        };
        // The ticket only on the second relay (up to 0.5.26 the app stopped at the first 404).
        assert_eq!(run([Ok(404), Ok(204)]), (Ok(()), 2));
        assert_eq!(run([Ok(204), Ok(404)]), (Ok(()), 1));
        assert_eq!(run([Err("down"), Ok(204)]), (Ok(()), 2));
        assert_eq!(run([Ok(404), Ok(404)]).0, Err(NOT_VALID.into()));
        assert_eq!(run([Ok(404), Err("down")]).0, Err(NOT_VALID.into()));
        assert!(run([Ok(404), Ok(503)]).0.unwrap_err().contains("busy"));
        assert!(run([Ok(413), Ok(404)]).0.unwrap_err().contains("larger"));
        assert!(run([Err("timeout"), Err("dns")]).0.unwrap_err().starts_with("Cannot reach the server: dns"));
        assert!(upload_each(&[], |_| Ok(204)).unwrap_err().contains("no server"));
    }
}

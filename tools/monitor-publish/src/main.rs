//! Issue device-status enrolment tokens and publish the device-status agent (`monitor`) and its
//! check packs (`monitor-packs`) to `download/<channel>/monitor.json`, without the Tether Manage
//! window. The admin key comes from Tether Manage's sealed key file (its own `keys.rs`, included
//! below, so the format is the same by construction); the passphrase only from `MANAGE_PASSPHRASE`.
//!
//! ```text
//! monitor-publish token --label <text> --days <1-30> [--tether-id <id>]
//! monitor-publish tokens
//! monitor-publish revoke --id <token id> [--exp <unix>] | --token-file <path> | --token - --relay <https://host>[,…]
//! monitor-publish devices [--token <token id>] --relay <https://host>[,…]
//! monitor-publish remove --uuid <uuid> --relay <https://host>[,…]
//! monitor-publish index --channel <c> --app monitor|monitor-packs --version <v> --file <artifact>
//!     [--rollout standard|fast|now] [--min-version <v>] --relay <https://host>[,<https://host>…]
//! monitor-publish refresh --channel <c> --relay <https://host>[,<https://host>…]
//! ```
//!
//! `monitor.json` expires 30 days after it is signed: `index` signs a fresh one, `refresh` signs the
//! published one again unchanged but for `serial + 1` and a new expiry. Both keep the highest serial
//! they ever published per channel in `monitor-serials.json` next to the key file, and refuse to go
//! below it (a relay that lost or rolled back its `monitor.json` must not take the serial back).
//!
//! Key file: `MANAGE_KEYS`, else `$TETHER_MANAGE_DIR/keys.json`, else Tether Manage's data folder
//! (`%APPDATA%\Tether-Manage\keys.json` on Windows). Issued tokens are journaled (id, label,
//! expiry, bound Tether ID; never the token) in `monitor-tokens.json` next to the key file.
//! `index` needs `<artifact>.sig` (the updater
//! signature, as the build writes it) and checks it against the updater key in the same key file.
//! A version below the published one is refused; the same version only with the same bytes, which
//! repairs the relays that lack the current index (a re-run after a partial publish).

#[allow(dead_code)] // Tether Manage's key vault; only the admin and updater keys are used here.
#[path = "../../../apps/manage/src-tauri/src/keys.rs"]
mod keys;

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tether_trust::release::{self, Entry, Index, MONITOR_APPS, MONITOR_INDEX_FILE, MONITOR_NAMES};
use tether_trust::{envelope, reqauth, SigningKey, VerifyingKey};
use zeroize::Zeroizing;

type R<T> = Result<T, String>;

/// The admin keys the apps are built with, as in Tether Manage (`TETHER_ADMIN_PUB_KEY` at build
/// time): after an admin key rotation the relays still serve indexes signed with the old key.
const TRUSTED_ADMIN_KEYS: Option<&str> = option_env!("TETHER_ADMIN_PUB_KEY");

const USAGE: &str = "usage:
  monitor-publish token --label <text> --days <1-30> [--tether-id <id>]
  monitor-publish tokens
  monitor-publish revoke (--id <token id> [--exp <unix>] | --token-file <path> | --token -) --relay <https://host>[,<https://host>...]
  monitor-publish devices [--token <token id>] --relay <https://host>[,<https://host>...]
  monitor-publish remove --uuid <uuid> --relay <https://host>[,<https://host>...]
  monitor-publish index --channel <c> --app monitor|monitor-packs --version <v> --file <artifact>
      [--rollout standard|fast|now] [--min-version <v>] --relay <https://host>[,<https://host>...]
  monitor-publish refresh --channel <c> --relay <https://host>[,<https://host>...]
env: MANAGE_PASSPHRASE (required), MANAGE_KEYS (key file; default Tether Manage's keys.json)";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = run(&args) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// `--name value` from `args`; every flag at most once, no unknown ones.
fn flags(args: &[String], known: &[&str]) -> R<std::collections::BTreeMap<String, String>> {
    let mut out = std::collections::BTreeMap::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let name = a.strip_prefix("--").filter(|n| known.contains(n)).ok_or_else(|| format!("unknown argument {a:?}\n{USAGE}"))?;
        let v = it.next().ok_or_else(|| format!("--{name} needs a value"))?;
        if out.insert(name.to_owned(), v.clone()).is_some() {
            return Err(format!("--{name} given twice"));
        }
    }
    Ok(out)
}

fn run(args: &[String]) -> R<()> {
    let (cmd, rest) = args.split_first().ok_or(USAGE)?;
    let need = |f: &std::collections::BTreeMap<String, String>, n: &str| f.get(n).cloned().ok_or_else(|| format!("--{n} is required\n{USAGE}"));
    match cmd.as_str() {
        "token" => {
            let f = flags(rest, &["label", "days", "tether-id"])?;
            let days = need(&f, "days")?.parse().map_err(|_| "--days must be a number")?;
            let label = need(&f, "label")?;
            let bound = f.get("tether-id").map(String::as_str);
            if bound.is_some_and(|t| !valid_bound_id(t)) {
                return Err("--tether-id must be a Tether ID (1-32 printable characters, no spaces, not starting with m:)".into());
            }
            let keys = open_keys(&key_file(), passphrase())?;
            let (tok, e) = issue(&keys.admin, &journal_file(), &label, days, bound, now())?;
            println!("{tok}");
            let only = e.tether_id.as_deref().map_or(String::new(), |t| format!(", for Tether ID {t} only (one device)"));
            eprintln!("enrolment token {} {label:?}, valid until {}{only}", e.id, utc(e.exp));
            Ok(())
        }
        "tokens" => {
            flags(rest, &[])?;
            print!("{}", tokens_table(&journal_read(&journal_file())?, now()));
            Ok(())
        }
        "revoke" => {
            let f = flags(rest, &["id", "exp", "relay", "token-file", "token"])?;
            let source = match (f.get("id"), f.get("token-file"), f.get("token").map(String::as_str)) {
                (Some(id), None, None) => {
                    if !tether_trust::status::valid_token_id(id) {
                        return Err(format!("bad token id {id:?}"));
                    }
                    None
                }
                (None, Some(p), None) => Some(std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?),
                (None, None, Some("-")) => Some(String::new()), // read below, after the checks
                (None, None, Some(_)) => return Err("--token takes only - (the token on standard input); use --token-file <path> for a file".into()),
                _ => return Err(format!("give one of --id, --token-file or --token -\n{USAGE}")),
            };
            if source.is_some() && f.contains_key("exp") {
                return Err("--exp comes from the token itself; leave it out".into());
            }
            let flag_exp = f.get("exp").map(|e| e.parse::<u64>().map_err(|_| "--exp must be unix seconds")).transpose()?;
            let relays = relays(&f)?;
            let journal = journal_file();
            let list = journal_read(&journal)?;
            let keys = open_keys(&key_file(), passphrase())?;
            let (id, exp) = match source {
                Some(mut text) => {
                    if text.is_empty() {
                        std::io::Read::read_to_string(&mut std::io::stdin(), &mut text).map_err(|e| format!("standard input: {e}"))?;
                    }
                    token_target(&text, &trusted_keys(keys.admin.verifying_key(), TRUSTED_ADMIN_KEYS.unwrap_or("")), now())?
                }
                None => {
                    let id = f["id"].clone();
                    let exp = revoke_exp(&list, &id, flag_exp).map_err(|e| format!("{e} ({})", journal.display()))?;
                    (id, exp)
                }
            };
            if exp <= now() {
                return Err(format!("token {id} expired at {}: relays refuse it anyway, there is nothing to revoke", utc(exp)));
            }
            let body = serde_json::json!({ "id": id, "exp": exp }).to_string();
            let failed = each_relay(&keys.admin, &relays, "POST", "/api/admin/status/revoke", body.as_bytes(), |code| (code == 204).then(|| "revoked".to_owned()))?;
            println!("revoked on {} of {} relays", relays.len() - failed.len(), relays.len());
            if failed.len() < relays.len() {
                journal_update(&journal, |l| mark_revoked(l, &id, &failed, now()))?;
            }
            eprintln!("Devices already enrolled with this token stay enrolled. List them with `monitor-publish devices --token {id} --relay …` and remove them with `monitor-publish remove --uuid <uuid> --relay …`.");
            fail_count(failed.len(), relays.len())
        }
        "devices" => {
            let f = flags(rest, &["token", "relay"])?;
            let filter = f.get("token").cloned();
            if filter.as_deref().is_some_and(|t| !tether_trust::status::valid_token_id(t)) {
                return Err("bad --token id".into());
            }
            let relays = relays(&f)?;
            let keys = open_keys(&key_file(), passphrase())?;
            let c = client()?;
            let mut failed = 0;
            for r in &relays {
                match list_devices(&c, &keys.admin, r, filter.as_deref()) {
                    Ok(list) => print!("{r}: {} devices\n{}", list.len(), devices_table(&list)),
                    Err(e) => {
                        failed += 1;
                        println!("{r}: FAILED: {e}");
                    }
                }
            }
            fail_count(failed, relays.len())
        }
        "remove" => {
            let f = flags(rest, &["uuid", "relay"])?;
            let uuid = need(&f, "uuid")?;
            if !tether_trust::status::valid_uuid(&uuid) {
                return Err(format!("bad uuid {uuid:?}"));
            }
            let relays = relays(&f)?;
            let keys = open_keys(&key_file(), passphrase())?;
            let failed = each_relay(&keys.admin, &relays, "DELETE", &format!("/api/admin/status/{uuid}"), b"", |code| match code {
                204 => Some("removed".into()),
                404 => Some("not enrolled there".into()),
                _ => None,
            })?;
            fail_count(failed.len(), relays.len())
        }
        "index" => {
            let f = flags(rest, &["channel", "app", "version", "file", "rollout", "min-version", "relay"])?;
            let (channel, app, version) = (need(&f, "channel")?, need(&f, "app")?, need(&f, "version")?);
            if !release::valid_channel(&channel) {
                return Err(format!("bad channel {channel:?}"));
            }
            target(&app, &version)?;
            let rollout = f.get("rollout").map_or("standard", String::as_str);
            release::preset(rollout).ok_or("--rollout must be standard, fast or now")?;
            let min_version = f.get("min-version").map_or("", String::as_str);
            if !min_version.is_empty() && !release::valid_version(min_version) {
                return Err(format!("bad --min-version {min_version:?}"));
            }
            let relays = relays(&f)?;
            let keys = open_keys(&key_file(), passphrase())?;
            let a = artifact(Path::new(&need(&f, "file")?), &app, &version, &keys.updater_pub())?;
            publish(&keys.admin, &relays, &channel, &a, rollout, min_version, &serials_file())
        }
        "refresh" => {
            let f = flags(rest, &["channel", "relay"])?;
            let channel = need(&f, "channel")?;
            if !release::valid_channel(&channel) {
                return Err(format!("bad channel {channel:?}"));
            }
            let relays = relays(&f)?;
            let keys = open_keys(&key_file(), passphrase())?;
            refresh(&keys.admin, &relays, &channel, &serials_file())
        }
        _ => Err(USAGE.into()),
    }
}

/// `MANAGE_KEYS`, else `TETHER_MANAGE_DIR/keys.json` (as Tether Manage), else its data folder.
fn key_file() -> PathBuf {
    if let Some(p) = std::env::var_os("MANAGE_KEYS") {
        return p.into();
    }
    if let Some(d) = std::env::var_os("TETHER_MANAGE_DIR") {
        return PathBuf::from(d).join("keys.json");
    }
    // Tauri's data_dir: Roaming AppData, ~/Library/Application Support, XDG data home.
    let data = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    };
    data.unwrap_or_default().join("Tether-Manage").join("keys.json")
}

fn passphrase() -> Option<Zeroizing<String>> {
    std::env::var("MANAGE_PASSPHRASE").ok().map(Zeroizing::new)
}

/// Tether Manage's sealed key file, opened with `pass` (from `MANAGE_PASSPHRASE`, never an
/// argument). The decrypted keys are wiped when dropped (ed25519-dalek's `SigningKey` zeroizes
/// itself; keys.rs wipes its plaintext buffers).
fn open_keys(path: &Path, pass: Option<Zeroizing<String>>) -> R<keys::Keys> {
    let pass = pass.filter(|p| !p.is_empty()).ok_or("set MANAGE_PASSPHRASE to the Tether Manage passphrase")?;
    let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    keys::Keys::open(&data, &pass)
}

/// `--relay a,b`: checked relay addresses.
fn relays(f: &std::collections::BTreeMap<String, String>) -> R<Vec<String>> {
    let list: Vec<String> = f.get("relay").ok_or_else(|| format!("--relay is required\n{USAGE}"))?.split(',').map(|s| s.trim().trim_end_matches('/').to_owned()).filter(|s| !s.is_empty()).collect();
    if list.is_empty() {
        return Err("--relay names no relay".into());
    }
    for r in &list {
        relay_ok(r)?;
    }
    Ok(list)
}

/// A Tether ID a token may be issued for (`m:` names the app's agent-only rows).
fn valid_bound_id(t: &str) -> bool {
    tether_trust::status::valid_tether_id(t) && !t.starts_with("m:")
}

/// An enrolment token valid for `days` (1-30) from `now`, optionally for one Tether ID.
fn token(admin: &SigningKey, label: &str, days: u64, tether_id: Option<&str>, now: u64) -> R<(String, Issued)> {
    if !(1..=30).contains(&days) {
        return Err("--days must be 1 to 30".into());
    }
    let exp = now + days * 86_400;
    let (tok, id) = tether_trust::status::make_token(label, exp, tether_id, admin)?;
    Ok((tok, Issued { id, label: label.into(), exp, tether_id: tether_id.map(Into::into), issued: now, revoked: None, partly_revoked: None }))
}

/// A token as the journal keeps it: never the token itself (it would enrol devices).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
struct Issued {
    id: String,
    label: String,
    exp: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tether_id: Option<String>,
    issued: u64,
    /// When this tool revoked it on every relay it was asked to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    revoked: Option<u64>,
    /// Revoked on some relays only: those that failed (run `revoke` again for them).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    partly_revoked: Option<Vec<String>>,
}

/// The journal after a revocation that `failed` on those relays (and worked on at least one).
fn mark_revoked(list: &mut [Issued], id: &str, failed: &[String], t: u64) {
    for e in list.iter_mut().filter(|e| e.id == id) {
        if failed.is_empty() {
            (e.revoked, e.partly_revoked) = (Some(t), None);
        } else if e.revoked.is_none() {
            e.partly_revoked = Some(failed.to_vec());
        }
    }
}

/// The id and expiry of a token, from the token itself (checked against the admin keys).
fn token_target(text: &str, admin: &[VerifyingKey], t: u64) -> R<(String, u64)> {
    match tether_trust::status::verify_token(text.trim(), admin, t) {
        Ok(p) => Ok((p.id, p.exp)),
        Err(e) if e == "expired" => Err("the token has expired: relays refuse it anyway, there is nothing to revoke".into()),
        Err(e) => Err(format!("not a token of these admin keys: {e}")),
    }
}

/// `monitor-tokens.json` next to the key file.
fn journal_file() -> PathBuf {
    key_file().with_file_name("monitor-tokens.json")
}

/// `monitor-serials.json` next to the key file: the highest `monitor.json` serial published per channel.
fn serials_file() -> PathBuf {
    key_file().with_file_name("monitor-serials.json")
}

/// The serial floors; empty when there is none. A damaged file is an error and is never overwritten.
fn floor_read(p: &Path) -> R<std::collections::BTreeMap<String, u64>> {
    match std::fs::read(p) {
        Ok(b) => serde_json::from_slice(&b).map_err(|e| format!("{}: {e}; repair it or move it away", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Default::default()),
        Err(e) => Err(format!("{}: {e}", p.display())),
    }
}

/// Refuses to build on `base` (the highest serial the relays serve, 0 for none) when this machine
/// already published a higher one in `channel`: a relay lost or rolled back its index.
fn floor_check(p: &Path, channel: &str, base: u64) -> R<()> {
    match floor_read(p)?.get(channel) {
        Some(&f) if base < f => Err(format!(
            "the relays serve {channel}/{MONITOR_INDEX_FILE} serial {base}, but serial {f} was published from here ({}): a relay lost or rolled back its index. Restore it from a backup; nothing was sent",
            p.display()
        )),
        _ => Ok(()),
    }
}

/// Records that `serial` was published in `channel` (it only goes up).
fn floor_raise(p: &Path, channel: &str, serial: u64) -> R<()> {
    let mut m = floor_read(p)?;
    if m.get(channel).is_some_and(|&f| f >= serial) {
        return Ok(());
    }
    m.insert(channel.to_owned(), serial);
    let tmp = p.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(&m).expect("serializable")).and_then(|_| std::fs::rename(&tmp, p)).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", p.display())
    })
}

/// The journal; empty when there is none. A damaged one is an error and is never overwritten.
fn journal_read(p: &Path) -> R<Vec<Issued>> {
    match std::fs::read(p) {
        Ok(b) => serde_json::from_slice(&b).map_err(|e| format!("{}: {e}; repair it or move it away", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("{}: {e}", p.display())),
    }
}

/// Changes the journal: written to a temp file, then renamed into place.
fn journal_update(p: &Path, f: impl FnOnce(&mut Vec<Issued>)) -> R<()> {
    let mut list = journal_read(p)?;
    f(&mut list);
    let tmp = p.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(&list).expect("serializable")).and_then(|_| std::fs::rename(&tmp, p)).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", p.display())
    })
}

/// A token, journaled before it is handed out: there is no token the journal does not know.
fn issue(admin: &SigningKey, journal: &Path, label: &str, days: u64, tether_id: Option<&str>, now: u64) -> R<(String, Issued)> {
    let (tok, e) = token(admin, label, days, tether_id, now)?;
    journal_update(journal, |l| l.push(e.clone()))?;
    Ok((tok, e))
}

/// The expiry to revoke `id` with: `--exp`, else the journal's.
/// A revocation is kept until `exp`: an `--exp` below the token's real expiry would end it early.
fn revoke_exp(journal: &[Issued], id: &str, flag: Option<u64>) -> R<u64> {
    let known = journal.iter().find(|e| e.id == id).map(|e| e.exp);
    match (flag, known) {
        (Some(f), Some(k)) if f < k => Err(format!("--exp {f} is earlier than the token's expiry in the journal ({k}, {}); leave --exp out", utc(k))),
        (Some(f), _) => Ok(f),
        (None, Some(k)) => Ok(k),
        (None, None) => Err(format!("token {id} is not in the journal; give the token with --token-file <path> (or --token -), or its expiry with --exp <unix seconds>")),
    }
}

fn tokens_table(list: &[Issued], now: u64) -> String {
    let row = |id: &str, state: &str, tid: &str, exp: &str, label: &str| format!("{id:<32}  {state:<14}  {tid:<16}  {exp:<20}  {label}\n");
    let mut out = row("ID", "STATE", "TETHER ID", "EXPIRES", "LABEL");
    for e in list {
        let state = if e.revoked.is_some() {
            "revoked"
        } else if e.partly_revoked.is_some() {
            "partly revoked"
        } else if e.exp <= now {
            "expired"
        } else {
            "valid"
        };
        out += &row(&e.id, state, e.tether_id.as_deref().unwrap_or("-"), &utc(e.exp), &e.label);
    }
    out
}

/// Unix seconds as `YYYY-MM-DD HH:MM UTC` (days to civil date after H. Hinnant).
fn utc(t: u64) -> String {
    let (days, secs) = ((t / 86_400) as i64, t % 86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02} UTC", secs / 3600, secs % 3600 / 60)
}

/// An admin-signed request to one relay: status and body.
fn admin_request(c: &reqwest::blocking::Client, admin: &SigningKey, relay: &str, method: &str, path: &str, body: &[u8]) -> R<(u16, Vec<u8>)> {
    let sig = reqauth::sign_request_for(admin, relay, method, path, body, now());
    let m = reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| e.to_string())?;
    let mut rq = c.request(m, format!("{relay}{path}")).header(reqauth::HEADER, sig);
    if !body.is_empty() {
        rq = rq.header("Content-Type", "application/json").body(body.to_vec());
    }
    let res = rq.send().map_err(|e| e.to_string())?;
    let code = res.status().as_u16();
    Ok((code, res.bytes().map_err(|e| e.to_string())?.to_vec()))
}

/// The same request to every relay, one line each; `ok(code)` names a success. Answers the relays that failed.
fn each_relay(admin: &SigningKey, relays: &[String], method: &str, path: &str, body: &[u8], ok: impl Fn(u16) -> Option<String>) -> R<Vec<String>> {
    let c = client()?;
    let mut failed = Vec::new();
    for r in relays {
        match admin_request(&c, admin, r, method, path, body) {
            Ok((code, b)) => match ok(code) {
                Some(what) => println!("{r}: {what}"),
                None => {
                    failed.push(r.clone());
                    println!("{r}: FAILED: HTTP {code}: {}", cell(Some(&String::from_utf8_lossy(&b)), 300));
                }
            },
            Err(e) => {
                failed.push(r.clone());
                println!("{r}: FAILED: {e}");
            }
        }
    }
    Ok(failed)
}

fn fail_count(failed: usize, of: usize) -> R<()> {
    if failed > 0 {
        return Err(format!("{failed} of {of} relays failed"));
    }
    Ok(())
}

/// One pin as `GET /api/admin/status/devices` lists it.
#[derive(serde::Deserialize, Debug)]
struct Device {
    uuid: String,
    tether_id: Option<String>,
    claimed_id: Option<String>,
    host: Option<String>,
    enrolled: u64,
    last_seen: Option<u64>,
}

/// Every pin of `relay` (with `token`: those enrolled with that token), page by page.
fn list_devices(c: &reqwest::blocking::Client, admin: &SigningKey, relay: &str, token: Option<&str>) -> R<Vec<Device>> {
    #[derive(serde::Deserialize)]
    struct Page {
        devices: Vec<Device>,
        next: Option<String>,
    }
    let mut out = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let q: Vec<String> = [token.map(|t| format!("token={t}")), after.as_ref().map(|a| format!("after={a}"))].into_iter().flatten().collect();
        let path = if q.is_empty() { "/api/admin/status/devices".to_owned() } else { format!("/api/admin/status/devices?{}", q.join("&")) };
        let (code, b) = admin_request(c, admin, relay, "GET", &path, b"")?;
        if code != 200 {
            return Err(format!("HTTP {code}: {}", cell(Some(&String::from_utf8_lossy(&b)), 300)));
        }
        let page: Page = serde_json::from_slice(&b).map_err(|e| format!("bad answer: {e}"))?;
        out.extend(page.devices);
        match page.next {
            // The cursor must move forward, or a confused relay would keep this loop going.
            Some(n) if after.as_ref().is_none_or(|a| n > *a) && tether_trust::status::valid_uuid(&n) => after = Some(n),
            _ => return Ok(out),
        }
    }
}

/// Device-chosen texts (host name, claimed ID) and relay answers: control characters dropped, cut to `n`.
fn cell(s: Option<&str>, n: usize) -> String {
    let s: String = s.unwrap_or("-").chars().filter(|c| !c.is_control()).collect();
    if s.chars().count() > n {
        s.chars().take(n - 1).collect::<String>() + "…"
    } else {
        s
    }
}

fn devices_table(list: &[Device]) -> String {
    let row = |u: &str, t: &str, c: &str, h: &str, e: &str, s: &str| format!("  {u:<36}  {t:<16}  {c:<16}  {h:<24}  {e:<20}  {s}\n");
    let mut out = row("UUID", "TETHER ID", "CLAIMED ID", "HOST", "ENROLLED", "LAST SEEN");
    for d in list {
        let seen = d.last_seen.map_or("never".to_owned(), utc);
        out += &row(&d.uuid, &cell(d.tether_id.as_deref(), 16), &cell(d.claimed_id.as_deref(), 16), &cell(d.host.as_deref(), 24), &utc(d.enrolled), &seen);
    }
    out
}

/// `(index key, artifact file name)` of `app` at `version`.
fn target(app: &str, version: &str) -> R<(String, String)> {
    let &(a, platform, pre, suf) = MONITOR_NAMES.iter().find(|n| n.0 == app).ok_or("--app must be monitor or monitor-packs")?;
    if !release::valid_version(version) {
        return Err(format!("bad version {version:?}"));
    }
    Ok((format!("{a}/{platform}"), format!("{pre}{version}{suf}")))
}

/// `https://host[:port]` (or `http://` to this machine, for tests): requests are signed for the
/// host read from this text, so nothing may make it differ from where they go.
fn relay_ok(r: &str) -> R<()> {
    let bad = || Err(format!("{r} is not a relay address: give it like https://relay.example.org"));
    let rest = match (r.strip_prefix("https://"), r.strip_prefix("http://")) {
        (Some(rest), _) => rest,
        (_, Some(rest)) if ["127.0.0.1", "localhost", "[::1]"].contains(&reqauth::host_of(r).as_str()) => rest,
        _ => return bad(),
    };
    let rest = rest.trim_end_matches('/');
    if rest.is_empty() || rest.contains(|c: char| c.is_whitespace() || c.is_control() || matches!(c, '\\' | '@' | '?' | '#' | '/')) || reqauth::host_of(r).is_empty() {
        return bad();
    }
    Ok(())
}

struct Artifact {
    dir: String,
    name: String,
    version: String,
    data: Vec<u8>,
    /// The `.sig` file as the build wrote it (base64 of the minisign signature text).
    sig: String,
}

/// The artifact at `path` and its `.sig`, which must verify with `updater_pub` and name this file
/// and version (what devices check before they install it).
fn artifact(path: &Path, app: &str, version: &str, updater_pub: &str) -> R<Artifact> {
    let (dir, name) = target(app, version)?;
    if path.file_name().and_then(|n| n.to_str()) != Some(name.as_str()) {
        return Err(format!("{}: the file must be named {name}", path.display()));
    }
    let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let sig_path = path.with_file_name(format!("{name}.sig"));
    let sig = std::fs::read_to_string(&sig_path).map_err(|e| format!("{}: {e} (the updater signature is required)", sig_path.display()))?.trim().to_owned();
    release::verify_artifact(&data, &sig, updater_pub, &name, version).map_err(|e| format!("{}: {e}", sig_path.display()))?;
    Ok(Artifact { dir, name, version: version.into(), data, sig })
}

/// `base` (the highest index on the relays) with this artifact's entry, serial + 1, issued `now`.
/// `Ok(None)`: `base` already publishes these very bytes as this version (a re-run after a partial
/// publish), so its entry, rollout start included, stays and only relays without it are repaired.
/// A lower version, or the same version with other bytes, is refused.
fn next_index(base: Option<Index>, channel: &str, a: &Artifact, rollout: &str, min_version: &str, now: u64) -> R<Option<Index>> {
    let steps = release::preset(rollout).ok_or("--rollout must be standard, fast or now")?;
    let sha256 = release::sha256_hex(&a.data);
    if let Some(cur) = base.as_ref().and_then(|b| b.releases.get(&a.dir)) {
        match tether_trust::version::compare(&a.version, &cur.version) {
            Some(std::cmp::Ordering::Greater) => {}
            Some(std::cmp::Ordering::Equal) if cur.sha256 == sha256 => return Ok(None),
            Some(std::cmp::Ordering::Equal) => return Err(format!("{} {} is already published with other bytes (sha256 {}); publish a new version", a.dir, cur.version, cur.sha256)),
            _ => return Err(format!("{} {} is published; {} is not newer", a.dir, cur.version, a.version)),
        }
    }
    let e = Entry {
        version: a.version.clone(),
        file: a.name.clone(),
        size: a.data.len() as u64,
        sha256,
        signature: a.sig.clone(),
        urls: vec![format!("https://{{relay}}/download/{channel}/{}/{}", a.dir, a.name)],
        minimum_version: min_version.into(),
        rollout: release::Rollout { start: now, steps: steps.to_vec() },
        ..Default::default()
    };
    let serial = base.as_ref().map_or(0, |b| b.serial) + 1;
    let mut releases = base.map(|b| b.releases).unwrap_or_default();
    releases.insert(a.dir.clone(), e);
    let ix = Index { kind: release::INDEX_TYPE.into(), channel: channel.into(), serial, issued: now, expires: now + release::EXPIRY_SECS, releases };
    ix.validate_for(MONITOR_APPS)?;
    Ok(Some(ix))
}

fn sign_index(ix: &Index, admin: &SigningKey) -> Vec<u8> {
    serde_json::to_vec(&envelope::sign(&serde_json::to_vec(ix).expect("serializable"), admin)).expect("serializable")
}

fn client() -> R<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(30 * 60))
        // A signed request is never sent on to wherever a relay points.
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("monitor-publish/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())
}

/// This key file's admin key and the ones the apps are built with ([`TRUSTED_ADMIN_KEYS`]).
fn trusted_keys(own: VerifyingKey, built_in: &str) -> Vec<VerifyingKey> {
    let mut keys = vec![own];
    keys.extend(built_in.split(',').filter(|k| !k.trim().is_empty()).filter_map(|k| tether_trust::key_from_b64(k.trim()).ok()).filter(|k| *k != own));
    keys
}

/// The relay's `monitor.json` for `channel` (verified with `admin`) and its bytes; `None` when it has none.
fn fetch_index(c: &reqwest::blocking::Client, relay: &str, channel: &str, admin: &[VerifyingKey]) -> R<Option<(Index, Vec<u8>)>> {
    let res = c.get(format!("{relay}/download/{channel}/{MONITOR_INDEX_FILE}")).send().map_err(|e| format!("{relay}: {e}"))?;
    if res.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !res.status().is_success() {
        return Err(format!("{relay}: {MONITOR_INDEX_FILE}: HTTP {}", res.status()));
    }
    let raw = res.bytes().map_err(|e| format!("{relay}: {e}"))?.to_vec();
    Index::open_for(&raw, admin, channel, MONITOR_APPS)
        .map(|ix| Some((ix, raw)))
        .map_err(|e| format!("{relay} serves a {MONITOR_INDEX_FILE} for {channel} no trusted admin key verifies ({e}); publishing would drop the releases in it"))
}

/// `Err` naming the files of `named` (`<app>/<platform>/<file>`) the relay lacks in `channel`
/// (admin `GET /api/admin/releases`; the files themselves are served by Caddy).
fn missing(c: &reqwest::blocking::Client, admin: &SigningKey, relay: &str, channel: &str, named: &[String]) -> R<()> {
    if named.is_empty() {
        return Ok(());
    }
    let path = "/api/admin/releases";
    let sig = reqauth::sign_request_for(admin, relay, "GET", path, b"", now());
    let res = c.get(format!("{relay}{path}")).header(reqauth::HEADER, sig).send().map_err(|e| format!("{path}: {e}"))?;
    if !res.status().is_success() {
        return Err(format!("{path}: HTTP {}", res.status()));
    }
    let list: Vec<serde_json::Value> = serde_json::from_slice(&res.bytes().map_err(|e| e.to_string())?).map_err(|e| format!("{path}: {e}"))?;
    let has = |p: &str| list.iter().any(|f| f["channel"] == channel && f["path"] == p);
    let lacks: Vec<&String> = named.iter().filter(|p| !has(p)).collect();
    if lacks.is_empty() {
        return Ok(());
    }
    Err(format!("index not written: this relay lacks {} which it names; run the publish command of that release again (it uploads the file), then this one", lacks.iter().map(|p| format!("{channel}/{p}")).collect::<Vec<_>>().join(", ")))
}

/// Admin-signed `PUT /api/admin/release/<rel>`.
fn put(c: &reqwest::blocking::Client, admin: &SigningKey, relay: &str, rel: &str, body: Vec<u8>) -> R<()> {
    let path = format!("/api/admin/release/{rel}");
    let sig = reqauth::sign_request_for(admin, relay, "PUT", &path, &body, now());
    let res = c.put(format!("{relay}{path}")).header(reqauth::HEADER, sig).header("Content-Type", "application/octet-stream").body(body).send().map_err(|e| format!("{rel}: {e}"))?;
    if res.status().is_success() {
        return Ok(());
    }
    Err(http_error(rel, res.status().as_u16(), &res.text().unwrap_or_default()))
}

/// A relay's refusal as one line: its body without control characters, cut to 300 characters.
fn http_error(what: &str, code: u16, body: &str) -> String {
    format!("{what}: HTTP {code}: {}", cell(Some(body), 300))
}

/// Every relay must answer the read (else the serial could go below one already published); the
/// files go to every relay, the index only to those that took them. A repair (see [`next_index`])
/// sends the current index unchanged, with the files it names, to the relays that lack it.
fn publish(admin: &SigningKey, relays: &[String], channel: &str, a: &Artifact, rollout: &str, min_version: &str, floor: &Path) -> R<()> {
    let c = client()?;
    let live = fetch_all(&c, admin, relays, channel)?;
    let base = live.iter().flatten().max_by_key(|(ix, _)| ix.serial).cloned();
    floor_check(floor, channel, base.as_ref().map_or(0, |b| b.0.serial))?;
    let (ix, doc, sig, targets): (Index, Vec<u8>, String, Vec<&String>) = match next_index(base.clone().map(|b| b.0), channel, a, rollout, min_version, now())? {
        Some(ix) => {
            let doc = sign_index(&ix, admin);
            (ix, doc, a.sig.clone(), relays.iter().collect())
        }
        None => {
            let (ix, raw) = base.expect("a repair has a base");
            let lacking = relays.iter().zip(&live).filter(|(_, l)| l.as_ref().is_none_or(|(i, _)| i.serial < ix.serial)).map(|(r, _)| r).collect();
            let sig = ix.releases[&a.dir].signature.clone();
            (ix, raw, sig, lacking)
        }
    };
    let serial = ix.serial;
    // Devices fetch `{relay}` URLs from the relay the index came from: it must hold every file.
    let named: Vec<String> = ix.releases.iter().filter(|(k, _)| **k != a.dir).map(|(k, e)| format!("{k}/{}", e.file)).collect();
    if targets.is_empty() {
        println!("every relay already publishes {} {} in {channel}/{MONITOR_INDEX_FILE}, serial {serial}", a.dir, a.version);
        return Ok(());
    }
    let mut failed = 0;
    for r in &targets {
        let res = put(&c, admin, r, &format!("{channel}/{}/{}", a.dir, a.name), a.data.clone())
            .and_then(|_| put(&c, admin, r, &format!("{channel}/{}/{}.sig", a.dir, a.name), sig.clone().into_bytes()))
            .and_then(|_| missing(&c, admin, r, channel, &named))
            .and_then(|_| put(&c, admin, r, &format!("{channel}/{MONITOR_INDEX_FILE}"), doc.clone()));
        match res {
            Ok(()) => println!("{r}: published {} {} in {channel}/{MONITOR_INDEX_FILE}, serial {serial}", a.dir, a.version),
            Err(e) => {
                failed += 1;
                println!("{r}: FAILED: {e}");
            }
        }
    }
    if failed < targets.len() {
        floor_raise(floor, channel, serial)?;
    }
    if failed > 0 {
        return Err(format!("{failed} of {} relays not updated; run the same command again to repair them", targets.len()));
    }
    Ok(())
}

/// Every relay's `monitor.json` for `channel`; every relay must answer (else the serial could go
/// below one already published).
fn fetch_all(c: &reqwest::blocking::Client, admin: &SigningKey, relays: &[String], channel: &str) -> R<Vec<Option<(Index, Vec<u8>)>>> {
    let trusted = trusted_keys(admin.verifying_key(), TRUSTED_ADMIN_KEYS.unwrap_or(""));
    relays.iter().map(|r| fetch_index(c, r, channel, &trusted)).collect()
}

/// The published index signed again, unchanged but for `serial + 1` and a new expiry.
fn refreshed(mut ix: Index, now: u64) -> R<Index> {
    ix.serial += 1;
    ix.expires = now + release::EXPIRY_SECS;
    ix.validate_for(MONITOR_APPS)?;
    Ok(ix)
}

/// `refresh`: the highest `monitor.json` of the relays, signed again with a new expiry, to every
/// relay that holds the files it names.
fn refresh(admin: &SigningKey, relays: &[String], channel: &str, floor: &Path) -> R<()> {
    let c = client()?;
    let live = fetch_all(&c, admin, relays, channel)?;
    let Some((base, _)) = live.into_iter().flatten().max_by_key(|(ix, _)| ix.serial) else {
        floor_check(floor, channel, 0)?;
        return Err(format!("nothing is published in {channel}/{MONITOR_INDEX_FILE} on these relays: publish with `index` first"));
    };
    floor_check(floor, channel, base.serial)?;
    let ix = refreshed(base, now())?;
    let doc = sign_index(&ix, admin);
    let named: Vec<String> = ix.releases.iter().map(|(k, e)| format!("{k}/{}", e.file)).collect();
    let mut failed = 0;
    for r in relays {
        match missing(&c, admin, r, channel, &named).and_then(|_| put(&c, admin, r, &format!("{channel}/{MONITOR_INDEX_FILE}"), doc.clone())) {
            Ok(()) => println!("{r}: refreshed {channel}/{MONITOR_INDEX_FILE}, serial {}", ix.serial),
            Err(e) => {
                failed += 1;
                println!("{r}: FAILED: {e}");
            }
        }
    }
    if failed < relays.len() {
        floor_raise(floor, channel, ix.serial)?;
        println!("{channel}/{MONITOR_INDEX_FILE} now expires {} (refresh again before then)", utc(ix.expires));
    }
    fail_count(failed, relays.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASS: &str = "throwaway passphrase";

    fn pass(p: &str) -> Option<Zeroizing<String>> {
        Some(Zeroizing::new(p.into()))
    }

    /// A temp folder, removed when dropped.
    struct Tmp(PathBuf);
    impl std::ops::Deref for Tmp {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tmp(name: &str) -> Tmp {
        let d = std::env::temp_dir().join(format!("monitor-publish-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Tmp(d)
    }

    /// The artifact `app` `version` with `data` and its `.sig` in `d`, read back.
    fn signed(d: &Path, k: &keys::Keys, app: &str, version: &str, data: &[u8]) -> Artifact {
        let name = target(app, version).unwrap().1;
        std::fs::write(d.join(&name), data).unwrap();
        std::fs::write(d.join(format!("{name}.sig")), k.sign_update(data, &name, version).unwrap()).unwrap();
        artifact(&d.join(&name), app, version, &k.updater_pub()).unwrap()
    }

    #[test]
    fn reads_the_manage_key_file_with_the_passphrase_only() {
        let d = tmp("vault");
        let k = keys::Keys::generate().unwrap();
        let p = d.join("keys.json");
        std::fs::write(&p, k.seal(PASS).unwrap()).unwrap();
        assert_eq!(open_keys(&p, pass(PASS)).unwrap().admin.to_bytes(), k.admin.to_bytes());
        assert_eq!(open_keys(&p, pass("wrong one")).err().unwrap(), "Wrong passphrase.");
        assert!(open_keys(&p, None).err().unwrap().contains("MANAGE_PASSPHRASE"));
        assert!(open_keys(&p.with_file_name("none.json"), pass(PASS)).err().unwrap().contains("none.json"));
    }

    #[test]
    fn token_days_are_bounded_and_the_token_verifies() {
        let k = keys::Keys::generate().unwrap();
        assert!(token(&k.admin, "x", 0, None, 1000).is_err());
        assert!(token(&k.admin, "x", 31, None, 1000).is_err());
        let (t, e) = token(&k.admin, "office", 7, None, 1000).unwrap();
        let p = tether_trust::status::verify_token(&t, &[k.admin.verifying_key()], 1000).unwrap();
        assert_eq!((p.exp, p.label.as_str(), p.id.as_str(), p.tether_id), (1000 + 7 * 86_400, "office", e.id.as_str(), None));
        let (t, e) = token(&k.admin, "front desk", 1, Some("123456789"), 1000).unwrap();
        let p = tether_trust::status::verify_token(&t, &[k.admin.verifying_key()], 1000).unwrap();
        assert_eq!((p.tether_id.as_deref(), e.tether_id.as_deref()), (Some("123456789"), Some("123456789")));
    }

    #[test]
    fn the_journal_never_holds_the_token() {
        let d = tmp("journal");
        let k = keys::Keys::generate().unwrap();
        let j = d.join("monitor-tokens.json");
        let (t1, e1) = issue(&k.admin, &j, "office", 7, None, 1000).unwrap();
        let (t2, e2) = issue(&k.admin, &j, "front desk", 1, Some("123456789"), 2000).unwrap();
        let raw = std::fs::read_to_string(&j).unwrap();
        assert!(!raw.contains(&t1) && !raw.contains(&t2), "no token in the journal");
        assert_eq!(journal_read(&j).unwrap(), vec![e1.clone(), e2.clone()]);
        assert_eq!((e2.label.as_str(), e2.exp, e2.tether_id.as_deref()), ("front desk", 2000 + 86_400, Some("123456789")));
        // The listing marks expired and revoked tokens.
        journal_update(&j, |l| l[0].revoked = Some(1500)).unwrap();
        let table = tokens_table(&journal_read(&j).unwrap(), 2000 + 86_400);
        let lines: Vec<&str> = table.lines().collect();
        assert!(lines[1].contains(&e1.id) && lines[1].contains("revoked"), "{table}");
        assert!(lines[2].contains(&e2.id) && lines[2].contains("expired") && lines[2].contains("123456789"), "{table}");
        assert!(tokens_table(&journal_read(&j).unwrap(), 2000).lines().nth(2).unwrap().contains("valid"));
        // A damaged journal is reported and left alone; no token is handed out then.
        std::fs::write(&j, "nonsense").unwrap();
        assert!(issue(&k.admin, &j, "x", 1, None, 1000).is_err());
        assert_eq!(std::fs::read_to_string(&j).unwrap(), "nonsense");
    }

    #[test]
    fn revoke_takes_the_expiry_from_the_journal() {
        let k = keys::Keys::generate().unwrap();
        let (_, e) = token(&k.admin, "office", 7, None, 1000).unwrap();
        let journal = vec![e.clone()];
        assert_eq!(revoke_exp(&journal, &e.id, None).unwrap(), 1000 + 7 * 86_400);
        assert!(revoke_exp(&journal, &e.id, Some(5)).unwrap_err().contains("earlier than the token's expiry"), "a revocation never ends before the token");
        assert_eq!(revoke_exp(&journal, &e.id, Some(1000 + 8 * 86_400)).unwrap(), 1000 + 8 * 86_400);
        assert_eq!(revoke_exp(&[], "abcdefghijklmnop", Some(9)).unwrap(), 9);
        assert!(revoke_exp(&journal, "abcdefghijklmnop", None).unwrap_err().contains("--token-file"));
    }

    #[test]
    fn the_token_itself_names_what_to_revoke() {
        let k = keys::Keys::generate().unwrap();
        let (t, e) = token(&k.admin, "office", 7, None, 1000).unwrap();
        assert_eq!(token_target(&format!("{t}
"), &[k.admin.verifying_key()], 1000).unwrap(), (e.id.clone(), e.exp));
        assert!(token_target(&t, &[k.admin.verifying_key()], e.exp).unwrap_err().contains("expired"));
        assert!(token_target(&t, &[keys::Keys::generate().unwrap().admin.verifying_key()], 1000).unwrap_err().contains("not a token of these admin keys"));
        // Revoked on some relays only: marked so, with the ones that failed; all of them later: revoked.
        let mut list = vec![e.clone()];
        mark_revoked(&mut list, &e.id, &["https://b".into()], 2000);
        assert_eq!((list[0].revoked, list[0].partly_revoked.clone()), (None, Some(vec!["https://b".to_owned()])));
        assert!(tokens_table(&list, 2000).lines().nth(1).unwrap().contains("partly revoked"));
        mark_revoked(&mut list, &e.id, &[], 3000);
        assert_eq!((list[0].revoked, list[0].partly_revoked.clone()), (Some(3000), None));
    }

    #[test]
    fn dates_and_device_cells() {
        assert_eq!(utc(0), "1970-01-01 00:00 UTC");
        assert_eq!(utc(951_782_400), "2000-02-29 00:00 UTC");
        assert_eq!(utc(1_795_000_000), "2026-11-18 11:06 UTC");
        assert_eq!(cell(Some("PC\u{1b}[31m-1"), 24), "PC[31m-1");
        assert_eq!(cell(Some(&"x".repeat(30)), 5), "xxxx…");
        assert_eq!(cell(None, 5), "-");
    }

    #[test]
    fn artifact_names_relay_addresses_and_trusted_keys() {
        assert_eq!(target("monitor", "0.1.0").unwrap(), ("monitor/windows-x86_64".into(), "tether-monitor-0.1.0.exe".into()));
        assert_eq!(target("monitor-packs", "2026.11.23.2").unwrap(), ("monitor-packs/any".into(), "monitor-packs-2026.11.23.2.json".into()));
        assert!(target("technician", "0.1.0").is_err());
        assert!(target("monitor", "../0.1").is_err());
        for ok in ["https://relay.example.org", "https://relay.example.org:8443/", "http://127.0.0.1:21120", "http://localhost:1"] {
            assert!(relay_ok(ok).is_ok(), "{ok}");
        }
        for bad in ["http://relay.example.org", "https://relay.example.org/api", "https://u@relay.example.org", "https://evil.com\\@relay.example.org", "relay.example.org", "https://r?x"] {
            assert!(relay_ok(bad).is_err(), "{bad}");
        }
        let (own, old) = (tether_trust::generate_key().verifying_key(), tether_trust::generate_key().verifying_key());
        let list = format!("{}, {},junk", tether_trust::key_to_b64(&old), tether_trust::key_to_b64(&own));
        assert_eq!(trusted_keys(own, &list), vec![own, old]);
        assert_eq!(trusted_keys(own, ""), vec![own]);
    }

    #[test]
    fn bad_arguments_are_refused_before_the_key_file_is_read() {
        let a = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        for (args, want) in [
            ("index --channel Bad --app monitor --version 0.1.0 --file x --relay https://r", "bad channel"),
            ("index --channel stable --app technician --version 0.1.0 --file x --relay https://r", "--app"),
            ("index --channel stable --app monitor --version ../1 --file x --relay https://r", "bad version"),
            ("index --channel stable --app monitor --version 0.1.0 --file x --rollout slow --relay https://r", "--rollout"),
            ("index --channel stable --app monitor --version 0.1.0 --file x --min-version a/b --relay https://r", "--min-version"),
            ("index --channel stable --app monitor --version 0.1.0 --file x --relay http://relay.example.org", "not a relay address"),
            ("token --label x --days 1 --tether-id m:dev-0001", "--tether-id"),
            ("revoke --id short --relay https://r", "bad token id"),
            ("revoke --id abcdefghijklmnop --exp soon --relay https://r", "--exp"),
            ("revoke --id abcdefghijklmnop --exp 1", "--relay is required"),
            ("devices --token bad! --relay https://r", "--token"),
            ("remove --uuid ../admin --relay https://r", "bad uuid"),
            ("remove --uuid dev-0001 --relay http://relay.example.org", "not a relay address"),
            ("tokens --all x", "unknown argument"),
            ("revoke --relay https://r", "give one of --id"),
            ("revoke --id abcdefghijklmnop --token-file x --relay https://r", "give one of --id"),
            ("revoke --token abc --relay https://r", "--token takes only -"),
        ] {
            let e = run(&a(args)).unwrap_err();
            assert!(e.contains(want), "{args}: {e}");
        }
    }

    #[test]
    fn signature_is_checked_before_anything_is_sent() {
        let d = tmp("sig");
        let k = keys::Keys::generate().unwrap();
        let f = d.join("tether-monitor-0.1.0.exe");
        std::fs::write(&f, b"agent").unwrap();
        assert!(artifact(&f, "monitor", "0.1.0", &k.updater_pub()).err().unwrap().contains(".sig"));
        std::fs::write(d.join("tether-monitor-0.1.0.exe.sig"), k.sign_update(b"agent", "tether-monitor-0.1.0.exe", "0.1.1").unwrap()).unwrap();
        assert!(artifact(&f, "monitor", "0.1.0", &k.updater_pub()).err().unwrap().contains("version"));
        let a = signed(&d, &k, "monitor", "0.1.0", b"agent");
        assert_eq!((a.dir.as_str(), a.name.as_str(), a.data.as_slice()), ("monitor/windows-x86_64", "tether-monitor-0.1.0.exe", &b"agent"[..]));
        assert!(artifact(&f, "monitor", "0.1.0", &keys::Keys::generate().unwrap().updater_pub()).is_err(), "other updater key");
        assert!(artifact(&f, "monitor", "0.2.0", &k.updater_pub()).err().unwrap().contains("tether-monitor-0.2.0.exe"), "file name must match");
    }

    #[test]
    fn next_index_merges_bumps_and_stays_monitor_only() {
        let d = tmp("ix");
        let k = keys::Keys::generate().unwrap();
        let a = signed(&d, &k, "monitor-packs", "2026.11.23", b"{}");
        let first = next_index(None, "stable", &a, "now", "", 1000).unwrap().unwrap();
        assert_eq!(first.serial, 1);
        let e = &first.releases["monitor-packs/any"];
        assert_eq!((e.size, e.urls.clone(), e.rollout.start), (2, vec!["https://{relay}/download/stable/monitor-packs/any/monitor-packs-2026.11.23.json".to_owned()], 1000));
        assert_eq!(e.signature, std::fs::read_to_string(d.join("monitor-packs-2026.11.23.json.sig")).unwrap().trim());
        let mut base = first.clone();
        base.serial = 7;
        base.releases.clear();
        base.releases.insert("monitor/windows-x86_64".into(), Entry { version: "0.0.1".into(), file: "tether-monitor-0.0.1.exe".into(), ..e.clone() });
        let next = next_index(Some(base), "stable", &a, "standard", "0.0.5", 2000).unwrap().unwrap();
        assert_eq!((next.serial, next.issued, next.releases.len()), (8, 2000, 2), "merged, serial bumped");
        assert_eq!(next.releases["monitor-packs/any"].rollout.steps, release::RAMP_STANDARD.to_vec());
        assert_eq!(next.releases["monitor-packs/any"].minimum_version, "0.0.5");
        assert!(next.validate_for(MONITOR_APPS).is_ok() && next.validate().is_err(), "never a valid releases.json");
        assert!(next_index(None, "stable", &a, "slow", "", 1).is_err());
        assert!(next_index(None, "Bad", &a, "now", "", 1).is_err());
        let doc = sign_index(&next, &k.admin);
        assert_eq!(Index::open_for(&doc, &[k.admin.verifying_key()], "stable", MONITOR_APPS).unwrap(), next);
    }

    #[test]
    fn refresh_changes_only_the_serial_and_the_expiry() {
        let d = tmp("refresh");
        let k = keys::Keys::generate().unwrap();
        let a = signed(&d, &k, "monitor", "0.1.0", b"agent");
        let base = next_index(None, "stable", &a, "fast", "", 1000).unwrap().unwrap();
        let r = refreshed(base.clone(), 5000).unwrap();
        assert_eq!((r.serial, r.expires), (base.serial + 1, 5000 + release::EXPIRY_SECS));
        assert_eq!((r.issued, &r.releases, &r.channel), (base.issued, &base.releases, &base.channel), "the releases and their rollout stay");
    }

    #[test]
    fn the_serial_floor_only_goes_up_and_refuses_lower_relays() {
        let d = tmp("floor");
        let p = d.join("monitor-serials.json");
        assert!(floor_check(&p, "stable", 0).is_ok(), "nothing published from here yet");
        floor_raise(&p, "stable", 5).unwrap();
        floor_raise(&p, "stable", 3).unwrap();
        floor_raise(&p, "test", 1).unwrap();
        assert_eq!(floor_read(&p).unwrap(), [("stable".to_owned(), 5), ("test".to_owned(), 1)].into());
        assert!(floor_check(&p, "stable", 5).is_ok(), "the same serial: the next one is 6");
        let e = floor_check(&p, "stable", 4).unwrap_err();
        assert!(e.contains("serial 4") && e.contains("serial 5") && e.contains("rolled back"), "{e}");
        assert!(floor_check(&p, "stable", 0).is_err(), "every relay answered 404");
        // A damaged file is reported and left alone.
        std::fs::write(&p, "nonsense").unwrap();
        assert!(floor_check(&p, "stable", 9).is_err() && floor_raise(&p, "stable", 9).is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "nonsense");
    }

    #[test]
    fn refresh_arguments_are_checked_before_the_key_file_is_read() {
        let a = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        for (args, want) in [("refresh --channel Bad --relay https://r", "bad channel"), ("refresh --channel stable", "--relay is required"), ("refresh --relay https://r", "--channel is required"), ("refresh --channel stable --relay http://relay.example.org", "not a relay address")] {
            let e = run(&a(args)).unwrap_err();
            assert!(e.contains(want), "{args}: {e}");
        }
    }

    #[test]
    fn relay_error_bodies_are_printed_plain_and_short() {
        let e = http_error("stable/monitor.json", 500, &format!("bad\u{1b}[2J\r\nnews{}", "y".repeat(400)));
        assert!(!e.chars().any(char::is_control), "{e:?}");
        assert!(e.starts_with("stable/monitor.json: HTTP 500: bad[2Jnewsyyy") && e.chars().count() < 340, "{e}");
    }

    #[test]
    fn versions_only_go_up_and_the_same_bytes_are_a_repair() {
        let d = tmp("mono");
        let k = keys::Keys::generate().unwrap();
        let v2 = signed(&d, &k, "monitor", "0.2.0", b"agent two");
        let base = next_index(None, "stable", &v2, "standard", "", 1000).unwrap().unwrap();
        let v1 = signed(&d, &k, "monitor", "0.1.0", b"agent one");
        assert!(next_index(Some(base.clone()), "stable", &v1, "now", "", 2000).unwrap_err().contains("not newer"), "lower");
        let other = signed(&d, &k, "monitor", "0.2.0", b"agent two, rebuilt");
        assert!(next_index(Some(base.clone()), "stable", &other, "now", "", 2000).unwrap_err().contains("other bytes"), "same version, other bytes");
        // Same version, same bytes: a repair; the published entry (rollout start 1000) stays.
        let again = signed(&d, &k, "monitor", "0.2.0", b"agent two");
        assert_eq!(next_index(Some(base.clone()), "stable", &again, "now", "", 2000).unwrap(), None);
        assert_eq!(base.releases["monitor/windows-x86_64"].rollout.start, 1000);
        let v3 = signed(&d, &k, "monitor", "0.3.0", b"agent three");
        let next = next_index(Some(base), "stable", &v3, "now", "", 3000).unwrap().unwrap();
        assert_eq!((next.serial, next.releases["monitor/windows-x86_64"].rollout.start), (2, 3000));
    }
}

//! Team activity (design 12.5): the relays' team audit logs (`GET /api/admin/team-audit`) merged
//! by time, and per relay the last chain head this PC checked (`team-audit-heads.json`: the head
//! and the time of the newest record checked, nothing else). Each read checks the chain itself:
//! every record since the stored head is hashed over its exact line bytes and must be the `prev`
//! of the next one, up to the relay's head; so a relay that rewrote its log, or set `prev` to the
//! stored head over rewritten records, is caught. The stored head moves only along a checked
//! chain (or when none was stored); a log that no longer continues, or one where the record after
//! the stored head was not found within what was read, is warned about until the person reads
//! further back or accepts the relay's current state. The problems the relay reports in what it
//! read (`problems`) and the records of dropped lines and chain gaps are warnings too.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use serde_json::{json, Value};
use tauri::State;

use crate::{App, R};

const HEADS: &str = "team-audit-heads.json";
/// Pages of 5000 records read to check the chain since the stored head; "Read further back": DEEP_PAGES.
const VERIFY_PAGES: usize = 10;
const DEEP_PAGES: usize = 100;
/// The check reads from this long before the stored head's time: a record is stamped when its
/// request came and written when it was answered, so the next one may be a little older.
/// shortcut: a record answered more than an hour after it came would read as a break; raise if seen.
const MARGIN: u64 = 3600;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Head {
    pub head: String,
    /// Relay time of the newest record checked with it.
    pub ts: u64,
}

#[derive(Deserialize, Default)]
pub struct Filter {
    since: Option<u64>,
    until: Option<u64>,
    actor: Option<String>,
    device: Option<String>,
    customer: Option<String>,
    event: Option<String>,
    limit: Option<u32>,
}

fn enc(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

impl Filter {
    fn query(&self) -> String {
        let mut q = vec![format!("limit={}", self.limit.unwrap_or(1000).clamp(1, 5000))];
        q.extend(self.since.map(|v| format!("since={v}")));
        q.extend(self.until.map(|v| format!("until={v}")));
        for (k, v) in [("actor", &self.actor), ("device", &self.device), ("customer", &self.customer), ("event", &self.event)] {
            if let Some(v) = v.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
                q.push(format!("{k}={}", enc(v)));
            }
        }
        q.join("&")
    }
}

fn load(app: &App) -> BTreeMap<String, Head> {
    std::fs::read(app.path(HEADS)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// What a read shows about the log itself: the relay's `problems`, and records of dropped lines
/// or a chain gap the relay found at start.
fn warnings_of(relay: &str, page: &Value) -> Vec<String> {
    let mut w = Vec::new();
    for p in page["problems"].as_array().into_iter().flatten() {
        let what = match p["problem"].as_str().unwrap_or("") {
            "prev-mismatch" => "a line does not follow the one before it (changed, removed or inserted)",
            "unreadable" => "an unreadable line",
            "truncated" => "a line cut off",
            "head-not-found" => "the newest lines are not the ones the relay last wrote",
            "starts-after-removed-day" => continue, // retention, not a break
            other => other,
        };
        w.push(format!("{relay}: the log shows {what} ({} at byte {}).", p["day"].as_str().unwrap_or("?"), p["at"]));
    }
    for r in page["records"].as_array().into_iter().flatten() {
        match r["event"].as_str() {
            Some("dropped") => w.push(format!("{relay}: records were dropped ({}).", r["detail"])),
            Some("chain-gap") => w.push(format!("{relay}: the relay found a gap in its chain at start ({}).", r["detail"])),
            _ => {}
        }
    }
    w
}

/// What the check found.
#[derive(Debug, PartialEq)]
enum Chain {
    /// Every record since the stored head follows the one before it, up to the relay's head.
    Continues(Head),
    /// Follows as far as read; more to read. The head moves to the last record checked.
    Partly(Head),
    /// The record after the stored head was not among those read; more to read. The head stays.
    Undecided,
    /// Removed, rewritten or cut.
    Broken,
}

/// The chain check over the records since the stored head, fed page by page (oldest first).
struct Walk {
    stored: Head,
    /// The hash of the last record checked; `None` until the record after the stored head is seen.
    cur: Option<String>,
    ts: u64,
    seen: HashSet<String>,
    broken: bool,
}

impl Walk {
    fn new(stored: &Head) -> Walk {
        Walk { stored: stored.clone(), cur: None, ts: stored.ts, seen: HashSet::new(), broken: false }
    }

    /// Records as the relay holds them (the exact line bytes it hashed).
    fn feed(&mut self, lines: &[&str]) {
        for l in lines {
            let v: Value = serde_json::from_str(l).unwrap_or(Value::Null);
            let prev = v["prev"].as_str().unwrap_or("");
            match &self.cur {
                None if prev != self.stored.head => continue, // before the stored head
                None => {}
                Some(c) if prev != c => self.broken = true,
                Some(_) => {}
            }
            let h = crate::proposals::sha256_hex(l.as_bytes());
            self.ts = self.ts.max(v["ts"].as_u64().unwrap_or(0));
            self.seen.insert(h.clone());
            self.cur = Some(h);
        }
    }

    /// `relay_head`: the head the relay named in its last answer; `done`: nothing more to read.
    fn finish(&self, relay_head: &str, done: bool) -> Chain {
        match &self.cur {
            _ if self.broken => Chain::Broken,
            None if relay_head == self.stored.head => Chain::Continues(self.stored.clone()),
            None if done => Chain::Broken,
            None => Chain::Undecided,
            Some(c) if !done => Chain::Partly(Head { head: c.clone(), ts: self.ts }),
            // The relay's head was written before it answered, so it is among the records read.
            Some(c) if self.seen.contains(relay_head) => Chain::Continues(Head { head: c.clone(), ts: self.ts }),
            Some(_) => Chain::Broken,
        }
    }
}

/// An answer of the route: parsed, and its records as the exact bytes the relay sent.
#[derive(Deserialize)]
struct Raw {
    head: String,
    records: Vec<Box<RawValue>>,
}

async fn page_bytes(app: &App, relay: &str, q: &str) -> R<Vec<u8>> {
    app.call_on(relay, "GET", &format!("/api/admin/team-audit?{q}"), Vec::new(), "application/json").await
}

async fn page(app: &App, relay: &str, q: &str) -> R<Value> {
    serde_json::from_slice(&page_bytes(app, relay, q).await?).map_err(|_| "unexpected team audit answer".to_owned())
}

/// Reads unfiltered from `since` up to `pages` pages, feeding `walk`: the relay's last head,
/// whether everything was read, the read's warnings and the newest record time.
async fn read_since(app: &App, relay: &str, since: u64, pages: usize, walk: &mut Walk) -> R<(String, bool, Vec<String>, Option<u64>)> {
    let (mut q, mut head, mut warnings, mut newest) = (format!("since={since}&limit=5000"), String::new(), Vec::new(), None);
    for _ in 0..pages {
        let b = page_bytes(app, relay, &q).await?;
        let bad = |_| "unexpected team audit answer".to_owned();
        let (v, raw): (Value, Raw) = (serde_json::from_slice(&b).map_err(bad)?, serde_json::from_slice(&b).map_err(bad)?);
        walk.feed(&raw.records.iter().map(|r| r.get()).collect::<Vec<_>>());
        warnings.extend(warnings_of(relay, &v));
        newest = newest.max(newest_ts(&v));
        head = raw.head;
        match v["next"].as_str() {
            Some(n) => q = format!("limit=5000&cursor={}", enc(n)),
            None => return Ok((head, true, warnings, newest)),
        }
    }
    Ok((head, false, warnings, newest))
}

/// One relay: the chain check against the stored head, then the filtered read.
async fn relay_activity(app: &App, relay: &str, stored: Option<Head>, f: &str, pages: usize) -> R<(Vec<Value>, Value, Option<Head>)> {
    let mut warnings = Vec::new();
    let mut chain = None;
    if let Some(h) = &stored {
        let mut walk = Walk::new(h);
        let (head, done, w, _) = read_since(app, relay, h.ts.saturating_sub(MARGIN), pages, &mut walk).await?;
        let c = walk.finish(&head, done);
        let short = &h.head[..h.head.len().min(12)];
        match &c {
            Chain::Broken => warnings.push(format!("{relay}: the log no longer continues from the point this PC last read ({short}…): records were removed or rewritten since. Compare with a copy kept elsewhere (the relay's journal), then accept the new state.")),
            Chain::Undecided => warnings.push(format!("{relay}: too many records since the last read to find where the log continues from the point this PC last checked ({short}…); that point is kept. Read further back, or accept the relay's current state.")),
            Chain::Partly(_) => warnings.push(format!("{relay}: the log continues as far as read, but more records since the last check remain; read again to check the rest.")),
            Chain::Continues(_) => {}
        }
        warnings.extend(w);
        chain = Some(c);
    }
    let main = page(app, relay, f).await?;
    warnings.extend(warnings_of(relay, &main));
    warnings.dedup();
    let head = main["head"].as_str().unwrap_or_default().to_owned();
    // The stored head moves only along a checked chain, or is set when there was none.
    let (new, state) = match chain {
        None => (Some(Head { head: head.clone(), ts: newest_ts(&main).unwrap_or(0) }), "first"),
        Some(Chain::Continues(h)) => (Some(h), "ok"),
        Some(Chain::Partly(h)) => (Some(h), "partly"),
        Some(Chain::Undecided) => (None, "undecided"),
        Some(Chain::Broken) => (None, "broken"),
    };
    let mut records: Vec<Value> = main["records"].as_array().cloned().unwrap_or_default();
    for r in &mut records {
        r["relay"] = json!(relay);
    }
    let info = json!({"server": relay, "ok": true, "head": head, "more": !main["next"].is_null(), "warnings": warnings, "chain": state, "broken": state == "broken"});
    Ok((records, info, new))
}

fn newest_ts(page: &Value) -> Option<u64> {
    page["records"].as_array().into_iter().flatten().filter_map(|r| r["ts"].as_u64()).max()
}

/// The team activity of every relay with `filter`, newest first, and each relay's state; `deep`:
/// the chain check reads further back ("Read further back").
#[tauri::command]
pub async fn team_activity(app: State<'_, App>, filter: Option<Filter>, deep: Option<bool>) -> R<Value> {
    activity(&app, filter.unwrap_or_default(), deep.unwrap_or(false)).await
}

async fn activity(app: &App, filter: Filter, deep: bool) -> R<Value> {
    let f = filter.query();
    let pages = if deep { DEEP_PAGES } else { VERIFY_PAGES };
    let servers = app.servers();
    let heads = load(app);
    let got = futures_util::future::join_all(servers.iter().map(|sv| relay_activity(app, sv, heads.get(sv).cloned(), &f, pages))).await;
    let (mut all, mut relays, mut heads_new) = (Vec::new(), Vec::new(), heads.clone());
    for (sv, r) in servers.iter().zip(got) {
        match r {
            Ok((recs, info, new)) => {
                all.extend(recs);
                relays.push(info);
                if let Some(h) = new {
                    heads_new.insert(sv.clone(), h);
                }
            }
            Err(e) => relays.push(json!({"server": sv, "ok": false, "error": e, "warnings": []})),
        }
    }
    if heads_new != heads {
        app.write(HEADS, &serde_json::to_vec(&heads_new).unwrap())?;
    }
    all.sort_by_key(|r| std::cmp::Reverse(r["ts"].as_u64()));
    Ok(json!({"records": all, "relays": relays}))
}

/// Accepts a relay's current state after a warning (the person compared it with a copy kept
/// elsewhere, or took the risk): its newest record and that record's time.
#[tauri::command]
pub async fn team_activity_accept(app: State<'_, App>, server: String) -> R<()> {
    accept(&app, &server).await
}

async fn accept(app: &App, server: &str) -> R<()> {
    if !app.servers().iter().any(|s| s == server) {
        return Err("This relay is not in the relay list.".into());
    }
    let mut heads = load(app);
    let old = heads.get(server).cloned().unwrap_or(Head { head: String::new(), ts: 0 });
    let mut walk = Walk::new(&old);
    let (head, done, _, newest) = read_since(app, server, old.ts.saturating_sub(MARGIN), DEEP_PAGES, &mut walk).await?;
    if !done {
        return Err("Too many records to read on this relay; try again later.".into());
    }
    // The relay's time of its newest record (not this PC's clock): the next check reads from there.
    heads.insert(server.to_owned(), Head { head, ts: newest.unwrap_or(old.ts) });
    app.write(HEADS, &serde_json::to_vec(&heads).unwrap())
}

/// Manage's own record of an export (`POST /api/admin/audit-event`) on every relay; failures are not shown.
#[tauri::command]
pub async fn audit_export(app: State<'_, App>, what: String, rows: u64) -> R<()> {
    let body = serde_json::to_vec(&json!({"event": "export", "what": what, "rows": rows})).unwrap();
    let _ = app.fan_out("POST", "/api/admin/audit-event", body, "application/json").await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proposals::sha256_hex;
    use std::sync::Mutex;

    /// `n` chained records after `prev0`, as the relay writes them (the line is what is hashed).
    fn chain(prev0: &str, n: usize, ts0: u64) -> Vec<String> {
        let mut prev = prev0.to_owned();
        (0..n)
            .map(|i| {
                let l = format!(r#"{{"ts":{},"event":"sign-in","prev":"{prev}"}}"#, ts0 + i as u64);
                prev = sha256_hex(l.as_bytes());
                l
            })
            .collect()
    }

    fn answer(head: &str, lines: &[String], next: bool) -> Vec<u8> {
        format!(r#"{{"head":"{head}","records":[{}],"next":{},"problems":[]}}"#, lines.join(","), if next { "\"2026-11-26:0:00\"" } else { "null" }).into_bytes()
    }

    fn walked(stored: &Head, lines: &[String], relay_head: &str, done: bool) -> Chain {
        let mut w = Walk::new(stored);
        w.feed(&lines.iter().map(String::as_str).collect::<Vec<_>>());
        w.finish(relay_head, done)
    }

    #[test]
    fn the_chain_is_checked_over_the_exact_lines_since_the_stored_head() {
        let h0 = "0".repeat(64);
        let older = chain(&"1".repeat(64), 2, 90);
        let stored = Head { head: sha256_hex(older[1].as_bytes()), ts: 91 };
        let l = chain(&stored.head, 3, 100);
        let tip = sha256_hex(l[2].as_bytes());
        let all: Vec<String> = older.iter().chain(&l).cloned().collect();
        assert_eq!(walked(&stored, &all, &tip, true), Chain::Continues(Head { head: tip.clone(), ts: 102 }));
        // Nothing new: the relay's head is still the stored one.
        assert_eq!(walked(&stored, &older, &stored.head, true), Chain::Continues(stored.clone()));
        // A record between the stored head and the end rewritten (its own prev still right): the
        // next record no longer names its hash.
        let mut forged = l.clone();
        forged[1] = forged[1].replace("sign-in", "sign-out");
        assert_eq!(walked(&stored, &forged, &tip, true), Chain::Broken);
        // A relay that names the stored head as prev of a rewritten record.
        let other = chain(&stored.head, 2, 100);
        assert_eq!(walked(&stored, &[other[0].replace("sign-in", "export"), l[1].clone(), l[2].clone()], &tip, true), Chain::Broken);
        // The relay's head is not among the records read.
        assert_eq!(walked(&stored, &l, &h0, true), Chain::Broken);
        // More to read: checked so far (the head moves along it), or the start not found yet.
        assert_eq!(walked(&stored, &l[..2], &tip, false), Chain::Partly(Head { head: sha256_hex(l[1].as_bytes()), ts: 101 }));
        assert_eq!(walked(&stored, &chain(&h0, 2, 100), &tip, false), Chain::Undecided);
        assert_eq!(walked(&stored, &chain(&h0, 2, 100), &tip, true), Chain::Broken);
    }

    #[test]
    fn problems_dropped_records_and_gaps_are_warnings() {
        let p = json!({"head": "h", "records": [{"event": "dropped", "detail": {"queue_full": 3}}, {"event": "chain-gap", "detail": {"expected": "a", "found": "b"}}, {"event": "summary"}], "problems": [{"day": "2026-11-26", "at": 120, "problem": "prev-mismatch"}, {"day": "2026-08-01", "at": 0, "problem": "starts-after-removed-day"}]});
        let w = warnings_of("https://r1", &p);
        assert_eq!(w.len(), 3, "{w:?}");
        assert!(w[0].contains("does not follow") && w[0].contains("2026-11-26"));
        assert!(w[1].contains("dropped") && w[2].contains("gap"));
    }

    #[test]
    fn filters_become_the_routes_query() {
        let f = Filter { since: Some(5), actor: Some("Anna Berger".into()), event: Some("".into()), limit: Some(99_999), ..Default::default() };
        assert_eq!(f.query(), "limit=5000&since=5&actor=Anna%20Berger");
        assert_eq!(Filter::default().query(), "limit=1000");
    }

    fn app_with(name: &str, relays: &[&String]) -> App {
        let dir = std::env::temp_dir().join(format!("tm-activity-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let app = App { dir, keys: Mutex::new(Some(crate::keys::Keys::generate().unwrap())), diag: Mutex::new(None) };
        app.write("settings.json", json!({"server": relays.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(",")}).to_string().as_bytes()).unwrap();
        app
    }

    /// Two relays: one whose record after the stored head is not within the pages read (its head
    /// stays, it is warned about), one read for the first time; their records merged by time.
    #[test]
    fn two_relays_merge_and_an_undecided_check_keeps_the_stored_head() {
        let h_a = Head { head: "a".repeat(64), ts: 10_000 };
        let (a, seen_a) = crate::api::tests::fake_relay(|_, path, _| {
            if path.contains("limit=5000") {
                // Always more, never the record after the stored head.
                return (200, answer(&"9".repeat(64), &chain(&"8".repeat(64), 2, 9_000), true));
            }
            (200, answer(&"9".repeat(64), &[r#"{"ts":5,"event":"export","prev":"x"}"#.into(), r#"{"ts":7,"event":"sign-in","prev":"y"}"#.into()], false))
        });
        let b_lines = chain(&"0".repeat(64), 1, 6);
        let b_head = sha256_hex(b_lines[0].as_bytes());
        let (b, _) = crate::api::tests::fake_relay(move |_, _, _| (200, answer(&b_head, &b_lines, false)));
        let app = app_with("merge", &[&a, &b]);
        app.write(HEADS, &serde_json::to_vec(&BTreeMap::from([(a.clone(), h_a.clone())])).unwrap()).unwrap();
        let out = tauri::async_runtime::block_on(activity(&app, Filter::default(), false)).unwrap();
        let got: Vec<(u64, String)> = out["records"].as_array().unwrap().iter().map(|r| (r["ts"].as_u64().unwrap(), r["relay"].as_str().unwrap().to_owned())).collect();
        assert_eq!(got, [(7, a.clone()), (6, b.clone()), (5, a.clone())]);
        let ra = &out["relays"][0];
        assert_eq!((ra["chain"].as_str(), ra["broken"].as_bool()), (Some("undecided"), Some(false)));
        assert!(ra["warnings"][0].as_str().unwrap().contains("that point is kept"), "{ra}");
        assert_eq!(out["relays"][1]["chain"], "first");
        let heads = load(&app);
        assert_eq!(heads[&a], h_a, "an undecided check does not move the stored head");
        assert_eq!(heads[&b].ts, 6);
        // The check read from before the stored time, then followed the relay's cursor.
        let verify: Vec<String> = seen_a.lock().unwrap().iter().map(|r| r.1.clone()).filter(|p| p.contains("limit=5000")).collect();
        assert_eq!((verify.len(), verify[0].as_str()), (VERIFY_PAGES, format!("/api/admin/team-audit?since={}&limit=5000", 10_000 - MARGIN).as_str()));
        assert!(verify[1].contains("cursor=2026-11-26%3A0%3A00"), "{verify:?}");
    }

    /// A relay that rewrote a record after the stored head is broken (head kept); accepting its
    /// state stores its head with the time of its newest record, not this PC's clock.
    #[test]
    fn a_rewritten_log_is_broken_and_accepting_takes_the_relays_time() {
        let stored = Head { head: "5".repeat(64), ts: 100 };
        let mut l = chain(&stored.head, 3, 100);
        let tip = sha256_hex(l[2].as_bytes());
        l[1] = l[1].replace("sign-in", "sign-out");
        let (c, _) = crate::api::tests::fake_relay(move |_, _, _| (200, answer(&tip, &l, false)));
        let app = app_with("rewritten", &[&c]);
        app.write(HEADS, &serde_json::to_vec(&BTreeMap::from([(c.clone(), stored.clone())])).unwrap()).unwrap();
        let out = tauri::async_runtime::block_on(activity(&app, Filter::default(), false)).unwrap();
        assert_eq!(out["relays"][0]["chain"], "broken");
        assert!(out["relays"][0]["warnings"][0].as_str().unwrap().contains("removed or rewritten"));
        assert_eq!(load(&app)[&c], stored);
        tauri::async_runtime::block_on(accept(&app, &c)).unwrap();
        let h = &load(&app)[&c];
        assert_eq!((h.ts, h.head.len()), (102, 64), "the newest record's time");
        assert!(tauri::async_runtime::block_on(accept(&app, "https://elsewhere")).is_err());
    }
}

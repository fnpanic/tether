//! Team activity (design 12.5): the relays' team audit logs (`GET /api/admin/team-audit`) merged
//! by time, and per relay the last chain head this PC saw (`team-audit-heads.json`: the head and
//! the time of the newest record read, nothing else). A relay whose log no longer continues from
//! that head (a record after it names it as `prev`, or the relay's head is still it) was rewritten
//! or cut, and is warned about until the user accepts the new head; so are the problems the relay
//! reports in what it read (`problems`) and the records of dropped lines and chain gaps.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::State;

use crate::{App, R};

const HEADS: &str = "team-audit-heads.json";
/// Pages of 5000 records read to find the record after the stored head.
const VERIFY_PAGES: usize = 10;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Head {
    pub head: String,
    /// Relay time of the newest record read with it (the next record is not older).
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

/// Whether the log on a relay still continues from `stored`, from unfiltered pages read since
/// its time: `Some(true)` it does, `Some(false)` it does not, `None` not decided within the pages.
fn continues(stored: &Head, pages: &[Value], done: bool) -> Option<bool> {
    if pages.iter().any(|p| p["head"] == stored.head.as_str() || p["records"].as_array().into_iter().flatten().any(|r| r["prev"] == stored.head.as_str())) {
        return Some(true);
    }
    done.then_some(false)
}

fn newest_ts(pages: &[Value]) -> Option<u64> {
    pages.iter().flat_map(|p| p["records"].as_array().into_iter().flatten()).filter_map(|r| r["ts"].as_u64()).max()
}

async fn page(app: &App, relay: &str, q: &str) -> R<Value> {
    let b = app.call_on(relay, "GET", &format!("/api/admin/team-audit?{q}"), Vec::new(), "application/json").await?;
    serde_json::from_slice(&b).map_err(|_| "unexpected team audit answer".to_owned())
}

/// One relay: the chain check against the stored head, then the filtered read.
async fn relay_activity(app: &App, relay: &str, stored: Option<Head>, f: &str) -> R<(Vec<Value>, Value, Option<Head>)> {
    let mut warnings = Vec::new();
    let mut pages = Vec::new();
    let mut verdict = None;
    if let Some(h) = &stored {
        let mut q = format!("since={}&limit=5000", h.ts.saturating_sub(60));
        for _ in 0..VERIFY_PAGES {
            let p = page(app, relay, &q).await?;
            let next = p["next"].as_str().map(str::to_owned);
            pages.push(p);
            verdict = continues(h, &pages, next.is_none());
            match (verdict, next) {
                (None, Some(n)) => q = format!("limit=5000&cursor={}", enc(&n)),
                _ => break,
            }
        }
        match verdict {
            Some(false) => warnings.push(format!("{relay}: the log no longer continues from the point this PC last read ({}…): records were removed or rewritten since. Compare with a copy kept elsewhere (the relay's journal), then accept the new state.", &h.head[..h.head.len().min(12)])),
            None => warnings.push(format!("{relay}: too many records since the last read to check that the log continues; read again later.")),
            Some(true) => {}
        }
        for p in &pages {
            warnings.extend(warnings_of(relay, p));
        }
    }
    let main = page(app, relay, f).await?;
    warnings.extend(warnings_of(relay, &main));
    warnings.dedup();
    let head = main["head"].as_str().unwrap_or_default().to_owned();
    let ts = newest_ts(&pages).into_iter().chain(newest_ts(std::slice::from_ref(&main))).max().or(stored.as_ref().map(|h| h.ts)).unwrap_or(0);
    // The stored head moves on only when the log continues from it (or there was none).
    let new = (verdict != Some(false)).then(|| Head { head: head.clone(), ts });
    let mut records: Vec<Value> = main["records"].as_array().cloned().unwrap_or_default();
    for r in &mut records {
        r["relay"] = json!(relay);
    }
    let info = json!({"server": relay, "ok": true, "head": head, "more": !main["next"].is_null(), "warnings": warnings, "broken": verdict == Some(false)});
    Ok((records, info, new))
}

/// The team activity of every relay with `filter`, newest first, and each relay's state.
#[tauri::command]
pub async fn team_activity(app: State<'_, App>, filter: Option<Filter>) -> R<Value> {
    let f = filter.unwrap_or_default().query();
    let servers = app.servers();
    let heads = load(&app);
    let got = futures_util::future::join_all(servers.iter().map(|sv| relay_activity(&app, sv, heads.get(sv).cloned(), &f))).await;
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

/// Accepts a relay's current head after a warning (the user compared it with a copy kept elsewhere).
#[tauri::command]
pub async fn team_activity_accept(app: State<'_, App>, server: String) -> R<()> {
    if !app.servers().contains(&server) {
        return Err("This relay is not in the relay list.".into());
    }
    let p = page(&app, &server, "limit=1").await?;
    let mut heads = load(&app);
    heads.insert(server, Head { head: p["head"].as_str().unwrap_or_default().to_owned(), ts: crate::api::now() });
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

    #[test]
    fn the_chain_continues_only_through_the_stored_head() {
        let h = Head { head: "h1".into(), ts: 100 };
        let page = |head: &str, prevs: &[&str], next: bool| json!({"head": head, "records": prevs.iter().map(|p| json!({"prev": p, "ts": 101})).collect::<Vec<_>>(), "next": if next { json!("c") } else { Value::Null }, "problems": []});
        // Nothing new: the relay's head is still ours.
        assert_eq!(continues(&h, &[page("h1", &[], false)], true), Some(true));
        // A record names it as its previous line.
        assert_eq!(continues(&h, &[page("h9", &["h0", "h1"], false)], true), Some(true));
        // Everything since read, the head nowhere: rewritten or cut.
        assert_eq!(continues(&h, &[page("h9", &["x", "y"], false)], true), Some(false));
        // More to read: not decided yet.
        assert_eq!(continues(&h, &[page("h9", &["x"], true)], false), None);
        assert_eq!(newest_ts(&[page("h9", &["x"], false)]), Some(101));
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
}

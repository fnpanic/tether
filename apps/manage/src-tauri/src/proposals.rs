//! Proposals (design 12.4): a PolicyEditor or Publisher signed in without the keys stores a draft
//! on every relay (`PUT /api/admin/proposals/<kind>`, unsigned, never live); a key holder lists
//! them, loads one, sees its exact difference against what is live, signs it with the vault and
//! publishes it, then removes it. Each relay names the proposal with its own id, so the list
//! merges the relays' copies by author, kind and content.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use sha2::Digest;
use tauri::State;

use crate::{App, RelayResult, R};

pub const KINDS: [&str; 3] = ["policy", "team", "release"];

fn kind_ok(kind: &str) -> R<()> {
    if KINDS.contains(&kind) { Ok(()) } else { Err("unknown proposal kind".into()) }
}

pub fn sha256_hex(b: &[u8]) -> String {
    sha2::Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// What `relay` serves now for `kind`, hashed as the relay hashes it (`live` of its list): the
/// signed policy, or the team document (signed upload or plain roster). Empty: none, or a release.
async fn base_on(app: &App, relay: &str, kind: &str) -> R<String> {
    Ok(match kind {
        "policy" => crate::api::public_quick(relay, "/api/policy").await?.map_or(String::new(), |b| sha256_hex(&b)),
        "team" => sha256_hex(&app.call_on(relay, "GET", "/api/admin/team", Vec::new(), "application/json").await?),
        _ => String::new(),
    })
}

/// Stores `doc` as a proposal on every relay, each with the hash of what that relay serves now.
#[tauri::command]
pub async fn proposal_put(app: State<'_, App>, kind: String, doc: Value, note: String) -> R<Vec<RelayResult>> {
    put(&app, &kind, &doc, &note).await
}

pub(crate) async fn put(app: &App, kind: &str, doc: &Value, note: &str) -> R<Vec<RelayResult>> {
    kind_ok(kind)?;
    let servers = app.servers();
    let path = format!("/api/admin/proposals/{kind}");
    let path = &path;
    let got = futures_util::future::join_all(servers.iter().map(|sv| async move {
        let base = base_on(app, sv, kind).await?;
        let body = serde_json::to_vec(&json!({"base": base, "note": note, "doc": doc})).map_err(|e| e.to_string())?;
        let out = app.call_on(sv, "PUT", path, body, "application/json").await?;
        Ok::<Value, String>(serde_json::from_slice(&out).unwrap_or(Value::Null))
    }))
    .await;
    let out: Vec<RelayResult> = servers.iter().zip(got).map(|(sv, r)| RelayResult::of(sv, r)).collect();
    match crate::none_ok(&out) {
        Some(e) => Err(e),
        None => Ok(out),
    }
}

/// One proposal as the relays hold it, merged: `ids` per relay; `stale` per relay that names what
/// it serves now (`live`) when that is not what the proposal was based on.
fn merge(answers: Vec<(String, Value)>) -> Vec<Value> {
    let mut by: BTreeMap<String, Value> = BTreeMap::new();
    for (relay, a) in answers {
        let live = a["live"].as_str().map(str::to_owned);
        for p in a["proposals"].as_array().into_iter().flatten() {
            let key = format!("{}|{}|{}|{}", p["author"]["key_fingerprint"], p["author"]["oid"], p["note"], sha256_hex(p["doc"].to_string().as_bytes()));
            let e = by.entry(key).or_insert_with(|| {
                let mut v = p.clone();
                v["ids"] = json!({});
                v["stale"] = json!([]);
                v["relays"] = json!([]);
                v
            });
            e["ids"][&relay] = p["id"].clone();
            e["relays"].as_array_mut().unwrap().push(json!(relay));
            e["created"] = json!(e["created"].as_u64().min(p["created"].as_u64()));
            if let Some(l) = &live {
                if p["base"].as_str() != Some(l.as_str()) {
                    e["stale"].as_array_mut().unwrap().push(json!(relay));
                }
            }
        }
    }
    let mut v: Vec<Value> = by.into_values().collect();
    v.sort_by_key(|p| p["created"].as_u64());
    v
}

/// The proposals of `kind` on every relay, merged (`proposals`), and the relays that failed.
#[tauri::command]
pub async fn proposals_list(app: State<'_, App>, kind: String) -> R<Value> {
    list(&app, &kind).await
}

pub(crate) async fn list(app: &App, kind: &str) -> R<Value> {
    kind_ok(kind)?;
    let servers = app.servers();
    let path = format!("/api/admin/proposals/{kind}");
    let got = futures_util::future::join_all(servers.iter().map(|sv| app.call_on(sv, "GET", &path, Vec::new(), "application/json"))).await;
    let (mut answers, mut failed) = (Vec::new(), Vec::new());
    for (sv, r) in servers.into_iter().zip(got) {
        match r.and_then(|b| serde_json::from_slice::<Value>(&b).map_err(|_| "unexpected proposal list".to_owned())) {
            Ok(v) => answers.push((sv, v)),
            Err(e) => failed.push(format!("{sv}: {e}")),
        }
    }
    if answers.is_empty() && !failed.is_empty() {
        return Err(failed.join("; "));
    }
    Ok(json!({"proposals": merge(answers), "failed": failed}))
}

/// Removes a proposal from every relay that holds it (`ids`: relay → its id there). While someone
/// is signed in, with their session key (not the vault's): the relay's `proposal-delete` record then
/// names the key holder who published it and whether they proposed it themselves (`same_person`).
#[tauri::command]
pub async fn proposal_delete(app: State<'_, App>, kind: String, ids: BTreeMap<String, String>) -> R<Vec<RelayResult>> {
    delete(&app, &kind, &ids, crate::oidc::signed_in()).await
}

pub(crate) async fn delete(app: &App, kind: &str, ids: &BTreeMap<String, String>, signed_in: bool) -> R<Vec<RelayResult>> {
    kind_ok(kind)?;
    let key = if signed_in { crate::signin::SESSION_KEY.clone() } else { app.request_key()? };
    let servers = app.servers();
    let todo: Vec<(&String, &String)> = ids.iter().filter(|(r, id)| servers.contains(r) && id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit())).collect();
    let paths: Vec<String> = todo.iter().map(|(_, id)| format!("/api/admin/proposals/{kind}/{id}")).collect();
    let got = futures_util::future::join_all(todo.iter().zip(&paths).map(|((sv, _), p)| crate::api::admin(sv, &key, "DELETE", p, Vec::new(), "application/json"))).await;
    Ok(todo.iter().zip(got).map(|((sv, _), r)| RelayResult::of(sv, r.map(|_| Value::Null))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_on_several_relays_are_one_proposal_and_stale_where_live_moved() {
        let p = |id: &str, base: &str, created: u64| json!({"id": id, "kind": "policy", "author": {"key_fingerprint": "aa", "name": "Ed", "oid": "o1"}, "created": created, "base": base, "note": "n", "serial": 3, "doc": {"serial": 3}});
        let other = json!({"id": "c", "kind": "policy", "author": {"key_fingerprint": "bb", "name": "Flo", "oid": "o2"}, "created": 5, "base": "h1", "note": "n", "doc": {"serial": 3}});
        let merged = merge(vec![
            ("https://a".into(), json!({"live": "h1", "proposals": [p("a1", "h1", 10), other]})),
            ("https://b".into(), json!({"live": "h2", "proposals": [p("b1", "h1", 9)]})),
        ]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0]["author"]["name"], "Flo");
        let m = &merged[1];
        assert_eq!((m["ids"].clone(), m["created"].clone(), m["stale"].clone()), (json!({"https://a": "a1", "https://b": "b1"}), json!(9), json!(["https://b"])));
        assert_eq!(m["relays"], json!(["https://a", "https://b"]));
    }
}

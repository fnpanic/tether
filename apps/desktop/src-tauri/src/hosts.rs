//! Advanced mode (docs/advanced.md): the host inventory window's backend. Every peer the core
//! remembers, the technician's own host store (tags, notes, saved searches; the webview owns the
//! schema), batched online status from hbbs, and the list of open session windows.

use hbb_common::{
    config::{Config, PeerConfig, CONNECT_TIMEOUT, READ_TIMEOUT},
    rendezvous_proto::*,
    socket_client::connect_tcp,
};
use serde_json::{json, Value};
use std::{path::PathBuf, time::UNIX_EPOCH};

type R<T> = Result<T, String>;

/// The store is the technician's own data; 16 MiB holds ~50k hosts with tags and notes.
pub const STORE_MAX: usize = 16 * 1024 * 1024;
/// hbbs answers one bit per ID; upstream sends whole lists, we cap a request at this many.
pub const ONLINE_BATCH: usize = 1000;
pub const ONLINE_MAX: usize = 10_000;

fn store_path() -> PathBuf {
    Config::path("hosts.json")
}

/// Every peer the core has a config for (not only the 24 the Home page shows), newest first.
pub fn peers() -> Vec<Value> {
    PeerConfig::peers(None)
        .into_iter()
        .map(|(id, t, p)| {
            json!({
                "id": id,
                "hostname": p.info.hostname,
                "username": p.info.username,
                "platform": p.info.platform,
                "last": t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
            })
        })
        .collect()
}

pub fn load_store_from(path: &PathBuf) -> R<String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("The host list could not be read: {e}")),
    }
}

/// Validated before it is written: a JSON object of at most `STORE_MAX` bytes, written atomically
/// (temp file + rename), so a crash mid-save keeps the previous version.
pub fn save_store_to(path: &PathBuf, doc: &str) -> R<()> {
    if doc.len() > STORE_MAX {
        return Err("The host list is too large to save (16 MB).".into());
    }
    match serde_json::from_str::<Value>(doc) {
        Ok(Value::Object(_)) => {}
        _ => return Err("invalid host list".into()),
    }
    crate::policy::write_atomic(path, doc.as_bytes()).map_err(|e| format!("The host list could not be saved: {e}"))
}

pub fn load_store() -> R<String> {
    load_store_from(&store_path())
}

pub fn save_store(doc: &str) -> R<()> {
    save_store_to(&store_path(), doc)
}

/// Export file names end in `.json` or `.csv`; anything else is refused.
pub fn export_ext(name: &str) -> R<&'static str> {
    let n = name.to_ascii_lowercase();
    if n.contains(['/', '\\']) || n.len() > 128 {
        return Err("bad file name".into());
    }
    if n.ends_with(".json") {
        Ok("json")
    } else if n.ends_with(".csv") {
        Ok("csv")
    } else {
        Err("exports are .json or .csv".into())
    }
}

/// An import file, at most `STORE_MAX` bytes, as text.
pub fn read_import(p: &std::path::Path) -> R<String> {
    let len = std::fs::metadata(p).map_err(|e| format!("The file could not be read: {e}"))?.len();
    if len > STORE_MAX as u64 {
        return Err("The file is too large (16 MB).".into());
    }
    let raw = std::fs::read(p).map_err(|e| format!("The file could not be read: {e}"))?;
    String::from_utf8(raw).map_err(|_| "The file is not UTF-8 text.".into())
}

/// IDs as hbbs knows them: validated, spaces removed, deduplicated, capped.
pub fn clean_ids(ids: Vec<String>) -> R<Vec<String>> {
    if ids.len() > ONLINE_MAX {
        return Err(format!("at most {ONLINE_MAX} IDs per query"));
    }
    let mut out: Vec<String> = Vec::with_capacity(ids.len());
    let mut seen = std::collections::HashSet::new();
    for id in ids {
        let id = id.replace(' ', "");
        crate::security::check_peer_id(&id)?;
        if seen.insert(id.clone()) {
            out.push(id);
        }
    }
    Ok(out)
}

/// Bit `i` of `states` (most significant bit first, as hbbs writes it) → online IDs.
pub fn decode_states(ids: &[String], states: &[u8]) -> R<Vec<String>> {
    if states.len() < ids.len().div_ceil(8) {
        return Err("short online response".into());
    }
    Ok(ids
        .iter()
        .enumerate()
        .filter(|(i, _)| states[i / 8] & (0x80 >> (i % 8)) != 0)
        .map(|(_, id)| id.clone())
        .collect())
}

/// Where hbbs answers online queries: the TCP port below the rendezvous port (21115), which
/// `connect_tcp` maps to the `/ws/id` WebSocket on 443-only networks. A `ws(s)://` server is used as is.
pub fn online_endpoint(rendezvous: &str) -> R<String> {
    if rendezvous.starts_with("ws://") || rendezvous.starts_with("wss://") {
        return Ok(rendezvous.to_owned());
    }
    let (host, port) = rendezvous.rsplit_once(':').ok_or("no rendezvous server")?;
    let port: u16 = port.parse().map_err(|_| "bad rendezvous port")?;
    if host.is_empty() || port < 2 {
        return Err("bad rendezvous server".into());
    }
    Ok(format!("{host}:{}", port - 1))
}

async fn online_batch(endpoint: &str, ids: &[String]) -> R<Vec<String>> {
    let mut msg = RendezvousMessage::new();
    msg.set_online_request(OnlineRequest { id: Config::get_id(), peers: ids.to_vec(), ..Default::default() });
    let mut s = connect_tcp(endpoint, CONNECT_TIMEOUT).await.map_err(|e| format!("offline: {e}"))?;
    s.send(&msg).await.map_err(|e| format!("offline: {e}"))?;
    match librustdesk::common::get_next_nonkeyexchange_msg(&mut s, Some(3_000)).await.and_then(|m| m.union) {
        Some(rendezvous_message::Union::OnlineResponse(r)) => decode_states(ids, &r.states),
        // hbbs before Tether 0.5.22 answers online queries only on TCP 21115, not on /ws/id.
        _ => Err("unsupported: the server does not report online status".into()),
    }
}

/// Online IDs among `ids`, queried in batches of `ONLINE_BATCH` over one connection each.
pub async fn online(ids: Vec<String>) -> R<Vec<String>> {
    crate::policy::check_outgoing()?;
    let ids = clean_ids(ids)?;
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let (server, _, _) = librustdesk::common::get_rendezvous_server(READ_TIMEOUT).await;
    let endpoint = online_endpoint(&server)?;
    let mut on = Vec::new();
    for chunk in ids.chunks(ONLINE_BATCH) {
        on.extend(online_batch(&endpoint, chunk).await?);
    }
    Ok(on)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_are_read_most_significant_bit_first() {
        let ids: Vec<String> = (0..10).map(|i| i.to_string()).collect();
        // 1000_0001 0100_0000 → 0, 7, 9
        assert_eq!(decode_states(&ids, &[0x81, 0x40]).unwrap(), ["0", "7", "9"]);
        assert!(decode_states(&ids, &[0xff]).is_err(), "10 IDs need two bytes");
        assert!(decode_states(&[], &[]).unwrap().is_empty());
    }

    #[test]
    fn ids_are_cleaned_and_bounded() {
        assert_eq!(clean_ids(vec!["123 456 789".into(), "123456789".into(), "abc".into()]).unwrap(), ["123456789", "abc"]);
        assert!(clean_ids(vec!["12/34".into()]).is_err());
        assert!(clean_ids(vec!["1".into(); ONLINE_MAX + 1]).is_err());
    }

    #[test]
    fn online_port_is_below_the_rendezvous_port() {
        assert_eq!(online_endpoint("relay.example:21116").unwrap(), "relay.example:21115");
        assert_eq!(online_endpoint("[::1]:21116").unwrap(), "[::1]:21115");
        assert_eq!(online_endpoint("wss://k8s.example/ws/id/1").unwrap(), "wss://k8s.example/ws/id/1");
        assert!(online_endpoint("relay.example").is_err());
        assert!(online_endpoint(":21116").is_err());
    }

    #[test]
    fn exports_are_json_or_csv() {
        assert_eq!(export_ext("hosts.JSON").unwrap(), "json");
        assert_eq!(export_ext("tether-hosts.csv").unwrap(), "csv");
        assert!(export_ext("hosts.exe").is_err());
        assert!(export_ext("../x.csv").is_err());
        assert!(export_ext("a\\b.csv").is_err());
    }

    #[test]
    fn store_round_trips_and_refuses_bad_documents() {
        let dir = std::env::temp_dir().join(format!("tether-hosts-{}", std::process::id()));
        let p = dir.join("hosts.json");
        assert_eq!(load_store_from(&p).unwrap(), "", "missing file = empty store");
        save_store_to(&p, r#"{"v":1,"hosts":{}}"#).unwrap();
        assert_eq!(load_store_from(&p).unwrap(), r#"{"v":1,"hosts":{}}"#);
        assert!(save_store_to(&p, "[1,2]").is_err(), "not an object");
        assert!(save_store_to(&p, "{").is_err());
        let big = format!(r#"{{"x":"{}"}}"#, "a".repeat(STORE_MAX));
        assert!(save_store_to(&p, &big).is_err());
        assert_eq!(load_store_from(&p).unwrap(), r#"{"v":1,"hosts":{}}"#, "a refused save keeps the old file");
        let _ = std::fs::remove_dir_all(dir);
    }
}

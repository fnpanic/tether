//! Pure helpers behind the publish flow: what changed between the live policy and the draft (in
//! plain words), the live effective values per profile with their source, relays the policy knows
//! but Tether Manage does not publish to, and the device list merged over every relay.

use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use tether_trust::policy::{self, Bundle, Feature, Kind, Layer, Rule, State};

#[derive(Serialize, Debug, PartialEq)]
pub struct Change {
    /// "Technician profile", "Customer profile", "Group <name>" or "Devices".
    pub scope: String,
    /// Feature label (English), when the change is about one feature.
    pub label: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    /// The whole change as one sentence.
    pub text: String,
}

fn shown(f: Option<&Feature>, v: &str) -> String {
    match (f.map(|f| f.kind), v) {
        (Some(Kind::Bool) | None, "Y") => "on".into(),
        (Some(Kind::Bool) | None, "N") => "off".into(),
        (_, "") => "empty".into(),
        _ => v.into(),
    }
}

/// A rule in plain words: "default", "on (best)", "locked to click", "off", "hidden".
pub fn rule_words(f: Option<&Feature>, r: Option<&Rule>) -> String {
    let Some(r) = r else { return "default".into() };
    let bool_ = f.map_or(false, |f| f.kind == Kind::Bool);
    let v = r.value.clone().unwrap_or_else(|| if bool_ { "Y".into() } else { f.map_or("", |f| f.default).into() });
    match r.state {
        State::On if bool_ && v == "Y" => "on".into(),
        State::On => format!("on ({}, user may change)", shown(f, &v)),
        State::Locked if bool_ => format!("locked {}", shown(f, &v)),
        State::Locked => format!("locked to {}", shown(f, &v)),
        State::Off => "off".into(),
        State::Hidden => "hidden".into(),
    }
}

fn layer_changes(scope: &str, from: &Layer, to: &Layer, out: &mut Vec<Change>) {
    let ids: BTreeSet<&String> = from.keys().chain(to.keys()).collect();
    for id in ids {
        let (a, b) = (from.get(id), to.get(id));
        if a == b {
            continue;
        }
        let f = policy::feature(id);
        let label = f.map_or(id.as_str(), |f| f.en).to_owned();
        let (fw, tw) = (rule_words(f, a), rule_words(f, b));
        out.push(Change { scope: scope.into(), text: format!("{scope}: {label}: {fw} → {tw}"), label: Some(label), from: Some(fw), to: Some(tw) });
    }
}

fn profile_scope(p: &str) -> String {
    match p {
        policy::PROFILE_TECHNICIAN => "Technician profile".into(),
        policy::PROFILE_CUSTOMER => "Customer profile".into(),
        _ => format!("Profile {p}"),
    }
}

fn short(uuid: &str) -> &str {
    uuid.get(..8).unwrap_or(uuid)
}

/// Everything that differs between `live` (what devices get now; `None` = nothing published) and
/// `draft`. Serial and issue time are ignored.
pub fn changes(live: Option<&Bundle>, draft: &Bundle) -> Vec<Change> {
    let empty = Bundle::default();
    let live = live.unwrap_or(&empty);
    let none = Layer::new();
    let mut out = Vec::new();
    let profiles: BTreeSet<&String> = live.profiles.keys().chain(draft.profiles.keys()).collect();
    // Technician first, then customer, then anything else.
    let mut profiles: Vec<&String> = profiles.into_iter().collect();
    profiles.sort_by_key(|p| (p.as_str() != policy::PROFILE_TECHNICIAN, p.as_str() != policy::PROFILE_CUSTOMER, p.as_str().to_owned()));
    for p in profiles {
        layer_changes(&profile_scope(p), live.profiles.get(p).unwrap_or(&none), draft.profiles.get(p).unwrap_or(&none), &mut out);
    }
    let groups: BTreeSet<&String> = live.groups.keys().chain(draft.groups.keys()).collect();
    for g in groups {
        let scope = format!("Group {g}");
        match (live.groups.get(g), draft.groups.get(g)) {
            (None, Some(d)) => {
                out.push(Change { scope: scope.clone(), label: None, from: None, to: None, text: format!("{scope} created for {} devices", d.profile) });
                layer_changes(&scope, &none, &d.overrides, &mut out);
            }
            (Some(_), None) => out.push(Change { scope: scope.clone(), label: None, from: None, to: None, text: format!("{scope} deleted") }),
            (Some(l), Some(d)) => {
                if l.profile != d.profile {
                    out.push(Change { scope: scope.clone(), label: None, from: Some(l.profile.clone()), to: Some(d.profile.clone()), text: format!("{scope} now targets {} devices (was {})", d.profile, l.profile) });
                }
                layer_changes(&scope, &l.overrides, &d.overrides, &mut out);
            }
            (None, None) => {}
        }
    }
    let devices: BTreeSet<&String> = live.assignments.keys().chain(draft.assignments.keys()).collect();
    for d in devices {
        let (a, b) = (live.assignments.get(d), draft.assignments.get(d));
        let text = match (a, b) {
            (None, Some(g)) => format!("Device {} added to group {g}", short(d)),
            (Some(g), None) => format!("Device {} removed from group {g}", short(d)),
            (Some(x), Some(y)) if x != y => format!("Device {} moved from group {x} to {y}", short(d)),
            _ => continue,
        };
        out.push(Change { scope: "Devices".into(), label: None, from: a.cloned(), to: b.cloned(), text });
    }
    out
}

#[derive(Serialize, Debug, PartialEq)]
pub struct EffRow {
    pub id: String,
    pub value: String,
    pub locked: bool,
    pub hidden: bool,
    /// "default" or "profile": which layer decides for a device without a group.
    pub source: &'static str,
}

/// What a device of `profile` without a group gets from `bundle` (`policy::resolve`), per feature.
pub fn effective_rows(bundle: Option<&Bundle>, profile: &str) -> Vec<EffRow> {
    let eff = policy::resolve(bundle, profile, "", None);
    let layer = bundle.and_then(|b| b.profiles.get(profile));
    eff.values
        .iter()
        .map(|(id, v)| EffRow {
            id: id.clone(),
            value: v.clone(),
            locked: eff.locked.contains(id),
            hidden: eff.hidden.contains(id),
            source: if layer.map_or(false, |l| l.contains_key(id)) { "profile" } else { "default" },
        })
        .collect()
}

/// Host of a `https://host[:port][/…]` relay address.
fn https_host(s: &str) -> String {
    let s = s.strip_prefix("https://").unwrap_or(s);
    s.split(['/', ':']).next().unwrap_or(s).to_ascii_lowercase()
}

/// Relay hosts named in a policy's `servers.list` (any profile or group, on or locked) that are not
/// in Tether Manage's own relay list: devices may use them, but publishes do not reach them.
pub fn unconfigured_hosts(servers: &[String], bundles: &[&Bundle]) -> Vec<String> {
    let mine: BTreeSet<String> = servers.iter().map(|s| https_host(s)).collect();
    let mut out = BTreeSet::new();
    for b in bundles {
        let layers = b.profiles.values().chain(b.groups.values().map(|g| &g.overrides));
        for l in layers {
            let Some(r) = l.get(policy::SERVERS_LIST) else { continue };
            if !matches!(r.state, State::On | State::Locked) {
                continue;
            }
            for e in policy::parse_servers(r.value.as_deref().unwrap_or("")).unwrap_or_default() {
                let h = policy::server_host(&e).to_ascii_lowercase();
                if !mine.contains(&h) {
                    out.insert(h);
                }
            }
        }
    }
    out.into_iter().collect()
}

/// Device lists of several relays merged by `uuid`, newest `last_seen` wins (a client heartbeats to
/// whichever relay it uses; docs/relay-ha.md).
pub fn merge_devices(lists: impl IntoIterator<Item = Vec<Value>>) -> Vec<Value> {
    let mut by: BTreeMap<String, Value> = BTreeMap::new();
    for d in lists.into_iter().flatten() {
        let Some(uuid) = d["uuid"].as_str().map(str::to_owned) else { continue };
        let seen = d["last_seen"].as_u64().unwrap_or(0);
        if by.get(&uuid).map_or(true, |o| o["last_seen"].as_u64().unwrap_or(0) < seen) {
            by.insert(uuid, d);
        }
    }
    by.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tether_trust::policy::Group;

    fn rule(state: State, v: Option<&str>) -> Rule {
        Rule { state, value: v.map(Into::into) }
    }

    #[test]
    fn changes_in_plain_words() {
        let mut live = Bundle { serial: 4, ..Default::default() };
        live.profiles.insert("technician".into(), Layer::from([("permission.clipboard".into(), rule(State::Off, None))]));
        live.groups.insert("old".into(), Group { profile: "technician".into(), overrides: Layer::new() });
        live.assignments.insert("aaaaaaaa-1111".into(), "old".into());
        let mut draft = Bundle { serial: 9, issued_at: 5, ..Default::default() };
        draft.profiles.insert(
            "technician".into(),
            Layer::from([
                ("permission.clipboard".into(), rule(State::On, None)),
                ("video.quality".into(), rule(State::Locked, Some("best"))),
            ]),
        );
        draft.profiles.insert("customer".into(), Layer::from([("security.approve-mode".into(), rule(State::On, Some("click")))]));
        draft.groups.insert("desk".into(), Group { profile: "customer".into(), overrides: Layer::from([("connect.chat".into(), rule(State::Hidden, None))]) });
        draft.assignments.insert("aaaaaaaa-1111".into(), "desk".into());
        let texts: Vec<String> = changes(Some(&live), &draft).into_iter().map(|c| c.text).collect();
        assert_eq!(
            texts,
            [
                "Technician profile: Allow clipboard sync: off → on",
                "Technician profile: Default image quality: default → locked to best",
                "Customer profile: How incoming sessions are approved: default → on (click, user may change)",
                "Group desk created for customer devices",
                "Group desk: Chat in sessions: default → hidden",
                "Group old deleted",
                "Device aaaaaaaa moved from group old to desk",
            ]
        );
        // Serial and time alone are no change; nothing published = compare with an empty bundle.
        assert!(changes(Some(&draft), &Bundle { serial: 1, ..draft.clone() }).is_empty());
        assert_eq!(changes(None, &draft).len(), 6);
        assert!(changes(None, &Bundle::default()).is_empty());
    }

    #[test]
    fn effective_rows_have_sources() {
        let mut b = Bundle::default();
        b.profiles.insert("customer".into(), Layer::from([("connect.chat".into(), rule(State::Hidden, None))]));
        let rows = effective_rows(Some(&b), "customer");
        let chat = rows.iter().find(|r| r.id == "connect.chat").unwrap();
        assert_eq!((chat.value.as_str(), chat.locked, chat.hidden, chat.source), ("N", true, true, "profile"));
        let t = rows.iter().find(|r| r.id == "customer.title").unwrap();
        assert_eq!((t.value.as_str(), t.locked, t.source), ("Tether Support", false, "default"));
        // Technician-only features are not part of the customer view.
        assert!(rows.iter().all(|r| r.id != "recording.mode"));
        assert!(effective_rows(None, "technician").iter().all(|r| r.source == "default"));
    }

    #[test]
    fn relays_known_to_the_policy_but_not_configured() {
        let mut b = Bundle::default();
        b.profiles.insert("technician".into(), Layer::from([(policy::SERVERS_LIST.into(), rule(State::Locked, Some("tether-desk.duckdns.org, Tether-Desk2.duckdns.org")))]));
        b.groups.insert("g".into(), Group { profile: "customer".into(), overrides: Layer::from([(policy::SERVERS_LIST.into(), rule(State::On, Some("wss://relay3.example.com/ws/id")))]) });
        let one = vec!["https://tether-desk.duckdns.org".to_owned()];
        assert_eq!(unconfigured_hosts(&one, &[&b]), ["relay3.example.com", "tether-desk2.duckdns.org"]);
        let all = vec!["https://tether-desk.duckdns.org".into(), "https://tether-desk2.duckdns.org/".into(), "https://relay3.example.com:443".into()];
        assert!(unconfigured_hosts(&all, &[&b]).is_empty());
        // An off/hidden server list adds nothing.
        b.profiles.get_mut("technician").unwrap().get_mut(policy::SERVERS_LIST).unwrap().state = State::Off;
        b.groups.clear();
        assert!(unconfigured_hosts(&one, &[&b]).is_empty());
    }

    #[test]
    fn devices_merge_by_uuid_newest_wins() {
        let a = vec![json!({"uuid":"u1","last_seen":10,"serial":3}), json!({"uuid":"u2","last_seen":50})];
        let b = vec![json!({"uuid":"u1","last_seen":20,"serial":4}), json!({"last_seen":99})];
        let m = merge_devices([a, b]);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0]["serial"], 4);
        assert_eq!(m[1]["uuid"], "u2");
    }
}

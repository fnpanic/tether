//! Enrolment grants for agents Tether installs (docs/management.md, "Health monitoring agent"):
//! an admin-signed enrolment token with `tether_proof` that Manage puts into a policy layer's
//! `health.enrol-grant`. The token lives only in the policy, where it is public by design; the
//! journal `health-grants.json` keeps id, label, layer, customer, expiry and state, never the token.

use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use tether_trust::policy::{Bundle, Layer, State};
use tether_trust::status::{self, TokenPayload};
use tether_trust::{envelope, SigningKey, VerifyingKey};

pub const AGENT: &str = "health.agent";
pub const GRANT: &str = "health.enrol-grant";
pub const JOURNAL: &str = "health-grants.json";
const LIFETIME: u64 = 365 * 86_400;
/// A grant with fewer days left is offered for renewal.
const RENEW_DAYS: u64 = 60;
/// What `health.enrol-grant` takes (`Bundle::validate`).
const MAX_LEN: usize = 1024;

/// Journal reads and writes (the revocations run between two of them, without the lock).
pub static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GrantState {
    Active,
    /// Renewed: revoked on a relay once that relay serves a policy without it.
    Replaced,
    /// Revoked by the administrator: revoked on every relay at once.
    Revoked,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: String,
    pub label: String,
    /// `profile:<name>` or `group:<name>`.
    pub layer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub customer: Option<String>,
    pub created: u64,
    pub exp: u64,
    pub state: GrantState,
    /// Relays that confirmed the revocation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub revoked_on: Vec<String>,
    /// Replaced: the relays not known to serve a policy without it at the last check.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub waiting: Vec<String>,
}

pub type Journal = Vec<Entry>;

/// The journal file's content (`None`: no file yet). A damaged file is an error, not an empty
/// journal: it holds the revocations still to do.
pub fn parse_journal(raw: Option<&[u8]>) -> Result<Journal, String> {
    raw.map_or(Ok(Vec::new()), |b| serde_json::from_slice(b).map_err(|e| format!("{JOURNAL} in the data folder is damaged ({e}); repair or remove it")))
}

/// Every layer of a bundle as (`profile:<name>` | `group:<name>`, rules).
pub fn layers(b: &Bundle) -> Vec<(String, &Layer)> {
    b.profiles.iter().map(|(p, l)| (format!("profile:{p}"), l)).chain(b.groups.iter().map(|(n, g)| (format!("group:{n}"), &g.overrides))).collect()
}

/// A layer key as the editor names it.
pub fn shown(key: &str) -> String {
    match key.split_once(':') {
        Some(("profile", "technician")) => "Technician profile".into(),
        Some(("profile", "customer")) => "Customer profile".into(),
        Some(("group", g)) => format!("Group {g}"),
        _ => key.into(),
    }
}

/// A valid layer key (what the editor sends).
pub fn valid_layer(key: &str) -> bool {
    matches!(key.split_once(':'), Some(("profile", "technician" | "customer"))) || key.strip_prefix("group:").is_some_and(|g| (1..=64).contains(&g.len()))
}

fn value<'a>(layer: &'a Layer, id: &str) -> Option<&'a str> {
    layer.get(id).filter(|r| matches!(r.state, State::On | State::Locked)).and_then(|r| r.value.as_deref())
}

/// `health.agent` of a layer when it needs a grant (`on-request`, `install`); `manual`, `remove`,
/// off and unset need none.
pub fn needs_grant(layer: &Layer) -> Option<&str> {
    value(layer, AGENT).filter(|v| matches!(*v, "on-request" | "install"))
}

/// The grant token a layer carries.
pub fn grant_in(layer: &Layer) -> Option<&str> {
    value(layer, GRANT).filter(|v| !v.is_empty())
}

/// A token's payload without checking it (id and expiry of an expired or foreign grant).
pub fn peek(token: &str) -> Option<TokenPayload> {
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(token.trim()).ok()?;
    let doc: envelope::Signed = serde_json::from_slice(&raw).ok()?;
    serde_json::from_slice(&base64::engine::general_purpose::STANDARD.decode(doc.payload).ok()?).ok()
}

/// What the editor shows for a grant.
#[derive(Serialize, Debug, PartialEq)]
pub struct Info {
    /// `valid`, `replaced` (renewed, the newer grant not on every relay yet: still valid everywhere),
    /// `expired`, `revoked`, `foreign` (not signed by this app's admin key) or `invalid`.
    pub status: &'static str,
    pub id: Option<String>,
    pub exp: Option<u64>,
    pub customer: Option<String>,
    /// In this PC's journal.
    pub made_here: bool,
    /// Valid with fewer than 60 days left.
    pub renew: bool,
    pub why: Option<String>,
}

pub fn info(token: &str, keys: &[VerifyingKey], journal: &[Entry], now: u64) -> Info {
    let p = peek(token);
    let entry = p.as_ref().and_then(|p| journal.iter().find(|e| e.id == p.id));
    let mut i = Info { status: "valid", id: p.as_ref().map(|p| p.id.clone()), exp: p.as_ref().map(|p| p.exp), customer: p.and_then(|p| p.customer), made_here: entry.is_some(), renew: false, why: None };
    match status::verify_token(token, keys, now) {
        Err(e) if e == "expired" => i.status = "expired",
        Err(e) if e.contains("untrusted") => i.status = "foreign",
        Err(e) => (i.status, i.why) = ("invalid", Some(e)),
        Ok(p) if !p.tether_proof => (i.status, i.why) = ("invalid", Some("an ordinary enrolment token, not a grant for agents Tether installs".into())),
        Ok(_) if entry.is_some_and(|e| e.state == GrantState::Replaced && e.revoked_on.is_empty()) => i.status = "replaced",
        Ok(_) if entry.is_some_and(|e| e.state != GrantState::Active) => i.status = "revoked",
        Ok(p) => i.renew = p.exp - now < RENEW_DAYS * 86_400,
    }
    i
}

/// The two health features as clients resolve them (`policy::resolve_with`, per feature, later
/// wins): each profile alone, and each group over its profile.
pub fn resolved(b: &Bundle) -> Vec<(String, Layer)> {
    let pick = |l: Option<&Layer>| -> Layer { l.into_iter().flatten().filter(|(id, _)| *id == AGENT || *id == GRANT).map(|(k, r)| (k.clone(), r.clone())).collect() };
    let groups = b.groups.iter().map(|(n, g)| {
        let mut l = pick(b.profiles.get(&g.profile));
        l.extend(pick(Some(&g.overrides)));
        (format!("group:{n}"), l)
    });
    b.profiles.iter().map(|(p, l)| (format!("profile:{p}"), pick(Some(l)))).chain(groups).collect()
}

/// Why `b` cannot be published: a profile, or a group over its profile, that resolves to a
/// `health.agent` needing a grant and to no grant that verifies with `keys`, is unexpired and not
/// revoked here.
pub fn check(b: &Bundle, keys: &[VerifyingKey], journal: &[Entry], now: u64) -> Result<(), String> {
    for (key, layer) in resolved(b) {
        let Some(agent) = needs_grant(&layer) else { continue };
        let why = match grant_in(&layer).map(|t| info(t, keys, journal, now)) {
            None => "it has no enrolment grant".to_owned(),
            Some(i) => match i.status {
                "valid" | "replaced" => continue,
                "expired" => "its enrolment grant has expired".to_owned(),
                "revoked" => "its enrolment grant was revoked".to_owned(),
                "foreign" => "its enrolment grant does not verify with this app's admin key (the admin key was changed, or another key made it)".to_owned(),
                _ => format!("its enrolment grant is not valid ({})", i.why.unwrap_or_default()),
            },
        };
        let place = if key.starts_with("group:") { "this group or its profile" } else { "this profile" };
        return Err(format!("{}: health.agent is {agent}, but {why}. Create or renew the grant in {place} (Health monitoring), or set health.agent to manual or remove.", shown(&key)));
    }
    Ok(())
}

/// A new grant for `layer` (365 days, `tether_proof`, label `policy:<name>`) and its journal entry.
pub fn create(layer: &str, customer: &str, admin: &SigningKey, now: u64) -> Result<(String, Entry), String> {
    if !valid_layer(layer) {
        return Err("invalid layer".into());
    }
    let customer = Some(customer.trim()).filter(|c| !c.is_empty());
    if customer.is_some_and(|c| !status::valid_customer(c)) {
        return Err("Customer reference: 1 to 32 letters, digits, dots, underscores or dashes.".into());
    }
    let name = layer.split_once(':').map_or(layer, |(_, n)| n);
    // The token's label takes at most 64 characters and no control characters; the journal keeps the full layer.
    let label: String = format!("policy:{name}").chars().filter(|c| !c.is_control()).take(64).collect();
    let exp = now + LIFETIME;
    let (token, id) = status::make_token(&label, exp, None, customer, true, admin)?;
    fits(&token)?;
    Ok((token, Entry { id, label, layer: layer.into(), customer: customer.map(Into::into), created: now, exp, state: GrantState::Active, revoked_on: Vec::new(), waiting: Vec::new() }))
}

fn fits(token: &str) -> Result<(), String> {
    if token.len() > MAX_LEN {
        return Err(format!("The grant would be {} characters long, but health.enrol-grant takes at most {MAX_LEN}. Use a shorter customer reference or group name.", token.len()));
    }
    Ok(())
}

/// Mark the grant `token` as replaced or revoked (an entry is added for a grant made elsewhere).
/// False when the token cannot be read (nothing to revoke).
pub fn retire(journal: &mut Journal, token: &str, layer: &str, state: GrantState) -> bool {
    let Some(p) = peek(token) else { return false };
    match journal.iter_mut().find(|e| e.id == p.id) {
        // Revoked stays revoked (a later renew of the same token does not soften it).
        Some(e) => e.state = if e.state == GrantState::Revoked { e.state } else { state },
        None => journal.push(Entry { id: p.id, label: p.label, layer: layer.into(), customer: p.customer, created: 0, exp: p.exp, state, revoked_on: Vec::new(), waiting: Vec::new() }),
    }
    true
}

/// Grant ids in a bundle (any layer).
pub fn ids_in(b: &Bundle) -> BTreeSet<String> {
    layers(b).into_iter().filter_map(|(_, l)| grant_in(l)).filter_map(peek).map(|p| p.id).collect()
}

/// The revocations to send now, as (relay, id, exp). `served`: every relay of the list with the
/// grant ids in the policy it serves now (`None`: not known). A replaced grant is revoked nowhere
/// until every relay is known to serve a policy without it, then on all of them: clients read the
/// policy from any relay but enrol on every relay, so a machine with the old policy from one relay
/// must still enrol on the others. Its `waiting` keeps the relays still missing. A revoked grant goes
/// to every relay at once. Expired grants need no revocation.
pub fn targets(journal: &mut Journal, served: &[(String, Option<BTreeSet<String>>)], now: u64) -> Vec<(String, String, u64)> {
    let mut out = Vec::new();
    for e in journal.iter_mut().filter(|e| e.state != GrantState::Active && e.exp > now) {
        if e.state == GrantState::Replaced {
            e.waiting = served.iter().filter(|(_, ids)| ids.as_ref().map_or(true, |ids| ids.contains(&e.id))).map(|(sv, _)| sv.clone()).collect();
            // Once revoked on one relay, the rest follow whatever they serve.
            if e.revoked_on.is_empty() && !e.waiting.is_empty() {
                continue;
            }
        }
        for (sv, _) in served.iter().filter(|(sv, _)| !e.revoked_on.contains(sv)) {
            out.push((sv.clone(), e.id.clone(), e.exp));
        }
    }
    out
}

/// Revoke for a layer: `token` and every other grant journalled for that layer (after a Renew the
/// old one is the one live on the relays). False when there is nothing to revoke.
pub fn revoke_layer(journal: &mut Journal, token: &str, layer: &str) -> bool {
    let read = retire(journal, token, layer, GrantState::Revoked);
    let mut any = false;
    for e in journal.iter_mut().filter(|e| e.layer == layer) {
        e.state = GrantState::Revoked;
        any = true;
    }
    read || any
}

/// A draft that carries a replaced grant again (the renewed draft was discarded, an older version
/// loaded): while it is revoked nowhere it becomes current again, and the layer's other current
/// grant is the replaced one. True when the journal changed.
pub fn restore(journal: &mut Journal, b: &Bundle) -> bool {
    let mut changed = false;
    for (key, layer) in layers(b) {
        let Some(id) = grant_in(layer).and_then(peek).map(|p| p.id) else { continue };
        if !journal.iter().any(|e| e.id == id && e.state == GrantState::Replaced && e.revoked_on.is_empty()) {
            continue;
        }
        for e in journal.iter_mut().filter(|e| e.layer == key || e.id == id) {
            match e.state {
                GrantState::Replaced if e.id == id => e.state = GrantState::Active,
                GrantState::Active if e.id != id => e.state = GrantState::Replaced,
                _ => continue,
            }
            changed = true;
        }
    }
    changed
}

pub fn mark_revoked_on(journal: &mut Journal, server: &str, id: &str) {
    if let Some(e) = journal.iter_mut().find(|e| e.id == id && !e.revoked_on.iter().any(|s| s == server)) {
        e.revoked_on.push(server.into());
    }
}

/// Retired grants not yet revoked on every relay in `servers`, for the editor: id, layer, state,
/// the relays still to do and, for a replaced one, the relays not yet serving a policy without it.
pub fn pending(journal: &Journal, servers: &[String], now: u64) -> Vec<serde_json::Value> {
    journal
        .iter()
        .filter(|e| e.state != GrantState::Active && e.exp > now)
        .filter_map(|e| {
            let left: Vec<&String> = servers.iter().filter(|s| !e.revoked_on.contains(s)).collect();
            (!left.is_empty()).then(|| serde_json::json!({ "id": e.id, "layer": e.layer, "state": e.state, "relays": left, "waiting": e.waiting }))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tether_trust::policy::Rule;

    const NOW: u64 = 1_800_000_000;

    fn rule(v: &str) -> Rule {
        Rule { state: State::On, value: Some(v.into()) }
    }

    fn bundle(agent: Option<&str>, grant: Option<&str>) -> Bundle {
        let mut layer = Layer::new();
        if let Some(a) = agent {
            layer.insert(AGENT.into(), rule(a));
        }
        if let Some(g) = grant {
            layer.insert(GRANT.into(), rule(g));
        }
        let mut b = Bundle::default();
        b.groups.insert("monitored".into(), tether_trust::policy::Group { profile: "technician".into(), overrides: layer });
        b
    }

    #[test]
    fn a_grant_verifies_with_the_vault_key_and_carries_flag_and_customer() {
        let admin = tether_trust::generate_key();
        let (token, e) = create("group:monitored", " 4711 ", &admin, NOW).unwrap();
        let p = status::verify_token(&token, &[admin.verifying_key()], NOW).unwrap();
        assert!(p.tether_proof && p.tether_id.is_none());
        assert_eq!((p.customer.as_deref(), p.label.as_str(), p.exp), (Some("4711"), "policy:monitored", NOW + 365 * 86_400));
        assert_eq!((e.id, e.layer.as_str(), e.customer.as_deref(), e.exp, e.state), (p.id, "group:monitored", Some("4711"), p.exp, GrantState::Active));
        // A layer without a customer gives a grant without one.
        let (t, e) = create("profile:technician", "  ", &admin, NOW).unwrap();
        assert_eq!((status::verify_token(&t, &[admin.verifying_key()], NOW).unwrap().customer, e.customer.clone()), (None, None));
        // The journal never holds the token.
        let j = serde_json::to_string(&vec![e]).unwrap();
        assert!(!j.contains(&t) && !j.contains("token"), "{j}");
    }

    #[test]
    fn customer_layer_and_length_are_checked() {
        let admin = tether_trust::generate_key();
        assert!(create("group:monitored", "a/b", &admin, NOW).unwrap_err().starts_with("Customer reference"));
        assert!(create("group:monitored", &"c".repeat(33), &admin, NOW).unwrap_err().starts_with("Customer reference"));
        assert_eq!(create("profile:admin", "", &admin, NOW).unwrap_err(), "invalid layer");
        // A group name longer than a label takes is cut in the label; the largest grant still fits.
        let long = format!("group:{}", "g".repeat(64));
        let (t, e) = create(&long, &"c".repeat(32), &admin, NOW).unwrap();
        assert!(t.len() <= 1024 && e.label.chars().count() == 64 && e.layer == long, "{}", t.len());
        // A group name of 64 bytes in four-byte characters (the most a group name takes).
        let (t, _) = create(&format!("group:{}", "\u{1F600}".repeat(16)), &"c".repeat(32), &admin, NOW).unwrap();
        assert!(t.len() <= 1024, "{}", t.len());
        // Over 1024 characters: refused with what to do.
        let e = fits(&"x".repeat(1025)).unwrap_err();
        assert!(e.contains("1025") && e.contains("at most 1024") && e.contains("shorter customer reference"), "{e}");
        assert!(fits(&"x".repeat(1024)).is_ok());
    }

    #[test]
    fn publish_needs_a_valid_grant_for_on_request_and_install_only() {
        let admin = tether_trust::generate_key();
        let keys = [admin.verifying_key()];
        let (token, entry) = create("group:monitored", "", &admin, NOW).unwrap();
        let mut j = vec![entry];
        for a in ["on-request", "install"] {
            let e = check(&bundle(Some(a), None), &keys, &j, NOW).unwrap_err();
            assert!(e.starts_with(&format!("Group monitored: health.agent is {a}, but it has no enrolment grant.")) && e.contains("set health.agent to manual"), "{e}");
            check(&bundle(Some(a), Some(&token)), &keys, &j, NOW).unwrap();
        }
        for a in ["manual", "remove"] {
            check(&bundle(Some(a), None), &keys, &j, NOW).unwrap();
        }
        // Off: the off value is manual.
        let mut off = bundle(Some("install"), None);
        off.groups.get_mut("monitored").unwrap().overrides.insert(AGENT.into(), Rule { state: State::Off, value: None });
        check(&off, &keys, &j, NOW).unwrap();
        // Expired.
        let e = check(&bundle(Some("install"), Some(&token)), &keys, &j, NOW + 366 * 86_400).unwrap_err();
        assert!(e.contains("has expired"), "{e}");
        assert_eq!(info(&token, &keys, &j, NOW + 366 * 86_400).status, "expired");
        // After an admin key rotation it no longer verifies: say so (and Renew is offered).
        let other = [tether_trust::generate_key().verifying_key()];
        assert!(check(&bundle(Some("install"), Some(&token)), &other, &j, NOW).unwrap_err().contains("does not verify with this app's admin key"));
        assert_eq!(info(&token, &other, &j, NOW).status, "foreign");
        // An ordinary enrolment token or junk.
        let (plain, _) = status::make_token("x", NOW + 100, None, None, false, &admin).unwrap();
        assert!(check(&bundle(Some("install"), Some(&plain)), &keys, &j, NOW).unwrap_err().contains("not valid (an ordinary enrolment token"));
        assert_eq!(info("junk", &keys, &j, NOW).status, "invalid");
        // Revoked here.
        assert!(retire(&mut j, &token, "group:monitored", GrantState::Revoked));
        assert!(check(&bundle(Some("install"), Some(&token)), &keys, &j, NOW).unwrap_err().contains("was revoked"));
    }

    #[test]
    fn renew_is_offered_below_60_days() {
        let admin = tether_trust::generate_key();
        let keys = [admin.verifying_key()];
        let (token, entry) = create("group:monitored", "", &admin, NOW).unwrap();
        let j = vec![entry];
        let i = info(&token, &keys, &j, NOW);
        assert_eq!((i.status, i.renew, i.made_here, i.exp), ("valid", false, true, Some(NOW + 365 * 86_400)));
        assert!(info(&token, &keys, &j, NOW + 306 * 86_400).renew);
        assert!(!info(&token, &keys, &[], NOW).made_here);
    }

    #[test]
    fn a_renewed_grant_is_revoked_on_all_relays_or_none() {
        let admin = tether_trust::generate_key();
        let (old, e) = create("group:monitored", "", &admin, NOW).unwrap();
        let mut j = vec![e];
        let (new, e) = create("group:monitored", "", &admin, NOW).unwrap();
        assert!(retire(&mut j, &old, "group:monitored", GrantState::Replaced));
        j.push(e);
        let old_id = peek(&old).unwrap().id;
        let (with_old, with_new) = (ids_in(&bundle(Some("install"), Some(&old))), ids_in(&bundle(Some("install"), Some(&new))));
        // Fake relays: a serves the new policy, b still the old one, c did not answer: revoked nowhere,
        // since a machine with the old policy from b enrols on a too.
        let served = vec![("a".to_owned(), Some(with_new.clone())), ("b".to_owned(), Some(with_old)), ("c".to_owned(), None)];
        assert!(targets(&mut j, &served, NOW).is_empty());
        assert_eq!(j[0].waiting, ["b", "c"]);
        let p = pending(&j, &["a".into(), "b".into(), "c".into()], NOW);
        assert_eq!(p, [serde_json::json!({ "id": old_id, "layer": "group:monitored", "state": "replaced", "relays": ["a", "b", "c"], "waiting": ["b", "c"] })]);
        // Still valid meanwhile: shown as replaced, and a draft that carries it again can be published.
        assert_eq!(info(&old, &[admin.verifying_key()], &j, NOW).status, "replaced");
        check(&bundle(Some("install"), Some(&old)), &[admin.verifying_key()], &j, NOW).unwrap();
        // Every relay serves the new policy: revoked on all of them at once.
        let served: Vec<_> = ["a", "b", "c"].iter().map(|s| (s.to_string(), Some(with_new.clone()))).collect();
        assert_eq!(targets(&mut j, &served, NOW).iter().map(|t| (t.0.as_str(), t.1.as_str())).collect::<Vec<_>>(), [("a", old_id.as_str()), ("b", &old_id), ("c", &old_id)]);
        assert!(j[0].waiting.is_empty());
        // a confirmed it: from now on b may fall back to "not known" and still gets it (it is revoked somewhere already).
        mark_revoked_on(&mut j, "a", &old_id);
        assert_eq!(info(&old, &[admin.verifying_key()], &j, NOW).status, "revoked");
        let served = vec![("a".to_owned(), Some(with_new.clone())), ("b".to_owned(), None), ("c".to_owned(), Some(with_new))];
        assert_eq!(targets(&mut j, &served, NOW).iter().map(|t| t.0.as_str()).collect::<Vec<_>>(), ["b", "c"]);
        mark_revoked_on(&mut j, "b", &old_id);
        mark_revoked_on(&mut j, "c", &old_id);
        assert!(pending(&j, &["a".into(), "b".into(), "c".into()], NOW).is_empty());
        // An expired grant needs no revocation (the relay would refuse it).
        assert!(targets(&mut j, &[("d".to_owned(), Some(BTreeSet::new()))], NOW + 366 * 86_400).is_empty());
    }

    #[test]
    fn revoke_takes_every_grant_of_the_layer() {
        let admin = tether_trust::generate_key();
        let (old, e) = create("group:monitored", "", &admin, NOW).unwrap();
        let mut j = vec![e];
        let (new, e) = create("group:monitored", "", &admin, NOW).unwrap();
        retire(&mut j, &old, "group:monitored", GrantState::Replaced);
        j.push(e);
        let (other, e) = create("profile:technician", "", &admin, NOW).unwrap();
        j.push(e);
        // Revoke after a Renew: the new grant (draft) and the old one (live on the relays).
        assert!(revoke_layer(&mut j, &new, "group:monitored"));
        assert_eq!(j.iter().map(|e| e.state).collect::<Vec<_>>(), [GrantState::Revoked, GrantState::Revoked, GrantState::Active]);
        let served = vec![("a".to_owned(), Some(ids_in(&bundle(Some("install"), Some(&old))))), ("b".to_owned(), None)];
        assert_eq!(targets(&mut j, &served, NOW).len(), 4);
        assert_eq!(info(&old, &[admin.verifying_key()], &j, NOW).status, "revoked");
        assert_eq!(info(&other, &[admin.verifying_key()], &j, NOW).status, "valid");
        assert!(!revoke_layer(&mut Journal::new(), "junk", "group:monitored"));
    }

    #[test]
    fn loading_the_old_grant_back_into_the_draft_restores_it() {
        let admin = tether_trust::generate_key();
        let (old, e) = create("group:monitored", "", &admin, NOW).unwrap();
        let mut j = vec![e];
        let (_new, e) = create("group:monitored", "", &admin, NOW).unwrap();
        retire(&mut j, &old, "group:monitored", GrantState::Replaced);
        j.push(e);
        // The renewed draft was discarded (an older version loaded): the old grant is current again,
        // the newer one is the replaced one.
        assert!(restore(&mut j, &bundle(Some("install"), Some(&old))));
        assert_eq!((j[0].state, j[1].state), (GrantState::Active, GrantState::Replaced));
        assert_eq!(info(&old, &[admin.verifying_key()], &j, NOW).status, "valid");
        assert!(!restore(&mut j, &bundle(Some("install"), Some(&old))), "nothing more to do");
        // Not once it was revoked on a relay.
        j[0].state = GrantState::Replaced;
        j[0].revoked_on.push("a".into());
        assert!(!restore(&mut j, &bundle(Some("install"), Some(&old))));
        assert_eq!(j[0].state, GrantState::Replaced);
    }

    #[test]
    fn the_check_uses_what_clients_resolve_group_over_profile() {
        let admin = tether_trust::generate_key();
        let keys = [admin.verifying_key()];
        let (token, e) = create("profile:technician", "", &admin, NOW).unwrap();
        let j = vec![e];
        let (expired, _) = status::make_token("policy:monitored", NOW - 1, None, None, true, &admin).unwrap();
        // A group set to install without its own grant inherits the profile's valid one.
        let mut b = bundle(Some("install"), None);
        b.profiles.insert("technician".into(), Layer::from([(GRANT.to_owned(), rule(&token))]));
        check(&b, &keys, &j, NOW).unwrap();
        // A group carrying only a grant under a profile set to install: that grant is checked.
        let mut b = bundle(None, Some(&expired));
        b.profiles.insert("technician".into(), Layer::from([(AGENT.to_owned(), rule("install")), (GRANT.to_owned(), rule(&token))]));
        let e = check(&b, &keys, &j, NOW).unwrap_err();
        assert!(e.starts_with("Group monitored: health.agent is install, but its enrolment grant has expired.") && e.contains("this group or its profile"), "{e}");
        // The profile alone: named as the profile.
        let mut b = Bundle::default();
        b.profiles.insert("customer".into(), Layer::from([(AGENT.to_owned(), rule("on-request"))]));
        assert!(check(&b, &keys, &j, NOW).unwrap_err().starts_with("Customer profile: health.agent is on-request, but it has no enrolment grant."));
        // A group switching it off over a profile set to install needs nothing.
        let mut b = bundle(Some("manual"), None);
        b.profiles.insert("technician".into(), Layer::from([(AGENT.to_owned(), rule("install")), (GRANT.to_owned(), rule(&token))]));
        check(&b, &keys, &j, NOW).unwrap();
    }

    #[test]
    fn a_revoked_grant_goes_to_every_relay_at_once_and_a_foreign_one_is_journalled() {
        let admin = tether_trust::generate_key();
        let (token, _) = create("profile:technician", "", &admin, NOW).unwrap();
        let mut j = Journal::new();
        // Made on another PC: added from the token itself.
        assert!(retire(&mut j, &token, "profile:technician", GrantState::Revoked));
        assert!(!retire(&mut j, "junk", "profile:technician", GrantState::Revoked));
        let id = peek(&token).unwrap().id;
        let served = vec![("a".to_owned(), Some(ids_in(&bundle(Some("install"), Some(&token))))), ("b".to_owned(), None)];
        assert_eq!(targets(&mut j, &served, NOW).iter().map(|t| t.0.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        // A later Replaced does not soften Revoked.
        retire(&mut j, &token, "profile:technician", GrantState::Replaced);
        assert_eq!((j.len(), j[0].state, j[0].id.as_str()), (1, GrantState::Revoked, id.as_str()));
    }

    #[test]
    fn journal_round_trip_and_damage() {
        assert!(parse_journal(None).unwrap().is_empty());
        assert!(parse_journal(Some(b"{not json")).unwrap_err().contains("damaged"));
        let e = Entry { id: "abcdefghABCDEFGH".into(), label: "policy:x".into(), layer: "group:x".into(), customer: None, created: 1, exp: 2, state: GrantState::Active, revoked_on: vec![], waiting: vec![] };
        let raw = serde_json::to_vec(&vec![e.clone()]).unwrap();
        assert_eq!(String::from_utf8(raw.clone()).unwrap(), r#"[{"id":"abcdefghABCDEFGH","label":"policy:x","layer":"group:x","created":1,"exp":2,"state":"active"}]"#);
        assert_eq!(parse_journal(Some(&raw)).unwrap(), [e]);
    }

    #[test]
    fn a_policy_without_the_features_loads_and_publishes_unchanged() {
        // A draft and a published payload of 0.6 (no health features): read, checked, written back byte for byte.
        let old = r#"{"serial":7,"issued_at":1700000000,"profiles":{"technician":{"permission.clipboard":{"state":"off"}}},"groups":{"g":{"profile":"customer","overrides":{}}},"assignments":{"u1":"g"}}"#;
        let b: Bundle = serde_json::from_str(old).unwrap();
        b.validate().unwrap();
        check(&b, &[tether_trust::generate_key().verifying_key()], &Journal::new(), NOW).unwrap();
        assert!(ids_in(&b).is_empty());
        assert_eq!(serde_json::to_string(&b).unwrap(), old);
    }
}

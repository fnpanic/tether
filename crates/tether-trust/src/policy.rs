//! Policy bundle (signed by the admin key), feature registry and resolution.
//!
//! Layers, later wins: built-in defaults < profile < custom app (settings built into a TetherQS
//! download; never for a feature the profile locks) < group overrides (device assigned to a
//! group) < machine policy (GPO/MDM, same `Layer` JSON). Every feature can be:
//! - `on`     enabled; the user may change it; `value` (if given) is the default;
//! - `off`    disabled and locked off;
//! - `locked` locked to `value` (bool features: on);
//! - `hidden` disabled, locked off and not shown in the UI.
//! A feature set by any non-default layer with state `off|locked|hidden` is locked.
//! Security-critical settings (TLS, encryption, 443-only networking) are not features and can
//! never be changed by policy: see [`FLOOR`].

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const PROFILE_TECHNICIAN: &str = "technician";
pub const PROFILE_CUSTOMER: &str = "customer";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    On,
    Off,
    Locked,
    Hidden,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub state: State,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

/// One policy layer: feature id → rule.
pub type Layer = BTreeMap<String, Rule>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Group {
    /// Profile the group applies to (`technician` or `customer`).
    pub profile: String,
    #[serde(default)]
    pub overrides: Layer,
}

/// The signed document's payload.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bundle {
    /// Strictly increasing; clients reject a bundle with a lower serial than the last accepted.
    pub serial: u64,
    /// Unix seconds.
    pub issued_at: u64,
    #[serde(default)]
    pub profiles: BTreeMap<String, Layer>,
    #[serde(default)]
    pub groups: BTreeMap<String, Group>,
    /// Device uuid (heartbeat `uuid`) → group name.
    #[serde(default)]
    pub assignments: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Bool,
    Choice,
    Text,
    Number,
}

#[derive(Debug, Clone, Serialize)]
pub struct Feature {
    pub id: &'static str,
    pub area: &'static str,
    pub en: &'static str,
    pub de: &'static str,
    pub kind: Kind,
    /// Default value (bool: "Y"/"N").
    pub default: &'static str,
    /// Allowed values for `Choice`.
    pub choices: &'static [&'static str],
    /// Value used when the feature is `off`/`hidden` (bool: "N").
    pub off_value: &'static str,
    /// Core (RustDesk) option this feature maps to, if any. Bool options use "Y"/"N".
    pub option: Option<&'static str>,
    /// Profiles this feature applies to.
    pub profiles: &'static [&'static str],
}

const BOTH: &[&str] = &[PROFILE_TECHNICIAN, PROFILE_CUSTOMER];
const TECH: &[&str] = &[PROFILE_TECHNICIAN];
const CUST: &[&str] = &[PROFILE_CUSTOMER];

macro_rules! b {
    ($id:expr, $area:expr, $en:expr, $de:expr, $def:expr, $opt:expr, $prof:expr) => {
        Feature { id: $id, area: $area, en: $en, de: $de, kind: Kind::Bool, default: $def, choices: &[], off_value: "N", option: $opt, profiles: $prof }
    };
}
macro_rules! c {
    ($id:expr, $area:expr, $en:expr, $de:expr, $def:expr, $choices:expr, $off:expr, $opt:expr, $prof:expr) => {
        Feature { id: $id, area: $area, en: $en, de: $de, kind: Kind::Choice, default: $def, choices: $choices, off_value: $off, option: $opt, profiles: $prof }
    };
}
macro_rules! t {
    ($id:expr, $area:expr, $en:expr, $de:expr, $kind:expr, $def:expr, $prof:expr) => {
        Feature { id: $id, area: $area, en: $en, de: $de, kind: $kind, default: $def, choices: &[], off_value: "", option: None, profiles: $prof }
    };
}

/// Every configurable feature. Adding one here makes it manageable in the management app.
/// `docs/policy.md` lists them all (checked by a test in the desktop app).
pub const FEATURES: &[Feature] = &[
    // Permissions granted to a remote technician when this device is controlled.
    b!("permission.control", "permissions", "Allow mouse and keyboard control", "Steuerung mit Maus und Tastatur erlauben", "Y", Some("enable-keyboard"), BOTH),
    b!("permission.clipboard", "permissions", "Allow clipboard sync", "Zwischenablage synchronisieren", "Y", Some("enable-clipboard"), BOTH),
    b!("permission.file-transfer", "permissions", "Allow file transfer", "Dateiübertragung erlauben", "Y", Some("enable-file-transfer"), BOTH),
    b!("permission.audio", "permissions", "Share audio", "Ton übertragen", "N", Some("enable-audio"), BOTH),
    b!("permission.restart", "permissions", "Allow remote restart", "Neustart aus der Ferne erlauben", "Y", Some("enable-remote-restart"), BOTH),
    b!("permission.tunnel", "permissions", "Allow TCP tunnels", "TCP-Tunnel erlauben", "N", Some("enable-tunnel"), BOTH),
    b!("permission.block-input", "permissions", "Allow blocking local input", "Lokale Eingabe sperren erlauben", "Y", Some("enable-block-input"), BOTH),
    // Access to this device.
    c!("security.approve-mode", "security", "How incoming sessions are approved", "Wie eingehende Sitzungen freigegeben werden", "both", &["both", "click", "password"], "click", Some("approve-mode"), BOTH),
    c!("security.verification-method", "security", "Accepted passwords", "Akzeptierte Passwörter", "both", &["both", "use-temporary-password", "use-permanent-password"], "use-temporary-password", Some("verification-method"), BOTH),
    c!("security.temporary-password-length", "security", "One-time password length", "Länge des Einmalpassworts", "10", &["6", "8", "10"], "10", Some("temporary-password-length"), BOTH),
    b!("security.permanent-password", "security", "Allow setting a permanent password", "Dauerhaftes Passwort erlauben", "Y", None, TECH),
    b!("security.unattended", "security", "Allow unattended access (background service)", "Unbeaufsichtigten Zugriff erlauben (Dienst)", "Y", None, TECH),
    // Connections.
    b!("connect.outgoing", "connections", "Allow connecting to other devices", "Verbindungen zu anderen Geräten erlauben", "Y", None, TECH),
    b!("connect.incoming", "connections", "Allow incoming sessions", "Eingehende Sitzungen erlauben", "Y", None, BOTH),
    b!("connect.chat", "connections", "Chat in sessions", "Chat in Sitzungen", "Y", None, BOTH),
    t!("connect.reconnect-minutes", "connections", "Keep reconnecting after a connection loss (minutes)", "Nach Verbindungsverlust erneut verbinden (Minuten)", Kind::Number, "10", BOTH),
    b!("settings.server", "connections", "Allow changing the relay server", "Relay-Server ändern erlauben", "N", None, TECH),
    t!("servers.list", "connections", "Additional relay servers (host names or wss:// URLs, comma-separated)", "Weitere Relay-Server (Hostnamen oder wss://-URLs, kommagetrennt)", Kind::Text, "", BOTH),
    c!("network.direct", "connections", "Direct connections (same network) beside the relay", "Direktverbindungen (gleiches Netzwerk) neben dem Relay", "auto", &["auto", "off"], "off", None, BOTH),
    // Video.
    c!("video.quality", "video", "Default image quality", "Standard-Bildqualität", "balanced", &["best", "balanced", "low"], "balanced", Some("image-quality"), TECH),
    c!("video.codec", "video", "Preferred codec", "Bevorzugter Codec", "auto", &["auto", "h264", "h265", "av1", "vp9", "vp8"], "auto", Some("codec-preference"), TECH),
    // Recording.
    c!("recording.mode", "recording", "Session recording", "Sitzungsaufzeichnung", "on-request", &["off", "on-request", "always"], "off", None, TECH),
    t!("recording.folder", "recording", "Recording folder", "Aufzeichnungsordner", Kind::Text, "", TECH),
    c!("recording.upload", "recording", "Upload recordings to", "Aufzeichnungen hochladen nach", "none", &["none", "unc", "https-put"], "none", None, TECH),
    t!("recording.upload-target", "recording", "Upload target (UNC path or HTTPS URL)", "Upload-Ziel (UNC-Pfad oder HTTPS-URL)", Kind::Text, "", TECH),
    t!("recording.upload-token", "recording", "Upload bearer token (HTTPS)", "Upload-Token (HTTPS)", Kind::Text, "", TECH),
    b!("recording.keep-local", "recording", "Keep local copy after upload", "Lokale Kopie nach Upload behalten", "Y", None, TECH),
    t!("recording.max-days", "recording", "Delete local recordings after (days, 0 = never)", "Lokale Aufzeichnungen löschen nach (Tage, 0 = nie)", Kind::Number, "0", TECH),
    t!("recording.consent-text", "recording", "Consent question shown to the other party", "Einwilligungsfrage an die Gegenseite", Kind::Text, "The technician wants to record this session. Allow?", BOTH),
    // Updates.
    b!("update.enabled", "updates", "Automatic updates", "Automatische Updates", "Y", None, BOTH),
    t!("update.channel", "updates", "Update channel", "Update-Kanal", Kind::Text, "stable", BOTH),
    t!("update.check-hours", "updates", "Check for updates every (hours)", "Nach Updates suchen alle (Stunden)", Kind::Number, "6", BOTH),
    t!("update.minimum-version", "updates", "Minimum version (older versions cannot connect)", "Mindestversion (ältere Versionen können nicht verbinden)", Kind::Text, "", BOTH),
    b!("update.force", "updates", "Install required updates automatically", "Pflicht-Updates automatisch installieren", "N", None, BOTH),
    c!("update.rollout", "updates", "Staged rollouts: early joins the first wave, late waits until a release reaches everyone", "Gestaffelte Verteilung: früh in der ersten Welle, spät erst wenn alle es bekommen", "default", &["early", "default", "late"], "default", None, BOTH),
    t!("update.restart-seconds", "updates", "Countdown before a forced update restarts the app (s)", "Countdown vor erzwungenem Neustart (s)", Kind::Number, "60", BOTH),
    // Policy delivery.
    t!("policy.refresh-minutes", "policy", "Check for policy changes every (minutes)", "Richtlinie prüfen alle (Minuten)", Kind::Number, "15", BOTH),
    b!("policy.fast-apply", "policy", "Fast apply (check every minute)", "Schnell anwenden (jede Minute prüfen)", "N", None, BOTH),
    // Diagnostics.
    c!("ui.advanced", "interface", "Advanced mode: host inventory with search and tags as the home view", "Erweiterter Modus: Host-Liste mit Suche und Tags als Startansicht", "available", &["off", "available", "forced"], "off", None, TECH),
    b!("diagnostics.page", "diagnostics", "Diagnostics page", "Diagnoseseite", "Y", None, BOTH),
    b!("diagnostics.upload", "diagnostics", "Send diagnostics to support", "Diagnose an Support senden", "Y", None, BOTH),
    b!("diagnostics.verbose-logging", "diagnostics", "Allow verbose logging", "Ausführliche Protokollierung erlauben", "Y", None, BOTH),
    // Customer module.
    t!("customer.title", "customer", "Window title / company name", "Fenstertitel / Firmenname", Kind::Text, "Tether Support", CUST),
    t!("customer.accent-color", "customer", "Accent colour (#rrggbb)", "Akzentfarbe (#rrggbb)", Kind::Text, "#0071e3", CUST),
    t!("customer.welcome", "customer", "Welcome text", "Begrüßungstext", Kind::Text, "Give this ID and password to your support technician.", CUST),
    b!("customer.close-after-session", "customer", "Quit when the session ends", "Nach Sitzungsende beenden", "N", None, CUST),
    b!("customer.elevate", "customer", "Admin access for UAC prompts (Elevate button, technician requests)", "Adminrechte für UAC (Erhöhen-Knopf, Anfragen des Technikers)", "Y", Some("allow-elevate"), CUST),
];

/// Core options that policy can never set: they guarantee encryption, TLS verification,
/// 443-only networking and the absence of silent recording. Enforced by the apps on top of policy.
pub const FLOOR: &[&str] = &[
    "allow-insecure-tls-fallback",
    "allow-remote-config-modification",
    "enable-check-update",
    "allow-auto-update",
    "direct-server",
    "enable-remote-printer",
    "allow-auto-record-incoming",
    "allow-auto-record-outgoing",
    "stop-service",
    "enable-camera",
    "enable-terminal",
    "allow-websocket",
    "disable-udp",
    "enable-lan-discovery",
    "api-server",
    "enable-webrtc",
    "enable-udp-punch",
    "enable-ipv6-punch",
    "custom-rendezvous-server",
    "relay-server",
    "key",
];

pub fn feature(id: &str) -> Option<&'static Feature> {
    FEATURES.iter().find(|f| f.id == id)
}

fn check_value(f: &Feature, v: &str) -> Result<(), String> {
    let ok = match (f.id, f.kind) {
        (SERVERS_LIST, _) => return parse_servers(v).map(|_| ()).map_err(|e| format!("feature {}: {e}", f.id)),
        // Free text that does not parse would silently never block (`version::below_minimum`).
        // Trimmed, as `version::compare` reads it: a padded minimum always was enforced.
        ("update.minimum-version", _) => v.trim().is_empty() || crate::release::valid_version(v.trim()),
        ("update.channel", _) => v.is_empty() || crate::release::valid_channel(v),
        (_, Kind::Bool) => v == "Y" || v == "N",
        (_, Kind::Choice) => f.choices.contains(&v),
        (_, Kind::Number) => v.parse::<u32>().is_ok(),
        (_, Kind::Text) => v.len() <= 1024 && !v.chars().any(|c| c.is_control() && c != '\n'),
    };
    if ok {
        Ok(())
    } else {
        Err(format!("invalid value {v:?} for feature {}", f.id))
    }
}

/// Feature holding extra rendezvous servers.
pub const SERVERS_LIST: &str = "servers.list";
/// Feature switching direct paths (LAN) beside the relay: `auto` or `off`.
pub const DIRECT: &str = "network.direct";
/// Most servers a policy may add.
pub const MAX_SERVERS: usize = 8;

/// Host name of a server entry: `host`, `host:port` or `wss://host/path`.
pub fn server_host(entry: &str) -> &str {
    let e = entry.strip_prefix("wss://").or_else(|| entry.strip_prefix("ws://")).unwrap_or(entry);
    let e = e.split('/').next().unwrap_or(e);
    e.split(':').next().unwrap_or(e)
}

/// Parse a comma-separated list of servers (`servers.list`): DNS names, or `wss://<name>/<path>`
/// for one rendezvous endpoint of a cluster. No port, IP literal, plain `ws://`, query or
/// credentials, so every entry is reached over TLS on 443 and verified with the same compiled-in
/// server key; a policy cannot point clients at anything else.
pub fn parse_servers(s: &str) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for raw in s.split(',').map(str::trim).filter(|x| !x.is_empty()) {
        let (host, path) = match raw.get(..6).filter(|p| p.eq_ignore_ascii_case("wss://")) {
            Some(_) => match raw[6..].split_once('/') {
                Some((h, p)) => (h, Some(p)),
                None => (&raw[6..], None),
            },
            None => (raw, None),
        };
        let path_ok = path.map_or(true, |p| {
            !p.is_empty() && p.len() <= 128 && p.chars().all(|c| c.is_ascii_alphanumeric() || "/_-.".contains(c)) && !p.contains("..")
        });
        if !path_ok {
            return Err(format!("{raw:?} has an invalid path"));
        }
        let h = host.to_ascii_lowercase();
        let labels: Vec<&str> = h.split('.').collect();
        let label_ok = |l: &&str| {
            (1..=63).contains(&l.len())
                && l.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                && !l.starts_with('-')
                && !l.ends_with('-')
        };
        let tld_ok = labels.last().is_some_and(|t| t.chars().any(|c| c.is_ascii_lowercase()));
        if h.len() > 253 || labels.len() < 2 || !labels.iter().all(label_ok) || !tld_ok {
            return Err(format!("{raw:?} is not a host name"));
        }
        let entry = match path {
            Some(p) => format!("wss://{h}/{p}"),
            None => h,
        };
        if !out.contains(&entry) {
            out.push(entry);
        }
    }
    if out.len() > MAX_SERVERS {
        return Err(format!("at most {MAX_SERVERS} servers"));
    }
    Ok(out)
}

/// The feature of one rule, if this build knows it and the rule is valid for it.
fn check_rule(id: &str, rule: &Rule) -> Result<&'static Feature, String> {
    let f = feature(id).ok_or_else(|| format!("unknown feature {id:?}"))?;
    if let Some(v) = &rule.value {
        check_value(f, v)?;
    }
    if rule.state == State::Locked && rule.value.is_none() && f.kind != Kind::Bool {
        return Err(format!("locked feature {id} needs a value"));
    }
    Ok(f)
}

fn check_layer(layer: &Layer, ctx: &str) -> Result<(), String> {
    for (id, rule) in layer {
        check_rule(id, rule).map_err(|e| format!("{ctx}: {e}"))?;
    }
    Ok(())
}

/// Takes out of a layer (JSON) the rules this build cannot read (a state or field of a newer
/// Tether Manage), so that they do not make the whole document unreadable.
pub(crate) fn drop_unreadable_rules(layer: &mut serde_json::Value) {
    if let Some(rules) = layer.as_object_mut() {
        rules.retain(|_, r| Rule::deserialize(&*r).is_ok());
    }
}

impl Bundle {
    /// Strict validation, for whoever writes or accepts a policy (Tether Manage, the relay):
    /// unknown features, bad values, unknown profiles or groups make the whole bundle invalid, so
    /// a typo can never silently do nothing.
    pub fn validate(&self) -> Result<(), String> {
        self.check(true)
    }

    /// Not `strict` (the apps): only what would make the document inconsistent; a rule, profile
    /// or group profile this build does not know or finds invalid is left to [`resolve`].
    fn check(&self, strict: bool) -> Result<(), String> {
        let known = |p: &str| !strict || p == PROFILE_TECHNICIAN || p == PROFILE_CUSTOMER;
        for (p, layer) in &self.profiles {
            if !known(p) {
                return Err(format!("unknown profile {p:?}"));
            }
            if strict {
                check_layer(layer, &format!("profile {p}"))?;
            }
        }
        for (name, g) in &self.groups {
            if name.is_empty() || name.len() > 64 {
                return Err("group names must be 1-64 characters".into());
            }
            if !known(&g.profile) {
                return Err(format!("group {name}: unknown profile {:?}", g.profile));
            }
            if strict {
                check_layer(&g.overrides, &format!("group {name}"))?;
            }
        }
        for (dev, group) in &self.assignments {
            if !self.groups.contains_key(group) {
                return Err(format!("device {dev} assigned to unknown group {group:?}"));
            }
        }
        Ok(())
    }

    /// Open a published bundle (the apps). Unlike [`Bundle::validate`], one rule this build does
    /// not know or finds invalid (a newer feature, a value an older Tether Manage let through)
    /// does not make the bundle invalid: [`resolve`] skips that rule and names it in `rejected`.
    /// An app must not ignore a whole policy, with its lock-downs, for one such rule.
    pub fn from_json(payload: &[u8]) -> Result<Self, String> {
        let mut v: serde_json::Value = serde_json::from_slice(payload).map_err(|e| format!("invalid policy: {e}"))?;
        let groups = v.get_mut("groups").and_then(|g| g.as_object_mut()).into_iter().flat_map(|g| g.values_mut()).filter_map(|g| g.get_mut("overrides"));
        groups.for_each(drop_unreadable_rules);
        v.get_mut("profiles").and_then(|p| p.as_object_mut()).into_iter().flat_map(|p| p.values_mut()).for_each(drop_unreadable_rules);
        let b: Bundle = serde_json::from_value(v).map_err(|e| format!("invalid policy: {e}"))?;
        b.check(false)?;
        Ok(b)
    }

    /// Group of a device for a profile, if assigned and the group targets that profile.
    pub fn group_of(&self, profile: &str, device: &str) -> Option<&str> {
        let g = self.assignments.get(device)?;
        (self.groups.get(g)?.profile == profile).then_some(g.as_str())
    }
}

/// The result clients apply.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Effective {
    pub profile: String,
    pub serial: u64,
    pub group: Option<String>,
    /// Feature id → value (every feature of the profile).
    pub values: BTreeMap<String, String>,
    pub locked: BTreeSet<String>,
    pub hidden: BTreeSet<String>,
    /// Which layers contributed: "defaults", "custom-app", "profile", "group:<name>", "machine".
    pub sources: Vec<String>,
    /// Rules that were not applied (unknown feature, bad value), as `<source>: <reason>`, for the
    /// log; the other rules of their layer are applied.
    pub rejected: Vec<String>,
}

impl Effective {
    pub fn get(&self, id: &str) -> &str {
        self.values.get(id).map(String::as_str).unwrap_or("")
    }
    pub fn on(&self, id: &str) -> bool {
        self.get(id) == "Y"
    }
    pub fn is_locked(&self, id: &str) -> bool {
        self.locked.contains(id)
    }
    /// Core options to force (overwrite settings): locked features with a core option.
    pub fn core_overrides(&self) -> Vec<(&'static str, String)> {
        FEATURES
            .iter()
            .filter(|f| self.locked.contains(f.id))
            .filter_map(|f| f.option.map(|o| (o, self.get(f.id).to_owned())))
            .collect()
    }
    /// Core option defaults (user may change): unlocked features with a core option.
    pub fn core_defaults(&self) -> Vec<(&'static str, String)> {
        FEATURES
            .iter()
            .filter(|f| f.option.is_some() && self.values.contains_key(f.id) && !self.locked.contains(f.id))
            .filter_map(|f| f.option.map(|o| (o, self.get(f.id).to_owned())))
            .collect()
    }
}

fn apply(eff: &mut Effective, layer: &Layer, source: String) {
    for (id, rule) in layer {
        // Rule by rule: one bad or unknown rule must not drop the rest of its layer.
        let f = match check_rule(id, rule) {
            Ok(f) => f,
            Err(e) => {
                eff.rejected.push(format!("{source}: {e}"));
                continue;
            }
        };
        if !eff.values.contains_key(f.id) {
            continue; // feature not part of this profile
        }
        let value = match rule.state {
            State::On | State::Locked => rule.value.clone().unwrap_or_else(|| {
                if f.kind == Kind::Bool { "Y".into() } else { f.default.into() }
            }),
            State::Off | State::Hidden => f.off_value.to_owned(),
        };
        eff.values.insert(f.id.to_owned(), value);
        match rule.state {
            State::On => {
                eff.locked.remove(f.id);
                eff.hidden.remove(f.id);
            }
            State::Off | State::Locked => {
                eff.locked.insert(f.id.to_owned());
                eff.hidden.remove(f.id);
            }
            State::Hidden => {
                eff.locked.insert(f.id.to_owned());
                eff.hidden.insert(f.id.to_owned());
            }
        }
    }
    eff.sources.push(source);
}

/// Resolve the effective policy for `profile` on `device`; `machine` is the optional GPO/MDM
/// layer. A rule that is not valid in this build is left out and named in `rejected`.
pub fn resolve(bundle: Option<&Bundle>, profile: &str, device: &str, machine: Option<&Layer>) -> Effective {
    resolve_with(bundle, profile, device, None, machine)
}

/// [`resolve`] with the settings of a custom app (a signed [`crate::custom::CustomProfile`] built
/// into a TetherQS download). Order, later wins: defaults < profile < custom app < group <
/// machine: the custom app is the admin's choice for everyone who uses that download, while groups
/// and GPO/MDM still target individual devices. Except for what the profile locks (`off`,
/// `locked`, `hidden`): there the custom app's rule is left out, because its signed profile has no
/// serial or expiry and cannot be revoked, so it must never unlock what the current policy locks.
pub fn resolve_with(bundle: Option<&Bundle>, profile: &str, device: &str, custom: Option<&Layer>, machine: Option<&Layer>) -> Effective {
    let mut eff = Effective { profile: profile.to_owned(), sources: vec!["defaults".into()], ..Default::default() };
    for f in FEATURES.iter().filter(|f| f.profiles.contains(&profile)) {
        eff.values.insert(f.id.to_owned(), f.default.to_owned());
    }
    if let Some(b) = bundle {
        eff.serial = b.serial;
        if let Some(layer) = b.profiles.get(profile) {
            apply(&mut eff, layer, "profile".into());
        }
    }
    if let Some(c) = custom {
        // Only the profile layer has locked anything so far.
        let open: Layer = c.iter().filter(|(id, _)| !eff.locked.contains(*id)).map(|(id, r)| (id.clone(), r.clone())).collect();
        apply(&mut eff, &open, "custom-app".into());
    }
    if let Some(b) = bundle {
        if let Some(g) = b.group_of(profile, device) {
            apply(&mut eff, &b.groups[g].overrides, format!("group:{g}"));
            eff.group = Some(g.to_owned());
        }
    }
    if let Some(m) = machine {
        apply(&mut eff, m, "machine".into());
    }
    eff
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(state: State, value: Option<&str>) -> Rule {
        Rule { state, value: value.map(Into::into) }
    }

    #[test]
    fn server_list_accepts_host_names_only() {
        assert_eq!(parse_servers("").unwrap(), Vec::<String>::new());
        assert_eq!(
            parse_servers(" Tether-Desk2.duckdns.org, relay.example.com ,tether-desk2.duckdns.org,").unwrap(),
            ["tether-desk2.duckdns.org", "relay.example.com"]
        );
        for bad in [
            "ws://relay.example.com",
            "https://relay.example.com",
            "relay.example.com:21116",
            "relay.example.com/ws/id",
            "158.101.205.185",
            "[::1]",
            "localhost",
            "-bad.example.com",
            "bad_.example.com",
            "a..example.com",
            "relay.example.com;evil.com",
            "relay example.com",
            "ws://relay.example.com/ws/id",
            "wss://relay.example.com:8443/ws/id",
            "wss://user@relay.example.com/ws/id",
            "wss://relay.example.com/ws/id?x=1",
            "wss://1.2.3.4/ws/id",
            "wss://relay.example.com/",
            "wss://relay.example.com/../x",
        ] {
            assert!(parse_servers(bad).is_err(), "{bad} accepted");
        }
        assert_eq!(
            parse_servers("WSS://K8s.Example.com/ws/id/1, wss://k8s.example.com/ws/id/2").unwrap(),
            ["wss://k8s.example.com/ws/id/1", "wss://k8s.example.com/ws/id/2"]
        );
        assert_eq!(server_host("wss://k8s.example.com/ws/id/1"), "k8s.example.com");
        assert_eq!(server_host("relay.example.com:21116"), "relay.example.com");
        let many = (0..9).map(|i| format!("r{i}.example.com")).collect::<Vec<_>>().join(",");
        assert!(parse_servers(&many).is_err());
        // A bundle with a bad list is rejected as a whole.
        let mut b = Bundle::default();
        let layer: Layer = [(SERVERS_LIST.to_owned(), rule(State::Locked, Some("1.2.3.4")))].into();
        b.profiles.insert(PROFILE_TECHNICIAN.into(), layer);
        assert!(b.validate().is_err());
        let layer: Layer = [(SERVERS_LIST.to_owned(), rule(State::Locked, Some("relay2.example.com")))].into();
        b.profiles.insert(PROFILE_TECHNICIAN.into(), layer);
        assert!(b.validate().is_ok());
    }

    #[test]
    fn elevate_off_reaches_the_core_of_tetherqs_only() {
        // The host's elevation-request handler reads `allow-elevate`: a technician's "Request
        // admin access" must honour the policy like the customer's own Elevate button.
        let mut b = Bundle::default();
        b.profiles.insert(PROFILE_CUSTOMER.into(), [("customer.elevate".to_owned(), rule(State::Locked, Some("N")))].into());
        let qs = resolve(Some(&b), PROFILE_CUSTOMER, "d", None);
        assert!(qs.core_overrides().contains(&("allow-elevate", "N".to_owned())));
        b.profiles.insert(PROFILE_CUSTOMER.into(), [("customer.elevate".to_owned(), rule(State::Off, None))].into());
        assert!(resolve(Some(&b), PROFILE_CUSTOMER, "d", None).core_overrides().contains(&("allow-elevate", "N".to_owned())));
        let tech = resolve(Some(&b), PROFILE_TECHNICIAN, "d", None);
        assert!(!tech.core_overrides().iter().chain(tech.core_defaults().iter()).any(|(k, _)| *k == "allow-elevate"));
    }

    #[test]
    fn registry_is_consistent_and_never_touches_the_floor() {
        let mut ids = BTreeSet::new();
        for f in FEATURES {
            assert!(ids.insert(f.id), "duplicate feature {}", f.id);
            assert!(check_value(f, f.default).is_ok(), "bad default for {}", f.id);
            if let Some(o) = f.option {
                assert!(!FLOOR.contains(&o), "{} maps to floor option {o}", f.id);
            }
            if f.kind == Kind::Choice {
                assert!(f.choices.contains(&f.off_value), "bad off value for {}", f.id);
            }
        }
        for o in ["enable-camera", "enable-terminal"] {
            assert!(FLOOR.contains(&o), "{o} must be a floor option");
        }
    }

    #[test]
    fn layers_merge_lock_and_hide() {
        let mut b = Bundle { serial: 7, ..Default::default() };
        b.profiles.insert(
            PROFILE_TECHNICIAN.into(),
            Layer::from([
                ("permission.clipboard".into(), rule(State::Off, None)),
                ("security.approve-mode".into(), rule(State::Locked, Some("click"))),
                ("video.quality".into(), rule(State::On, Some("best"))),
            ]),
        );
        b.groups.insert(
            "restricted".into(),
            Group { profile: PROFILE_TECHNICIAN.into(), overrides: Layer::from([("permission.file-transfer".into(), rule(State::Hidden, None))]) },
        );
        b.assignments.insert("dev-1".into(), "restricted".into());
        b.validate().unwrap();

        let e = resolve(Some(&b), PROFILE_TECHNICIAN, "dev-1", None);
        assert_eq!(e.serial, 7);
        assert_eq!(e.group.as_deref(), Some("restricted"));
        assert_eq!(e.get("permission.clipboard"), "N");
        assert!(e.is_locked("permission.clipboard"));
        assert_eq!(e.get("security.approve-mode"), "click");
        assert_eq!(e.get("video.quality"), "best");
        assert!(!e.is_locked("video.quality"));
        assert!(e.hidden.contains("permission.file-transfer"));
        assert!(e.core_overrides().contains(&("enable-clipboard", "N".into())));
        assert!(e.core_defaults().contains(&("image-quality", "best".into())));

        // Another device is not in the group.
        let e2 = resolve(Some(&b), PROFILE_TECHNICIAN, "dev-2", None);
        assert_eq!(e2.get("permission.file-transfer"), "Y");
        // Customer profile has no technician-only features.
        let c = resolve(Some(&b), PROFILE_CUSTOMER, "dev-1", None);
        assert_eq!(c.get("recording.mode"), "");
        assert_eq!(c.group, None);
        // Machine layer wins.
        let m = Layer::from([("permission.clipboard".into(), rule(State::On, None))]);
        let e3 = resolve(Some(&b), PROFILE_TECHNICIAN, "dev-2", Some(&m));
        assert_eq!(e3.get("permission.clipboard"), "Y");
    }

    #[test]
    fn invalid_bundles_are_rejected() {
        // Authoring (`validate`): everything unknown or invalid; the apps (`from_json`): only
        // what is not a matter of one rule.
        for bad in [
            &br#"{"serial":1,"issued_at":0,"profiles":{"nobody":{}}}"#[..],
            br#"{"serial":1,"issued_at":0,"groups":{"g":{"profile":"nobody"}}}"#,
            br#"{"serial":1,"issued_at":0,"profiles":{"technician":{"no.such":{"state":"on"}}}}"#,
            br#"{"serial":1,"issued_at":0,"profiles":{"technician":{"security.approve-mode":{"state":"locked","value":"never"}}}}"#,
        ] {
            let b = Bundle::from_json(bad).unwrap_or_else(|e| panic!("{e}"));
            assert!(b.validate().is_err(), "{}", String::from_utf8_lossy(bad));
        }
        let bad_group = br#"{"serial":1,"issued_at":0,"assignments":{"d":"nope"}}"#;
        assert!(Bundle::from_json(bad_group).is_err());
        let unknown_field = br#"{"serial":1,"issued_at":0,"extra":1}"#;
        assert!(Bundle::from_json(unknown_field).is_err());
        let ok = br#"{"serial":1,"issued_at":0,"profiles":{"customer":{"customer.title":{"state":"locked","value":"ACME Support"}}}}"#;
        assert!(Bundle::from_json(ok).is_ok());
    }

    #[test]
    fn unknown_feature_is_skipped_by_apps_and_refused_when_authoring() {
        // A bundle from a newer Tether Manage: features this build does not know.
        let newer = br#"{"serial":3,"issued_at":0,"profiles":{"technician":{"no.such":{"state":"off"},"permission.clipboard":{"state":"off"}}},"groups":{"g":{"profile":"technician","overrides":{"no.such2":{"state":"locked","value":"x"}}}},"assignments":{"d":"g"}}"#;
        let b = Bundle::from_json(newer).expect("the rest of the policy still applies");
        assert!(b.validate().unwrap_err().contains("unknown feature"));
        let e = resolve(Some(&b), PROFILE_TECHNICIAN, "d", None);
        assert!(e.is_locked("permission.clipboard") && !e.on("permission.clipboard"));
        assert_eq!(e.rejected, ["profile: unknown feature \"no.such\"", "group:g: unknown feature \"no.such2\""]);
    }

    #[test]
    fn values_a_released_manage_let_through_do_not_drop_the_policy() {
        // Free text until now: a padded minimum (enforced by every released app), one that never
        // parsed, a channel no relay can have.
        let old = br#"{"serial":4,"issued_at":0,"profiles":{"technician":{"update.minimum-version":{"state":"locked","value":"0.5.3 "},"update.channel":{"state":"locked","value":"Stable"},"permission.clipboard":{"state":"off"}},"customer":{"update.minimum-version":{"state":"locked","value":"0.5.x"},"update.channel":{"state":"locked","value":"beta_1"},"permission.clipboard":{"state":"off"}}}}"#;
        let b = Bundle::from_json(old).expect("the policy still applies");
        let e = resolve(Some(&b), PROFILE_TECHNICIAN, "d", None);
        assert_eq!(e.get("update.minimum-version"), "0.5.3 ");
        assert!(crate::version::below_minimum("0.5.2", e.get("update.minimum-version")), "still blocks");
        assert_eq!(e.get("update.channel"), "stable");
        assert!(e.is_locked("permission.clipboard") && !e.on("permission.clipboard"));
        assert_eq!(e.rejected.len(), 1, "{:?}", e.rejected);
        let e = resolve(Some(&b), PROFILE_CUSTOMER, "d", None);
        assert_eq!((e.get("update.minimum-version"), e.get("update.channel")), ("", "stable"));
        assert!(e.is_locked("permission.clipboard") && !e.on("permission.clipboard"));
        assert!(e.rejected.iter().any(|r| r.contains("0.5.x")) && e.rejected.len() == 2, "{:?}", e.rejected);
        // Publishing them again is refused.
        assert!(b.validate().is_err());
    }

    #[test]
    fn rules_of_a_newer_manage_are_left_out() {
        // A state and a rule field this build cannot read: the other rules still apply.
        let newer = br#"{"serial":5,"issued_at":0,"profiles":{"technician":{"permission.audio":{"state":"forced"},"permission.tunnel":{"state":"on","until":9},"permission.clipboard":{"state":"off"}}},"groups":{"g":{"profile":"technician","overrides":{"permission.restart":{"state":"forced"},"permission.control":{"state":"off"}}}},"assignments":{"d":"g"}}"#;
        let b = Bundle::from_json(newer).expect("the rest of the policy still applies");
        let e = resolve(Some(&b), PROFILE_TECHNICIAN, "d", None);
        assert!(e.is_locked("permission.clipboard") && e.is_locked("permission.control"));
        assert!(!e.is_locked("permission.audio") && !e.is_locked("permission.restart") && !e.on("permission.tunnel"));
        // Authoring (serde, as Tether Manage and the relay read a bundle) stays strict.
        assert!(serde_json::from_slice::<Bundle>(newer).is_err());
    }

    #[test]
    fn custom_app_cannot_unlock_what_the_policy_locks() {
        let mut b = Bundle::default();
        b.profiles.insert(
            PROFILE_CUSTOMER.into(),
            Layer::from([
                ("permission.file-transfer".into(), rule(State::Off, None)),
                ("permission.restart".into(), rule(State::On, None)),
                ("customer.title".into(), rule(State::On, Some("Policy"))),
            ]),
        );
        let custom = Layer::from([
            ("permission.file-transfer".into(), rule(State::On, None)),
            ("permission.audio".into(), rule(State::Locked, None)),
            ("permission.restart".into(), rule(State::Off, None)),
            ("customer.title".into(), rule(State::On, Some("ACME"))),
        ]);
        let e = resolve_with(Some(&b), PROFILE_CUSTOMER, "d", Some(&custom), None);
        assert_eq!(e.get("permission.file-transfer"), "N");
        assert!(e.is_locked("permission.file-transfer"));
        // What the policy does not lock is the custom app's: its own lock-down survives a profile
        // `on`, and so do its settings.
        assert!(e.on("permission.audio") && e.is_locked("permission.audio"));
        assert!(!e.on("permission.restart") && e.is_locked("permission.restart"));
        assert_eq!(e.get("customer.title"), "ACME");
        assert_eq!(e.sources, ["defaults", "profile", "custom-app"]);
        assert!(e.rejected.is_empty());
        // Groups and the machine layer still win over the custom app.
        b.groups.insert("g".into(), Group { profile: PROFILE_CUSTOMER.into(), overrides: Layer::from([("permission.restart".into(), rule(State::On, None))]) });
        b.assignments.insert("d".into(), "g".into());
        assert!(resolve_with(Some(&b), PROFILE_CUSTOMER, "d", Some(&custom), None).on("permission.restart"));
    }

    #[test]
    fn one_bad_rule_does_not_drop_its_layer() {
        let layer = Layer::from([
            ("permission.clipboard".into(), rule(State::Off, None)),
            ("no.such".into(), rule(State::Off, None)),
            ("security.approve-mode".into(), rule(State::Locked, Some("never"))),
            ("video.quality".into(), rule(State::Locked, None)),
        ]);
        for (custom, machine, source) in [(None, Some(&layer), "machine"), (Some(&layer), None, "custom-app")] {
            let e = resolve_with(None, PROFILE_TECHNICIAN, "d", custom, machine);
            assert!(e.is_locked("permission.clipboard") && !e.on("permission.clipboard"), "{source}");
            assert_eq!(e.get("security.approve-mode"), "both");
            assert!(!e.is_locked("video.quality"));
            assert_eq!(e.rejected.len(), 3, "{:?}", e.rejected);
            assert!(e.rejected.iter().all(|r| r.starts_with(&format!("{source}: "))), "{:?}", e.rejected);
            assert!(e.sources.contains(&source.to_owned()));
        }
        assert!(resolve(None, PROFILE_TECHNICIAN, "d", None).rejected.is_empty());
    }

    #[test]
    fn minimum_version_and_channel_must_parse() {
        let check = |id: &str, v: &str| check_value(feature(id).unwrap(), v).is_ok();
        for ok in ["", "0.5.35", "0.5.0-test.1", "1.0", " 0.5.35 "] {
            assert!(check("update.minimum-version", ok), "{ok:?}");
        }
        for bad in ["0.5.x", "latest", "0.5 .35", ">=0.5"] {
            assert!(!check("update.minimum-version", bad), "{bad:?}");
        }
        // What made it a defect: such a minimum never blocked anyone.
        assert!(!crate::version::below_minimum("0.0.1", "0.5.x"));
        for ok in ["", "stable", "beta-2"] {
            assert!(check("update.channel", ok), "{ok:?}");
        }
        for bad in ["Stable", "../x", "a b"] {
            assert!(!check("update.channel", bad), "{bad:?}");
        }
    }
}

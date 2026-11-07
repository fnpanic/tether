//! Policy bundle (signed by the admin key), feature registry and resolution.
//!
//! Layers, later wins: built-in defaults < profile < group overrides (device assigned to a group)
//! < machine policy (GPO/MDM, same `Layer` JSON). Every feature can be:
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
    b!("customer.elevate", "customer", "Offer \"Elevate\" (admin access for UAC prompts)", "\"Erhöhen\" anbieten (Adminrechte für UAC)", "Y", None, CUST),
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
    let ok = match f.kind {
        Kind::Bool => v == "Y" || v == "N",
        Kind::Choice => f.choices.contains(&v),
        Kind::Number => v.parse::<u32>().is_ok(),
        Kind::Text => v.len() <= 1024 && !v.chars().any(|c| c.is_control() && c != '\n'),
    };
    if f.id == SERVERS_LIST {
        return parse_servers(v).map(|_| ()).map_err(|e| format!("feature {}: {e}", f.id));
    }
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

fn check_layer(layer: &Layer, ctx: &str) -> Result<(), String> {
    for (id, rule) in layer {
        let f = feature(id).ok_or_else(|| format!("{ctx}: unknown feature {id:?}"))?;
        if let Some(v) = &rule.value {
            check_value(f, v).map_err(|e| format!("{ctx}: {e}"))?;
        }
        if rule.state == State::Locked && rule.value.is_none() && f.kind != Kind::Bool {
            return Err(format!("{ctx}: locked feature {id} needs a value"));
        }
    }
    Ok(())
}

impl Bundle {
    /// Strict validation: unknown features, bad values, unknown profiles or groups make the whole
    /// bundle invalid, so a typo can never silently do nothing.
    pub fn validate(&self) -> Result<(), String> {
        for (p, layer) in &self.profiles {
            if p != PROFILE_TECHNICIAN && p != PROFILE_CUSTOMER {
                return Err(format!("unknown profile {p:?}"));
            }
            check_layer(layer, &format!("profile {p}"))?;
        }
        for (name, g) in &self.groups {
            if name.is_empty() || name.len() > 64 {
                return Err("group names must be 1-64 characters".into());
            }
            if g.profile != PROFILE_TECHNICIAN && g.profile != PROFILE_CUSTOMER {
                return Err(format!("group {name}: unknown profile {:?}", g.profile));
            }
            check_layer(&g.overrides, &format!("group {name}"))?;
        }
        for (dev, group) in &self.assignments {
            if !self.groups.contains_key(group) {
                return Err(format!("device {dev} assigned to unknown group {group:?}"));
            }
        }
        Ok(())
    }

    pub fn from_json(payload: &[u8]) -> Result<Self, String> {
        let b: Bundle = serde_json::from_slice(payload).map_err(|e| format!("invalid policy: {e}"))?;
        b.validate()?;
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
    /// Which layers contributed: "defaults", "profile", "group:<name>", "machine".
    pub sources: Vec<String>,
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

fn apply(eff: &mut Effective, layer: &Layer) {
    for (id, rule) in layer {
        let Some(f) = feature(id) else { continue };
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
}

/// Resolve the effective policy for `profile` on `device`. `bundle` must already be validated
/// (it is, when obtained via [`Bundle::from_json`]); `machine` is the optional GPO/MDM layer.
pub fn resolve(bundle: Option<&Bundle>, profile: &str, device: &str, machine: Option<&Layer>) -> Effective {
    resolve_with(bundle, profile, device, None, machine)
}

/// [`resolve`] with the settings of a custom app (a signed [`crate::custom::CustomProfile`] built
/// into a TetherQS download). Order, later wins: defaults < profile < custom app < group <
/// machine: the custom app is the admin's choice for everyone who uses that download, while groups
/// and GPO/MDM still target individual devices.
pub fn resolve_with(bundle: Option<&Bundle>, profile: &str, device: &str, custom: Option<&Layer>, machine: Option<&Layer>) -> Effective {
    let mut eff = Effective { profile: profile.to_owned(), sources: vec!["defaults".into()], ..Default::default() };
    for f in FEATURES.iter().filter(|f| f.profiles.contains(&profile)) {
        eff.values.insert(f.id.to_owned(), f.default.to_owned());
    }
    if let Some(b) = bundle {
        eff.serial = b.serial;
        if let Some(layer) = b.profiles.get(profile) {
            apply(&mut eff, layer);
            eff.sources.push("profile".into());
        }
    }
    if let Some(c) = custom {
        if check_layer(c, "custom app").is_ok() {
            apply(&mut eff, c);
            eff.sources.push("custom-app".into());
        }
    }
    if let Some(b) = bundle {
        if let Some(g) = b.group_of(profile, device) {
            apply(&mut eff, &b.groups[g].overrides);
            eff.group = Some(g.to_owned());
            eff.sources.push(format!("group:{g}"));
        }
    }
    if let Some(m) = machine {
        if check_layer(m, "machine policy").is_ok() {
            apply(&mut eff, m);
            eff.sources.push("machine".into());
        }
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
        let bad_feature = br#"{"serial":1,"issued_at":0,"profiles":{"technician":{"no.such":{"state":"on"}}}}"#;
        assert!(Bundle::from_json(bad_feature).is_err());
        let bad_value = br#"{"serial":1,"issued_at":0,"profiles":{"technician":{"security.approve-mode":{"state":"locked","value":"never"}}}}"#;
        assert!(Bundle::from_json(bad_value).is_err());
        let bad_group = br#"{"serial":1,"issued_at":0,"assignments":{"d":"nope"}}"#;
        assert!(Bundle::from_json(bad_group).is_err());
        let unknown_field = br#"{"serial":1,"issued_at":0,"extra":1}"#;
        assert!(Bundle::from_json(unknown_field).is_err());
        let ok = br#"{"serial":1,"issued_at":0,"profiles":{"customer":{"customer.title":{"state":"locked","value":"ACME Support"}}}}"#;
        assert!(Bundle::from_json(ok).is_ok());
    }
}

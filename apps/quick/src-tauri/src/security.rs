//! Security floor and customer-module identity, applied in every process role before the core
//! starts. Copied from apps/desktop (security.rs); rationale for each setting: /SECURITY.md.

use hbb_common::config::{DEFAULT_SETTINGS, HARD_SETTINGS, OVERWRITE_LOCAL_SETTINGS, OVERWRITE_SETTINGS};

/// Forced: users, remote peers and policy cannot change these (all are in `tether_trust::policy::FLOOR`).
const ENFORCED: &[(&str, &str)] = &[
    ("enable-check-update", "N"),
    ("allow-auto-update", "N"),
    ("allow-remote-config-modification", "N"),
    ("allow-insecure-tls-fallback", "N"),
    ("enable-remote-printer", "N"),
    // `direct-server` (the direct listener) is set by netpolicy.rs from policy `network.direct`.
    ("allow-auto-record-incoming", "N"),
    ("stop-service", "N"),
    // Incoming screen sessions only: no camera view, no command line into the customer's computer.
    ("enable-camera", "N"),
    ("enable-terminal", "N"),
];

/// Forced like `ENFORCED`; the core reads these from the local config map.
const ENFORCED_LOCAL: &[(&str, &str)] = &[("allow-auto-record-outgoing", "N")];

const DEFAULTS: &[(&str, &str)] = &[("temporary-password-length", "10")];

/// The customer module only ever receives sessions and never installs anything.
const HARD: &[(&str, &str)] = &[("conn-type", "incoming"), ("disable-installation", "Y")];

pub fn apply_policy() {
    let mut h = HARD_SETTINGS.write().unwrap();
    for (k, v) in HARD {
        h.insert((*k).to_owned(), (*v).to_owned());
    }
    drop(h);
    let mut o = OVERWRITE_SETTINGS.write().unwrap();
    for (k, v) in ENFORCED {
        o.insert((*k).to_owned(), (*v).to_owned());
    }
    drop(o);
    let mut l = OVERWRITE_LOCAL_SETTINGS.write().unwrap();
    for (k, v) in ENFORCED_LOCAL {
        l.insert((*k).to_owned(), (*v).to_owned());
    }
    drop(l);
    let mut d = DEFAULT_SETTINGS.write().unwrap();
    for (k, v) in DEFAULTS {
        d.insert((*k).to_owned(), (*v).to_owned());
    }
    // No plain ws:// to a server (an IP endpoint would get one); loopback stays for local tests.
    hbb_common::websocket::require_tls();
}

#[cfg(test)]
mod tests {
    #[test]
    fn incoming_only_and_floor() {
        super::apply_policy();
        assert!(hbb_common::config::is_incoming_only());
        assert!(hbb_common::config::is_disable_installation());
        for (k, _) in super::ENFORCED.iter().chain(super::ENFORCED_LOCAL) {
            assert!(tether_trust::policy::FLOOR.contains(k), "{k} must be a floor key");
        }
        // Each in the map the core reads it from.
        let forced = |m: &std::sync::RwLock<std::collections::HashMap<String, String>>, k: &str| m.read().unwrap().get(k).map(String::as_str) == Some("N");
        assert!(forced(&hbb_common::config::OVERWRITE_SETTINGS, "enable-camera"));
        assert!(forced(&hbb_common::config::OVERWRITE_SETTINGS, "enable-terminal"));
        assert!(forced(&hbb_common::config::OVERWRITE_LOCAL_SETTINGS, "allow-auto-record-outgoing"));
    }
}

//! Security and privacy policy, applied in every process role before the core starts.
//! Rationale for each setting: /SECURITY.md.

use hbb_common::config::{DEFAULT_SETTINGS, OVERWRITE_SETTINGS};

/// Forced: users and remote peers cannot change these.
pub(crate) const ENFORCED: &[(&str, &str)] = &[
    // No calls home to third-party update servers.
    ("enable-check-update", "N"),
    ("allow-auto-update", "N"),
    // A connected peer must never be able to change this machine's settings.
    ("allow-remote-config-modification", "N"),
    // Never downgrade TLS for API/web requests.
    ("allow-insecure-tls-fallback", "N"),
    // Reduce attack surface: features that install drivers or open listeners.
    ("enable-remote-printer", "N"),
    // `direct-server` (the direct listener) is set by netpolicy.rs from policy `network.direct`.
    // No session recordings written to disk unless the user starts one explicitly.
    ("allow-auto-record-incoming", "N"),
    ("allow-auto-record-outgoing", "N"),
    // Tether has no "stop service" control. The core's --uninstall-service (run by every
    // upgrade's old uninstaller) writes stop-service=Y, which would keep the app offline for good.
    ("stop-service", "N"),
];

/// Secure defaults the user may relax deliberately in Settings.
const DEFAULTS: &[(&str, &str)] = &[
    // 10-character one-time password (~52 bits) instead of 6.
    ("temporary-password-length", "10"),
];

pub fn apply_policy() {
    let mut d = DEFAULT_SETTINGS.write().unwrap();
    for (k, v) in DEFAULTS {
        d.insert((*k).to_owned(), (*v).to_owned());
    }
    drop(d);
    enforce();
    // No plain ws:// to a server (an IP endpoint would get one); loopback stays for local tests.
    hbb_common::websocket::require_tls();
}

/// Re-applies the forced settings; the admin policy (`policy.rs`) calls this last, so a policy can
/// never weaken them.
pub fn enforce() {
    let mut o = OVERWRITE_SETTINGS.write().unwrap();
    for (k, v) in ENFORCED {
        o.insert((*k).to_owned(), (*v).to_owned());
    }
}

/// `set_option` guard: the admin policy may lock an option or forbid changing the server.
pub fn check_option_write(key: &str) -> Result<(), String> {
    crate::policy::check_option_write(&crate::policy::effective(), key)
}

/// Options the UI may read or write. Anything else is rejected at the IPC boundary so a
/// compromised or buggy webview cannot flip arbitrary core switches.
pub const UI_OPTIONS: &[&str] = &[
    "approve-mode",
    "verification-method",
    "enable-keyboard",
    "enable-clipboard",
    "enable-file-transfer",
    "enable-audio",
    "temporary-password-length",
    "custom-rendezvous-server",
    "relay-server",
    "key",
    "image-quality",
    "codec-preference",
];

pub fn check_option(key: &str) -> Result<(), String> {
    if UI_OPTIONS.contains(&key) {
        Ok(())
    } else {
        Err(format!("option '{key}' is not accessible from the UI"))
    }
}

/// Peer IDs: digits or an alias/`id@server` form, bounded length, no whitespace or control chars.
pub fn check_peer_id(id: &str) -> Result<(), String> {
    let ok = !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '@' | ':'));
    if ok {
        Ok(())
    } else {
        Err("invalid device ID".to_owned())
    }
}

/// A session to this device's own ID would show its screen inside itself and feed its input back.
pub fn check_not_self(peer: &str) -> Result<(), String> {
    // The ID the window shows (status sync), else this profile's config (a `--connect` process).
    let own = Some(librustdesk::ui::api::get_connect_status().id).filter(|i| !i.is_empty()).unwrap_or_else(hbb_common::config::Config::get_id);
    if peer == own {
        return Err("That is this device's own ID. Enter the ID of the device you want to connect to.".to_owned());
    }
    Ok(())
}

/// Permanent passwords: at least 12 characters with at least three character classes.
pub fn check_password_strength(pw: &str) -> Result<(), String> {
    if pw.chars().count() < 12 {
        return Err("Use at least 12 characters.".to_owned());
    }
    if pw.len() > 128 {
        return Err("Use at most 128 characters.".to_owned());
    }
    let classes = [
        pw.chars().any(|c| c.is_lowercase()),
        pw.chars().any(|c| c.is_uppercase()),
        pw.chars().any(|c| c.is_ascii_digit()),
        pw.chars().any(|c| !c.is_alphanumeric()),
    ]
    .iter()
    .filter(|x| **x)
    .count();
    if classes < 3 {
        return Err("Mix at least three of: lowercase, uppercase, digits, symbols.".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_ids() {
        assert!(check_peer_id("123456789").is_ok());
        assert!(check_peer_id("alias@relay.example.com:21116").is_ok());
        assert!(check_peer_id("").is_err());
        assert!(check_peer_id("12 34").is_err());
        assert!(check_peer_id("a;rm -rf").is_err());
        assert!(check_peer_id(&"9".repeat(65)).is_err());
    }

    #[test]
    fn password_strength() {
        assert!(check_password_strength("short1A!").is_err());
        assert!(check_password_strength("alllowercaseletters").is_err());
        assert!(check_password_strength("Correct-horse-42").is_ok());
        assert!(check_password_strength("correct horse battery 9").is_ok());
    }

    #[test]
    fn option_allowlist() {
        assert!(check_option("approve-mode").is_ok());
        assert!(check_option("allow-remote-config-modification").is_err());
        assert!(check_option("2fa").is_err());
    }

    #[test]
    fn policy_applies() {
        apply_policy();
        assert_eq!(OVERWRITE_SETTINGS.read().unwrap().get("enable-check-update").map(String::as_str), Some("N"));
        assert_eq!(DEFAULT_SETTINGS.read().unwrap().get("temporary-password-length").map(String::as_str), Some("10"));
    }
}

//! FluxRT streaming defaults (docs/fluxrt-spec.md). Applied in every process before the core starts.
//! These are defaults, not overrides: users can still pick other values per device.

use hbb_common::config::DEFAULT_DISPLAY_SETTINGS;

const DISPLAY_DEFAULTS: &[(&str, &str)] = &[
    // Viewer-requested frame-rate ceiling. Upstream default is 30, which alone adds up to a full
    // 33 ms frame of waiting; the host's delay-based controller still lowers it on bad links.
    ("custom-fps", "60"),
];

/// Frame-rate ceiling to request for a session. Upstream only sends a custom fps when image
/// quality is "custom", so the host would otherwise fall back to its fixed 30 fps.
pub fn session_fps() -> i32 {
    hbb_common::config::UserDefaultConfig::load()
        .get("custom-fps")
        .parse()
        .unwrap_or(60)
}

pub fn apply_defaults() {
    let mut d = DEFAULT_DISPLAY_SETTINGS.write().unwrap();
    for (k, v) in DISPLAY_DEFAULTS {
        d.insert((*k).to_owned(), (*v).to_owned());
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sixty_fps_default() {
        super::apply_defaults();
        let cfg = hbb_common::config::UserDefaultConfig::load().get("custom-fps");
        assert_eq!(cfg, "60");
    }
}

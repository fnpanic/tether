fn main() {
    // A release build without the pinned server, the policy signing key and the updater key would
    // talk to upstream defaults, accept unsigned policy or never update: refuse to produce one.
    const REQUIRED: [&str; 4] = ["TETHER_SERVER", "TETHER_PUB_KEY", "TETHER_ADMIN_PUB_KEY", "TETHER_UPDATER_PUB_KEY"];
    for var in REQUIRED {
        println!("cargo:rerun-if-env-changed={var}");
    }
    if std::env::var("PROFILE").as_deref() == Ok("release") {
        for var in REQUIRED {
            if std::env::var(var).map(|v| v.trim().is_empty()).unwrap_or(true) {
                panic!("{var} must be set for release builds of TetherQS (see docs/quick-support.md)");
            }
        }
    }
    tauri_build::build()
}

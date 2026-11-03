fn main() {
    // A release build without a pinned server and key would fall back to upstream defaults:
    // refuse to produce one.
    println!("cargo:rerun-if-env-changed=TETHER_SERVER");
    println!("cargo:rerun-if-env-changed=TETHER_PUB_KEY");
    if std::env::var("PROFILE").as_deref() == Ok("release") {
        for var in ["TETHER_SERVER", "TETHER_PUB_KEY"] {
            if std::env::var(var).map(|v| v.trim().is_empty()).unwrap_or(true) {
                panic!("{var} must be set for release builds (see server/install.sh output)");
            }
        }
    }
    tauri_build::build()
}

#[path = "src/instance.rs"]
mod instance;

fn main() {
    // A release build without a pinned server and keys would fall back to upstream defaults, accept
    // no policy and install no updates: refuse to produce one.
    const REQUIRED: [&str; 4] = ["TETHER_SERVER", "TETHER_PUB_KEY", "TETHER_ADMIN_PUB_KEY", "TETHER_UPDATER_PUB_KEY"];
    for var in REQUIRED {
        println!("cargo:rerun-if-env-changed={var}");
    }
    if std::env::var("PROFILE").as_deref() == Ok("release") {
        for var in REQUIRED {
            if std::env::var(var).map(|v| v.trim().is_empty()).unwrap_or(true) {
                panic!("{var} must be set for release builds (see server/install.sh output and the management app)");
            }
        }
    }
    // A second build for another organisation, installed beside the default one: its own product
    // name and so its own service, config folder, firewall rule and policy key (scripts/instance-config.sh).
    println!("cargo:rerun-if-env-changed=TETHER_INSTANCE");
    let inst = match std::env::var("TETHER_INSTANCE") {
        Ok(v) => v,
        Err(std::env::VarError::NotPresent) => String::new(),
        Err(e) => panic!("TETHER_INSTANCE: {e}"),
    };
    if !instance::valid_instance(&inst) {
        panic!("TETHER_INSTANCE must be empty or 1 to 16 characters a-z, 0-9 (got {inst:?})");
    }
    let name = if inst.is_empty() { "Tether".to_owned() } else { format!("Tether-{inst}") };
    println!("cargo:rustc-env=TETHER_APP_NAME={name}");
    println!("cargo:rustc-env=TETHER_INSTANCE={inst}");
    link_clang_rt();
    tauri_build::build()
}

/// ffmpeg's VideoToolbox code (via hwcodec) uses `__builtin_available`, i.e.
/// `___isPlatformVersionAtLeast` from clang's runtime. rustc links with `-nodefaultlibs`, so that
/// library is not linked by itself on macOS: add Apple clang's `libclang_rt.osx.a` (universal).
fn link_clang_rt() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let out = std::process::Command::new("xcrun").args(["clang", "-print-resource-dir"]).output();
    let dir = match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_owned(),
        _ => panic!("xcrun clang -print-resource-dir failed (Xcode Command Line Tools installed?)"),
    };
    let lib = std::path::Path::new(&dir).join("lib/darwin/libclang_rt.osx.a");
    assert!(lib.exists(), "{} not found", lib.display());
    println!("cargo:rustc-link-arg={}", lib.display());
}

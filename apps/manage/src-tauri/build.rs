fn main() {
    // The desktop's sign-in module (src/oidc.rs, included with #[path]) builds its Manage variant.
    println!("cargo:rustc-check-cfg=cfg(tether_manage)");
    println!("cargo:rustc-cfg=tether_manage");
    tauri_build::build()
}

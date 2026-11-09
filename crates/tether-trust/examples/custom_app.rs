//! Test helper for custom TetherQS apps (no Tether Manage needed).
//!
//! `cargo run --example custom_app -- <keyfile> [<exe> <out.exe>]`
//! Creates `<keyfile>` (32-byte Ed25519 seed, keep it private) if missing and prints its public key
//! (build TetherQS with `TETHER_ADMIN_PUB_KEY=<that>`). With `<exe> <out.exe>` it signs a test
//! profile ("Example Support", accent #c8102e, 1x1 PNG logo, approve-mode locked to click) and
//! writes `<out.exe>` = `<exe>` + trailer (inside the signature if signed), like `GET /q/<token>` does. With `--profile <out.json>`
//! it writes the signed profile alone, as `GET /q/m/<mac id>` serves it (macOS test: put it in
//! TetherQS's cache as `custom-<mac id>.json`, docs/macos.md).

use base64::Engine;
use tether_trust::{custom, envelope, policy};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(keyfile) = args.first() else {
        eprintln!("usage: custom_app <keyfile> [<exe> <out.exe>]");
        std::process::exit(2);
    };
    let key = match std::fs::read(keyfile) {
        Ok(b) => tether_trust::SigningKey::from_bytes(&b.try_into().expect("keyfile must hold 32 bytes")),
        Err(_) => {
            let k = tether_trust::generate_key();
            std::fs::write(keyfile, k.to_bytes()).expect("write keyfile");
            k
        }
    };
    println!("{}", tether_trust::key_to_b64(&key.verifying_key()));
    let [_, exe, out] = args.as_slice() else { return };
    // 1x1 transparent PNG.
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
    assert!(base64::engine::general_purpose::STANDARD.decode(png).is_ok());
    let p = custom::CustomProfile {
        id: "example-1".into(),
        name: "Example Support".into(),
        accent: "#c8102e".into(),
        welcome: "Welcome to Example Support.".into(),
        logo_png: png.into(),
        layer: policy::Layer::from([(
            "security.approve-mode".into(),
            policy::Rule { state: policy::State::Locked, value: Some("click".into()) },
        )]),
        issued_at: 1,
    };
    p.validate().expect("valid profile");
    let doc = serde_json::to_vec(&envelope::sign(&serde_json::to_vec(&p).unwrap(), &key)).unwrap();
    if exe == "--profile" {
        std::fs::write(out, &doc).expect("write profile");
        return eprintln!("wrote {out}");
    }
    let bytes = std::fs::read(exe).expect("read exe");
    std::fs::write(out, custom::attach(&bytes, &doc)).expect("write out.exe");
    eprintln!("wrote {out}");
}

//! Test helper: the `Tether-Sig` header of a signed admin request (no Tether Manage needed), for
//! scripts that call a test relay with curl (tools/macbuild/jobs/tether/diagupload.sh).
//!
//! `cargo run --example admin_request -- <keyfile> --pub` creates `<keyfile>` (32-byte Ed25519
//! seed; a throw-away test key, never a real admin key) if missing and prints its public key, for
//! the relay's `ADMIN_PUB_KEY`.
//! `cargo run --example admin_request -- <keyfile> <METHOD> <path> [<body>]` prints the header value
//! for exactly that request (sign it right before sending: the relay accepts it for a few minutes,
//! once).

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(keyfile) = args.first() else {
        eprintln!("usage: admin_request <keyfile> --pub | <METHOD> <path> [<body>]");
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
    match args.get(1).map(String::as_str) {
        Some("--pub") => println!("{}", tether_trust::key_to_b64(&key.verifying_key())),
        Some(method) => {
            let path = args.get(2).expect("path");
            let body = args.get(3).map(String::as_bytes).unwrap_or_default();
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
            println!("{}", tether_trust::reqauth::sign_request(&key, method, path, body, now));
        }
        None => {
            eprintln!("usage: admin_request <keyfile> --pub | <METHOD> <path> [<body>]");
            std::process::exit(2);
        }
    }
}

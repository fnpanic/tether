//! oidc-mock [--config <file.json>] [--listen 127.0.0.1:0] [--seed <u64>] [--now <unix>] [--allow-network [--issuer-host <host>]]
//! Prints `listening on <addr>` and `issuer <url>` on stdout, then serves until killed.

use oidc_mock::{Config, Mock};

fn die(msg: String) -> ! {
    eprintln!("oidc-mock: {msg}");
    std::process::exit(2)
}

fn main() {
    let (mut config, mut listen, mut seed, mut now, mut allow, mut issuer_host) = (None, "127.0.0.1:0".to_owned(), None, None, false, None);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = || args.next().unwrap_or_else(|| die(format!("{a} needs a value")));
        match a.as_str() {
            "--config" => config = Some(val()),
            "--listen" => listen = val(),
            "--seed" => seed = Some(val().parse().unwrap_or_else(|_| die("--seed: a number".into()))),
            "--now" => now = Some(val().parse().unwrap_or_else(|_| die("--now: unix seconds".into()))),
            "--allow-network" => allow = true,
            "--issuer-host" => issuer_host = Some(val()),
            _ => die("usage: oidc-mock [--config <file.json>] [--listen 127.0.0.1:0] [--seed <u64>] [--now <unix>] [--allow-network [--issuer-host <host>]] (see README.md; a test tool with no security)".into()),
        }
    }
    let mut cfg: Config = match config {
        Some(p) => serde_json::from_str(&std::fs::read_to_string(&p).unwrap_or_else(|e| die(format!("{p}: {e}")))).unwrap_or_else(|e| die(format!("{p}: {e}"))),
        None => Config::default(),
    };
    cfg.seed = seed.or(cfg.seed);
    cfg.now = now.or(cfg.now);
    cfg.issuer_host = issuer_host.or(cfg.issuer_host);
    let m = Mock::start(cfg, &listen, allow).unwrap_or_else(|e| die(e));
    eprintln!("oidc-mock: TEST TOOL, NO SECURITY: never make it reachable from a network");
    println!("listening on {}", m.addr());
    println!("issuer {}", m.issuer());
    m.wait();
}

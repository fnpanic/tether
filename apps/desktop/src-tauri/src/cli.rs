//! Command line and `tether://connect/<id>` links. Everything not listed here (`--server`, `--cm`,
//! `--service`, `--install-service`, `--password`, …) is left to the core's `core_main`.

use std::{
    path::PathBuf,
    process::ExitCode,
    sync::{Mutex, OnceLock},
};

use crate::{diag, policy, security, update};

const HELP: &str = "\
Usage: tether [command]

  (no command)                      start Tether
  --connect <id> [password]         open a session to <id> (a positional or --password <pw>
                                    value is visible in the process list: prefer the next two)
      --password-stdin              read the password from the first line of stdin
      --password-env <VAR>          read the password from environment variable VAR
      --file-transfer               open file transfer instead of the remote screen
      --view-only                   do not send mouse or keyboard input
      --relay                       accepted for compatibility, no effect
      --fullscreen                  open the session window in full screen
  --view-camera <id> / --terminal <id>   open the camera view / a terminal instead (same options)
  tether://connect/<id>             same as --connect <id> (links never carry passwords)
  --get-id                          print this device's ID
  --version                         print the version
  --check-update                    check for an update and print the result
  --policy-refresh                  fetch the organisation policy now
  --diagnostics <out.zip>           write a diagnostics archive (logs, settings without secrets)
  --diag-upload <code>              send the diagnostics to support with a ticket code (as Settings ›
                                    Diagnostics › Send to support)
  --help, -h, /?                    show this help

Exit codes: 0 ok, 1 error, 2 bad arguments, 3 offline, 4 policy signature rejected,
5 not allowed by policy, 6 update required.";

#[derive(Debug, Clone, PartialEq)]
pub enum Password {
    Stdin,
    Env(String),
    Value(String),
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Connect {
    pub peer: String,
    /// `default`, `file-transfer`, `view-camera` or `terminal`.
    pub conn: String,
    #[serde(skip)]
    pub password: Option<Password>,
    pub view_only: bool,
    pub relay: bool,
    pub fullscreen: bool,
}

#[derive(Debug, PartialEq)]
pub enum Cmd {
    Connect(Connect),
    GetId,
    Version,
    Help,
    Diagnostics(PathBuf),
    DiagUpload(String),
    CheckUpdate,
    PolicyRefresh,
}

/// `--help`: an instance build names its own binary and link scheme (`tether-<i>`).
pub fn help() -> String {
    let bin = crate::APP_NAME.to_lowercase();
    HELP.replace("Usage: tether ", &format!("Usage: {bin} ")).replace("  tether://", &format!("  {}", uri_prefix()))
}

/// `tether://`, `tether-<i>://` for an instance build (its deep-link scheme, scripts/instance-config.sh).
fn uri_prefix() -> String {
    format!("{}://", crate::APP_NAME.to_lowercase())
}

/// `tether://connect/<id>` (optional trailing `/`). Nothing else: no query, no password.
pub fn parse_uri(uri: &str) -> Result<Connect, String> {
    let prefix = uri_prefix();
    let rest = uri
        .get(..prefix.len())
        .filter(|p| p.eq_ignore_ascii_case(&prefix))
        .map(|_| &uri[prefix.len()..])
        .ok_or_else(|| format!("not a {prefix} link"))?;
    let peer = rest
        .strip_prefix("connect/")
        .map(|p| p.strip_suffix('/').unwrap_or(p))
        .ok_or_else(|| format!("only {prefix}connect/<id> links are supported"))?;
    security::check_peer_id(peer)?;
    Ok(Connect { peer: peer.to_owned(), conn: "default".into(), ..Default::default() })
}

/// `None`: not a Tether command, the core handles it.
pub fn parse(args: &[String]) -> Option<Result<Cmd, String>> {
    let first = args.first()?;
    let prefix = uri_prefix();
    if first.get(..prefix.len()).is_some_and(|p| p.eq_ignore_ascii_case(&prefix)) {
        if args.len() > 1 {
            return Some(Err(format!("a {prefix} link takes no other arguments")));
        }
        return Some(parse_uri(first).map(Cmd::Connect));
    }
    let simple = match first.as_str() {
        "--get-id" => Some(Cmd::GetId),
        "--version" => Some(Cmd::Version),
        "--help" | "-h" | "/?" => Some(Cmd::Help),
        "--check-update" => Some(Cmd::CheckUpdate),
        "--policy-refresh" => Some(Cmd::PolicyRefresh),
        _ => None,
    };
    if let Some(cmd) = simple {
        return Some(if args.len() == 1 { Ok(cmd) } else { Err(format!("{first} takes no arguments")) });
    }
    match first.as_str() {
        "--diagnostics" => Some(match args {
            [_, out] if !out.starts_with("--") => Ok(Cmd::Diagnostics(PathBuf::from(out))),
            _ => Err("usage: --diagnostics <out.zip>".into()),
        }),
        "--diag-upload" => Some(match args {
            [_, code] if !code.starts_with("--") => Ok(Cmd::DiagUpload(code.clone())),
            _ => Err("usage: --diag-upload <code>".into()),
        }),
        "--connect" | "--file-transfer" | "--view-camera" | "--terminal" => Some(parse_connect(args).map(Cmd::Connect)),
        _ => None,
    }
}

fn parse_connect(args: &[String]) -> Result<Connect, String> {
    let peer = args.get(1).filter(|p| !p.starts_with("--")).ok_or("usage: --connect <id> [options]")?;
    let peer = peer.replace(' ', "");
    security::check_peer_id(&peer)?;
    let mut c = Connect { peer, conn: args[0].trim_start_matches("--").replace("connect", "default"), ..Default::default() };
    let mut it = args[2..].iter();
    fn set_pw(c: &mut Connect, p: Password) -> Result<(), String> {
        match c.password.replace(p) {
            Some(_) => Err("give the password only once".to_owned()),
            None => Ok(()),
        }
    }
    while let Some(a) = it.next() {
        match a.as_str() {
            "--password-stdin" => set_pw(&mut c, Password::Stdin)?,
            "--password-env" => {
                let var = it.next().filter(|v| !v.starts_with("--")).ok_or("--password-env needs a variable name")?;
                set_pw(&mut c, Password::Env(var.clone()))?
            }
            // RustDesk spelling, kept for compatibility.
            "--password" => {
                let pw = it.next().ok_or("--password needs a value")?;
                set_pw(&mut c, Password::Value(pw.clone()))?
            }
            "--file-transfer" => c.conn = "file-transfer".into(),
            "--view-only" => c.view_only = true,
            "--relay" => c.relay = true,
            "--fullscreen" => c.fullscreen = true,
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            pw => set_pw(&mut c, Password::Value(pw.to_owned()))?,
        }
    }
    Ok(c)
}

fn read_password(p: &Password) -> Result<String, String> {
    let pw = match p {
        Password::Value(v) => v.clone(),
        Password::Env(var) => std::env::var(var).map_err(|_| format!("environment variable {var} is not set"))?,
        Password::Stdin => {
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).map_err(|e| e.to_string())?;
            line.trim_end_matches(['\r', '\n']).to_owned()
        }
    };
    if pw.len() > 128 {
        return Err("password too long".into());
    }
    Ok(pw)
}

/// The session requested on the command line, with its password, for `app.rs`.
static CONNECT: OnceLock<Mutex<Option<(Connect, Option<String>)>>> = OnceLock::new();

pub fn take_connect() -> Option<(Connect, Option<String>)> {
    CONNECT.get()?.lock().unwrap().take()
}

/// Set for the Tether a `--connect` to this device's own ID starts to say why (`app.rs`).
pub const OWN_ID_ENV: &str = "TETHER_OWN_ID_REFUSED";

/// argv[1] of a session request a second process hands to the running instance (`app.rs`).
pub const FORWARD_ARG: &str = "--forwarded-connect";

/// Session request + password as one hex token: survives the single-instance channel's `|`
/// separators and never appears on a command line (it travels in a local window message).
pub fn encode_forward(c: &Connect, password: Option<&str>) -> String {
    let json = serde_json::json!({ "c": c, "pw": password }).to_string();
    json.bytes().map(|b| format!("{b:02x}")).collect()
}

/// Inverse of [`encode_forward`], with the same checks as the command line.
pub fn decode_forward(hex: &str) -> Result<(Connect, Option<String>), String> {
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|i| hex.get(i..i + 2).and_then(|b| u8::from_str_radix(b, 16).ok()))
        .collect::<Option<Vec<u8>>>()
        .ok_or("bad forwarded request")?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let c: Connect = serde_json::from_value(v["c"].clone()).map_err(|e| e.to_string())?;
    security::check_peer_id(&c.peer)?;
    if !matches!(c.conn.as_str(), "default" | "file-transfer" | "view-camera" | "terminal") {
        return Err("unsupported connection type".into());
    }
    let pw = v["pw"].as_str().map(str::to_owned);
    if pw.as_ref().is_some_and(|p| p.len() > 128) {
        return Err("password too long".into());
    }
    Ok((c, pw))
}

/// Windows release builds have no console; print into the one we were started from.
fn console() {
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS);
    }
}

fn exit_for(err: &str) -> u8 {
    if err.starts_with("offline:") {
        3
    } else if err.contains("untrusted key") || err.contains("signature") {
        4
    } else if err.starts_with("locked:update-required") {
        6
    } else if err.starts_with("locked:") {
        5
    } else {
        1
    }
}

/// Runs Tether's own commands before the core starts. `Some(code)`: exit now.
pub fn run(args: &[String]) -> Option<ExitCode> {
    let cmd = match parse(args)? {
        Ok(c) => c,
        Err(e) => {
            console();
            eprintln!("{e}\n\n{}", help());
            return Some(ExitCode::from(2));
        }
    };
    if !matches!(cmd, Cmd::Connect(_)) {
        console();
    }
    let result: Result<String, String> = match cmd {
        Cmd::Help => Ok(help()),
        Cmd::Version => Ok(update::VERSION.into()),
        Cmd::GetId => Ok(librustdesk::ipc::get_id()),
        Cmd::Diagnostics(out) => diag::export_to(&out).map(|_| format!("wrote {}", out.display())),
        Cmd::DiagUpload(code) => diag::upload(&code).map(|()| "sent".to_owned()),
        Cmd::PolicyRefresh => policy::refresh().map(|_| {
            let e = policy::effective();
            format!("policy serial {} ({})", e.serial, e.sources.join(", "))
        }),
        Cmd::CheckUpdate => update::check().and_then(|s| {
            let line = match s["version"].as_str() {
                Some(v) => format!("update {v} available (installed {})", update::VERSION),
                None => format!("{} is up to date", update::VERSION),
            };
            if update::blocked() {
                Err(format!("locked:update-required: {line}; this version may not be used for sessions"))
            } else {
                Ok(line)
            }
        }),
        Cmd::Connect(c) => {
            // A link or shortcut has no console: a Tether started for it shows the message in the main
            // window, or hands it to the running one (app.rs). Its own start: this one has no server.
            if let Err(e) = security::check_not_self(&c.peer) {
                console();
                eprintln!("{e}");
                if let Ok(exe) = std::env::current_exe() {
                    use std::process::{Command, Stdio};
                    let child = Command::new(exe).env(OWN_ID_ENV, "1").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
                    if let Err(err) = child {
                        hbb_common::log::warn!("own-ID notice: {err}");
                    }
                }
                return Some(ExitCode::from(2));
            }
            let checked = policy::check_outgoing()
                .and_then(|_| c.password.as_ref().map(read_password).transpose());
            return match checked {
                Ok(pw) => {
                    let _ = CONNECT.set(Mutex::new(Some((c, pw))));
                    None
                }
                Err(e) => {
                    console();
                    eprintln!("{e}");
                    Some(ExitCode::from(if e.starts_with("locked:") { exit_for(&e) } else { 2 }))
                }
            };
        }
    };
    Some(match result {
        Ok(out) => {
            println!("{out}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(exit_for(&e))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Option<Result<Cmd, String>> {
        parse(&s.split_whitespace().map(String::from).collect::<Vec<_>>())
    }

    fn connect(s: &str) -> Connect {
        match p(s) {
            Some(Ok(Cmd::Connect(c))) => c,
            other => panic!("{s}: {other:?}"),
        }
    }

    #[test]
    fn connect_forms() {
        let c = connect("--connect 123456789");
        assert_eq!((c.peer.as_str(), c.conn.as_str(), c.password), ("123456789", "default", None));
        assert_eq!(connect("--connect 123456789 hunter2").password, Some(Password::Value("hunter2".into())));
        assert_eq!(connect("--connect 123456789 --password-stdin").password, Some(Password::Stdin));
        assert_eq!(connect("--connect 1234567 --password-env PW").password, Some(Password::Env("PW".into())));
        let c = connect("--connect 123456789 --file-transfer --view-only --relay --fullscreen");
        assert!(c.view_only && c.relay && c.fullscreen);
        assert_eq!(c.conn, "file-transfer");
        // Old form: the connection type as the command.
        assert_eq!(connect("--file-transfer 123456789 pw").conn, "file-transfer");
    }

    #[test]
    fn bad_args_are_errors() {
        for s in [
            "--connect",
            "--connect --view-only",
            "--connect 12;3",
            "--connect 123456789 --bogus",
            "--connect 123456789 a b",
            "--connect 123456789 --password-stdin pw",
            "--connect 123456789 --password-env",
            "--get-id extra",
            "--diagnostics",
            "--diag-upload",
        ] {
            assert!(matches!(p(s), Some(Err(_))), "{s} accepted");
        }
    }

    #[test]
    fn simple_commands_and_core_passthrough() {
        assert_eq!(p("--get-id"), Some(Ok(Cmd::GetId)));
        assert_eq!(p("--version"), Some(Ok(Cmd::Version)));
        assert_eq!(p("--check-update"), Some(Ok(Cmd::CheckUpdate)));
        assert_eq!(p("--policy-refresh"), Some(Ok(Cmd::PolicyRefresh)));
        assert_eq!(p("--diagnostics out.zip"), Some(Ok(Cmd::Diagnostics("out.zip".into()))));
        assert_eq!(p("--diag-upload K7Q2M9XD"), Some(Ok(Cmd::DiagUpload("K7Q2M9XD".into()))));
        for core in ["--server", "--cm", "--service", "--install-service", "--tray", "--password Secret-1234"] {
            assert_eq!(p(core), None, "{core}");
        }
        assert_eq!(p(""), None);
    }

    #[test]
    fn forward_round_trip() {
        let c = connect("--connect 123456789 --file-transfer --view-only");
        let hex = encode_forward(&c, Some("a|b c\"é"));
        assert!(hex.chars().all(|ch| ch.is_ascii_hexdigit()));
        let (d, pw) = decode_forward(&hex).unwrap();
        assert_eq!((d.peer.as_str(), d.conn.as_str(), d.view_only, pw.as_deref()), ("123456789", "file-transfer", true, Some("a|b c\"é")));
        assert_eq!(decode_forward(&encode_forward(&c, None)).unwrap().1, None);
        let bad = Connect { peer: "1;2".into(), conn: "default".into(), ..Default::default() };
        assert!(decode_forward(&encode_forward(&bad, None)).is_err());
        let bad = Connect { conn: "port-forward".into(), ..c.clone() };
        assert!(decode_forward(&encode_forward(&bad, None)).is_err());
        assert!(decode_forward(&encode_forward(&c, Some(&"x".repeat(129)))).is_err());
        assert!(decode_forward("zz").is_err());
    }

    #[test]
    fn uri_prefix_follows_the_app_name() {
        assert_eq!(uri_prefix(), format!("{}://", crate::APP_NAME.to_lowercase()));
        if env!("TETHER_INSTANCE").is_empty() {
            assert_eq!(uri_prefix(), "tether://");
            assert_eq!(help(), HELP);
        } else {
            let i = env!("TETHER_INSTANCE");
            assert!(help().starts_with(&format!("Usage: tether-{i} [command]\n")));
            assert!(help().contains(&format!("\n  tether-{i}://connect/<id>   ")));
            assert!(!help().contains(" tether://"));
        }
    }

    #[test]
    fn uris() {
        let u = |rest: &str| format!("{}{rest}", uri_prefix());
        assert_eq!(connect(&u("connect/123456789")).peer, "123456789");
        assert_eq!(connect(&format!("{}connect/123456789/", uri_prefix().to_uppercase())).peer, "123456789");
        for bad in [
            u("connect/123456789?password=x"),
            u("connect/123456789/x"),
            u("file-transfer/123456789"),
            u("connect/"),
            u("connect/1@2#3"),
        ] {
            assert!(matches!(p(&bad), Some(Err(_))), "{bad} accepted");
        }
        assert!(matches!(p(&u("connect/123456789 pw")), Some(Err(_))));
        // Another build's scheme is not this app's link.
        let other = if env!("TETHER_INSTANCE").is_empty() { "tether-north://" } else { "tether://" };
        assert!(parse_uri(&format!("{other}connect/123456789")).is_err());
    }
}

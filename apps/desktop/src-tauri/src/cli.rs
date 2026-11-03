//! Command line and `tether://connect/<id>` links. Everything not listed here (`--server`, `--cm`,
//! `--service`, `--install-service`, `--password`, …) is left to the core's `core_main`.

use std::{
    path::PathBuf,
    process::ExitCode,
    sync::{Mutex, OnceLock},
};

use crate::{diag, policy, security, update};

pub const HELP: &str = "\
Usage: tether [command]

  (no command)                      start Tether
  --connect <id> [password]         open a session to <id>
      --password-stdin              read the password from the first line of stdin
      --password-env <VAR>          read the password from environment variable VAR
      --file-transfer               open file transfer instead of the remote screen
      --view-only                   do not send mouse or keyboard input
      --relay                       always use the relay (Tether always does)
      --fullscreen                  open the session window in full screen
  tether://connect/<id>             same as --connect <id> (links never carry passwords)
  --get-id                          print this device's ID
  --version                         print the version
  --check-update                    check for an update and print the result
  --policy-refresh                  fetch the organisation policy now
  --diagnostics <out.zip>           write a diagnostics archive (logs, settings without secrets)
  --help                            show this help

Exit codes: 0 ok, 1 error, 2 bad arguments, 3 offline, 4 policy signature rejected,
5 not allowed by policy, 6 update required.";

#[derive(Debug, Clone, PartialEq)]
pub enum Password {
    Stdin,
    Env(String),
    Value(String),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Connect {
    pub peer: String,
    /// `default`, `file-transfer`, `view-camera` or `terminal`.
    pub conn: String,
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
    CheckUpdate,
    PolicyRefresh,
}

const URI_PREFIX: &str = "tether://";

/// `tether://connect/<id>` (optional trailing `/`). Nothing else: no query, no password.
pub fn parse_uri(uri: &str) -> Result<Connect, String> {
    let rest = uri
        .get(..URI_PREFIX.len())
        .filter(|p| p.eq_ignore_ascii_case(URI_PREFIX))
        .map(|_| &uri[URI_PREFIX.len()..])
        .ok_or("not a tether:// link")?;
    let peer = rest
        .strip_prefix("connect/")
        .map(|p| p.strip_suffix('/').unwrap_or(p))
        .ok_or("only tether://connect/<id> links are supported")?;
    security::check_peer_id(peer)?;
    Ok(Connect { peer: peer.to_owned(), conn: "default".into(), ..Default::default() })
}

/// `None`: not a Tether command, the core handles it.
pub fn parse(args: &[String]) -> Option<Result<Cmd, String>> {
    let first = args.first()?;
    if first.len() >= URI_PREFIX.len() && first[..URI_PREFIX.len()].eq_ignore_ascii_case(URI_PREFIX) {
        if args.len() > 1 {
            return Some(Err("a tether:// link takes no other arguments".into()));
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
            eprintln!("{e}\n\n{HELP}");
            return Some(ExitCode::from(2));
        }
    };
    if !matches!(cmd, Cmd::Connect(_)) {
        console();
    }
    let result: Result<String, String> = match cmd {
        Cmd::Help => Ok(HELP.into()),
        Cmd::Version => Ok(update::VERSION.into()),
        Cmd::GetId => Ok(librustdesk::ipc::get_id()),
        Cmd::Diagnostics(out) => diag::export_to(&out).map(|_| format!("wrote {}", out.display())),
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
            let checked = policy::check_outgoing().and_then(|_| c.password.as_ref().map(read_password).transpose());
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
        for core in ["--server", "--cm", "--service", "--install-service", "--tray", "--password Secret-1234"] {
            assert_eq!(p(core), None, "{core}");
        }
        assert_eq!(p(""), None);
    }

    #[test]
    fn uris() {
        assert_eq!(connect("tether://connect/123456789").peer, "123456789");
        assert_eq!(connect("TETHER://connect/123456789/").peer, "123456789");
        for bad in [
            "tether://connect/123456789?password=x",
            "tether://connect/123456789/x",
            "tether://file-transfer/123456789",
            "tether://connect/",
            "tether://connect/1@2#3",
        ] {
            assert!(matches!(p(bad), Some(Err(_))), "{bad} accepted");
        }
        assert!(matches!(p("tether://connect/123456789 pw"), Some(Err(_))));
    }
}

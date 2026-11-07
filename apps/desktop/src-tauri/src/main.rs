#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]

mod app;
mod cli;
mod commands;
mod diag;
mod fluxrt;
mod hosts;
mod keys;
mod netpolicy;
mod policy;
mod recording;
mod security;
mod sink;
mod team;
mod update;

use std::process::ExitCode;

use librustdesk::core_main::{core_main, CoreMainAction};

pub const APP_NAME: &str = "Tether";
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const ORG: &str = "app.tether";
/// Set in the main window's process, inherited by the `--cm` process its server starts (app.rs).
#[cfg(target_os = "macos")]
pub const MAIN_GUI_ENV: &str = "TETHER_MAIN_GUI";

fn main() -> ExitCode {
    // Must run before anything reads config paths, service names or the IPC socket name.
    // TETHER_PROFILE gives a separate identity (config dir, ID, IPC pipe) so a host and a viewer
    // can run side by side on one machine during development.
    let name = match std::env::var("TETHER_PROFILE") {
        Ok(p) if !p.is_empty() => format!("{APP_NAME}-{p}"),
        _ => APP_NAME.to_owned(),
    };
    *hbb_common::config::APP_NAME.write().unwrap() = name;
    #[cfg(target_os = "macos")]
    {
        *hbb_common::config::ORG.write().unwrap() = ORG.to_owned();
    }

    security::apply_policy();
    netpolicy::apply();
    fluxrt::apply_defaults();
    // Cached admin policy (no network) in every role; it never weakens the settings above.
    policy::init();
    diag::apply_verbose_env();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = cli::run(&args) {
        return code;
    }
    #[cfg(target_os = "macos")]
    if args.is_empty() {
        std::env::set_var(MAIN_GUI_ENV, "1");
    }
    if args.first().map(String::as_str) == Some("--server") {
        // The server role enforces the core options: keep its policy current too.
        policy::start(false);
    }

    #[cfg(windows)]
    if let Some(code) = librustdesk::platform::handle_service_control() {
        return code;
    }

    // core_main handles every headless role (--server, --service, --tray, --install-service, …)
    // and hands back the argv of the roles that need a window.
    let code = match core_main() {
        Some(CoreMainAction::StartUi(args)) => {
            app::run(args);
            ExitCode::SUCCESS
        }
        Some(CoreMainAction::ExitFailure(err)) => {
            eprintln!("Command failed: {err}");
            hbb_common::log::error!("Command failed: {err}");
            ExitCode::FAILURE
        }
        None => ExitCode::SUCCESS,
    };
    librustdesk::common::global_clean();
    code
}

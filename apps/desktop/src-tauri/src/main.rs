#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]

mod app;
mod commands;
mod fluxrt;
mod security;
mod sink;

use std::process::ExitCode;

use librustdesk::core_main::{core_main, CoreMainAction};

pub const APP_NAME: &str = "Tether";
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const ORG: &str = "app.tether";

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
    fluxrt::apply_defaults();

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

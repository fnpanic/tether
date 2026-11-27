#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]

mod agent;
mod agent_elevated;
mod app;
mod cli;
mod commands;
mod diag;
mod diag_bundle;
mod fluxrt;
mod hosts;
#[cfg(test)]
mod instance;
mod keys;
mod netpolicy;
mod oidc;
mod policy;
mod recording;
mod security;
mod sink;
mod team;
mod update;

use std::process::ExitCode;

use librustdesk::core_main::{core_main, CoreMainAction};

/// `Tether`, or `Tether-<i>` for an instance build (`TETHER_INSTANCE`, build.rs).
pub const APP_NAME: &str = env!("TETHER_APP_NAME");
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const ORG: &str = "app.tether";
/// Set in the main window's process, inherited by the `--cm` process its server starts (app.rs).
#[cfg(target_os = "macos")]
pub const MAIN_GUI_ENV: &str = "TETHER_MAIN_GUI";

/// Mixed into the MAC-derived ID (hbb_common `Config::auto_id_from_mac`) of an instance build:
/// FNV-1a 64 of the app name, never 0 (0 is the default build). Never change it.
fn instance_id_salt(name: &str) -> u64 {
    name.bytes().fold(0xcbf2_9ce4_8422_2325, |h: u64, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)) | 1
}

fn main() -> ExitCode {
    // Release builds abort on panic (0xc0000409, no message anywhere): log it first.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        hbb_common::log::error!("panic: {info}\n{}", std::backtrace::Backtrace::force_capture());
        hbb_common::log::logger().flush();
        default_hook(info);
    }));
    // Must run before anything reads config paths, service names or the IPC socket name.
    // TETHER_PROFILE gives a separate identity (config dir, ID, IPC pipe) so a host and a viewer
    // can run side by side on one machine during development.
    let name = match std::env::var("TETHER_PROFILE") {
        Ok(p) if !p.is_empty() => format!("{APP_NAME}-{p}"),
        _ => APP_NAME.to_owned(),
    };
    *hbb_common::config::APP_NAME.write().unwrap() = name;
    // An instance build gets its own ID: the default build on the same PC derives the same one
    // from the MAC address, and two apps registering one ID take it from each other.
    if !env!("TETHER_INSTANCE").is_empty() {
        hbb_common::config::AUTO_ID_SALT.store(instance_id_salt(APP_NAME), std::sync::atomic::Ordering::Relaxed);
    }
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
    // A technician's health agent request in a session, wherever this process hosts one.
    librustdesk::ui::agent::set_handler(commands::host_agent);

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
    // macOS launchd roles (unattended access): no tray icon from the core's `--server` loop (as on
    // Windows), no App Nap for the thread that injects input, and make way for an updated bundle.
    #[cfg(target_os = "macos")]
    if matches!(args.first().map(String::as_str), Some("--server" | "--service")) {
        hbb_common::config::BUILTIN_SETTINGS.write().unwrap().insert("hide-tray".to_owned(), "Y".to_owned());
        librustdesk::platform::macos::disable_app_nap();
        librustdesk::platform::macos::exit_when_replaced();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_id_salt_is_stable_and_not_zero() {
        // Pinned: a changed salt gives every installed instance build a new ID.
        assert_eq!(instance_id_salt("Tether-north"), 0xf982_4b2b_f7b9_c7bf);
        assert_ne!(instance_id_salt("Tether-north"), instance_id_salt("Tether-south"));
        assert_ne!(instance_id_salt("Tether-a"), 0);
    }
}

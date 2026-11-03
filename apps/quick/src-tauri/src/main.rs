#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]
//! TetherQS: portable customer module. Same core as the technician app, incoming sessions only,
//! no installation, no service, no tray. The host server and the connection manager both run in
//! this one process (see app.rs).

mod app;
mod commands;
mod netpolicy;
mod policy;
mod security;
mod sink;
mod update;

use std::{path::Path, process::ExitCode};

use hbb_common::config::{Config, LocalConfig};
use librustdesk::core_main::{core_main, CoreMainAction};

/// Config, logs and IPC names live under this name: `%APPDATA%\Tether-QS`, separate from an
/// installed technician app.
pub const APP_NAME: &str = "Tether-QS";
/// macOS config folder prefix (`~/Library/Preferences/app.tether.Tether-QS`), as in the technician app.
#[cfg(target_os = "macos")]
const ORG: &str = "app.tether";
const HANDOFF: &str = "handoff.json";
/// Read by the core at startup: start the elevated portable service (UAC prompt).
const PRE_ELEVATE: &str = "pre-elevate-service";

fn main() -> ExitCode {
    *hbb_common::config::APP_NAME.write().unwrap() = APP_NAME.to_owned();
    #[cfg(target_os = "macos")]
    {
        *hbb_common::config::ORG.write().unwrap() = ORG.to_owned();
    }
    security::apply_policy();
    netpolicy::apply();
    policy::load_cached();

    let ui_start = std::env::args().len() == 1;
    if ui_start {
        #[cfg(windows)]
        if tauri::webview_version().is_err() {
            webview2_missing();
            return ExitCode::FAILURE;
        }
        take_handoff();
    }

    let code = match core_main() {
        Some(CoreMainAction::StartUi(args)) => {
            // The core has read it; a later normal start must not prompt for UAC again.
            if LocalConfig::get_option(PRE_ELEVATE) == "Y" {
                LocalConfig::set_option(PRE_ELEVATE.into(), String::new());
            }
            app::run(args);
            ExitCode::SUCCESS
        }
        Some(CoreMainAction::ExitFailure(err)) => {
            hbb_common::log::error!("Command failed: {err}");
            ExitCode::FAILURE
        }
        None => ExitCode::SUCCESS,
    };
    librustdesk::common::global_clean();
    code
}

/// Start a new instance of `exe` that takes over from this one (same one-time password, so the
/// technician reconnects with what they have), optionally elevating via the core's portable
/// service. Exits this process on success.
pub fn relaunch(exe: &Path, elevate: bool) -> Result<(), String> {
    let h = serde_json::json!({
        "pid": std::process::id(),
        "password": hbb_common::password_security::temporary_password(),
        "at": unix_now(),
    });
    std::fs::write(Config::path(HANDOFF), h.to_string()).map_err(|e| e.to_string())?;
    if elevate {
        LocalConfig::set_option(PRE_ELEVATE.into(), "Y".into());
    }
    if let Err(e) = std::process::Command::new(exe).spawn() {
        let _ = std::fs::remove_file(Config::path(HANDOFF));
        LocalConfig::set_option(PRE_ELEVATE.into(), String::new());
        return Err(format!("could not restart: {e}"));
    }
    std::process::exit(0);
}

/// Second half of [`relaunch`]: wait for the old process to release the IPC names, keep its password.
fn take_handoff() {
    let path = Config::path(HANDOFF);
    let Ok(raw) = std::fs::read_to_string(&path) else { return };
    let _ = std::fs::remove_file(&path);
    let Ok(h) = serde_json::from_str::<serde_json::Value>(&raw) else { return };
    if unix_now().saturating_sub(h["at"].as_u64().unwrap_or(0)) > 60 {
        return;
    }
    if let Some(pid) = h["pid"].as_u64() {
        wait_for_exit(pid as u32);
    }
    if let Some(pw) = h["password"].as_str().filter(|p| !p.is_empty() && p.len() <= 64) {
        *hbb_common::password_security::TEMPORARY_PASSWORD.write().unwrap() = pw.to_owned();
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(windows)]
fn wait_for_exit(pid: u32) {
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE},
    };
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if !h.is_null() {
            WaitForSingleObject(h, 15_000);
            CloseHandle(h);
        }
    }
}

#[cfg(not(windows))]
fn wait_for_exit(_pid: u32) {
    std::thread::sleep(std::time::Duration::from_secs(2));
}

/// Portable exe, no installer to bring the runtime: explain instead of crashing.
#[cfg(windows)]
fn webview2_missing() {
    use windows_sys::Win32::UI::{
        Shell::ShellExecuteW,
        WindowsAndMessaging::{MessageBoxW, IDYES, MB_ICONWARNING, MB_YESNO, SW_SHOWNORMAL},
    };
    let w = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let text = w("TetherQS needs the Microsoft Edge WebView2 Runtime, which is not installed on this computer.\n\n\
        Open the download page now? Install the \"Evergreen Bootstrapper\", then start TetherQS again.\n\n\
        https://go.microsoft.com/fwlink/p/?LinkId=2124703");
    let title = w("TetherQS");
    unsafe {
        if MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_YESNO | MB_ICONWARNING) == IDYES {
            let url = w("https://go.microsoft.com/fwlink/p/?LinkId=2124703");
            ShellExecuteW(std::ptr::null_mut(), w("open").as_ptr(), url.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL);
        }
    }
}

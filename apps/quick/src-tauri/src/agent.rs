//! TetherQS and the health monitoring agent (spec 11): the session handler and this window's
//! pre-dialog for the shared elevated path (`agent_elevated.rs`, the desktop's file). TetherQS has no
//! service and never runs the service's manager: the agent is installed only through the elevation
//! prompt, after the person here said yes, and linked to TetherQS's own ID with TetherQS's own key.
//! `customer.elevate` off, or the machine policy's `health.agent` = `manual`, refuses before any dialog.
#![cfg_attr(not(windows), allow(dead_code))]

use librustdesk::ui::agent::{Action as CoreAction, TetherAgentAnswer};

use crate::agent_elevated::{self as el, Action, AgentAnswer};

fn wire(a: AgentAnswer) -> TetherAgentAnswer {
    TetherAgentAnswer { state: a.state, version: a.version, installed_by: a.installed_by, detail: a.detail, at: a.at, ..Default::default() }
}

/// The installed Tether's machine policy key (the default build's `HKLM\SOFTWARE\Policies\Tether`):
/// TetherQS reads only its `health.agent` rule, nothing else of that layer.
const MACHINE_POLICY_KEY: &str = r"SOFTWARE\Policies\Tether";

/// What the customer profile, the machine policy's `PolicyJson` (`machine`) and this build say.
fn inputs(e: &tether_trust::policy::Effective, machine: Option<&str>) -> el::Inputs {
    let ch = e.get("update.channel");
    // From the signed policy or the signed custom app; the feature's default names no organisation.
    let title = e.get("customer.title");
    let default = tether_trust::policy::feature("customer.title").map_or("", |f| f.default);
    el::Inputs {
        mode: e.get("health.agent").to_owned(),
        grant: e.get("health.enrol-grant").to_owned(),
        channel: if tether_trust::release::valid_channel(ch) { ch.to_owned() } else { "stable".into() },
        admin: crate::policy::trusted_keys(),
        updater_key: crate::update::PUBKEY.unwrap_or_default().to_owned(),
        elevate: e.on("customer.elevate"),
        identity: el::own_identity(),
        org: "TetherQS".into(),
        support_org: if title.trim() == default { String::new() } else { title.to_owned() },
        relays: crate::policy::base_urls().iter().map(|b| b.trim_start_matches("https://").trim_start_matches("http://").to_owned()).collect(),
        machine_off: el::machine_manual(machine),
    }
}

/// The core's handler for a technician's request in a session (set in `main`).
pub fn host_agent(action: CoreAction, viewer: &str) -> TetherAgentAnswer {
    let action = match action {
        CoreAction::Status => Action::Status,
        CoreAction::Install => Action::Install,
        CoreAction::Remove => Action::Remove,
    };
    wire(answer(action, viewer))
}

#[cfg(windows)]
fn answer(action: Action, viewer: &str) -> AgentAnswer {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let machine = el::win::machine_policy(MACHINE_POLICY_KEY);
    el::Host { m: &win::QsMachine, p: inputs(&crate::policy::current(), machine.as_deref()) }.request(action, Some(viewer), now)
}

#[cfg(not(windows))]
fn answer(_action: Action, _viewer: &str) -> AgentAnswer {
    AgentAnswer::only("not-supported", "the health monitoring agent is for Windows")
}

#[cfg(windows)]
mod win {
    use std::path::{Path, PathBuf};

    use tether_trust::release::{self, Candidate};

    use crate::agent_elevated::{self as el, win};

    pub struct QsMachine;

    impl el::Machine for QsMachine {
        fn user_dir(&self) -> PathBuf {
            win::user_dir()
        }
        fn agent_service(&self) -> Result<bool, String> {
            win::service_exists()
        }
        fn removed_at(&self) -> Option<u64> {
            win::removed_at()
        }
        fn installed_exe(&self) -> Result<PathBuf, String> {
            win::installed_exe()
        }
        fn service_image(&self) -> Result<String, String> {
            win::service_image()
        }
        fn installed_version(&self) -> Option<String> {
            win::installed_version()
        }
        fn uac(&self) -> el::Uac {
            // TetherQS elevated itself, or its elevated helper runs: runas would not prompt either.
            el::Uac { elevated_service: crate::commands::elevated(), ..win::uac() }
        }
        fn owner_window(&self) -> usize {
            crate::sink::front_main_window()
        }
        fn fetch_index(&self, channel: &str) -> Vec<(String, Vec<u8>)> {
            let Ok(client) = crate::policy::http() else { return Vec::new() };
            crate::policy::base_urls()
                .into_iter()
                .filter_map(|base| match crate::update::get_index(&client, &base, channel, release::MONITOR_INDEX_FILE) {
                    Ok(Some(raw)) => Some((base, raw)),
                    Ok(None) => None,
                    Err(e) => {
                        hbb_common::log::info!("health agent: {base}: {e}");
                        None
                    }
                })
                .collect()
        }
        fn download(&self, c: &Candidate, key: &str) -> Result<Vec<u8>, String> {
            // the dialog was answered already: the desktop's total deadline, not 30 minutes per source
            crate::update::download_within(c, key, Some(el::download_time(c.size)))
        }
        fn prepare_stage(&self, dir: &Path) -> Result<(), String> {
            win::prepare_stage(dir)
        }
        fn open_locked(&self, path: &Path) -> Result<(Vec<u8>, Box<dyn std::any::Any>), String> {
            win::open_locked(path)
        }
        fn pipe_name(&self) -> Result<String, String> {
            win::pipe_name()
        }
        fn create_pipe(&self, name: &str) -> Result<Box<dyn el::Pipe>, String> {
            win::create_pipe(name)
        }
        fn launch(&self, exe: &Path, args: &[&str], owner: usize) -> Result<Box<dyn el::Child>, el::Launch> {
            win::launch(exe, args, owner)
        }
        fn consent(&self, d: &el::Dialog) -> el::Consent {
            el::ask_window(d.clone(), crate::sink::show_agent_consent, el::CONSENT_WAIT)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tether_trust::policy::{resolve, Bundle, PROFILE_CUSTOMER};

    fn eff(rules: &str) -> tether_trust::policy::Effective {
        let b = Bundle::from_json(format!(r#"{{"serial":1,"issued_at":0,"profiles":{{"customer":{{{rules}}}}}}}"#).as_bytes()).unwrap();
        resolve(Some(&b), PROFILE_CUSTOMER, "u", None)
    }

    #[test]
    fn the_customer_profile_decides_and_customer_elevate_off_refuses() {
        let i = inputs(&eff(r#""health.agent":{"state":"locked","value":"on-request"},"customer.elevate":{"state":"locked","value":"N"},"customer.title":{"state":"locked","value":"Example Practice IT"}"#), None);
        assert_eq!((i.mode.as_str(), i.elevate, i.org.as_str(), i.support_org.as_str(), i.channel.as_str(), i.machine_off), ("on-request", false, "TetherQS", "Example Practice IT", "stable", false));
        // the shared host refuses before any dialog (agent_elevated's tests cover the rest)
        let a = host_agent(CoreAction::Install, "Sam");
        assert!(matches!(a.state.as_str(), "not-allowed" | "not-supported"), "{}", a.state);
        let i = inputs(&eff(r#""health.agent":{"state":"locked","value":"install"}"#), None);
        // default: allowed; the default title names no organisation, so none is shown
        assert!(i.elevate && i.org == "TetherQS" && i.support_org.is_empty(), "{}", i.support_org);
    }

    #[test]
    fn the_machine_policy_manual_switches_it_off_whatever_the_customer_profile_says() {
        let e = eff(r#""health.agent":{"state":"locked","value":"install"}"#);
        assert!(inputs(&e, Some(r#"{"health.agent":{"state":"locked","value":"manual"}}"#)).machine_off);
        assert!(!inputs(&e, Some(r#"{"health.agent":{"state":"locked","value":"on-request"}}"#)).machine_off);
        assert!(!inputs(&e, Some(r#"{"connect.chat":{"state":"off"}}"#)).machine_off);
        assert!(!inputs(&e, None).machine_off);
    }
}

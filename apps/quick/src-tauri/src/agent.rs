//! TetherQS and the health monitoring agent (spec 11): the session handler and this window's
//! pre-dialog for the shared elevated path (`agent_elevated.rs`, the desktop's file). TetherQS has no
//! service and never runs the service's manager: the agent is installed only through the elevation
//! prompt, after the person here said yes, and linked to TetherQS's own ID with TetherQS's own key.
//! `customer.elevate` off refuses before any dialog.
#![cfg_attr(not(windows), allow(dead_code))]

use librustdesk::ui::agent::{Action as CoreAction, TetherAgentAnswer};

use crate::agent_elevated::{self as el, Action, AgentAnswer};

fn wire(a: AgentAnswer) -> TetherAgentAnswer {
    TetherAgentAnswer { state: a.state, version: a.version, installed_by: a.installed_by, detail: a.detail, at: a.at, ..Default::default() }
}

/// What the customer profile and this build say.
fn inputs(e: &tether_trust::policy::Effective) -> el::Inputs {
    let ch = e.get("update.channel");
    let title = e.get("customer.title");
    el::Inputs {
        mode: e.get("health.agent").to_owned(),
        grant: e.get("health.enrol-grant").to_owned(),
        channel: if tether_trust::release::valid_channel(ch) { ch.to_owned() } else { "stable".into() },
        admin: crate::policy::trusted_keys(),
        updater_key: crate::update::PUBKEY.unwrap_or_default().to_owned(),
        elevate: e.on("customer.elevate"),
        identity: el::own_identity(),
        org: if title.trim().is_empty() { "TetherQS".into() } else { title.to_owned() },
        relays: crate::policy::base_urls().iter().map(|b| b.trim_start_matches("https://").trim_start_matches("http://").to_owned()).collect(),
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
    el::Host { m: &win::QsMachine, p: inputs(&crate::policy::current()) }.request(action, Some(viewer), now)
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
        fn installed_exe(&self) -> PathBuf {
            win::installed_exe()
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
            // shortcut: each source may take up to 30 minutes (the update's own limit); a total
            // deadline as in the desktop when a slow relay turns out to matter here.
            crate::update::download(c, key)
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
        fn launch(&self, exe: &Path, args: &[&str]) -> Result<Box<dyn el::Child>, el::Launch> {
            win::launch(exe, args)
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
        let i = inputs(&eff(r#""health.agent":{"state":"locked","value":"on-request"},"customer.elevate":{"state":"locked","value":"N"},"customer.title":{"state":"locked","value":"Example Practice IT"}"#));
        assert_eq!((i.mode.as_str(), i.elevate, i.org.as_str(), i.channel.as_str()), ("on-request", false, "Example Practice IT", "stable"));
        // the shared host refuses before any dialog (agent_elevated's tests cover the rest)
        let a = host_agent(CoreAction::Install, "Sam");
        assert!(matches!(a.state.as_str(), "not-allowed" | "not-supported"), "{}", a.state);
        let i = inputs(&eff(r#""health.agent":{"state":"locked","value":"install"}"#));
        // default: allowed, and the default title names the organisation
        assert!(i.elevate && !i.org.trim().is_empty(), "{}", i.org);
    }
}

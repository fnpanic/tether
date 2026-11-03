//! Policy values read by sessions and recording.

// INTEGRATE: replace with crate::policy::get once policy.rs lands
pub fn get(id: &str) -> String {
    tether_trust::policy::resolve(None, tether_trust::policy::PROFILE_TECHNICIAN, "", None)
        .get(id)
        .to_owned()
}

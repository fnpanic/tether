//! Policy values read by sessions and recording.

pub fn get(id: &str) -> String {
    crate::policy::get(id)
}

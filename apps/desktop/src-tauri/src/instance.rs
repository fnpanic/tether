// Used by build.rs (`#[path]` module) and by the unit tests below; the app itself reads the result
// through env!("TETHER_APP_NAME") / env!("TETHER_INSTANCE").

/// `TETHER_INSTANCE`: empty (the default build) or 1 to 16 characters a-z, 0-9. It becomes part of
/// the product name, the Windows service name (`sc create` unquoted) and paths: nothing else.
pub fn valid_instance(i: &str) -> bool {
    i.len() <= 16 && i.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_values() {
        for ok in ["", "north", "a", "0", "abcdefghij123456"] {
            assert!(valid_instance(ok), "{ok:?} refused");
        }
        for bad in ["North", "a b", "abcdefghij1234567", "../x", "tether-x", " ", "nörd"] {
            assert!(!valid_instance(bad), "{bad:?} accepted");
        }
    }

    #[test]
    fn default_build_keeps_todays_names() {
        if env!("TETHER_INSTANCE").is_empty() {
            assert_eq!(crate::APP_NAME, "Tether");
        } else {
            assert_eq!(crate::APP_NAME, format!("Tether-{}", env!("TETHER_INSTANCE")));
        }
    }
}

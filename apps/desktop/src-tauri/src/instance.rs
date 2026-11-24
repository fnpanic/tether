// Used by build.rs (`#[path]` module) and by the unit tests below; the app itself reads the result
// through env!("TETHER_APP_NAME") / env!("TETHER_INSTANCE").

/// `TETHER_INSTANCE`: empty (the default build) or 1 to 16 characters a-z, 0-9. It becomes part of
/// the product name, the Windows service name (`sc create` unquoted) and paths: nothing else.
pub fn valid_instance(i: &str) -> bool {
    i.len() <= 16 && i.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// Product name and `APP_NAME` of an instance: `Tether`, or `Tether-<i>`.
pub fn app_name(instance: &str) -> String {
    if instance.is_empty() { "Tether".to_owned() } else { format!("Tether-{instance}") }
}

/// Release builds: the Tauri product name must be this instance's, or the setup would install over
/// the other build (same uninstall key and exe) with a different config folder, ID and policy key.
/// `tauri_config` is `TAURI_CONFIG`: only the `--config` JSON (merged), set by the Tauri CLI, absent
/// without `--config`; then tauri.conf.json's `Tether` applies.
pub fn check_product_name(instance: &str, tauri_config: Option<&str>) -> Result<(), String> {
    let want = app_name(instance);
    let got = tauri_config
        .and_then(|c| serde_json::from_str::<serde_json::Value>(c).ok())
        .and_then(|v| v.get("productName")?.as_str().map(str::to_owned))
        .unwrap_or_else(|| "Tether".to_owned());
    if got == want {
        return Ok(());
    }
    let fix = if instance.is_empty() {
        let i = got.strip_prefix("Tether-").unwrap_or("<instance>");
        format!("set TETHER_INSTANCE={i} for --config \"$(bash scripts/instance-config.sh {i})\", or drop that --config")
    } else {
        format!("build with --config \"$(bash scripts/instance-config.sh {instance})\"")
    };
    Err(format!("TETHER_INSTANCE={instance:?} needs productName {want:?}, but the Tauri config gives {got:?}: {fix}"))
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
    fn product_name_must_match_the_instance() {
        let north = r#"{"productName":"Tether-north","mainBinaryName":"tether-north"}"#;
        let other = r#"{"build":{"frontendDist":"/x"}}"#; // build-macos.sh PREBUILT_FRONTEND only
        // Matching.
        assert!(check_product_name("north", Some(north)).is_ok());
        assert!(check_product_name("", None).is_ok());
        assert!(check_product_name("", Some(other)).is_ok());
        assert!(check_product_name("", Some(r#"{"productName":"Tether"}"#)).is_ok());
        // Instance without its --config: absent variable, productName absent, another instance.
        let e = check_product_name("north", None).unwrap_err();
        assert!(e.contains("\"Tether-north\"") && e.contains("\"Tether\""), "{e}");
        assert!(e.contains(r#"--config "$(bash scripts/instance-config.sh north)""#), "{e}");
        assert!(check_product_name("north", Some(other)).is_err());
        assert!(check_product_name("north", Some(r#"{"productName":"Tether-south"}"#)).is_err());
        // Instance --config without TETHER_INSTANCE.
        let e = check_product_name("", Some(north)).unwrap_err();
        assert!(e.contains("\"Tether-north\"") && e.contains("TETHER_INSTANCE=north"), "{e}");
    }

    #[test]
    fn default_build_keeps_todays_names() {
        assert_eq!(app_name(""), "Tether");
        if env!("TETHER_INSTANCE").is_empty() {
            assert_eq!(crate::APP_NAME, "Tether");
        } else {
            assert_eq!(crate::APP_NAME, format!("Tether-{}", env!("TETHER_INSTANCE")));
        }
    }
}

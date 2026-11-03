//! Version ordering: `MAJOR.MINOR.PATCH[-pre]`. A pre-release sorts before its release
//! (`0.3.1-test < 0.3.1`). Missing parts count as 0; non-numeric parts make the version invalid.

use std::cmp::Ordering;

fn parse(v: &str) -> Option<(Vec<u64>, Option<&str>)> {
    let v = v.trim().trim_start_matches('v');
    let (core, pre) = match v.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (v, None),
    };
    let nums = core.split('.').map(|p| p.parse::<u64>().ok()).collect::<Option<Vec<_>>>()?;
    if nums.is_empty() || nums.len() > 4 {
        return None;
    }
    Some((nums, pre))
}

pub fn compare(a: &str, b: &str) -> Option<Ordering> {
    let (mut na, pa) = parse(a)?;
    let (mut nb, pb) = parse(b)?;
    let len = na.len().max(nb.len());
    na.resize(len, 0);
    nb.resize(len, 0);
    Some(na.cmp(&nb).then(match (pa, pb) {
        (None, None) => Ordering::Equal,
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (Some(x), Some(y)) => x.cmp(y),
    }))
}

/// True when `current` is older than `minimum`. An empty or invalid minimum never blocks.
pub fn below_minimum(current: &str, minimum: &str) -> bool {
    !minimum.trim().is_empty() && compare(current, minimum) == Some(Ordering::Less)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordering() {
        assert_eq!(compare("0.3.0", "0.3.1"), Some(Ordering::Less));
        assert_eq!(compare("0.3.1-test", "0.3.1"), Some(Ordering::Less));
        assert_eq!(compare("0.10.0", "0.9.9"), Some(Ordering::Greater));
        assert_eq!(compare("1.0", "1.0.0"), Some(Ordering::Equal));
        assert_eq!(compare("x", "1"), None);
        assert!(below_minimum("0.2.1", "0.3.0"));
        assert!(!below_minimum("0.3.0", "0.3.0"));
        assert!(!below_minimum("0.3.0", ""));
        assert!(!below_minimum("0.3.0", "garbage"));
    }
}

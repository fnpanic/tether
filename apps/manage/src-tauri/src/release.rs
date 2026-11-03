//! Update manifests (`latest.json`, docs/api.md) and the download page.

use serde::Serialize;
use std::collections::BTreeMap;

pub const PLATFORM: &str = "windows-x86_64";

#[derive(Serialize)]
pub struct Platform {
    pub signature: String,
    pub url: String,
}

#[derive(Serialize)]
pub struct Manifest {
    pub version: String,
    pub notes: String,
    pub pub_date: String,
    pub platforms: BTreeMap<String, Platform>,
    pub mandatory: bool,
    #[serde(rename = "minimumVersion")]
    pub minimum_version: String,
    pub forced: bool,
}

/// Remote file name and directory for a release kind.
pub fn target(kind: &str, version: &str) -> Result<(String, String), String> {
    match kind {
        "technician" => Ok((format!("technician/{PLATFORM}"), format!("Tether_{version}_x64-setup.exe"))),
        "customer" => Ok((format!("customer/{PLATFORM}"), "TetherQS.exe".into())),
        _ => Err(format!("unknown release kind {kind:?}")),
    }
}

pub fn valid_version(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 32
        && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        && tether_trust::version::compare(v, v).is_some()
}

pub fn valid_channel(c: &str) -> bool {
    (1..=32).contains(&c.len()) && c.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// RFC 3339 UTC timestamp (`2026-11-04T10:00:00Z`) without a date crate.
pub fn rfc3339(unix: u64) -> String {
    let (days, secs) = ((unix / 86_400) as i64, unix % 86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", secs / 3600, secs / 60 % 60, secs % 60)
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub struct Offer {
    pub label: &'static str,
    pub detail: String,
    pub href: String,
}

/// Self-contained page in the DESIGN.md look: no external resources, light and dark.
pub fn download_page(title: &str, intro: &str, offers: &[Offer]) -> String {
    let cards: String = offers
        .iter()
        .map(|o| {
            format!(
                "<a class=\"card\" href=\"{}\"><span class=\"label\">{}</span><span class=\"detail\">{}</span><span class=\"cta\">Download</span></a>",
                esc(&o.href),
                esc(o.label),
                esc(&o.detail)
            )
        })
        .collect();
    let intro: String = intro.split("\n\n").filter(|p| !p.trim().is_empty()).map(|p| format!("<p>{}</p>", esc(p.trim()))).collect();
    format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>{title}</title>
<style>
:root{{--canvas:#fff;--parchment:#f5f5f7;--tile:#fff;--ink:#1d1d1f;--muted:#6e6e73;--hairline:rgba(0,0,0,.08);--accent:#0066cc;--focus:#0071e3}}
@media (prefers-color-scheme:dark){{:root{{--canvas:#000;--parchment:#1c1c1e;--tile:#272729;--ink:#f5f5f7;--muted:#98989d;--hairline:rgba(255,255,255,.1);--accent:#2997ff;--focus:#2997ff}}}}
*{{box-sizing:border-box}}
body{{margin:0;background:var(--canvas);color:var(--ink);font-family:-apple-system,"SF Pro Text","Segoe UI Variable","Segoe UI",system-ui,sans-serif;-webkit-font-smoothing:antialiased}}
main{{max-width:980px;margin:0 auto;padding:80px 32px}}
h1{{font-size:40px;line-height:1.1;letter-spacing:-.02em;font-weight:600;margin:0 0 16px}}
p{{font-size:17px;line-height:1.47;letter-spacing:-.022em;color:var(--muted);margin:0 0 12px;max-width:640px}}
.grid{{display:grid;grid-template-columns:repeat(auto-fill,minmax(280px,1fr));gap:24px;margin-top:48px}}
.card{{display:flex;flex-direction:column;gap:8px;padding:24px;border:1px solid var(--hairline);border-radius:18px;background:var(--tile);color:var(--ink);text-decoration:none}}
.card:hover{{background:var(--parchment)}}
.card:focus-visible{{outline:2px solid var(--focus);outline-offset:2px}}
.label{{font-size:21px;line-height:1.19;font-weight:600}}
.detail{{font-size:14px;line-height:1.43;color:var(--muted)}}
.cta{{align-self:flex-start;margin-top:16px;background:var(--accent);color:#fff;border-radius:9999px;padding:10px 22px;font-size:15px}}
footer{{margin-top:80px;font-size:12px;color:var(--muted)}}
</style></head>
<body><main><h1>{title}</h1>{intro}<div class="grid">{cards}</div><footer>Downloads are signed and served over HTTPS.</footer></main></body></html>
"#,
        title = esc(title)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_matches_api_doc() {
        let mut platforms = BTreeMap::new();
        platforms.insert(PLATFORM.to_owned(), Platform { signature: "SIG".into(), url: "https://h/download/stable/technician/windows-x86_64/Tether_0.3.1_x64-setup.exe".into() });
        let m = Manifest {
            version: "0.3.1".into(),
            notes: "n".into(),
            pub_date: rfc3339(1_793_786_400),
            platforms,
            mandatory: false,
            minimum_version: String::new(),
            forced: false,
        };
        let v: serde_json::Value = serde_json::to_value(&m).unwrap();
        let expected = serde_json::json!({"version":"0.3.1","notes":"n","pub_date":"2026-11-04T10:00:00Z",
            "platforms":{"windows-x86_64":{"signature":"SIG","url":"https://h/download/stable/technician/windows-x86_64/Tether_0.3.1_x64-setup.exe"}},
            "mandatory":false,"minimumVersion":"","forced":false});
        assert_eq!(v, expected);
    }

    #[test]
    fn dates_targets_and_validation() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(target("technician", "0.3.1").unwrap().1, "Tether_0.3.1_x64-setup.exe");
        assert_eq!(target("customer", "0.3.1").unwrap(), ("customer/windows-x86_64".into(), "TetherQS.exe".into()));
        assert!(valid_version("0.3.1") && valid_version("0.3.1-test") && !valid_version("0.3.x") && !valid_version("../1"));
        assert!(valid_channel("stable") && !valid_channel("Stable") && !valid_channel(""));
    }

    #[test]
    fn page_escapes_input() {
        let html = download_page("<b>", "a\n\nb", &[Offer { label: "Tether", detail: "x".into(), href: "/download/a\"b".into() }]);
        assert!(html.contains("<title>&lt;b&gt;</title>") && html.contains("<p>a</p><p>b</p>") && html.contains("a&quot;b"));
        assert!(!html.contains("http://") && !html.contains("<script"));
    }
}

//! Update manifests (`latest.json`, docs/api.md), the signed release index (`releases.json`,
//! docs/update-design.md) and the download page.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tether_trust::release::{self as rel, Entry, Index, Packed, Rollout, NAMES};
// The name table is shared with tether-api (prune): tether_trust::release.
pub use tether_trust::release::{build_of, dmg_name, wants_packed, MAC, WINDOWS};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Platform {
    pub signature: String,
    pub url: String,
}

#[derive(Serialize, Deserialize)]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub pub_date: String,
    #[serde(default)]
    pub platforms: BTreeMap<String, Platform>,
    #[serde(default)]
    pub mandatory: bool,
    #[serde(rename = "minimumVersion", default)]
    pub minimum_version: String,
    #[serde(default)]
    pub forced: bool,
}

/// Remote directory and file name of a release. Versioned names: an upload never replaces the
/// file the current latest.json points to.
pub fn target(kind: &str, platform: &str, version: &str) -> Result<(String, String), String> {
    let (_, _, pre, suf) = NAMES
        .iter()
        .find(|n| (n.0, n.1) == (kind, platform))
        .ok_or_else(|| format!("unknown release kind {kind:?} or platform {platform:?}"))?;
    Ok((format!("{kind}/{platform}"), format!("{pre}{version}{suf}")))
}

/// Version of a release artifact from its path below the channel (`technician/windows-x86_64/
/// Tether_0.3.1_x64-setup.exe`); `None` for signatures, manifests, disk images and other files.
pub fn version_of(path: &str) -> Option<(&'static str, &'static str, String)> {
    let mut it = path.split('/');
    let (kind, platform, file) = (it.next()?, it.next()?, it.next()?);
    if it.next().is_some() {
        return None;
    }
    build_of(file).filter(|b| (b.0, b.1) == (kind, platform))
}

/// Highest version per (kind, platform) among local build files: `(kind, platform) -> (version, path)`.
pub fn latest_builds(paths: impl IntoIterator<Item = std::path::PathBuf>) -> BTreeMap<(&'static str, &'static str), (String, std::path::PathBuf)> {
    let mut out: BTreeMap<_, (String, std::path::PathBuf)> = BTreeMap::new();
    for path in paths {
        let Some((k, p, v)) = path.file_name().and_then(|n| n.to_str()).and_then(build_of) else { continue };
        match out.get(&(k, p)) {
            Some((cur, _)) if tether_trust::version::compare(&v, cur) != Some(std::cmp::Ordering::Greater) => {}
            _ => {
                out.insert((k, p), (v, path));
            }
        }
    }
    out
}

/// Highest artifact version per (channel, kind, platform) over the file listings of every relay
/// (`GET /api/admin/releases`: `(channel, path)` pairs). "Latest" means uploaded, not published.
pub fn latest_versions<'a>(files: impl IntoIterator<Item = (&'a str, &'a str)>) -> BTreeMap<(String, &'static str, &'static str), String> {
    let mut out: BTreeMap<(String, &'static str, &'static str), String> = BTreeMap::new();
    for (channel, path) in files {
        let Some((k, p, v)) = version_of(path) else { continue };
        let e = out.entry((channel.to_owned(), k, p)).or_insert_with(|| v.clone());
        if tether_trust::version::compare(&v, e) == Some(std::cmp::Ordering::Greater) {
            *e = v;
        }
    }
    out
}

/// Manifest keys a release is listed under. A universal mac build also answers the per-arch keys,
/// so any Tauri updater on a Mac finds it whichever `{{target}}` it asks for.
pub fn platform_keys(platform: &str) -> &'static [&'static str] {
    if platform == MAC {
        &[MAC, "darwin-aarch64", "darwin-x86_64"]
    } else {
        &[WINDOWS]
    }
}

/// Platforms for a new manifest: entries of the `existing` one for the same version are kept, so
/// publishing one OS never drops another OS's build of that version. Another version's entries
/// are dropped (they would point at other files).
pub fn merge_platforms(existing: Option<&[u8]>, version: &str, new: BTreeMap<String, Platform>) -> BTreeMap<String, Platform> {
    let mut out = existing
        .and_then(|b| serde_json::from_slice::<Manifest>(b).ok())
        .filter(|m| m.version == version)
        .map(|m| m.platforms)
        .unwrap_or_default();
    out.extend(new);
    out
}

/// Key of a release in the index: `technician/windows-x86_64`.
pub fn index_key(kind: &str, platform: &str) -> String {
    format!("{kind}/{platform}")
}

/// The ramp of a new release from `start`: a preset (`standard`, the D4 default; `fast`; `now`) or
/// `custom:<hours>=<percent>,…` (e.g. `custom:0=5,2=25,12=100`).
pub fn rollout(preset: &str, start: u64) -> Result<Rollout, String> {
    if let Some(list) = preset.strip_prefix("custom:") {
        return Ok(Rollout { start, steps: custom_ramp(list)? });
    }
    let steps = rel::preset(if preset.is_empty() { "standard" } else { preset }).ok_or_else(|| format!("unknown rollout preset {preset:?}"))?;
    Ok(Rollout { start, steps: steps.to_vec() })
}

/// Longest custom ramp: 30 days to 100 %.
const CUSTOM_MAX_HOURS: f64 = 720.0;

/// `<hours>=<percent>,…`: starts at 0 h, hours and percents strictly rising, percent 1-100, ending
/// at 100 % within [`CUSTOM_MAX_HOURS`]; at most 16 steps (what clients accept). Hours may have a
/// fraction (whole minutes).
fn custom_ramp(list: &str) -> Result<Vec<(u64, u8)>, String> {
    let bad = |why: &str| format!("custom rollout {list:?}: {why}");
    let mut steps: Vec<(u64, u8)> = Vec::new();
    for part in list.split(',') {
        let (h, p) = part.split_once('=').ok_or_else(|| bad("expected hours=percent steps"))?;
        let h: f64 = h.trim().parse().map_err(|_| bad("hours must be a number"))?;
        let p: u8 = p.trim().parse().map_err(|_| bad("percent must be a whole number"))?;
        if !(0.0..=CUSTOM_MAX_HOURS).contains(&h) {
            return Err(bad("hours must be 0 to 720 (30 days)"));
        }
        if !(1..=100).contains(&p) {
            return Err(bad("percent must be 1 to 100"));
        }
        let secs = (h * 60.0).round() as u64 * 60;
        if let Some(&(s, q)) = steps.last() {
            if secs <= s || p <= q {
                return Err(bad("hours and percents must rise from step to step"));
            }
        } else if secs != 0 {
            return Err(bad("the first step starts at 0 hours"));
        }
        steps.push((secs, p));
    }
    if steps.len() > 16 {
        return Err(bad("at most 16 steps"));
    }
    if steps.last().map(|s| s.1) != Some(100) {
        return Err(bad("the last step must reach 100 %"));
    }
    Ok(steps)
}

pub fn gzip(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    e.write_all(data).expect("in memory");
    e.finish().expect("in memory")
}

/// One index entry. `cdn`: the CDN base when this release's file is on it. The relay URL is always
/// last (`{relay}`: the relay the index came from), so a blocked CDN falls back to it.
#[allow(clippy::too_many_arguments)]
pub fn entry(channel: &str, dir: &str, name: &str, data: &[u8], signature: &str, packed: Option<&[u8]>, cdn: Option<&str>, m: &Manifest, security: bool, rollout: Rollout) -> Entry {
    let at = |base: &str, f: &str| format!("{}/{channel}/{dir}/{f}", base.trim_end_matches('/'));
    let relay = |f: &str| format!("https://{{relay}}/download/{channel}/{dir}/{f}");
    let gz = format!("{name}.gz");
    let urls = match (cdn, packed) {
        (Some(b), None) => vec![at(b, name), relay(name)],
        _ => vec![relay(name)],
    };
    Entry {
        version: m.version.clone(),
        file: name.to_owned(),
        size: data.len() as u64,
        sha256: rel::sha256_hex(data),
        signature: signature.to_owned(),
        urls,
        notes: m.notes.clone(),
        mandatory: m.mandatory,
        minimum_version: m.minimum_version.clone(),
        forced: m.forced,
        security,
        rollout,
        halted: false,
        packed: packed.map(|p| Packed {
            encoding: "gzip".into(),
            size: p.len() as u64,
            sha256: rel::sha256_hex(p),
            urls: cdn.map(|b| at(b, &gz)).into_iter().chain([relay(&gz)]).collect(),
        }),
    }
}

/// The next index: `base` (the highest verified one on the relays) with `change` applied, a new
/// serial, issued now, valid for 30 days (D3).
pub fn next_index(base: Option<Index>, channel: &str, serial: u64, now: u64, change: impl FnOnce(&mut BTreeMap<String, Entry>)) -> Result<Index, String> {
    let mut releases = base.map(|b| b.releases).unwrap_or_default();
    change(&mut releases);
    let ix = Index { kind: rel::INDEX_TYPE.into(), channel: channel.into(), serial, issued: now, expires: now + rel::EXPIRY_SECS, releases };
    ix.validate()?;
    Ok(ix)
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
    /// `None`: no release for this OS yet.
    pub href: Option<String>,
}

pub struct OsSection {
    /// `windows` or `macos`: element id, switch target, and what the page script detects.
    pub id: &'static str,
    pub name: &'static str,
    pub offers: Vec<Offer>,
}

/// Picks the visitor's OS and wires the switch. Without it every section stays visible and the
/// switch tabs jump to them, so all downloads work without JavaScript.
const SCRIPT: &str = r#"(function(){var d=document.documentElement,u=navigator.userAgentData,p=(u&&u.platform)||navigator.platform||navigator.userAgent||"";
var mine=/mac/i.test(p)?"macos":/win/i.test(p)?"windows":"";
var tabs=document.querySelectorAll(".switch a");
function pick(os){d.setAttribute("data-os",os);for(var i=0;i<tabs.length;i++){tabs[i].setAttribute("aria-current",tabs[i].getAttribute("data-os")===os?"true":"false")}}
for(var i=0;i<tabs.length;i++){tabs[i].addEventListener("click",function(e){e.preventDefault();pick(this.getAttribute("data-os"))})}
if(mine&&document.getElementById(mine)){d.setAttribute("data-mine",mine);pick(mine)}})();"#;

/// Self-contained page in the DESIGN.md look: no external resources, light and dark. One section
/// per OS; the inline script shows the visitor's and marks it, the switch shows the other.
pub fn download_page(title: &str, intro: &str, sections: &[OsSection]) -> String {
    // One card per app with one download; signatures and updater archives are for the updater
    // (Tether Manage lists them under Releases), not for visitors.
    let card = |o: &Offer| match &o.href {
        Some(href) => format!(
            "<div class=\"card\"><span class=\"label\">{}</span><span class=\"detail\">{}</span><a class=\"cta\" href=\"{}\">Download</a></div>",
            esc(o.label),
            esc(&o.detail),
            esc(href)
        ),
        None => format!("<div class=\"card off\"><span class=\"label\">{}</span><span class=\"detail\">Not available yet</span></div>", esc(o.label)),
    };
    let switch: String = sections.iter().map(|s| format!("<a href=\"#{0}\" data-os=\"{0}\">{1}</a>", s.id, esc(s.name))).collect();
    let body: String = sections
        .iter()
        .map(|s| {
            let cards: String = s.offers.iter().map(card).collect();
            format!("<section class=\"os\" id=\"{}\"><h2>{}<span class=\"mine\">Your computer</span></h2><div class=\"grid\">{cards}</div></section>", s.id, esc(s.name))
        })
        .collect();
    // With an OS picked, only its section shows; the visitor's own one is marked.
    let os_css: String = sections
        .iter()
        .map(|s| {
            format!(
                "html[data-os={0}] #{0}{{display:block}}html[data-os={0}] .switch a[data-os={0}]{{background:var(--ink);color:var(--canvas)}}html[data-mine={0}] #{0} .mine{{display:inline-block}}",
                s.id
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
.card.off{{opacity:.6}}
a:focus-visible{{outline:2px solid var(--focus);outline-offset:2px}}
.label{{font-size:21px;line-height:1.19;font-weight:600}}
.detail{{font-size:14px;line-height:1.43;color:var(--muted)}}
.detail{{margin-bottom:16px}}
.cta{{align-self:flex-start;margin-top:auto;background:var(--accent);color:#fff;border-radius:9999px;padding:10px 22px;font-size:15px;text-decoration:none}}
.cta:hover{{filter:brightness(1.1)}}
.switch{{display:inline-flex;gap:4px;margin-top:32px;padding:4px;border-radius:9999px;background:var(--parchment)}}
.switch a{{padding:8px 18px;border-radius:9999px;color:var(--ink);text-decoration:none;font-size:15px}}
.switch a[aria-current=true]{{font-weight:600}}
.switch a:focus-visible{{outline:2px solid var(--focus);outline-offset:2px}}
.os{{margin-top:48px}}
html[data-os] .os{{display:none}}
h2{{font-size:28px;line-height:1.14;letter-spacing:-.01em;font-weight:600;margin:0;display:flex;align-items:center;gap:12px}}
.mine{{display:none;font-size:12px;font-weight:600;letter-spacing:0;color:var(--accent);border:1px solid currentColor;border-radius:9999px;padding:2px 10px}}
.os .grid{{margin-top:24px}}
{os_css}
footer{{margin-top:80px;font-size:12px;color:var(--muted)}}
</style></head>
<body><main><h1>{title}</h1>{intro}<nav class="switch" aria-label="Operating system">{switch}</nav>{body}<footer>Downloads are signed and served over HTTPS.</footer></main><script>{SCRIPT}</script></body></html>
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
        platforms.insert(WINDOWS.to_owned(), Platform { signature: "SIG".into(), url: "https://h/download/stable/technician/windows-x86_64/Tether_0.3.1_x64-setup.exe".into() });
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
        assert_eq!(target("technician", WINDOWS, "0.3.1").unwrap().1, "Tether_0.3.1_x64-setup.exe");
        assert_eq!(target("customer", WINDOWS, "0.3.1").unwrap(), ("customer/windows-x86_64".into(), "TetherQS-0.3.1.exe".into()));
        assert_eq!(target("technician", MAC, "0.3.1").unwrap(), ("technician/darwin-universal".into(), "Tether_0.3.1_universal.app.tar.gz".into()));
        assert_eq!(target("customer", MAC, "0.3.1").unwrap().1, "TetherQS-0.3.1-universal.zip");
        assert!(target("technician", "linux-x86_64", "0.3.1").is_err() && target("x", WINDOWS, "1").is_err());
        assert_eq!(dmg_name("0.3.1"), "Tether_0.3.1_universal.dmg");
        assert_eq!(platform_keys(MAC), ["darwin-universal", "darwin-aarch64", "darwin-x86_64"]);
        assert_eq!(platform_keys(WINDOWS), ["windows-x86_64"]);
        assert!(valid_version("0.3.1") && valid_version("0.3.1-test") && !valid_version("0.3.x") && !valid_version("../1"));
        assert!(valid_channel("stable") && !valid_channel("Stable") && !valid_channel(""));
    }

    fn p(url: &str) -> Platform {
        Platform { signature: "S".into(), url: url.into() }
    }

    #[test]
    fn merge_keeps_same_version_platforms_only() {
        let existing = br#"{"version":"0.3.1","platforms":{"windows-x86_64":{"signature":"W","url":"w"},"darwin-universal":{"signature":"old","url":"old"}}}"#;
        let new: BTreeMap<_, _> = platform_keys(MAC).iter().map(|k| (k.to_string(), p("mac"))).collect();
        let m = merge_platforms(Some(existing), "0.3.1", new.clone());
        assert_eq!(m.keys().collect::<Vec<_>>(), ["darwin-aarch64", "darwin-universal", "darwin-x86_64", "windows-x86_64"]);
        assert_eq!(m["windows-x86_64"].url, "w");
        assert_eq!(m["darwin-universal"].url, "mac");
        // Another version, no manifest, or garbage: only the new entries.
        assert_eq!(merge_platforms(Some(existing), "0.3.2", new.clone()), new);
        assert_eq!(merge_platforms(None, "0.3.1", new.clone()), new);
        assert_eq!(merge_platforms(Some(b"<html>"), "0.3.1", new.clone()), new);
        // A merged manifest still has the api.md shape.
        let back: Manifest = serde_json::from_slice(existing).unwrap();
        assert_eq!((back.notes.as_str(), back.mandatory, back.minimum_version.as_str()), ("", false, ""));
    }

    fn sections(mac: Option<&str>) -> Vec<OsSection> {
        let offer = |label, href: Option<&str>| Offer {
            label,
            detail: "d".into(),
            href: href.map(Into::into),
        };
        vec![
            OsSection { id: "windows", name: "Windows", offers: vec![offer("Tether", Some("/download/stable/technician/windows-x86_64/a\"b.exe")), offer("QS", Some("/w.exe"))] },
            OsSection { id: "macos", name: "macOS", offers: vec![offer("Tether", mac), offer("QS", None)] },
        ]
    }

    #[test]
    fn page_escapes_input() {
        let html = download_page("<b>", "a\n\nb", &sections(None));
        assert!(html.contains("<title>&lt;b&gt;</title>") && html.contains("<p>a</p><p>b</p>") && html.contains("a&quot;b.exe"));
        // Self-contained: no external scripts, styles or fonts.
        assert!(!html.contains("http://") && !html.contains("https://") && !html.contains(" src=") && !html.contains("<link"));
    }

    #[test]
    fn page_has_a_section_and_switch_per_os() {
        let html = download_page("T", "", &sections(Some("/download/stable/technician/darwin-universal/Tether_0.3.1_universal.dmg")));
        for id in ["windows", "macos"] {
            assert!(html.contains(&format!("<section class=\"os\" id=\"{id}\">")));
            // Works without JS: a plain anchor link to every section.
            assert!(html.contains(&format!("<a href=\"#{id}\" data-os=\"{id}\">")));
            assert!(html.contains(&format!("html[data-os={id}] #{id}{{display:block}}")));
        }
        assert!(html.contains("href=\"/download/stable/technician/darwin-universal/Tether_0.3.1_universal.dmg\""));
        // TetherQS for macOS is not released: no link, a notice instead.
        assert_eq!(html.matches("Not available yet").count(), 1);
        assert_eq!(html.matches("class=\"cta\"").count(), 3);
        assert!(html.contains("navigator.userAgentData") && html.contains("<script>"));
        // Sections are hidden only once the script picked an OS.
        assert!(html.contains("html[data-os] .os{display:none}") && !html.contains("\n.os{display:none}"));
    }

    #[test]
    fn page_one_button_per_card_at_the_same_height() {
        let dmg = "/download/stable/technician/darwin-universal/Tether_0.3.1_universal.dmg";
        let html = download_page("T", "", &sections(Some(dmg)));
        assert_eq!(html.matches("<div class=\"card\">").count(), 3);
        let mac = &html[html.find("id=\"macos\"").unwrap()..];
        let card = &mac[..mac.find("</div>").unwrap()];
        assert_eq!(card.matches("<a ").count(), 1);
        assert!(card.contains(&format!("class=\"cta\" href=\"{dmg}\"")));
        // No signatures or updater archives for visitors.
        assert!(!html.contains(".sig") && !html.contains("<details"));
        // Cards stretch to the row height; the button sits at the bottom of each.
        assert!(html.contains(".cta{align-self:flex-start;margin-top:auto;"));
    }

    fn manifest(v: &str) -> Manifest {
        Manifest { version: v.into(), notes: "n".into(), pub_date: String::new(), platforms: BTreeMap::new(), mandatory: false, minimum_version: String::new(), forced: false }
    }

    #[test]
    fn index_entries_list_cdn_then_relay() {
        let data = b"installer";
        let e = entry("stable", "technician/windows-x86_64", "Tether_0.5.24_x64-setup.exe", data, "SIG", None, Some("https://tether-dl.x.workers.dev/"), &manifest("0.5.24"), false, rollout("", 100).unwrap());
        assert_eq!(e.urls, ["https://tether-dl.x.workers.dev/stable/technician/windows-x86_64/Tether_0.5.24_x64-setup.exe", "https://{relay}/download/stable/technician/windows-x86_64/Tether_0.5.24_x64-setup.exe"]);
        assert_eq!((e.size, e.sha256.len(), e.rollout.steps.len()), (9, 64, 4));
        e.validate().unwrap();
        // TetherQS: the plain exe only from the relays; the gzip copy from the CDN, then the relay.
        let gz = gzip(b"TetherQS exe");
        let q = entry("stable", "customer/windows-x86_64", "TetherQS-0.5.24.exe", b"TetherQS exe", "SIG", Some(&gz), Some("https://cdn"), &manifest("0.5.24"), true, rollout("now", 0).unwrap());
        assert_eq!(q.urls, ["https://{relay}/download/stable/customer/windows-x86_64/TetherQS-0.5.24.exe"]);
        let p = q.packed.as_ref().unwrap();
        assert_eq!(p.urls, ["https://cdn/stable/customer/windows-x86_64/TetherQS-0.5.24.exe.gz", "https://{relay}/download/stable/customer/windows-x86_64/TetherQS-0.5.24.exe.gz"]);
        assert!(q.security);
        q.validate().unwrap();
        // gzip round trip.
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(&gz[..]), &mut out).unwrap();
        assert_eq!(out, b"TetherQS exe");
        assert!(wants_packed("customer", WINDOWS) && !wants_packed("technician", WINDOWS) && !wants_packed("customer", MAC));
        assert!(rollout("bogus", 0).is_err());
    }

    #[test]
    fn custom_ramps_are_validated() {
        let r = rollout("custom:0=5, 1.5=25,12=100", 50).unwrap();
        assert_eq!((r.start, r.steps.clone()), (50, vec![(0, 5), (5400, 25), (43_200, 100)]));
        assert_eq!(rollout("custom:0=100", 0).unwrap().steps, [(0, 100)]);
        assert_eq!(rollout("custom:0=1,0.01=100", 0).unwrap().steps, [(0, 1), (60, 100)], "whole minutes");
        for bad in ["custom:", "custom:1=10,2=100", "custom:0=0,1=100", "custom:0=10,1=101", "custom:0=10,1=50", "custom:0=10,2=50,1=100", "custom:0=50,1=50,2=100", "custom:0=10,721=100", "custom:0=10,-1=100", "custom:0=10,x=100", "custom:0=10;1=100", "custom:0=10,0.001=100", "custom:0=1.5,1=100"] {
            assert!(rollout(bad, 0).is_err(), "{bad}");
        }
        let many: Vec<String> = (0..17).map(|i| format!("{i}={}", if i == 16 { 100 } else { i + 1 })).collect();
        assert!(rollout(&format!("custom:{}", many.join(",")), 0).is_err());
        // What clients accept (tether_trust validates the signed entry).
        let e = entry("stable", "technician/windows-x86_64", "Tether_0.5.24_x64-setup.exe", b"x", "S", None, None, &manifest("0.5.24"), false, rollout("custom:0=2,3=30,48=100", 0).unwrap());
        e.validate().unwrap();
        assert_eq!(e.percent(3 * 3600 - 1), 2);
        assert_eq!(e.percent(3 * 3600), 30);
        assert_eq!(e.percent(48 * 3600), 100);
    }

    #[test]
    fn next_index_keeps_other_entries() {
        let a = entry("stable", "technician/windows-x86_64", "Tether_1.0.0_x64-setup.exe", b"a", "S", None, None, &manifest("1.0.0"), false, rollout("now", 0).unwrap());
        let b = entry("stable", "customer/darwin-universal", "TetherQS-1.0.0-universal.zip", b"b", "S", None, None, &manifest("1.0.0"), false, rollout("now", 0).unwrap());
        let first = next_index(None, "stable", 1, 1000, |r| { r.insert(index_key("technician", WINDOWS), a.clone()); }).unwrap();
        let second = next_index(Some(first), "stable", 2, 2000, |r| { r.insert(index_key("customer", MAC), b.clone()); }).unwrap();
        assert_eq!(second.releases.len(), 2);
        assert_eq!((second.serial, second.issued, second.expires), (2, 2000, 2000 + 30 * 86_400));
        assert!(next_index(None, "Bad", 1, 0, |_| {}).is_err());
    }

    #[test]
    fn versions_from_listings() {
        assert_eq!(version_of("technician/windows-x86_64/Tether_0.3.1_x64-setup.exe"), Some(("technician", WINDOWS, "0.3.1".into())));
        assert_eq!(version_of("customer/darwin-universal/TetherQS-0.4.0-beta-universal.zip"), Some(("customer", MAC, "0.4.0-beta".into())));
        for none in [
            "technician/windows-x86_64/Tether_0.3.1_x64-setup.exe.sig",
            "technician/windows-x86_64/latest.json",
            "technician/darwin-universal/Tether_0.3.1_universal.dmg",
            "customer/windows-x86_64/TetherQS-x.exe",
            "a/b/c/d",
        ] {
            assert_eq!(version_of(none), None, "{none}");
        }
        let files = [
            ("stable", "technician/windows-x86_64/Tether_0.3.9_x64-setup.exe"),
            ("stable", "technician/windows-x86_64/Tether_0.3.10_x64-setup.exe"),
            ("stable", "technician/windows-x86_64/Tether_0.3.10-test_x64-setup.exe"),
            ("beta", "technician/windows-x86_64/Tether_0.4.0_x64-setup.exe"),
            ("stable", "customer/windows-x86_64/TetherQS-0.3.2.exe"),
        ];
        let l = latest_versions(files);
        assert_eq!(l[&("stable".to_owned(), "technician", WINDOWS)], "0.3.10");
        assert_eq!(l[&("beta".to_owned(), "technician", WINDOWS)], "0.4.0");
        assert_eq!(l[&("stable".to_owned(), "customer", WINDOWS)], "0.3.2");
        assert_eq!(l.len(), 3);
    }

    #[test]
    fn latest_local_builds() {
        let b = latest_builds(
            ["w/Tether_0.5.0_x64-setup.exe", "w/Tether_0.5.1_x64-setup.exe", "w/Tether_0.5.1_x64-setup.exe.sig", "q/TetherQS-0.5.1.exe", "m/TetherQS-0.5.1-universal.zip", "m/Tether_0.5.1_universal.dmg", "m/Tether_0.5.1_universal.app.tar.gz", "x/TetherQS.exe"]
                .map(std::path::PathBuf::from),
        );
        assert_eq!(b[&("technician", WINDOWS)], ("0.5.1".into(), "w/Tether_0.5.1_x64-setup.exe".into()));
        assert_eq!(b[&("customer", WINDOWS)].0, "0.5.1");
        assert_eq!(b[&("technician", MAC)].1, std::path::PathBuf::from("m/Tether_0.5.1_universal.app.tar.gz"));
        assert_eq!(b[&("customer", MAC)].0, "0.5.1");
        assert_eq!(b.len(), 4);
    }
}

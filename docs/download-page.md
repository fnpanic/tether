# Download page: Standard design and custom HTML templates

The download page at `https://<relay>/` is published by Tether Manage (**Download page**). It has
two designs, chosen with **Design: Standard (built-in) | Custom HTML**:

- **Standard (built-in)**, the default: the generated page described in
  [management.md](management.md#download-page). Nothing changes for anyone who never picks Custom.
- **Custom HTML**: your own HTML file with `{{tether.…}}` placeholders. Tether Manage fills in the
  links and release details when it publishes.

Switching back to Standard publishes the built-in page again on the next publish. Stored templates
are not deleted.

## How the page is built today (code map)

| Step | Where |
| --- | --- |
| Standard page: the stable `latest.json` per app and OS from the first relay, then the HTML | `page_render`, `apps/manage/src-tauri/src/main.rs:1322`; `release::download_page`, `apps/manage/src-tauri/src/release.rs:253` (not changed by this feature) |
| Upload to every relay | `page_publish`, `main.rs:1369`: `PUT /api/admin/download-page` |
| Relay stores it | `server/api/src/main.rs:835` writes `download/index.html`. The body limit is `LIMIT_PAGE` = 512 KiB (`server/api/src/main.rs:52`, [api.md](api.md) lines 13 and 138) |
| Relay serves it | Caddy `handle /` (`server/caddy/Caddyfile:109`) and nginx `location = /` (`server/nginx/nginx.conf:196`) serve `index.html`. It is also served as `/download/index.html` by the `/download/*` file server |
| Rebuilt after a release | Releases → **Publish the latest builds**: after a `stable` publish, `apps/manage/src/pages/Releases.tsx:252` rebuilds the page with the saved settings |
| Halted releases | **Halt** changes only the signed index, so a published page keeps its links until it is published again (Manage says so after a Halt on `stable`). Custom templates leave halted releases out (`page_data`). The Standard page is built from `latest.json`, so it refuses to build while the version it would offer is halted in the stable index: resume it or publish a newer version first. |
| Preview | Sandboxed `<iframe sandbox="" srcdoc>`; `previewDoc` (`apps/manage/src/lib/page-preview.ts`) adds `<base target="_blank">` so that clicking a link cannot navigate the frame |

Both designs go through one function, `buildPage` (`apps/manage/src/lib/page-template.ts:288`).
With Standard, or with no `page_design` setting, it calls `page_render` with exactly the arguments
used before, so the output is byte-identical to the old generator (unit test "Standard (the
default) is today's generator, unchanged"). The Download page (`DownloadPage.tsx:58`, `:68`) and
the rebuild after a release (`Releases.tsx:252`) both use it. A custom design therefore also
survives a release.

## Template syntax

**Decision: text placeholders, a small Mustache subset, no element hooks.**

- `{{tether.<name>}}` inserts a value, HTML-escaped (`& < > " '`).
- `{{#tether.<section>}}…{{/tether.<section>}}` is shown only when the section exists (a stable
  release, or an active custom-app link). `{{^tether.<section>}}…{{/tether.<section>}}` is shown
  only when it does not ("Not available yet"). Sections can be nested.
- Spaces inside the braces are allowed (`{{ tether.title }}`).
- HTML editors often percent-encode braces in `href` (`%7B%7Btether.x.url%7D%7D`). These are
  recognised.

Why not `data-tether="technician-windows"` hooks on `<a>` elements: filling them would need an HTML
parser in Manage, and a hook cannot express text such as "version 0.5.24 · 24 MB" or a fallback
block. Placeholders work in any editor, in text and in attributes, and they are easy to check.

### Placeholders

Apps (`<app>`): `technician` (Tether for technicians) and `customer` (TetherQS, Tether Quick
Support). OS (`<os>`): `windows` (`windows-x86_64`) and `macos` (`darwin-universal`).

| Placeholder | Value |
| --- | --- |
| `{{tether.title}}`, `{{tether.intro}}` | Title and introduction from the Download page fields (plain text; `white-space:pre-line` keeps the line breaks) |
| `{{tether.relay}}` | Host of the first relay |
| `{{tether.published}}` | Date of this publish (YYYY-MM-DD, UTC) |
| `{{#tether.<app>.<os>}}` / `{{^…}}` | Section: there is / is not a stable release |
| `{{tether.<app>.<os>.url}}` | Download link, relative (`/download/stable/<app>/<platform>/<file>`), so each relay serves its own copy. For the macOS technician app this is the disk image (`.dmg`) |
| `{{tether.<app>.<os>.cdn_url}}` | The same file on the Cloudflare CDN when it is there, otherwise the same as `url`. Only the Windows installer and the macOS TetherQS zip go to the CDN; the disk image does not, and for TetherQS.exe the CDN only has the `.gz` |
| `{{tether.<app>.<os>.file}}` | File name |
| `{{tether.<app>.<os>.version}}` | Version |
| `{{tether.<app>.<os>.size}}` / `.bytes` | Size (`24.1 MB`) / in bytes |
| `{{tether.<app>.<os>.sha256}}` | SHA-256 (hex) of the file behind `url` |
| `{{tether.<app>.<os>.date}}` | Release date: the start of its rollout (YYYY-MM-DD) |
| `{{tether.<app>.<os>.notes}}` | Release notes (plain text) |
| `{{tether.<app>.<os>.minimum_version}}` | Minimum version (empty when there is none) |
| `{{#tether.custom.<id>}}` / `{{^…}}` | Section: the custom app `<id>` has an active link |
| `{{tether.custom.<id>.url}}`, `.name`, `.expires` | Its private download link (`https://<first relay>/q/<token>`), name, link expiry |

The **Placeholder reference** on the Download page lists all of them, with a copy button for each.
It includes the ids of your custom apps.
[examples/download-template.html](examples/download-template.html) uses every placeholder. It is
the **Download starter template** in Manage and looks like the Standard page.

A custom-app link on the page is no longer private: anyone who opens the page can use it. A custom
app that is deleted or expires does not break the page. Its sections are hidden, and Manage shows a
warning.

### Where the data comes from

| Data | Source |
| --- | --- |
| Releases | `page_data` (`main.rs:1382`): the highest **verified** (admin-signed) stable `releases.json` on the relays. Version, file, size, sha256, notes, minimum version, rollout start and the CDN URL are covered by the signature; the unsigned `latest.json` is not used. Releases that are only in `latest.json` (published before the signed index) are missing until they are published again. |
| macOS disk image | Not in the index. Its size and SHA-256 come from downloading the first relay's copy once per version and session (`dmg_facts`, `main.rs:1401`); if it is missing, the release is left out. |
| Custom apps | `custom_list` (only when the template uses `tether.custom.`): active links |
| Title, introduction, relay | Manage settings |

## Rules checked before publishing

`fill` (`page-template.ts:202`) returns `errors` and `warnings`. **Preview** shows them, and
**Publish…** is disabled while there are errors. When the page is rebuilt after a release, an
error is reported in the release results row "Download page" and the old page stays online.

Errors:
- Unknown `{{tether.…}}` names, with their line number. They are never left in the page.
- An unclosed block, or a block closed with the wrong name.
- A field of a section that does not exist, outside its `{{#…}}` block ("wrap it in …"). Without
  this rule the page would have a dead link.
- A placeholder in a place where HTML escaping is not enough (`context`, `page-template.ts:160`):
  inside `<script>` or `<style>`; in an unquoted attribute; in an `on…` event handler; in a URL
  attribute (`href`, `src`, `srcset`, `action`, … `URL_ATTRS`, `:157`) unless it is a `.url` /
  `.cdn_url` placeholder at the very start of the value (no `javascript:` prefix, no notes in
  `href`). To pass a value to your own script, put it in a `data-` attribute and read that
  attribute.
- `<base>`: it would redirect the relative download links.
- Resources loaded from other sites (`EXTERNAL`, `:186`): `src`/`srcset`/`poster`/`data` with an
  `http(s)://` or `//` URL, `<link href>`, CSS `url()` and `@import`. Turn on **Allow resources from
  other sites** to publish them anyway; they are then shown as warnings. Plain links
  (`<a href="https://…">`) are always allowed.
- Template or filled page larger than 512 KiB (the relay limit).

URLs Manage inserts are only relative `/download/stable/…` paths built from file names that pass
`^[A-Za-z0-9._-]+$`. The exceptions are `https://` URLs whose host is a configured relay or the
configured CDN (`cdn_url`, custom links). Anything else falls back to the relay link, or the section
is hidden. Release notes and custom-app names come from the relay or the index and are always
escaped (unit tests with `<script>`, `onerror` and quote payloads).

Warnings: `{{…}}` that are not tether placeholders (left as they are; this catches `{{tehter.x}}`),
inactive custom apps, and allowed external resources.

## Security model and CSP

The template author is the admin, so the template's own HTML, CSS and JavaScript are trusted. The
values inserted into it are not trusted, and they are escaped and checked as described above.

The relays send **no Content-Security-Policy** for `/` (Caddy and nginx add only HSTS). The
built-in page also needs inline CSS and an inline script. So a template may contain:

| Works | Notes |
| --- | --- |
| Inline `<style>` and `style=""` | yes |
| Inline `<script>` | yes (no placeholders inside) |
| Images as `data:` URIs or inline `<svg>` | yes (keep under 512 KiB in total) |
| External fonts, CSS, scripts, images | only with **Allow resources from other sites**. Visitors' browsers then contact those sites (privacy, and availability if the site is down) |
| Uploaded asset files beside the page | no: the relay stores one `index.html` only |

The page is on the same origin as `/api/*`. That is safe because the admin API authenticates with
an Ed25519 signature per request, not with cookies, so script on the page gains nothing. No relay
change is needed. If you want the relay to enforce the same rule, an optional hardening is
`Content-Security-Policy: default-src 'self' 'unsafe-inline' data:` on `/` in Caddy and nginx. It
is not deployed, because it would block templates that were explicitly allowed to load external
resources.

## Storage and versions

The Rust commands `page_template_save` / `page_template_list` / `page_template_read`
(`main.rs:1427`, `:1439`, `:1451`) keep every chosen file as
`<data dir>/page-templates/<millis>-<name>.html` (at most 512 KiB). The **Template** list shows all
of them, newest first; pick an earlier one to go back. `settings.json` keeps `page_design`
(`standard` | `custom`, absent = standard), `page_template` (the file) and `page_allow_external`.
These are saved when you publish, like the title and introduction, so the rebuild after a release
uses the design that was last published.

**Deleting** a version: pick it in the **Template** list and click the bin next to it (asks first;
`page_template_delete`). The template the published page uses is marked "· published" and cannot be
deleted while the Custom design is published, because the page is rebuilt from it after every stable
release; publish another template or the Standard design first. With Standard published, every
template can be deleted.

## Using it

1. Download page → **Design: Custom HTML** → **Download starter template**, and edit the file.
2. **Choose HTML file…** → **Preview**. Fix the errors listed on the left. The preview runs without
   scripts and its links do nothing.
3. **Publish…** → review → every relay.
4. Later releases rebuild the page automatically (Releases, stable channel). To go back, choose
   **Standard (built-in)** and publish.

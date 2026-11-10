// Custom download page templates (docs/download-page.md): the admin's own HTML with
// `{{tether.…}}` placeholders that Tether Manage fills at publish time. Pure: no IPC, so
// `node --test` runs it.

/** One release of the signed stable index (`page_data`). */
export interface Rel {
  version: string;
  file: string;
  size: number;
  sha256: string;
  notes: string;
  minimum_version: string;
  mandatory: boolean;
  /** Unix seconds: start of its rollout (the publish time unless scheduled). */
  date: number;
  cdn_url: string | null;
}

export interface CustomApp {
  id: string;
  name: string;
  link: string;
  expires: number;
  status: string;
}

export interface PageData {
  title: string;
  intro: string;
  /** Relay hosts in order; the first is `tether.relay`. */
  relays: string[];
  /** CDN host, when the CDN is on (cdn_url may point there). */
  cdn: string | null;
  /** Unix seconds of this publish. */
  now: number;
  /** `page_data`: `kind/platform` → release. */
  releases: Record<string, Rel>;
  custom: CustomApp[];
}

/** The relay stores at most this much (`LIMIT_PAGE`, server/api). */
export const MAX_PAGE = 512 * 1024;

export const APPS = { technician: "Tether for technicians", customer: "Tether Quick Support (TetherQS)" } as const;
export const OSES = { windows: "windows-x86_64", macos: "darwin-universal" } as const;

const FIELDS: [string, string][] = [
  ["url", "Download link, relative: each relay serves its own copy"],
  ["cdn_url", "Download link on the CDN when this file is there, else the same as url"],
  ["file", "File name"],
  ["version", "Version"],
  ["size", "Size, e.g. 24.1 MB"],
  ["bytes", "Size in bytes"],
  ["sha256", "SHA-256 checksum (hex)"],
  ["date", "Release date, YYYY-MM-DD"],
  ["notes", "Release notes (plain text)"],
  ["minimum_version", "Minimum version (empty when none)"],
];

/** Every placeholder with a description; `custom` adds one block per custom app id. */
export function reference(custom: { id: string; name: string }[] = []): { name: string; desc: string }[] {
  const out = [
    { name: "{{tether.title}}", desc: "Title (Download page settings)" },
    { name: "{{tether.intro}}", desc: "Introduction text (plain text)" },
    { name: "{{tether.relay}}", desc: "Host of the first relay" },
    { name: "{{tether.published}}", desc: "Date of this publish, YYYY-MM-DD" },
  ];
  for (const [app, label] of Object.entries(APPS))
    for (const os of Object.keys(OSES)) {
      const b = `tether.${app}.${os}`;
      out.push({ name: `{{#${b}}}…{{/${b}}}`, desc: `Shown only when ${label} for ${os} has a stable release` });
      out.push({ name: `{{^${b}}}…{{/${b}}}`, desc: `Shown only when it has none` });
      for (const [f, d] of FIELDS) out.push({ name: `{{${b}.${f}}}`, desc: d });
    }
  for (const c of custom) {
    const b = `tether.custom.${c.id}`;
    out.push({ name: `{{#${b}}}…{{/${b}}}`, desc: `Custom app ${c.name}: shown while its link is active` });
    out.push({ name: `{{${b}.url}}`, desc: `Private download link of ${c.name} (public once on the page)` });
    out.push({ name: `{{${b}.name}}`, desc: "Its name" });
    out.push({ name: `{{${b}.expires}}`, desc: "Link expiry, YYYY-MM-DD" });
  }
  return out;
}

const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;").replace(/'/g, "&#39;");
const day = (unix: number) => (unix > 0 ? new Date(unix * 1000).toISOString().slice(0, 10) : "");
export const human = (n: number) => (n >= 1e6 ? `${(n / 1e6).toFixed(1)} MB` : `${Math.max(1, Math.round(n / 1e3))} KB`);
const SAFE_FILE = /^[A-Za-z0-9._-]{1,255}$/;

/** https URL on one of `hosts`, else null. */
function httpsOn(u: string, hosts: string[]): string | null {
  try {
    const x = new URL(u);
    return x.protocol === "https:" && hosts.includes(x.host) && !x.username && !x.password ? x.href : null;
  } catch {
    return null;
  }
}

/** A placeholder's value; `url` values may stand at the start of href/src. */
interface Val {
  v: string;
  url?: boolean;
}

/** Sections (block names) present and the value of every placeholder. */
export function table(d: PageData): { blocks: Set<string>; sections: Set<string>; known: Set<string>; values: Map<string, Val> } {
  /** Sections that are shown (`{{#…}}`), and all section names. */
  const blocks = new Set<string>();
  const sections = new Set<string>();
  const known = new Set<string>(["tether.title", "tether.intro", "tether.relay", "tether.published"]);
  const values = new Map<string, Val>([
    ["tether.title", { v: d.title }],
    ["tether.intro", { v: d.intro }],
    ["tether.relay", { v: d.relays[0] ?? "" }],
    ["tether.published", { v: day(d.now) }],
  ]);
  for (const app of Object.keys(APPS))
    for (const [os, platform] of Object.entries(OSES)) {
      const b = `tether.${app}.${os}`;
      sections.add(b);
      FIELDS.forEach(([f]) => known.add(`${b}.${f}`));
      const r = d.releases[`${app}/${platform}`];
      if (!r || !SAFE_FILE.test(r.file) || r.file.startsWith(".")) continue;
      blocks.add(b);
      const url = `/download/stable/${app}/${platform}/${r.file}`;
      // The CDN copy only counts when it is this very file (the disk image and TetherQS.exe.gz are not).
      const cdn = r.cdn_url && httpsOn(r.cdn_url, [...(d.cdn ? [d.cdn] : []), ...d.relays]);
      const set = (f: string, v: string, isUrl = false) => values.set(`${b}.${f}`, { v, url: isUrl });
      set("url", url, true);
      set("cdn_url", cdn && cdn.endsWith(`/${r.file}`) ? cdn : url, true);
      set("file", r.file);
      set("version", r.version);
      set("size", human(r.size));
      set("bytes", String(r.size));
      set("sha256", r.sha256);
      set("date", day(r.date));
      set("notes", r.notes);
      set("minimum_version", r.minimum_version);
    }
  for (const c of d.custom) {
    if (!/^[a-z0-9-]{1,40}$/.test(c.id)) continue;
    const b = `tether.custom.${c.id}`;
    sections.add(b);
    ["url", "name", "expires"].forEach((f) => known.add(`${b}.${f}`));
    const link = httpsOn(c.link, d.relays);
    if (c.status !== "active" || c.expires <= d.now || !link) continue;
    blocks.add(b);
    values.set(`${b}.url`, { v: link, url: true });
    values.set(`${b}.name`, { v: c.name });
    values.set(`${b}.expires`, { v: day(c.expires) });
  }
  return { blocks, sections, known, values };
}

/** Attributes whose value is fetched or navigated to: only url placeholders, at the start. */
const URL_ATTRS = new Set(["href", "src", "srcset", "action", "formaction", "poster", "data", "cite", "background", "ping", "xlink:href", "manifest"]);

/** Start of the tag or comment that `at` is inside, or -1 in text. `<` and `>` inside quoted
 *  attribute values, comments, scripts and styles do not open or close a tag. */
function tagStart(html: string, at: number): number {
  for (let i = 0; i < at; ) {
    const lt = html.indexOf("<", i);
    if (lt < 0 || lt >= at) return -1;
    if (html.startsWith("<!--", lt)) {
      const end = html.indexOf("-->", lt + 4);
      if (end < 0 || end + 3 > at) return lt;
      i = end + 3;
      continue;
    }
    // As in HTML, `<` opens a tag only before a letter, `/`, `!` or `?`.
    if (!/[a-z/!?]/i.test(html[lt + 1] ?? "")) {
      i = lt + 1;
      continue;
    }
    let quote = "";
    let j = lt + 1;
    for (; j < at; j++) {
      const c = html[j];
      if (quote) quote = c === quote ? "" : quote;
      else if (c === '"' || c === "'") quote = c;
      else if (c === ">") break;
    }
    if (j >= at) return lt;
    i = j + 1;
    const raw = /^<(script|style)\b/i.exec(html.slice(lt, lt + 8));
    if (raw) {
      const end = html.toLowerCase().indexOf(`</${raw[1].toLowerCase()}`, i);
      if (end < 0 || end >= at) return -1;
      i = end;
    }
  }
  return -1;
}

/** Why a placeholder at `at` in `html` is not allowed there, or null. */
function context(html: string, at: number, val: Val | undefined): string | null {
  const before = html.slice(0, at).toLowerCase();
  for (const el of ["script", "style"]) {
    if (before.lastIndexOf(`<${el}`) > before.lastIndexOf(`</${el}`)) return `not inside <${el}> (put it in a data- attribute and read that)`;
  }
  const lt = tagStart(html, at);
  if (lt < 0) return null; // text
  if (before.startsWith("<!--", lt)) return null; // comment
  // Closed quoted values out of the way: the first quote left opens the value we are in.
  const t = html.slice(lt, at).replace(/"[^"]*"|'[^']*'/g, "_");
  const q = t.search(/["']/);
  const m = /([^\s"'=<>/]+)\s*=\s*$/.exec(q < 0 ? t : t.slice(0, q));
  if (q < 0) {
    const u = /([^\s"'=<>/]+)\s*=\s*[^\s"'>]*$/.exec(t);
    return u ? `quote the ${u[1]} attribute` : "only inside quoted attribute values or text";
  }
  if (!m) return "only inside quoted attribute values or text";
  const attr = m[1].toLowerCase();
  if (attr.startsWith("on")) return `not inside the event handler ${attr}`;
  if (URL_ATTRS.has(attr) && (!val?.url || q !== t.length - 1)) return `${attr}="…" takes only a .url placeholder, at its start`;
  return null;
}

const TOKEN = /\{\{\s*([#^/]?)\s*(tether(?:\.[A-Za-z0-9_-]+)*)\s*\}\}/g;

/** Resources the browser would load from another site (CSP-free relay: Manage checks it). */
const EXTERNAL = [
  /<(?:script|img|iframe|frame|embed|source|video|audio|track|object|input)\b[^>]*?\s(?:src|srcset|data|poster)\s*=\s*["']?\s*(?:https?:)?\/\/[^\s"'>]*/gi,
  /<link\b[^>]*?\shref\s*=\s*["']?\s*(?:https?:)?\/\/[^\s"'>]*/gi,
  /url\(\s*["']?\s*(?:https?:)?\/\/[^)"'\s]*/gi,
  /@import\s+(?:url\()?\s*["']?\s*(?:https?:)?\/\/[^\s"');]*/gi,
];

export interface Filled {
  html: string;
  errors: string[];
  warnings: string[];
  /** Placeholder and block names the template uses. */
  used: string[];
}

/** Fill `tpl`. Publish only when `errors` is empty. */
export function fill(tpl: string, d: PageData, opts: { allowExternal?: boolean } = {}): Filled {
  const errors: string[] = [];
  const warnings: string[] = [];
  const line = (at: number) => tpl.slice(0, at).split("\n").length;
  if (tpl.length > MAX_PAGE) errors.push(`The template is larger than ${MAX_PAGE / 1024} KiB.`);
  // HTML editors percent-encode braces inside href: %7B%7Btether.x%7D%7D.
  const src = tpl.replace(/%7B%7B(\s*[#^/]?\s*tether[A-Za-z0-9_.-]*\s*)%7D%7D/gi, "{{$1}}");
  const { blocks, sections, known, values } = table(d);
  const used = new Set<string>();
  // Any custom app id is a section: a deleted or expired app's block just disappears, so the page
  // still publishes after a release.
  for (const m of src.matchAll(/tether\.custom\.([a-z0-9-]{1,40})(?:\.(url|name|expires))?\b/g)) {
    const b = `tether.custom.${m[1]}`;
    sections.add(b);
    ["url", "name", "expires"].forEach((f) => known.add(`${b}.${f}`));
    if (!blocks.has(b)) warnings.push(`Custom app "${m[1]}" has no active link: its sections are hidden.`);
  }

  // Blocks first (innermost first by repetition), checked for balance on the way.
  const stack: { name: string; line: number }[] = [];
  for (const m of src.matchAll(TOKEN)) {
    const [, kind, name] = m;
    used.add(name);
    const at = `Line ${line(m.index!)}: {{${kind}${name}}}`;
    if (!known.has(name) && !sections.has(name)) errors.push(`${at} is not a placeholder (see the reference)`);
    else if (kind === "#" || kind === "^") {
      if (!sections.has(name)) errors.push(`${at}: only sections (tether.<app>.<os>, tether.custom.<id>) open a block`);
      stack.push({ name, line: line(m.index!) });
    } else if (kind === "/") {
      const open = stack.pop();
      if (open?.name !== name) errors.push(`${at} closes ${open ? `{{#${open.name}}} from line ${open.line}` : "nothing"}`);
    } else if (sections.has(name)) errors.push(`${at} is a section: use {{#${name}}}…{{/${name}}} or one of its fields`);
  }
  stack.forEach((o) => errors.push(`Line ${o.line}: {{#${o.name}}} is never closed`));
  if (errors.length) return { html: "", errors, warnings, used: [...used] };

  const BLOCK = /\{\{\s*([#^])\s*(tether(?:\.[A-Za-z0-9_-]+)*)\s*\}\}((?:(?!\{\{\s*[#^]\s*\2\s*\}\})[\s\S])*?)\{\{\s*\/\s*\2\s*\}\}/;
  let out = src;
  for (let m; (m = BLOCK.exec(out)); ) out = out.slice(0, m.index) + ((m[1] === "#") === blocks.has(m[2]) ? m[3] : "") + out.slice(m.index + m[0].length);

  let html = "";
  let last = 0;
  for (const m of out.matchAll(TOKEN)) {
    const name = m[2];
    const val = values.get(name);
    html += out.slice(last, m.index);
    last = m.index! + m[0].length;
    if (!val) {
      const sec = name.slice(0, name.lastIndexOf("."));
      errors.push(`{{${name}}}: ${sec} has no ${sec.startsWith("tether.custom.") ? "active link" : "stable release"}; wrap it in {{#${sec}}}…{{/${sec}}}`);
      continue;
    }
    const why = context(out, m.index!, val);
    if (why) errors.push(`{{${name}}}: ${why}`);
    html += esc(val.v);
  }
  html += out.slice(last);

  const other = [...new Set(out.match(/\{\{(?!\s*[#^/]?\s*tether[.}\s])[^{}]{0,60}\}\}/g) ?? [])];
  if (other.length) warnings.push(`Left as they are (not tether placeholders): ${other.join(" ")}`);
  if (/<base\b/i.test(html)) errors.push("<base> is not allowed: it would redirect the relative download links.");
  const ext = [...new Set(EXTERNAL.flatMap((re) => html.match(re) ?? []))];
  if (ext.length) (opts.allowExternal ? warnings : errors).push(`Loads from other sites${opts.allowExternal ? "" : " (allow external resources to publish)"}: ${ext.map((e) => e.slice(0, 120)).join(" · ")}`);
  if (new TextEncoder().encode(html).length > MAX_PAGE) errors.push(`The filled page is larger than ${MAX_PAGE / 1024} KiB.`);
  return { html: errors.length ? "" : html, errors: [...new Set(errors)], warnings: [...new Set(warnings)], used: [...used] };
}

/** Settings that choose the page (settings.json). */
export interface PageSettings {
  page_title?: string;
  page_intro?: string;
  page_technician?: boolean;
  page_customer?: boolean;
  /** "standard" (default): the built-in generator. "custom": `page_template`. */
  page_design?: "standard" | "custom";
  page_template?: string | null;
  page_allow_external?: boolean;
  cdn?: { enabled?: boolean; base?: string };
  /** The rest of settings.json, saved back as it is. */
  [k: string]: unknown;
}

type Invoke = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;

/** The page to publish for `s`: Standard is exactly today's `page_render`; Custom fills the stored
 *  template and throws its errors. Used by Download page and by Releases after a stable publish. */
/** `skip`: relays left out of the publish; they are not read. */
export async function buildPage(s: PageSettings, servers: string[], invoke: Invoke, now = Math.floor(Date.now() / 1000), skip: string[] = []): Promise<Filled> {
  const title = s.page_title ?? "Remote support";
  const intro = s.page_intro ?? "Download Tether to get help from our support team.";
  if (s.page_design !== "custom") {
    const html = await invoke<string>("page_render", { title, intro, technician: s.page_technician ?? true, customer: s.page_customer ?? true, skip });
    return { html, errors: [], warnings: [], used: [] };
  }
  if (!s.page_template) throw new Error("Choose a template file for the custom design, or switch to Standard.");
  const tpl = await invoke<string>("page_template_read", { file: s.page_template });
  const releases = await invoke<Record<string, Rel>>("page_data", { skip });
  const custom = /tether\.custom\./.test(tpl) ? await invoke<CustomApp[]>("custom_list", { skip }) : [];
  const host = (u: string) => u.replace(/^https?:\/\//, "").replace(/\/.*$/, "");
  const cdn = s.cdn?.enabled && s.cdn.base ? host(s.cdn.base) : null;
  return fill(tpl, { title, intro, relays: servers.map(host), cdn, now, releases, custom }, { allowExternal: s.page_allow_external });
}

/** `buildPage`, throwing when the template has errors. */
export async function pageHtml(s: PageSettings, servers: string[], invoke: Invoke, skip: string[] = []): Promise<string> {
  const f = await buildPage(s, servers, invoke, undefined, skip);
  if (f.errors.length) throw new Error(`The page template has errors: ${f.errors.join("; ")}`);
  return f.html;
}

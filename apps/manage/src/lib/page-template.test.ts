// node --test src/lib/page-template.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { buildPage, fill, reference, type PageData, type Rel } from "./page-template.ts";

const rel = (file: string, extra: Partial<Rel> = {}): Rel => ({
  version: "0.5.24",
  file,
  size: 24_100_000,
  sha256: "ab".repeat(32),
  notes: "Faster reconnects",
  minimum_version: "0.4.0",
  mandatory: false,
  date: 1_793_786_400,
  cdn_url: null,
  ...extra,
});

const data = (over: Partial<PageData> = {}): PageData => ({
  title: "Remote support",
  intro: "Download Tether.",
  relays: ["relay1.example", "relay2.example"],
  cdn: "dl.example.workers.dev",
  now: 1_793_786_400,
  releases: {
    "technician/windows-x86_64": rel("Tether_0.5.24_x64-setup.exe", { cdn_url: "https://dl.example.workers.dev/stable/technician/windows-x86_64/Tether_0.5.24_x64-setup.exe" }),
    "technician/darwin-universal": rel("Tether_0.5.24_universal.dmg"),
    "customer/windows-x86_64": rel("TetherQS-0.5.24.exe"),
    "customer/darwin-universal": rel("TetherQS-0.5.24-universal.zip"),
  },
  custom: [{ id: "acme", name: "ACME", link: "https://relay1.example/q/abcDEF_-1", expires: 1_793_786_400 + 86400, status: "active" }],
  ...over,
});

test("fills values and keeps the rest of the template", () => {
  const f = fill('<a href="{{tether.technician.windows.url}}">v{{ tether.technician.windows.version }}, {{tether.technician.windows.size}}</a> {{tether.published}}', data());
  assert.deepEqual(f.errors, []);
  assert.equal(f.html, '<a href="/download/stable/technician/windows-x86_64/Tether_0.5.24_x64-setup.exe">v0.5.24, 24.1 MB</a> 2026-11-04');
});

test("CDN link only when it is this file on the CDN or a relay, else the relay link", () => {
  const f = fill('{{tether.technician.windows.cdn_url}} {{tether.customer.windows.cdn_url}}', data());
  assert.equal(f.html, "https://dl.example.workers.dev/stable/technician/windows-x86_64/Tether_0.5.24_x64-setup.exe /download/stable/customer/windows-x86_64/TetherQS-0.5.24.exe");
  const evil = data();
  evil.releases["technician/windows-x86_64"].cdn_url = "https://evil.example/Tether_0.5.24_x64-setup.exe";
  assert.equal(fill("{{tether.technician.windows.cdn_url}}", evil).html, "/download/stable/technician/windows-x86_64/Tether_0.5.24_x64-setup.exe");
  evil.releases["technician/windows-x86_64"].cdn_url = "javascript:alert(1)//Tether_0.5.24_x64-setup.exe";
  assert.equal(fill("{{tether.technician.windows.cdn_url}}", evil).html, "/download/stable/technician/windows-x86_64/Tether_0.5.24_x64-setup.exe");
});

test("every inserted value is HTML-escaped (XSS in notes, names, title)", () => {
  const d = data({ title: '"><script>alert(1)</script>' });
  d.releases["customer/windows-x86_64"].notes = `<img src=x onerror=alert(1)> & 'quote' "dq"`;
  d.custom[0].name = "<b>ACME</b>";
  const f = fill('<p title="{{tether.title}}">{{tether.customer.windows.notes}}</p>{{#tether.custom.acme}}{{tether.custom.acme.name}}{{/tether.custom.acme}}', d);
  assert.deepEqual(f.errors, []);
  assert.equal(f.html, '<p title="&quot;&gt;&lt;script&gt;alert(1)&lt;/script&gt;">&lt;img src=x onerror=alert(1)&gt; &amp; &#39;quote&#39; &quot;dq&quot;</p>&lt;b&gt;ACME&lt;/b&gt;');
});

test("a release file name that is not a plain name drops the release", () => {
  const d = data();
  d.releases["customer/windows-x86_64"].file = '"><script>x</script>.exe';
  const f = fill("{{^tether.customer.windows}}none{{/tether.customer.windows}}", d);
  assert.equal(f.html, "none");
});

test("custom app links must be https on a relay", () => {
  const d = data();
  d.custom[0].link = "https://evil.example/q/x";
  assert.equal(fill("{{#tether.custom.acme}}yes{{/tether.custom.acme}}{{^tether.custom.acme}}no{{/tether.custom.acme}}", d).html, "no");
});

test("unknown placeholders and broken blocks are errors, not left in", () => {
  const f = fill("{{tether.technician.windows.verison}} {{tether.foo}} {{#tether.technician.windows}}x", data());
  assert.equal(f.html, "");
  assert.equal(f.errors.length, 3);
  assert.match(f.errors[0], /Line 1: \{\{tether\.technician\.windows\.verison\}\} is not a placeholder/);
  assert.match(f.errors[2], /never closed/);
  assert.match(fill("{{#tether.title}}x{{/tether.title}}", data()).errors[0], /only sections/);
  assert.match(fill("{{tether.technician.windows}}", data()).errors[0], /is a section/);
  assert.match(fill("{{#tether.technician.windows}}{{/tether.customer.windows}}", data()).errors[0], /closes \{\{#tether\.technician\.windows\}\}/);
});

test("other {{…}} stay and are reported as warnings", () => {
  const f = fill("{{name}} {{tehter.x}}", data());
  assert.equal(f.html, "{{name}} {{tehter.x}}");
  assert.match(f.warnings[0], /\{\{name\}\} \{\{tehter\.x\}\}/);
});

test("missing releases: sections hide, unguarded fields are errors", () => {
  const d = data({ releases: {} });
  const ok = fill("{{#tether.technician.macos}}<a href=\"{{tether.technician.macos.url}}\">dl</a>{{/tether.technician.macos}}{{^tether.technician.macos}}Not available yet{{/tether.technician.macos}}", d);
  assert.deepEqual(ok.errors, []);
  assert.equal(ok.html, "Not available yet");
  const bad = fill('<a href="{{tether.technician.macos.url}}">dl</a>', d);
  assert.match(bad.errors[0], /tether\.technician\.macos has no stable release; wrap it in \{\{#tether\.technician\.macos\}\}/);
  // A deleted custom app hides its section (the page still publishes after a release) with a warning.
  const gone = fill("{{#tether.custom.gone}}{{tether.custom.gone.url}}{{/tether.custom.gone}}", d);
  assert.deepEqual(gone.errors, []);
  assert.equal(gone.html, "");
  assert.match(gone.warnings[0], /"gone" has no active link/);
});

test("placeholders only where escaping is enough", () => {
  const err = (t: string) => fill(t, data()).errors[0] ?? "";
  assert.match(err("<script>var v='{{tether.technician.windows.version}}'</script>"), /not inside <script>/);
  assert.match(err("<style>a::after{content:'{{tether.title}}'}</style>"), /not inside <style>/);
  assert.match(err("<a href={{tether.technician.windows.url}}>x</a>"), /quote the href attribute/);
  assert.match(err('<a onclick="go(\'{{tether.technician.windows.url}}\')">x</a>'), /event handler onclick/);
  assert.match(err('<a href="{{tether.customer.windows.notes}}">x</a>'), /takes only a \.url placeholder/);
  assert.match(err('<a href="javascript:{{tether.technician.windows.url}}">x</a>'), /at its start/);
  assert.match(err("<a {{tether.title}}>x</a>"), /quoted attribute values or text/);
  // A `>` inside an earlier quoted value does not end the tag.
  assert.match(err('<a title="a>b" onclick="go(\'{{tether.title}}\')">x</a>'), /event handler onclick/);
  assert.match(err('<a title=\'a>b\' href="javascript:{{tether.technician.windows.url}}">x</a>'), /at its start/);
  // Quotes and `<` in comments, scripts and text before the tag do not confuse it.
  assert.deepEqual(fill('<!-- don\'t --><script>if(a<b)x="\'"</script><p>1 < 2 {{tether.title}}</p><a title="a>b" href="{{tether.technician.windows.url}}">x</a>', data()).errors, []);
  assert.deepEqual(fill('<div data-v="{{tether.technician.windows.version}}" title=\'{{tether.title}}\'></div><script>var a=1</script>{{tether.title}}', data()).errors, []);
});

test("editor-encoded braces in href are recognised", () => {
  assert.equal(fill('<a href="%7B%7Btether.customer.macos.url%7D%7D">x</a>', data()).html, '<a href="/download/stable/customer/darwin-universal/TetherQS-0.5.24-universal.zip">x</a>');
});

test("external resources need the explicit allow; <base> never", () => {
  const t = '<link rel="stylesheet" href="https://fonts.example/x.css"><img src="//cdn.example/logo.png"><a href="https://example.com">site</a>';
  const f = fill(t, data());
  assert.match(f.errors[0], /Loads from other sites.*fonts\.example/);
  assert.match(f.errors[0], /cdn\.example/);
  assert.doesNotMatch(f.errors[0], /example\.com/);
  const ok = fill(t, data(), { allowExternal: true });
  assert.deepEqual(ok.errors, []);
  assert.equal(ok.warnings.length, 1);
  assert.deepEqual(fill('<img src="data:image/png;base64,AAAA"><style>b{background:url(data:image/png;base64,AA)}</style>', data()).errors, []);
  assert.match(fill('<base href="https://evil.example/">', data(), { allowExternal: true }).errors[0], /<base>/);
});

test("size limits", () => {
  assert.match(fill("x".repeat(512 * 1024 + 1), data()).errors[0], /template is larger/);
  assert.match(fill("{{tether.customer.windows.notes}}".repeat(1), data({ releases: { "customer/windows-x86_64": rel("TetherQS-1.exe", { notes: "<".repeat(200_000) }) } })).errors[0], /filled page is larger/);
});

test("the example template uses every placeholder and fills to a page with all links", () => {
  const tpl = readFileSync(new URL("../../../../docs/examples/download-template.html", import.meta.url), "utf8");
  const f = fill(tpl, data());
  assert.deepEqual(f.errors, []);
  assert.deepEqual(f.warnings, []);
  const names = reference([{ id: "acme", name: "ACME" }]).flatMap((r) => [...r.name.matchAll(/\{\{[#^]?(tether[^}]*)\}\}/g)].map((m) => m[1]));
  for (const n of names) assert.ok(f.used.includes(n), `example template lacks ${n}`);
  assert.doesNotMatch(f.html, /tether\./);
  for (const r of Object.entries(data().releases)) assert.ok(f.html.includes(`href="/download/stable/${r[0]}/${r[1].file}"`), r[0]);
  assert.ok(f.html.includes('href="https://dl.example.workers.dev/stable/technician/windows-x86_64/Tether_0.5.24_x64-setup.exe"'));
  assert.ok(f.html.includes('href="https://relay1.example/q/abcDEF_-1"'));
  // Valid HTML at the level that matters: balanced elements, one doctype, no stray "<".
  assert.match(f.html, /^<!doctype html>/);
  for (const el of ["html", "head", "body", "main", "section", "div", "a", "details", "dl", "script", "style"]) {
    const open = (f.html.match(new RegExp(`<${el}[\\s>]`, "g")) ?? []).length;
    assert.equal(open, (f.html.match(new RegExp(`</${el}>`, "g")) ?? []).length, el);
  }
  // Without releases the same template still fills: every card says "Not available yet".
  const none = fill(tpl, data({ releases: {}, custom: [] }));
  assert.deepEqual(none.errors, []);
  assert.equal(none.html.match(/Not available yet/g)?.length, 4);
  assert.doesNotMatch(none.html, /href="\/download/);
});

test("Standard (the default) is today's generator, unchanged", async () => {
  const calls: [string, unknown][] = [];
  const inv = async <T,>(cmd: string, args?: Record<string, unknown>) => {
    calls.push([cmd, args]);
    return "<!doctype html>GENERATED" as T;
  };
  for (const s of [{}, { page_design: "standard" as const, page_template: "1-x.html" }]) {
    calls.length = 0;
    const f = await buildPage(s, ["https://r1"], inv);
    assert.equal(f.html, "<!doctype html>GENERATED");
    assert.deepEqual(calls, [["page_render", { title: "Remote support", intro: "Download Tether to get help from our support team.", technician: true, customer: true, skip: [] }]]);
  }
});

test("Custom reads the stored template and the signed release data", async () => {
  const answers: Record<string, unknown> = { page_template_read: "<p>{{tether.relay}} {{tether.technician.windows.version}}</p>", page_data: data().releases };
  const inv = async <T,>(cmd: string) => answers[cmd] as T;
  const f = await buildPage({ page_design: "custom", page_template: "1-x.html" }, ["https://relay1.example/", "https://relay2.example"], inv);
  assert.equal(f.html, "<p>relay1.example 0.5.24</p>");
  await assert.rejects(buildPage({ page_design: "custom" }, [], inv), /Choose a template/);
});

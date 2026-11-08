// Release publishing with a relay that accepts TCP and never answers (judge 3, D1-D5), end to end:
// Tether Manage on two local relays, relay 1 switched to "hang" (or "hangput": only release
// uploads hang). Every call must end within about a minute with a per-relay error while relay 2
// gets its data; "Publish to the others anyway" leaves relay 1 out; Cancel ends uploads at once;
// a 200 MB release still goes through on healthy relays (the answer limit after the last byte).
// - tether-api from scripts/build-relay.sh (WSL, BIN), twice, data dirs in a temp folder here;
// - a proxy per relay stands in for Caddy (/download/* from the data dir, the rest to tether-api)
//   and switches modes: ok, hang (accepts, never answers, never reads), hangput, down;
// - Tether Manage (debug exe with the frontend) on a COPY of the test vault, driven over CDP.
// Judge 4 (J0-J9): a third relay that "Publish to missing relays" brings up (every file the
// newest index names is copied before the index), a first source that stalls file bodies (asked
// once per copy, not once per file), Cancel before / during the index write and during
// latest.json, Cancel between two builds (run id), the left-out relay gets no request (clean-up,
// page), and every relay silent / one 404 + one silent named per relay.
// The CDN deploy with a silent Cloudflare and the re-read of older files from a silent relay are
// covered by cargo tests (cdn::tests::deploy_against_a_silent_api_fails_quickly,
// tests::fetch_file_moves_on_from_a_silent_relay): the Cloudflare API address is fixed in the app.
//
//   node e2e/hang-e2e.mjs
// Env: MANAGE_EXE, BIN (WSL dir with tether-api), VAULT, SHOTS (folder for screenshots), BIG_MB.
import { execFileSync, spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import { createServer, request } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { chromium } from "playwright-core";

const MANAGE = process.env.MANAGE_EXE || "C:\\cargo-target\\r4\\debug\\tether-manage.exe";
const BIN = process.env.BIN || "/home/thomas/judge2-build/bin";
const VAULT = process.env.VAULT || "C:\\vms\\manage-test";
const SHOTS = process.env.SHOTS || "";
const BIG_MB = Number(process.env.BIG_MB || 200);
const PASS = readFileSync(join(VAULT, "test-passphrase.txt"), "utf8").trim();
const ADMIN_PUB = readFileSync(join(VAULT, "env.txt"), "utf8").trim().split(/\s+/)[0];
const BASE = 23600 + 6 * Math.floor(Math.random() * 100);
// `rule(req, path)`: a mode for one request (null: `mode`); `hits`: requests seen.
const RELAYS = [0, 1, 2].map((i) => ({ api: BASE + 2 * i, proxy: BASE + 2 * i + 1, mode: "ok", rule: () => null, hits: 0 }));
const [R1, R2, R3] = RELAYS.map((r) => `http://127.0.0.1:${r.proxy}`);
const QS = "stable/customer/windows-x86_64", TECH = "stable/technician/windows-x86_64", MACT = "stable/technician/darwin-universal";
const work = mkdtempSync(join(tmpdir(), "hang-e2e-")), vault = join(work, "vault");
const wslPath = (p) => "/mnt/" + p[0].toLowerCase() + p.slice(2).replace(/\\/g, "/");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const results = [];
const check = (name, ok, detail = "") => {
  results.push({ name, ok: !!ok });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? `  (${detail})` : ""}`);
};
const onDisk = (r, rel) => existsSync(join(r.dir, "download", ...rel.split("/")));
const shot = (page, name) => SHOTS && page.screenshot({ path: join(SHOTS, `${name}.png`) }).catch(() => {});
/** Per release PUT on a relay: when the body had arrived and when tether-api answered. */
const timing = [];
const held = [];

for (const [i, r] of RELAYS.entries()) {
  r.dir = join(work, `api${i + 1}`);
  mkdirSync(r.dir, { recursive: true });
  const p = spawn("wsl", ["-d", "Ubuntu-22.04", "--", "env", `ADMIN_PUB_KEY=${ADMIN_PUB}`, `${BIN}/tether-api`, "--data", wslPath(r.dir), "--listen", `127.0.0.1:${r.api}`], { stdio: ["ignore", "pipe", "pipe"] });
  r.out = "";
  p.stdout.on("data", (d) => (r.out += d));
  p.stderr.on("data", (d) => (r.out += d));
  r.proc = p;
  r.server = createServer(async (req, res) => {
    r.hits++;
    held.push(req.socket);
    const upload = req.method === "PUT" && req.url.startsWith("/api/admin/release/") && !/\/(releases|latest)\.json$/.test(req.url);
    const u = req.url.split("?")[0];
    const mode = r.rule(req, u) ?? r.mode;
    if (mode === "hang" || (mode === "hangput" && upload)) return; // accepts, never answers, never reads the body
    if (mode === "down") return req.socket.destroy();
    if (mode === "stallbody") {
      // Headers and a few bytes, then the body never completes.
      res.writeHead(200, { "Content-Length": 10_000_000 });
      return void res.write(Buffer.alloc(100, 1));
    }
    if (u.startsWith("/download/") && !/^\/download\/[a-z0-9-]+\/releases\.json$/.test(u) && (req.method === "GET" || req.method === "HEAD")) {
      try {
        const b = readFileSync(join(r.dir, "download", ...u.slice(10).split("/").filter((x) => x && x !== "..")));
        res.writeHead(200, { "Content-Length": b.length });
        return res.end(req.method === "HEAD" ? undefined : b);
      } catch {
        return res.writeHead(404).end("not found");
      }
    }
    // Streamed like Caddy: the answer time counts from the last byte of the request body.
    let end = 0, n = 0;
    const headers = { ...req.headers };
    delete headers.host;
    const up = request({ host: "127.0.0.1", port: r.api, path: req.url, method: req.method, headers, agent: false }, (ur) => {
      if (upload) timing.push({ relay: i + 1, path: u, bytes: n, answer_ms: Date.now() - end });
      res.writeHead(ur.statusCode, ur.headers);
      ur.pipe(res);
    });
    up.on("error", (e) => res.headersSent || res.writeHead(502).end(String(e)));
    req.on("data", (d) => (n += d.length));
    req.on("end", () => (end = Date.now()));
    req.pipe(up);
  });
  r.server.requestTimeout = 0;
  r.server.headersTimeout = 0;
  r.server.timeout = 0;
  r.server.listen(r.proxy, "127.0.0.1");
}

async function cdp(port) {
  let b;
  for (let i = 0; i < 120 && !b; i++) { b = await chromium.connectOverCDP(`http://127.0.0.1:${port}`).catch(() => null); if (!b) await sleep(500); }
  let page;
  for (let i = 0; i < 80 && !page; i++) { page = b.contexts().flatMap((c) => c.pages()).find((p) => !p.url().startsWith("devtools")); if (!page) await sleep(250); }
  await page.waitForFunction(() => !!window.__TAURI_INTERNALS__, null, { timeout: 60000 });
  return { b, page, invoke: (cmd, args = {}) => page.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a), [cmd, args]) };
}
/** `f()` with its duration; `{ ok: null }` when it is still running after `cap` ms. */
async function timed(f, cap = 120000) {
  const t = Date.now();
  const r = await Promise.race([f().then((v) => ({ ok: true, v }), (e) => ({ ok: false, v: String(e?.message ?? e) })), sleep(cap).then(() => ({ ok: null, v: `still waiting after ${cap / 1000} s` }))]);
  return { ...r, s: (Date.now() - t) / 1000 };
}
const relay = (v, sv) => (Array.isArray(v) ? v.find((x) => x.server === sv) : null);
/** The relay's signed release index for stable lists `version` for `key`. */
const indexHas = async (r, key, version) => {
  try {
    const doc = JSON.parse(readFileSync(join(r.dir, "download", "stable", "releases.json"), "utf8"));
    return JSON.parse(Buffer.from(doc.payload, "base64").toString()).releases[key]?.version === version;
  } catch {
    return false;
  }
};
/** The relay's signed release index for stable (payload), or null. */
const index = (r) => {
  try {
    return JSON.parse(Buffer.from(JSON.parse(readFileSync(join(r.dir, "download", "stable", "releases.json"), "utf8")).payload, "base64").toString());
  } catch {
    return null;
  }
};
/** Every index entry or latest.json on any relay that points at a file that relay lacks. */
const halfPublished = () => {
  const bad = [];
  for (const [i, r] of RELAYS.entries()) {
    for (const [k, e] of Object.entries(index(r)?.releases ?? {})) {
      for (const u of [...(e.urls ?? []), ...(e.packed?.urls ?? [])]) {
        if (u.startsWith("https://{relay}/download/") && !onDisk(r, u.slice("https://{relay}/download/".length))) bad.push(`relay ${i + 1} index ${k} -> ${u}`);
      }
    }
    for (const d of [QS, TECH, MACT]) {
      let m;
      try { m = JSON.parse(readFileSync(join(r.dir, "download", ...d.split("/"), "latest.json"), "utf8")); } catch { continue; }
      for (const p of Object.values(m.platforms ?? {})) {
        const f = p.url.split("/download/")[1];
        if (f && !onDisk(r, f)) bad.push(`relay ${i + 1} latest.json ${d} -> ${p.url}`);
      }
    }
  }
  return bad;
};
/** Starts `start()`, waits until `reached()` (the request to cancel in arrived), presses Cancel
 *  (release_cancel) a second later; the outcome and the seconds from Cancel to the end. */
async function cancelAt(start, reached) {
  const p = start().then((v) => ({ ok: true, v }), (e) => ({ ok: false, v: String(e?.message ?? e) }));
  for (let i = 0; i < 300 && !reached(); i++) await sleep(100);
  const valid = reached();
  await sleep(1000);
  const t = Date.now();
  await M.invoke("release_cancel", {});
  const r = await Promise.race([p, sleep(60000).then(() => ({ ok: null, v: "still waiting 60 s after Cancel" }))]);
  return { ...r, valid, after: (Date.now() - t) / 1000 };
}
const rel = (kind, v, file, platform = "windows-x86_64") => ({ args: { file, kind, platform, channel: "stable", version: v, notes: "", mandatory: false, minimum_version: "", forced: false, rollout: "now", security: false } });
const kill = (p) => { try { if (p) execFileSync("taskkill", ["/PID", String(p.pid), "/T", "/F"], { stdio: "ignore" }); } catch {} };

let manage, M;
try {
  for (const r of RELAYS) {
    for (let i = 0; i < 60 && !r.out.includes("listening"); i++) await sleep(250);
    if (!r.out.includes("listening")) throw new Error(r.out);
    // WSL forwards 127.0.0.1 a moment after tether-api listens.
    for (let i = 0; i < 60 && !(await fetch(`http://127.0.0.1:${r.api}/api/policy`).then(() => true, () => false)); i++) await sleep(250);
  }
  mkdirSync(vault);
  for (const f of ["keys.json", "test-passphrase.txt"]) copyFileSync(join(VAULT, f), join(vault, f));
  const builds = join(work, "builds");
  mkdirSync(join(builds, "quick"), { recursive: true });
  writeFileSync(join(vault, "settings.json"), JSON.stringify({ server: `${R1},${R2}`, prune: { keep: 2, auto: false }, release_folder: builds, rollout: "now" }));
  manage = spawn(MANAGE, [], { env: { ...process.env, TETHER_MANAGE_DIR: vault, WEBVIEW2_USER_DATA_FOLDER: join(work, "wv2"), WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9491" }, stdio: "ignore" });
  M = await cdp(9491);
  await M.invoke("keys_unlock", { passphrase: PASS });
  await M.page.setViewportSize({ width: 1180, height: 800 }).catch(() => {});
  const file = (name, size, fill) => { const p = join(work, name); writeFileSync(p, size > 1e6 ? randomBytes(size) : Buffer.alloc(size, fill)); return p; };

  // ---- healthy relays: a baseline release and the 200 MB timing ----
  let r = await timed(() => M.invoke("release_publish", rel("customer", "0.9.1", file("TetherQS-0.9.1.exe", 300 * 1024, 1))));
  check("baseline: 0.9.1 on both relays", r.ok && r.v.filter((x) => x.ok).length === 2, `${r.s} s ${JSON.stringify(r.v).slice(0, 300)}`);
  const big = file("Tether_0.9.5_x64-setup.exe", BIG_MB * 1e6, 0);
  timing.length = 0;
  r = await timed(() => M.invoke("release_publish", rel("technician", "0.9.5", big)), 30 * 60 * 1000);
  const bigPut = timing.filter((t) => t.path.endsWith(".exe"));
  const worst = Math.max(...bigPut.map((t) => t.answer_ms));
  check(`${BIG_MB} MB release on two healthy relays at once`, r.ok && r.v.filter((x) => x.ok).length === 2 && onDisk(RELAYS[0], `${TECH}/Tether_0.9.5_x64-setup.exe`) && onDisk(RELAYS[1], `${TECH}/Tether_0.9.5_x64-setup.exe`), `${r.s} s total`);
  check(`relay answer after the last byte of ${BIG_MB} MB is well within the 30 s answer limit`, bigPut.length === 2 && worst < 10000, `tether-api answered ${bigPut.map((t) => `${t.answer_ms} ms`).join(" / ")} after the last byte (body streamed through the proxy, as Caddy does)`);
  rmSync(big, { force: true });

  // ---- relay 1 hangs ----
  RELAYS[0].mode = "hang";
  r = await timed(() => M.invoke("release_publish", rel("customer", "0.9.2", file("TetherQS-0.9.2.exe", 300 * 1024, 2))));
  check("D1 release_publish, relay 1 silent: ends within 60 s", r.ok !== null && r.s < 60, `${r.s} s`);
  check("D1 relay 1 gets a per-relay error, relay 2 publishes", relay(r.v, R1)?.ok === false && relay(r.v, R2)?.ok === true && onDisk(RELAYS[1], `${QS}/TetherQS-0.9.2.exe`) && (await indexHas(RELAYS[1], "customer/windows-x86_64", "0.9.2")), (relay(r.v, R1)?.error ?? JSON.stringify(r.v)).slice(0, 160));

  const several = [];
  for (const [kind, v, n] of [["customer", "0.9.3", "TetherQS-0.9.3.exe"], ["technician", "0.9.3", "Tether_0.9.3_x64-setup.exe"]]) several.push(await timed(() => M.invoke("release_publish", rel(kind, v, file(n, 300 * 1024, 3)))));
  check("D1 several builds, relay 1 silent: each ends within 60 s, relay 2 gets both", several.every((x) => x.ok && x.s < 60 && relay(x.v, R2)?.ok && !relay(x.v, R1)?.ok), several.map((x) => `${x.s} s`).join(", "));

  const skipped = [];
  for (const [kind, v, n] of [["customer", "0.9.4", "TetherQS-0.9.4.exe"], ["technician", "0.9.4", "Tether_0.9.4_x64-setup.exe"]]) skipped.push(await timed(() => M.invoke("release_publish", { ...rel(kind, v, file(n, 300 * 1024, 4)), skip: [R1] })));
  check("D2 several builds with relay 1 left out: each ends within 15 s, relay 1 'skipped (not reachable)'", skipped.every((x) => x.ok && x.s < 15 && relay(x.v, R1)?.error === "skipped (not reachable)" && relay(x.v, R2)?.ok), skipped.map((x) => `${x.s} s`).join(", "));
  check("D2 relay 2 has the left-out publish, relay 1's data dir is untouched", onDisk(RELAYS[1], `${TECH}/Tether_0.9.4_x64-setup.exe`) && !onDisk(RELAYS[0], `${TECH}/Tether_0.9.4_x64-setup.exe`));

  // UI: Publish the latest builds, "Publish to the others anyway" ticked.
  writeFileSync(join(builds, "quick", "TetherQS-0.9.6.exe"), Buffer.alloc(300 * 1024, 6));
  mkdirSync(join(builds, "windows"), { recursive: true });
  writeFileSync(join(builds, "windows", "Tether_0.9.6_x64-setup.exe"), Buffer.alloc(300 * 1024, 6));
  await M.page.reload();
  await sleep(2000);
  const nav = () => M.page.getByRole("navigation", { name: "Sections" }).getByRole("button", { name: /^Releases/ }).click();
  const dlg = M.page.getByRole("dialog");
  const untilDone = async (cap) => {
    const t = Date.now();
    while (Date.now() - t < cap) {
      if (await dlg.getByRole("button", { name: "Done" }).isVisible().catch(() => false)) return (Date.now() - t) / 1000;
      await sleep(250);
    }
    return null;
  };
  try {
    await nav();
    await M.page.getByRole("button", { name: /^Publish 2 new builds/ }).click({ timeout: 40000 });
    await dlg.getByRole("checkbox").check({ timeout: 30000 });
    const label = await dlg.getByRole("button", { name: /^Sign and publish/ }).innerText();
    await shot(M.page, "hang-ui-review");
    await dlg.getByRole("button", { name: /^Sign and publish/ }).click();
    const s = await untilDone(60000);
    const text = await dlg.innerText();
    await shot(M.page, "hang-ui-anyway-done");
    check("D2 UI 'Publish to the others anyway': two builds and the page done within 30 s", s !== null && s < 30, `${s} s, button "${label}"`);
    check("D2 UI result: relay 1 skipped (not reachable), relay 2 published", text.includes("skipped (not reachable)") && onDisk(RELAYS[1], `${QS}/TetherQS-0.9.6.exe`) && onDisk(RELAYS[1], `${TECH}/Tether_0.9.6_x64-setup.exe`) && !onDisk(RELAYS[0], `${QS}/TetherQS-0.9.6.exe`), text.replace(/\s+/g, " ").slice(0, 300));
    await dlg.getByRole("button", { name: "Done" }).click();
  } catch (e) {
    check("D2 UI 'Publish to the others anyway'", false, e.message.split("\n")[0]);
    await shot(M.page, "hang-ui-anyway-error");
  }

  // UI: Cancel while relay 1 takes the connection but never answers an upload.
  RELAYS[0].mode = "hangput";
  writeFileSync(join(builds, "quick", "TetherQS-0.9.7.exe"), Buffer.alloc(300 * 1024, 7));
  await M.page.reload();
  await sleep(2000);
  try {
    await nav();
    await M.page.getByRole("button", { name: /^Publish 1 new build/ }).click({ timeout: 40000 });
    await dlg.getByRole("button", { name: /^Sign and publish · 2 relays/ }).click({ timeout: 30000 });
    await sleep(6000);
    await shot(M.page, "hang-ui-busy-per-relay");
    const busy = await dlg.innerText();
    const bars = await dlg.getByRole("progressbar").count();
    const cancel = dlg.getByRole("button", { name: "Cancel" });
    const enabled = await cancel.isEnabled();
    await cancel.click();
    const s = await untilDone(15000);
    const text = await dlg.innerText();
    await shot(M.page, "hang-ui-cancelled");
    check("D1 UI progress per relay while uploading (relay 2 finished, relay 1 waiting)", /Relay 1 of 2/.test(busy) && /files uploaded, waiting for the other relays/.test(busy) && bars >= 1, busy.replace(/\s+/g, " ").slice(-260));
    check("D1 UI Cancel is enabled while publishing and ends it within 3 s", enabled && s !== null && s < 3, `${s} s`);
    check("D1 UI after Cancel: 'Cancelled', nothing published on relay 2", /Cancelled/.test(text) && !(await indexHas(RELAYS[1], "customer/windows-x86_64", "0.9.7")), text.replace(/\s+/g, " ").slice(0, 300));
    await dlg.getByRole("button", { name: "Done" }).click();
  } catch (e) {
    check("D1 UI Cancel", false, e.message.split("\n")[0]);
    await shot(M.page, "hang-ui-cancel-error");
  }

  // ---- the other release calls, relay 1 silent (relay 2 publishes 0.9.6, relay 1 lacks it) ----
  RELAYS[0].mode = "hang";
  r = await timed(() => M.invoke("release_sync", { channel: "stable", kind: "customer", platform: "windows-x86_64", version: "0.9.6" }));
  check("D2 release_sync to silent relay 1: ends within 60 s, relay 1 error, relay 2 already", r.ok && r.s < 60 && relay(r.v, R1)?.ok === false && relay(r.v, R2)?.detail?.already, `${r.s} s: ${(relay(r.v, R1)?.error ?? JSON.stringify(r.v)).slice(0, 140)}`);
  r = await timed(() => M.invoke("release_sync", { channel: "stable", kind: "customer", platform: "windows-x86_64", version: "0.9.6", skip: [R1] }));
  check("D2 release_sync with relay 1 left out: within 5 s, 'skipped (not reachable)'", r.ok && r.s < 5 && relay(r.v, R1)?.error === "skipped (not reachable)", `${r.s} s`);
  // Relay 1 answers reads but its uploads hang: the copy reaches it through api::file, bounded.
  RELAYS[0].mode = "hangput";
  r = await timed(() => M.invoke("release_sync", { channel: "stable", kind: "technician", platform: "windows-x86_64", version: "0.9.6" }));
  check("D2 release_sync, relay 1 takes uploads but never answers: within 60 s, relay 1 error", r.ok && r.s < 60 && relay(r.v, R1)?.ok === false, `${r.s} s: ${(relay(r.v, R1)?.error ?? JSON.stringify(r.v)).slice(0, 140)}`);
  RELAYS[0].mode = "hang";

  r = await timed(() => M.invoke("release_control", { channel: "stable", key: "customer/windows-x86_64", action: "halt", preset: null }));
  const e1 = relay(r.v, R1)?.error ?? "";
  check("D4 release_control halt: within 30 s, relay 1 'not reachable' (not 'lacks …')", r.ok && r.s < 30 && /not reachable/.test(e1) && !/lacks/.test(e1) && relay(r.v, R2)?.ok, `${r.s} s: ${e1.slice(0, 160)}`);
  await M.invoke("release_control", { channel: "stable", key: "customer/windows-x86_64", action: "resume", preset: null });

  // Older than a day, so the clean-up offers versions (the relays keep uploads of the last 24 h).
  const old = new Date(Date.now() - 3 * 86400e3);
  const age = (d) => readdirSync(d, { withFileTypes: true }).forEach((e) => (e.isDirectory() ? age(join(d, e.name)) : utimesSync(join(d, e.name), old, old)));
  RELAYS.forEach((x) => age(join(x.dir, "download")));
  r = await timed(() => M.invoke("release_prune_plan", { keep: 0 }));
  check("D5 prune plan: within 15 s, relay 1 listed as blocked, nothing offered", r.ok && r.s < 15 && r.v.blocked?.[0]?.server === R1 && r.v.offer.versions === 0 && r.v.groups.some((g) => g.versions.some((v) => v.withheld)), `${r.s} s`);
  r = await timed(() => M.invoke("release_prune", { keep: 0, delete: [{ channel: "stable", dir: "customer/windows-x86_64", version: "0.9.1" }] }));
  check("D5 prune: within 15 s, per-relay results naming relay 1, nothing deleted", r.ok && r.s < 15 && /not reachable/.test(relay(r.v, R1)?.error ?? "") && /does not answer/.test(relay(r.v, R2)?.error ?? "") && onDisk(RELAYS[1], `${QS}/TetherQS-0.9.1.exe`), `${r.s} s: ${relay(r.v, R2)?.error}`);
  await nav().catch(() => {});
  try {
    await M.page.getByRole("button", { name: /Clean up old releases/ }).click({ timeout: 20000 });
    await dlg.getByText("Nothing can be deleted while a relay does not answer.").waitFor({ timeout: 30000 });
    await shot(M.page, "hang-ui-cleanup-blocked");
    check("D5 UI clean-up sheet names the silent relay", (await dlg.innerText()).includes(`127.0.0.1:${RELAYS[0].proxy}`));
    await dlg.getByRole("button", { name: "Cancel" }).click();
  } catch (e) {
    check("D5 UI clean-up sheet", false, e.message.split("\n")[0]);
  }

  // UI: two builds, relay 1 answers the check but its uploads hang (no "anyway" box): the first
  // build fails there after the answer limit, the second build and the page leave it out.
  RELAYS[0].mode = "hangput";
  writeFileSync(join(builds, "quick", "TetherQS-0.9.8.exe"), Buffer.alloc(300 * 1024, 8));
  writeFileSync(join(builds, "windows", "Tether_0.9.8_x64-setup.exe"), Buffer.alloc(300 * 1024, 8));
  await M.page.reload();
  await sleep(2000);
  try {
    await nav();
    await M.page.getByRole("button", { name: /^Publish 2 new builds/ }).click({ timeout: 40000 });
    await dlg.getByRole("button", { name: /^Sign and publish · 2 relays/ }).click({ timeout: 30000 });
    const s = await untilDone(120000);
    const text = await dlg.innerText();
    await shot(M.page, "hang-ui-several-silent");
    check("D1 UI two builds, relay 1 stops answering uploads: done within 60 s, relay 2 has both", s !== null && s < 60 && onDisk(RELAYS[1], `${QS}/TetherQS-0.9.8.exe`) && onDisk(RELAYS[1], `${TECH}/Tether_0.9.8_x64-setup.exe`), `${s} s`);
    check("D1 UI the second build skips relay 1 after the first failed there", /No answer from the server within 30 s after the upload/.test(text) && /skipped \(not reachable\)/.test(text), text.replace(/\s+/g, " ").slice(0, 400));
    await dlg.getByRole("button", { name: "Done" }).click();
  } catch (e) {
    check("D1 UI two builds with relay 1 silent", false, e.message.split("\n")[0]);
  }

  RELAYS[0].mode = "ok";
  r = await timed(() => M.invoke("release_prune_plan", { keep: 0 }));
  check("D5 with every relay answering, the plan offers versions again", r.ok && !r.v.blocked.length && r.v.offer.versions > 0, `${r.v?.offer?.versions} versions`);

  // ---- judge 4 ----
  const settings = (list, extra = {}) => writeFileSync(join(vault, "settings.json"), JSON.stringify({ server: list.join(","), prune: { keep: 2, auto: false }, release_folder: builds, rollout: "now", ...extra }));
  const short = (x) => (typeof x === "string" ? x : JSON.stringify(x)).replace(/\s+/g, " ").slice(0, 300);
  const reset = () => { RELAYS.forEach((x) => { x.mode = "ok"; x.rule = () => null; }); held.splice(0).forEach((sk) => sk.destroy()); };
  // J0: relay 1 missed several publishes (its index is behind and names none of their files).
  // Publish to missing of one release must copy every file the newest index names before the index.
  r = await timed(() => M.invoke("release_sync", { channel: "stable", kind: "customer", platform: "windows-x86_64", version: "0.9.8" }));
  let bad = halfPublished();
  check("J0 Publish to missing brings relay 1 up: the index's other releases are copied too, nothing half-published", r.ok && relay(r.v, R1)?.ok && index(RELAYS[0])?.serial === index(RELAYS[1])?.serial && onDisk(RELAYS[0], `${TECH}/Tether_0.9.8_x64-setup.exe`) && !bad.length, `${r.s} s ${bad.join("; ") || short(r.v)}`);

  // J1: three relays; relay 3 is new and empty; relay 1 (the first source) stalls file bodies.
  settings([R1, R2, R3]);
  const macDir = join(work, "mac");
  mkdirSync(macDir);
  writeFileSync(join(macDir, "Tether_0.9.12_universal.dmg"), randomBytes(1 << 20));
  writeFileSync(join(macDir, "Tether_0.9.12_universal.app.tar.gz"), randomBytes(1 << 20));
  r = await timed(() => M.invoke("release_publish", { ...rel("technician", "0.9.12", join(macDir, "Tether_0.9.12_universal.app.tar.gz"), "darwin-universal"), skip: [R3] }));
  check("J1 prep: relays 1 and 2 publish mac 0.9.12 (4 files), relay 3 left out", r.ok && relay(r.v, R1)?.ok && relay(r.v, R2)?.ok && !index(RELAYS[2]), short(r.v));
  RELAYS[0].rule = (req, u) => (req.method === "GET" && /^\/download\/.*\.(dmg|tar\.gz|sig)$/.test(u) ? "stallbody" : null);
  r = await timed(() => M.invoke("release_sync", { channel: "stable", kind: "technician", platform: "darwin-universal", version: "0.9.12" }));
  check("J1 Publish to missing, 4 files, first source stalls: one stall (< 45 s), not one per file; relay 3 publishes", r.ok && r.s < 45 && relay(r.v, R3)?.ok && relay(r.v, R3)?.detail?.url, `${r.s} s ${short(r.v)}`);
  bad = halfPublished();
  check("J1 nothing half-published: relay 3's index names only files it has", !bad.length && index(RELAYS[2])?.serial === index(RELAYS[1])?.serial && onDisk(RELAYS[2], `${QS}/TetherQS-0.9.8.exe`), bad.join("; ") || `serial ${index(RELAYS[2])?.serial}`);
  reset();
  settings([R1, R2]);

  // J2-J4: Cancel before the index write (relay 1 never answers the index read), during it, and
  // during latest.json.
  let seen = false;
  RELAYS[0].rule = (req, u) => (req.method === "GET" && u === "/download/stable/releases.json" ? ((seen = true), "hang") : null);
  const before = index(RELAYS[1])?.serial;
  r = await cancelAt(() => M.invoke("release_publish", rel("customer", "0.9.21", file("TetherQS-0.9.21.exe", 300 * 1024, 21))), () => seen);
  check("J2 Cancel while the index is read (before any write): 'Cancelled', nothing published, ends within 12 s", r.valid && r.ok === false && /Cancelled.*Nothing was published/.test(r.v) && r.after < 12 && index(RELAYS[1])?.serial === before && !halfPublished().length, `${r.after} s after Cancel: ${short(r.v)}`);
  reset();
  seen = false;
  RELAYS[0].rule = (req, u) => (req.method === "PUT" && u.endsWith("/releases.json") ? ((seen = true), "hang") : null);
  r = await cancelAt(() => M.invoke("release_publish", rel("customer", "0.9.22", file("TetherQS-0.9.22.exe", 300 * 1024, 22))), () => seen);
  const late = (v) => (Array.isArray(v) ? v.find((x) => x.server === "Cancel") : null);
  check("J3 Cancel during the index write: finishes (index + latest.json) and says 'Too late to cancel … published on relay 2'", r.valid && r.ok && /Too late to cancel/.test(late(r.v)?.error ?? "") && (late(r.v)?.error ?? "").includes(R2) && relay(r.v, R2)?.ok && index(RELAYS[1])?.releases["customer/windows-x86_64"]?.version === "0.9.22" && !halfPublished().length, `${r.after} s after Cancel: ${short(r.v)}`);
  reset();
  seen = false;
  RELAYS[0].rule = (req, u) => (req.method === "PUT" && u.endsWith("/latest.json") ? ((seen = true), "hang") : null);
  r = await cancelAt(() => M.invoke("release_publish", rel("customer", "0.9.23", file("TetherQS-0.9.23.exe", 300 * 1024, 23))), () => seen);
  check("J4 Cancel during latest.json: finishes and says 'Too late to cancel', never a plain success", r.valid && r.ok && /Too late to cancel/.test(late(r.v)?.error ?? "") && relay(r.v, R2)?.ok && !halfPublished().length, `${r.after} s after Cancel: ${short(r.v)}`);
  reset();

  // J5: Cancel pressed while the UI starts the next build: that build carries the run id.
  RELAYS.forEach((x) => (x.hits = 0));
  await M.invoke("release_cancel", { run: 4242 });
  r = await timed(() => M.invoke("release_publish", { ...rel("customer", "0.9.24", file("TetherQS-0.9.24.exe", 300 * 1024, 24)), run: 4242 }));
  check("J5 Cancel between two builds is not lost: the next build of that run stops at once, no relay contacted", r.ok === false && /Cancelled/.test(r.v) && r.s < 3 && RELAYS[0].hits + RELAYS[1].hits === 0, `${r.s} s: ${short(r.v)}`);
  r = await timed(() => M.invoke("release_publish", rel("customer", "0.9.24", file("TetherQS-0.9.24.exe", 300 * 1024, 24))));
  check("J5 a new run after it publishes normally", r.ok && relay(r.v, R1)?.ok && relay(r.v, R2)?.ok, short(r.v));

  // J6: relay 1 left out with Clean up after each publish on: it gets no request at all.
  settings([R1, R2], { prune: { keep: 0, auto: true } });
  RELAYS.forEach((x) => (x.hits = 0));
  r = await timed(() => M.invoke("release_publish", { ...rel("customer", "0.9.25", file("TetherQS-0.9.25.exe", 300 * 1024, 25)), skip: [R1] }));
  const page = await timed(() => M.invoke("page_render", { title: "t", intro: "i", technician: true, customer: true, skip: [R1] }));
  const cleanup = Array.isArray(r.v) ? r.v.find((x) => x.server === "Cleanup") : null;
  check("J6 left-out relay gets no request (publish, auto clean-up, page render); clean-up skipped and says why", r.ok && relay(r.v, R2)?.ok && RELAYS[0].hits === 0 && page.ok && /left out/.test(cleanup?.error ?? "") && RELAYS[1].hits > 0, `relay 1 hits ${RELAYS[0].hits}; ${short(cleanup)}`);
  settings([R1, R2]);

  // J7: every relay silent: per-relay "not reachable", not "not found".
  RELAYS[0].mode = RELAYS[1].mode = "hang";
  const silentAll = (v) => /No relay answered/.test(v) && v.includes(R1) && v.includes(R2) && /not reachable/.test(v);
  r = await timed(() => M.invoke("release_control", { channel: "stable", key: "customer/windows-x86_64", action: "halt", preset: null }));
  check("J7 halt, every relay silent: 'No relay answered … not reachable', not 'not in the release index'", r.ok === false && silentAll(r.v) && !/is not in the release index/.test(r.v) && r.s < 15, `${r.s} s: ${short(r.v)}`);
  r = await timed(() => M.invoke("release_sync", { channel: "stable", kind: "customer", platform: "windows-x86_64", version: "0.9.25" }));
  check("J7 Publish to missing, every relay silent: names them, not 'No relay publishes'", r.ok === false && silentAll(r.v) && !/No relay publishes/.test(r.v) && r.s < 15, `${r.s} s: ${short(r.v)}`);
  r = await timed(() => M.invoke("policy_sync"));
  check("J7 policy sync, every relay silent: names them, not 'No relay serves a policy'", r.ok === false && silentAll(r.v) && !/No relay serves/.test(r.v) && r.s < 15, `${r.s} s: ${short(r.v)}`);
  reset();

  // J8: diagnostics upload opened without a relay: relay 2 answers 404, relay 1 is silent.
  RELAYS[0].mode = "hang";
  r = await timed(() => M.invoke("diag_open", { ticket: "ABCDEFGH", relay: null }));
  const i2 = String(r.v).indexOf(`${R2}: not on this relay`), i1 = String(r.v).indexOf(`${R1}: not reachable`);
  check("J8 diag_open, one 404 and one silent relay: both shown, the 404 first", r.ok === false && i2 >= 0 && i1 > i2 && r.s < 15, `${r.s} s: ${short(r.v)}`);
  reset();
} catch (e) {
  check("run", false, e.stack ?? String(e));
} finally {
  await M?.b.close().catch(() => {});
  kill(manage);
  held.forEach((sk) => sk.destroy());
  for (const r of RELAYS) {
    try { execFileSync("wsl", ["-d", "Ubuntu-22.04", "--", "pkill", "-f", `tether-api --data ${wslPath(r.dir)}`], { stdio: "ignore" }); } catch {}
    r.proc.kill();
    r.server.closeAllConnections?.();
    r.server.close();
  }
  await sleep(500);
  rmSync(work, { recursive: true, force: true });
  const failed = results.filter((x) => !x.ok).length;
  console.log(`\n${results.length - failed}/${results.length} PASS`);
  process.exit(failed ? 1 : 0);
}

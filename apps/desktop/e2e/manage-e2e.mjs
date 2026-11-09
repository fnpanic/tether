// End-to-end test of Tether Manage 0.5 against two local relays (tether-api trusting the TEST admin
// key; start them with C:\vms\e2e-relays.sh in WSL): publish to all relays, releases overview and
// "publish to missing relays", policy publish + live read-back, custom apps (create, download via
// /q/<token>, run the branded TetherQS, new link, revoke) and the masked password of plain TetherQS.
//
//   node e2e/manage-e2e.mjs
// Env: MANAGE_DIR (test vault, default C:\vms\manage-test), MANAGE_EXE, QS_EXE (TetherQS built with
// the test keys), RELAYS (default http://127.0.0.1:21131,http://127.0.0.1:21132).
import { spawn, execFileSync } from "node:child_process";
import { copyFileSync, existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { chromium } from "playwright-core";

const DIR = process.env.MANAGE_DIR || "C:\\vms\\manage-test";
const EXE = process.env.MANAGE_EXE || "C:\\cargo-target\\manage\\debug\\tether-manage.exe";
const QS = process.env.QS_EXE || "C:\\cargo-target\\tether\\release\\TetherQS.exe";
// Caddy stand-ins: /download/* from the relay's data dir (as Caddy's file_server), the rest proxied
// to tether-api (21131/21132 in WSL, data in ~/e2e-api/d1, d2).
const API = [21131, 21132];
const RELAYS = API.map((p) => `http://127.0.0.1:${p + 10}`);
const { createServer } = await import("node:http");
API.forEach((port, i) => {
  const data = `\\\\wsl$\\Ubuntu-22.04\\home\\thomas\\e2e-api\\d${i + 1}`;
  createServer(async (req, res) => {
    if (req.url.startsWith("/download/")) {
      const f = join(data, ...decodeURIComponent(req.url.split("?")[0]).split("/").filter((s) => s && s !== ".."));
      if (!existsSync(f)) return res.writeHead(404).end();
      return res.writeHead(200, { "cache-control": "no-cache" }).end(readFileSync(f));
    }
    const body = await new Promise((r) => { const c = []; req.on("data", (d) => c.push(d)); req.on("end", () => r(Buffer.concat(c))); });
    const headers = { ...req.headers };
    delete headers.host;
    const up = await fetch(`http://127.0.0.1:${port}${req.url}`, { method: req.method, headers, body: body.length ? body : undefined });
    const out = {};
    up.headers.forEach((v, k) => { if (k !== "content-length" && k !== "transfer-encoding") out[k] = v; });
    res.writeHead(up.status, out).end(Buffer.from(await up.arrayBuffer()));
  }).listen(port + 10, "127.0.0.1");
});
const PNG = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const results = [];
const check = (name, ok, detail = "") => {
  results.push({ name, ok: !!ok });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? `  (${detail})` : ""}`);
};

async function cdpApp(exe, args, env, port) {
  // WEBVIEW2_USER_DATA_FOLDER (optional, e.g. when a stray WebView2 holds an app's default folder): one folder per app.
  const wv2 = process.env.WEBVIEW2_USER_DATA_FOLDER ? { WEBVIEW2_USER_DATA_FOLDER: `${process.env.WEBVIEW2_USER_DATA_FOLDER}-${port}` } : {};
  const p = spawn(exe, args, { env: { ...process.env, ...env, ...wv2, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` }, stdio: "ignore" });
  let browser;
  for (let i = 0; i < 80 && !browser; i++) {
    browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`).catch(() => null);
    if (!browser) await sleep(500);
  }
  if (!browser) throw new Error(`${exe}: no CDP`);
  let page;
  for (let i = 0; i < 40 && !page; i++) {
    page = browser.contexts().flatMap((c) => c.pages())[0];
    if (!page) await sleep(250);
  }
  await page.waitForFunction(() => !!window.__TAURI_INTERNALS__);
  return { p, page, stop: async () => { await browser.close().catch(() => {}); p.kill(); } };
}

const get = async (url) => {
  const r = await fetch(url);
  return { status: r.status, headers: r.headers, body: Buffer.from(await r.arrayBuffer()) };
};

// The vault's relay list for this test; restored afterwards.
const settingsPath = join(DIR, "settings.json");
const savedSettings = readFileSync(settingsPath);
writeFileSync(settingsPath, JSON.stringify({ server: RELAYS.join(",") }, null, 2));

const m = await cdpApp(EXE, [], { TETHER_MANAGE_DIR: DIR }, 9446);
const invoke = (cmd, args = {}) => m.page.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a), [cmd, args]);
try {
  await invoke("keys_unlock", { passphrase: readFileSync(join(DIR, "test-passphrase.txt"), "utf8").trim() });

  // Relays
  const health = await invoke("relays_check");
  check("both relays reachable", health.length === 2 && health.every((r) => r.ok), JSON.stringify(health.map((r) => r.ok)));

  // Release to every relay
  const ver = JSON.parse(readFileSync("C:\\install\\remote-support\\apps\\quick\\package.json", "utf8")).version;
  const rel = await invoke("release_publish", { args: { file: QS, kind: "customer", platform: "windows-x86_64", channel: "stable", version: ver, notes: "e2e", mandatory: false, minimum_version: "", forced: false } });
  check("release published to both relays", rel.length === 2 && rel.every((r) => r.ok), JSON.stringify(rel.map((r) => r.error ?? "ok")));
  let ov = await invoke("releases_overview");
  const row = (o) => JSON.stringify(o).includes(ver);
  check("releases overview lists the version", row(ov));

  // Remove it from relay 2 by hand, then "publish to missing relays"
  execFileSync("wsl", ["-d", "Ubuntu-22.04", "--", "bash", "-c", "rm -rf ~/e2e-api/d2/download/stable/customer"], { env: { ...process.env, MSYS_NO_PATHCONV: "1" } });
  ov = await invoke("releases_overview");
  const r2 = ov.rows?.find((r) => r.kind === "customer" && r.platform === "windows-x86_64")?.relays?.[1];
  check("overview shows the version missing on relay 2", r2 && !r2.published && !r2.has_latest, JSON.stringify(r2));
  const sync = await invoke("release_sync", { channel: "stable", kind: "customer", platform: "windows-x86_64", version: ver });
  check("publish to missing relays", sync.some((r) => r.ok), JSON.stringify(sync.map((r) => `${r.server}:${r.ok ? "ok" : r.error}`)));

  // Policy publish + live read-back
  const live0 = await invoke("policy_live");
  const draft = await invoke("policy_draft");
  const pub = await invoke("policy_publish", { bundle: draft });
  const live = await invoke("policy_live");
  const serials = JSON.stringify(live).match(/"serial":\s*(\d+)/g) || [];
  check("policy published and read back from both relays", JSON.stringify(pub).length > 2 && serials.length >= 2, `${serials.join(" ")}; before ${JSON.stringify(live0).slice(0, 80)}`);

  // Custom app
  const created = await invoke("custom_save", { args: { id: null, name: "ACME Support", accent: "#c8102e", welcome: "Hello from ACME", logo_png: PNG, layer: { "security.approve-mode": { state: "locked", value: "click" } }, channel: "stable", days: 30, new_link: false } });
  check("custom app created on both relays", created.relays.length === 2 && created.relays.every((r) => r.ok), created.link);
  const token = created.link.split("/q/")[1];
  const dl = await get(`${RELAYS[1]}/q/${token}`);
  const plain = readFileSync(QS);
  // A signed TetherQS gets the trailer inside its certificate table (two size fields change), so
  // compare up to the PE header only and let Windows judge the signature.
  check("link downloads TetherQS + signed trailer (relay 2)", dl.status === 200 && dl.body.subarray(0, 0x100).equals(plain.subarray(0, 0x100)) && dl.body.length > plain.length && dl.body.subarray(-8).toString() === "TQSPROF1", `${dl.status} ${dl.body.length} bytes, plain ${plain.length}`);
  check("download filename", (dl.headers.get("content-disposition") || "").includes("ACME Support-Support.exe"), dl.headers.get("content-disposition"));
  check("unknown token 404", (await get(`${RELAYS[0]}/q/${"A".repeat(43)}`)).status === 404);
  // MAC_ZIP_OUT: save the macOS download (relay 1 needs a darwin-universal release) for the
  // signature check on a Mac (docs/code-signing.md).
  if (process.env.MAC_ZIP_OUT) {
    const mac = await get(`${RELAYS[0]}/q/${token}/mac`);
    check("macOS link downloads the renamed zip", mac.status === 200 && mac.body.subarray(0, 2).toString() === "PK", `${mac.status} ${mac.body.length} bytes`);
    writeFileSync(process.env.MAC_ZIP_OUT, mac.body);
  }

  // Run the branded exe
  const dir = mkdtempSync(join(tmpdir(), "tqs-e2e-"));
  const branded = join(dir, "ACME Support-Support.exe");
  writeFileSync(branded, dl.body);
  const sigOf = (f) => execFileSync("powershell", ["-NoProfile", "-Command", `(Get-AuthenticodeSignature -LiteralPath '${f}').Status`]).toString().trim();
  const plainSig = sigOf(QS);
  check("custom exe keeps the Authenticode signature", plainSig === "NotSigned" || sigOf(branded) === plainSig, `plain ${plainSig}, custom ${sigOf(branded)}`);
  const qs = await cdpApp(branded, [], {}, 9447);
  try {
    await qs.page.waitForSelector('[data-testid="qs-title"]', { timeout: 30000 });
    const title = await qs.page.locator('[data-testid="qs-title"]').innerText();
    const logo = await qs.page.locator('[data-testid="qs-logo"]').count();
    const body = await qs.page.evaluate(() => document.body.innerText);
    check("branded TetherQS shows custom name", title.includes("ACME Support"), title);
    check("branded TetherQS shows logo", logo > 0);
    check("branded TetherQS shows welcome text", body.includes("Hello from ACME"));
    check("custom setting applied (approve by click: no password shown)", (await qs.page.locator('[data-testid="qs-password"]').count()) === 0);
  } finally {
    await qs.stop();
  }
  await sleep(1500);

  // Plain TetherQS: masked password
  const plainCopy = join(dir, "TetherQS.exe");
  copyFileSync(QS, plainCopy);
  const pq = await cdpApp(plainCopy, [], {}, 9448);
  try {
    const el = pq.page.locator('[data-testid="qs-password"]');
    await el.waitFor({ timeout: 30000 });
    await pq.page.waitForFunction(() => (document.querySelector('[data-testid="qs-password"]')?.getAttribute("data-password") || "").length > 0, null, { timeout: 30000 });
    const shown = await el.innerText();
    const real = await el.getAttribute("data-password");
    check("plain TetherQS masks the one-time password", real && !shown.includes(real) && /^[•●*\s]+$/.test(shown.trim()), `shown "${shown}"`);
  } finally {
    await pq.stop();
  }

  // List, new link, revoke
  const list = await invoke("custom_list");
  const entry = list.find((c) => c.id === created.id);
  const want = process.env.MAC_ZIP_OUT ? 2 : 1; // the macOS download counts too
  check(`custom list shows ${want} download(s)`, entry && entry.downloads === want, JSON.stringify(entry ? { downloads: entry.downloads, expires: entry.expires } : null));
  const renewed = await invoke("custom_save", { args: { id: created.id, name: "ACME Support", accent: "#c8102e", welcome: "Hello from ACME", logo_png: PNG, layer: {}, channel: "stable", days: 30, new_link: true } });
  const token2 = renewed.link.split("/q/")[1];
  check("new link: old link dead", (await get(`${RELAYS[0]}/q/${token}`)).status === 404);
  check("new link works", (await get(`${RELAYS[0]}/q/${token2}`)).status === 200);
  const del = await invoke("custom_delete", { id: created.id });
  check("revoke on both relays", del.every((r) => r.ok), JSON.stringify(del.map((r) => r.ok)));
  check("revoked link 404 on both relays", (await get(`${RELAYS[0]}/q/${token2}`)).status === 404 && (await get(`${RELAYS[1]}/q/${token2}`)).status === 404);
} catch (e) {
  check("no unexpected error", false, e?.message ?? String(e));
} finally {
  await m.stop();
  writeFileSync(settingsPath, savedSettings);
}
const failed = results.filter((r) => !r.ok).length;
console.log(`\n${results.length - failed}/${results.length} passed`);
process.exit(failed ? 1 : 0);

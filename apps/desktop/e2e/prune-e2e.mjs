// Release clean-up end to end (docs/release-cleanup.md): Tether Manage publishes six TetherQS
// versions to a local tether-api, then cleans up through its backend commands, and the files on
// the relay's disk are checked after every step.
// - tether-api from scripts/build-relay.sh (WSL, BIN), data dir in a temp folder on this PC;
// - a small proxy stands in for Caddy (/download/* from the data dir, the rest to tether-api);
// - Tether Manage (debug exe with the frontend, `npx tauri build --debug --no-bundle`) on a COPY of
//   the test vault (C:\vms\manage-test), driven over CDP through window.__TAURI_INTERNALS__.
// Checks: the merged plan keeps the published version, the previous one (keep 1), a CDN version
// (cdn-assets.json pin) and offers the rest; the execute deletes exactly that and keeps a
// published version named in the list; the single-file DELETE of the published exe is refused
// (409); with "Clean up after each publish" on (keep 0) the next publish cleans up and reports it.
//
//   node e2e/prune-e2e.mjs
// Env: MANAGE_EXE, BIN (WSL dir with tether-api), VAULT.
import { execFileSync, spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { copyFileSync, createReadStream, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createServer } from "node:http";
import { chromium } from "playwright-core";

const MANAGE = process.env.MANAGE_EXE || "C:\\cargo-target\\prune\\debug\\tether-manage.exe";
const BIN = process.env.BIN || "/home/thomas/prune-build/bin";
const VAULT = process.env.VAULT || "C:\\vms\\manage-test";
const API_PORT = 22150;
const PROXY = "http://127.0.0.1:22151";
const QS = "stable/customer/windows-x86_64";
const PASS = readFileSync(join(VAULT, "test-passphrase.txt"), "utf8").trim();
const ADMIN_PUB = readFileSync(join(VAULT, "env.txt"), "utf8").trim().split(/\s+/)[0];
const work = mkdtempSync(join(tmpdir(), "prune-e2e-"));
const apiDir = join(work, "api");
const vault = join(work, "vault");
const wslPath = (p) => "/mnt/" + p[0].toLowerCase() + p.slice(2).replace(/\\/g, "/");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const results = [];
const check = (name, ok, detail = "") => {
  results.push({ name, ok: !!ok });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? `  (${detail})` : ""}`);
};
const has = (f) => existsSync(join(apiDir, "download", ...`${QS}/${f}`.split("/")));
const qsFiles = (v) => [`TetherQS-${v}.exe`, `TetherQS-${v}.exe.sig`, `TetherQS-${v}.exe.gz`];
const whole = (v) => qsFiles(v).every(has);
const none = (v) => !qsFiles(v).some(has);

// ---- tether-api in WSL and the Caddy stand-in ----
mkdirSync(apiDir);
const api = spawn("wsl", ["-d", "Ubuntu-22.04", "--", "env", `ADMIN_PUB_KEY=${ADMIN_PUB}`, `${BIN}/tether-api`, "--data", wslPath(apiDir), "--listen", `127.0.0.1:${API_PORT}`], { stdio: ["ignore", "pipe", "pipe"] });
let apiOut = "";
api.stdout.on("data", (d) => (apiOut += d));
api.stderr.on("data", (d) => (apiOut += d));
const proxy = createServer(async (req, res) => {
  if (req.url.startsWith("/download/") && !req.url.endsWith("/releases.json")) {
    const f = join(apiDir, ...decodeURIComponent(req.url.split("?")[0]).split("/").filter((s) => s && s !== ".."));
    if (!existsSync(f) || !statSync(f).isFile()) return res.writeHead(404).end();
    res.writeHead(200, { "cache-control": "no-cache" });
    return createReadStream(f).pipe(res);
  }
  const body = await new Promise((r) => { const c = []; req.on("data", (d) => c.push(d)); req.on("end", () => r(Buffer.concat(c))); });
  const headers = { ...req.headers };
  delete headers.host;
  const up = await fetch(`http://127.0.0.1:${API_PORT}${req.url}`, { method: req.method, headers, body: body.length ? body : undefined });
  const out = {};
  up.headers.forEach((v, k) => { if (k !== "content-length" && k !== "transfer-encoding") out[k] = v; });
  res.writeHead(up.status, out).end(Buffer.from(await up.arrayBuffer()));
}).listen(22151, "127.0.0.1");

// ---- Manage on a copy of the test vault ----
mkdirSync(vault);
for (const f of ["keys.json", "test-passphrase.txt"]) copyFileSync(join(VAULT, f), join(vault, f));
writeFileSync(join(vault, "settings.json"), JSON.stringify({ server: PROXY }));
// The CDN holds 0.5.2: the clean-up must keep it (its next deploy re-reads it from a relay).
writeFileSync(join(vault, "cdn-assets.json"), JSON.stringify({ [`/${QS}/TetherQS-0.5.2.exe.gz`]: { hash: "0".repeat(32), size: 1, group: QS, version: "0.5.2" } }));
const builds = join(work, "builds");
mkdirSync(builds);

let browser, manage;
try {
  for (let i = 0; i < 60 && !apiOut.includes("listening"); i++) await sleep(250);
  if (!apiOut.includes("listening")) throw new Error(`tether-api did not start: ${apiOut}`);
  manage = spawn(MANAGE, [], { env: { ...process.env, TETHER_MANAGE_DIR: vault, WEBVIEW2_USER_DATA_FOLDER: join(work, "wv2"), WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9447" }, stdio: "ignore" });
  for (let i = 0; i < 80 && !browser; i++) {
    browser = await chromium.connectOverCDP("http://127.0.0.1:9447").catch(() => null);
    if (!browser) await sleep(500);
  }
  if (!browser) throw new Error("Tether Manage did not expose CDP");
  let page;
  for (let i = 0; i < 40 && !page; i++) {
    page = browser.contexts().flatMap((c) => c.pages())[0];
    if (!page) await sleep(250);
  }
  await page.waitForFunction(() => !!window.__TAURI_INTERNALS__);
  const invoke = (cmd, args = {}) => page.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a), [cmd, args]);
  const invokeErr = (cmd, args) => invoke(cmd, args).then(() => null, (e) => String(e?.message ?? e));
  await invoke("keys_unlock", { passphrase: PASS });

  const publish = async (v) => {
    const file = join(builds, `TetherQS-${v}.exe`);
    writeFileSync(file, randomBytes(64 * 1024));
    return invoke("release_publish", { args: { file, kind: "customer", platform: "windows-x86_64", channel: "stable", version: v, notes: "", mandatory: false, minimum_version: "", forced: false, rollout: "now" } });
  };
  for (const v of ["0.5.1", "0.5.2", "0.5.3", "0.5.4", "0.5.5"]) await publish(v);
  check("five versions published, each with .exe, .sig and .exe.gz", ["0.5.1", "0.5.2", "0.5.3", "0.5.4", "0.5.5"].every(whole));

  // Uploaded just now: the 24 h grace keeps everything.
  let plan = await invoke("release_prune_plan", { keep: 1 });
  check("fresh uploads are all kept (24 h)", plan.offer.versions === 0, JSON.stringify(plan.offer));
  // Two days old, as on a real relay.
  const old = new Date(Date.now() - 2 * 86_400_000);
  const age = (d) => readdirSync(d, { withFileTypes: true }).forEach((e) => (e.isDirectory() ? age(join(d, e.name)) : utimesSync(join(d, e.name), old, old)));
  age(join(apiDir, "download"));

  plan = await invoke("release_prune_plan", { keep: 1 });
  const g = plan.groups.find((x) => x.dir === "customer/windows-x86_64");
  const why = (v) => g.versions.find((x) => x.version === v)?.why ?? [];
  const offered = g.versions.filter((v) => v.offer).map((v) => v.version);
  check("plan offers 0.5.3 and 0.5.1", JSON.stringify(offered) === JSON.stringify(["0.5.3", "0.5.1"]), offered.join(","));
  check("plan keeps the published 0.5.5 (index, latest.json)", why("0.5.5").includes("index") && why("0.5.5").includes("latest.json"), why("0.5.5").join(","));
  check("plan keeps 0.5.4 (previous) and 0.5.2 (CDN pin)", why("0.5.4").includes("previous") && why("0.5.2").includes("pinned"));

  const del = [...offered, "0.5.5"].map((version) => ({ channel: "stable", dir: "customer/windows-x86_64", version }));
  const r = await invoke("release_prune", { keep: 1, delete: del });
  const d = r[0]?.detail ?? {};
  check("execute: one result per relay, ok", r.length === 1 && r[0].ok, r[0]?.error);
  check("execute deletes the two offered versions, keeps the published one", d.deleted?.length === 2 && d.kept?.[0]?.version === "0.5.5", JSON.stringify({ deleted: d.deleted?.map((x) => x.version), kept: d.kept }));
  check("on disk: 0.5.1 and 0.5.3 gone, 0.5.2, 0.5.4, 0.5.5 whole", none("0.5.1") && none("0.5.3") && whole("0.5.2") && whole("0.5.4") && whole("0.5.5"));
  check("on disk: latest.json and releases.json stay", has("latest.json") && existsSync(join(apiDir, "download", "stable", "releases.json")));
  const again = await invoke("release_prune", { keep: 1, delete: del.slice(0, 2) });
  check("execute again: already gone", again[0]?.detail?.gone?.length === 2);

  const err = await invokeErr("admin", { method: "DELETE", path: `/api/admin/release/${QS}/TetherQS-0.5.5.exe` });
  check("single DELETE of the published exe is refused (409)", err?.includes("HTTP 409") && err.includes("published release") && has("TetherQS-0.5.5.exe"), err ?? "deleted");

  // Automatic clean-up after a publish, keep 0.
  // settings_set wants https:// relays; this test's stand-in is http, so write the file.
  writeFileSync(join(vault, "settings.json"), JSON.stringify({ server: PROXY, prune: { auto: true, keep: 0 } }));
  const pub = await publish("0.5.6");
  const line = pub.find((x) => x.server === "Cleanup");
  check("publish with auto clean-up reports a Cleanup line", line?.ok && /2 old versions deleted/.test(line.detail?.published?.[0] ?? ""), JSON.stringify(line));
  check("on disk after auto clean-up: 0.5.4 and 0.5.5 gone, 0.5.2 (CDN) and 0.5.6 whole", none("0.5.4") && none("0.5.5") && whole("0.5.2") && whole("0.5.6"));
  const log = readFileSync(join(apiDir, "release-prune.jsonl"), "utf8").trim().split("\n");
  check("release-prune.jsonl has a record per execute", log.length === 3, `${log.length} lines`);
} catch (e) {
  check("run", false, e.stack ?? String(e));
} finally {
  await browser?.close().catch(() => {});
  manage?.kill();
  api.kill();
  try {
    execFileSync("wsl", ["-d", "Ubuntu-22.04", "--", "pkill", "-f", `tether-api --data ${wslPath(apiDir)}`], { stdio: "ignore" });
  } catch {}
  proxy.close();
  await sleep(500);
  rmSync(work, { recursive: true, force: true });
}
const failed = results.filter((r) => !r.ok).length;
console.log(`\n${results.length - failed}/${results.length} passed`);
process.exit(failed ? 1 : 0);

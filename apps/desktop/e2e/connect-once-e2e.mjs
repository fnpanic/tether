// One connect action ends with exactly one session window for the peer, showing the remote
// screen; none left behind on "Connecting…" (0.5.12: a double-clicked Recent card opened two).
//
//   node e2e/connect-once-e2e.mjs --exe C:\cargo-target\connwin\debug\tether.exe   (debug build: TETHER_DEV_PASSWORD)
//
// Host (profile cw-host, dev password) and viewer (cw-view, main window) run on an invisible
// desktop (hidden-run.ps1) and are driven over DevTools only. Connects: ID + Connect button,
// a double click on the Recent card, Enter pressed twice, and `--connect` forwarded to the
// running viewer. Only processes of this exe (this script's build) are stopped.
import { execFileSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { chromium } from "playwright-core";

const { values: opt } = parseArgs({ options: { exe: { type: "string" } } });
if (!opt.exe) throw new Error("--exe is required");
const DESK = "tether-connwin-test";
const HIDDEN = resolve(import.meta.dirname, "hidden-run.ps1");
const PW = "Cw-Pass-" + randomBytes(4).toString("hex") + "!";
const VIEW = { TETHER_PROFILE: "cw-view", WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9262" };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let failures = 0;
const check = (what, ok) => {
  console.log(`${ok ? "PASS" : "FAIL"} ${what}`);
  if (!ok) failures++;
};

async function until(what, fn, timeout = 30000, every = 250) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    try {
      const v = await fn();
      if (v) return v;
    } catch {}
    await sleep(every);
  }
  throw new Error(`timeout: ${what}`);
}
const cdp = (port) => until(`cdp ${port}`, () => chromium.connectOverCDP(`http://127.0.0.1:${port}`), 120000, 500);
const pages = (b, m) => b.contexts().flatMap((c) => c.pages()).filter((p) => !p.isClosed() && p.url().includes(m));
const ipc = (p, cmd, args) => p.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a), [cmd, args]);
const text = (p) => p.evaluate(() => document.body.innerText);

const hidden = (args, env) =>
  execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", HIDDEN, "-Exe", opt.exe, "-Args", args, "-Desktop", DESK], {
    env: { ...process.env, ...env },
    stdio: "ignore",
  });
const killAll = () =>
  execFileSync("powershell", ["-NoProfile", "-Command", `Get-CimInstance Win32_Process -Filter "Name='tether.exe'" | Where-Object { $_.ExecutablePath -eq '${opt.exe}' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }`], { stdio: "ignore" });

/** Session windows of the viewer by label (Tauri's own list, not only the pages CDP sees). */
async function sessionLabels(main) {
  const all = await ipc(main, "plugin:window|get_all_windows");
  return all.filter((l) => l.startsWith("s-"));
}

/** Logs in where a window asks for the password; true once its canvas has a frame. */
async function connected(p) {
  const t = await text(p);
  if (t.includes("Enter the one-time password")) {
    await p.locator("#pw").fill(PW);
    await p.locator("#pw").press("Enter");
    return false;
  }
  return p.evaluate(() => document.querySelector("canvas")?.width > 0);
}

/** After one connect action: exactly one session window, showing the remote screen. */
async function exactlyOne(viewB, main, what) {
  await until(`${what}: a session window`, async () => (await sessionLabels(main)).length > 0, 30000);
  const s = await until(`${what}: connected`, async () => {
    for (const p of pages(viewB, "/session/")) if (await connected(p)) return p;
  }, 90000).catch(() => null);
  check(`${what}: session shows the remote screen`, !!s);
  await sleep(5000); // a late second window or a stuck one has time to show
  const labels = await sessionLabels(main);
  const stuck = [];
  for (const p of pages(viewB, "/session/")) if (!(await p.evaluate(() => document.querySelector("canvas")?.width > 0))) stuck.push((await text(p)).slice(0, 60));
  check(`${what}: exactly one session window ${JSON.stringify(labels)}`, labels.length === 1);
  check(`${what}: no window stuck connecting ${JSON.stringify(stuck)}`, stuck.length === 0);
  await closeSessions(viewB, main);
}

async function closeSessions(viewB, main) {
  for (const l of await sessionLabels(main)) await ipc(main, "plugin:window|close", { label: l }).catch(() => {});
  await until("session windows closed", async () => (await sessionLabels(main)).length === 0, 15000);
  await sleep(2000); // the host lets the old connection go
}

async function main() {
  // Every profile on one PC derives the same machine ID: give the test host its own (see reconnect-bench.mjs).
  const dir = join(process.env.APPDATA, "Tether-cw-host", "config");
  if (!existsSync(join(dir, "Tether-cw-host.toml"))) {
    mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, "Tether-cw-host.toml"), `id = '${700000000 + Math.floor(Math.random() * 99999999)}'\n`);
  }
  hidden("", { TETHER_PROFILE: "cw-host", TETHER_DEV_PASSWORD: PW, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9261" });
  const hostB = await cdp(9261);
  const hostMain = await until("host main", () => pages(hostB, "index.html")[0], 60000);
  const st = await until("host online", async () => {
    const s = await ipc(hostMain, "status");
    return s.status === 1 && s.key_confirmed && s.id ? s : null;
  }, 90000);
  await sleep(4000); // dev password is set 3 s after start
  console.log(`host ${st.id}`);

  // The Recent card for the host (written like the core's PeerConfig, before the viewer starts).
  const peers = join(process.env.APPDATA, "Tether-cw-view", "config", "peers");
  mkdirSync(peers, { recursive: true });
  writeFileSync(join(peers, `${st.id}.toml`), "[info]\nusername = 'test'\nhostname = 'cw-host'\nplatform = 'Windows'\n");
  hidden("", VIEW);
  const viewB = await cdp(9262);
  const main = await until("viewer main", () => pages(viewB, "index.html").find((p) => !p.url().includes("/session/")), 60000);
  await until("viewer online", async () => (await ipc(main, "status")).status === 1, 90000);
  await closeSessions(viewB, main);

  // 1. Main window: type the ID, click Connect.
  await main.getByLabel("Remote device ID").fill(st.id);
  await main.getByLabel("Remote device ID").blur(); // the Recent suggestion list covers the button
  await sleep(300);
  await main.getByRole("button", { name: "Connect", exact: true }).click();
  await exactlyOne(viewB, main, "Connect button");

  // 2. Enter pressed twice in the ID field.
  await main.getByLabel("Remote device ID").fill(st.id);
  await main.getByLabel("Remote device ID").press("Enter");
  await main.getByLabel("Remote device ID").press("Enter");
  await exactlyOne(viewB, main, "Enter twice");

  // 3. Recent card double-clicked (a desktop habit).
  await main.evaluate(() => window.dispatchEvent(new Event("focus"))); // reloads the Recent list
  const card = main.getByRole("button", { name: /^Connect to / }).first();
  await card.waitFor({ timeout: 15000 });
  await card.dblclick();
  await exactlyOne(viewB, main, "Recent card double click");

  // 4. `--connect` forwarded to the running viewer, twice (a link opened twice).
  hidden(`--connect ${st.id} --password-env TETHER_E2E_PW`, { ...VIEW, TETHER_E2E_PW: PW });
  hidden(`--connect ${st.id} --password-env TETHER_E2E_PW`, { ...VIEW, TETHER_E2E_PW: PW });
  await exactlyOne(viewB, main, "--connect forwarded twice");
}

main()
  .catch((e) => {
    console.error(e);
    failures++;
  })
  .finally(() => {
    killAll();
    console.log(failures ? `${failures} FAILED` : "ALL PASSED");
    process.exit(failures ? 1 : 0);
  });

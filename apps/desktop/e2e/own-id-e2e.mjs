// End-to-end: `tether --connect <this device's own ID>` says why it is refused, in the main window.
// Everything on an invisible desktop (hidden-run.ps1), own profile, devtools on CDP_PORT.
//   TECH_EXE=C:\cargo-target\watchdog\debug\tether.exe node e2e/own-id-e2e.mjs
// 1. Tether running: the refused process starts a Tether that hands the notice to the running one.
// 2. No Tether running: the Tether it starts is the main window and shows the notice itself.
import { execFileSync, spawn } from "node:child_process";
import { resolve } from "node:path";
import { chromium } from "playwright-core";

const TECH = process.env.TECH_EXE || "C:\\cargo-target\\watchdog\\debug\\tether.exe";
const HIDDEN = resolve(import.meta.dirname, "hidden-run.ps1");
const CDP_PORT = 9251;
const MSG = "That is this device's own ID: it cannot connect to itself.";
const env = { ...process.env, TETHER_PROFILE: "watchdog-ownid", WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${CDP_PORT}` };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const pages = (b) => b.contexts().flatMap((c) => c.pages());
async function until(what, fn, timeout = 30000, every = 100) {
  const end = Date.now() + timeout;
  for (;;) {
    try {
      const v = await fn();
      if (v) return v;
    } catch {}
    if (Date.now() > end) throw new Error(`timeout: ${what}`);
    await sleep(every);
  }
}
const run = (args) => execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", HIDDEN, "-Exe", TECH, ...(args ? ["-Args", args] : [])], { env, stdio: "ignore", timeout: 30000 });
/** Only the processes of this exe (this test's build). */
function kill() {
  const ps = `Get-CimInstance Win32_Process -Filter "Name='tether.exe'" | Where-Object { $_.ExecutablePath -eq '${TECH}' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }`;
  execFileSync("powershell", ["-NoProfile", "-Command", ps], { stdio: "ignore" });
}
const count = () =>
  Number(execFileSync("powershell", ["-NoProfile", "-Command", `@(Get-CimInstance Win32_Process -Filter "Name='tether.exe'" | Where-Object { $_.ExecutablePath -eq '${TECH}' }).Count`], { encoding: "utf8" }).trim());

async function mainPage() {
  const b = await until("devtools", () => chromium.connectOverCDP(`http://127.0.0.1:${CDP_PORT}`, { timeout: 5000 }), 90000, 200);
  const p = await until("main window", () => pages(b).find((p) => p.url().endsWith("#/") || p.url().endsWith("index.html")), 30000);
  return { b, p };
}
/** The toast shows for ~3.5 s: poll its text. */
const toast = (p, timeout = 20000) => until("toast", async () => (await p.locator('[role="status"]').allInnerTexts()).find((t) => t.includes("own ID")), timeout, 100);

const out = {};
const log = (...a) => console.log(new Date().toISOString().slice(11, 19), ...a);
kill();
log("killed");
try {
  // Main instance, then its ID.
  run("");
  log("started");
  let { b, p } = await mainPage();
  log("main window");
  const id = await until("own ID", async () => (await p.evaluate(() => window.__TAURI_INTERNALS__.invoke("status"))).id, 60000, 500);
  console.log("own ID", id);

  // 1. With a running instance.
  const before = count();
  const t0 = Date.now();
  run(`--connect ${id}`);
  out.running = { text: await toast(p), ms: Date.now() - t0 };
  await sleep(3000);
  out.running.processesBefore = before;
  out.running.processesAfter = count(); // the refused one and the one it started have exited
  out.running.sessionWindows = pages(b).filter((x) => x.url().includes("/session/")).length;
  console.log("1. running instance:", out.running);
  await b.close();

  // 2. Nothing running.
  kill();
  // The killed Tether's webview processes must be gone, or the new one cannot open its profile.
  const webviews = () =>
    Number(execFileSync("powershell", ["-NoProfile", "-Command", `@(Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" | Where-Object { $_.CommandLine -match 'profile-watchdog-ownid' }).Count`], { encoding: "utf8" }).trim());
  await until("old webview gone", () => webviews() === 0, 30000, 500);
  // The invisible desktop lives as long as a handle to it: the refused process exits right after
  // starting Tether, so hold it while the new Tether attaches (the user's desktop always exists).
  const holder = spawn("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", `& '${HIDDEN}' -Exe '${TECH}' -Args '--connect ${id}' | Out-Null; Start-Sleep 30`], { env, stdio: "ignore" });
  const t1 = Date.now();
  ({ b, p } = await mainPage());
  out.none = { text: await toast(p), ms: Date.now() - t1 };
  out.none.sessionWindows = pages(b).filter((x) => x.url().includes("/session/")).length;
  holder.kill();
  console.log("2. no instance:", out.none);
  await b.close();
} finally {
  kill();
}
const ok = out.running?.text === MSG && out.none?.text === MSG && out.running.processesAfter <= out.running.processesBefore && !out.running.sessionWindows && !out.none.sessionWindows;
console.log(ok ? "PASS" : "FAIL");
process.exit(ok ? 0 : 1);

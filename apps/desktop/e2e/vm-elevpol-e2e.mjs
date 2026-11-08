// End-to-end: "Request admin access" honours TetherQS's customer.elevate policy.
// Technician app on this PC on an invisible desktop -> TetherQS in the Hyper-V VM.
//   node e2e/vm-elevpol-e2e.mjs
// 1. TetherQS with allow-elevate = N (what policy customer.elevate off sets in the core): the request
//    is refused with the policy reason, no UAC prompt (consent.exe) opens in the VM, session goes on.
// 2. allow-elevate removed (policy on): the same request opens the UAC prompt; answered No (Esc).
// Needs: the broker, C:\vms\vm-elevpol.ps1, vm-uac-qs.ps1, vm-uac-key.ps1; the TetherQS build installed
// with vm-elevpol.ps1 -Action install. Env: TECH_EXE, VM_IP (172.30.252.30), QS_CDP (9334).
import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { chromium } from "playwright-core";

const ROOT = resolve(import.meta.dirname, "../../..");
const TECH = process.env.TECH_EXE || "C:\\cargo-target\\tether-rel\\release\\tether.exe";
const VM_IP = process.env.VM_IP || "172.30.252.30";
const QS_CDP = `http://${VM_IP}:${process.env.QS_CDP || 9334}`;
const HIDDEN = resolve(import.meta.dirname, "hidden-run.ps1");
const CDP_PORT = 9245;
const OUT = join(ROOT, "tools/bench/results/vm-elevpol-e2e.json");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const pages = (b) => b.contexts().flatMap((c) => c.pages());
const broker = (script, ...args) =>
  execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "C:\\vms\\broker\\run-elevated.ps1", script, ...args], { encoding: "utf8" });
const consent = () => /proc \d+ consent\.exe/.test(broker("vm-uac-qs.ps1", "-Action", "state"));
async function until(what, fn, timeout = 30000, every = 250) {
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
function killTech() {
  const ps = `Get-CimInstance Win32_Process -Filter "Name='tether.exe'" | Where-Object { $_.ExecutablePath -eq '${TECH}' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }`;
  execFileSync("powershell", ["-NoProfile", "-Command", ps], { stdio: "ignore" });
}

async function session() {
  const qsB = await until("TetherQS devtools in the VM", () => chromium.connectOverCDP(QS_CDP), 60000, 1000);
  const qs = await until("TetherQS page", () => pages(qsB).find((p) => !p.url().startsWith("devtools")), 20000);
  await until("TetherQS ready", async () => (await qs.locator('[data-testid="qs-status"]').innerText()).includes("Ready"), 90000, 1000);
  const id = (await qs.locator('[data-testid="qs-id"]').innerText()).replace(/\s/g, "");
  const pw = ((await qs.locator('[data-testid="qs-password"]').getAttribute("data-password")) ?? "").trim();
  killTech();
  const env = { ...process.env, TETHER_PROFILE: "elevpol-tech", WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${CDP_PORT}`, TETHER_E2E_PW: pw };
  execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", HIDDEN, "-Exe", TECH, "-Args", `--connect ${id} --password-env TETHER_E2E_PW`], { env, stdio: "ignore" });
  const vB = await until("technician devtools", () => chromium.connectOverCDP(`http://127.0.0.1:${CDP_PORT}`), 120000, 500);
  const v = await until("session window", () => pages(vB).find((p) => p.url().includes("/session/")), 40000);
  await until("session active", async () => {
    const accept = qs.getByRole("button", { name: "Accept" });
    if (await accept.isVisible().catch(() => false)) await accept.click();
    return (await qs.locator('[data-testid="qs-status"]').innerText()).includes("Session active");
  }, 40000, 300);
  await until("first frame", () => v.evaluate(() => { const c = document.querySelector("canvas"); return !!c && !(c.width === 300 && c.height === 150); }), 40000);
  const request = async () => {
    await v.evaluate(() => document.querySelector('[aria-label="Show toolbar"]')?.click());
    await v.getByRole("button", { name: "Remote actions" }).click();
    await v.getByRole("button", { name: /Request admin access/ }).click();
  };
  const body = () => v.evaluate(() => document.body.innerText);
  const alive = () => v.evaluate(() => getComputedStyle(document.querySelector("canvas")).opacity === "1" && !!document.querySelector('[data-testid="toolbar"]'));
  return { qsB, vB, v, request, body, alive };
}

async function main() {
  const results = [];
  // 1. Policy off.
  console.log(broker("vm-elevpol.ps1", "-Action", "setopt", "-Value", "N").split(/\r?\n/).filter((l) => l.startsWith("config")).join(" "));
  broker("vm-uac-qs.ps1", "-Action", "restart");
  let s = await session();
  await s.request();
  const toast = await until("policy toast", async () => (await s.body()).match(/Admin access could not be requested[^\n]*/)?.[0], 15000);
  await sleep(6000);
  const prompt = consent();
  results.push({ step: "policy off", toast, uac_prompt_opened: prompt, session_alive: await s.alive() });
  if (!/turned off by policy/.test(toast) || prompt) throw new Error(`policy off: ${JSON.stringify(results.at(-1))}`);
  killTech();
  // 2. Policy on (option removed): the prompt opens, answered No.
  console.log(broker("vm-elevpol.ps1", "-Action", "clearopt").split(/\r?\n/).filter((l) => l.startsWith("config")).join(" "));
  broker("vm-uac-qs.ps1", "-Action", "restart");
  s = await session();
  await s.request();
  const opened = await until("consent.exe", consent, 40000, 1000);
  broker("vm-uac-key.ps1", "-Action", "no");
  const declined = await until("declined toast", async () => (await s.body()).match(/Admin access was declined[^\n]*/)?.[0], 30000);
  results.push({ step: "policy on", uac_prompt_opened: opened, toast: declined, session_alive: await s.alive() });
  killTech();
  writeFileSync(OUT, JSON.stringify({ at: new Date().toISOString(), tech: TECH, results }, null, 1));
  console.log(JSON.stringify(results, null, 1));
}
main().then(() => process.exit(0), (e) => { console.error("FAIL", e.message); killTech(); process.exit(1); });

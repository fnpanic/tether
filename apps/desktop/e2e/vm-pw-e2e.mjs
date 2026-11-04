// End-to-end, password path: the technician (inside the VM, nothing opens on this PC) connects to
// TetherQS with its one-time password; the session must start without anyone clicking Accept.
//   node e2e/vm-pw-e2e.mjs     (copy the build first: run-elevated.ps1 vm-tech-run.ps1 -Exe …)
import { execFileSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { chromium } from "playwright-core";

const ROOT = resolve(import.meta.dirname, "../../..");
const VM_IP = process.env.VM_IP || "172.30.252.30";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const pages = (b) => b.contexts().flatMap((c) => c.pages());
const text = (p) => p.evaluate(() => document.body.innerText);
const broker = (...a) => execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "C:\\vms\\broker\\run-elevated.ps1", "vm-tech-run.ps1", ...a]).toString();
async function until(what, fn, timeout = 30000, every = 300) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    try {
      const v = await fn();
      if (v) return v;
    } catch {}
    await sleep(every);
  }
  throw new Error(`timeout waiting for ${what}`);
}

const results = [];
const record = (name, ok, d = {}) => (results.push({ name, ok, ...d }), console.log(`${ok ? "PASS" : "FAIL"}  ${name}`, JSON.stringify(d)));
try {
  const qsB = await until("TetherQS devtools", () => chromium.connectOverCDP(`http://${VM_IP}:9334`), 60000, 1000);
  const qs = pages(qsB)[0];
  await until("TetherQS ready", async () => (await qs.locator('[data-testid="qs-status"]').innerText()).includes("Ready"), 90000, 1000);
  const id = (await qs.locator('[data-testid="qs-id"]').innerText()).replace(/\s/g, "");
  const pw = (await qs.locator('[data-testid="qs-password"]').getAttribute("data-password")) ?? "";
  if (!pw) throw new Error("no one-time password shown (approve mode click?)");
  broker("-Connect", id, "-Password", pw);
  const vB = await until("technician devtools", () => chromium.connectOverCDP(`http://${VM_IP}:9235`), 60000, 500);
  const v = await until("session window", () => pages(vB).find((p) => p.url().includes("/session/")), 40000);
  const t0 = Date.now();
  let askedAccept = false;
  await until("first frame", async () => {
    if ((await text(qs)).includes("wants to see and control")) askedAccept = true;
    return v.evaluate(() => { const c = document.querySelector("canvas"); return !!c && c.width !== 300; });
  }, 40000, 200);
  const active = await until("Session active", async () => (await qs.locator('[data-testid="qs-status"]').innerText()).includes("Session active"), 10000).catch(() => false);
  record("password login: session starts without Accept, TetherQS shows it", !askedAccept && !!active, { ms: Date.now() - t0, askedAccept, active: !!active });
  // The core hands out a new one-time password after a login.
  await qs.getByRole("button", { name: "Disconnect" }).click();
  await until("idle", async () => (await qs.locator('[data-testid="qs-status"]').innerText()).includes("Ready"), 15000);
  const pw2 = (await qs.locator('[data-testid="qs-password"]').getAttribute("data-password")) ?? "";
  record("after the session the one-time password is new", pw2 !== "" && pw2 !== pw);
} catch (e) {
  record("run", false, { error: e.message });
} finally {
  try {
    broker("-Stop");
  } catch {}
  writeFileSync(join(ROOT, "tools/bench/results/vm-pw-e2e.json"), JSON.stringify({ when: new Date().toISOString(), results }, null, 2));
  console.log(`${results.filter((r) => r.ok).length}/${results.length} passed`);
  process.exit(results.every((r) => r.ok) ? 0 : 1);
}

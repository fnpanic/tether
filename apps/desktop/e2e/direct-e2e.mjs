// Direct paths: which path a session takes and how long it needs, technician app on this PC
// started on an invisible desktop (no window or focus on the user's screen).
//
//   node e2e/direct-e2e.mjs vm  [runs]          TetherQS in the Hyper-V VM (ID/password over CDP)
//   node e2e/direct-e2e.mjs <id> [runs]         any host; password from env HOST_PW or file HOST_PW_FILE
// Env: EXTRA_ARGS (e.g. --view-only), TECH_EXE (C:\cargo-target\direct\release\tether.exe), VM_IP (172.30.252.30), QS_CDP (9334),
//      EXPECT ("Direct (LAN)" / "Relay": fail when the badge differs).
// Prints one line per run: path badge, ms from start to the badge (connection secured) and to
// the first frame. Kills only processes started from TECH_EXE.
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { chromium } from "playwright-core";

const TECH = process.env.TECH_EXE || "C:\\cargo-target\\direct\\release\\tether.exe";
const VM_IP = process.env.VM_IP || "172.30.252.30";
const QS_CDP = `http://${VM_IP}:${process.env.QS_CDP || 9334}`;
const HIDDEN = process.env.HIDDEN_RUN || resolve(import.meta.dirname, "hidden-run.ps1");
const CDP_PORT = 9241;
const target = process.argv[2] || "vm";
const runs = Number(process.argv[3] || 3);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const pages = (b) => b.contexts().flatMap((c) => c.pages());

async function until(what, fn, timeout = 30000, every = 100) {
  const end = Date.now() + timeout;
  let last;
  while (Date.now() < end) {
    try {
      const v = await fn();
      if (v) return v;
    } catch (e) {
      last = e;
    }
    await sleep(every);
  }
  throw new Error(`timeout waiting for ${what}${last ? `: ${last.message}` : ""}`);
}

function killTech() {
  const ps = `Get-CimInstance Win32_Process -Filter "Name='tether.exe'" | Where-Object { $_.ExecutablePath -eq '${TECH}' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }`;
  execFileSync("powershell", ["-NoProfile", "-Command", ps], { stdio: "ignore" });
}

async function main() {
  let id = target, pw = process.env.HOST_PW || "", qs = null;
  if (!pw && process.env.HOST_PW_FILE) pw = readFileSync(process.env.HOST_PW_FILE, "utf8").trim();
  if (target === "vm") {
    const qsB = await until("TetherQS devtools in the VM", () => chromium.connectOverCDP(QS_CDP), 60000, 1000);
    qs = await until("TetherQS page", () => pages(qsB).find((p) => !p.url().startsWith("devtools")), 20000);
    await until("TetherQS ready", async () => (await qs.locator('[data-testid="qs-status"]').innerText()).includes("Ready"), 90000, 1000);
    id = (await qs.locator('[data-testid="qs-id"]').innerText()).replace(/\s/g, "");
    pw = ((await qs.locator('[data-testid="qs-password"]').getAttribute("data-password")) ?? "").trim();
  }
  console.log(`host ${id}, password ${pw ? pw.length + " chars" : "MISSING"}`);
  const out = [];
  for (let i = 0; i < runs; i++) {
    killTech();
    await sleep(1500);
    // The one-time password changes after each session: read it again.
    if (qs) pw = ((await qs.locator('[data-testid="qs-password"]').getAttribute("data-password")) ?? "").trim();
    if (process.env.WRONG_PW) pw += "x"; // negative test: the direct path must refuse it
    const env = { ...process.env, TETHER_PROFILE: "direct-tech", WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${CDP_PORT}`, TETHER_E2E_PW: pw };
    const t0 = Date.now();
    execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", HIDDEN, "-Exe", TECH, "-Args", `--connect ${id} --password-env TETHER_E2E_PW ${process.env.EXTRA_ARGS || ""}`], { env, stdio: "ignore" });
    const vB = await until("technician devtools", () => chromium.connectOverCDP(`http://127.0.0.1:${CDP_PORT}`), 40000, 200);
    const v = await until("session window", () => pages(vB).find((p) => p.url().includes("/session/")), 40000);
    const route = await until("path badge", async () => {
      if (qs && !process.env.NO_ACCEPT) {
        const accept = qs.getByRole("button", { name: "Accept" });
        if (await accept.isVisible().catch(() => false)) await accept.click();
      }
      const t = await v.evaluate(() => document.body.textContent);
      return ["Direct (LAN)", "Direct (IPv6)", "P2P", "Relay"].find((l) => t.includes(l));
    }, 40000, 50);
    const tRoute = Date.now() - t0;
    await until("first frame", () => v.evaluate(() => { const c = document.querySelector("canvas"); return !!c && !(c.width === 300 && c.height === 150); }), 40000, 100);
    const tFrame = Date.now() - t0;
    out.push({ route, tRoute, tFrame });
    console.log(`run ${i + 1}: ${route}  secured+badge ${tRoute} ms  first frame ${tFrame} ms`);
    await vB.close().catch(() => {});
    killTech();
    await sleep(2000);
  }
  const want = process.env.EXPECT;
  const bad = want && out.some((r) => r.route !== want);
  console.log(JSON.stringify({ target: target === "vm" ? `vm:${id}` : id, runs: out }));
  if (bad) {
    console.log(`FAIL: expected ${want}`);
    process.exitCode = 1;
  }
  process.exit();
}

main().catch((e) => {
  console.error(e);
  try { killTech(); } catch {}
  process.exit(1);
});

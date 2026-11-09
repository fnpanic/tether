// End-to-end: the technician app and TetherQS on the same PC have different IDs, and every connection
// lands on the app whose ID it dials (production relays). Both apps run in the Hyper-V VM as the customer;
// the viewer runs on this PC on an invisible desktop.
//   node e2e/vm-qsid-e2e.mjs
// 1. Old TetherQS (OLD_QS, <= 0.5.28) next to the technician app: both show the same ID (the bug).
// 2. New TetherQS (NEW_QS, 0.5.29) replaces it: moves once to its own ID (log line), the technician app's
//    ID stays; time until the technician app is reachable again with its own key (hbbs re-register request).
// 3. TetherQS restarted twice: the new ID is kept.
// 4. ROUNDS x each, alternating: connect to the QS ID with its one-time password (QS shows "Session active")
//    and to the technician ID with its permanent password (first frame, QS stays "Ready").
// Needs: the broker, C:\vms\vm-qsid.ps1, vm-svc-setup.ps1; the technician app installed and running in the VM with
// devtools (vm-svc-setup.ps1, TECH_CDP 9235); its permanent password in C:\vms\payload\qsid-pw.json (set here).
// Env: VIEWER_EXE, OLD_QS, NEW_QS, VM_IP, ROUNDS.
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { chromium } from "playwright-core";

const ROOT = resolve(import.meta.dirname, "../../..");
const VIEWER = process.env.VIEWER_EXE || "C:\\cargo-target\\tether-rel\\release\\tether.exe";
const OLD_QS = process.env.OLD_QS || "C:\\cargo-target\\tether-rel\\release\\TetherQS.exe";
const NEW_QS = process.env.NEW_QS || "C:\\cargo-target\\qsid\\release\\TetherQS.exe";
const VM_IP = process.env.VM_IP || "172.30.252.30";
const ROUNDS = Number(process.env.ROUNDS || 5);
const TECH_PW = JSON.parse(readFileSync("C:/vms/payload/qsid-pw.json", "utf8")).password;
const HIDDEN = resolve(import.meta.dirname, "hidden-run.ps1");
const CDP_PORT = 9246;
const OUT = join(ROOT, "tools/bench/results/vm-qsid-e2e.json");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const pages = (b) => b.contexts().flatMap((c) => c.pages());
const broker = (script, ...args) =>
  execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "C:\\vms\\broker\\run-elevated.ps1", script, ...args], { encoding: "utf8" });
const vm = (...a) => broker("vm-qsid.ps1", ...a);
async function until(what, fn, timeout = 30000, every = 300) {
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
function killViewer() {
  const ps = `Get-CimInstance Win32_Process -Filter "Name='tether.exe'" | Where-Object { $_.ExecutablePath -eq '${VIEWER}' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }`;
  execFileSync("powershell", ["-NoProfile", "-Command", ps], { stdio: "ignore" });
}

// The two MAC-derived IDs (hbb_common Config::auto_id_from_mac): plain for the technician app, salted for TetherQS.
function autoId(mac, salt) {
  let id = ((mac[2] << 24) | (mac[3] << 16) | (mac[4] << 8) | mac[5]) & 0x1fffffff;
  if (!salt) return id;
  let h = 0xcbf29ce484222325n;
  const bytes = [...Buffer.from(salt, "latin1"), ...mac];
  for (const b of bytes) h = ((h ^ BigInt(b)) * 0x100000001b3n) & 0xffffffffffffffffn;
  const s = 100_000_000 + Number(h % 900_000_000n);
  return s !== id ? s : s === 999_999_999 ? 100_000_000 : s + 1;
}

async function qsPage() {
  const b = await until("TetherQS devtools in the VM", () => chromium.connectOverCDP(`http://${VM_IP}:9334`), 90000, 1000);
  const p = await until("TetherQS page", () => pages(b).find((x) => !x.url().startsWith("devtools")), 20000);
  await until("TetherQS ready", async () => (await p.locator('[data-testid="qs-status"]').innerText()).includes("Ready"), 120000, 1000);
  const id = (await p.locator('[data-testid="qs-id"]').innerText()).replace(/\s/g, "");
  return { b, p, id, status: () => p.locator('[data-testid="qs-status"]').innerText(), pw: async () => ((await p.locator('[data-testid="qs-password"]').getAttribute("data-password")) ?? "").trim() };
}
async function techPage() {
  const b = await until("technician devtools in the VM", () => chromium.connectOverCDP(`http://${VM_IP}:9235`), 60000, 1000);
  const p = await until("technician page", () => pages(b).find((x) => !x.url().startsWith("devtools") && !x.url().includes("#/cm")), 20000);
  const st = await until("technician status", () => p.evaluate(() => window.__TAURI_INTERNALS__.invoke("status")), 60000, 1000);
  return { b, p, id: String(st.id) };
}
async function setTechPassword(p) {
  if (!(await p.getByText("Permanent password", { exact: true }).isVisible().catch(() => false))) {
    await p.getByRole("button", { name: "Settings" }).first().click();
    await sleep(800);
  }
  await p.getByText("Unattended access", { exact: true }).first().click();
  await sleep(800);
  await p.locator('input[aria-label="New password"]:visible').first().fill(TECH_PW);
  await p.locator('input[aria-label="Repeat password"]:visible').first().fill(TECH_PW);
  await p.getByRole("button", { name: "Set password" }).click();
  await sleep(1500);
}

// One viewer session from this PC: first frame within `wait` ms, or the reason it failed.
async function connect(id, pw, wait = 45000) {
  killViewer();
  const env = { ...process.env, TETHER_PROFILE: "qsid-viewer", WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${CDP_PORT}`, TETHER_E2E_PW: pw };
  const t0 = Date.now();
  execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", HIDDEN, "-Exe", VIEWER, "-Args", `--connect ${id} --password-env TETHER_E2E_PW`], { env, stdio: "ignore" });
  const vB = await until("viewer devtools", () => chromium.connectOverCDP(`http://127.0.0.1:${CDP_PORT}`), 60000, 500);
  try {
    const v = await until("session window", () => pages(vB).find((p) => p.url().includes("/session/")), 30000);
    const ok = await until("first frame or error", () => v.evaluate(() => {
      const c = document.querySelector("canvas");
      if (c && !(c.width === 300 && c.height === 150)) return "frame";
      const t = document.body.innerText;
      return /wrong password|password.*(incorrect|invalid)|key mismatch|offline|not exist|failed/i.test(t) ? t.replace(/\s+/g, " ").slice(0, 160) : null;
    }), wait, 300).catch(async () => `timeout: ${(await v.evaluate(() => document.body.innerText).catch(() => "")).replace(/\s+/g, " ").slice(0, 160)}`);
    return { ok: ok === "frame", ms: Date.now() - t0, detail: ok === "frame" ? undefined : ok, v };
  } catch (e) {
    return { ok: false, ms: Date.now() - t0, detail: e.message };
  } finally {
    vB.close().catch(() => {});
  }
}

const r = { at: new Date().toISOString(), viewer: VIEWER, old_qs: OLD_QS, new_qs: NEW_QS, ok: false, steps: [] };
async function main() {
  const step = (name, ok, d = {}) => {
    r.steps.push({ name, ok, ...d });
    console.log(`${ok ? "PASS" : "FAIL"}  ${name}`, JSON.stringify(d));
    if (!ok) throw new Error(name);
  };
  const state0 = vm("-Action", "state");
  const mac = state0.match(/^mac: ([0-9A-F-]{17})/m)[1].split("-").map((x) => parseInt(x, 16));
  const plain = String(autoId(mac, 0)), salted = String(autoId(mac, "TetherQS"));
  r.vm_mac = mac.map((x) => x.toString(16).padStart(2, "0")).join(":");
  r.expected = { technician: plain, tetherqs: salted };

  // Technician app (host): permanent password, its ID before anything else runs.
  const tech = await techPage();
  await setTechPassword(tech.p);
  const techId0 = tech.id;
  step("technician app: ID is the plain MAC-derived ID", techId0 === plain, { id: techId0, expected: plain });

  // 1. Old TetherQS stored the shared ID and registered it after the technician app (takes it over).
  vm("-Action", "install", "-Exe", OLD_QS);
  vm("-Action", "setqsid", "-Value", plain);
  vm("-Action", "startqs");
  let qs = await qsPage();
  step("old TetherQS shows the technician app's ID (the bug)", qs.id === techId0, { qs_id: qs.id, tech_id: techId0 });
  const qsOldPw = await qs.pw();
  const bug = await connect(techId0, TECH_PW, 25000);
  killViewer();
  r.bug_demo = { dial: techId0, with: "technician permanent password", ok: bug.ok, detail: bug.detail, qs_status: await qs.status().catch(() => "") };
  console.log("bug demo (informational):", JSON.stringify(r.bug_demo), qsOldPw ? "" : "(no QS password)");
  qs.b.close().catch(() => {});

  // 2. New TetherQS: moves once, technician app reachable again with its own key.
  vm("-Action", "install", "-Exe", NEW_QS);
  const tMove = Date.now();
  vm("-Action", "startqs");
  qs = await qsPage();
  const moved = vm("-Action", "log", "-Value", "is the technician app's on this machine", "-Last", "5");
  step("new TetherQS moved to its own ID", qs.id === salted && qs.id !== techId0 && /moving to \d+/.test(moved), {
    qs_id: qs.id, expected: salted, log: moved.split(/\r?\n/).filter((l) => /moved to/.test(l)).map((l) => l.slice(0, 160)),
  });
  let back;
  for (let i = 0; i < 8 && !(back = await connect(techId0, TECH_PW, 25000)).ok; i++) await sleep(5000);
  killViewer();
  step("technician app reachable with its own password after the move", back.ok, { seconds_after_qs_start: Math.round((Date.now() - tMove) / 1000), detail: back.detail });
  qs.b.close().catch(() => {});

  // 3. Kept across restarts.
  const restarts = [];
  for (let i = 0; i < 2; i++) {
    vm("-Action", "startqs");
    const q = await qsPage();
    restarts.push(q.id);
    q.b.close().catch(() => {});
  }
  step("TetherQS keeps its new ID across restarts", restarts.every((x) => x === salted), { ids: restarts });

  // 4. Each ID with its own password, alternating.
  qs = await qsPage();
  const rounds = [];
  for (let i = 0; i < ROUNDS; i++) {
    const pw = await qs.pw();
    const a = await connect(qs.id, pw);
    const qsActive = a.ok && (await until("QS session active", async () => (await qs.status()).includes("Session active"), 15000, 300).catch(() => false));
    killViewer();
    await until("QS back to Ready", async () => (await qs.status()).includes("Ready"), 30000, 500).catch(() => {});
    rounds.push({ round: i + 1, dial: "tetherqs", id: qs.id, ok: !!(a.ok && qsActive), ms: a.ms, qs_session_active: !!qsActive, detail: a.detail });
    const b = await connect(techId0, TECH_PW);
    let qsDuring = "";
    if (b.ok) qsDuring = await qs.status();
    killViewer();
    rounds.push({ round: i + 1, dial: "technician", id: techId0, ok: b.ok && !qsDuring.includes("Session active"), ms: b.ms, qs_status_during: qsDuring.replace(/\s+/g, " ").slice(0, 40), detail: b.detail });
    console.log(JSON.stringify(rounds.slice(-2)));
    await sleep(2000);
  }
  r.rounds = rounds;
  step(`${ROUNDS}x each: every connection lands on the app it dials`, rounds.every((x) => x.ok), { ok: rounds.filter((x) => x.ok).length, of: rounds.length });

  // Host-side evidence: the technician app's log has the technician sessions, TetherQS's log the QS ones.
  r.host_logs = vm("-Action", "log", "-Value", "Connection opened|authorized|Login|logon|new connection", "-Last", String(ROUNDS * 2 + 4)).split(/\r?\n/).filter((l) => l.trim()).slice(0, 60);
  const techEnd = await techPage();
  step("technician app's ID unchanged", techEnd.id === techId0, { before: techId0, after: techEnd.id });
  techEnd.b.close().catch(() => {});
  r.state_end = vm("-Action", "state").split(/\r?\n/).filter((l) => /^(id |qs exe|tech exe)/.test(l));
  r.ok = true;
}
main().then(
  () => (writeFileSync(OUT, JSON.stringify(r, null, 1)), process.exit(0)),
  (e) => { r.error = e.message; writeFileSync(OUT, JSON.stringify(r, null, 1)); console.error("FAIL", e.message); killViewer(); process.exit(1); },
);

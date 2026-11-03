// Fair Tether vs AnyDesk comparison against a Hyper-V benchmark VM (see C:\vms, docs/benchmarks.md).
//
// The VM is the HOST for both products (tools/bench/probe.ps1 draws the VM's clock as a barcode).
// This PC is the VIEWER. The VM clock offset is measured over UDP (clocksrv.ps1 / clock_offset.py),
// so latency = capture time - (barcode shown in the viewer + offset). Only the pinned viewer window
// needs to be visible. Both products share the same host, network path and meter.
//
//   node e2e/bench-vm.mjs [--seconds 20] [--only anydesk|tether]

import { spawn, execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";

const { values: opt } = parseArgs({ options: { seconds: { type: "string", default: "20" }, only: { type: "string" } } });
const ROOT = resolve(import.meta.dirname, "../../..");
const PY = join(ROOT, "tools/bench/.venv/Scripts/python.exe");
const OUT = join(ROOT, "tools/bench/results/vm-comparison.json");
const TETHER = join(ROOT, "apps/desktop/src-tauri/target/profiling/tether.exe");
const ANYDESK = String.raw`C:\Program Files (x86)\AnyDesk\AnyDesk.exe`;
const AD_CMD = String.raw`C:\vms\ad-connect.cmd`;
const raw = JSON.parse(readFileSync("C:/vms/ids.json", "utf8").replace(/^\uFEFF/, ""));
// PowerShell serialises Get-Content lines as objects; the text is in .value.
const ids = { ip: raw.ip, anydesk: String(raw.anydesk?.value ?? raw.anydesk).trim() };
const { password } = JSON.parse(readFileSync("C:/vms/payload/bench.json", "utf8"));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const ps = (file, ...args) => execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", join(ROOT, "tools/bench", file), ...args]).toString().trim();
const meter = (...args) => JSON.parse(execFileSync(PY, [join(ROOT, "tools/bench/glass2glass.py"), ...args], { cwd: join(ROOT, "tools/bench") }).toString());

async function until(what, fn, timeout, every = 1000) {
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

/** The VM's Tether ID, from the relay's peer registry (hbbs sqlite) by the VM's IP. */
function tetherIdOfVm() {
  const code = [
    "import sqlite3, os, sys",
    "c = sqlite3.connect('file:' + os.path.expandvars('%APPDATA%/RustDesk/config/db_v2.sqlite3') + '?mode=ro', uri=True)",
    "rows = c.execute('select id, info from peer').fetchall()",
    "print(next((r[0] for r in rows if sys.argv[1] in (r[1] or '')), ''))",
  ].join("\n");
  return execFileSync(PY, ["-c", code, ids.ip]).toString().trim();
}

async function measureViewer(label, match) {
  const place = () => ps("topmost.ps1", ...match, "-X", "1040", "-Y", "420", "-W", "1000", "-H", "620") === "ok";
  await until(`${label} window`, place, 60000, 1000);
  // Viewers may show a short "connecting" dialog with the same title first; let the real session
  // window appear, then pin whichever window matches now.
  await sleep(12000);
  const clock = ["--pair", "clock", "--offset", `${-OFFSET}`];
  await until(`${label} remote barcode`, () => place() && meter("--seconds", "2", ...clock).valid_samples > 5, 90000, 1000);
  await sleep(3000);
  console.log(`measuring ${label} …`);
  const r = meter("--seconds", opt.seconds, "--label", label, "--out", OUT, ...clock);
  console.log(`  ${label}: mean ${r.latency_ms?.mean} ms, median ${r.latency_ms?.median}, p10 ${r.latency_ms?.p10}, p95 ${r.latency_ms?.p95}, ${r.distinct_remote_frames_per_s} remote fps`);
  return r;
}

// VM clock offset (VM - this PC): NTP-style lowest-RTT sample over the Hyper-V switch.
const OFFSET = await until("VM clock server", () => parseFloat(execFileSync(PY, [join(ROOT, "tools/bench/clock_offset.py"), "probe", ids.ip, "47999"]).toString()), 180000, 3000);
console.log(`VM clock offset: ${OFFSET.toFixed(2)} ms`);

if (opt.only !== "anydesk") {
  const tid = await until("VM Tether ID in relay", () => tetherIdOfVm(), 120000, 3000);
  console.log(`VM ${ids.ip}: Tether ${tid}`);
  const t = spawn(TETHER, ["--connect", tid, password], { env: { ...process.env, TETHER_PROFILE: "vm-viewer" }, stdio: "ignore" });
  try {
    await measureViewer("tether · VM host · balanced", ["-Process", "tether", "-Title", "tether-bench"]);
  } finally {
    execFileSync("taskkill", ["/PID", String(t.pid), "/T", "/F"], { stdio: "ignore" });
  }
  await sleep(3000);
}

if (opt.only !== "tether") {
  // Password piped on stdin through cmd, as AnyDesk's CLI documents for --with-password.
  writeFileSync(AD_CMD, `@echo off\r\necho ${password}| "${ANYDESK}" ${ids.anydesk} --with-password\r\n`);
  spawn("cmd", ["/c", AD_CMD], { stdio: "ignore", detached: true }).unref();
  // AnyDesk titles the session window with the ID grouped by spaces ("1 605 296 385").
  const spacedId = ids.anydesk.replace(/\B(?=(\d{3})+(?!\d))/g, " ");
  // If AnyDesk still shows its authorization prompt (stdin password not taken), answer it.
  setTimeout(() => {
    try {
      ps("anydesk-login.ps1", "-Title", `${spacedId} - AnyDesk`);
    } catch {}
  }, 9000);
  try {
    await measureViewer("anydesk · VM host · default", ["-Process", "AnyDesk", "-Title", spacedId]);
  } finally {
    // Close only the session window; the user's AnyDesk service keeps running.
    execFileSync("powershell", ["-NoProfile", "-Command", `Get-Process AnyDesk -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowTitle -like '*${spacedId}*' } | ForEach-Object { $_.CloseMainWindow() | Out-Null }`]);
  }
}

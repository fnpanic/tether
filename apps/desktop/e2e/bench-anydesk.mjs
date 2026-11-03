// Black-box glass-to-glass benchmark of AnyDesk on one machine (see tools/bench/README.md).
// The AnyDesk host is the normal Windows install; the viewer is AnyDesk for Linux under WSLg.
// The extra presentation delay of a WSLg window is calibrated first and reported separately.
//
//   node e2e/bench-anydesk.mjs --host-id 1289116920 [--seconds 20]
//
// Someone must click "Accept" in Windows AnyDesk when the request appears.

import { spawn, execFileSync } from "node:child_process";
import { mkdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { chromium } from "playwright-core";

const { values: opt } = parseArgs({ options: { "host-id": { type: "string" }, seconds: { type: "string", default: "20" }, distro: { type: "string", default: "Ubuntu-22.04" } } });
if (!opt["host-id"]) throw new Error("--host-id required");
const ROOT = resolve(import.meta.dirname, "../../..");
const EDGE = "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe";
const PROBE = "file:///" + join(ROOT, "tools/bench/probe.html").replace(/\\/g, "/");
const PY = join(ROOT, "tools/bench/.venv/Scripts/python.exe");
const OUT = join(ROOT, "tools/bench/results/latency.json");
const WSL_BENCH = "/mnt/c/install/remote-support/tools/bench";
const WORK = join(tmpdir(), `tether-bench-ad-${Date.now()}`);
const procs = [];
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const launch = (cmd, args) => procs.push(spawn(cmd, args, { stdio: "ignore" }));
const ps = (file, ...args) => execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", join(ROOT, "tools/bench", file), ...args]).toString().trim();
const wsl = (cmd) => execFileSync("wsl", ["-d", opt.distro, "-e", "bash", "-lc", cmd]).toString().replace(/\0/g, "").trim();
const py = (...args) => execFileSync(PY, [join(ROOT, "tools/bench/glass2glass.py"), ...args]).toString();

async function until(what, fn, timeout, every = 500) {
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

function measure(label, extra = []) {
  console.log(`measuring ${label} …`);
  const r = JSON.parse(py("--seconds", opt.seconds, "--label", label, "--out", OUT, ...extra));
  console.log(`  ${label}: median ${r.latency_ms?.median} ms, p95 ${r.latency_ms?.p95} ms, ${r.distinct_remote_frames_per_s} fps seen, ${r.valid_samples} samples`);
  return r;
}

async function main() {
  rmSync(WORK, { recursive: true, force: true });
  mkdirSync(join(ROOT, "tools/bench/results"), { recursive: true });

  launch(EDGE, [`--app=${PROBE}`, "--window-position=40,40", "--window-size=900,420", "--remote-debugging-port=9224", `--user-data-dir=${join(WORK, "edge")}`, "--no-first-run", "--inprivate", "--disable-sync", "--no-default-browser-check"]);
  const edge = await until("cdp", () => chromium.connectOverCDP("http://127.0.0.1:9224"), 30000);
  const probe = await until("probe", () => edge.contexts().flatMap((c) => c.pages()).find((p) => p.url().includes("probe.html")), 15000);
  await until("probe window", () => ps("topmost.ps1", "-Title", "Tether probe") === "ok", 10000);

  // 1. WSL clock offset (remote − local).
  const ip = wsl("hostname -I | awk '{print $1}'");
  wsl(`cd ${WSL_BENCH} && (setsid nohup python3 clock_offset.py serve 0.0.0.0 47999 >/dev/null 2>&1 &) ; sleep 0.5`);
  const offset = parseFloat(execFileSync(PY, [join(ROOT, "tools/bench/clock_offset.py"), "probe", ip, "47999"]).toString());
  console.log(`WSL clock offset: ${offset.toFixed(2)} ms`);

  // 2. Calibrate WSLg presentation delay vs a native window.
  wsl(`cd ${WSL_BENCH} && (DISPLAY=:0 setsid nohup python3 wsl_probe.py >/dev/null 2>&1 &) ; sleep 2`);
  await until("wsl probe window", () => ps("topmost.ps1", "-Title", "WSLg probe", "-X", "40", "-Y", "520", "-W", "900", "-H", "110") === "ok", 15000);
  await sleep(1000);
  const cal = measure("calibration · WSLg window vs native window", ["--pair", "newest", "--offset", String(-offset)]);
  wsl("pkill -f wsl_probe.py || true");
  const overhead = cal.latency_ms?.median ?? 0;

  // 3. AnyDesk viewer (WSLg) -> AnyDesk host (Windows).
  wsl(`(DISPLAY=:0 setsid nohup anydesk ${opt["host-id"]} >/dev/null 2>&1 &) ; sleep 1`);
  console.log("\n>>> Click ACCEPT in the Windows AnyDesk prompt now (waiting up to 3 minutes) <<<\n");
  await until("AnyDesk session window", () => ps("topmost.ps1", "-Title", `${opt["host-id"]}`, "-X", "960", "-Y", "300", "-W", "1060", "-H", "680") === "ok", 180000, 2000);
  await until("mirrored barcode", () => JSON.parse(py("--seconds", "2")).valid_samples > 5, 120000, 1000);
  await sleep(3000);

  const sub = ["--subtract", String(overhead)];
  measure("anydesk · idle desktop · default", sub);
  await probe.goto(PROBE + "?mode=scroll");
  await sleep(1500);
  measure("anydesk · scrolling text · default", sub);
}

try {
  await main();
} catch (e) {
  console.error("bench failed:", e.message);
  process.exitCode = 1;
} finally {
  for (const p of procs) {
    try {
      execFileSync("taskkill", ["/PID", String(p.pid), "/T", "/F"], { stdio: "ignore" });
    } catch {}
  }
  try {
    wsl("pkill -f 'anydesk [0-9]' || true; pkill -f wsl_probe.py || true");
  } catch {}
}

// Glass-to-glass benchmark for Tether on one machine (see tools/bench/README.md).
//
//   node e2e/bench-tether.mjs [--exe path] [--seconds 20]
//
// Appends results to tools/bench/results/latency.json.

import { spawn, execFileSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { mkdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { chromium } from "playwright-core";

const { values: opt } = parseArgs({ options: { exe: { type: "string" }, seconds: { type: "string", default: "20" } } });
const ROOT = resolve(import.meta.dirname, "../../..");
const EXE = opt.exe ?? join(ROOT, "apps/desktop/src-tauri/target/profiling/tether.exe");
const EDGE = "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe";
const PROBE = "file:///" + join(ROOT, "tools/bench/probe.html").replace(/\\/g, "/");
const PY = join(ROOT, "tools/bench/.venv/Scripts/python.exe");
const OUT = join(ROOT, "tools/bench/results/latency.json");
const PW = "Bench-Pass-" + randomBytes(4).toString("hex") + "!";
const WORK = join(tmpdir(), `tether-bench-${Date.now()}`);
const procs = [];
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

import { openSync } from "node:fs";
const launch = (cmd, args, env = {}, log) =>
  procs.push(spawn(cmd, args, { env: { ...process.env, ...env }, stdio: log ? ["ignore", openSync(log, "w"), openSync(log, "a")] : "ignore" }));
const ps = (file, ...args) =>
  execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", join(ROOT, "tools/bench", file), ...args]).toString().trim();

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
const cdp = (port) => until(`cdp ${port}`, () => chromium.connectOverCDP(`http://127.0.0.1:${port}`), 30000, 500);
const page = (b, m) => until(`page ${m}`, () => b.contexts().flatMap((c) => c.pages()).find((p) => p.url().includes(m)));
const ipc = (p, cmd, args) => p.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a), [cmd, args]);

function measure(label) {
  console.log(`measuring ${label} …`);
  const out = execFileSync(PY, [join(ROOT, "tools/bench/glass2glass.py"), "--seconds", opt.seconds, "--label", label, "--out", OUT]).toString();
  const r = JSON.parse(out);
  console.log(`  ${label}: median ${r.latency_ms?.median} ms, p95 ${r.latency_ms?.p95} ms, ${r.distinct_remote_frames_per_s} fps seen, ${r.valid_samples} samples`);
  return r;
}

async function main() {
  rmSync(WORK, { recursive: true, force: true });
  mkdirSync(join(ROOT, "tools/bench/results"), { recursive: true });

  launch(EDGE, [`--app=${PROBE}`, "--window-position=40,40", "--window-size=900,420", "--remote-debugging-port=9224", `--user-data-dir=${join(WORK, "edge")}`, "--no-first-run", "--inprivate", "--disable-sync", "--no-default-browser-check"]);
  const probe = await page(await cdp(9224), "probe.html");
  await until("probe window", () => ps("topmost.ps1", "-Title", "Tether probe") === "ok", 10000, 300);

  launch(EXE, [], { TETHER_PROFILE: "bench-a", TETHER_DEV_PASSWORD: PW, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9222" }, process.env.BENCH_HOST_LOG);
  const a = await page(await cdp(9222), "index.html");
  const st = await until("host online", async () => {
    const s = await ipc(a, "status");
    return s.status === 1 && s.key_confirmed && s.id ? s : null;
  }, 45000);
  await ipc(a, "plugin:window|minimize", { label: "main" });
  await sleep(3500);

  launch(EXE, ["--connect", st.id, PW], { TETHER_PROFILE: "bench-b", WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9223" });
  const b = await page(await cdp(9223), "/session/");
  const key = b.url().match(/session\/([0-9a-f]{32})/)[1];
  await until("video", () => b.evaluate(() => document.querySelector("canvas")?.width > 0), 30000);
  await until("viewer placed", () => ps("topmost.ps1", "-Title", "— Tether", "-X", "960", "-Y", "300", "-W", "1060", "-H", "680") === "ok", 10000, 300);
  await sleep(4000); // toolbar auto-hides, encoder settles

  if (process.env.BENCH_HOLD) {
    console.log("holding setup for", process.env.BENCH_HOLD, "ms");
    await sleep(+process.env.BENCH_HOLD);
    return [];
  }
  const results = [];
  results.push(measure("tether · idle desktop · balanced"));
  await probe.goto(PROBE + "?mode=scroll");
  await sleep(1500);
  results.push(measure("tether · scrolling text · balanced"));
  await ipc(b, "session_quality", { key, quality: "low" });
  await sleep(2500);
  results.push(measure("tether · scrolling text · speed"));
  return results;
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
  // Only processes of the benchmarked binary, never the user's installed Tether.
  try {
    const exe = EXE.replace(/'/g, "''");
    execFileSync("powershell", ["-NoProfile", "-Command", `Get-CimInstance Win32_Process -Filter "Name='tether.exe'" | Where-Object { $_.ExecutablePath -eq '${exe}' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }`], { stdio: "ignore" });
  } catch {}
}

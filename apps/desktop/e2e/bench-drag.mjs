// Window-drag benchmark against the Hyper-V bench VM (see docs/performance.md).
//
// The VM is the host (TetherQS for the "customer" user). tools/bench/probe.ps1 draws the VM clock
// as a barcode; C:\vms\perf\drag.ps1 drags a window with real mouse input inside the VM. This PC is
// the viewer, but its window stays off the visible desktop: instead of optical capture, a hook in
// the viewer's webview (over CDP) decodes the probe barcode from every picture drawn into the session
// canvas. That gives "glass-to-canvas" latency (VM screen -> picture drawn in the viewer, without the
// viewer's own display refresh), distinct remote frames per second and the longest freeze.
//
//   node e2e/bench-drag.mjs --tech <tether.exe> [--seconds 20] [--label x] [--codec vp8|vp9|av1]
//                           [--id <id> --password <pw>]   (default: read from TetherQS over CDP :9334)

import { spawn, execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { chromium } from "playwright-core";

const { values: opt } = parseArgs({
  options: {
    seconds: { type: "string", default: "20" },
    label: { type: "string", default: "run" },
    tech: { type: "string" },
    id: { type: "string" },
    password: { type: "string" },
    idle: { type: "string", default: "8" },
    out: { type: "string" },
    codec: { type: "string" },
  },
});
const ROOT = resolve(import.meta.dirname, "../../..");
const PY = process.env.BENCH_PY || join(ROOT, "tools/bench/.venv/Scripts/python.exe");
const OUT = opt.out || join(ROOT, "tools/bench/results/drag.json");
const VM_IP = "172.30.252.30";
const CDP = 9235;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

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

/** Start the drag workload in the VM's interactive session (PowerShell Direct, elevated through the
 * broker in C:\vms\broker so no UAC prompt appears on this PC). */
function startDrag(seconds) {
  writeFileSync("C:/vms/perf/cmd.txt", `-File C:\\bench\\perf\\drag.ps1 -Seconds ${seconds}`);
  writeFileSync("C:/vms/perf/wait.txt", "0");
  execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "C:\\vms\\broker\\run-elevated.ps1", "vm-perf-exec.ps1", "-Script", "interactive.ps1"]);
  return readFileSync("C:/vms/perf-exec.log", "utf8").trim();
}

async function qsCreds() {
  const b = await until("TetherQS devtools", () => chromium.connectOverCDP(`http://${VM_IP}:9334`, { timeout: 5000 }), 120000, 3000);
  const p = b.contexts().flatMap((c) => c.pages())[0];
  await until("TetherQS ready", async () => (await p.locator('[data-testid="qs-status"]').innerText()).includes("Ready"), 120000, 2000);
  const id = (await p.locator('[data-testid="qs-id"]').innerText()).replace(/\s/g, "");
  const password = ((await p.locator('[data-testid="qs-password"]').getAttribute("data-password")) ?? "").trim();
  await b.close();
  return { id, password };
}

/**
 * Runs in the viewer page: after every drawImage into a canvas, decode the probe barcode
 * (probe.ps1 encoding: red cell, 32 bits of VM epoch ms, 8-bit checksum, green cell) from the
 * decoded remote frame (1:1 with the remote screen), and record [Date.now() at draw, value].
 */
function canvasMeter() {
  if (window.__g2c) return;
  const rec = (window.__g2c = { s: [], draws: 0, bad: 0, y: -1, x0: 0, cell: 0 });
  const red = (d, i) => d[i] > 150 && d[i + 1] < 90 && d[i + 2] < 90;
  const green = (d, i) => d[i + 1] > 150 && d[i] < 90 && d[i + 2] < 90;
  const locate = (ctx) => {
    const W = Math.min(ctx.canvas.width, 1200), H = Math.min(ctx.canvas.height, 240);
    const d = ctx.getImageData(0, 0, W, H).data;
    for (let y = 0; y < H; y += 2)
      for (let x = 0; x < W; x++) {
        if (!red(d, (y * W + x) * 4)) continue;
        let n = 0;
        while (x + n < W && red(d, (y * W + x + n) * 4)) n++;
        const g = x + Math.round(n * 41.5);
        if (n >= 10 && n <= 40 && g < W && green(d, (y * W + g) * 4)) {
          Object.assign(rec, { y: y + 4, x0: x, cell: n });
          return true;
        }
        x += n;
      }
    return false;
  };
  // Decode luminance at the 40 cell centres; null unless the checksum matches.
  const decode = (lum) => {
    let v = 0, sum = 0;
    for (let i = 0; i < 32; i++) v = v * 2 + (lum(i) < 128 ? 1 : 0);
    for (let i = 32; i < 40; i++) sum = sum * 2 + (lum(i) < 128 ? 1 : 0);
    return ((v & 255) + ((v >>> 8) & 255) + ((v >>> 16) & 255) + ((v >>> 24) & 255) + 0x5a) % 256 === sum ? v : null;
  };
  const orig = CanvasRenderingContext2D.prototype.drawImage;
  CanvasRenderingContext2D.prototype.drawImage = function (...a) {
    const t = Date.now();
    rec.draws++;
    rec.d?.push(t);
    const on = !window.__g2cCountOnly;
    // Read the barcode asynchronously from a clone of the decoded frame: a synchronous canvas
    // readback per frame would stall the viewer's GPU pipeline and distort what is measured.
    let copy = null;
    if (on && rec.y >= 0 && typeof VideoFrame !== "undefined" && a[0] instanceof VideoFrame) {
      try {
        copy = a[0].clone();
      } catch {}
    }
    const r = orig.apply(this, a);
    if (on && rec.y < 0) {
      try {
        locate(this); // one synchronous readback, only until the probe is found
      } catch {}
    }
    if (copy) {
      const c = rec.cell, x = rec.x0 & ~1;
      const rect = { x, y: rec.y & ~1, width: Math.min((c * 42 + 2) & ~1, copy.codedWidth - x), height: 2 };
      const buf = new Uint8Array(copy.allocationSize({ rect }));
      copy.copyTo(buf, { rect }).then(
        (layout) => {
          const rgb = /^(RGB|BGR)/.test(copy.format || "");
          copy.close();
          const off = layout[0].offset;
          const lum = (i) => {
            const px = Math.floor(rec.x0 - x + c * (i + 1) + c / 2);
            if (!rgb) return buf[off + px]; // Y plane (I420/NV12)
            const p = off + px * 4;
            return (buf[p] * 3 + buf[p + 1] * 6 + buf[p + 2]) / 10;
          };
          const v = decode(lum);
          if (v !== null) rec.s.push([t, v]);
          else rec.fails = (rec.fails || 0) + 1;
          if (v === null && ++rec.bad > 30) Object.assign(rec, { y: -1, bad: 0 }); // probe covered or moved
        },
        () => copy.close(),
      );
    }
    return r;
  };
}

/** Latency / frame-rate / freeze statistics from [[pc_ms, vm_ms]] samples; offset = VM − PC clock. */
export function summarize(samples, offset, seconds) {
  const u32 = (x) => ((x % 2 ** 32) + 2 ** 32) % 2 ** 32;
  const diff = (a, b) => {
    const d = u32(a - b);
    return d >= 2 ** 31 ? d - 2 ** 32 : d;
  };
  const lat = [], changes = [];
  for (const [t, v] of [...samples].sort((a, b) => a[0] - b[0])) {
    const d = diff(u32(t), u32(v - offset));
    if (d < -50 || d > 10000) continue;
    lat.push(d);
    if (!changes.length || changes.at(-1)[1] !== v) changes.push([t, v]);
  }
  const q = (xs, p) => xs[Math.min(xs.length - 1, Math.floor(xs.length * p))];
  const s = [...lat].sort((a, b) => a - b);
  const gaps = changes.slice(1).map((c, i) => c[0] - changes[i][0]).sort((a, b) => a - b);
  return {
    samples: lat.length,
    distinct_remote_frames_per_s: +(changes.length / seconds).toFixed(1),
    latency_ms: s.length ? { mean: +(s.reduce((a, b) => a + b, 0) / s.length).toFixed(1), median: q(s, 0.5), p95: q(s, 0.95), max: s.at(-1) } : null,
    freeze_ms: gaps.length ? { max: gaps.at(-1), p99: q(gaps, 0.99), over_300ms: gaps.filter((g) => g > 300).length, over_100ms: gaps.filter((g) => g > 100).length } : null,
  };
}

// Self-check of the statistics (run on every invocation; cheap).
{
  const r = summarize([[1000, 960], [1016, 980], [1033, 980], [1500, 1450]], 0, 1);
  if (r.latency_ms.max !== 53 || r.freeze_ms.max !== 484 || r.freeze_ms.over_300ms !== 1 || r.distinct_remote_frames_per_s !== 3) throw new Error("summarize self-check failed " + JSON.stringify(r));
}

const OFFSET = await until("VM clock server", () => parseFloat(execFileSync(PY, [join(ROOT, "tools/bench/clock_offset.py"), "probe", VM_IP, "47999"]).toString()), 120000, 3000);
console.log(`VM clock offset ${OFFSET.toFixed(1)} ms`);

const { id, password } = opt.id ? { id: opt.id, password: opt.password } : await qsCreds();
const viewer = spawn(opt.tech, ["--connect", id, "--password-stdin"], {
  env: { ...process.env, TETHER_PROFILE: "perf-viewer", WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${CDP}` },
  stdio: ["pipe", "ignore", "ignore"],
});
viewer.stdin.end(`${password}\n`);

const result = { label: opt.label, at: new Date().toISOString(), offset_ms: OFFSET };
try {
  const b = await until("viewer devtools", () => chromium.connectOverCDP(`http://127.0.0.1:${CDP}`), 60000, 1000);
  const page = await until("session page", () => b.contexts().flatMap((c) => c.pages()).find((x) => x.url().includes("/session/")), 60000, 500);
  await page.evaluate(canvasMeter);
  await until("remote barcode in the canvas", () => page.evaluate(() => window.__g2c.s.length > 5), 90000, 1000);
  await page.evaluate(() => document.querySelector('[aria-label="Statistics"]')?.click());
  if (opt.codec) {
    // Viewer-side codec preference (what the codec menu does); the host re-negotiates.
    await page.evaluate((codec) => window.__TAURI_INTERNALS__.invoke("session_codec", { key: location.hash.replace(/^#/, "").split("?")[0].slice("/session/".length), codec }), opt.codec);
  }
  await sleep(5000);
  if (process.env.BENCH_COUNT_ONLY) await page.evaluate(() => (window.__g2cCountOnly = true));
  const overlay = [];
  let sampling = true;
  const sampler = (async () => {
    while (sampling) {
      overlay.push(await page.evaluate(() => [...document.querySelectorAll("div")].find((d) => d.textContent?.startsWith("fps"))?.innerText.replace(/\n/g, " ") ?? "").catch(() => ""));
      await sleep(1000);
    }
  })();
  const phase = async (name, secs) => {
    await page.evaluate(() => Object.assign(window.__g2c, { s: [], d: [], fails: 0 }));
    const draws0 = await page.evaluate(() => window.__g2c.draws);
    await sleep(secs * 1000);
    const { s, d, draws, fails } = await page.evaluate(() => ({ ...window.__g2c }));
    // Freeze = time without a new picture drawn (during a drag every encoded frame is new content).
    const gaps = d.slice(1).map((t, i) => t - d[i]).sort((a, b) => a - b);
    const draw_gap_ms = gaps.length ? { max: gaps.at(-1), p99: gaps[Math.floor(gaps.length * 0.99)], over_300ms: gaps.filter((g) => g > 300).length, over_100ms: gaps.filter((g) => g > 100).length } : null;
    result[name] = { ...summarize(s, OFFSET, secs), drawn_per_s: +((draws - draws0) / secs).toFixed(1), draw_gap_ms, undecodable: fails };
  };
  console.log(`idle ${opt.idle} s …`);
  await phase("idle", Number(opt.idle));
  const idleOverlay = overlay.splice(0);
  const secs = Number(opt.seconds);
  console.log(startDrag(secs + 6));
  await sleep(3500); // task start + PowerShell/C# compile + 1.5 s settle in drag.ps1
  console.log(`drag ${secs} s …`);
  await phase("drag", secs);
  sampling = false;
  await sampler;
  result.overlay = { idle: idleOverlay, drag: overlay };
  for (const k of ["idle", "drag"]) {
    const r = result[k];
    console.log(`${k}: latency mean ${r.latency_ms?.mean} p95 ${r.latency_ms?.p95} max ${r.latency_ms?.max} | ${r.distinct_remote_frames_per_s} remote fps, ${r.drawn_per_s} drawn/s | freeze max ${r.freeze_ms?.max} ms, >300ms ${r.freeze_ms?.over_300ms}, >100ms ${r.freeze_ms?.over_100ms} | ${r.samples} decoded, ${r.undecodable} undecodable | draw gap max ${r.draw_gap_ms?.max} ms, >300ms ${r.draw_gap_ms?.over_300ms}, >100ms ${r.draw_gap_ms?.over_100ms}`);
  }
  let all = [];
  try {
    all = JSON.parse(readFileSync(OUT, "utf8"));
  } catch {}
  all.push(result);
  writeFileSync(OUT, JSON.stringify(all, null, 2));
} catch (e) {
  console.error("FAILED:", e.message);
} finally {
  execFileSync("taskkill", ["/PID", String(viewer.pid), "/T", "/F"], { stdio: "ignore" });
  process.exit(0);
}

// Sharpening in a real session: frame rate, draw time and latency with Sharpening Off / Auto / Strong,
// plus switching, context loss, window resize and the stress sampler on the WebGL canvas.
//
//   node e2e/sharpen-e2e.mjs --exe C:\cargo-target\sharp\debug\tether.exe [--seconds 40]
//
// Debug build (frontend from the Vite dev server this script starts on :1420; TETHER_DEV_PASSWORD).
// Host (profile sharp-host) and viewer (sharp-view) run on an invisible desktop (hidden-run.ps1) and
// are driven over DevTools only. The host streams the PC's screen, which shows tools/bench/stress.html
// (pinned to the scrolling-text phase) in an Edge window started without activation. Latency comes
// from tools/bench/stress-sampler.mjs (barcode, same clock on both sides). Only processes this script
// started are stopped (by PID tree).
import { execFileSync, spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { createWriteStream, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { chromium } from "playwright-core";

const { values: opt } = parseArgs({ options: { exe: { type: "string" }, seconds: { type: "string", default: "40" }, out: { type: "string" } } });
if (!opt.exe) throw new Error("--exe is required");
const SEC = +opt.seconds;
const ROOT = resolve(import.meta.dirname, "..");
const REPO = resolve(ROOT, "../..");
const DESK = "tether-sharp-test";
const HIDDEN = resolve(import.meta.dirname, "hidden-run.ps1");
const EDGE = "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe";
const WORK = join(tmpdir(), `tether-sharp-${Date.now()}`);
const SHOTS = join(WORK, "shots");
const OUT = opt.out ?? join(REPO, "tools/bench/results/sharpen/e2e.json");
const PW = "Sharp-Pass-" + randomBytes(4).toString("hex") + "!";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
mkdirSync(SHOTS, { recursive: true });
const pids = [];
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
const page = (b, m) => until(`page ${m}`, () => b.contexts().flatMap((c) => c.pages()).find((p) => m(p.url())), 60000);
const ipc = (p, cmd, args) => p.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a), [cmd, args]);
// hidden-run prints the PID; the started app inherits the pipe and logs into it (a Rust app exits on
// a closed stdout), so the pipe is drained into a log file for the app's lifetime and never awaited.
// An empty -Args value would be dropped by PowerShell, so it is left out.
const hidden = (exe, args, env, desk = DESK, show = -1) =>
  new Promise((ok, fail) => {
    const n = pids.length;
    const log = createWriteStream(join(WORK, `hidden-${n}.log`));
    const ps = spawn("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", HIDDEN, "-Exe", exe, ...(args ? ["-Args", args] : []), "-Desktop", desk, ...(show >= 0 ? ["-Show", String(show)] : [])], {
      env: { ...process.env, ...env },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let first = "";
    ps.stdout.on("data", (d) => {
      log.write(d);
      if (pids.length > n) return;
      first += d;
      const m = first.match(/^\s*(\d+)\s*$/m);
      if (m) {
        pids.push(+m[1]);
        ok(+m[1]);
      }
    });
    ps.stderr.on("data", (d) => log.write(d));
    ps.on("exit", (c) => pids.length > n || fail(new Error(`hidden-run ${exe} exited ${c} (see ${join(WORK, `hidden-${n}.log`)})`)));
    ps.stdout.unref?.();
    ps.unref();
  });

function topmost(title) {
  const ps = `Add-Type @"
using System; using System.Text; using System.Runtime.InteropServices;
public class TM { public delegate bool P(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(P f, IntPtr l);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr a, int x, int y, int w, int hh, uint f); }
"@
$f = [IntPtr]::Zero
[TM]::EnumWindows({ param($h, $l) $b = New-Object Text.StringBuilder 256; [void][TM]::GetWindowText($h, $b, 256); if ([TM]::IsWindowVisible($h) -and $b.ToString().Contains('${title}')) { $script:f = $h; return $false }; $true }, [IntPtr]::Zero) | Out-Null
if ($f -eq [IntPtr]::Zero) { 'notfound' } else { [void][TM]::SetWindowPos($f, [IntPtr](-1), 0, 0, 1100, 800, 0x0050); 'ok' }`; // HWND_TOPMOST, SWP_SHOWWINDOW|SWP_NOACTIVATE
  return execFileSync("powershell", ["-NoProfile", "-Command", ps]).toString().trim();
}

// ------------------------------------------------------------------ viewer helpers (DOM over DevTools)
const S = {
  // The toolbar's Image quality menu holds the Sharpening radio group.
  setMode: (m) => {
    const q = document.querySelector('button[title="Image quality"]');
    q.click();
    return new Promise((r) =>
      setTimeout(() => {
        const b = [...document.querySelectorAll('[aria-label="Sharpening"] [role=radio]')].find((x) => x.textContent === m);
        b?.click();
        setTimeout(() => (q.click(), r(!!b)), 50);
      }, 50),
    );
  },
  read: () => {
    const c = document.querySelector("canvas");
    const t = document.body.innerText;
    const fps = t.match(/fps\s*(\d+) shown \/ (\d+) recv/);
    const rn = t.match(/render\s*([\d.]+) ms · sharpen ([^\n]+)/);
    return {
      t: Date.now(),
      fps: fps ? +fps[1] : null,
      recv: fps ? +fps[2] : null,
      renderMs: rn ? +rn[1] : null,
      sharpen: rn ? rn[2].trim() : null,
      // From the statistics: probing with getContext would give a fresh canvas a context of its own.
      gl: !!rn && !/^(off|unavailable)/.test(rn[2].trim()),
      canvas: [c.width, c.height],
      box: [Math.round(c.getBoundingClientRect().width * devicePixelRatio), Math.round(c.getBoundingClientRect().height * devicePixelRatio)],
      dpr: devicePixelRatio,
      heap: performance.memory?.usedJSHeapSize ?? null,
    };
  },
};

async function phase(sess, mode, seconds, log) {
  check(`switch to ${mode}`, await sess.evaluate(S.setMode, mode));
  await sleep(3000); // settle; the stats refresh every second
  const from = Date.now();
  const rows = [];
  while (Date.now() - from < seconds * 1000) {
    await sleep(1000);
    rows.push(await sess.evaluate(S.read));
  }
  log.push({ mode, from, to: Date.now(), rows });
  if (SHOTS && !log.slice(0, -1).some((p) => p.mode === mode)) await sess.screenshot({ path: join(SHOTS, `viewer-${mode.toLowerCase()}.png`) }).catch(() => {});
  const avg = (k) => +(rows.reduce((s, r) => s + (r[k] ?? 0), 0) / rows.length).toFixed(2);
  console.log(`.. ${mode}: fps ${avg("fps")} (recv ${avg("recv")}), render ${avg("renderMs")} ms, ${rows.at(-1).sharpen}, gl ${rows.at(-1).gl}, canvas ${rows.at(-1).canvas} box ${rows.at(-1).box}`);
  return rows;
}

function latencyBy(log, file) {
  const lines = existsSync(file) ? readFileSync(file, "utf8").trim().split("\n").map((l) => JSON.parse(l)) : [];
  return log.map((p) => {
    const lat = lines.filter((s) => s.lat != null && s.t >= p.from && s.t <= p.to).map((s) => s.lat).sort((a, b) => a - b);
    const q = (x) => (lat.length ? lat[Math.min(lat.length - 1, Math.floor(x * lat.length))] : null);
    return { mode: p.mode, samples: lat.length, p50: q(0.5), p95: q(0.95), mean: lat.length ? +(lat.reduce((a, b) => a + b, 0) / lat.length).toFixed(1) : null };
  });
}

const summary = (log) =>
  log.map((p) => {
    const avg = (k) => +(p.rows.reduce((s, r) => s + (r[k] ?? 0), 0) / p.rows.length).toFixed(3);
    const last = p.rows.at(-1);
    return { mode: p.mode, fps: avg("fps"), recv: avg("recv"), renderMs: avg("renderMs"), sharpen: last.sharpen, gl: last.gl, canvas: last.canvas, box: last.box, dpr: last.dpr };
  });

async function main() {
  // Every profile on one PC derives the same machine ID: give the test host its own.
  const dir = join(process.env.APPDATA, "Tether-sharp-host", "config");
  if (!existsSync(join(dir, "Tether-sharp-host.toml"))) {
    mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, "Tether-sharp-host.toml"), `id = '${700000000 + Math.floor(Math.random() * 99999999)}'\n`);
  }
  const vite = spawn(process.execPath, [join(ROOT, "node_modules/vite/bin/vite.js"), "--port", "1420", "--strictPort"], { cwd: ROOT, stdio: ["ignore", "pipe", "inherit"] });
  await new Promise((ok, fail) => {
    vite.stdout.on("data", (d) => String(d).includes("ready") && ok());
    vite.on("exit", (c) => fail(new Error(`vite exited ${c} (port 1420 busy?)`)));
  });
  try {
    // Screen content: the stress page (scrolling text + barcode) on the user's desktop, not activated (SW_SHOWNOACTIVATE).
    const stress = "file:///" + join(REPO, "tools/bench/stress.html").replace(/\\/g, "/") + "?phase=1";
    await hidden(EDGE, `--app=${stress} --window-position=0,0 --window-size=1100,800 --user-data-dir=${join(WORK, "edge")} --no-first-run --inprivate --disable-sync --no-default-browser-check`, {}, "Default", 4);

    // On top without activation (no focus change on the user's desktop), so the host captures it.
    await sleep(3000);
    console.log("stress window:", topmost("Tether stress"));
    await hidden(opt.exe, "", { TETHER_PROFILE: "sharp-host", TETHER_DEV_PASSWORD: PW, WEBVIEW2_USER_DATA_FOLDER: join(WORK, "wv2-host"), WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9291" });
    const hostB = await cdp(9291);
    const main = await page(hostB, (u) => u.includes("localhost:1420") && !u.includes("#/cm") && !u.includes("/session/"));
    const st = await until("host online", async () => {
      const s = await ipc(main, "status");
      return s.status === 1 && s.key_confirmed && s.id ? s : null;
    }, 90000);
    await sleep(4000); // dev password is set 3 s after start
    console.log(`host ${st.id}`);

    await hidden(opt.exe, `--connect ${st.id} --password-env TETHER_E2E_PW`, { TETHER_PROFILE: "sharp-view", TETHER_E2E_PW: PW, WEBVIEW2_USER_DATA_FOLDER: join(WORK, "wv2-view"), WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9292" });
    const viewB = await cdp(9292);
    const sess = await page(viewB, (u) => u.includes("/session/"));
    const warnings = [];
    sess.on("console", (m) => /webgl|context|\[video\]/i.test(m.text()) && warnings.push(m.text()));
    await until("picture", () => sess.evaluate(() => (document.querySelector("canvas")?.getBoundingClientRect().width ?? 0) > 0), 60000);
    await sess.evaluate(() => document.querySelector('button[title="Statistics"]').click());
    await sleep(2500);
    check("Auto by default", (await sess.evaluate(S.read)).gl);

    const sampler = spawn(process.execPath, [join(REPO, "tools/bench/stress-sampler.mjs"), "--port", "9292", "--seconds", String(SEC * 6 + 60), "--every", "100", "--out", join(WORK, "lat.jsonl")], { stdio: "ignore" });

    await sleep(8000); // the sampler connects and finds the barcode
    // 1. Normal mode (no GPU waits): frame rate, CPU draw time, latency.
    const log = [];
    for (const m of ["Off", "Auto", "Off", "Auto", "Strong"]) await phase(sess, m, SEC, log);
    sampler.kill();
    await sleep(500);
    const lat = latencyBy(log, join(WORK, "lat.jsonl"));
    for (const l of lat) console.log(`.. latency ${l.mode}: ${l.samples} samples, p50 ${l.p50} ms, p95 ${l.p95} ms, mean ${l.mean} ms`);
    check("stress sampler decodes the WebGL canvas", lat.filter((l) => l.mode !== "Off").every((l) => l.samples > 10));
    check("stress sampler decodes the 2D canvas", lat.filter((l) => l.mode === "Off").every((l) => l.samples > 10));

    // 2. Debug measurement mode: every draw waits for the GPU (renderMs = CPU + GPU).
    await sess.evaluate(() => localStorage.setItem("tether.renderSync", "1")); // read by the pipeline once a second
    const sync = [];
    for (const m of ["Off", "Auto", "Strong", "Off", "Auto"]) await phase(sess, m, Math.max(15, SEC / 2), sync);
    await sess.evaluate(() => localStorage.removeItem("tether.renderSync"));

    // 3. Size changes: Original size (1:1, Auto skips), a smaller window (DevTools emulation: the
    // window API is not allowed for the page), back. The WebGL canvas follows its device pixels.
    await sess.evaluate(S.setMode, "Auto");
    const sizes = [];
    const size = async (what) => {
      await sleep(1500);
      const r = await sess.evaluate(S.read);
      sizes.push({ what, canvas: r.canvas, box: r.box, dpr: r.dpr, sharpen: r.sharpen });
      check(`${what}: canvas ${r.canvas} = on-screen pixels ${r.box} (dpr ${r.dpr}, ${r.sharpen})`, r.gl && Math.abs(r.canvas[0] - r.box[0]) <= 1 && Math.abs(r.canvas[1] - r.box[1]) <= 1);
      return r;
    };
    await sess.evaluate(() => document.querySelector('button[title="Original size"]').click());
    const one = await size("original size");
    check(`original size: Auto skips 1:1 (${one.sharpen})`, one.sharpen === "auto 0.00");
    await sess.evaluate(() => document.querySelector('button[title="Fit to window"]').click());
    const devtools = await sess.context().newCDPSession(sess);
    // deviceScaleFactor 0: keep the display's real scale (the ResizeObserver reports real device
    // pixels, an emulated scale would only change devicePixelRatio). HiDPI: this PC runs at 125 %.
    await devtools.send("Emulation.setDeviceMetricsOverride", { width: 1100, height: 650, deviceScaleFactor: 0, mobile: false });
    await size("window 1100×650");
    await devtools.send("Emulation.clearDeviceMetricsOverride");
    await size("window restored");

    // 4. Switching back and forth (new canvas each time): no context leak, picture keeps coming.
    const heap0 = (await sess.evaluate(S.read)).heap;
    for (let i = 0; i < 30; i++) await sess.evaluate(S.setMode, ["Off", "Auto", "Strong"][i % 3]);
    await sess.evaluate(S.setMode, "Auto");
    await sleep(4000);
    const after = await sess.evaluate(S.read);
    check(`30 switches: WebGL again, picture moving (fps ${after.fps})`, after.gl && after.fps > 0);
    check(`no "too many WebGL contexts" (${warnings.filter((w) => /too many/i.test(w)).length})`, !warnings.some((w) => /too many/i.test(w)));

    // 5. Context loss: the 2D canvas takes over and the picture comes back.
    await sess.evaluate(() => document.querySelector("canvas").getContext("webgl2").getExtension("WEBGL_lose_context").loseContext());
    const lost = await until("2D after context loss", async () => {
      const r = await sess.evaluate(S.read);
      return !r.gl && r.sharpen?.startsWith("unavailable") && r.fps > 0 ? r : null;
    }, 15000).catch(() => sess.evaluate(S.read));
    check(`context loss → 2D canvas, picture moving (${lost.sharpen}, fps ${lost.fps})`, !lost.gl && lost.fps > 0);

    const result = { date: new Date().toISOString(), exe: opt.exe, seconds: SEC, normal: summary(log), latency: lat, sync: summary(sync), resize: sizes, heap: { before: heap0, after: after.heap }, warnings: warnings.slice(0, 20) };
    writeFileSync(OUT, JSON.stringify(result, null, 1));
    console.log(JSON.stringify({ normal: result.normal, latency: lat, sync: result.sync }, null, 1));
  } finally {
    vite.kill();
  }
}

main()
  .catch((e) => {
    console.error(e);
    failures++;
  })
  .finally(() => {
    for (const pid of pids) {
      try {
        execFileSync("taskkill", ["/T", "/F", "/PID", String(pid)], { stdio: "ignore" });
      } catch {}
    }
    console.log(failures ? `${failures} FAILED` : "ALL PASSED");
    process.exit(failures ? 1 : 0);
  });

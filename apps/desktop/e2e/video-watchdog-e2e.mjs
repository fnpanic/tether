// End-to-end: the viewer's video watchdog (lib/video.ts `watchdog`, Session.tsx).
// Host: a Tether host profile on this PC (debug build, TETHER_DEV_PASSWORD), or TetherQS in the VM
// (HOST=vm: ID/password over its devtools, as vm-elevpol-e2e.mjs). Viewer: the debug build, own
// profile, devtools on 9253. Everything on an invisible desktop (hidden-run.ps1).
//   node e2e/video-watchdog-e2e.mjs normal [secs]   no forced failure: the watchdog must stay quiet
//   node e2e/video-watchdog-e2e.mjs webcodecs       WebCodecs draws nothing -> native decoding
//   node e2e/video-watchdog-e2e.mjs native          native draws nothing either -> another codec
// The two failure modes use TETHER_DEV_VIDEO_STALL, read only by debug builds.
// Prints one line per second (fps shown/recv, codec) and the watchdog's console lines and toasts.
import { execFileSync, spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { resolve } from "node:path";
import { chromium } from "playwright-core";

const mode = process.argv[2] || "normal";
const secs = Number(process.argv[3] || (mode === "normal" ? 120 : 40));
const TECH = process.env.TECH_EXE || "C:\\cargo-target\\watchdog\\debug\\tether.exe";
const HIDDEN = resolve(import.meta.dirname, "hidden-run.ps1");
const VM = process.env.HOST === "vm";
const QS_CDP = `http://${process.env.VM_IP || "172.30.252.30"}:${process.env.QS_CDP || 9334}`;
const HOST_CDP = 9252, VIEW_CDP = 9253;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const pages = (b) => b.contexts().flatMap((c) => c.pages());
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
const cdp = (url) => until(`devtools ${url}`, () => chromium.connectOverCDP(url, { timeout: 5000 }), 90000, 500);
function kill() {
  const ps = `Get-CimInstance Win32_Process -Filter "Name='tether.exe'" | Where-Object { $_.ExecutablePath -eq '${TECH}' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }`;
  execFileSync("powershell", ["-NoProfile", "-Command", ps], { stdio: "ignore" });
}
/** Start on the invisible desktop; the powershell keeps the desktop alive meanwhile. */
const holders = [];
function start(args, env) {
  holders.push(spawn("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", `& '${HIDDEN}' -Exe '${TECH}' -Args '${args}' | Out-Null; Start-Sleep 3600`], { env: { ...process.env, ...env }, stdio: "ignore" }));
}

kill();
const t0 = Date.now();
const log = (...a) => console.log(`${((Date.now() - t0) / 1000).toFixed(1).padStart(6)}s`, ...a);
let id, pw;
try {
  if (VM) {
    const qsB = await cdp(QS_CDP);
    const qs = await until("TetherQS page", () => pages(qsB).find((p) => !p.url().startsWith("devtools")));
    id = (await qs.locator('[data-testid="qs-id"]').innerText()).replace(/\s/g, "");
    pw = ((await qs.locator('[data-testid="qs-password"]').getAttribute("data-password")) ?? "").trim();
  } else {
    pw = "Wd-" + randomBytes(6).toString("hex") + "!A";
    // Its own ID: profiles on one machine otherwise share the machine-derived one (and the viewer
    // would refuse it as its own).
    const dir = join(process.env.APPDATA, "Tether-watchdog-host", "config");
    rmSync(dir, { recursive: true, force: true });
    mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, "Tether-watchdog-host.toml"), `id = '${700000000 + Math.floor(Math.random() * 99999999)}'
`);
    start("", { TETHER_PROFILE: "watchdog-host", TETHER_DEV_PASSWORD: pw, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${HOST_CDP}` });
    const hb = await cdp(`http://127.0.0.1:${HOST_CDP}`);
    const hp = await until("host window", () => pages(hb).find((p) => p.url().endsWith("#/")));
    id = await until("host ID online", async () => {
      const s = await hp.evaluate(() => window.__TAURI_INTERNALS__.invoke("status"));
      return s.status === 1 && s.id;
    }, 60000, 500);
    await sleep(4000); // TETHER_DEV_PASSWORD is applied after 3 s
  }
  log("host", id);
  start(`--connect ${id} --password-env WD_PW`, {
    TETHER_PROFILE: "watchdog-view",
    WD_PW: pw,
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${VIEW_CDP}`,
    TETHER_DEV_VIDEO_STALL: mode === "normal" ? "" : mode,
  });
  const vb = await cdp(`http://127.0.0.1:${VIEW_CDP}`);
  const v = await until("session window", () => pages(vb).find((p) => p.url().includes("/session/")), 40000);
  const events = [];
  v.on("console", (m) => /watchdog|webcodecs|decoder|key frame/.test(m.text()) && (events.push(m.text()), log("console:", m.text())));
  await until("connected", () => v.evaluate(() => !!document.querySelector('[aria-label="Statistics"]')), 60000);
  await v.evaluate(() => document.querySelector('[aria-label="Statistics"]').click());
  log("connected, stats open");
  const toasts = new Set();
  const rows = [];
  for (let i = 0; i < secs; i++) {
    await sleep(1000);
    const st = await v.evaluate(() => {
      const o = {};
      for (const d of document.querySelectorAll("div.flex.justify-between")) {
        const [k, val] = d.querySelectorAll("span");
        if (k && val) o[k.textContent] = val.textContent;
      }
      const c = document.querySelector("canvas");
      o.canvas = c ? `${c.width}x${c.height}` : "";
      o.toasts = [...document.querySelectorAll('[role="status"]')].map((t) => t.textContent);
      return o;
    });
    for (const t of st.toasts) if (!toasts.has(t)) toasts.add(t), log("toast:", t);
    rows.push(st);
    log(`fps ${st.fps} · codec ${st.codec} · ${st.stall} · canvas ${st.canvas}`);
  }
  const shown = rows.map((r) => Number(/^(\d+) shown/.exec(r.fps ?? "")?.[1] ?? 0));
  const recv = rows.map((r) => Number(/(\d+) recv/.exec(r.fps ?? "")?.[1] ?? 0));
  const summary = {
    mode,
    secs,
    secondsWithFrames: recv.filter((x) => x > 0).length,
    secondsStill: recv.filter((x) => x === 0).length,
    framesRecv: recv.reduce((a, b) => a + b, 0),
    framesShown: shown.reduce((a, b) => a + b, 0),
    lastCodec: rows.at(-1)?.codec,
    watchdogLines: events.filter((e) => e.includes("watchdog")),
    toasts: [...toasts],
  };
  console.log(JSON.stringify(summary, null, 2));
  const ok =
    mode === "normal"
      ? summary.watchdogLines.length === 0 && summary.toasts.length === 0
      : mode === "webcodecs"
        ? summary.watchdogLines.length === 1 && shown.slice(-5).some((x) => x > 0)
        : summary.watchdogLines.length === 2 && shown.slice(-5).some((x) => x > 0);
  console.log(ok ? "PASS" : "FAIL");
  process.exitCode = ok ? 0 : 1;
} finally {
  kill();
  for (const h of holders) h.kill();
}

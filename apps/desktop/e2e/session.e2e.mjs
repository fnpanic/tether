// End-to-end session test on one Windows machine.
//
// Starts: the bench probe page (host screen content), Tether host "A" and Tether viewer "B" (debug
// build, separate profiles), then drives B's real session window over WebView2's DevTools
// protocol. Because host and viewer share a desktop and a clock, every effect is checked on the
// host side and every latency is exact.
//
//   node e2e/session.e2e.mjs            (needs the relay running and a debug build)
//
// Results: tools/bench/results/e2e.json

import { spawn, execFileSync } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync, existsSync, statSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { chromium } from "playwright-core";

const ROOT = resolve(import.meta.dirname, "../../..");
const EXE = join(ROOT, "apps/desktop/src-tauri/target/debug/tether.exe");
const EDGE = "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe";
const PROBE = "file:///" + join(ROOT, "tools/bench/probe.html").replace(/\\/g, "/");
const PW = "E2e-Pass-" + randomBytes(4).toString("hex") + "!";
const WORK = join(tmpdir(), "tether-e2e");
const results = { started: new Date().toISOString(), tests: [] };
const procs = [];
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let currentStep = "start";
const step = (name) => {
  currentStep = name;
  console.log(`…  ${name}`);
};
// Any single step hanging longer than this fails the run instead of stalling it.
const withTimeout = (p, ms, what) => Promise.race([p, sleep(ms).then(() => { throw new Error(`${what} timed out after ${ms} ms`); })]);

function launch(cmd, args, env = {}) {
  const p = spawn(cmd, args, { env: { ...process.env, ...env }, stdio: "ignore", detached: false });
  procs.push(p);
  return p;
}

async function until(what, fn, timeout = 30000, every = 250) {
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
  throw new Error(`timeout waiting for ${what}${last ? ": " + last.message : ""}`);
}

async function cdp(port) {
  return until(`devtools on :${port}`, () => chromium.connectOverCDP(`http://127.0.0.1:${port}`), 30000, 500);
}

async function page(browser, match) {
  return until(`page ${match}`, () => browser.contexts().flatMap((c) => c.pages()).find((p) => p.url().includes(match)));
}

const ipc = (p, cmd, args) =>
  Promise.race([
    p.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a), [cmd, args]),
    sleep(20000).then(() => {
      throw new Error(`IPC ${cmd} did not return within 20 s`);
    }),
  ]);

function stats(xs) {
  const s = [...xs].sort((a, b) => a - b);
  const q = (f) => s[Math.min(s.length - 1, Math.floor(s.length * f))];
  return { n: s.length, median: q(0.5), p95: q(0.95), min: s[0], max: s.at(-1) };
}

function record(name, ok, detail = {}) {
  results.tests.push({ name, ok, ...detail });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}`, Object.keys(detail).length ? JSON.stringify(detail) : "");
}

async function main() {
  rmSync(WORK, { recursive: true, force: true });
  mkdirSync(WORK, { recursive: true });

  // 1. Probe page at a known place on the host screen.
  launch(EDGE, [`--app=${PROBE}`, "--window-position=40,40", "--window-size=900,420", "--remote-debugging-port=9224", `--user-data-dir=${join(WORK, "edge")}`, "--no-first-run", "--inprivate", "--disable-sync", "--no-default-browser-check", "--disable-features=msEdgeSyncConsent,EdgeSignInPrompt"]);
  const edge = await cdp(9224);
  const probe = await page(edge, "probe.html");
  const pin = () =>
    execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", join(ROOT, "tools/bench/topmost.ps1"), "-Title", "Tether probe"]).toString().trim();
  await until("probe window", () => pin() === "ok", 10000, 300);

  // 2. Host A and viewer B.
  launch(EXE, [], { TETHER_PROFILE: "e2e-a", TETHER_DEV_PASSWORD: PW, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9222" });
  const a = await page(await cdp(9222), "index.html");
  const st = await until("host online", async () => {
    const s = await ipc(a, "status");
    return s.status === 1 && s.key_confirmed && s.id ? s : null;
  }, 45000);
  await sleep(3500); // dev password is applied 3 s after start
  record("host registers with relay (pinned key)", true, { id: st.id });

  launch(EXE, ["--connect", st.id, PW], { TETHER_PROFILE: "e2e-b", WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9223" });
  const bBrowser = await cdp(9223);
  const b = await page(bBrowser, "/session/");
  const key = b.url().match(/session\/([0-9a-f]{32})/)[1];
  const t0 = Date.now();
  await until("first video frame", () => b.evaluate(() => document.querySelector("canvas")?.width > 0), 30000);
  record("password login + first frame", true, { ms_to_first_frame: Date.now() - t0 });
  // Host and viewer share this desktop: keep the viewer window out of the injected input's way.
  step("minimize viewer");
  await ipc(b, "plugin:window|minimize", { label: `s-${key}` }).catch((e) => console.log("minimize:", e.message));
  step("pin probe");
  pin();
  await sleep(500);

  // --no-input: skip everything that injects mouse/keyboard on this desktop (safe while the PC is in use).
  if (!process.argv.includes("--no-input")) {
  // 3. Mouse: click inside the probe window through the remote session.
  step("geometry");
  const geo = await withTimeout(probe.evaluate(() => ({ x: screenX, y: screenY, dpr: devicePixelRatio, top: outerHeight - innerHeight })), 10000, "probe geometry");
  const target = (dx, dy) => ({ x: Math.round((geo.x + dx) * geo.dpr), y: Math.round((geo.y + geo.top + dy) * geo.dpr) });
  const mouse = (mask, p) => ipc(b, "session_mouse", { key, mask, x: p.x, y: p.y, alt: false, ctrl: false, shift: false, command: false });
  step("click");
  await probe.evaluate(() => (window.__events.length = 0));
  const p1 = target(200, 250);
  await mouse(0, p1);
  const tClick = Date.now();
  await mouse(1 | (1 << 3), p1);
  await mouse(2 | (1 << 3), p1);
  const down = await until("remote click", () => probe.evaluate(() => window.__events.find((e) => e.type === "pointerdown")), 5000, 20).catch(async (e) => {
    const evs = await probe.evaluate(() => window.__events.map((x) => `${x.type}@${x.x},${x.y}`));
    const fg = execFileSync("powershell", ["-NoProfile", "-Command", "Add-Type -AssemblyName System.Windows.Forms; $p=[System.Windows.Forms.Cursor]::Position; \"$($p.X),$($p.Y)\""]).toString().trim();
    throw new Error(`${e.message}; probe saw [${evs.slice(-8).join(" ")}] (${evs.length}), cursor=${fg}, target=${JSON.stringify(p1)}, geo=${JSON.stringify(geo)}`);
  });
  record("mouse click reaches host", true, { latency_ms: down.t - tClick });

  step("keyboard");
  // 4. Keyboard: physical keys (KeyboardEvent.code) typed into the focused probe.
  await probe.evaluate(() => (window.__events.length = 0));
  const typed = ["KeyT", "KeyE", "KeyT", "KeyH", "KeyE", "KeyR"];
  const sent = [];
  for (const code of typed) {
    sent.push(Date.now());
    await ipc(b, "session_key", { key, code, down: true, lockModes: 0, character: "" });
    await ipc(b, "session_key", { key, code, down: false, lockModes: 0, character: "" });
    await sleep(60);
  }
  const keys = await until("remote keys", async () => {
    const ev = await probe.evaluate(() => window.__events.filter((e) => e.type === "keydown"));
    return ev.length >= typed.length ? ev : null;
  }, 5000, 20);
  const word = keys.map((k) => k.key).join("");
  record("keyboard reaches host", word === "tether", { typed: word, latency_ms: stats(keys.map((k, i) => k.t - sent[i])) });

  step("input latency");
  // 5. Input latency: 40 pointer moves, time until the host page sees each one.
  const moves = [];
  for (let i = 0; i < 40; i++) {
    await probe.evaluate(() => (window.__events.length = 0));
    const p = target(100 + (i % 2) * 300, 200 + (i % 5) * 20);
    const t = Date.now();
    await mouse(0, p);
    const ev = await until("move", () => probe.evaluate(() => window.__events.find((e) => e.type === "pointermove")), 3000, 2).catch(() => null);
    if (ev) moves.push(ev.t - t);
    await sleep(30);
  }
  record("input latency (viewer → host event)", moves.length >= 35, { pointer_move_ms: stats(moves) });

  step("file transfer");
  }

  // 6. File transfer: 64 MB upload through an encrypted file-transfer session, verified by hash.
  const src = join(WORK, "upload.bin");
  const dstDir = join(WORK, "remote-dst");
  mkdirSync(dstDir, { recursive: true });
  const blob = randomBytes(64 * 1024 * 1024);
  writeFileSync(src, blob);
  const hash = createHash("sha256").update(blob).digest("hex");
  await ipc(b, "open_session", { peer: st.id, conn: "file-transfer" });
  const ft = await page(bBrowser, "conn=file-transfer");
  const ftKey = ft.url().match(/session\/([0-9a-f]{32})/)[1];
  await sleep(1500);
  if (await ft.evaluate(() => document.body.innerText.includes("Password required"))) {
    await ipc(ft, "session_login", { key: ftKey, password: PW, remember: false });
  }
  await until("file session ready", () => ft.evaluate(() => !document.body.innerText.includes("Connecting")), 20000);
  const tf = Date.now();
  await ipc(ft, "fs_transfer", { key: ftKey, id: 9001, path: src, to: join(dstDir, "upload.bin"), fileNum: 0, hidden: false, isRemote: false });
  const dst = join(dstDir, "upload.bin");
  await until("file arrives", () => existsSync(dst) && statSync(dst).size === blob.length, 120000, 200);
  await sleep(300);
  const secs = (Date.now() - tf) / 1000;
  const same = createHash("sha256").update(readFileSync(dst)).digest("hex") === hash;
  record("file transfer 64 MB, SHA-256 verified", same, { seconds: +secs.toFixed(2), MBps: +(64 / secs).toFixed(1) });

  // 7. Security: the session must be end-to-end encrypted.
  const secured = await b.evaluate(() => !!document.querySelector('[title^="End-to-end encrypted"]'));
  record("session reports end-to-end encryption", secured);
}

const outDir = join(ROOT, "tools/bench/results");
const watchdog = setTimeout(() => {
  record("run", false, { error: `watchdog: stuck in step '${currentStep}'` });
  cleanup();
}, 240000);
try {
  await main();
} catch (e) {
  record("run", false, { step: currentStep, error: String(e.stack || e) });
} finally {
  cleanup();
}

function cleanup() {
  clearTimeout(watchdog);
  for (const p of procs) {
    try {
      execFileSync("taskkill", ["/PID", String(p.pid), "/T", "/F"], { stdio: "ignore" });
    } catch {}
  }
  try {
    execFileSync("taskkill", ["/IM", "tether.exe", "/F"], { stdio: "ignore" });
  } catch {}
  mkdirSync(outDir, { recursive: true });
  writeFileSync(join(outDir, "e2e.json"), JSON.stringify(results, null, 2));
  const failed = results.tests.filter((t) => !t.ok).length;
  console.log(`\n${results.tests.length - failed}/${results.tests.length} passed`);
  process.exit(failed ? 1 : 0);
}

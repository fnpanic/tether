// Text drag-select in a WebKit view on a macOS host through Tether, end to end (docs/macos.md
// "Drags: CGEvent and HID"): a Mac build-runner job (tools/macbuild) compiles and shows
// mac-webview-drag.swift, a floating window with a WKWebView text area; this PC's technician app
// (TECH, the installed one by default) connects to the Mac on a hidden desktop and drags across the
// text (press, MOVES steps, release). The job log tells how many characters the drag selected
// and whether a plain move reached the view after the press (what broke the selection).
//
//   node e2e/mac-webview-drag-e2e.mjs
// Env: TECH, MAC_ID, MAC_PW_FILE, OUT, MOVES (20), MOVE_MS (40), START_DELAY (60), RUNS (3 drags).
// TIGHT=1: move, press and first drag steps without pauses (the press right behind a move).
// LOCAL=1: no job; the window is already up on the Mac (./wkdrag 600), only the drags are made.
import { execFile, execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { promisify } from "node:util";
import { chromium } from "playwright-core";

const run = promisify(execFile);
const TECH = process.env.TECH || "C:\\Program Files\\Tether\\tether.exe";
const MAC_ID = process.env.MAC_ID || "128974683";
const PW = readFileSync(process.env.MAC_PW_FILE || "C:/vms/mac.pw", "utf8").split(/\r?\n/)[0].trim();
const OUT = resolve(process.env.OUT || `C:/vms/signtest/macwebdrag-${Date.now()}`);
const MOVES = Number(process.env.MOVES || 20);
const MOVE_MS = Number(process.env.MOVE_MS || 40);
const RUNS = Number(process.env.RUNS || 3);
const LOCAL = process.env.LOCAL === "1";
const TIGHT = process.env.TIGHT === "1";
const START_DELAY = Number(process.env.START_DELAY || 60);
const ROOT = resolve(import.meta.dirname, "../../..");
const HID = join(import.meta.dirname, "hidden-run.ps1");
const MB = join(ROOT, "tools", "macbuild", "macbuild.py");
const PORT = 9600 + Math.floor(Math.random() * 200);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
// The text starts at (0.2, 0.3) of the screen (mac-webview-drag.swift); the end stays inside the
// 680x280 pt text area on any screen from 1280 pt wide.
const [fx0, fy0, fx1, fy1] = [0.2, 0.3, 0.35, 0.35];
const results = [];
const check = (name, ok, detail = "") => {
  results.push({ name, ok: !!ok });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? `  (${detail})` : ""}`);
};
mkdirSync(join(OUT, "job"), { recursive: true });
const mb = async (...args) => (await run("python", [MB, ...args], { maxBuffer: 1 << 26, timeout: 600000 })).stdout.toString();
copyFileSync(join(import.meta.dirname, "mac-webview-drag.swift"), join(OUT, "job", "wkdrag.swift"));
// No status polling while the window is up: macbuild's own Tether session would take the front.
writeFileSync(join(OUT, "job", "run.sh"), `#!/bin/bash
swiftc -O wkdrag.swift -o wkdrag || exit 1
while [ "$(date +%s)" -lt "$START" ]; do sleep 0.2; done
./wkdrag 60
`);

let browser, viewerPid;
try {
  const env = { ...process.env, TETHER_PROFILE: "mac-drag-e2e", MACPW: PW, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${PORT}` };
  viewerPid = execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", HID, "-Exe", TECH, "-Desktop", `tether-macwebdrag-${PORT}`, "-Args", `--connect ${MAC_ID} --password-env MACPW`], { env }).toString().trim();
  let page;
  for (let i = 0; i < 200 && !page; i++) {
    try {
      browser = await chromium.connectOverCDP(`http://127.0.0.1:${PORT}`);
      page = browser.contexts().flatMap((c) => c.pages()).find((p) => p.url().includes("/session/"));
      if (!page) { await browser.close(); browser = null; }
    } catch {}
    if (!page) await sleep(300);
  }
  if (!page) throw new Error("no session page");
  const canvasReady = () => page.evaluate(() => { const c = document.querySelector("canvas"); return !!c && !(c.width === 300 && c.height === 150) && c.getBoundingClientRect().width > 100; });
  for (let i = 0; i < 150 && !(await canvasReady()); i++) await sleep(200);
  if (!(await canvasReady())) throw new Error("no picture");
  check("viewer shows the Mac", true);
  const shot = (name) => page.screenshot({ path: join(OUT, `${name}.png`) }).catch(() => {});
  let id;
  if (!LOCAL) {
    const startAt = Math.floor(Date.now() / 1000) + START_DELAY + 30;
    // The upload (a Tether file transfer) sometimes fails once; START leaves room for a second try.
    const submit = () => mb("submit", "--user", "tether", "--work", join(OUT, "job"), "--timeout", "300", "--env", `START=${startAt}`, "--", "bash", "run.sh");
    id = (await submit().catch(submit)).trim().split(/\s+/).pop();
    check("Mac job submitted", /^\d{8}T\d{6}Z-/.test(id), id);
    while (Date.now() / 1000 < startAt + 8) await sleep(500);
  }
  const r = await page.evaluate(() => { const b = document.querySelector("canvas").getBoundingClientRect(); return { x: b.x, y: b.y, w: b.width, h: b.height }; });
  const at = (fx, fy) => [r.x + r.w * fx, r.y + r.h * fy];
  const [x0, y0] = at(fx0, fy0), [x1, y1] = at(fx1, fy1);
  // A click below the text ends the selection (a press on selected text would drag the text itself)
  // and, before the first drag, makes the window the active one. 50 ms between press and release:
  // the viewer does not keep the order of a press and a release issued in the same instant.
  const deselect = async () => {
    await page.mouse.click(...at(fx0 + 0.05, fy0 + 0.12), { delay: 50 });
    await sleep(1200); // the next press must be a single click
  };
  await deselect();
  for (let n = 1; n <= RUNS; n++) {
    await page.mouse.move(x0 - 20, y0);
    await sleep(200);
    await page.mouse.move(x0, y0);
    if (!TIGHT) await sleep(1200); // longer than the double-click time: every press is a single click
    await page.mouse.down();
    if (!TIGHT) await sleep(300);
    for (let i = 1; i <= MOVES; i++) {
      await page.mouse.move(x0 + ((x1 - x0) * i) / MOVES, y0 + ((y1 - y0) * i) / MOVES);
      await sleep(MOVE_MS);
    }
    await sleep(400);
    if (n === 1) await shot("1-before-release");
    await page.mouse.up();
    await sleep(1000);
    await deselect();
  }
  await shot("2-after");
  if (LOCAL) console.log("INFO  drags made; read the window's output on the Mac");
  else {
    let state = "";
    for (let i = 0; i < 40 && !/"state": "(ok|failed|error|timeout)"/.test(state); i++) {
      await sleep(5000);
      state = await mb("status", "--user", "tether", id).catch(() => state);
    }
    const log = await mb("log", "--user", "tether", id).catch((e) => String(e));
    writeFileSync(join(OUT, "job.log"), log);
    const selected = [...log.matchAll(/JS SELECTED (\d+)/g)].map((m) => +m[1]);
    check(`each of the ${RUNS} drags selected text`, selected.length === RUNS && selected.every((n) => n > 50), `characters: ${selected.join(", ") || "none"}`);
    check("no plain move reached the view after a press", !log.includes("MOVED-AFTER-PRESS"));
  }
} catch (e) {
  check("no unexpected error", false, String(e?.stack ?? e).slice(0, 500));
} finally {
  await browser?.close().catch(() => {});
  if (viewerPid) try { execFileSync("taskkill", ["/PID", viewerPid, "/T", "/F"], { stdio: "ignore" }); } catch {}
}
console.log(`\nfiles: ${OUT}`);
const failed = results.filter((x) => !x.ok);
console.log(`${results.length - failed.length}/${results.length} passed`);
process.exit(failed.length ? 1 : 0);

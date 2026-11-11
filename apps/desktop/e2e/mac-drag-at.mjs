// Drag across the Mac's screen through Tether, between two points given as fractions of the picture.
// The building block for drag checks whose result is read on the Mac (docs/macos.md "Drags"):
// this PC's technician app (TECH) connects to the Mac on a hidden desktop, moves to FROM, presses,
// drags to TO in MOVES steps and releases. Screenshots go to OUT.
//
//   FROM=0.2,0.3 TO=0.46,0.4 node e2e/mac-drag-at.mjs
// Env: TECH, MAC_ID, MAC_PW_FILE, OUT, MOVES (20), MOVE_MS (40).
import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { chromium } from "playwright-core";

const TECH = process.env.TECH || "C:\\Program Files\\Tether\\tether.exe";
const MAC_ID = process.env.MAC_ID || "128974683";
const PW = readFileSync(process.env.MAC_PW_FILE || "C:/vms/mac.pw", "utf8").split(/\r?\n/)[0].trim();
const OUT = resolve(process.env.OUT || `C:/vms/signtest/macdragat-${Date.now()}`);
const MOVES = Number(process.env.MOVES || 20);
const MOVE_MS = Number(process.env.MOVE_MS || 40);
const HID = join(import.meta.dirname, "hidden-run.ps1");
const PORT = 9600 + Math.floor(Math.random() * 200);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const frac = (v) => v.split(",").map(Number);
const [fx0, fy0] = frac(process.env.FROM), [fx1, fy1] = frac(process.env.TO);
mkdirSync(OUT, { recursive: true });

let browser, viewerPid;
try {
  const env = { ...process.env, TETHER_PROFILE: "mac-drag-e2e", MACPW: PW, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${PORT}` };
  viewerPid = execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", HID, "-Exe", TECH, "-Desktop", `tether-macdragat-${PORT}`, "-Args", `--connect ${MAC_ID} --password-env MACPW`], { env }).toString().trim();
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
  await sleep(1500);
  const shot = (name) => page.screenshot({ path: join(OUT, `${name}.png`) }).catch(() => {});
  const r = await page.evaluate(() => { const b = document.querySelector("canvas").getBoundingClientRect(); return { x: b.x, y: b.y, w: b.width, h: b.height }; });
  const at = (fx, fy) => [r.x + r.w * fx, r.y + r.h * fy];
  const [x0, y0] = at(fx0, fy0), [x1, y1] = at(fx1, fy1);
  await page.mouse.move(x0 - 20, y0);
  await sleep(200);
  await page.mouse.move(x0, y0);
  await sleep(300);
  await shot("1-before");
  await page.mouse.down();
  await sleep(300);
  for (let i = 1; i <= MOVES; i++) {
    await page.mouse.move(x0 + ((x1 - x0) * i) / MOVES, y0 + ((y1 - y0) * i) / MOVES);
    await sleep(MOVE_MS);
  }
  await sleep(500);
  await shot("2-before-release");
  await page.mouse.up();
  await sleep(1500);
  await shot("3-after-release");
  console.log(`dragged ${fx0},${fy0} -> ${fx1},${fy1}; files: ${OUT}`);
} catch (e) {
  console.log(`FAIL  ${String(e?.stack ?? e).slice(0, 500)}`);
  process.exitCode = 1;
} finally {
  await browser?.close().catch(() => {});
  if (viewerPid) try { execFileSync("taskkill", ["/PID", viewerPid, "/T", "/F"], { stdio: "ignore" }); } catch {}
}

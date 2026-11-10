// Drag-select on a macOS host through Tether, end to end (CHANGELOG "macOS host: a remote drag"):
// this PC's technician app (TECH, the installed one by default) controls the build Mac on a hidden
// desktop; a Mac build-runner job (tools/macbuild) starts the screenshot selection
// (`screencapture -i`) and waits; the test drags a rectangle across the session canvas (mouse
// down, MOVES small steps, mouse up). The job reports whether macOS took a region and how big it
// is (a click without a drag selects nothing or the whole window under it). Viewer screenshots
// before, during and after the drag and the captured region go to OUT for review.
//
//   node e2e/mac-drag-e2e.mjs
// Env: TECH, MAC_ID (default the build Mac), MAC_PW_FILE (first line), OUT, MOVES (default 30),
// OVER_TETHER=1 (Tether's window open on the Mac, the drag starts on it), TOOLBAR=1 (the Screenshot
// app's interface, Cmd+Shift+5: region, then Enter through the session), FROM/TO, START_DELAY.
import { execFile, execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { promisify } from "node:util";
import { chromium } from "playwright-core";

const run = promisify(execFile);
const TECH = process.env.TECH || "C:\\Program Files\\Tether\\tether.exe";
const MAC_ID = process.env.MAC_ID || "128974683";
const PW = readFileSync(process.env.MAC_PW_FILE || "C:/vms/mac.pw", "utf8").split(/\r?\n/)[0].trim();
const OUT = resolve(process.env.OUT || `C:/vms/signtest/macdrag-${Date.now()}`);
const MOVES = Number(process.env.MOVES || 30);
const TOOLBAR = process.env.TOOLBAR === "1";
const MOVE_MS = Number(process.env.MOVE_MS || 50);
const ROOT = resolve(import.meta.dirname, "../../..");
const HID = join(import.meta.dirname, "hidden-run.ps1");
const MB = join(ROOT, "tools", "macbuild", "macbuild.py");
const PORT = 9600 + Math.floor(Math.random() * 200);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const results = [];
const check = (name, ok, detail = "") => {
  results.push({ name, ok: !!ok });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? `  (${detail})` : ""}`);
};
mkdirSync(join(OUT, "job"), { recursive: true });
const mb = async (...args) => (await run("python", [MB, ...args], { maxBuffer: 1 << 26, timeout: 600000 })).stdout.toString();

// The Mac side: selection mode from START (unix seconds) for up to 90 s, then report. No status
// polling meanwhile: macbuild talks to the Mac through a second Tether session (file transfer),
// whose connection brings Tether to the front there and ends the selection mode.
writeFileSync(join(OUT, "job", "dragcheck.sh"), `#!/bin/bash
mkdir -p "$JOB_WORK/out"; f="$JOB_WORK/out/selection.png"; rm -f "$f"
# OVER_TETHER=1: Tether's main window open (the drag starts on it), as when the user has it open.
if [ "$OVER_TETHER" = 1 ]; then open -a /Applications/Tether.app; fi
while [ "$(date +%s)" -lt "$START" ]; do sleep 0.2; done
# TOOLBAR=1: the Screenshot app's interface (Cmd+Shift+5: toolbar, Capture/Enter takes the region).
if [ "$TOOLBAR" = 1 ]; then screencapture -i -U -x "$f" & sc=$!; else screencapture -i -x "$f" & sc=$!; fi
echo "selection mode from $(date +%T)"
# The windows the guard sees while the selection overlay is up (owner, layer, alpha, bounds).
sleep 2; swift -e 'import CoreGraphics
let l = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as! [[String: Any]]
for w in l.prefix(12) { print("window", w[kCGWindowOwnerName as String] ?? "?", "layer", w[kCGWindowLayer as String] ?? "?", "alpha", w[kCGWindowAlpha as String] ?? "?", w[kCGWindowBounds as String] ?? "") }' 2>&1 | head -12
for i in $(seq 1 90); do kill -0 $sc 2>/dev/null || break; sleep 1; done
kill $sc 2>/dev/null && echo "still waiting after 90 s: no selection was made"
grep -h "Tether window dropped" "$HOME"/Library/Logs/Tether*/*.log 2>/dev/null | tail -3 | sed 's/^/LOG /'
if [ -f "$f" ]; then echo "SELECTION $(sips -g pixelWidth "$f" | awk '/pixelWidth/{print $2}')x$(sips -g pixelHeight "$f" | awk '/pixelHeight/{print $2}') px at $(date +%T)"; else echo "NO SELECTION at $(date +%T)"; fi
`.replace(/\r/g, ""));

let browser, viewerPid;
try {
  // 1. Viewer: connect to the Mac, wait for a picture.
  const env = { ...process.env, TETHER_PROFILE: "mac-drag-e2e", MACPW: PW, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${PORT}` };
  viewerPid = execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", HID, "-Exe", TECH, "-Desktop", `tether-macdrag-${PORT}`, "-Args", `--connect ${MAC_ID} --password-env MACPW`], { env }).toString().trim();
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
  check("viewer shows the Mac", await canvasReady());
  const shot = async (name) => page.screenshot({ path: join(OUT, `${name}.png`) }).catch(() => {});
  // Brightest grey in a small box (fractions of the canvas) at full canvas resolution: the white
  // handles of a selection rectangle sit at its corners and edge midpoints.
  const brightest = (fx, fy, r = 0.006) => page.evaluate(([x, y, rr]) => {
    const src = document.querySelector("canvas"), o = document.createElement("canvas");
    o.width = src.width; o.height = src.height;
    const g = o.getContext("2d");
    g.drawImage(src, 0, 0);
    const w = Math.max(2, Math.round(rr * 2 * src.width)), h = Math.max(2, Math.round(rr * 2 * src.height));
    const px = g.getImageData(Math.round((x - rr) * src.width), Math.round((y - rr) * src.height), w, h).data;
    let m = 0;
    for (let i = 0; i < px.length; i += 4) m = Math.max(m, (px[i] + px[i + 1] + px[i + 2]) / 3);
    return m;
  }, [fx, fy, r]);
  await shot("1-connected");

  // 2. The Mac job: selection mode.
  // The runner picks a job up within seconds of the upload; the selection starts START_DELAY s
  // after the upload finished, so the file-transfer session is long gone by then.
  const START_DELAY = Number(process.env.START_DELAY || 60);
  const startAt = Math.floor(Date.now() / 1000) + START_DELAY + 30;
  const id = (await mb("submit", "--user", "tether", "--work", join(OUT, "job"), "--timeout", "300", "--env", `START=${startAt}`, "--env", `OVER_TETHER=${process.env.OVER_TETHER || 0}`, "--env", `TOOLBAR=${TOOLBAR ? 1 : 0}`, "--artifact", "out/*", "--", "bash", "dragcheck.sh")).trim().split(/\s+/).pop();
  check("Mac job submitted", /^\d{8}T\d{6}Z-/.test(id), id);
  await shot("2a-before-selection-mode");
  while (Date.now() / 1000 < startAt + 4) await sleep(500);
  let state = "";
  await shot("2-selection-mode");

  // 3. Drag across the canvas: 20 % -> 55 % of the picture in MOVES steps.
  const r = await page.evaluate(() => { const b = document.querySelector("canvas").getBoundingClientRect(); return { x: b.x, y: b.y, w: b.width, h: b.height }; });
  const at = (fx, fy) => [r.x + r.w * fx, r.y + r.h * fy];
  // FROM/TO: fractions of the picture ("0.2,0.25"); OVER_TETHER starts in the middle, on Tether's window.
  const frac = (v, d) => (v || d).split(",").map(Number);
  const [fx0, fy0] = frac(process.env.FROM, process.env.OVER_TETHER === "1" ? "0.45,0.45" : "0.2,0.25");
  const [fx1, fy1] = frac(process.env.TO, process.env.OVER_TETHER === "1" ? "0.8,0.8" : "0.55,0.6");
  const [x0, y0] = at(fx0, fy0), [x1, y1] = at(fx1, fy1);
  const handleBefore = await brightest(fx1, fy1);
  await page.mouse.move(x0, y0);
  await sleep(300);
  await page.mouse.down();
  await sleep(300);
  for (let i = 1; i <= MOVES; i++) {
    await page.mouse.move(x0 + ((x1 - x0) * i) / MOVES, y0 + ((y1 - y0) * i) / MOVES);
    await sleep(MOVE_MS);
    if (i === Math.round(MOVES * 0.8)) await shot("3-during-drag");
  }
  await sleep(600);
  await shot("4-before-release");
  await page.mouse.up();
  await sleep(2500);
  await shot("5-after-release");
  // Screenshot-app mode: the selection stays until Capture, drawn as a dashed rectangle with white
  // handles; one sits exactly where the drag ended (a selection that stopped early has none there).
  // A white handle proves something only where the screen was dark: on a bright end point the
  // picture decides (4-before-release.png: the size label next to the drag's end).
  const handleAt = await brightest(fx1, fy1), handleMid = await brightest((fx0 + fx1) / 2, fy1);
  if (TOOLBAR && handleBefore >= 150) console.log(`INFO  bright screen at the drag's end (${handleBefore.toFixed(0)}): not decidable from pixels, see ${join(OUT, "4-before-release.png")}`);
  else if (TOOLBAR) check("the selection reaches the drag's end (white handles at its corner and edge)", handleAt > 180 && handleMid > 180, `end ${handleBefore.toFixed(0)} -> ${handleAt.toFixed(0)}, edge middle ${handleMid.toFixed(0)}`);
  if (TOOLBAR) {
    // The toolbar keeps the region until Capture: Enter, typed through the session.
    await page.keyboard.press("Enter");
    await sleep(2500);
    await shot("6-after-enter");
  }

  // 4. What macOS made of it.
  for (let i = 0; i < 60 && !/"state": "(ok|failed|error|timeout)"/.test(state); i++) {
    await sleep(5000);
    state = await mb("status", "--user", "tether", id).catch(() => state);
  }
  const log = await mb("log", "--user", "tether", id).catch((e) => String(e));
  writeFileSync(join(OUT, "job.log"), log);
  const m = log.match(/SELECTION (\d+)x(\d+) px/);
  // Screenshot-app mode: Capture by Enter through the session; reported, the rectangle above decides.
  if (TOOLBAR) console.log(`INFO  Enter through the session captured: ${m ? `${m[1]}x${m[2]} px` : "no"}`);
  else check("macOS took a dragged region (not a click, not nothing)", m && +m[1] > 200 && +m[2] > 200, log.trim().split("\n").slice(-2).join(" | "));
  if (m) await mb("fetch", "--user", "tether", id, "selection.png", "--out", join(OUT, "selection.png")).catch(() => {});
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

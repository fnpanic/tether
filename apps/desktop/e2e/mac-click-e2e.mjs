// Ordinary clicks on a macOS host through Tether, end to end (regression check for the HID drag
// path in libs/enigo/src/macos/hid_post.rs): a Mac build-runner job opens a TextEdit document with
// large words at START; this PC's technician app (hidden desktop) clicks the window active,
// double-clicks "charlie" (the word must turn into a selection) and drags across "delta echo
// foxtrot" (the text must be selected). Selection = TextEdit's light-blue highlight, counted in
// the session canvas. Pictures go to OUT.
//
//   node e2e/mac-click-e2e.mjs
// Env: TECH, MAC_ID, MAC_PW_FILE, OUT. The word positions are those of TextEdit's restored window
// on the build Mac (top left, 72 pt Helvetica); check 1-opened.png if the window moved.
import { execFile, execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { promisify } from "node:util";
import { chromium } from "playwright-core";

const run = promisify(execFile);
const TECH = process.env.TECH || "C:\\Program Files\\Tether\\tether.exe";
const MAC_ID = process.env.MAC_ID || "128974683";
const PW = readFileSync(process.env.MAC_PW_FILE || "C:/vms/mac.pw", "utf8").split(/\r?\n/)[0].trim();
const OUT = resolve(process.env.OUT || `C:/vms/signtest/macclick-${Date.now()}`);
const ROOT = resolve(import.meta.dirname, "../../..");
const HID = join(import.meta.dirname, "hidden-run.ps1");
const MB = join(ROOT, "tools", "macbuild", "macbuild.py");
const PORT = 9700 + Math.floor(Math.random() * 200);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const results = [];
const check = (name, ok, detail = "") => {
  results.push({ name, ok: !!ok });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? `  (${detail})` : ""}`);
};
mkdirSync(join(OUT, "job"), { recursive: true });
const mb = async (...args) => (await run("python", [MB, ...args], { maxBuffer: 1 << 26, timeout: 600000 })).stdout.toString();

// Fractions of the Mac's picture (canvas): TextEdit's window at the top left, 72 pt words.
const ACTIVATE = [0.287, 0.65], CHARLIE = [0.171, 0.278], ROW3 = [[0.11, 0.355], [0.43, 0.355]];
const CHARLIE_BOX = [0.11, 0.25, 0.23, 0.30], ECHO_BOX = [0.215, 0.33, 0.29, 0.38];

writeFileSync(join(OUT, "job", "words.rtf"), "{\\rtf1\\ansi{\\fonttbl\\f0 Helvetica;}\\f0\\fs144 alpha bravo charlie\\par delta echo foxtrot\\par}\n");
writeFileSync(join(OUT, "job", "textedit.sh"), `#!/bin/bash
while [ "$(date +%s)" -lt "$START" ]; do sleep 0.2; done
killall TextEdit 2>/dev/null; sleep 1
f="/tmp/tether-click-$(date +%s).rtf"; cp words.rtf "$f"
open -a TextEdit "$f"; echo "opened $(date +%T)"
sleep 60
killall TextEdit 2>/dev/null; rm -f "$f"; echo "closed $(date +%T)"
`);

let browser, viewerPid;
try {
  const env = { ...process.env, TETHER_PROFILE: "mac-drag-e2e", MACPW: PW, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${PORT}` };
  viewerPid = execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", HID, "-Exe", TECH, "-Desktop", `tether-macclick-${PORT}`, "-Args", `--connect ${MAC_ID} --password-env MACPW`], { env }).toString().trim();
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
  for (let i = 0; i < 150; i++) {
    if (await page.evaluate(() => { const c = document.querySelector("canvas"); return !!c && c.width > 300 && c.getBoundingClientRect().width > 100; })) break;
    await sleep(200);
  }
  const shot = (name) => page.screenshot({ path: join(OUT, `${name}.png`) }).catch(() => {});
  const r = await page.evaluate(() => { const b = document.querySelector("canvas").getBoundingClientRect(); return { x: b.x, y: b.y, w: b.width, h: b.height }; });
  const at = ([fx, fy]) => [r.x + r.w * fx, r.y + r.h * fy];
  // Share of TextEdit's light-blue selection pixels in a box (fractions of the canvas).
  const blue = ([x0, y0, x1, y1]) => page.evaluate(([a, b, c, d]) => {
    const src = document.querySelector("canvas"), o = document.createElement("canvas");
    o.width = src.width; o.height = src.height;
    const g = o.getContext("2d");
    g.drawImage(src, 0, 0);
    const px = g.getImageData(Math.round(a * src.width), Math.round(b * src.height), Math.round((c - a) * src.width), Math.round((d - b) * src.height)).data;
    let n = 0;
    for (let i = 0; i < px.length; i += 4) if (px[i + 2] > 200 && px[i + 2] - px[i] > 40 && px[i + 1] > 170) n++;
    return n / (px.length / 4);
  }, [x0, y0, x1, y1]);

  const startAt = Math.floor(Date.now() / 1000) + 90;
  const id = (await mb("submit", "--user", "tether", "--work", join(OUT, "job"), "--timeout", "240", "--env", `START=${startAt}`, "--", "bash", "textedit.sh")).trim().split(/\s+/).pop();
  check("Mac job submitted", /^\d{8}T\d{6}Z-/.test(id), id);
  while (Date.now() / 1000 < startAt + 6) await sleep(500);
  await shot("1-opened");
  const before = { charlie: await blue(CHARLIE_BOX), echo: await blue(ECHO_BOX) };

  await page.mouse.click(...at(ACTIVATE));
  await sleep(800);
  await page.mouse.dblclick(...at(CHARLIE));
  await sleep(1500);
  await shot("2-double-click");
  const charlie = await blue(CHARLIE_BOX);
  check("double-click selects the word (click count arrives)", charlie > 0.3 && before.charlie < 0.05, `blue share ${before.charlie.toFixed(2)} -> ${charlie.toFixed(2)}`);

  const [a, b] = ROW3.map(at);
  await page.mouse.move(...a);
  await page.mouse.down();
  for (let i = 1; i <= 15; i++) { await page.mouse.move(a[0] + ((b[0] - a[0]) * i) / 15, a[1]); await sleep(50); }
  await page.mouse.up();
  await sleep(1500);
  await shot("3-drag-select");
  const echo = await blue(ECHO_BOX), charlieAfter = await blue(CHARLIE_BOX);
  check("drag selects the text (and moves the selection off the first word)", echo > 0.3 && charlieAfter < 0.05, `echo ${before.echo.toFixed(2)} -> ${echo.toFixed(2)}, charlie ${charlieAfter.toFixed(2)}`);
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

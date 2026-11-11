// What the Mac's picture looks like in the viewer: this PC's technician app (TECH) connects to the
// Mac on a hidden desktop, prints the session statistics, the canvas size and the colour at each
// PROBE point (fractions of the picture, 5x5 mean), and saves the decoded picture 1:1 to OUT.
// With a window of known flat colours on the Mac this shows colour and sharpness loss end to end
// (docs/performance.md "Mac host: picture").
//
//   PROBE="0.2,0.31;0.26,0.31" node e2e/mac-picture-probe.mjs
// Env: TECH, MAC_ID, MAC_PW_FILE, OUT, SETTLE_MS (4000), CODEC (auto|vp9|av1|h264|h265).
import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { chromium } from "playwright-core";

const TECH = process.env.TECH || "C:\\Program Files\\Tether\\tether.exe";
const MAC_ID = process.env.MAC_ID || "128974683";
const PW = readFileSync(process.env.MAC_PW_FILE || "C:/vms/mac.pw", "utf8").split(/\r?\n/)[0].trim();
const OUT = resolve(process.env.OUT || `C:/vms/signtest/macprobe-${Date.now()}`);
const HID = join(import.meta.dirname, "hidden-run.ps1");
const PORT = 9600 + Math.floor(Math.random() * 200);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const points = (process.env.PROBE || "").split(";").filter(Boolean).map((p) => p.split(",").map(Number));
mkdirSync(OUT, { recursive: true });

let browser, viewerPid;
try {
  const env = { ...process.env, TETHER_PROFILE: "mac-drag-e2e", MACPW: PW, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${PORT}` };
  viewerPid = execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", HID, "-Exe", TECH, "-Desktop", `tether-macprobe-${PORT}`, "-Args", `--connect ${MAC_ID} --password-env MACPW`], { env }).toString().trim();
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
  if (process.env.CODEC) {
    const key = decodeURIComponent(page.url().split("/session/")[1].split(/[?#]/)[0]);
    console.log("codec:", await page.evaluate(([k, c]) => window.__TAURI__?.core?.invoke("session_codec", { key: k, codec: c }).then(() => c, String), [key, process.env.CODEC]));
  }
  await sleep(Number(process.env.SETTLE_MS || 4000));
  await page.evaluate(() => document.querySelector('[aria-label="Statistics"]')?.click());
  await sleep(2500);
  console.log("stats:", await page.evaluate(() => [...document.querySelectorAll("div")].find((d) => d.textContent?.startsWith("fps"))?.innerText.replace(/\n/g, " ") ?? "overlay not found"));
  // The decoded picture 1:1 (the session canvas may be WebGL: draw it into a 2D canvas in the same task as a frame).
  const grab = await page.evaluate((pts) => new Promise((done) => requestAnimationFrame(() => {
    const src = document.querySelector("canvas"), o = document.createElement("canvas");
    o.width = src.width; o.height = src.height;
    const g = o.getContext("2d");
    g.drawImage(src, 0, 0);
    const b = src.getBoundingClientRect();
    const colours = pts.map(([fx, fy]) => {
      const d = g.getImageData(Math.round(fx * o.width) - 2, Math.round(fy * o.height) - 2, 5, 5).data;
      const m = [0, 0, 0];
      for (let i = 0; i < d.length; i += 4) for (let k = 0; k < 3; k++) m[k] += d[i + k] / 25;
      return m.map((v) => Math.round(v));
    });
    done({ w: o.width, h: o.height, css: [Math.round(b.width), Math.round(b.height)], dpr: devicePixelRatio, colours, png: o.toDataURL("image/png") });
  })), points);
  writeFileSync(join(OUT, "canvas.png"), Buffer.from(grab.png.split(",")[1], "base64"));
  console.log(`canvas ${grab.w}x${grab.h}, shown at ${grab.css.join("x")} CSS px, dpr ${grab.dpr}`);
  points.forEach((p, i) => console.log(`colour at ${p.join(",")}: ${grab.colours[i].join(",")}`));
  console.log(`files: ${OUT}`);
} catch (e) {
  console.log(`FAIL  ${String(e?.stack ?? e).slice(0, 500)}`);
  process.exitCode = 1;
} finally {
  await browser?.close().catch(() => {});
  if (viewerPid) try { execFileSync("taskkill", ["/PID", viewerPid, "/T", "/F"], { stdio: "ignore" }); } catch {}
}

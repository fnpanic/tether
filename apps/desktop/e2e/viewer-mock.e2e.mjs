// Session window in mock mode (no Tauri, no network): the toolbar reveal and mouse drags, in
// Chromium and WebKit (the engine of the macOS app). Starts its own Vite dev server.
//
//   node e2e/viewer-mock.e2e.mjs                 both engines
//   node e2e/viewer-mock.e2e.mjs webkit          one engine
//
// Browsers: `npx playwright-core install chromium-headless-shell webkit` (PLAYWRIGHT_BROWSERS_PATH
// may point to a cache dir). Mouse input is synthesized inside the headless browser only.
import { spawn } from "node:child_process";
import { resolve } from "node:path";
import { chromium, webkit } from "playwright-core";

const ROOT = resolve(import.meta.dirname, "..");
const PORT = 14620 + Math.floor(Math.random() * 200);
const engines = process.argv[2] ? [process.argv[2]] : ["chromium", "webkit"];
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const DISPLAY = { width: 2560, height: 1440 }; // the mock's first display
let failures = 0;
const check = (what, ok, detail = "") => {
  console.log(`${ok ? "PASS" : "FAIL"} ${what}${detail ? `  (${detail})` : ""}`);
  if (!ok) failures++;
};
async function until(fn, timeout = 10000, every = 50) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    if (await fn().catch(() => false)) return true;
    await sleep(every);
  }
  return false;
}

const vite = spawn(process.execPath, [resolve(ROOT, "node_modules/vite/bin/vite.js"), "--port", String(PORT), "--strictPort"], { cwd: ROOT, stdio: ["ignore", "pipe", "pipe"] });
const ready = new Promise((ok) => vite.stdout.on("data", (d) => String(d).includes("Local") && ok()));

async function run(name) {
  const browser = await { chromium, webkit }[name].launch();
  const p = await browser.newPage({ viewport: { width: 1280, height: 720 } });
  await p.addInitScript(() => (window.__mockCalls = []));
  await p.goto(`http://localhost:${PORT}/#/session/mock?peer=184392571`, { timeout: 120000 }); // first Vite transform is slow on a busy PC
  const tag = (s) => `[${name}] ${s}`;
  const bar = p.getByRole("button", { name: "Disconnect" });
  const mouse = async () => p.evaluate(() => window.__mockCalls.filter((c) => c.cmd === "session_mouse").map((c) => c.args));
  const clear = () => p.evaluate(() => (window.__mockCalls.length = 0));
  /** Where a page point lands on the remote display (same formula as Session.tsx toRemote). */
  const remote = (x, y) =>
    p.evaluate(([x, y, d]) => {
      const r = document.querySelector("canvas").getBoundingClientRect();
      return { x: Math.round(((x - r.left) / r.width) * d.width), y: Math.round(((y - r.top) / r.height) * d.height) };
    }, [x, y, DISPLAY]);

  check(tag("picture shown"), await until(() => p.evaluate(() => getComputedStyle(document.querySelector("canvas")).opacity === "1")));
  const canvas = await p.locator("canvas").boundingBox();
  // Where the bar sits while it is shown (it starts shown): the top-edge click goes under it.
  await bar.waitFor({ timeout: 3000 }).catch(() => {});
  const barBox = await p.locator("[data-testid=toolbar], .glass").first().boundingBox();
  // Start state: the toolbar shows, then hides by itself once the pointer is elsewhere.
  await p.mouse.move(640, 400);
  check(tag("toolbar hides by itself"), await until(async () => !(await bar.isVisible()), 6000));

  // ---- Bug: the toolbar popped up whenever the pointer reached the top edge.
  // Under where the bar would be (its left part), well beside the handle at the centre.
  const top = { x: Math.round(barBox.x + 30), y: Math.ceil(canvas.y) + Math.round(barBox.y + barBox.height / 2) };
  await p.mouse.move(top.x, top.y + 30, { steps: 3 });
  await p.mouse.move(top.x, top.y, { steps: 3 });
  await sleep(900);
  check(tag("top edge outside the handle: toolbar stays hidden"), !(await bar.isVisible()));
  await clear();
  await p.mouse.down();
  await p.mouse.up();
  const want = await remote(top.x, top.y);
  const got = await mouse();
  const down = got.find((m) => m.mask === 9);
  check(tag("click at the top edge reaches the remote side"), !!down && down.x === want.x && down.y === want.y && got.some((m) => m.mask === 10), JSON.stringify({ want, got: got.map((m) => [m.mask, m.x, m.y]) }));

  // ---- Handle: hover (short delay) reveals; leaving the toolbar hides it again.
  const handle = p.getByRole("button", { name: "Show toolbar" });
  check(tag("handle shown while the toolbar is hidden"), await handle.isVisible().catch(() => false));
  if (await handle.isVisible().catch(() => false)) {
    const hb = await handle.boundingBox();
    check(tag("handle is small (<= 120 x 24 px)"), hb.width <= 120 && hb.height <= 24, `${hb.width}x${hb.height}`);
    await p.mouse.move(hb.x + hb.width / 2, hb.y + hb.height / 2, { steps: 2 });
    await sleep(100);
    check(tag("no reveal before the hover delay"), !(await bar.isVisible()));
    check(tag("hovering the handle reveals the toolbar"), await until(() => bar.isVisible(), 1500));
    const bb = await p.locator("[data-testid=toolbar]").boundingBox();
    await p.mouse.move(bb.x + 40, bb.y + bb.height / 2, { steps: 4 }); // onto the bar
    await sleep(1500);
    check(tag("stays while the pointer is on the toolbar"), await bar.isVisible());
    await p.mouse.move(640, 450, { steps: 4 });
    check(tag("hides after the pointer left the toolbar"), await until(async () => !(await bar.isVisible()), 2500));

    // Click on the handle reveals right away.
    await handle.click();
    check(tag("click on the handle reveals the toolbar"), await until(() => bar.isVisible(), 300));
    await p.mouse.move(640, 450, { steps: 4 });
    await until(async () => !(await bar.isVisible()), 2500);

    // A drag that ends on the handle (button held) must not reveal it.
    await p.mouse.move(640, 300);
    await p.mouse.down();
    await p.mouse.move(hb.x + hb.width / 2, hb.y + hb.height / 2, { steps: 8 });
    await sleep(800);
    check(tag("drag over the handle (button held): toolbar stays hidden"), !(await bar.isVisible()));
    await p.mouse.up();
    await sleep(700);
    check(tag("button released on the handle: still hidden (needs a fresh hover)"), !(await bar.isVisible()));
    await p.mouse.move(640, 450, { steps: 2 });

    // Pinned: stays up with the pointer anywhere.
    await handle.click();
    await until(() => bar.isVisible(), 1000);
    await p.getByRole("button", { name: "Pin toolbar" }).click();
    await p.mouse.move(640, 450, { steps: 4 });
    await sleep(2800);
    check(tag("pinned toolbar stays visible"), await bar.isVisible());
    await p.getByRole("button", { name: "Unpin toolbar" }).click();
    await p.mouse.move(640, 450, { steps: 4 });
    check(tag("unpinned toolbar hides again"), await until(async () => !(await bar.isVisible()), 3500));
  }

  // ---- Drags on the picture (remote selection rectangles: move, resize), also up to the top edge.
  const drag = async (from, to, pause = 20) => {
    await p.mouse.move(from.x, from.y);
    await clear();
    await p.mouse.down();
    // A hand-speed drag: moves are coalesced to one per frame.
    for (let i = 1; i <= 12; i++) {
      await p.mouse.move(from.x + ((to.x - from.x) * i) / 12, from.y + ((to.y - from.y) * i) / 12);
      if (pause) await sleep(pause);
    }
    await p.mouse.up();
    await sleep(150);
    const ev = await mouse();
    const iDown = ev.findIndex((m) => m.mask === 9);
    const iUp = ev.findIndex((m) => m.mask === 10);
    const moves = ev.slice(iDown + 1, iUp).filter((m) => m.mask === 0);
    const w0 = await remote(from.x, from.y);
    const w1 = await remote(to.x, to.y);
    const last = moves.at(-1);
    const afterUp = ev.slice(iUp + 1).filter((m) => m.mask === 0).length;
    return {
      ok: afterUp === 0 && iDown >= 0 && iUp > iDown && moves.length >= (pause ? 3 : 1) && ev[iDown].x === w0.x && ev[iDown].y === w0.y && last && last.x === w1.x && last.y === w1.y && ev[iUp].x === w1.x && ev[iUp].y === w1.y,
      detail: JSON.stringify({ w0, w1, down: [ev[iDown]?.x, ev[iDown]?.y], up: [ev[iUp]?.x, ev[iUp]?.y], moves: moves.length, last: [last?.x, last?.y], movesAfterUp: afterUp }),
    };
  };
  let r = await drag({ x: 400, y: 300 }, { x: 700, y: 500 });
  check(tag("drag (create selection): down, moves, up at the right remote points"), r.ok, r.detail);
  r = await drag({ x: 550, y: 400 }, { x: 600, y: 250 });
  check(tag("drag (move selection)"), r.ok, r.detail);
  r = await drag({ x: 700, y: 500 }, { x: 900, y: 600 });
  check(tag("drag (resize handle)"), r.ok, r.detail);
  r = await drag({ x: 500, y: 500 }, { x: 560, y: 380 }, 0);
  check(tag("fast flick (release within the same frame): last position goes out before the release"), r.ok, r.detail);
  r = await drag({ x: 640, y: 300 }, { x: 640, y: Math.ceil(canvas.y) + 2 });
  check(tag("drag up to the top edge: moves and release reach the remote side"), r.ok, r.detail);
  check(tag("toolbar did not cover the drag"), !(await bar.isVisible()));
  // The selection now sits at the top: grab it there again and move it on (this second press
  // landed on the popped-up toolbar before).
  await sleep(600);
  r = await drag({ x: 600, y: Math.ceil(canvas.y) + 20 }, { x: 800, y: 300 });
  check(tag("second drag starting at the top edge reaches the remote side"), r.ok, r.detail);

  // ---- The agent menu item follows the team's agent permission (the host cannot check it).
  for (const [perm, allowed] of [["sbom", false], ["sbom,agent", true]]) {
    await p.goto(`http://localhost:${PORT}/?perm=${perm}#/session/mock?peer=184392571`);
    await until(() => p.evaluate(() => getComputedStyle(document.querySelector("canvas")).opacity === "1"));
    await p.getByRole("button", { name: "Remote actions" }).click({ timeout: 5000 }).catch(async () => {
      await p.getByRole("button", { name: "Show toolbar" }).hover();
      await p.getByRole("button", { name: "Remote actions" }).click();
    });
    const item = p.getByRole("menu").getByRole("button", { name: /^Health monitoring agent/ });
    await until(async () => (await item.isDisabled()) === !allowed, 3000);
    const hint = await item.innerText();
    check(tag(`agent menu item with permissions ${perm}: ${allowed ? "enabled" : "disabled with the reason"}`), (await item.isDisabled()) === !allowed && hint.includes("does not allow installing or removing") === !allowed, hint);
  }
  await browser.close();
}

try {
  await Promise.race([ready, sleep(30000).then(() => Promise.reject(new Error("vite did not start")))]);
  for (const e of engines) await run(e);
} finally {
  vite.kill();
}
console.log(failures ? `\n${failures} FAILED` : "\nall passed");
process.exit(failures ? 1 : 0);

// Screenshots of Tether Manage's release sheets at small window heights, in browser mock mode.
//   (cd apps/manage && npx vite --port 5200 --strictPort) ; node e2e/manage-shots.mjs <out dir>
// Checks that every open dialog fits the viewport with its title and buttons visible.
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { chromium } from "playwright-core";

const OUT = process.argv[2] || "manage-shots";
const URL = process.env.URL || "http://localhost:5200/?many";
mkdirSync(OUT, { recursive: true });
const SIZES = [
  ["1280x720", 1280, 720 - 80, 1], // the window inside a 720 px screen (title bar, taskbar)
  ["1366x768@125", 1093, 614 - 64, 1.25],
  ["1366x768@150", 911, 512 - 53, 1.5],
  ["min-900x600", 900, 600, 1], // tauri.conf.json minWidth/minHeight
];
const results = [];
const check = (name, ok, detail = "") => {
  results.push(ok);
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? `  (${detail})` : ""}`);
};

/** The open dialog fits; its first and last buttons and the heading are inside the viewport. */
async function fits(p, what, size) {
  const r = await p.evaluate(() => {
    const d = document.querySelector('[role="dialog"]');
    if (!d) return null;
    const box = d.getBoundingClientRect();
    const btns = [...d.querySelectorAll("button")].filter((b) => b.offsetParent);
    const last = btns[btns.length - 1]?.getBoundingClientRect();
    const h = d.querySelector("h2")?.getBoundingClientRect();
    return { top: box.top, bottom: box.bottom, vh: innerHeight, last: last && { top: last.top, bottom: last.bottom, text: btns[btns.length - 1].textContent }, h: h && { top: h.top, bottom: h.bottom } };
  });
  const ok = r && r.top >= 0 && r.bottom <= r.vh && r.last && r.last.top >= 0 && r.last.bottom <= r.vh && (!r.h || (r.h.top >= 0 && r.h.bottom <= r.vh));
  check(`${size}: ${what} fits, title and "${r?.last?.text}" visible`, ok, r && `dialog ${Math.round(r.top)}-${Math.round(r.bottom)} of ${r.vh}`);
}

const browser = await chromium.launch({ channel: "msedge" });
try {
  for (const [name, w, h, dpr] of SIZES) {
    const ctx = await browser.newContext({ viewport: { width: w, height: h }, deviceScaleFactor: dpr });
    const p = await ctx.newPage();
    await p.goto(URL);
    await p.fill("input[type=password]", "x");
    await p.keyboard.press("Enter");
    await p.getByRole("button", { name: "Releases" }).click();
    await p.getByRole("button", { name: /^Publish \d+ new build/ }).click();
    const custom = p.getByRole("dialog").getByRole("radio", { name: "Custom", exact: true });
    if (await custom.count()) await custom.click(); // not in builds before the custom ramp
    await p.waitForTimeout(300);
    await fits(p, "Publish the latest builds (custom ramp)", name);
    await p.screenshot({ path: join(OUT, `${name}-publish-latest.png`) });
    await p.evaluate(() => document.querySelector('[role="dialog"] .overflow-auto')?.scrollTo(0, 1e6));
    await p.screenshot({ path: join(OUT, `${name}-publish-latest-scrolled.png`) });
    await p.getByRole("button", { name: "Cancel" }).click();
    await p.screenshot({ path: join(OUT, `${name}-releases.png`), fullPage: false });
    await p.getByRole("button", { name: "Clean up old releases…" }).click();
    await p.waitForTimeout(300);
    await fits(p, "Clean up old releases", name);
    await p.screenshot({ path: join(OUT, `${name}-cleanup.png`) });
    await p.keyboard.press("Escape");
    await p.getByRole("button", { name: "Custom apps" }).click();
    const add = p.getByRole("button", { name: /New custom app/ });
    if (await add.count()) {
      await add.first().click();
      await p.waitForTimeout(300);
      await fits(p, "New custom app", name);
      await p.screenshot({ path: join(OUT, `${name}-custom-app.png`) });
      await p.keyboard.press("Escape");
    }
    await ctx.close();
  }
} finally {
  await browser.close();
}
const failed = results.filter((x) => !x).length;
console.log(`\n${results.length - failed}/${results.length} checks passed`);
process.exitCode = failed ? 1 : 0;

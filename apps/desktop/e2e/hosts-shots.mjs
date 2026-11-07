// Screenshots of Advanced mode for docs/advanced/*.png (mock mode, 5,000 hosts, light and dark).
//   node e2e/hosts-shots.mjs
import { spawn } from "node:child_process";
import { resolve } from "node:path";
import { chromium } from "playwright-core";

const ROOT = resolve(import.meta.dirname, "..");
const OUT = resolve(ROOT, "../../docs/advanced");
const PORT = 14980;
const vite = spawn(process.execPath, [resolve(ROOT, "node_modules/vite/bin/vite.js"), "--port", String(PORT), "--strictPort"], { cwd: ROOT, stdio: ["ignore", "pipe", "pipe"] });
await new Promise((ok) => vite.stdout.on("data", (d) => String(d).includes("Local") && ok()));
const b = await chromium.launch();
try {
  for (const theme of ["light", "dark"]) {
    const p = await b.newPage({ viewport: { width: 1440, height: 860 }, deviceScaleFactor: 1 });
    await p.addInitScript((t) => localStorage.setItem("tether.theme", t), theme);
    await p.addInitScript(() => localStorage.setItem("tether.mode", "advanced"));
    await p.goto(`http://localhost:${PORT}/?hosts=5000#/`, { timeout: 120000 });
    await p.waitForSelector("[role=row][data-id]", { timeout: 60000 });
    await p.waitForTimeout(1200);
    await p.getByRole("button", { name: /^site\s/ }).click();
    await p.getByRole("button", { name: "Expand site" }).click();
    await p.getByRole("button", { name: "Expand customer" }).click();
    await p.getByRole("textbox", { name: "Search hosts" }).fill("#customer:acme -#env:test is:online");
    await p.waitForTimeout(300);
    await p.locator("[role=row][data-id]").nth(2).click();
    await p.locator("[role=row][data-id]").nth(4).click({ modifiers: ["Shift"] });
    await p.waitForTimeout(200);
    await p.screenshot({ path: `${OUT}/hosts-${theme}.png` });
    await p.keyboard.press("Control+k");
    await p.getByRole("combobox", { name: "Command" }).fill("ber srv");
    await p.waitForTimeout(200);
    await p.screenshot({ path: `${OUT}/palette-${theme}.png` });
    await p.keyboard.press("Escape");
    await p.keyboard.press("t");
    await p.waitForTimeout(300);
    await p.screenshot({ path: `${OUT}/bulk-tags-${theme}.png` });
    await p.close();
  }
} finally {
  await b.close();
  vite.kill();
}
console.log("written to", OUT);

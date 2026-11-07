// quality-view.mjs: drive a Tether viewer over CDP for the picture-quality bench (docs/performance.md).
//   node quality-view.mjs <port> original          show the remote picture 1:1 (canvas = stream size)
//   node quality-view.mjs <port> grab <out.png>    save the session canvas as PNG, print the stats panel
//   node quality-view.mjs <port> stats             print the stats panel (opens it)
import { createRequire } from "module";
import fs from "node:fs";
const require = createRequire(new URL("../../apps/desktop/package.json", import.meta.url));
const { chromium } = require("playwright-core");
const [port, cmd, out] = process.argv.slice(2);
const b = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
const p = b.contexts().flatMap((c) => c.pages()).find((x) => x.url().includes("/session/"));
if (!p) throw new Error("no session page");
const stats = () => p.evaluate(async () => {
  if (!document.querySelector(".font-mono.text-footnote")) document.querySelector('[aria-label="Statistics"]')?.click();
  await new Promise((r) => setTimeout(r, 1200));
  return document.querySelector(".font-mono.text-footnote")?.innerText.replace(/\n/g, " | ") ?? "";
});
if (cmd === "original") {
  console.log(await p.evaluate(() => { const e = document.querySelector('[aria-label="Original size"]'); e?.click(); return e ? "original" : "already original"; }));
} else if (cmd === "grab") {
  const r = await p.evaluate(() => {
    const cv = [...document.querySelectorAll("canvas")].sort((a, c) => c.width * c.height - a.width * a.height)[0];
    const o = document.createElement("canvas"); o.width = cv.width; o.height = cv.height;
    o.getContext("2d").drawImage(cv, 0, 0);
    return { w: cv.width, h: cv.height, url: o.toDataURL("image/png") };
  });
  fs.writeFileSync(out, Buffer.from(r.url.split(",")[1], "base64"));
  console.log(JSON.stringify({ w: r.w, h: r.h, stats: await stats() }));
} else if (cmd === "stats") {
  console.log(await stats());
}
await b.close();

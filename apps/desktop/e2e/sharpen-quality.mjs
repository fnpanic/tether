// Picture quality of the viewer's sharpening (src/lib/sharpen.ts) on a deterministic fixture:
// e2e/sharpen-fixture.html, served by its own Vite dev server, in headless Edge (GPU via ANGLE).
// Writes the metrics (JSON) and zoomed crops (PNG) for Off / WebGL strengths at 1:1, 0.75× and 1.5×.
//   node e2e/sharpen-quality.mjs [--out ../../tools/bench/results/sharpen] [--codec vp09.00.10.08] [--bitrate 3000000] [--frames 12]
// --frames 1: the key frame only (a picture that just changed); 12: a static screen refined by deltas.
import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { chromium } from "playwright-core";

const { values: opt } = parseArgs({
  options: { out: { type: "string" }, codec: { type: "string" }, bitrate: { type: "string" }, frames: { type: "string" }, port: { type: "string" }, "no-crops": { type: "boolean" } },
});
const ROOT = resolve(import.meta.dirname, "..");
const OUT = resolve(opt.out ?? join(ROOT, "../../tools/bench/results/sharpen"));
const PORT = opt.port ?? "1431";
const EDGE = "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe";
mkdirSync(OUT, { recursive: true });

const vite = spawn(process.execPath, [join(ROOT, "node_modules/vite/bin/vite.js"), "--port", PORT, "--strictPort"], { cwd: ROOT, stdio: ["ignore", "pipe", "inherit"] });
await new Promise((ok, fail) => {
  vite.stdout.on("data", (d) => String(d).includes("ready") && ok());
  vite.on("exit", (c) => fail(new Error(`vite exited ${c}`)));
});
let browser;
try {
  browser = await chromium.launch({ executablePath: EDGE, headless: true, args: ["--use-angle=d3d11", "--ignore-gpu-blocklist", "--enable-gpu"] });
  const page = await browser.newPage();
  page.on("console", (m) => m.type() === "error" && console.log("page:", m.text()));
  await page.goto(`http://localhost:${PORT}/e2e/sharpen-fixture.html`);
  await page.waitForFunction(() => window.ready);
  const r = await page.evaluate((a) => window.run(a), { codec: opt.codec, bitrate: opt.bitrate ? +opt.bitrate : undefined, frames: opt.frames ? +opt.frames : undefined, crops: !opt["no-crops"] });
  for (const [name, url] of Object.entries(r.crops)) writeFileSync(join(OUT, `${name}.png`), Buffer.from(url.split(",")[1], "base64"));
  delete r.crops;
  writeFileSync(join(OUT, `quality-${r.codec.split(".")[0]}-${r.bitrate}-f${r.frames}.json`), JSON.stringify(r, null, 1));
  console.log(`renderer: ${r.renderer}; ${r.codec} @ ${r.bitrate / 1e6} Mbps, ${r.bytes} bytes for ${r.frames} frame(s)`);
  console.log("scale path        k     | text: mae sharp over  | lines: mae sharp over | photo: mae sharp | noise: mae sharp");
  for (const c of r.cases) {
    const m = c.m, f = (x) => String(x).padStart(6);
    console.log(
      `${String(c.scale).padEnd(5)} ${c.path.padEnd(9)} ${String(c.k ?? "-").padEnd(5)} |${f(m.text.mae)}${f(m.text.sharp)}${f(m.text.over)} |${f(m.lines.mae)}${f(m.lines.sharp)}${f(m.lines.over)} |${f(m.photo.mae)}${f(m.photo.sharp)} |${f(m.noise.mae)}${f(m.noise.sharp)}` +
        (c.auto != null ? `   (Auto: k=${c.auto})` : ""),
    );
  }
  console.log("draw cost (ms, GPU waited for):", JSON.stringify(r.perf));
} finally {
  await browser?.close();
  vite.kill();
}

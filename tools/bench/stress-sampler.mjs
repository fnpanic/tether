// Viewer-side sampler for tools/bench/stress.html shown on a remote host.
// Reads the session canvas of a Tether viewer over CDP, decodes the host's Date.now() barcode and
// writes one JSON line per sample: {t, v, lat, phase, state}. lat = viewer clock - host clock at the
// frame the viewer shows (includes the clock offset between the machines; compare phases, not absolutes).
//   node stress-sampler.mjs --port 9271 --seconds 3600 --every 250 --out run.jsonl
// --split <host clock - viewer clock, ms>: the host runs with TETHER_PTS_WALL=1 (frames stamped with
// its wall clock once captured and converted); each sample then also has `split`, the delay of the
// picture on screen in four parts: page drew it -> host has it (`capture`), -> the viewer hands it
// to the decoder (`send`: encode, network, the app's own hand-over), -> drawn (`draw`), -> read
// by this sampler (`age`). All in ms, host times corrected by the clock offset.
import { createRequire } from "module";
import fs from "node:fs";
const require = createRequire(new URL("../../apps/desktop/package.json", import.meta.url));
const { chromium } = require("playwright-core");
const arg = (k, d) => { const i = process.argv.indexOf(`--${k}`); return i > 0 ? process.argv[i + 1] : d; };
const PORT = arg("port", "9271"), SECONDS = +arg("seconds", "60"), EVERY = +arg("every", "250"), OUT = arg("out", "stress.jsonl");
const SPLIT = process.argv.includes("--split") ? +arg("split", "0") : null;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const out = fs.createWriteStream(OUT, { flags: "a" });

async function page() {
  const b = await chromium.connectOverCDP(`http://127.0.0.1:${PORT}`);
  const p = b.contexts().flatMap((c) => c.pages()).find((x) => x.url().includes("/session/"));
  if (!p) throw new Error("no session page");
  return [b, p];
}

// Runs in the viewer page once (--split): remember when each frame went into the decoder and when
// it was drawn. `window.__vt` is the frame on screen: host stamp, decode call, draw (viewer clock).
const HOOK = () => {
  if (window.__vtHooked) return;
  window.__vtHooked = true;
  const fed = new Map();
  const decode = VideoDecoder.prototype.decode;
  VideoDecoder.prototype.decode = function (c) {
    fed.set(c.timestamp, Date.now());
    if (fed.size > 300) fed.delete(fed.keys().next().value);
    return decode.call(this, c);
  };
  for (const [proto, name] of [[WebGL2RenderingContext.prototype, "texImage2D"], [CanvasRenderingContext2D.prototype, "drawImage"]]) {
    const f = proto[name];
    proto[name] = function (...a) {
      const v = a.find((x) => x instanceof VideoFrame);
      if (v) window.__vt = { pts: v.timestamp / 1000, fed: fed.get(v.timestamp), drawn: Date.now() };
      return f.apply(this, a);
    };
  }
};

// Runs in the viewer page: find the barcode once (cached in window), then decode it.
const SAMPLE = () => {
  const cv = document.querySelector("canvas");
  if (!cv || cv.width < 400) return { v: null, state: "no canvas" };
  const st = document.body.innerText.match(/Reconnecting|Connecting|disconnected|Connection (lost|closed)/i);
  const W = Math.min(cv.width, 1400), Hh = Math.min(cv.height, 500);
  const o = (window.__sc ||= document.createElement("canvas"));
  o.width = W; o.height = Hh;
  const x = o.getContext("2d", { willReadFrequently: true });
  x.drawImage(cv, 0, 0, W, Hh, 0, 0, W, Hh);
  const d = x.getImageData(0, 0, W, Hh).data;
  const px = (i, j) => { const k = (j * W + i) * 4; return [d[k], d[k + 1], d[k + 2]]; };
  const red = ([r, g, b]) => r > 150 && g < 90 && b < 90, grn = ([r, g, b]) => g > 150 && r < 90 && b < 90;
  let L = window.__loc;
  const decodeAt = (L) => {
    const bits = [];
    for (let i = 0; i < 40; i++) { const [r, g, b] = px(Math.round(L.rx + L.cell * (i + 1) + L.cell / 2), L.y); bits.push((r * 3 + g * 6 + b) / 10 < 128 ? 1 : 0); }
    let v = 0; for (let i = 0; i < 32; i++) v = v * 2 + bits[i];
    let s = 0; for (let i = 32; i < 40; i++) s = s * 2 + bits[i];
    const by = [v & 255, (v >>> 8) & 255, (v >>> 16) & 255, (v >>> 24) & 255];
    return ((by[0] + by[1] + by[2] + by[3] + 0x5a) & 255) === s ? v : null;
  };
  if (L) { const v = decodeAt(L); if (v !== null) return { v, state: st ? st[0] : "" }; }
  for (let j = 2; j < Hh; j += 3) {
    for (let i = 0; i < W - 50; i++) {
      if (!red(px(i, j))) continue;
      let e = i; while (e < W && red(px(e, j))) e++;
      let gx = e; while (gx < W && !grn(px(gx, j))) gx++;
      if (gx >= W) break;
      const cell = (gx - i) / 41;
      if (cell < 4 || Math.abs(cell - (e - i)) > 3) { i = e; continue; }
      const cand = { rx: i, y: j, cell };
      const v = decodeAt(cand);
      if (v !== null) { window.__loc = cand; return { v, state: st ? st[0] : "", found: cand }; }
      i = e;
    }
  }
  return { v: null, state: st ? st[0] : "no barcode" };
};

const end = Date.now() + SECONDS * 1000;
let b, p, n = 0, ok = 0;
while (Date.now() < end) {
  try {
    if (!p) { [b, p] = await page(); if (SPLIT !== null) await p.evaluate(HOOK); }
    // The frame's stamps are read in the same task as the canvas: no frame can come in between.
    const r = SPLIT === null ? await p.evaluate(SAMPLE) : await p.evaluate((src) => { const r = (0, eval)(`(${src})`)(); r.vt = window.__vt && { ...window.__vt, now: Date.now() }; return r; }, SAMPLE.toString());
    const t = Date.now();
    let rec = { t, state: r.state };
    if (r.v !== null && r.v !== undefined) {
      // Host value is the low 32 bits of epoch ms; restore the high bits from the viewer clock.
      const hi = Math.floor(t / 2 ** 32) * 2 ** 32;
      let host = hi + r.v; if (host - t > 2 ** 31) host -= 2 ** 32; if (t - host > 2 ** 31) host += 2 ** 32;
      rec = { t, host, lat: t - host, phase: Math.floor(host / 60000) % 4, state: r.state };
      if (r.vt?.fed) {
        // The host stamp is the low 31 bits of its epoch ms; both host times move to the viewer's clock.
        let cap = Math.floor(t / 2 ** 31) * 2 ** 31 + r.vt.pts; if (cap - t > 2 ** 30) cap -= 2 ** 31; if (t - cap > 2 ** 30) cap += 2 ** 31;
        rec.split = { capture: cap - host, send: r.vt.fed - (cap - SPLIT), draw: r.vt.drawn - r.vt.fed, age: r.vt.now - r.vt.drawn };
      }
      ok++;
    }
    if (r.found) rec.found = r.found;
    out.write(JSON.stringify(rec) + "\n");
    n++;
  } catch (e) {
    out.write(JSON.stringify({ t: Date.now(), err: String(e.message || e).slice(0, 120) }) + "\n");
    try { await b?.close(); } catch {}
    b = p = undefined;
    await sleep(2000);
  }
  await sleep(EVERY);
}
try { await b?.close(); } catch {}
out.end();
console.log(`samples ${n}, decoded ${ok}`);

// udp-stamp.mjs --ip <wsl-ip> --port 47124 --seconds 60 --out x.jsonl: poll xgrab.py sample --serve every 5 ms and
// time-stamp each new barcode value with this machine's clock (minus its age in WSL); writes stress-sampler.mjs
// lines {t, host, lat, phase}. The WSL clock is not used: it can be a second off.
import dgram from "node:dgram";
import fs from "node:fs";
const arg = (k, d) => { const i = process.argv.indexOf(`--${k}`); return i > 0 ? process.argv[i + 1] : d; };
const out = fs.createWriteStream(arg("out", "udp.jsonl"), { flags: "a" });
const s = dgram.createSocket("udp4");
let prev = null, sentAt = 0;
s.on("message", (m) => {
  const now = Date.now(), [v, age] = String(m).split(" ").map(Number);
  if (v === prev || now - sentAt > 20) return; // a late answer cannot be dated well
  prev = v;
  const t = Math.round(now - age);
  const hi = Math.floor(t / 2 ** 32) * 2 ** 32;
  let host = hi + v; if (host - t > 2 ** 31) host -= 2 ** 32; if (t - host > 2 ** 31) host += 2 ** 32;
  out.write(JSON.stringify({ t, host, lat: t - host, phase: Math.floor(host / 60000) % 4, state: "" }) + "\n");
});
const poll = setInterval(() => { sentAt = Date.now(); s.send("p", +arg("port", "47124"), arg("ip")); }, 5);
setTimeout(() => { clearInterval(poll); s.close(); out.end(); }, +arg("seconds", "60") * 1000);

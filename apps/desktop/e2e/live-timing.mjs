// Time to "Live: serial …" after unlocking Tether Manage, against two local tether-api relays.
//   node e2e/live-timing.mjs <manage.exe> [runs]
// Env: BIN (WSL dir with tether-api), VAULT (test vault), DELAY_MS: relay B (behind a proxy here)
// answers that late; a large value is a relay that accepts the connection and never answers.
import { execFileSync, spawn } from "node:child_process";
import { closeSync, copyFileSync, mkdirSync, mkdtempSync, openSync, readFileSync, writeFileSync } from "node:fs";
import { createServer, request } from "node:http";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { chromium } from "playwright-core";

const MANAGE = process.argv[2];
const RUNS = +(process.argv[3] || 3);
const BIN = process.env.BIN || "/home/thomas/rel-0526/bin";
const VAULT = process.env.VAULT || "C:/vms/manage-test";
const DELAY = +(process.env.DELAY_MS || 0);
let seeded = false; // the policy is published to both relays before relay B turns slow
const HIDDEN = resolve(import.meta.dirname, "hidden-run.ps1");
const PASS = readFileSync(join(VAULT, "test-passphrase.txt"), "utf8").trim();
const ADMIN_PUB = readFileSync(join(VAULT, "env.txt"), "utf8").trim().split(/\s+/)[0];
const work = mkdtempSync(join(tmpdir(), "live-timing-"));
const wslPath = (p) => "/mnt/" + p[0].toLowerCase() + p.slice(2).replaceAll("\\", "/");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const pages = (b) => b.contexts().flatMap((c) => c.pages());
const children = [];
async function until(fn, timeout = 30000, every = 100) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    try { const v = await fn(); if (v) return v; } catch {}
    await sleep(every);
  }
  return null;
}
const kill = (pid) => { try { execFileSync("taskkill", ["/PID", String(pid), "/T", "/F"], { stdio: "ignore" }); } catch {} };

for (const [port, d] of [[22150, "a"], [22160, "b"]]) {
  mkdirSync(join(work, d));
  children.push(spawn("wsl", ["-d", "Ubuntu-22.04", "--", "env", `ADMIN_PUB_KEY=${ADMIN_PUB}`, "RELAY_HOST=127.0.0.1,localhost", `${BIN}/tether-api`, "--data", wslPath(join(work, d)), "--listen", `127.0.0.1:${port}`], { stdio: "ignore" }));
}
// Relay B behind a proxy that can answer late.
const proxy = createServer((req, res) => {
  setTimeout(() => {
    const up = request({ host: "127.0.0.1", port: 22160, path: req.url, method: req.method, headers: req.headers }, (r) => { res.writeHead(r.statusCode, r.headers); r.pipe(res); });
    up.on("error", () => res.writeHead(502).end());
    req.pipe(up);
  }, seeded ? DELAY : 0);
});
await new Promise((r) => proxy.listen(22161, "127.0.0.1", r));
for (const p of [22150, 22160]) await until(() => fetch(`http://127.0.0.1:${p}/api/policy`).then((r) => r.status === 404 || r.status === 200), 30000, 300);

let n = 0;
async function run(seed) {
  const dir = join(work, `m${n++}`);
  mkdirSync(dir);
  copyFileSync(join(VAULT, "keys.json"), join(dir, "keys.json"));
  writeFileSync(join(dir, "settings.json"), JSON.stringify({ server: "http://127.0.0.1:22150,http://127.0.0.1:22161" }));
  const port = 9530 + n;
  const out = join(dir, "pid.txt");
  const fd = openSync(out, "w");
  execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", HIDDEN, "-Desktop", "tether-live-timing", "-Exe", MANAGE], { env: { ...process.env, TETHER_MANAGE_DIR: dir, WEBVIEW2_USER_DATA_FOLDER: join(dir, "wv2"), WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` }, stdio: ["ignore", fd, "inherit"] });
  closeSync(fd);
  const pid = +readFileSync(out, "utf8").trim().split(/\s+/).pop();
  try {
    const b = await until(() => chromium.connectOverCDP(`http://127.0.0.1:${port}`), 60000, 300);
    const p = await until(() => pages(b).find((x) => !x.url().startsWith("devtools")), 30000);
    await until(() => p.evaluate(() => !!document.querySelector("input[type=password]")), 30000);
    if (seed) {
      await p.evaluate((pw) => window.__TAURI_INTERNALS__.invoke("keys_unlock", { passphrase: pw }), PASS);
      const bundle = await p.evaluate(() => window.__TAURI_INTERNALS__.invoke("policy_draft"));
      const r = await p.evaluate((bd) => window.__TAURI_INTERNALS__.invoke("policy_publish", { bundle: bd }), bundle);
      console.log("seeded policy serial", r.serial);
      return;
    }
    await p.fill("input[type=password]", PASS);
    const t0 = Date.now();
    await p.press("input[type=password]", "Enter");
    const unlocked = await until(() => p.evaluate(() => !document.querySelector("input[type=password]")), 30000, 20);
    const t1 = Date.now();
    const live = await until(() => p.evaluate(() => /Live: (serial|nothing|could not)[^\n]*/.exec(document.body.innerText)?.[0]), 120000, 20);
    const t2 = Date.now();
    const ipc0 = Date.now();
    const again = await Promise.race([
      p.evaluate(() => window.__TAURI_INTERNALS__.invoke("policy_live").then((l) => l.relays.map((r) => (r.reachable ? "ok" : r.error)).join(" | "))),
      sleep(60000).then(() => "no answer in 60 s"),
    ]);
    const ipc = Date.now() - ipc0;
    console.log(`${MANAGE.split(/[\\/]/).pop()}: unlock ${t1 - t0} ms, then "${live ?? "Live: checking… (still after 120 s)"}" after ${t2 - t1} ms; policy_live again ${ipc} ms (${again})`);
    await b.close().catch(() => {});
  } finally {
    kill(pid);
    await sleep(1500);
  }
}
try {
  await run(true);
  seeded = true;
  for (let i = 0; i < RUNS; i++) await run(false);
} finally {
  proxy.close();
  for (const c of children) c.kill();
  try { execFileSync("wsl", ["-d", "Ubuntu-22.04", "--", "pkill", "-f", `${BIN}/tether-api --data /mnt/c`], { stdio: "ignore" }); } catch {}
}
// Pending late answers (DELAY_MS) would keep the process alive.
process.exit(0);

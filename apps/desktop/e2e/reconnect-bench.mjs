// Visible interruption when the relay connection is lost (docs/connection-resilience.md).
//
//   node e2e/reconnect-bench.mjs --exe path [--runs 5] [--restart "<shell command>"] [--logs dir]
//
// One machine, two profiles (host rcx-a with its own ID, viewer rcx-b), driven only over DevTools:
// no window is moved, raised or focused. DevTools hooks in the viewer timestamp every canvas draw
// and the moment the reconnect overlay appears. Each run executes --restart (default: restart hbbr
// on the production relay over SSH). The desktop may be static, so the last frame before the drop
// is not observable; a new connection always sends a first frame. Reported per run:
//   loss→frame: overlay shown (drop detected) → first frame drawn after it (lower bound)
//   cmd→frame:  restart command started → first frame after it (upper bound; includes SSH)
// Only processes this script started are stopped (by PID).

import { spawn, execFileSync, execSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { existsSync, mkdirSync, openSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { parseArgs } from "node:util";
import { chromium } from "playwright-core";

const { values: opt } = parseArgs({
  options: {
    exe: { type: "string" },
    runs: { type: "string", default: "5" },
    restart: { type: "string", default: 'ssh -i ~/.ssh/tether_oracle opc@158.101.205.185 "sudo systemctl restart tether-hbbr"' },
    hold: { type: "string" },
    // Restart the rendezvous server instead: the running session must not notice (the host's
    // re-registration time is in the hbbs log: "IP change of <id>" after "Listening on").
    "hbbs-restart": { type: "string" },
    logs: { type: "string" },
    "viewer-profile": { type: "string", default: "rcx-b" },
  },
});
if (!opt.exe) throw new Error("--exe is required");
const PW = "Rc-Pass-" + randomBytes(4).toString("hex") + "!";
const procs = [];
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const launch = (cmd, args, env, log) => {
  const stdio = log && opt.logs ? ["ignore", openSync(join(opt.logs, log), "w"), openSync(join(opt.logs, log), "a")] : "ignore";
  procs.push(spawn(cmd, args, { env: { ...process.env, ...env }, stdio }));
};

async function until(what, fn, timeout = 30000, every = 250) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    try {
      const v = await fn();
      if (v) return v;
    } catch {}
    await sleep(every);
  }
  throw new Error(`timeout: ${what}`);
}
const cdp = (port) => until(`cdp ${port}`, () => chromium.connectOverCDP(`http://127.0.0.1:${port}`), 30000, 500);
const page = (b, m) => until(`page ${m}`, () => b.contexts().flatMap((c) => c.pages()).find((p) => p.url().includes(m)));
const ipc = (p, cmd, args) => p.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a), [cmd, args]);
const median = (a) => {
  const s = [...a].sort((x, y) => x - y);
  return s.length % 2 ? s[(s.length - 1) / 2] : (s[s.length / 2 - 1] + s[s.length / 2]) / 2;
};

// Every profile on one PC derives the same machine ID, and hbbs routes an ID to whoever
// registered last (the user's own Tether, other test hosts). Give the test host its own ID.
function seedHostId(profile) {
  const dir = join(process.env.APPDATA, `Tether-${profile}`, "config");
  const file = join(dir, `Tether-${profile}.toml`);
  if (existsSync(file)) return;
  mkdirSync(dir, { recursive: true });
  writeFileSync(file, `id = '${700000000 + Math.floor(Math.random() * 99999999)}'
`);
}

async function main() {
  seedHostId("rcx-a");
  launch(opt.exe, [], { TETHER_PROFILE: "rcx-a", TETHER_DEV_PASSWORD: PW, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9242" }, "host.log");
  const a = await page(await cdp(9242), "index.html");
  const st = await until("host online", async () => {
    const s = await ipc(a, "status");
    return s.status === 1 && s.key_confirmed && s.id ? s : null;
  }, 60000);
  await sleep(4000); // dev password is set 3 s after start

  launch(opt.exe, ["--connect", st.id, PW], { TETHER_PROFILE: opt["viewer-profile"], WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9243" }, "viewer.log");
  const b = await page(await cdp(9243), "/session/");
  const key = b.url().match(/session\/([0-9a-f]{32})/)[1];
  await until("video", () => b.evaluate(() => document.querySelector("canvas")?.width > 0), 40000);
  await b.evaluate((key) => {
    const rc = (window.__rc = { frames: [], lost: [], back: [] });
    for (const m of ["drawImage", "putImageData"]) {
      const orig = CanvasRenderingContext2D.prototype[m];
      CanvasRenderingContext2D.prototype[m] = function (...args) {
        rc.frames.push(Date.now());
        if (rc.frames.length > 2000) rc.frames.splice(0, 1000);
        return orig.apply(this, args);
      };
    }
    let shown = false;
    new MutationObserver(() => {
      const now = document.body.innerText.includes("Reconnecting");
      if (now && !shown) rc.lost.push(Date.now());
      if (!now && shown) rc.back.push(Date.now());
      shown = now;
    }).observe(document.body, { subtree: true, childList: true, characterData: true });
  }, key);
  await sleep(3000);
  if (opt.hold) {
    console.log(`holding ${opt.hold} ms (host ${st.id})`);
    await sleep(+opt.hold);
    return;
  }

  if (opt["hbbs-restart"]) {
    console.log(`host ${st.id}`);
    for (let run = 1; run <= +opt.runs; run++) {
      await b.evaluate(() => (window.__rc.lost = []));
      execSync(opt["hbbs-restart"], { stdio: "ignore", shell: "bash" });
      await sleep(5000);
      const dropped = await b.evaluate(() => window.__rc.lost.length);
      console.log(`hbbs run ${run}: session ${dropped ? "DROPPED" : "kept"}`);
    }
    return;
  }

  const gaps = [];
  const upper = [];
  let lost = 0;
  for (let run = 1; run <= +opt.runs; run++) {
    const t0 = Date.now();
    execSync(opt.restart, { stdio: "ignore", shell: "bash" });
    const r = await until(`run ${run}`, () => b.evaluate((t0) => {
      const loss = window.__rc.lost.find((t) => t >= t0);
      // First frame of the new connection: drawn after the overlay went away (frames still
      // queued from the old one can land just after the overlay appears).
      const back = loss && window.__rc.back.find((t) => t > loss);
      const frame = back && window.__rc.frames.find((t) => t >= back);
      return frame ? { loss, frame } : null;
    }, t0), 30000, 100).catch(() => null);
    if (!r) {
      lost++;
      const text = await b.evaluate(() => document.body.innerText.slice(0, 200).replace(/\s+/g, " "));
      console.log(`run ${run}: no frame within 30 s (${text}); reconnecting by hand`);
      await ipc(b, "session_reconnect", { key, forceRelay: false });
      await sleep(15000);
      continue;
    }
    gaps.push(r.frame - r.loss);
    upper.push(r.frame - t0);
    console.log(`run ${run}: loss→frame ${r.frame - r.loss} ms, cmd→frame ${r.frame - t0} ms`);
    await sleep(6000);
  }
  const sum = (a) => (a.length ? { median: median(a), max: Math.max(...a), all: a } : null);
  console.log(JSON.stringify({ runs: +opt.runs, lost, loss_to_frame_ms: sum(gaps), cmd_to_frame_ms: sum(upper) }));
}

try {
  await main();
} catch (e) {
  console.error("reconnect bench failed:", e.message);
  process.exitCode = 1;
} finally {
  for (const p of procs) {
    try {
      execFileSync("taskkill", ["/PID", String(p.pid), "/T", "/F"], { stdio: "ignore" });
    } catch {}
  }
}

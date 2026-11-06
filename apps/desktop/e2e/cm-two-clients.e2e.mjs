// Two connections at once: each one's permissions can be seen and changed, and the switch sends
// cm_permission for that connection. TetherQS (apps/quick, 420x560 window, Tauri IPC replaced by a
// stand-in in the page) and the technician app's connection window (/cm, mock mode). Starts its own
// Vite dev servers; headless Chromium and WebKit.
//
//   node e2e/cm-two-clients.e2e.mjs [chromium|webkit]
import { spawn } from "node:child_process";
import { resolve } from "node:path";
import { chromium, webkit } from "playwright-core";

const APPS = resolve(import.meta.dirname, "../..");
const engines = process.argv[2] ? [process.argv[2]] : ["chromium", "webkit"];
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let failures = 0;
const check = (what, ok, detail = "") => {
  console.log(`${ok ? "PASS" : "FAIL"} ${what}${detail ? `  (${detail})` : ""}`);
  if (!ok) failures++;
};

function vite(app) {
  const port = 14820 + Math.floor(Math.random() * 150) + (app === "quick" ? 0 : 160);
  const cwd = resolve(APPS, app);
  const p = spawn(process.execPath, [resolve(cwd, "node_modules/vite/bin/vite.js"), "--port", String(port), "--strictPort"], { cwd, stdio: ["ignore", "pipe", "pipe"] });
  const ready = new Promise((ok, fail) => {
    p.stdout.on("data", (d) => String(d).includes("Local") && ok());
    setTimeout(() => fail(new Error(`vite ${app} did not start`)), 30000);
  });
  return { p, port, ready };
}

// TetherQS state with two accepted technicians (the screenshot of the report).
const PERMS = [
  ["keyboard", "Control mouse and keyboard"],
  ["clipboard", "Use the clipboard"],
  ["file", "Transfer files"],
  ["audio", "Hear this computer's sound"],
  ["restart", "Restart this computer"],
  ["block_input", "Block my mouse and keyboard"],
].map(([name, label]) => ({ name, label, allowed: true, locked: false }));
const client = (id) => ({ id, peer_id: String(100000000 + id), name: "Thomas", authorized: true, is_file_transfer: false, keyboard: true, clipboard: true, file: true, audio: true, restart: true, block_input: false });
const STATE = {
  id: "731204986",
  password: "k7x9q2",
  status: 1,
  version: "0.5.19",
  elevated: false,
  blocked: false,
  mac: null,
  // "Allow admin access" and an update banner take height too (the worst case of the report).
  branding: { title: "Tether Support", accent: "#0071e3", welcome: "", elevate: true, approve_mode: "", consent_text: "", diagnostics: false, perms: PERMS, custom_app: null, logo: "" },
  update: { available: "0.5.20", notes: "", required: false, forced_at: null, installing: false, error: null, last_check: null, progress: null },
  clients: [client(1), client(2)],
};

/** In-page stand-in for window.__TAURI_INTERNALS__ (what @tauri-apps/api calls). */
function fakeTauri(state) {
  window.__calls = [];
  let n = 0;
  window.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: "main" }, currentWebview: { windowLabel: "main", label: "main" } },
    transformCallback: () => ++n,
    unregisterCallback: () => {},
    convertFileSrc: (s) => s,
    invoke: async (cmd, args) => {
      window.__calls.push({ cmd, args });
      if (cmd === "state") return state;
      if (cmd === "cm_permission") state.clients.find((c) => c.id === args.id)[args.name] = args.enabled;
      return cmd.startsWith("plugin:") ? (cmd.endsWith("listen") ? ++n : false) : null;
    },
  };
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
}

const calls = (p) => p.evaluate(() => window.__calls ?? window.__mockCalls);

async function quick(name, port) {
  const browser = await { chromium, webkit }[name].launch();
  const p = await browser.newPage({ viewport: { width: 420, height: 560 } });
  await p.addInitScript(fakeTauri, STATE);
  await p.goto(`http://localhost:${port}/`);
  const tag = (s) => `[${name}] TetherQS ${s}`;
  await p.getByRole("button", { name: "Disconnect" }).nth(1).waitFor({ timeout: 10000 });
  for (const [i, id] of [[1, 2], [0, 1]]) {
    const sw = p.getByRole("switch", { name: "Control mouse and keyboard" }).nth(i);
    // The switch must be on screen and not clipped (the click checks it is the hit target).
    const box = await sw.boundingBox();
    check(tag(`card ${i + 1}: permission switch rendered with a size`), !!box && box.height > 0, JSON.stringify(box));
    await p.evaluate(() => (window.__calls.length = 0));
    const clicked = await sw.click({ timeout: 3000 }).then(() => true, (e) => String(e.message).split("\n")[0]);
    check(tag(`card ${i + 1}: switch can be clicked`), clicked === true, clicked === true ? "" : clicked);
    await sleep(100);
    const got = (await calls(p)).filter((c) => c.cmd === "cm_permission");
    check(tag(`card ${i + 1}: cm_permission for connection ${id}`), got.length === 1 && got[0].args.id === id && got[0].args.name === "keyboard" && got[0].args.enabled === false, JSON.stringify(got));
  }
  // Every permission row (label + switch) of both cards can be brought fully into view: no
  // scrolling container squeezed to a sliver around it.
  const all = p.getByRole("switch");
  check(tag("both cards show all six switches"), (await all.count()) === 12, String(await all.count()));
  const clipped = [];
  for (let i = 0; i < (await all.count()); i++) {
    const ok = await all.nth(i).evaluate((sw) => {
      const row = sw.parentElement;
      row.scrollIntoView({ block: "nearest" });
      const r = row.getBoundingClientRect();
      if (r.height < 30 || r.top < 0 || r.bottom > innerHeight) return false;
      for (let a = row.parentElement; a; a = a.parentElement) {
        if (getComputedStyle(a).overflowY === "visible") continue;
        const b = a.getBoundingClientRect();
        if (r.top < b.top - 0.5 || r.bottom > b.bottom + 0.5) return false;
      }
      const hit = document.elementFromPoint(r.x + r.width - 20, r.y + r.height / 2);
      return hit === sw || sw.contains(hit);
    });
    if (!ok) clipped.push(i);
  }
  check(tag("every permission row of both cards can be scrolled fully into view"), clipped.length === 0, clipped.length ? `clipped rows: ${clipped}` : "");
  const lastSw = all.nth(11);
  await p.evaluate(() => (window.__calls.length = 0));
  const c2 = await lastSw.click({ timeout: 3000 }).then(() => true, (e) => String(e.message).split("\n")[0]);
  const got = (await calls(p)).filter((c) => c.cmd === "cm_permission");
  check(tag("card 2: last switch (block input) sends cm_permission for connection 2"), c2 === true && got.length === 1 && got[0].args.id === 2 && got[0].args.name === "block_input" && got[0].args.enabled === true, c2 === true ? JSON.stringify(got) : c2);
  await browser.close();
}

async function cm(name, port) {
  const browser = await { chromium, webkit }[name].launch();
  const p = await browser.newPage({ viewport: { width: 380, height: 560 } });
  await p.addInitScript(() => (window.__mockCalls = []));
  await p.goto(`http://localhost:${port}/#/cm`);
  const tag = (s) => `[${name}] technician-app connection window ${s}`;
  const tabs = p.locator("button", { hasText: /@/ });
  await tabs.nth(1).waitFor({ timeout: 10000 }).catch(() => {});
  check(tag("one tab per connection"), (await tabs.count()) === 2, String(await tabs.count()));
  for (const [i, id] of [[1, 2], [0, 1]]) {
    if ((await tabs.count()) < 2) break;
    await tabs.nth(i).click();
    await p.evaluate(() => (window.__mockCalls.length = 0));
    const clicked = await p.getByRole("switch", { name: "Use clipboard" }).click({ timeout: 3000 }).then(() => true, (e) => String(e.message).split("\n")[0]);
    check(tag(`connection ${id}: switch can be clicked`), clicked === true, clicked === true ? "" : clicked);
    const got = (await calls(p)).filter((c) => c.cmd === "cm_permission");
    check(tag(`connection ${id}: cm_permission for connection ${id}`), got.length === 1 && got[0].args.id === id && got[0].args.name === "clipboard" && got[0].args.enabled === false, JSON.stringify(got));
  }
  await browser.close();
}

const servers = [vite("quick"), vite("desktop")];
try {
  await Promise.all(servers.map((s) => s.ready));
  for (const e of engines) {
    await quick(e, servers[0].port);
    await cm(e, servers[1].port);
  }
} finally {
  servers.forEach((s) => s.p.kill());
}
console.log(failures ? `\n${failures} FAILED` : "\nall passed");
process.exit(failures ? 1 : 0);

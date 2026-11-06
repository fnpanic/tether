// Server key rotation end to end (docs/key-rotation.md), everything on this PC:
// - hbbs/hbbr from this checkout (Windows build) on 127.0.0.1:22116/22117, started with key A;
// - tether-api from this checkout in WSL on 127.0.0.1:22130, trusting the test vault's admin key;
// - Tether Manage (copy of the test vault) drives the rotation on Keys › Server key;
// - tether.exe built with TETHER_SERVER=127.0.0.1:22116, TETHER_PUB_KEY=A, the test admin key
//   (debug build: TETHER_DEV_API points its API at the local tether-api), as host "kr-host",
//   viewer "kr-view" (gets the lists), "kr-stale" (cannot reach the API, never gets a list) and
//   "kr-new" (a fresh install of the old build after the rotation).
// Every app runs on an invisible desktop, with its own WebView2 data folder (a Tether or Tether
// Manage already running on this PC keeps the default one locked), and is driven over DevTools only. Only processes started
// here are stopped (by PID).
//
//   node e2e/keyrot-e2e.mjs
// Env: TECH_EXE, MANAGE_EXE, SERVER_BIN (dir with hbbs.exe/hbbr.exe), KEY_A (dir with id_ed25519 of
// key A), VAULT (C:\vms\manage-test), API_BIN (tether-api path inside WSL).
import { execFileSync, spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { closeSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, openSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { chromium } from "playwright-core";

const T = "C:\\cargo-target\\keyrot";
const TECH = process.env.TECH_EXE || `${T}\\debug\\tether.exe`;
const MANAGE = process.env.MANAGE_EXE || `${T}\\debug\\tether-manage.exe`;
const SBIN = process.env.SERVER_BIN || `${T}\\debug`;
const KEY_A = process.env.KEY_A || `${T}\\e2e\\keyA`;
const VAULT = process.env.VAULT || "C:\\vms\\manage-test";
const API_BIN = process.env.API_BIN || "/home/thomas/keyrot-build/server/api/target/debug/tether-api";
const HIDDEN = resolve(import.meta.dirname, "hidden-run.ps1");
const DESK = "tether-keyrot-test";
const API = "http://127.0.0.1:22130";
const PW = "Kr-Pass-" + randomBytes(4).toString("hex") + "!";
const PASS = readFileSync(join(VAULT, "test-passphrase.txt"), "utf8").trim();
const ADMIN_PUB = readFileSync(join(VAULT, "env.txt"), "utf8").trim().split(/\s+/)[0];
const A = readFileSync(join(KEY_A, "id_ed25519.pub"), "utf8").trim();
const work = mkdtempSync(join(tmpdir(), "keyrot-e2e-"));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const pages = (b) => b.contexts().flatMap((c) => c.pages());
const ipc = (p, cmd, args) => p.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a), [cmd, args]);
const results = [];
const check = (name, ok, detail = "") => {
  results.push({ name, ok: !!ok });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? `  (${detail})` : ""}`);
};
const pids = [];
const children = [];

async function until(what, fn, timeout = 30000, every = 250) {
  if (process.env.VERBOSE) console.log(`  ${new Date().toISOString().slice(11, 19)} … ${what}`);
  const end = Date.now() + timeout;
  let last;
  while (Date.now() < end) {
    try {
      // A call that never settles (fetch on a half-open socket, CDP) must not stall the loop.
      const v = await Promise.race([fn(), sleep(Math.max(every, 10000)).then(() => { throw new Error("attempt timed out"); })]);
      if (v) return v;
    } catch (e) {
      last = e;
    }
    await sleep(every);
  }
  throw new Error(`timeout: ${what}${last ? ` (${last.message})` : ""}`);
}

function hidden(exe, args, env) {
  const a = ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", HIDDEN, "-Desktop", DESK, "-Exe", exe];
  if (args) a.push("-Args", args); // an empty -Args value would be dropped
  // PID via a file: a console child (debug builds) gets PowerShell's std handles, so a pipe here
  // would never close.
  const out = join(work, `pid-${pids.length}.txt`);
  const fd = openSync(out, "w");
  execFileSync("powershell", a, { env: { ...process.env, ...env }, stdio: ["ignore", fd, "inherit"] });
  closeSync(fd);
  const pid = +readFileSync(out, "utf8").trim().split(/\s+/).pop();
  if (process.env.VERBOSE) console.log(`  started ${exe.split("\\").pop()} ${args} -> pid ${pid}`);
  pids.push(pid);
  return pid;
}
const alive = (pid) => {
  try {
    return execFileSync("tasklist", ["/FI", `PID eq ${pid}`, "/NH"]).toString().includes(String(pid));
  } catch {
    return false;
  }
};
const kill = (pid) => {
  try {
    execFileSync("taskkill", ["/PID", String(pid), "/T", "/F"], { stdio: "ignore" });
  } catch {}
};

// ---- relay (hbbs + hbbr) on this PC ----
const relayDir = join(work, "relay");
mkdirSync(relayDir);
let relay = [];
function setRelayKey(dir) {
  copyFileSync(join(dir, "id_ed25519"), join(relayDir, "id_ed25519"));
  copyFileSync(join(dir, "id_ed25519.pub"), join(relayDir, "id_ed25519.pub"));
}
async function startRelay(accept) {
  for (const p of relay) p.kill();
  relay = [];
  await sleep(1500);
  const env = { ...process.env, HEALTH_PORT: "0", KEYS_ACCEPT: accept };
  for (const [bin, args] of [["hbbr", ["-p", "22117", "-k", "_"]], ["hbbs", ["-p", "22116", "-r", "127.0.0.1:22117", "-k", "_"]]]) {
    const p = spawn(join(SBIN, `${bin}.exe`), args, { cwd: relayDir, env, stdio: ["ignore", "pipe", "pipe"] });
    let out = "";
    const grab = (d) => (out += d);
    p.stdout.on("data", grab);
    p.stderr.on("data", grab);
    p.out = () => out;
    relay.push(p);
  }
  await until("hbbs listening", () => relay[1].out().includes("Listening on websocket"), 20000);
  await sleep(1000);
}

// ---- tether-api in WSL ----
function startApi() {
  const p = spawn("wsl", ["-d", "Ubuntu-22.04", "--", "env", `ADMIN_PUB_KEY=${ADMIN_PUB}`, API_BIN, "--data", `/tmp/keyrot-e2e-api-${process.pid}`, "--listen", "127.0.0.1:22130"], { stdio: "ignore" });
  children.push(p);
}
const getJson = async (path) => {
  const r = await fetch(API + path, { signal: AbortSignal.timeout(5000) });
  return r.status === 200 ? r.json() : null;
};
const listKeys = async () => {
  const doc = await getJson("/api/server-keys");
  return doc ? JSON.parse(Buffer.from(doc.payload, "base64")) : null;
};

// ---- Tether Manage ----
async function startManage() {
  if (process.env.VERBOSE) console.log("  starting Tether Manage");
  const dir = join(work, "manage");
  mkdirSync(dir);
  copyFileSync(join(VAULT, "keys.json"), join(dir, "keys.json"));
  writeFileSync(join(dir, "settings.json"), JSON.stringify({ server: API }));
  hidden(MANAGE, "", { TETHER_MANAGE_DIR: dir, WEBVIEW2_USER_DATA_FOLDER: join(dir, "wv2"), WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9381" });
  const b = await until("manage devtools", () => chromium.connectOverCDP("http://127.0.0.1:9381"), 60000, 500);
  const p = await until("manage page", () => pages(b).find((x) => !x.url().startsWith("devtools")), 30000);
  await until("manage ready", () => p.evaluate(() => !!window.__TAURI_INTERNALS__), 30000);
  await ipc(p, "keys_unlock", { passphrase: PASS });
  await p.reload();
  return p;
}

// ---- Tether clients ----
const profileDir = (profile) => join(process.env.APPDATA, `Tether-${profile}`);
function freshProfile(profile, id) {
  rmSync(profileDir(profile), { recursive: true, force: true });
  if (id) {
    mkdirSync(join(profileDir(profile), "config"), { recursive: true });
    writeFileSync(join(profileDir(profile), "config", `Tether-${profile}.toml`), `id = '${id}'\n`);
  }
}
const withApi = (profile) => ({ TETHER_PROFILE: profile, TETHER_DEV_API: API });
/** `tether --policy-refresh` in that profile: fetches policy and key list into its cache. */
function policyRefresh(env) {
  return execFileSync(TECH, ["--policy-refresh"], { env: { ...process.env, ...env }, timeout: 60000 }).toString().trim();
}

let cdpPort = 9390;
/** One viewer session to the host: "connected" (first video frame) or the error text shown. */
async function connect(env, id) {
  const port = cdpPort++;
  const pid = hidden(TECH, `--connect ${id} --password-env TETHER_E2E_PW`, { ...env, TETHER_E2E_PW: PW, WEBVIEW2_USER_DATA_FOLDER: join(work, `wv2-${port}`), WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` });
  try {
    const b = await until("viewer devtools", () => chromium.connectOverCDP(`http://127.0.0.1:${port}`), 60000, 500);
    const s = await until("session page", () => pages(b).find((p) => p.url().includes("/session/")), 30000);
    let closed = false;
    s.on("close", () => (closed = true));
    const out = await until("session outcome", () =>
      closed ? `window closed (process ${alive(pid) ? "running" : "gone"})` : s.evaluate(() => {
        // Connected: a frame is on the canvas (opacity 1) and the session is end-to-end encrypted.
        if (document.querySelector("canvas")?.style.opacity === "1" && document.querySelector('[title^="End-to-end encrypted"]')) return "connected";
        const t = document.body.innerText;
        return /server key mismatch|connection blocked|offline|timed out|failed|refused|declined|error/i.test(t) ? t.replace(/\s+/g, " ").slice(0, 400) : null;
      }), 60000, 300);
    await b.close().catch(() => {});
    return out;
  } finally {
    kill(pid);
    await sleep(1500);
  }
}

async function main() {
  for (const f of [TECH, MANAGE, join(SBIN, "hbbs.exe"), join(SBIN, "hbbr.exe")]) if (!existsSync(f)) throw new Error(`missing ${f}`);
  console.log(`work dir ${work}\nkey A ${A}`);
  setRelayKey(KEY_A);
  await startRelay("");
  startApi();
  await until("tether-api", () => fetch(API + "/api/policy", { signal: AbortSignal.timeout(3000) }).then((r) => r.status === 404), 30000, 500);

  const m = await startManage();
  // Fast apply: the host checks policy and key list, and heartbeats, every minute.
  const bundle = { serial: 0, issued_at: 0, profiles: { technician: { "policy.fast-apply": { state: "locked" } } }, groups: {}, assignments: {} };
  const pub = await ipc(m, "policy_publish", { bundle });
  check("Manage publishes the fast-apply policy to the local tether-api", pub.relays.every((r) => r.ok));

  const hostId = String(700000000 + Math.floor(Math.random() * 99999999));
  for (const p of ["kr-host", "kr-view", "kr-stale", "kr-new"]) freshProfile(p, p === "kr-host" ? hostId : null);
  hidden(TECH, "", { ...withApi("kr-host"), TETHER_DEV_PASSWORD: PW, WEBVIEW2_USER_DATA_FOLDER: join(work, "wv2-host"), WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9382" });
  const hb = await until("host devtools", () => chromium.connectOverCDP("http://127.0.0.1:9382"), 60000, 500);
  const h = await until("host page", () => pages(hb).find((p) => p.url().includes("index.html") || p.url().startsWith("tauri") || p.url().startsWith("http://tauri")), 30000);
  await until("host online", async () => {
    const st = await ipc(h, "status");
    return st.status === 1 && st.key_confirmed && st.id === hostId;
  }, 90000, 500);
  await sleep(4000); // dev password is set 3 s after start
  const hostRefresh = () => ipc(h, "policy_refresh");
  const VIEW = withApi("kr-view");
  const STALE = { TETHER_PROFILE: "kr-stale" }; // no API: https://127.0.0.1 does not answer

  // Phase 0: everything on key A.
  check("before: viewer connects with the built-in key", (await connect(VIEW, hostId)) === "connected");
  check("before: stale viewer connects with the built-in key", (await connect(STALE, hostId)) === "connected");

  // Phase 1: Manage generates B and announces [A, B].
  await m.getByRole("button", { name: "Keys" }).click();
  const steps = m.getByTestId("sk-steps");
  await until("rotation card", () => steps.isVisible(), 20000);
  await m.getByPlaceholder("base64, 44 characters").fill(A);
  await m.locator('[data-testid="sk-step-generate"] input[type="password"]').fill(PASS);
  await m.getByRole("button", { name: "Generate new key" }).click();
  await until("announce step", async () => (await steps.getAttribute("data-step")) === "announce", 60000);
  const state = await ipc(m, "state");
  const B = state.server_keys.at(-1);
  check("Manage generated a new server key and sealed it into the vault", B && B !== A && readFileSync(join(work, "manage", "keys.json"), "utf8").length > 0, B);
  await m.getByRole("button", { name: "Publish key list…" }).click();
  await m.getByRole("button", { name: /^Publish · 1 relay/ }).click();
  await m.getByRole("dialog").getByRole("button", { name: "Done" }).click();
  let list = await listKeys();
  check("relay API serves the signed list [A, B]", list && list.keys.map((k) => k.key).join() === [A, B].join(), list && `serial ${list.serial}`);
  await until("switch step", async () => (await steps.getAttribute("data-step")) === "switch", 20000);
  await hostRefresh();
  console.log("  view:", policyRefresh(VIEW));
  check("phase 1: viewer with the list still connects (relays on A)", (await connect(VIEW, hostId)) === "connected");
  check("phase 1: stale viewer still connects (relays on A)", (await connect(STALE, hostId)) === "connected");
  const confirmed = await until("devices confirm list", async () => {
    await m.reload();
    await m.getByRole("button", { name: "Keys" }).click();
    const t = await m.getByTestId("sk-confirmed").innerText();
    const n = +(t.match(/^(\d+) of/) || [])[1];
    return n >= 1 ? t : null; // the host; --connect viewers quit before their first heartbeat
  }, 150000, 5000).catch(() => null);
  const devs = (await ipc(m, "devices_all")).devices.map((d) => `${d.id}:${d.server_keys_serial ?? "-"}`).join(" ");
  check("Manage counts devices that applied the list (heartbeat server_keys_serial)", confirmed, `${confirmed ?? "no count"}; devices ${devs}`);

  // Phase 2: relays sign with B and still accept A.
  // The button opens a folder dialog; the command behind it is called directly.
  mkdirSync(join(work, "relay-new"));
  await ipc(m, "serverkeys_save_files", { public: B, folder: join(work, "relay-new") });
  check("Manage writes id_ed25519 / id_ed25519.pub for the relays", readFileSync(join(work, "relay-new", "id_ed25519.pub"), "utf8") === B);
  setRelayKey(join(work, "relay-new"));
  await startRelay(A);
  check("hbbs signs with B and accepts A", relay[1].out().includes(`Key: ${B}`) && relay[1].out().includes(`KEYS_ACCEPT): ${A}`));
  await until("host re-registered", async () => (await ipc(h, "status")).status === 1, 60000, 500);
  await sleep(3000);
  check("phase 2: viewer with the list connects without reinstall", (await connect(VIEW, hostId)) === "connected");
  let out = await connect(STALE, hostId);
  check("phase 2: viewer without the list fails with the server key message", /server key mismatch: the server's key is not trusted/i.test(out), out.slice(0, 160));
  await m.getByRole("button", { name: "Every relay is switched" }).click();
  await until("retire step", async () => (await steps.getAttribute("data-step")) === "retire", 20000);

  // Phase 3a: list [B] only.
  await m.getByRole("button", { name: "Publish list without the old key…" }).click();
  await m.getByRole("button", { name: /^Publish · 1 relay/ }).click();
  await m.getByRole("dialog").getByRole("button", { name: "Done" }).click();
  list = await listKeys();
  check("relay API serves the retire list [B]", list && list.keys.map((k) => k.key).join() === B, list && `serial ${list.serial}`);
  await hostRefresh();
  console.log("  view:", policyRefresh(VIEW));
  check("phase 3a: viewer (now presenting B) connects", (await connect(VIEW, hostId)) === "connected");

  // Phase 3b: relays stop accepting A.
  await startRelay("");
  await until("host re-registered", async () => (await ipc(h, "status")).status === 1, 60000, 500);
  await sleep(3000);
  check("phase 3b: viewer connects after A is retired on the relays", (await connect(VIEW, hostId)) === "connected");
  out = await connect(STALE, hostId);
  check("phase 3b: viewer without the list is refused with the server key message", /server key mismatch: the server did not accept/i.test(out), out.slice(0, 160));
  console.log("  new:", policyRefresh(withApi("kr-new")));
  check("an old-build install that fetches the list once works too", (await connect(withApi("kr-new"), hostId)) === "connected");
  await until("relays step", async () => (await steps.getAttribute("data-step")) === "relays", 20000);
  await m.getByRole("button", { name: "Every relay is done" }).click();
  check("Manage shows the rotation as finished", await until("done", () => m.getByTestId("sk-done").isVisible(), 20000).catch(() => false));
}

try {
  await main();
} catch (e) {
  console.error("keyrot e2e failed:", e.message);
  results.push({ name: e.message, ok: false });
} finally {
  for (const p of pids) kill(p);
  for (const p of relay) p.kill();
  for (const p of children) p.kill();
  try {
    execFileSync("wsl", ["-d", "Ubuntu-22.04", "--", "pkill", "-f", `keyrot-e2e-api-${process.pid}`], { stdio: "ignore" });
  } catch {}
  const failed = results.filter((r) => !r.ok).length;
  console.log(`\n${results.length - failed}/${results.length} checks passed`);
  process.exitCode = failed ? 1 : 0;
}

// Session audit log end to end (docs/api.md "session audit log"), everything on this PC:
// - hbbs/hbbr (Windows build) on 127.0.0.1:22216/22217 with key A;
// - tether-api from this checkout in WSL on 127.0.0.1:22230, trusting the test vault's admin key;
// - tether.exe debug build with TETHER_SERVER=127.0.0.1:22216, TETHER_PUB_KEY=A and the test admin
//   key; TETHER_DEV_API points its API and its audit posts (vendor common.rs get_audit_server) at
//   the local tether-api. Host "au-host"; viewer "au-view" makes a remote session, then a file
//   transfer session with one upload and one download, each ended by closing the viewer;
// - Tether Manage (copy of the test vault) reads the records and shows them on Sessions.
// Every app runs on an invisible desktop with its own WebView2 data folder and is driven over
// DevTools only. Only processes started here are stopped (by PID).
//
//   node e2e/audit-e2e.mjs
// Env: TECH_EXE, MANAGE_EXE, SERVER_BIN (dir with hbbs.exe/hbbr.exe), KEY_A (dir with id_ed25519 of
// key A), VAULT (C:\vms\manage-test), API_BIN (tether-api path inside WSL).
import { execFileSync, spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { closeSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, openSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { chromium } from "playwright-core";

const T = "C:\\cargo-target\\audit";
const TECH = process.env.TECH_EXE || `${T}\\debug\\tether.exe`;
const MANAGE = process.env.MANAGE_EXE || `${T}\\debug\\tether-manage.exe`;
const SBIN = process.env.SERVER_BIN || "C:\\cargo-target\\keyrot\\debug";
const KEY_A = process.env.KEY_A || "C:\\cargo-target\\keyrot\\e2e\\keyA";
const VAULT = process.env.VAULT || "C:\\vms\\manage-test";
const API_BIN = process.env.API_BIN || "/home/thomas/audit-build/server/api/target/x86_64-unknown-linux-musl/release/tether-api";
const HIDDEN = resolve(import.meta.dirname, "hidden-run.ps1");
const DESK = "tether-audit-test";
const API = "http://127.0.0.1:22230";
const PW = "Au-Pass-" + randomBytes(4).toString("hex") + "!";
const PASS = readFileSync(join(VAULT, "test-passphrase.txt"), "utf8").trim();
const ADMIN_PUB = readFileSync(join(VAULT, "env.txt"), "utf8").trim().split(/\s+/)[0];
const work = mkdtempSync(join(tmpdir(), "audit-e2e-"));
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
      const v = await Promise.race([fn(), sleep(Math.max(every, 10000)).then(() => { throw new Error("attempt timed out"); })]);
      if (v) return v;
    } catch (e) {
      last = e;
    }
    await sleep(every);
  }
  throw new Error(`timeout: ${what}${last ? ` (${last.message})` : ""}`);
}

function hidden(exe, args, env, errFile) {
  const a = ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", HIDDEN, "-Desktop", DESK, "-Exe", exe];
  if (args) a.push("-Args", args); // an empty -Args value would be dropped
  const out = join(work, `pid-${pids.length}.txt`);
  const fd = openSync(out, "w");
  // Debug builds log to the console they inherit: the host's goes to a file for the log check.
  const err = errFile ? openSync(errFile, "w") : "inherit";
  execFileSync("powershell", a, { env: { ...process.env, ...env }, stdio: ["ignore", fd, err] });
  closeSync(fd);
  if (errFile) closeSync(err);
  const pid = +readFileSync(out, "utf8").trim().split(/\s+/).pop();
  pids.push(pid);
  return pid;
}
const kill = (pid) => {
  try {
    execFileSync("taskkill", ["/PID", String(pid), "/T", "/F"], { stdio: "ignore" });
  } catch {}
};

// ---- relay, API, Manage ----
function startRelay() {
  const dir = join(work, "relay");
  mkdirSync(dir);
  for (const f of ["id_ed25519", "id_ed25519.pub"]) copyFileSync(join(KEY_A, f), join(dir, f));
  const env = { ...process.env, HEALTH_PORT: "0" };
  const out = [];
  for (const [bin, args] of [["hbbr", ["-p", "22217", "-k", "_"]], ["hbbs", ["-p", "22216", "-r", "127.0.0.1:22217", "-k", "_"]]]) {
    const p = spawn(join(SBIN, `${bin}.exe`), args, { cwd: dir, env, stdio: ["ignore", "pipe", "pipe"] });
    let s = "";
    p.stdout.on("data", (d) => (s += d));
    p.stderr.on("data", (d) => (s += d));
    out.push(() => s);
    children.push(p);
  }
  return until("hbbs listening", () => out[1]().includes("Listening on websocket"), 20000);
}
function startApi() {
  children.push(spawn("wsl", ["-d", "Ubuntu-22.04", "--", "env", `ADMIN_PUB_KEY=${ADMIN_PUB}`, "RELAY_HOST=127.0.0.1,localhost", API_BIN, "--data", `/tmp/audit-e2e-api-${process.pid}`, "--listen", "127.0.0.1:22230"], { stdio: "ignore" }));
  return until("tether-api", () => fetch(API + "/api/policy", { signal: AbortSignal.timeout(3000) }).then((r) => r.status === 404), 30000, 500);
}
async function startManage() {
  const dir = join(work, "manage");
  mkdirSync(dir);
  copyFileSync(join(VAULT, "keys.json"), join(dir, "keys.json"));
  writeFileSync(join(dir, "settings.json"), JSON.stringify({ server: API }));
  hidden(MANAGE, "", { TETHER_MANAGE_DIR: dir, WEBVIEW2_USER_DATA_FOLDER: join(dir, "wv2"), WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9481" });
  const b = await until("manage devtools", () => chromium.connectOverCDP("http://127.0.0.1:9481"), 60000, 500);
  const p = await until("manage page", () => pages(b).find((x) => !x.url().startsWith("devtools")), 30000);
  await until("manage ready", () => p.evaluate(() => !!window.__TAURI_INTERNALS__), 30000);
  await ipc(p, "keys_unlock", { passphrase: PASS });
  await p.reload();
  return p;
}

// ---- Tether ----
const profileDir = (profile) => join(process.env.APPDATA, `Tether-${profile}`);
function freshProfile(profile, id) {
  rmSync(profileDir(profile), { recursive: true, force: true });
  mkdirSync(join(profileDir(profile), "config"), { recursive: true });
  writeFileSync(join(profileDir(profile), "config", `Tether-${profile}.toml`), `id = '${id}'\n`);
}
let cdpPort = 9490;
/** Starts a viewer to the host; `ready(page)` decides when its session is up. Returns { page, pid }. */
async function viewer(id, extra, match, ready) {
  const port = cdpPort++;
  const env = { TETHER_PROFILE: "au-view", TETHER_DEV_API: API, TETHER_E2E_PW: PW, WEBVIEW2_USER_DATA_FOLDER: join(work, `wv2-${port}`), WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` };
  const pid = hidden(TECH, `--connect ${id} --password-env TETHER_E2E_PW${extra}`, env);
  const b = await until("viewer devtools", () => chromium.connectOverCDP(`http://127.0.0.1:${port}`), 60000, 500);
  const page = await until("session page", () => pages(b).find((p) => p.url().includes(match)), 30000);
  await until("session up", () => ready(page), 60000, 300);
  return { page, pid, b };
}

async function main() {
  for (const f of [TECH, MANAGE, join(SBIN, "hbbs.exe"), join(SBIN, "hbbr.exe")]) if (!existsSync(f)) throw new Error(`missing ${f}`);
  console.log(`work dir ${work}`);
  await startRelay();
  await startApi();

  const hostId = String(710000000 + Math.floor(Math.random() * 9999999));
  const viewId = String(720000000 + Math.floor(Math.random() * 9999999));
  freshProfile("au-host", hostId);
  freshProfile("au-view", viewId);
  hidden(TECH, "", { TETHER_PROFILE: "au-host", TETHER_DEV_API: API, TETHER_DEV_PASSWORD: PW, WEBVIEW2_USER_DATA_FOLDER: join(work, "wv2-host"), WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9482" }, join(work, "host.log"));
  const hb = await until("host devtools", () => chromium.connectOverCDP("http://127.0.0.1:9482"), 60000, 500);
  const h = await until("host page", () => pages(hb).find((p) => p.url().includes("index.html")), 30000);
  await until("host online", async () => {
    const st = await ipc(h, "status");
    return st.status === 1 && st.key_confirmed && st.id === hostId;
  }, 90000, 500);
  await sleep(4000); // dev password is set 3 s after start
  const t0 = Math.floor(Date.now() / 1000);

  // 1. Remote session, ~8 s, closed by the viewer.
  const remote = await viewer(hostId, "", "/session/", (s) =>
    s.evaluate(() => document.querySelector("canvas")?.style.opacity === "1" && !!document.querySelector('[title^="End-to-end encrypted"]')));
  check("remote session connected", true);
  await sleep(8000);
  await remote.b.close().catch(() => {});
  kill(remote.pid);

  // 2. File transfer session: one upload (host receives) and one download (host sends).
  const dir = join(work, "files");
  for (const d of ["view", "host"]) mkdirSync(join(dir, d), { recursive: true });
  const up = randomBytes(64 * 1024 + 5);
  const down = randomBytes(20 * 1024 + 3);
  writeFileSync(join(dir, "view", "upload.bin"), up);
  writeFileSync(join(dir, "host", "download.bin"), down);
  const ft = await viewer(hostId, " --file-transfer", "conn=file-transfer", async (p) => (await p.getByRole("listbox").count()) === 2 && (await p.getByRole("listbox").nth(1).getByRole("option").count()) > 0);
  const key = ft.page.url().match(/session\/([0-9a-f]{32})/)[1];
  await ipc(ft.page, "fs_transfer", { key, id: 9001, path: join(dir, "view", "upload.bin"), to: join(dir, "host", "upload.bin"), fileNum: 0, hidden: false, isRemote: false });
  await until("upload on the host side", () => existsSync(join(dir, "host", "upload.bin")) && readFileSync(join(dir, "host", "upload.bin")).equals(up), 30000, 300);
  await ipc(ft.page, "fs_transfer", { key, id: 9002, path: join(dir, "host", "download.bin"), to: join(dir, "view", "download.bin"), fileNum: 0, hidden: false, isRemote: true });
  await until("download on the viewer side", () => existsSync(join(dir, "view", "download.bin")) && readFileSync(join(dir, "view", "download.bin")).equals(down), 30000, 300);
  check("file transfer: upload and download arrived byte for byte", true);
  await sleep(2000);
  await ft.b.close().catch(() => {});
  kill(ft.pid);

  // 3. The records on tether-api, read through Manage (signed admin request).
  const m = await startManage();
  const want = ["connect", "login", "disconnect", "receive", "send"];
  const events = await until("audit records on tether-api", async () => {
    const r = await ipc(m, "audit_all", { since: t0 - 60, limit: 5000 });
    const mine = r.events.filter((e) => e.id === hostId);
    const n = (a) => mine.filter((e) => e.action === a).length;
    return n("connect") >= 2 && n("login") >= 2 && n("disconnect") >= 2 && n("receive") >= 1 && n("send") >= 1 ? mine : null;
  }, 90000, 2000).catch(() => null);
  check("tether-api stored connect, login, disconnect (x2), receive and send for the host", events, events && events.map((e) => e.action).join(" "));
  if (events) {
    const login = events.filter((e) => e.action === "login");
    check("login records name the viewer and the connection kinds", login.every((e) => e.peer_id === viewId) && login.map((e) => e.conn_type).sort().join() === "file_transfer,remote", login.map((e) => `${e.peer_id}/${e.conn_type}`).join(" "));
    const rec = events.find((e) => e.action === "receive");
    const snd = events.find((e) => e.action === "send");
    check("file records carry the paths", rec.path === join(dir, "host", "upload.bin") && snd.path === join(dir, "host", "download.bin"), `${rec.path} | ${snd.path}`);
    // WSL and Windows clocks may differ by a few seconds.
    check("server timestamps, inside the test window", events.every((e) => e.ts >= t0 - 5 && e.ts <= Date.now() / 1000 + 5), events.map((e) => e.ts - t0).join(","));
    check("each record stored once (nonce dedupe on retries)", new Set(events.map((e) => e.nonce)).size === events.length);
  }
  const hostLog = readFileSync(join(work, "host.log"), "utf8");
  check("host log has no failed or dropped audit posts", /Connection closed/.test(hostLog) && !/Audit post (dropped|failed)/.test(hostLog), (hostLog.match(/Audit post .*/) || [`${hostLog.length} bytes of log`])[0].slice(0, 160));

  // 4. Manage › Sessions.
  await m.getByRole("button", { name: "Sessions" }).click();
  const rows = m.getByTestId("session-row");
  await until("session rows", async () => (await rows.count()) >= 8, 30000).catch(() => {});
  const texts = await rows.allInnerTexts();
  const mine = texts.filter((t) => t.includes(hostId));
  check("Sessions lists the host's events", mine.length >= 8, `${mine.length} rows`);
  check("Sessions: technician ID on every row of the host", mine.every((t) => t.includes(viewId)));
  check("Sessions: technician IP on every row of the host", mine.every((t) => /(^|\s)\d+\.\d+\.\d+\.\d+(\s|$)/.test(t)));
  check("Sessions: actions and file paths", ["Connected", "Session started", "Disconnected", "Files received from the technician", "Files sent to the technician"].every((a) => mine.some((t) => t.includes(a))) && mine.some((t) => t.includes("upload.bin")) && mine.some((t) => t.includes("download.bin")));
  const dur = mine.filter((t) => t.includes("Disconnected")).map((t) => t.match(/(\d+ h )?(\d+ min )?\d+ s\s*$|\d+ h \d+ min\s*$/)?.[0].trim());
  check("Sessions: duration on both disconnects", dur.length >= 2 && dur.every(Boolean), dur.join(", "));
  await m.getByRole("textbox", { name: "Search sessions" }).fill(viewId.replace(/(\d{3})(?=\d)/g, "$1 "));
  await sleep(300);
  check("Sessions: search by technician ID (spaces ignored)", (await rows.count()) === mine.length, `${await rows.count()}`);
  check("Sessions: Export CSV is enabled", await m.getByRole("button", { name: "Export CSV" }).isEnabled());
  await m.screenshot({ path: join(work, "sessions.png") });
  console.log(`  screenshot ${join(work, "sessions.png")}`);
}

try {
  await main();
} catch (e) {
  console.error("audit e2e failed:", e.message);
  results.push({ name: e.message, ok: false });
} finally {
  for (const p of pids) kill(p);
  for (const p of children) p.kill();
  try {
    execFileSync("wsl", ["-d", "Ubuntu-22.04", "--", "pkill", "-f", `audit-e2e-api-${process.pid}`], { stdio: "ignore" });
  } catch {}
  const failed = results.filter((r) => !r.ok).length;
  console.log(`\n${results.length - failed}/${results.length} checks passed`);
  process.exitCode = failed ? 1 : 0;
}

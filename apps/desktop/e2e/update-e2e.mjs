// Update delivery end to end (docs/update-design.md, docs/updates.md), everything on this PC.
// Published test versions are 0.9.x, above the version of this checkout.
// - hbbs + hbbr + tether-api from scripts/build-relay.sh (WSL, ~/updimpl-build/bin), hbbs on
//   127.0.0.1:22116 with key A of keyrot-e2e, watching tether-api's release-generation file
//   (RELEASE_HINT_SPREAD=10 so the test does not wait a minute per step);
// - a small reverse proxy in this script stands in for Caddy (/api/* and releases.json ->
//   tether-api, other /download/* files from tether-api's data dir); it can also misbehave like a
//   compromised relay or CDN: tampered index, replayed older index, swapped (downgraded) binary;
// - Tether Manage (copy of the test vault C:\vms\manage-test) publishes through the proxy;
// - tether.exe (debug build of THIS checkout's HEAD: TETHER_SERVER=127.0.0.1:22116, key A, test
//   vault keys; TETHER_DEV_API = the proxy), and Tether Manage from the same HEAD, both in TARGET
//   (default C:\cargo-target\update-e2e; e.g. /c/vms/night/build-e2e.sh update-e2e 22116 desktop,
//   then the same with manage, after committing). An exe older than the last commit to its code
//   stops the run (an old prebuilt client once failed the superseded-download check); STALE_OK=1
//   skips that check. Host "up-host"; a 0.5.23 build (keyrot-e2e's, without ReleaseHint in its
//   proto) as host "up-old"; a viewer "up-view" for the running session.
// Checks: the hbbs push reaches the running client within ~1 min and it fetches and verifies the
// signed index; the rollout bucket is respected; the update is downloaded, verified and staged;
// halt and resume; a custom ramp (as "Publish the latest builds" sends it) is signed into the index
// and honoured; a tampered index, an older serial and a downgraded binary are refused; no
// install while a session runs, install once idle; the old client ignores the hint.
// A second relay (tether-api on 22140 behind a second proxy, 22141) is in Manage's relay list:
// when its upload fails it keeps its old index (no index pointing at files it lacks), and the
// client on the first relay still updates. Finally an admin key rotation: a second Tether Manage
// with a new vault (built with the old key in TETHER_ADMIN_PUB_KEY) publishes TetherQS and the new
// index keeps the release signed with the old key; relay B, still without the 0.9.25 files, does
// not get it until "Publish to missing relays" (release_sync) copies them, then a publish reaches
// both.
//
//   node e2e/update-e2e.mjs
// Env: TARGET, TECH_EXE, OLD_EXE, MANAGE_EXE, STALE_OK, BIN (WSL dir with hbbs/hbbr/tether-api), KEY_A, VAULT.
import { execFileSync, spawn } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { closeSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, openSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createServer, request } from "node:http";
import { chromium } from "playwright-core";

const T = process.env.TARGET || "C:\\cargo-target\\update-e2e";
const TECH = process.env.TECH_EXE || `${T}\\debug\\tether.exe`;
const OLD = process.env.OLD_EXE || "C:\\cargo-target\\keyrot\\debug\\tether.exe";
const MANAGE = process.env.MANAGE_EXE || `${T}\\debug\\tether-manage.exe`;
const BIN = process.env.BIN || "/home/thomas/updimpl-build/bin";
const KEY_A = process.env.KEY_A || "C:\\cargo-target\\keyrot\\e2e\\keyA";
const VAULT = process.env.VAULT || "C:\\vms\\manage-test";
const HIDDEN = resolve(import.meta.dirname, "hidden-run.ps1");
const DESK = "tether-update-test";
const API = "http://127.0.0.1:22130";
const PROXY = "http://127.0.0.1:22131";
const API_B = "http://127.0.0.1:22140";
const PROXY_B = "http://127.0.0.1:22141";
const HBBS_METRICS = "http://127.0.0.1:22133/metrics";
const SPREAD = 10;
const PW = "Up-Pass-" + randomBytes(4).toString("hex") + "!";
const PASS = readFileSync(join(VAULT, "test-passphrase.txt"), "utf8").trim();
const ADMIN_PUB = readFileSync(join(VAULT, "env.txt"), "utf8").trim().split(/\s+/)[0];
const work = mkdtempSync(join(tmpdir(), "update-e2e-"));
const wslPath = (p) => "/mnt/" + p[0].toLowerCase() + p.slice(2).replace(/\\/g, "/");
const apiDir = join(work, "api");
const apiDirB = join(work, "api-b");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const pages = (b) => b.contexts().flatMap((c) => c.pages());
const ipc = (p, cmd, args) => p.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a), [cmd, args]);
const ipcErr = (p, cmd, args) => ipc(p, cmd, args).then(() => null, (e) => String(e.message ?? e));
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

function hidden(exe, args, env) {
  const a = ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", HIDDEN, "-Desktop", DESK, "-Exe", exe];
  if (args) a.push("-Args", args);
  const out = join(work, `pid-${pids.length}.txt`);
  const fd = openSync(out, "w");
  execFileSync("powershell", a, { env: { ...process.env, ...env }, stdio: ["ignore", fd, "inherit"] });
  closeSync(fd);
  const pid = +readFileSync(out, "utf8").trim().split(/\s+/).pop();
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
const wsl = (args, env = []) => spawn("wsl", ["-d", "Ubuntu-22.04", "--", "env", ...env, ...args], { stdio: ["ignore", "pipe", "pipe"] });

// ---- relay (hbbs + hbbr) and tether-api in WSL ----
let relayOut = "";
function startServers() {
  const rdir = `/tmp/update-e2e-relay-${process.pid}`;
  execFileSync("wsl", ["-d", "Ubuntu-22.04", "--", "bash", "-c", `rm -rf ${rdir} && mkdir -p ${rdir} && cp ${wslPath(KEY_A)}/id_ed25519* ${rdir}/`]);
  mkdirSync(apiDir);
  mkdirSync(apiDirB);
  const gen = `${wslPath(apiDir)}/release-generation`;
  startApis(ADMIN_PUB);
  const hbbr = wsl(["bash", "-c", `cd ${rdir} && exec ${BIN}/hbbr -p 22117 -k _`], ["HEALTH_PORT=0", "METRICS_PORT=22134"]);
  const hbbs = wsl(["bash", "-c", `cd ${rdir} && exec ${BIN}/hbbs -p 22116 -r 127.0.0.1:22117 -k _`], [
    "HEALTH_PORT=22132",
    "METRICS_PORT=22133",
    `RELEASE_GENERATION_FILE=${gen}`,
    `RELEASE_HINT_SPREAD=${SPREAD}`,
  ]);
  for (const p of [hbbr, hbbs]) watch(p);
}
function watch(p) {
  p.stdout.on("data", (d) => (relayOut += d));
  p.stderr.on("data", (d) => (relayOut += d));
  children.push(p);
}
/** Both tether-api instances (relay A and B), trusting `keys`. */
function startApis(keys) {
  watch(wsl([`${BIN}/tether-api`, "--data", wslPath(apiDir), "--listen", "127.0.0.1:22130"], [`ADMIN_PUB_KEY=${keys}`, "RELAY_HOST=127.0.0.1,localhost"]));
  watch(wsl([`${BIN}/tether-api`, "--data", wslPath(apiDirB), "--listen", "127.0.0.1:22140"], [`ADMIN_PUB_KEY=${keys}`, "RELAY_HOST=127.0.0.1,localhost"]));
}
async function restartApis(keys) {
  try {
    execFileSync("wsl", ["-d", "Ubuntu-22.04", "--", "pkill", "-f", `${BIN}/tether-api --data`], { stdio: "ignore" });
  } catch {}
  await sleep(1000);
  startApis(keys);
  for (const a of [API, API_B]) await until(`tether-api ${a}`, () => fetch(a + "/api/policy", { signal: AbortSignal.timeout(3000) }).then((r) => r.status !== 502), 30000, 500);
}
const indexOf = async (api) => {
  const r = await fetch(`${api}/download/stable/releases.json`);
  if (r.status !== 200) return null;
  const doc = await r.json();
  return { key: doc.key, ...JSON.parse(Buffer.from(doc.payload, "base64")) };
};

// ---- relay B's Caddy stand-in: can refuse release file uploads (a full disk, a broken node) ----
let failB = false;
let proxyB;
function startProxyB() {
  proxyB = createServer((req, res) => {
    const path = req.url.split("?")[0];
    const file = /^\/api\/admin\/release\//.test(path) && !/\/(releases|latest)\.json$/.test(path);
    if (failB && req.method === "PUT" && file) {
      req.resume();
      return res.writeHead(507, { "Content-Type": "text/plain" }).end("insufficient storage");
    }
    if (path.startsWith("/download/") && !/^\/download\/[a-z0-9-]+\/releases\.json$/.test(path)) {
      const rel = decodeURIComponent(path).replace(/^\/download\//, "");
      const f = join(apiDirB, "download", ...rel.split("/"));
      if (rel.includes("..") || !existsSync(f)) return res.writeHead(404).end();
      return res.writeHead(200, { "Content-Type": "application/octet-stream" }).end(readFileSync(f));
    }
    const up = request({ host: "127.0.0.1", port: 22140, path: req.url, method: req.method, headers: req.headers }, (r) => {
      res.writeHead(r.statusCode, r.headers);
      r.pipe(res);
    });
    up.on("error", () => res.writeHead(502).end("bad gateway"));
    req.pipe(up);
  });
  return new Promise((r) => proxyB.listen(22141, "127.0.0.1", r));
}
const metric = async (name) => {
  const t = await fetch(HBBS_METRICS, { signal: AbortSignal.timeout(3000) }).then((r) => r.text());
  return t.split("\n").filter((l) => l.startsWith(name)).reduce((s, l) => s + +l.split(" ").pop(), 0);
};

// ---- Caddy stand-in, optionally misbehaving ----
let mode = "pass";
let oldIndex = null; // the first index ever served, for the replay test
let swapFrom = null; // [requested file, file served instead]
let proxy;
function startProxy() {
  proxy = createServer(async (req, res) => {
    const path = req.url.split("?")[0];
    const toApi = path.startsWith("/api/") || /^\/download\/[a-z0-9-]+\/releases\.json$/.test(path);
    if (toApi) {
      if (path.endsWith("/releases.json") && mode !== "pass") {
        // Answer as a compromised relay would, without asking tether-api.
        const r = await fetch(API + path).catch(() => null);
        let body = r && r.status === 200 ? Buffer.from(await r.arrayBuffer()) : null;
        if (mode === "replay") body = oldIndex;
        if (mode === "tamper" && body) {
          const doc = JSON.parse(body);
          const p = JSON.parse(Buffer.from(doc.payload, "base64"));
          p.serial += 100;
          for (const e of Object.values(p.releases)) e.version = "9.9.9";
          doc.payload = Buffer.from(JSON.stringify(p)).toString("base64");
          body = Buffer.from(JSON.stringify(doc));
        }
        res.writeHead(body ? 200 : 404, { "Content-Type": "application/json" });
        return res.end(body ?? "");
      }
      const up = request({ host: "127.0.0.1", port: 22130, path: req.url, method: req.method, headers: req.headers }, (r) => {
        if (path.endsWith("/releases.json") && r.statusCode === 200 && !oldIndex) {
          const chunks = [];
          r.on("data", (c) => chunks.push(c));
          r.on("end", () => {
            oldIndex = Buffer.concat(chunks);
            res.writeHead(200, r.headers);
            res.end(oldIndex);
          });
          return;
        }
        res.writeHead(r.statusCode, r.headers);
        r.pipe(res);
      });
      up.on("error", () => {
        res.writeHead(502);
        res.end("bad gateway");
      });
      return req.pipe(up);
    }
    if (path.startsWith("/download/")) {
      let rel = decodeURIComponent(path).replace(/^\/download\//, "");
      if (rel.includes("..")) return res.writeHead(400).end();
      if (swapFrom && rel.endsWith(swapFrom[0])) rel = rel.replace(swapFrom[0], swapFrom[1]); // a relay serving an older signed build
      const f = join(apiDir, "download", ...rel.split("/"));
      if (!existsSync(f)) return res.writeHead(404).end();
      res.writeHead(200, { "Content-Type": "application/octet-stream" });
      return res.end(readFileSync(f));
    }
    res.writeHead(404).end();
  });
  return new Promise((r) => proxy.listen(22131, "127.0.0.1", r));
}

// ---- Tether Manage ----
/** `fresh`: a new vault (a new admin key) instead of a copy of the test vault. */
async function startManage(name = "manage", port = 9481, fresh = false) {
  const dir = join(work, name);
  mkdirSync(dir);
  if (!fresh) copyFileSync(join(VAULT, "keys.json"), join(dir, "keys.json"));
  writeFileSync(join(dir, "settings.json"), JSON.stringify({ server: `${PROXY},${PROXY_B}` }));
  hidden(MANAGE, "", { TETHER_MANAGE_DIR: dir, WEBVIEW2_USER_DATA_FOLDER: join(dir, "wv2"), WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` });
  const b = await until(`${name} devtools`, () => chromium.connectOverCDP(`http://127.0.0.1:${port}`), 60000, 500);
  const p = await until(`${name} page`, () => pages(b).find((x) => !x.url().startsWith("devtools")), 30000);
  await until(`${name} ready`, () => p.evaluate(() => !!window.__TAURI_INTERNALS__), 30000);
  if (fresh) await ipc(p, "keys_create", { passphrase: PW });
  else await ipc(p, "keys_unlock", { passphrase: PASS });
  return p;
}

// ---- Tether clients ----
const cfg = (profile, f = "") => join(process.env.APPDATA, `Tether-${profile}`, "config", f);
function freshProfile(profile, id) {
  rmSync(join(process.env.APPDATA, `Tether-${profile}`), { recursive: true, force: true });
  mkdirSync(cfg(profile), { recursive: true });
  if (id) writeFileSync(cfg(profile, `Tether-${profile}.toml`), `id = '${id}'\n`);
}
async function startHost(exe, profile, id, port) {
  freshProfile(profile, id);
  const pid = hidden(exe, "", { TETHER_PROFILE: profile, TETHER_DEV_API: PROXY, TETHER_DEV_PASSWORD: PW, WEBVIEW2_USER_DATA_FOLDER: join(work, `wv2-${profile}`), WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` });
  const b = await until(`${profile} devtools`, () => chromium.connectOverCDP(`http://127.0.0.1:${port}`), 60000, 500);
  const h = await until(`${profile} page`, () => pages(b).find((p) => p.url().includes("index.html") || p.url().startsWith("tauri") || p.url().startsWith("http://tauri")), 30000);
  await until(`${profile} online`, async () => {
    const st = await ipc(h, "status");
    return st.status === 1 && st.key_confirmed && st.id === id;
  }, 120000, 500);
  return { pid, h };
}

/** A viewer session to `id`, held open until `close()`. */
async function holdSession(id) {
  freshProfile("up-view");
  const port = 9490;
  const pid = hidden(TECH, `--connect ${id} --password-env TETHER_E2E_PW`, { TETHER_PROFILE: "up-view", TETHER_E2E_PW: PW, WEBVIEW2_USER_DATA_FOLDER: join(work, "wv2-view"), WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` });
  const b = await until("viewer devtools", () => chromium.connectOverCDP(`http://127.0.0.1:${port}`), 60000, 500);
  const s = await until("session page", () => pages(b).find((p) => p.url().includes("/session/")), 30000);
  await until("session connected", () => s.evaluate(() => document.querySelector("canvas")?.style.opacity === "1"), 60000, 300);
  return { close: async () => (await b.close().catch(() => {}), kill(pid), await sleep(2000)) };
}

/** The rollout bucket the client computes (tether_trust::release::bucket). */
function bucket(uuid, app, version) {
  const h = createHash("sha256");
  for (const p of ["tether-rollout", uuid, app, version]) h.update(Buffer.concat([Buffer.from(p), Buffer.from([0])]));
  const d = h.digest();
  return ((d[0] << 8) | d[1]) / 655.36;
}

async function main() {
  for (const f of [TECH, OLD, MANAGE]) if (!existsSync(f)) throw new Error(`missing ${f}`);
  // The client and Manage under test must be built from this checkout's HEAD (OLD is old on purpose).
  // Per exe the newest commit that changed its code (not docs, e2e scripts, tools or the other app).
  // shortcut: compares file times, so build after committing; embed the commit id if this misfires.
  for (const [f, other] of [[TECH, "apps/manage"], [MANAGE, "apps/desktop"]]) {
    const code = ["log", "-1", "--format=%ct", "--", ".", ":!docs", ":!*.md", ":!apps/desktop/e2e", ":!tools", `:!${other}`];
    const head = Number(execFileSync("git", code, { cwd: resolve(import.meta.dirname, "..", "..", ".."), encoding: "utf8" }).trim()) * 1000;
    if (statSync(f).mtimeMs < head && !process.env.STALE_OK) throw new Error(`${f} is older than this checkout's last commit to its code (${new Date(head).toISOString()}): build it from HEAD after committing (see the header) or set STALE_OK=1`);
  }
  console.log(`work dir ${work}`);
  startServers();
  await until("tether-api", () => fetch(API + "/api/policy", { signal: AbortSignal.timeout(3000) }).then((r) => r.status === 404), 30000, 500);
  await until("hbbs", () => relayOut.includes("Listening on websocket"), 30000);
  await until("tether-api B", () => fetch(API_B + "/api/policy", { signal: AbortSignal.timeout(3000) }).then((r) => r.status === 404), 30000, 500);
  await startProxy();
  await startProxyB();
  const m = await startManage();

  // The installer of the "new" releases: a harmless exe (the NSIS step runs it and the app exits).
  const art = (v) => {
    const f = join(work, `Tether_${v}_x64-setup.exe`);
    copyFileSync(join(process.env.SystemRoot, "System32", v === "0.9.25" ? "whoami.exe" : "hostname.exe"), f);
    return f;
  };
  const publish = (v, rollout) =>
    ipc(m, "release_publish", { args: { file: art(v), kind: "technician", platform: "windows-x86_64", channel: "stable", version: v, notes: `e2e ${v}`, mandatory: false, minimum_version: "", forced: false, rollout, security: false } });
  const KEY = "technician/windows-x86_64";

  const hostId = String(710000000 + Math.floor(Math.random() * 9999999));
  const oldId = String(720000000 + Math.floor(Math.random() * 9999999));
  const { pid: hostPid, h } = await startHost(TECH, "up-host", hostId, 9482);
  const { pid: oldPid, h: old } = await startHost(OLD, "up-old", oldId, 9483);
  const st0 = await until("first update check", async () => {
    const s = await ipc(h, "update_status");
    return s && s.phase ? s : null;
  }, 90000, 1000);
  check("client starts with no release published", !st0.version && !st0.rollout, JSON.stringify(st0).slice(0, 120));
  await sleep(3000);

  // 1. Publish 0.9.24 (Standard ramp) and wait for the push. No update_check from here on.
  const uuid = readFileSync(cfg("up-host", "install-uuid"), "utf8").trim();
  const b24 = bucket(uuid, "technician", "0.9.24");
  const hintsBefore = await metric("hbbs_release_hints_sent_total");
  const t0 = Date.now();
  const pub = await publish("0.9.24", "standard");
  const relayRes = (out) => out.filter((r) => r.server === PROXY || r.server === PROXY_B);
  check("Manage publishes 0.9.24 with a signed index to both relays", relayRes(pub).length === 2 && relayRes(pub).every((r) => r.ok), JSON.stringify(pub).slice(0, 160));
  const idx = await fetch(`${API}/download/stable/releases.json`).then((r) => r.json());
  const payload = JSON.parse(Buffer.from(idx.payload, "base64"));
  check("tether-api serves the signed index (serial, Standard ramp)", payload.serial >= 1 && payload.releases[KEY]?.rollout.steps.length === 4, `serial ${payload.serial}`);
  const seen = await until("push reaches the client", async () => {
    const s = await ipc(h, "update_status");
    return s.serial === payload.serial ? s : null;
  }, 120000, 1000).catch(() => null);
  const secs = Math.round((Date.now() - t0) / 1000);
  check("the hbbs push reaches the running client, which fetches and verifies the index (< 75 s)", seen && secs < 75, `${secs} s`);
  check("hbbs sent ReleaseHint frames (both hosts)", (await metric("hbbs_release_hints_sent_total")) - hintsBefore >= 2, `+${(await metric("hbbs_release_hints_sent_total")) - hintsBefore}`);
  check("the release generation file drives hbbs", (await metric("hbbs_release_generation")) >= 2);
  const inRamp = b24 < 1;
  check(`rollout bucket respected (bucket ${b24.toFixed(2)} vs 1 % at the first step)`, inRamp ? seen?.version === "0.9.24" : seen?.phase === "waiting-rollout" && !seen.version && seen.rollout?.version === "0.9.24", seen && `${seen.phase} ${seen.version}`);
  const oldSt = await ipc(old, "status");
  check("the 0.5.23 client ignores the ReleaseHint (still online, still running)", oldSt.status === 1 && alive(oldPid));

  // 2. Everyone now: eligible -> downloaded, verified and staged in the background.
  await ipc(m, "release_control", { channel: "stable", key: KEY, action: "ramp", preset: "now" });
  const staged = await until("staged", async () => {
    const s = await ipc(h, "update_status");
    return s.phase === "staged" && s.version === "0.9.24" ? s : null;
  }, 120000, 1000).catch(() => null);
  check("after 'Everyone now' the client downloads, verifies and stages 0.9.24", staged && existsSync(cfg("up-host", "update-0.9.24.staged")));
  const floor = () => readFileSync(cfg("up-host", "releases-stable.serial"), "utf8").trim().split(" ")[0];
  check("the client keeps the serial floor", +floor() >= payload.serial + 1, floor());

  // 3. A running session: no automatic install.
  const sess = await holdSession(hostId);
  const busy = await until("busy", async () => (await ipc(h, "update_status")).busy, 30000, 500).catch(() => false);
  check("while a session runs the client reports busy", busy);
  const refused = await ipcErr(h, "update_install", { auto: true });
  check("the forced/automatic install is refused during the session", refused?.includes("busy:"), refused);
  check("the client is still running", alive(hostPid));

  // 4. Halt and resume.
  await ipc(m, "release_control", { channel: "stable", key: KEY, action: "halt" });
  const halted = await until("halted", async () => ((await ipc(h, "update_status")).phase === "halted" ? true : null), 90000, 1000).catch(() => false);
  check("halt reaches the client (push) and stops the offer", halted);
  const noInstall = await ipcErr(h, "update_install", { auto: false });
  check("installing a halted release is refused (re-checked before install)", /halted|No update/.test(noInstall ?? ""), noInstall);
  await ipc(m, "release_control", { channel: "stable", key: KEY, action: "resume" });
  check("resume offers it again", await until("resumed", async () => ((await ipc(h, "update_status")).version === "0.9.24" ? true : null), 90000, 1000).catch(() => false));

  // 5. A compromised relay: tampered index, replayed older index.
  const serialNow = +floor();
  mode = "tamper";
  await ipc(h, "update_check").catch(() => null);
  let s = await ipc(h, "update_status");
  check("a tampered index (9.9.9, serial +100, old signature) is refused", s.version === "0.9.24" && +floor() === serialNow, `${s.version}, floor ${floor()}`);
  mode = "replay";
  await ipc(h, "update_check").catch(() => null);
  s = await ipc(h, "update_status");
  check("a replayed older index (lower serial) is refused", s.serial === serialNow && +floor() === serialNow, `serial ${s.serial}`);
  mode = "pass";

  // 6. A downgraded binary: 0.9.25 published, but the relay serves the 0.9.24 installer for it.
  // ... and relay B cannot store the 0.9.25 files: it must keep its old index.
  swapFrom = ["Tether_0.9.25_x64-setup.exe", "Tether_0.9.24_x64-setup.exe"];
  // ... published as "Publish the latest builds" does, with a custom ramp whose first wave just
  // covers this client's bucket (the client must honour the signed steps).
  const b25 = bucket(uuid, "technician", "0.9.25");
  const first = Math.min(100, Math.floor(b25) + 1);
  const custom = first < 100 ? `custom:0=${first},0.5=100` : "custom:0=100";
  const bBefore = await indexOf(API_B);
  failB = true;
  const pub25 = await publish("0.9.25", custom);
  failB = false;
  const rB = pub25.find((r) => r.server === PROXY_B);
  check("relay B's failed upload is reported, with 'index not updated'", rB && !rB.ok && /index not updated/.test(rB.error ?? ""), rB?.error);
  check("relay A took 0.9.25", pub25.find((r) => r.server === PROXY)?.ok);
  const bAfter = await indexOf(API_B);
  check("relay B keeps its old index (same serial, still 0.9.24: no index pointing at files it lacks)", bAfter && bAfter.serial === bBefore.serial && bAfter.releases[KEY]?.version === "0.9.24", `serial ${bBefore?.serial} -> ${bAfter?.serial}, ${bAfter?.releases[KEY]?.version}`);
  const aAfter = await indexOf(API);
  check("relay A's index has 0.9.25 with a higher serial than B's", aAfter.releases[KEY]?.version === "0.9.25" && aAfter.serial > bAfter.serial, `A ${aAfter.serial}, B ${bAfter.serial}`);
  const steps25 = aAfter.releases[KEY]?.rollout.steps;
  check(`the signed index carries the custom ramp ${custom}`, JSON.stringify(steps25) === JSON.stringify(first < 100 ? [[0, first], [1800, 100]] : [[0, 100]]), JSON.stringify(steps25));
  check("Manage refuses a custom ramp that does not end at 100 %", /100 %/.test((await ipcErr(m, "release_publish", { args: { file: art("0.9.25"), kind: "technician", platform: "windows-x86_64", channel: "stable", version: "0.9.25", notes: "", mandatory: false, minimum_version: "", forced: false, rollout: "custom:0=5,1=50", security: false } })) ?? ""));
  const failed = await until("0.9.25 refused", async () => {
    const st = await ipc(h, "update_status");
    return st.version === "0.9.25" && st.phase === "failed" ? st : null;
  }, 120000, 1000).catch(() => null);
  check(`the client takes 0.9.25 in the first wave of the custom ramp (bucket ${b25.toFixed(2)} < ${first} %)`, failed?.version === "0.9.25", failed?.phase);
  check("a downgraded binary for 0.9.25 is refused (size/sha256 against the signed index)", failed && !existsSync(cfg("up-host", "update-0.9.25.staged")), failed?.phase);
  swapFrom = null;
  await ipc(h, "update_check");
  check("with the right file 0.9.25 is staged", await until("0.9.25 staged", () => existsSync(cfg("up-host", "update-0.9.25.staged")), 60000, 1000).catch(() => false));

  // 7. Session ends: the install goes ahead (the "installer" runs and the app exits).
  await sess.close();
  await until("idle", async () => !(await ipc(h, "update_status")).busy, 30000, 500);
  let installErr = null;
  ipc(h, "update_install", { auto: true }).catch((e) => (installErr = String(e.message ?? e)));
  const exited = await until("host exits", () => !alive(hostPid) || installErr, 60000, 1000).catch(() => false);
  check("once idle the update installs (the app hands over to the installer and exits)", exited && !alive(hostPid), installErr ?? "");
  const stagedLeft = ["0.9.24", "0.9.25"].filter((v) => existsSync(cfg("up-host", `update-${v}.staged`)));
  check("the superseded 0.9.24 download was removed by the check that found 0.9.25", !stagedLeft.includes("0.9.24"), stagedLeft.join(", ") || "none");

  // 8. Admin key rotation: a new vault on "another PC"; both relays trust the old and the new key.
  // Its first publish (TetherQS) must keep the technician release signed with the old key.
  const m2 = await startManage("manage-new", 9484, true);
  const newPub = (await ipc(m2, "state")).admin_pub;
  await restartApis(`${ADMIN_PUB},${newPub}`);
  const before = await indexOf(API);
  const qs = join(work, "TetherQS-0.9.26.exe");
  copyFileSync(join(process.env.SystemRoot, "System32", "hostname.exe"), qs);
  const pubQs = await ipc(m2, "release_publish", { args: { file: qs, kind: "customer", platform: "windows-x86_64", channel: "stable", version: "0.9.26", notes: "e2e rotation", mandatory: false, minimum_version: "", forced: false, rollout: "now", security: false } }).catch((e) => [{ ok: false, error: String(e.message ?? e) }]);
  const qsA = pubQs.find((r) => r.server === PROXY);
  const qsB = pubQs.find((r) => r.server === PROXY_B);
  check("after the admin key rotation the new Manage publishes to relay A", qsA?.ok, JSON.stringify(pubQs).slice(0, 300));
  let ixA = await indexOf(API);
  check("relay A: the new-key index keeps the old-key release and raises the serial", ixA?.key === newPub && ixA.releases["customer/windows-x86_64"]?.version === "0.9.26" && ixA.releases[KEY]?.version === "0.9.25" && ixA.serial > before.serial, ixA && `serial ${ixA.serial}, ${Object.keys(ixA.releases).join(" ")}`);
  check("relay B (no 0.9.25 files) is reported with the missing file and the hint", qsB && !qsB.ok && qsB.error?.includes("Tether_0.9.25_x64-setup.exe") && qsB.error.includes("Publish to missing relays"), qsB?.error);
  let ixB = await indexOf(API_B);
  check("relay B keeps its old index", ixB?.serial === bAfter.serial && ixB.key !== newPub, `serial ${ixB?.serial}`);
  // Publish to missing relays: release_sync copies 0.9.25 and the newest index to B.
  const sync = await ipc(m2, "release_sync", { channel: "stable", kind: "technician", platform: "windows-x86_64", version: "0.9.25" }).catch((e) => [{ ok: false, error: String(e.message ?? e) }]);
  ixB = await indexOf(API_B);
  check("Publish to missing relays brings relay B up to date", sync.find((r) => r.server === PROXY_B)?.ok && ixB?.serial === ixA.serial && ixB.releases[KEY]?.version === "0.9.25", `${JSON.stringify(sync).slice(0, 200)}; B serial ${ixB?.serial}`);
  const qs27 = join(work, "TetherQS-0.9.27.exe");
  copyFileSync(join(process.env.SystemRoot, "System32", "hostname.exe"), qs27);
  const pub27 = await ipc(m2, "release_publish", { args: { file: qs27, kind: "customer", platform: "windows-x86_64", channel: "stable", version: "0.9.27", notes: "e2e after sync", mandatory: false, minimum_version: "", forced: false, rollout: "now", security: false } }).catch((e) => [{ ok: false, error: String(e.message ?? e) }]);
  [ixA, ixB] = [await indexOf(API), await indexOf(API_B)];
  check("the next publish reaches both relays (same new-key index)", relayRes(pub27).length === 2 && relayRes(pub27).every((r) => r.ok) && ixA.serial === ixB.serial && ixB.key === newPub && ixB.releases["customer/windows-x86_64"]?.version === "0.9.27", JSON.stringify(pub27).slice(0, 200));
}

try {
  await main();
} catch (e) {
  console.error("update e2e failed:", e.message);
  if (process.env.VERBOSE) console.error(relayOut.slice(-3000));
  results.push({ name: e.message, ok: false });
} finally {
  for (const p of pids) kill(p);
  for (const p of children) p.kill();
  for (const p of [proxy, proxyB]) {
    p?.closeAllConnections?.();
    p?.close();
  }
  try {
    execFileSync("wsl", ["-d", "Ubuntu-22.04", "--", "pkill", "-f", `update-e2e-`], { stdio: "ignore" });
    execFileSync("wsl", ["-d", "Ubuntu-22.04", "--", "pkill", "-f", `${BIN}/`], { stdio: "ignore" });
  } catch {}
  if (process.env.VERBOSE) console.log(relayOut.split("\n").filter((l) => /release|hint|error/i.test(l)).slice(-30).join("\n"));
  const failed = results.filter((r) => !r.ok).length;
  console.log(`\n${results.length - failed}/${results.length} checks passed`);
  process.exitCode = failed ? 1 : 0;
}

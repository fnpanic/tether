// Diagnostics tickets end to end, on TWO local relays (the production setup): the admin opens a
// ticket in Tether Manage (on every relay), the technician app and TetherQS upload their bundle
// with that code, Manage lists the upload (whichever relay it went to), downloads it, closes,
// archives and deletes tickets. Checks the zip contents (and that no secret is in it), the
// refusals (wrong, used, expired, closed code, bad format), the 2 MiB limit, the upload cap
// (4 at once per relay) and the cases behind "diagnostics cannot be sent": the app reaches the
// second relay first, and a ticket only one relay knows. Since round 3 also: the relay holding
// the ticket is down while the other does not know the code ("could not reach", not "not valid"),
// and one code uploaded once on each relay (Manage lists, opens and deletes both uploads).
//
// - tether-api from scripts/build-relay.sh (WSL, BIN), twice, data dirs in a temp folder here;
//   a small proxy per relay stands in for Caddy;
// - Tether Manage (debug exe with the frontend) on a COPY of the test vault (C:\vms\manage-test),
//   server setting = both stand-ins, driven over CDP through window.__TAURI_INTERNALS__;
// - the technician app and TetherQS: DEBUG builds with the frontend (`npx tauri build --debug
//   --no-bundle`); TETHER_DEV_API lists the stand-ins, relay 2 first; TETHER_PROFILE keeps their
//   config and logs apart from the PC's real ones. Technician over invoke, TetherQS over its UI.
//
//   node e2e/diag-e2e.mjs
// Env: TECH_EXE, QS_EXE, MANAGE_EXE (default C:\cargo-target\diagtix\debug\…, or T for that folder), BIN, VAULT.
import { execFileSync, spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { createServer, request } from "node:http";
import { connect } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { chromium } from "playwright-core";

const T = process.env.T || "C:\\cargo-target\\diagtix\\debug";
const TECH = process.env.TECH_EXE || join(T, "tether.exe");
const QS = process.env.QS_EXE || join(T, "TetherQS.exe");
const MANAGE = process.env.MANAGE_EXE || join(T, "tether-manage.exe");
const BIN = process.env.BIN || "/home/thomas/diag-build/bin";
const VAULT = process.env.VAULT || "C:\\vms\\manage-test";
const PROFILE = "diag-e2e";
const PASS = readFileSync(join(VAULT, "test-passphrase.txt"), "utf8").trim();
const ADMIN_PUB = readFileSync(join(VAULT, "env.txt"), "utf8").trim().split(/\s+/)[0];
// Fresh ports per run: WSL's localhost forwarding can keep a port of the last run for a while.
const BASE = 22200 + 4 * Math.floor(Math.random() * 200);
const RELAYS = [
  { api: BASE, proxy: BASE + 1 },
  { api: BASE + 2, proxy: BASE + 3 },
];
const [R1, R2] = RELAYS.map((r) => `http://127.0.0.1:${r.proxy}`);
const work = mkdtempSync(join(tmpdir(), "diag-e2e-"));
const vault = join(work, "vault");
const wslPath = (p) => "/mnt/" + p[0].toLowerCase() + p.slice(2).replace(/\\/g, "/");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const results = [];
const check = (name, ok, detail = "") => {
  results.push({ name, ok: !!ok });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? `  (${detail})` : ""}`);
};
const NOT_VALID = "This code is not valid, has expired or was already used";
const UNREACHABLE = "Could not reach the support server";
const t0 = Math.floor(Date.now() / 1000);
// Valid alphabet, created two days ago on both relays: expired.
const EXPIRED = "EXPXRED2";

// ---- two tether-api in WSL, a Caddy stand-in each ----
const procs = [];
for (const [i, r] of RELAYS.entries()) {
  r.dir = join(work, `api${i + 1}`);
  mkdirSync(join(r.dir, "diag"), { recursive: true });
  writeFileSync(join(r.dir, "diag", "tickets.json"), JSON.stringify([{ ticket: EXPIRED, note: "expired", created: t0 - 2 * 86400, uploaded: null, size: null }]));
  const p = spawn("wsl", ["-d", "Ubuntu-22.04", "--", "env", `ADMIN_PUB_KEY=${ADMIN_PUB}`, "RELAY_HOST=127.0.0.1,localhost", `${BIN}/tether-api`, "--data", wslPath(r.dir), "--listen", `127.0.0.1:${r.api}`], { stdio: ["ignore", "pipe", "pipe"] });
  r.out = "";
  p.stdout.on("data", (d) => (r.out += d));
  p.stderr.on("data", (d) => (r.out += d));
  procs.push(p);
  r.server = createServer(async (req, res) => {
    if (r.down) return req.socket.destroy(); // a relay that is down (connection reset)
    const body = await new Promise((ok) => { const c = []; req.on("data", (d) => c.push(d)); req.on("end", () => ok(Buffer.concat(c))); });
    const headers = { ...req.headers };
    delete headers.host;
    try {
      const up = await fetch(`http://127.0.0.1:${r.api}${req.url}`, { method: req.method, headers, body: body.length ? body : undefined });
      const out = {};
      up.headers.forEach((v, k) => { if (!["content-length", "transfer-encoding", "connection"].includes(k)) out[k] = v; });
      res.writeHead(up.status, out).end(Buffer.from(await up.arrayBuffer()));
    } catch (e) {
      res.writeHead(502).end(String(e));
    }
  }).listen(r.proxy, "127.0.0.1");
}
const tickets = (r) => JSON.parse(readFileSync(join(r.dir, "diag", "tickets.json"), "utf8"));
const zipOn = (r, t) => existsSync(join(r.dir, "diag", `${t}.zip`));

// ---- zip contents, secrets ----
function unzip(file) {
  const dir = join(work, `unz-${randomBytes(4).toString("hex")}`);
  execFileSync("powershell", ["-NoProfile", "-Command", `Expand-Archive -LiteralPath '${file}' -DestinationPath '${dir}'`]);
  const files = {};
  const walk = (d, pre) => readdirSync(d, { withFileTypes: true }).forEach((e) => (e.isDirectory() ? walk(join(d, e.name), `${pre}${e.name}/`) : (files[`${pre}${e.name}`] = readFileSync(join(d, e.name)))));
  walk(dir, "");
  return files;
}
// Values of secret-looking keys in the app's real config files, and the planted ones.
function secretsOf(cfgDir) {
  const out = ["planted-secret-123", "planted-token-456", "planted-proxy-pass"];
  for (const f of existsSync(cfgDir) ? readdirSync(cfgDir).filter((n) => n.endsWith(".toml") && n !== "Planted.toml") : []) {
    for (const m of readFileSync(join(cfgDir, f), "utf8").matchAll(/^\s*([A-Za-z0-9_.-]+)\s*=\s*['"]([^'"]{6,})['"]/gm)) {
      if (/password|salt|key|token|secret|pin|otp|2fa/i.test(m[1]) && !/^(Y|N|true|false)$/.test(m[2])) out.push(m[2]);
    }
  }
  return out;
}
function plant(cfgDir) {
  mkdirSync(cfgDir, { recursive: true });
  writeFileSync(join(cfgDir, "Planted.toml"), 'password = "planted-secret-123"\n[options]\nrecording-upload-token = "planted-token-456"\n[socks]\nproxy = "proxy:3128"\npassword = "planted-proxy-pass"\n');
}
function checkZip(app, files, cfgDir, expect) {
  const names = Object.keys(files).sort();
  check(`${app}: zip has diagnostics.json, logs/, config/`, names.includes("diagnostics.json") && names.includes("logs/e2e.log") && names.some((n) => n.startsWith("config/")), names.join(", "));
  const info = JSON.parse(files["diagnostics.json"].toString("utf8"));
  check(`${app}: diagnostics.json is the app's, with the log's errors`, expect(info) && info.errors.some((l) => l.includes("planted log line")), JSON.stringify(info).slice(0, 200));
  check(`${app}: planted config is redacted, the proxy host kept`, /<redacted>/.test(files["config/Planted.toml"]?.toString()) && files["config/Planted.toml"].toString().includes("proxy:3128"));
  const all = Buffer.concat(Object.values(files)).toString("latin1");
  const secrets = secretsOf(cfgDir);
  const leaked = secrets.filter((s) => all.includes(s));
  check(`${app}: no secret in the zip (${secrets.length} values checked: passwords, salts, keys, tokens)`, !leaked.length, leaked.map((s) => s.slice(0, 4) + "…").join(", "));
}

// ---- CDP ----
async function cdp(port) {
  let b;
  for (let i = 0; i < 120 && !b; i++) {
    b = await chromium.connectOverCDP(`http://127.0.0.1:${port}`).catch(() => null);
    if (!b) await sleep(500);
  }
  if (!b) throw new Error(`no CDP on ${port}`);
  let page;
  for (let i = 0; i < 80 && !page; i++) {
    page = b.contexts().flatMap((c) => c.pages()).find((p) => !p.url().startsWith("devtools"));
    if (!page) await sleep(250);
  }
  await page.waitForFunction(() => !!window.__TAURI_INTERNALS__, null, { timeout: 60000 });
  const invoke = (cmd, args = {}) => page.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a), [cmd, args]);
  const invokeErr = (cmd, args) => invoke(cmd, args).then(() => null, (e) => String(e?.message ?? e));
  return { b, page, invoke, invokeErr };
}

/** Slow uploads straight to a relay's tether-api (the stand-in would buffer): they take its 4 upload slots. */
function holdSlots(r, ticket, n) {
  return Array.from({ length: n }, () => {
    const s = connect(r.api, "127.0.0.1");
    s.on("error", () => {});
    s.write(`POST /api/diag/${ticket} HTTP/1.1\r\nHost: t\r\nContent-Type: application/zip\r\nContent-Length: 100000\r\n\r\nPK`);
    return s;
  });
}
/** Status line of a request with only its head sent (the relay refuses an oversized body unread). */
const headOnly = (r, head) =>
  new Promise((ok) => {
    const s = connect(r.api, "127.0.0.1");
    let got = "";
    s.on("data", (d) => (got += d));
    s.on("error", () => {});
    s.on("close", () => ok(Number(got.split(" ")[1])));
    s.write(head);
    setTimeout(() => s.destroy(), 5000);
  });
const plantLog = (dir) => {
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, "e2e.log"), "[2026-11-08 08:00:00.000] ERROR [e2e] planted log line\n");
};
const post = (base, path, body) =>
  new Promise((ok, bad) => {
    const rq = request(`${base}${path}`, { method: "POST", headers: { "Content-Type": "application/zip", "Content-Length": body.length } }, (res) => {
      res.resume();
      res.on("end", () => ok(res.statusCode));
    });
    rq.on("error", bad);
    rq.end(body);
  });

let manage, tech, qs, M, A, Q, res9;
const kill = (p) => { try { if (p) execFileSync("taskkill", ["/PID", String(p.pid), "/T", "/F"], { stdio: "ignore" }); } catch {} };
try {
  for (const r of RELAYS) {
    for (let i = 0; i < 60 && !r.out.includes("listening"); i++) await sleep(250);
    if (!r.out.includes("listening")) throw new Error(`tether-api did not start: ${r.out}`);
  }
  mkdirSync(vault);
  for (const f of ["keys.json", "test-passphrase.txt"]) copyFileSync(join(VAULT, f), join(vault, f));
  // settings_set wants https:// relays; the stand-ins are http, so write the file.
  const setServers = (s) => writeFileSync(join(vault, "settings.json"), JSON.stringify({ server: s }));
  setServers(`${R1},${R2}`);
  manage = spawn(MANAGE, [], { env: { ...process.env, TETHER_MANAGE_DIR: vault, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9461" }, stdio: "ignore" });
  M = await cdp(9461);
  await M.invoke("keys_unlock", { passphrase: PASS });
  const open = async (note) => (await M.invoke("admin", { method: "POST", path: "/api/admin/diag-ticket", body: { note } })).ticket;
  const list = async (archived = false) => (await M.invoke("diag_list", { archived })).tickets;
  const entry = async (t, archived = false) => (await list(archived)).find((x) => x.ticket === t);
  const download = async (t, app, relay) => {
    const got = await M.invoke("diag_open", relay ? { ticket: t, relay } : { ticket: t });
    const file = join(work, `${app}-${t}.zip`);
    await M.invoke("diag_save", { path: file });
    return { got, file };
  };

  // ---------------------------------------------------------------- technician app
  const appEnv = { ...process.env, TETHER_PROFILE: PROFILE, TETHER_DEV_API: `${R2},${R1}` };
  tech = spawn(TECH, [], { env: { ...appEnv, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9462" }, stdio: "ignore" });
  A = await cdp(9462);
  const techInfo = await A.invoke("diag_info");
  const techCfg = join(techInfo.logDir, "..", "config");
  plant(techCfg);
  plantLog(techInfo.logDir);

  const t1 = await open("e2e technician");
  check("Manage opens the ticket on both relays", RELAYS.every((r) => tickets(r).some((k) => k.ticket === t1)), t1);
  let e = await A.invokeErr("diag_upload", { ticket: t1.toLowerCase().replace(/^(....)/, "$1 ") });
  check("technician: upload with the code (typed lower case, with a space)", e === null, e ?? "");
  check("technician: the upload went to relay 2 (the app's first relay), not relay 1", zipOn(RELAYS[1], t1) && !zipOn(RELAYS[0], t1));
  let row = await entry(t1);
  check("Manage lists it as uploaded with its size, from relay 2 (Manage read relay 1 only before)", row?.state === "uploaded" && row.size > 0 && row.upload_relay === R2, JSON.stringify(row));
  let dl = await download(t1, "tech");
  check("Manage downloads a valid zip of that size", dl.got.size === row.size && statSync(dl.file).size === row.size && dl.got.files.some((f) => f.name === "diagnostics.json"), `${dl.got.files.length} files, ${dl.got.size} bytes`);
  checkZip("technician", unzip(dl.file), techCfg, (i) => typeof i.version === "string" && "id" in i && Array.isArray(i.errors));

  e = await A.invokeErr("diag_upload", { ticket: t1 });
  check("technician: used code refused", e?.includes(NOT_VALID), e);
  e = await A.invokeErr("diag_upload", { ticket: "ZZZZZZZZ" });
  check("technician: unknown code refused", e?.includes(NOT_VALID), e);
  e = await A.invokeErr("diag_upload", { ticket: EXPIRED });
  check("technician: expired code (24 h) refused", e?.includes(NOT_VALID), e);
  e = await A.invokeErr("diag_upload", { ticket: "ABC" });
  check("technician: malformed code refused before sending", e?.includes("8-character code"), e);

  // Ticket only on relay 1 (Manage knew one relay, or relay 2 was down): the app's first relay
  // answers 404, the app goes on to relay 1.
  setServers(R1);
  const t3 = await open("relay 1 only");
  setServers(`${R1},${R2}`);
  check("ticket on relay 1 only", tickets(RELAYS[0]).some((k) => k.ticket === t3) && !tickets(RELAYS[1]).some((k) => k.ticket === t3));
  e = await A.invokeErr("diag_upload", { ticket: t3 });
  row = await entry(t3);
  check("technician: relay 2 says 404, the upload lands on relay 1", e === null && zipOn(RELAYS[0], t3) && row?.upload_relay === R1, e ?? JSON.stringify(row));

  // The relay with the ticket is down and the other does not know the code: the code may well be
  // valid, so the app must not call it invalid.
  setServers(R1);
  const t8 = await open("relay 1 only, then down");
  setServers(`${R1},${R2}`);
  RELAYS[0].down = true;
  e = await A.invokeErr("diag_upload", { ticket: t8 });
  check("technician: relay with the ticket down, the other 404 -> 'could not reach', not 'not valid'", e?.includes(UNREACHABLE) && !e.includes(NOT_VALID), e);
  RELAYS[0].down = false;
  e = await A.invokeErr("diag_upload", { ticket: t8 });
  check("technician: the same code works once that relay is back", e === null && zipOn(RELAYS[0], t8), e ?? "");

  // One upload per code holds per relay: a code used on relay 2 can still be uploaded once on
  // relay 1 (an app that reached relay 1 first). Manage lists both, opens each, deletes both.
  const t9 = await open("uploaded on both relays");
  e = await A.invokeErr("diag_upload", { ticket: t9 });
  // The second upload: a small valid zip of its own (a different size from the app's).
  writeFileSync(join(work, "diagnostics.json"), JSON.stringify({ app: "second upload" }));
  execFileSync("powershell", ["-NoProfile", "-Command", `Compress-Archive -LiteralPath '${join(work, "diagnostics.json")}' -DestinationPath '${join(work, "second.zip")}'`]);
  const st9 = await post(R1, `/api/diag/${t9}`, readFileSync(join(work, "second.zip")));
  row = await entry(t9);
  check("a used code is taken once more by the relay where it was still open", e === null && st9 === 204 && zipOn(RELAYS[0], t9) && zipOn(RELAYS[1], t9), `${e} ${st9}`);
  check("Manage lists both uploads of the code, newest first", row?.uploads?.length === 2 && row.uploads[0].relay === R1 && row.uploads[1].relay === R2 && row.upload_relay === R1, JSON.stringify(row?.uploads));
  const both = [await download(t9, "both-r1", R1), await download(t9, "both-r2", R2)];
  check("Manage opens each upload from its relay", both[0].got.size === row.uploads[0].size && both[1].got.size === row.uploads[1].size && both[0].got.size !== both[1].got.size, both.map((d) => d.got.size).join(" vs "));
  e = await M.invokeErr("diag_open", { ticket: t9, relay: "http://127.0.0.1:1" });
  check("Manage opens uploads only from its configured relays", e?.includes("not in the relay list"), e);
  res9 = await M.invoke("diag_delete", { ticket: t9 });
  check("delete removes both uploads", res9.every((x) => x.ok) && !zipOn(RELAYS[0], t9) && !zipOn(RELAYS[1], t9), JSON.stringify(res9));

  // Upload cap: relay 2's four slots taken by slow uploads -> 503 there, relay 1 takes it.
  const hold = await open("slot holder");
  const t4 = await open("behind the cap");
  let held = holdSlots(RELAYS[1], hold, 4);
  await sleep(700);
  check("relay 2 at its upload cap answers 503", (await post(R2, `/api/diag/${t4}`, Buffer.from("PK"))) === 503);
  e = await A.invokeErr("diag_upload", { ticket: t4 });
  row = await entry(t4);
  check("technician: upload behind relay 2's cap succeeds on relay 1", e === null && row?.upload_relay === R1, e ?? JSON.stringify(row));
  held.forEach((s) => s.destroy());
  const t5 = await open("both busy");
  held = [...holdSlots(RELAYS[0], hold, 4), ...holdSlots(RELAYS[1], hold, 4)];
  await sleep(1000);
  const probe = [await post(R1, `/api/diag/${hold}`, Buffer.from("PK")), await post(R2, `/api/diag/${hold}`, Buffer.from("PK"))];
  check("both relays at their upload cap answer 503", probe.every((x) => x === 503), probe.join(","));
  e = await A.invokeErr("diag_upload", { ticket: t5 });
  check("technician: both relays at the cap -> 'busy, try again'", e?.includes("busy"), e);
  held.forEach((s) => s.destroy());
  await sleep(1000);
  e = await A.invokeErr("diag_upload", { ticket: t5 });
  check("technician: the same code works once slots are free", e === null, e ?? "");

  // 2 MiB: incompressible logs make the bundle larger; the app sends diagnostics.json only.
  const big = [1, 2, 3].map((i) => join(techInfo.logDir, `big-e2e-${i}.log`));
  big.forEach((f) => writeFileSync(f, randomBytes(1 << 20)));
  const t6 = await open("too large");
  e = await A.invokeErr("diag_upload", { ticket: t6 });
  dl = await download(t6, "tech-big");
  const bigNames = dl.got.files.map((f) => f.name);
  check("technician: bundle over 2 MiB is sent as diagnostics.json only", e === null && bigNames.length === 1 && bigNames[0] === "diagnostics.json", e ?? bigNames.join(","));
  big.forEach((f) => rmSync(f, { force: true }));
  const t7 = await open("raw limit");
  const st = await headOnly(RELAYS[1], `POST /api/diag/${t7} HTTP/1.1\r\nHost: t\r\nContent-Type: application/zip\r\nContent-Length: ${(2 << 20) + 1}\r\n\r\n`);
  check("relay: a body over 2 MiB is refused (413), the ticket stays open", st === 413 && (await entry(t7))?.state === "open", String(st));

  // ---------------------------------------------------------------- close, archive, delete
  const t2 = await open("closed before upload");
  let res = await M.invoke("diag_close", { ticket: t2 });
  check("Manage closes on every relay (one result each)", res.length === 2 && res.every((x) => x.ok), JSON.stringify(res));
  e = await A.invokeErr("diag_upload", { ticket: t2 });
  check("technician: closed code refused like an expired one", e?.includes(NOT_VALID), e);
  res = await M.invoke("diag_close", { ticket: t1 });
  check("close the uploaded ticket", res.every((x) => x.ok));
  check("closed tickets leave the active list", !(await entry(t1)) && !(await entry(t2)));
  const a1 = await entry(t1, true);
  check("Archived lists them with note, dates, size", a1?.state === "closed" && a1.note === "e2e technician" && a1.closed >= t0 && a1.size > 0 && (await entry(t2, true))?.state === "closed", JSON.stringify(a1));
  dl = await download(t1, "tech-archived");
  check("an archived upload still downloads", dl.got.size === a1.size);
  res = await M.invoke("diag_delete", { ticket: t1 });
  check("Manage deletes on every relay", res.length === 2 && res.every((x) => x.ok), JSON.stringify(res));
  check("delete removes the entry and the zip on disk", !zipOn(RELAYS[1], t1) && RELAYS.every((r) => !tickets(r).some((k) => k.ticket === t1)) && !(await entry(t1, true)));
  res = await M.invoke("diag_delete", { ticket: t1 });
  check("delete again: done, 'not on this relay' on each", res.every((x) => x.ok && x.detail === "not on this relay"), JSON.stringify(res));
  res = await M.invoke("diag_delete", { ticket: t3 });
  check("delete a ticket one relay has", res.every((x) => x.ok) && res.filter((x) => x.detail === "not on this relay").length === 1 && !zipOn(RELAYS[0], t3), JSON.stringify(res));
  e = await M.invokeErr("diag_delete", { ticket: "../x/abcd" });
  check("Manage refuses a malformed code", e?.includes("invalid ticket"), e);
  const journal = RELAYS.map((r) => r.out).join("");
  check("relay journal has a line per close and delete", journal.includes(`diag ticket ${t2} closed`) && journal.includes(`diag ticket ${t1} deleted`));
  kill(tech);
  tech = null;

  // ---------------------------------------------------------------- TetherQS
  const qsCfg = join(process.env.APPDATA, `Tether-QS-${PROFILE}`, "config");
  const qsLog = join(process.env.APPDATA, `Tether-QS-${PROFILE}`, "log");
  plant(qsCfg);
  plantLog(qsLog);
  qs = spawn(QS, [], { env: { ...appEnv, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9463" }, stdio: "ignore" });
  Q = await cdp(9463);
  const qp = Q.page;
  await qp.getByRole("button", { name: "Help" }).click({ timeout: 30000 });
  const send = async (code) => {
    await qp.getByTestId("qs-diag-code").fill(code);
    await qp.getByRole("button", { name: "Send", exact: true }).click();
    // The click shows "Sending…" and clears the last result; wait for the new one.
    await qp.waitForFunction(() => !document.body.innerText.includes("Sending…") && document.querySelector('[data-testid="qs-diag-result"]'), null, { timeout: 60000 });
    return qp.getByTestId("qs-diag-result").innerText();
  };
  const q1 = await open("e2e TetherQS");
  let msg = await send(q1);
  check("TetherQS: Help › Send to support with the code", msg.startsWith("Sent"), msg);
  row = await entry(q1);
  check("TetherQS: upload on relay 2, listed in Manage with its size", row?.state === "uploaded" && row.size > 0 && row.upload_relay === R2, JSON.stringify(row));
  dl = await download(q1, "qs");
  checkZip("TetherQS", unzip(dl.file), qsCfg, (i) => i.app === "TetherQS" && typeof i.version === "string" && Array.isArray(i.servers));
  for (const [what, code] of [["used", q1], ["unknown", "ZZZZZZZZ"], ["expired", EXPIRED], ["closed", t2]]) {
    msg = await send(code);
    check(`TetherQS: ${what} code refused with the message`, msg.includes(NOT_VALID), msg);
  }
  e = await Q.invokeErr("diag_upload", { ticket: "ABC" });
  check("TetherQS: malformed code refused before sending", e?.includes("8-character code"), e);
  setServers(R1);
  const q3 = await open("TetherQS relay 1 only");
  setServers(`${R1},${R2}`);
  RELAYS[0].down = true;
  msg = await send(q3);
  check("TetherQS: relay with the ticket down, the other 404 -> 'could not reach', not 'not valid'", msg.includes(UNREACHABLE) && !msg.includes(NOT_VALID), msg);
  RELAYS[0].down = false;
  msg = await send(q3);
  check("TetherQS: relay 2 says 404, the upload lands on relay 1", msg.startsWith("Sent") && zipOn(RELAYS[0], q3) && (await entry(q3))?.upload_relay === R1, msg);
  const qbig = [1, 2, 3].map((i) => join(qsLog, `big-e2e-${i}.log`));
  qbig.forEach((f) => writeFileSync(f, randomBytes(1 << 20)));
  const q6 = await open("TetherQS too large");
  msg = await send(q6);
  dl = await download(q6, "qs-big");
  check("TetherQS: bundle over 2 MiB is sent as diagnostics.json only", msg.startsWith("Sent") && dl.got.files.length === 1, dl.got.files.map((f) => f.name).join(","));
  qbig.forEach((f) => rmSync(f, { force: true }));
  for (const t of [q1, q3, q6]) await M.invoke("diag_close", { ticket: t });
  check("TetherQS tickets closed and archived", (await list(true)).filter((x) => [q1, q3, q6].includes(x.ticket)).length === 3);
} catch (err) {
  check("run", false, err.stack ?? String(err));
} finally {
  for (const x of [M, A, Q]) await x?.b.close().catch(() => {});
  [manage, tech, qs].forEach(kill);
  procs.forEach((p) => p.kill());
  for (const r of RELAYS) {
    try { execFileSync("wsl", ["-d", "Ubuntu-22.04", "--", "pkill", "-f", `tether-api --data ${wslPath(r.dir)}`], { stdio: "ignore" }); } catch {}
    r.server.close();
  }
  await sleep(500);
  rmSync(work, { recursive: true, force: true });
}
const failed = results.filter((r) => !r.ok).length;
console.log(`\n${results.length - failed}/${results.length} passed`);
process.exit(failed ? 1 : 0);

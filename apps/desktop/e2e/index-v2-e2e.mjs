// Two index files end to end (docs/updates.md "Two index files"), everything on this PC:
// - two tether-api relays (WSL, BIN) behind small proxies standing in for Caddy (index files ->
//   tether-api, other /download/* from the relay's data dir; every request logged);
// - Tether Manage of this checkout (MANAGE_EXE, copy of the test vault) publishes TetherQS for
//   Windows and macOS: releases.json (gzip only) and releases-v2.json (same serial, xz copies);
// - releases.json must still be taken by the release-index code of the installed apps
//   (OLD_CHECK: tether-trust before releases-v2.json, built from 176c62c), releases-v2.json is
//   refused by it (why it is a separate file);
// - a debug TetherQS of this checkout (QS_EXE, test keys, TETHER_DEV_API = the proxies) checks for
//   updates: it reads releases-v2.json, downloads the .xz copy, unpacks and stages exactly the
//   published exe;
// - stale v2: Manage switched to releases.json only (marker file), a new version goes into v1
//   only; TetherQS takes v1 (higher serial) and the .gz copy;
// - scripts/release-index-v1.sh against these relays deletes releases-v2.json (404 at once);
//   TetherQS keeps updating from releases.json; `--on` and a publish write v2 again.
//
//   node e2e/index-v2-e2e.mjs
// Env: MANAGE_EXE, QS_EXE, OLD_CHECK, BIN (WSL dir with tether-api), VAULT, QS_FILE (an exe to
// publish, compressible), MAC_FILE (a TetherQS universal zip).
import { execFileSync, spawn } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { createServer, request } from "node:http";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { chromium } from "playwright-core";

const MANAGE = process.env.MANAGE_EXE || "C:\\cargo-target\\manage-v2\\release\\tether-manage.exe";
const QS_EXE = process.env.QS_EXE || "C:\\cargo-target\\v2dbg\\debug\\TetherQS.exe";
const OLD_CHECK = process.env.OLD_CHECK || "C:\\cargo-target\\oldtt\\debug\\examples\\index_check.exe";
const BIN = process.env.BIN || "/home/thomas/ha-build/bin";
const VAULT = process.env.VAULT || "C:\\vms\\manage-test";
const QS_FILE = process.env.QS_FILE || "C:\\install\\remote-support\\dist\\quick\\TetherQS-0.5.30.exe";
const MAC_FILE = process.env.MAC_FILE || "C:\\install\\remote-support\\dist\\macos\\TetherQS-0.5.30-universal.zip";
const MAC_TECH = process.env.MAC_TECH || "C:\\install\\remote-support\\dist\\macos\\Tether_0.5.32_universal.app.tar.gz";
const ROOT = resolve(import.meta.dirname, "../../..");
// Git Bash (a plain "bash" may be WSL's on Windows).
const BASH = process.env.BASH_EXE || "C:/Program Files/Git/bin/bash.exe";
const PASS = readFileSync(join(VAULT, "test-passphrase.txt"), "utf8").trim();
const ADMIN_PUB = readFileSync(join(VAULT, "env.txt"), "utf8").trim().split(/\s+/)[0];
const BASE = 24800 + 4 * Math.floor(Math.random() * 100);
// CDP ports of Manage and TetherQS, new each run (a leftover app keeps its port).
const CDP = 9400 + 2 * Math.floor(Math.random() * 250);
const RELAYS = [0, 1].map((i) => ({ api: BASE + 2 * i, proxy: BASE + 2 * i + 1, reqs: [] }));
const PROXIES = RELAYS.map((r) => `http://127.0.0.1:${r.proxy}`);
const work = mkdtempSync(join(tmpdir(), "index-v2-e2e-")), vault = join(work, "vault");
const wslPath = (p) => "/mnt/" + p[0].toLowerCase() + p.slice(2).replace(/\\/g, "/");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const results = [];
const check = (name, ok, detail = "") => {
  results.push({ name, ok: !!ok });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? `  (${detail})` : ""}`);
};
const INDEX = /^\/download\/[a-z0-9-]+\/releases(-v2)?\.json$/;
const kill = (p) => { try { if (p) execFileSync("taskkill", ["/PID", String(p.pid), "/T", "/F"], { stdio: "ignore" }); } catch {} };

for (const [i, r] of RELAYS.entries()) {
  r.dir = join(work, `api${i + 1}`);
  mkdirSync(r.dir, { recursive: true });
  r.proc = spawn("wsl", ["-d", "Ubuntu-22.04", "--", "env", `ADMIN_PUB_KEY=${ADMIN_PUB}`, "RELAY_HOST=127.0.0.1,localhost", `${BIN}/tether-api`, "--data", wslPath(r.dir), "--listen", `127.0.0.1:${r.api}`], { stdio: ["ignore", "pipe", "pipe"] });
  r.out = "";
  r.proc.stdout.on("data", (d) => (r.out += d));
  r.proc.stderr.on("data", (d) => (r.out += d));
  createServer((req, res) => {
    const u = req.url.split("?")[0];
    r.reqs.push(`${req.method} ${u}`);
    if (u.startsWith("/download/") && !INDEX.test(u) && (req.method === "GET" || req.method === "HEAD")) {
      try {
        const b = readFileSync(join(r.dir, "download", ...u.slice(10).split("/").filter((x) => x && x !== "..")));
        res.writeHead(200, { "Content-Length": b.length });
        return res.end(req.method === "HEAD" ? undefined : b);
      } catch {
        return res.writeHead(404).end("not found");
      }
    }
    const headers = { ...req.headers };
    delete headers.host;
    const up = request({ host: "127.0.0.1", port: r.api, path: req.url, method: req.method, headers, agent: false }, (ur) => {
      res.writeHead(ur.statusCode, ur.headers);
      ur.pipe(res);
    });
    up.on("error", (e) => res.headersSent || res.writeHead(502).end(String(e)));
    req.pipe(up);
  }).listen(r.proxy, "127.0.0.1");
}

async function cdp(port) {
  let b;
  for (let i = 0; i < 120 && !b; i++) { b = await chromium.connectOverCDP(`http://127.0.0.1:${port}`).catch(() => null); if (!b) await sleep(500); }
  if (!b) throw new Error(`no CDP on ${port}`);
  let page;
  for (let i = 0; i < 80 && !page; i++) { page = b.contexts().flatMap((c) => c.pages()).find((p) => !p.url().startsWith("devtools")); if (!page) await sleep(250); }
  await page.waitForFunction(() => !!window.__TAURI_INTERNALS__, null, { timeout: 60000 });
  return { b, page, invoke: (cmd, args = {}) => page.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a), [cmd, args]) };
}
/** A relay's index file `name` for stable: { doc (raw), payload (parsed), raw payload bytes } or null. */
const indexOf = (r, name) => {
  try {
    const raw = readFileSync(join(r.dir, "download", "stable", name), "utf8");
    const bytes = Buffer.from(JSON.parse(raw).payload, "base64");
    return { raw, bytes, ix: JSON.parse(bytes.toString()) };
  } catch {
    return null;
  }
};
/** The old apps' verdict on an index payload. */
const oldVerdict = (bytes, tag) => {
  const f = join(work, `${tag}.payload.json`);
  writeFileSync(f, bytes);
  return execFileSync(OLD_CHECK, [f]).toString().trim();
};
// python: <original> <twin> <twin.xz> <sha256 of the twin>; same tar (gzip) or same zip entries.
const TWIN_CHECK = `
import sys, lzma, hashlib, gzip, zipfile
orig, twin, xz, want = sys.argv[1:5]
t = open(twin, "rb").read()
assert lzma.decompress(open(xz, "rb").read()) == t, "xz does not unpack to the twin"
assert hashlib.sha256(t).hexdigest() == want, "twin sha256 differs from the index"
if orig.endswith(".tar.gz"):
    assert gzip.decompress(t) == gzip.decompress(open(orig, "rb").read()), "different tar"
    print("ok tar %.1f MiB, twin %.1f MiB, xz %.1f MiB" % (len(gzip.decompress(t)) / 2**20, len(t) / 2**20, len(open(xz, "rb").read()) / 2**20))
else:
    a, b = zipfile.ZipFile(orig), zipfile.ZipFile(twin)
    ea = [(i.filename, i.CRC, i.file_size, i.external_attr >> 16) for i in a.infolist()]
    eb = [(i.filename, i.CRC, i.file_size, i.external_attr >> 16) for i in b.infolist()]
    assert ea == eb, "zip entries differ"
    assert all(i.compress_type == zipfile.ZIP_STORED for i in b.infolist()), "twin not stored"
    print("ok %d entries, twin %.1f MiB, xz %.1f MiB" % (len(eb), len(t) / 2**20, len(open(xz, "rb").read()) / 2**20))
`;
const rel = (platform, v, file) => ({ args: { file, kind: "customer", platform, channel: "stable", version: v, notes: "", mandatory: false, minimum_version: "", forced: false, rollout: "now", security: false } });
const fileAs = (src, name) => { const p = join(work, "builds", name); copyFileSync(src, p); return p; };
const sha = (b) => execFileSync("powershell", ["-NoProfile", "-Command", `(Get-FileHash -Algorithm SHA256 -LiteralPath '${b}').Hash.ToLower()`]).toString().trim();

let manage, M, qs, Q;
const qsHome = join(work, "qs-home");
/** TetherQS (debug, this checkout) with its own profile, against the proxies. */
async function startQs() {
  mkdirSync(join(qsHome, "Roaming"), { recursive: true });
  mkdirSync(join(qsHome, "Local"), { recursive: true });
  // TETHER_PROFILE: its own config folder (index cache and serial floor; the config dir comes
  // from the Windows known folder, not APPDATA), new for every run. LOCALAPPDATA: the staging dir.
  const env = { ...process.env, TETHER_PROFILE: `v2e2e${BASE}`, LOCALAPPDATA: join(qsHome, "Local"), TETHER_DEV_API: PROXIES.join(","), WEBVIEW2_USER_DATA_FOLDER: join(work, "wv2-qs"), WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${CDP + 1}` };
  qs = spawn(QS_EXE, [], { env, stdio: "ignore" });
  Q = await cdp(CDP + 1);
}
const staged = (v) => join(qsHome, "Local", "Tether-QS", `update-${v}.staged`);
/** The profile's config folder (removed at the end: it is in the real roaming profile). */
const qsConfig = () => join(process.env.APPDATA, `Tether-QS-v2e2e${BASE}`);
async function updateTo(v) {
  RELAYS.forEach((r) => (r.reqs = []));
  const state = await Q.invoke("update_check").catch((e) => `error: ${e}`);
  for (let i = 0; i < 240 && !existsSync(staged(v)); i++) await sleep(500);
  await sleep(500);
  return { state, reqs: RELAYS.flatMap((r) => r.reqs) };
}

try {
  for (const r of RELAYS) {
    for (let i = 0; i < 60 && !r.out.includes("listening"); i++) await sleep(250);
    if (!r.out.includes("listening")) throw new Error(r.out);
    for (let i = 0; i < 60 && !(await fetch(`http://127.0.0.1:${r.api}/api/policy`).then(() => true, () => false)); i++) await sleep(250);
  }
  mkdirSync(vault);
  for (const f of ["keys.json", "test-passphrase.txt"]) copyFileSync(join(VAULT, f), join(vault, f));
  mkdirSync(join(work, "builds"), { recursive: true });
  writeFileSync(join(vault, "settings.json"), JSON.stringify({ server: PROXIES.join(","), prune: { keep: 2, auto: false }, rollout: "now" }));
  manage = spawn(MANAGE, [], { env: { ...process.env, TETHER_MANAGE_DIR: vault, WEBVIEW2_USER_DATA_FOLDER: join(work, "wv2"), WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${CDP}` }, stdio: "ignore" });
  M = await cdp(CDP);
  await M.invoke("keys_unlock", { passphrase: PASS });

  // 1. Publish TetherQS 0.9.31 for Windows and macOS: v1 + v2.
  const win1 = fileAs(QS_FILE, "TetherQS-0.9.31.exe");
  const r1 = await M.invoke("release_publish", rel("windows-x86_64", "0.9.31", win1));
  check("publish Windows 0.9.31 to both relays", r1.filter((x) => x.ok).length === 2, JSON.stringify(r1).slice(0, 200));
  const r2 = await M.invoke("release_publish", rel("darwin-universal", "0.9.31", fileAs(MAC_FILE, "TetherQS-0.9.31-universal.zip")));
  check("publish macOS 0.9.31 to both relays", r2.filter((x) => x.ok).length === 2, JSON.stringify(r2).slice(0, 200));
  const tech = fileAs(MAC_TECH, "Tether_0.9.31_universal.app.tar.gz");
  fileAs(MAC_TECH.replace(".app.tar.gz", ".dmg"), "Tether_0.9.31_universal.dmg");
  const r2t = await M.invoke("release_publish", { args: { ...rel("darwin-universal", "0.9.31", tech).args, kind: "technician" } });
  check("publish technician macOS 0.9.31 to both relays", r2t.filter((x) => x.ok).length === 2, JSON.stringify(r2t).slice(0, 200));
  for (const [i, r] of RELAYS.entries()) {
    const v1 = indexOf(r, "releases.json"), v2 = indexOf(r, "releases-v2.json");
    const w1 = v1?.ix.releases["customer/windows-x86_64"], w2 = v2?.ix.releases["customer/windows-x86_64"];
    const m1 = v1?.ix.releases["customer/darwin-universal"], m2 = v2?.ix.releases["customer/darwin-universal"];
    check(`relay ${i + 1}: v1 and v2 with the same serial`, v1 && v2 && v1.ix.serial === v2.ix.serial, `${v1?.ix.serial} / ${v2?.ix.serial}`);
    check(`relay ${i + 1}: v1 has gzip only (TetherQS.exe .gz, mac none)`, w1?.packed?.encoding === "gzip" && !m1?.packed && !v1.bytes.toString().includes('"xz"'));
    check(`relay ${i + 1}: v2 has xz copies (Windows: the exe; macOS: the stored twin, signed on its own)`, w2?.packed?.encoding === "xz" && w2.sha256 === w1.sha256 && m2?.packed?.encoding === "xz" && m2.file === "TetherQS-0.9.31-universal-stored.zip" && m2.version === m1.version && m2.signature !== m1.signature && m2.size > m1.size, `mac v2 ${m2?.file} ${m2?.size} vs ${m1?.size}`);
    const xzFiles = ["customer/windows-x86_64/TetherQS-0.9.31.exe.xz", "customer/windows-x86_64/TetherQS-0.9.31.exe.gz", "customer/darwin-universal/TetherQS-0.9.31-universal-stored.zip", "customer/darwin-universal/TetherQS-0.9.31-universal-stored.zip.sig", "customer/darwin-universal/TetherQS-0.9.31-universal-stored.zip.xz"];
    check(`relay ${i + 1}: .xz, .gz and twin files on disk`, xzFiles.every((f) => existsSync(join(r.dir, "download", "stable", ...f.split("/")))));
    if (i === 0) {
      const mx = readFileSync(join(r.dir, "download", "stable", "customer", "darwin-universal", "TetherQS-0.9.31-universal-stored.zip.xz"));
      check("mac zip xz copy is smaller than the 25 MiB CDN limit", mx.length < 25 * 1024 * 1024 && mx.length === m2.packed.size, `${(mx.length / 1048576).toFixed(1)} MiB`);
      // The twins: xz unpacks to exactly the signed twin, and the twin holds what the original does.
      for (const [dir, orig, key] of [["customer/darwin-universal", "TetherQS-0.9.31-universal.zip", "customer/darwin-universal"], ["technician/darwin-universal", "Tether_0.9.31_universal.app.tar.gz", "technician/darwin-universal"]]) {
        const e = v2.ix.releases[key];
        const d = join(r.dir, "download", "stable", ...dir.split("/"));
        const out = execFileSync("python", ["-I", "-c", TWIN_CHECK, join(d, orig), join(d, e.file), join(d, `${e.file}.xz`), e.sha256]).toString().trim();
        check(`${key}: xz -> the signed twin, same content as ${orig}`, out.startsWith("ok"), out);
      }
      check("installed apps' code takes releases.json", /: ok serial/.test(oldVerdict(v1.bytes, "v1")), oldVerdict(v1.bytes, "v1"));
      check("installed apps' code refuses releases-v2.json (why it is a second file)", /REJECTED/.test(oldVerdict(v2.bytes, "v2")), oldVerdict(v2.bytes, "v2"));
    }
  }

  // 2. A new TetherQS reads v2 and takes the xz copy.
  await startQs();
  let u = await updateTo("0.9.31");
  check("TetherQS fetched releases-v2.json", u.reqs.includes("GET /download/stable/releases-v2.json"), u.reqs.filter((x) => x.includes("releases")).join(", "));
  check("TetherQS downloaded the .xz copy (not .gz, not the plain exe)", u.reqs.some((x) => x.endsWith("TetherQS-0.9.31.exe.xz")) && !u.reqs.some((x) => /TetherQS-0\.9\.31\.exe(\.gz)?$/.test(x)), u.reqs.filter((x) => x.includes("0.9.31")).join(", "));
  check("TetherQS staged exactly the published exe", existsSync(staged("0.9.31")) && sha(staged("0.9.31")) === sha(win1), u.state);

  // 3. Stale v2: Manage writes releases.json only; 0.9.32 is in v1 only, v1 serial is higher.
  writeFileSync(join(vault, "release-index-v1-only"), "");
  const win2 = fileAs(QS_FILE, "TetherQS-0.9.32.exe");
  const r3 = await M.invoke("release_publish", rel("windows-x86_64", "0.9.32", win2));
  check("publish 0.9.32 with v2 switched off", r3.filter((x) => x.ok).length === 2);
  const s1 = indexOf(RELAYS[0], "releases.json")?.ix, s2 = indexOf(RELAYS[0], "releases-v2.json")?.ix;
  check("v1 is ahead of the stale v2, no .xz made", s1.serial === s2.serial + 1 && !existsSync(join(RELAYS[0].dir, "download", "stable", "customer", "windows-x86_64", "TetherQS-0.9.32.exe.xz")), `${s1.serial} / ${s2.serial}`);
  u = await updateTo("0.9.32");
  check("TetherQS takes v1 (higher serial) and the .gz copy", u.reqs.some((x) => x.endsWith("TetherQS-0.9.32.exe.gz")) && existsSync(staged("0.9.32")) && sha(staged("0.9.32")) === sha(win2), u.reqs.filter((x) => x.includes("0.9.32")).join(", "));

  // 4. scripts/release-index-v1.sh against these relays, then --on.
  // Not execFileSync: the proxies run in this process and must answer the script's curl checks.
  const script = (args) => new Promise((done) => {
    const env = { ...process.env, MANAGE_DIR: vault, RELAY_SSH: "local", RELAY_EXEC: `bash ${join(work, "local-exec.sh").replace(/\\/g, "/")}`, RELAY_DATA: `${wslPath(work)}/api*`, SUDO: "", RELAY_HTTPS: PROXIES.join(" ") };
    const p = spawn(BASH, [join(ROOT, "scripts", "release-index-v1.sh").replace(/\\/g, "/"), ...args], { env });
    let out = "";
    p.stdout.on("data", (d) => (out += d));
    p.stderr.on("data", (d) => (out += d));
    p.on("close", (code) => done({ ok: code === 0, out }));
  });
  // "<target> <command>": run the command in WSL (the relays' data dirs), ignore the target.
  // --exec: no login shell in between that would expand the command's $variables first.
  writeFileSync(join(work, "local-exec.sh"), 'shift; MSYS_NO_PATHCONV=1 wsl -d Ubuntu-22.04 --exec bash -c "$1"\n');
  rmMarker();
  let s = await script([]);
  check("release-index-v1.sh: v2 deleted, 404 on both relays", s.ok && /single releases\.json on every relay/.test(s.out) && RELAYS.every((r) => !indexOf(r, "releases-v2.json")), s.out.split("\n").filter(Boolean).slice(-6).join(" | "));
  check("release-index-v1.sh: Manage switched to v1 only", existsSync(join(vault, "release-index-v1-only")));
  const win3 = fileAs(QS_FILE, "TetherQS-0.9.33.exe");
  await M.invoke("release_publish", rel("windows-x86_64", "0.9.33", win3));
  u = await updateTo("0.9.33");
  check("after the script TetherQS updates from releases.json (v2 404)", u.reqs.some((x) => x === "GET /download/stable/releases-v2.json") && u.reqs.some((x) => x.endsWith("TetherQS-0.9.33.exe.gz")) && existsSync(staged("0.9.33")), u.reqs.filter((x) => x.includes("0.9.33") || x.includes("releases")).join(", "));
  s = await script(["--on"]);
  check("release-index-v1.sh --on: marker removed", s.ok && !existsSync(join(vault, "release-index-v1-only")), s.out.trim());
  const win4 = fileAs(QS_FILE, "TetherQS-0.9.34.exe");
  await M.invoke("release_publish", rel("windows-x86_64", "0.9.34", win4));
  const on1 = indexOf(RELAYS[1], "releases.json")?.ix, on2 = indexOf(RELAYS[1], "releases-v2.json")?.ix;
  check("after --on the next publish writes v2 again (same serial, xz)", on2 && on1.serial === on2.serial && on2.releases["customer/windows-x86_64"].packed.encoding === "xz", `${on1?.serial} / ${on2?.serial}`);
  u = await updateTo("0.9.34");
  check("TetherQS takes the xz copy again", u.reqs.some((x) => x.endsWith("TetherQS-0.9.34.exe.xz")) && existsSync(staged("0.9.34")) && sha(staged("0.9.34")) === sha(win4), u.reqs.filter((x) => x.includes("0.9.34")).join(", "));
} catch (e) {
  check("no unexpected error", false, String(e?.stack ?? e).slice(0, 600));
} finally {
  await Q?.b.close().catch(() => {});
  await M?.b.close().catch(() => {});
  kill(qs);
  kill(manage);
  for (const r of RELAYS) kill(r.proc);
  try { execFileSync("cmd", ["/c", "rmdir", "/s", "/q", qsConfig()], { stdio: "ignore" }); } catch {}
  try { execFileSync("wsl", ["-d", "Ubuntu-22.04", "--", "pkill", "-f", `tether-api --data ${wslPath(work)}`], { stdio: "ignore" }); } catch {}
}
const failed = results.filter((r) => !r.ok);
console.log(`\n${results.length - failed.length}/${results.length} passed`);
process.exit(failed.length ? 1 : 0);

function rmMarker() {
  try { execFileSync("cmd", ["/c", "del", join(vault, "release-index-v1-only")], { stdio: "ignore" }); } catch {}
}

// "Publish the latest builds" with the real 0.5.x builds from dist, as the admin does it (r7):
// Tether Manage (debug exe, on a hidden desktop) on a COPY of the test vault, two local tether-api
// relays behind proxies that take release uploads at RATE bytes/s each (a home uplink).
// L1 the live policy shows in the bar, the Live policy tab and Status and relays after unlocking;
// L2 while publishing the sheet shows the progress as its content (the review form is hidden, no
//    heading cut at the scroll edge), Cancel stays in view;
// L3 Escape and a click beside the sheet while publishing do not cancel it (only Cancel does):
//    every build and the download page reach both relays;
// L4 the live policy still shows after the publish, and a live check during the uploads answers;
// L5 when no relay answers the check the bar says "could not check" (not "nothing published
//    yet"), and it is asked again by itself until the relays answer.
//   node e2e/publish-latest-e2e.mjs
// Env: MANAGE_EXE, BIN (WSL dir with tether-api), VAULT, DIST (folder with macos/, quick/,
// windows/ builds; copied, never changed), RATE (bytes/s per relay, default 3 MB/s), SHOTS.
import { execFileSync, spawn } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, openSync, closeSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer, request } from "node:http";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { chromium } from "playwright-core";

const MANAGE = process.env.MANAGE_EXE || "C:\\cargo-target\\r7\\debug\\tether-manage.exe";
const BIN = process.env.BIN || "/home/thomas/judge2-build/bin";
const VAULT = process.env.VAULT || "C:\\vms\\manage-test";
const DIST = process.env.DIST || "C:\\install\\remote-support\\dist";
const RATE = Number(process.env.RATE || 3e6);
const SHOTS = process.env.SHOTS || "";
const HIDDEN = resolve(import.meta.dirname, "hidden-run.ps1");
const PASS = readFileSync(join(VAULT, "test-passphrase.txt"), "utf8").trim();
const ADMIN_PUB = readFileSync(join(VAULT, "env.txt"), "utf8").trim().split(/\s+/)[0];
const BASE = 24600 + 4 * Math.floor(Math.random() * 100);
const RELAYS = [0, 1].map((i) => ({ api: BASE + 2 * i, proxy: BASE + 2 * i + 1 }));
const work = mkdtempSync(join(tmpdir(), "publish-latest-e2e-")), vault = join(work, "vault"), builds = join(work, "builds");
const wslPath = (p) => "/mnt/" + p[0].toLowerCase() + p.slice(2).replace(/\\/g, "/");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const results = [];
const check = (name, ok, detail = "") => {
  results.push({ name, ok: !!ok });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? `  (${detail})` : ""}`);
};
const shot = (page, name) => SHOTS && page.screenshot({ path: join(SHOTS, `${name}.png`) }).catch(() => {});
const short = (s) => String(s).replace(/\s+/g, " ").slice(0, 300);

// The newest build of each app in dist (the four "Publish the latest builds" finds).
const newest = (dir, re) => readdirSync(join(DIST, dir)).filter((f) => re.test(f)).sort((a, b) => a.localeCompare(b, undefined, { numeric: true })).pop();
const pick = [
  ["macos", newest("macos", /^TetherQS-[\d.]+-universal\.zip$/)],
  ["macos", newest("macos", /^Tether_[\d.]+_universal\.app\.tar\.gz$/)],
  ["quick", newest("quick", /^TetherQS-[\d.]+\.exe$/)],
  ["windows", newest("windows", /^Tether_[\d.]+_x64-setup\.exe$/)],
];
pick.push(["macos", pick[1][1].replace(".app.tar.gz", ".dmg")]);
for (const [d, f] of pick) {
  mkdirSync(join(builds, d), { recursive: true });
  copyFileSync(join(DIST, d, f), join(builds, d, f));
}
const VERSION = /_([\d.]+)_/.exec(pick[1][1])[1];

for (const [i, r] of RELAYS.entries()) {
  r.dir = join(work, `api${i + 1}`);
  mkdirSync(r.dir, { recursive: true });
  r.out = "";
  r.proc = spawn("wsl", ["-d", "Ubuntu-22.04", "--", "env", `ADMIN_PUB_KEY=${ADMIN_PUB}`, `${BIN}/tether-api`, "--data", wslPath(r.dir), "--listen", `127.0.0.1:${r.api}`], { stdio: ["ignore", "pipe", "pipe"] });
  r.proc.stdout.on("data", (d) => (r.out += d));
  r.proc.stderr.on("data", (d) => (r.out += d));
  // Caddy's part: /download/* from the data dir, the rest to tether-api; upload bodies at RATE.
  r.server = createServer((req, res) => {
    if (r.down) return req.socket.destroy();
    const u = req.url.split("?")[0];
    if (u.startsWith("/download/") && !/^\/download\/[a-z0-9-]+\/releases\.json$/.test(u) && (req.method === "GET" || req.method === "HEAD")) {
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
    if (req.method !== "PUT") return req.pipe(up);
    let sent = 0;
    const t0 = Date.now();
    req.on("data", (d) => {
      up.write(d);
      sent += d.length;
      const ahead = sent / RATE * 1000 - (Date.now() - t0);
      if (ahead > 20) {
        req.pause();
        setTimeout(() => req.resume(), ahead);
      }
    });
    req.on("end", () => up.end());
    req.on("aborted", () => up.destroy());
  });
  r.server.requestTimeout = r.server.headersTimeout = r.server.timeout = 0;
  r.server.listen(r.proxy, "127.0.0.1");
}
const [R1, R2] = RELAYS.map((r) => `http://127.0.0.1:${r.proxy}`);

let pid, b;
try {
  for (const r of RELAYS) {
    for (let i = 0; i < 80 && !r.out.includes("listening"); i++) await sleep(250);
    if (!r.out.includes("listening")) throw new Error(r.out || "tether-api did not start");
    for (let i = 0; i < 60 && !(await fetch(`http://127.0.0.1:${r.api}/api/policy`).then(() => true, () => false)); i++) await sleep(250);
  }
  mkdirSync(vault);
  copyFileSync(join(VAULT, "keys.json"), join(vault, "keys.json"));
  writeFileSync(join(vault, "settings.json"), JSON.stringify({ server: `${R1},${R2}`, prune: { keep: 3, auto: false }, release_folder: builds, rollout: "now", page_title: "Remote support", page_intro: "Test" }));
  const port = 9600 + Math.floor(Math.random() * 300);
  const out = join(work, "pid.txt");
  const fd = openSync(out, "w");
  execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", HIDDEN, "-Desktop", "tether-publish-latest", "-Exe", MANAGE], { env: { ...process.env, TETHER_MANAGE_DIR: vault, WEBVIEW2_USER_DATA_FOLDER: join(work, "wv2"), WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` }, stdio: ["ignore", fd, "inherit"] });
  closeSync(fd);
  pid = +readFileSync(out, "utf8").trim().split(/\s+/).pop();
  for (let i = 0; i < 120 && !b; i++) { b = await chromium.connectOverCDP(`http://127.0.0.1:${port}`).catch(() => null); if (!b) await sleep(500); }
  let page;
  for (let i = 0; i < 80 && !page; i++) { page = b.contexts().flatMap((c) => c.pages()).find((p) => !p.url().startsWith("devtools")); if (!page) await sleep(250); }
  await page.waitForFunction(() => !!window.__TAURI_INTERNALS__, null, { timeout: 60000 });
  await page.setViewportSize({ width: 1366, height: 768 }).catch(() => {});
  const invoke = (cmd, args = {}) => page.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a), [cmd, args]);
  const text = () => page.evaluate(() => document.body.innerText);
  const bar = () => page.getByRole("status", { name: "Policy state" }).innerText().catch(() => "");

  // Seed a live policy on both relays, then lock: the run starts at the unlock screen like the admin's.
  await invoke("keys_unlock", { passphrase: PASS });
  const seeded = await invoke("policy_publish", { bundle: await invoke("policy_draft") });
  await invoke("keys_lock");
  await page.reload();
  await page.locator("input[type=password]").waitFor({ timeout: 30000 });

  // ---- L1 ----
  await page.fill("input[type=password]", PASS);
  const t0 = Date.now();
  await page.press("input[type=password]", "Enter");
  let barText = "";
  for (let i = 0; i < 300 && !/Live: (serial|could not|nothing)/.test(barText); i++) { await sleep(50); barText = await bar(); }
  const liveMs = Date.now() - t0;
  check(`L1 bar shows the live policy after unlocking: "Live: serial ${seeded.serial} on 2/2 relays"`, barText.includes(`Live: serial ${seeded.serial} on 2/2 relays`), `${liveMs} ms: ${short(barText)}`);
  await page.getByRole("navigation", { name: "Sections" }).getByRole("button", { name: /^Policy/ }).click();
  await page.getByRole("radio", { name: /^Live policy/ }).click();
  await sleep(500);
  let t = await text();
  check("L1 Live policy tab lists the serial and both relays", t.includes(`Serial ${seeded.serial} · live on 2 of 2 relays`) && t.includes(`127.0.0.1:${RELAYS[1].proxy}`), short(t.slice(t.indexOf("Serial"), t.indexOf("Serial") + 200)));
  await shot(page, "L1-live-tab");
  await page.getByRole("navigation", { name: "Sections" }).getByRole("button", { name: /^Status and relays/ }).click();
  await page.getByText("Admin key accepted").first().waitFor({ timeout: 15000 }).catch(() => {});
  t = await text();
  check("L1 Status and relays: both relays healthy, policy Live", (t.match(/Admin key accepted/g) ?? []).length === 2 && (t.match(new RegExp(`${seeded.serial} Live`, "g")) ?? []).length === 2, short(t.slice(t.indexOf("Relay\t"), t.indexOf("Relay\t") + 300)));

  // ---- L2, L3 ----
  await page.getByRole("navigation", { name: "Sections" }).getByRole("button", { name: /^Releases/ }).click();
  await page.getByRole("button", { name: /^Publish 4 new builds/ }).click({ timeout: 40000 });
  const dlg = page.getByRole("dialog");
  await dlg.getByRole("button", { name: /^Sign and publish · 2 relays/ }).click({ timeout: 30000 });
  await dlg.getByRole("progressbar").first().waitFor({ timeout: 30000 });
  await sleep(1500);
  await shot(page, "L2-publishing");
  const layout = await page.evaluate(() => {
    const d = document.querySelector("[role=dialog]"), box = d.getBoundingClientRect();
    const seen = (el) => { const r = el.getBoundingClientRect(); return r.height > 0 && r.bottom > box.top && r.top < box.bottom; };
    // Every heading and line of the sheet is either fully inside its scroll area or not shown.
    const cut = [...d.querySelectorAll("h2,h3,p,li,label,textarea")].filter((el) => {
      if (!el.getClientRects().length) return false;
      for (let a = el.parentElement; a && a !== d; a = a.parentElement) {
        if (!/auto|scroll|hidden/.test(getComputedStyle(a).overflowY)) continue;
        const r = el.getBoundingClientRect(), s = a.getBoundingClientRect();
        if (r.top < s.bottom - 1 && r.bottom > s.bottom + 1) return true;
        if (r.bottom > s.top + 1 && r.top < s.top - 1) return true;
      }
      return false;
    }).map((el) => el.innerText.slice(0, 60));
    const cancel = [...d.querySelectorAll("button")].find((x) => x.innerText.trim() === "Cancel");
    const cr = cancel?.getBoundingClientRect();
    return {
      form: [...d.querySelectorAll("textarea")].some((x) => x.getClientRects().length && seen(x)),
      progress: [...d.querySelectorAll("[role=progressbar]")].some(seen),
      cancel: !!cr && cr.bottom <= innerHeight && cr.top >= 0,
      cut,
    };
  });
  check("L2 while publishing the review form is hidden and the progress is the content", !layout.form && layout.progress, JSON.stringify(layout));
  check("L2 nothing cut at the sheet's scroll edge, Cancel in view", !layout.cut.length && layout.cancel, JSON.stringify(layout.cut));

  // A live check while both relays take uploads (the bar's refresh does this).
  const ipc0 = Date.now();
  const during = await Promise.race([invoke("policy_live").then((l) => l.relays.map((r) => (r.reachable ? "ok" : r.error)).join(" | ")), sleep(30000).then(() => "no answer in 30 s")]);
  check("L4 a live check during the uploads answers within 10 s", Date.now() - ipc0 < 10000 && during === "ok | ok", `${Date.now() - ipc0} ms: ${during}`);

  await page.keyboard.press("Escape");
  await sleep(300);
  await page.mouse.click(4, 300); // the dimmed area left of the sheet, e.g. to bring the window to the front
  await sleep(300);
  await page.mouse.click(4, 760);
  const stillBusy = await dlg.getByRole("button", { name: "Publishing…" }).isVisible().catch(() => false);
  check("L3 Escape and clicks beside the sheet do not cancel: still publishing", stillBusy && !(await dlg.getByRole("button", { name: "Cancelling…" }).isVisible().catch(() => false)));
  const t1 = Date.now();
  // The result screen, or no sheet at all (a click beside a finished sheet closes it).
  for (let i = 0; i < 15 * 240 && (await dlg.count()) && !(await dlg.getByRole("button", { name: "Done" }).isVisible().catch(() => false)); i++) await sleep(250);
  const done = await dlg.innerText().catch(() => "");
  await shot(page, "L3-result");
  const files = [`stable/customer/darwin-universal/${pick[0][1]}`, `stable/technician/darwin-universal/${pick[1][1]}`, `stable/technician/darwin-universal/${pick[4][1]}`, `stable/customer/windows-x86_64/${pick[2][1]}`, `stable/technician/windows-x86_64/${pick[3][1]}`, "index.html"];
  const missing = RELAYS.flatMap((r, i) => files.filter((f) => !existsSync(join(r.dir, "download", ...f.split("/")))).map((f) => `relay ${i + 1} lacks ${f}`));
  check(`L3 all four ${VERSION} builds and the download page reach both relays, nothing cancelled`, !missing.length && !/Cancel/.test(done) && /Published on every relay|Live on every relay/.test(done), `${(Date.now() - t1) / 1000} s more; ${missing.join("; ") || short(done)}`);
  await dlg.getByRole("button", { name: "Done" }).click().catch(() => {});

  // ---- L4 ----
  await sleep(500);
  barText = await bar();
  check(`L4 after the publish the bar still shows "Live: serial ${seeded.serial} on 2/2 relays"`, barText.includes(`Live: serial ${seeded.serial} on 2/2 relays`), short(barText));

  // ---- L5: no relay answers the check (network not up yet), then both come back ----
  RELAYS.forEach((r) => (r.down = true));
  await page.getByRole("button", { name: "Check the live policy again" }).click();
  for (let i = 0; i < 100 && !/could not check/.test(barText); i++) { await sleep(200); barText = await bar(); }
  await shot(page, "L5-could-not-check");
  check("L5 no relay answers: the bar says 'could not check', not 'nothing published yet'", /Live: could not check/.test(barText) && !/nothing published/.test(barText), short(barText));
  RELAYS.forEach((r) => (r.down = false));
  const t5 = Date.now();
  for (let i = 0; i < 200 && !barText.includes(`Live: serial ${seeded.serial} on 2/2 relays`); i++) { await sleep(250); barText = await bar(); }
  check("L5 the check is asked again by itself once the relays answer (within 30 s)", barText.includes(`Live: serial ${seeded.serial} on 2/2 relays`), `${(Date.now() - t5) / 1000} s: ${short(barText)}`);
} catch (e) {
  check("run", false, String(e?.message ?? e).split("\n")[0]);
} finally {
  await b?.close().catch(() => {});
  if (pid) try { execFileSync("taskkill", ["/PID", String(pid), "/T", "/F"], { stdio: "ignore" }); } catch {}
  for (const r of RELAYS) {
    try { execFileSync("wsl", ["-d", "Ubuntu-22.04", "--", "pkill", "-f", `tether-api --data ${wslPath(r.dir)}`], { stdio: "ignore" }); } catch {}
    r.proc.kill();
    r.server.closeAllConnections?.();
    r.server.close();
  }
  await sleep(1000);
  rmSync(work, { recursive: true, force: true });
  const failed = results.filter((x) => !x.ok).length;
  console.log(`\n${results.length - failed}/${results.length} PASS`);
  process.exit(failed ? 1 : 0);
}

// Tether Manage relay reads with one relay that never answers (docs/ui-audit.md, round 2):
// Rollouts, Devices, Sessions, Releases and the release-index publish step must come back within
// about 10 s (api::QUICK) and name that relay's error. Round 3 adds the calls a blind review found
// hanging or sequential: Create ticket and the other small writes, Diagnostics list and Open, Keys,
// the download page, Team, Custom apps, Cleanup and Bring relays up to date. Two local tether-api relays; relay B sits
// behind a proxy that accepts every connection and never answers.
//   node e2e/relay-timing.mjs <manage.exe>
// Env: BIN (WSL dir with tether-api), VAULT (test vault, as live-timing.mjs).
import { execFileSync, spawn } from "node:child_process";
import { closeSync, copyFileSync, mkdirSync, mkdtempSync, openSync, readFileSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { chromium } from "playwright-core";

const MANAGE = process.argv[2];
const BIN = process.env.BIN || "/home/thomas/rel-0526/bin";
const VAULT = process.env.VAULT || "C:/vms/manage-test";
const LIMIT_MS = 12000; // api::QUICK is 10 s; reads run in parallel, so one bound for the whole call
const HIDDEN = resolve(import.meta.dirname, "hidden-run.ps1");
const PASS = readFileSync(join(VAULT, "test-passphrase.txt"), "utf8").trim();
const ADMIN_PUB = readFileSync(join(VAULT, "env.txt"), "utf8").trim().split(/\s+/)[0];
const work = mkdtempSync(join(tmpdir(), "relay-timing-"));
const wslPath = (p) => "/mnt/" + p[0].toLowerCase() + p.slice(2).replaceAll("\\", "/");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const pages = (b) => b.contexts().flatMap((c) => c.pages());
async function until(fn, timeout = 30000, every = 100) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    try { const v = await fn(); if (v) return v; } catch {}
    await sleep(every);
  }
  return null;
}
const kill = (pid) => { try { execFileSync("taskkill", ["/PID", String(pid), "/T", "/F"], { stdio: "ignore" }); } catch {} };

mkdirSync(join(work, "a"));
const relay = spawn("wsl", ["-d", "Ubuntu-22.04", "--", "env", `ADMIN_PUB_KEY=${ADMIN_PUB}`, "RELAY_HOST=127.0.0.1,localhost", `${BIN}/tether-api`, "--data", wslPath(join(work, "a")), "--listen", "127.0.0.1:22170"], { stdio: "ignore" });
// Relay B: accepts the connection, reads the request, never answers.
const held = [];
const dead = createServer((s) => { held.push(s); s.on("error", () => {}); });
await new Promise((r) => dead.listen(22171, "127.0.0.1", r));
await until(() => fetch("http://127.0.0.1:22170/api/policy").then((r) => r.status === 404 || r.status === 200), 30000, 300);

const dir = join(work, "m");
mkdirSync(dir);
copyFileSync(join(VAULT, "keys.json"), join(dir, "keys.json"));
writeFileSync(join(dir, "settings.json"), JSON.stringify({ server: "http://127.0.0.1:22170,http://127.0.0.1:22171" }));
const port = 9561;
const out = join(dir, "pid.txt");
const fd = openSync(out, "w");
execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", HIDDEN, "-Desktop", "tether-relay-timing", "-Exe", MANAGE], { env: { ...process.env, TETHER_MANAGE_DIR: dir, WEBVIEW2_USER_DATA_FOLDER: join(dir, "wv2"), WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` }, stdio: ["ignore", fd, "inherit"] });
closeSync(fd);
const pid = +readFileSync(out, "utf8").trim().split(/\s+/).pop();
let failed = 0;
try {
  const b = await until(() => chromium.connectOverCDP(`http://127.0.0.1:${port}`), 60000, 300);
  const p = await until(() => pages(b).find((x) => !x.url().startsWith("devtools")), 30000);
  await until(() => p.evaluate(() => !!document.querySelector("input[type=password]")), 30000);
  await p.evaluate((pw) => window.__TAURI_INTERNALS__.invoke("keys_unlock", { passphrase: pw }), PASS);
  const call = async (label, cmd, args, show, named = true) => {
    const t0 = Date.now();
    const r = await Promise.race([
      p.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a).then((v) => ({ v }), (e) => ({ e: String(e?.message ?? e) })), [cmd, args]),
      sleep(120000).then(() => ({ e: "no answer in 120 s" })),
    ]);
    const ms = Date.now() - t0;
    const text = r.e ? `error: ${r.e}` : show(r.v);
    const ok = ms <= LIMIT_MS && (!named || /22171/.test(text));
    if (!ok) failed++;
    console.log(`${ok ? "ok  " : "FAIL"} ${label}: ${ms} ms | ${text.slice(0, 260)}`);
  };
  await call("Rollouts (release_index)", "release_index", { channel: "stable" }, (v) => JSON.stringify(v.relays));
  await call("Devices (devices_all)", "devices_all", {}, (v) => `${v.devices.length} devices, failed ${JSON.stringify(v.failed)}`);
  await call("Sessions (audit_all)", "audit_all", { since: 0, limit: 100 }, (v) => `${v.events.length} events, failed ${JSON.stringify(v.failed)}`);
  await call("Releases (releases_overview)", "releases_overview", {}, (v) => `errors ${JSON.stringify(v.errors)}`);
  await call("Status (relays_check)", "relays_check", {}, (v) => JSON.stringify(v.map((r) => [r.server, r.error])));
  // The publish index step: fetch every index, check the files on each target, put the new one
  // (the key is not in the new channel's index, so the call ends in that error after publishing).
  await call("Publish index (release_control)", "release_control", { channel: "timing", key: "technician/windows-x86_64", action: "halt" }, (v) => JSON.stringify(v), false);
  await call("Rollouts after the publish", "release_index", { channel: "timing" }, (v) => `serial ${v.serial}, ${JSON.stringify(v.relays)}`);
  // Round 3.
  await call("Create ticket (POST diag-ticket)", "admin", { method: "POST", path: "/api/admin/diag-ticket", body: { note: "timing" } }, (v) => JSON.stringify(v));
  let ticket = null;
  await call("Diagnostics list (diag_list)", "diag_list", { archived: false }, (v) => ((ticket = v.tickets[0]?.ticket), `${v.tickets.length} tickets, failed ${JSON.stringify(v.failed)}`));
  writeFileSync(join(work, "diagnostics.json"), "{}");
  execFileSync("powershell", ["-NoProfile", "-Command", `Compress-Archive -LiteralPath '${join(work, "diagnostics.json")}' -DestinationPath '${join(work, "d.zip")}'`]);
  const up = await fetch(`http://127.0.0.1:22170/api/diag/${ticket}`, { method: "POST", headers: { "Content-Type": "application/zip" }, body: readFileSync(join(work, "d.zip")) });
  console.log(`     upload of ${ticket} to relay A: HTTP ${up.status}`);
  await call("Diagnostics Open, zip on relay A (diag_open)", "diag_open", { ticket }, (v) => `${v.files.length} files, ${v.size} bytes`, false);
  await call("Diagnostics Open on the silent relay (diag_open relay B)", "diag_open", { ticket, relay: "http://127.0.0.1:22171" }, (v) => JSON.stringify(v), false);
  await call("Diagnostics Close (diag_close)", "diag_close", { ticket }, (v) => JSON.stringify(v));
  await call("Keys page (serverkeys_live)", "serverkeys_live", {}, (v) => JSON.stringify(v.relays.map((r) => [r.server, r.note])));
  await call("Download page (page_render)", "page_render", { title: "t", intro: "i", technician: true, customer: true }, (v) => v.slice(0, 80), false);
  await call("Download page data (page_data)", "page_data", {}, (v) => JSON.stringify(v), false);
  await call("Team (team_load)", "team_load", {}, (v) => JSON.stringify(v.failed));
  await call("Custom apps (custom_list)", "custom_list", {}, (v) => `${v.length} apps`, false);
  await call("Cleanup review (release_prune_plan)", "release_prune_plan", {}, (v) => JSON.stringify(v).slice(0, 120));
  await call("Policy: bring relays up to date (policy_sync)", "policy_sync", {}, (v) => JSON.stringify(v), false);
  await b.close().catch(() => {});
} finally {
  kill(pid);
  dead.close();
  for (const s of held) s.destroy();
  relay.kill();
  try { execFileSync("wsl", ["-d", "Ubuntu-22.04", "--", "pkill", "-f", `${BIN}/tether-api --data /mnt/c`], { stdio: "ignore" }); } catch {}
}
console.log(failed ? `${failed} check(s) failed` : "all within the bound, with the dead relay's error");
process.exit(failed ? 1 : 0);

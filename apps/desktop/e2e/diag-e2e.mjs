// Diagnostics round trip: admin opens a ticket in Tether Manage -> the technician app uploads its
// diagnostics with that code -> the admin opens the bundle. Uses the Manage test vault (manage-cli).
import { spawn, execFileSync } from "node:child_process";
import { join, resolve } from "node:path";
import { chromium } from "playwright-core";

const HERE = import.meta.dirname;
const TECH = process.env.TECH_EXE || "C:\\cargo-target\\tether\\release\\tether.exe";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const manage = (cmd, args) => JSON.parse(execFileSync("node", [join(HERE, "manage-cli.mjs"), "call", cmd, JSON.stringify(args)]).toString().trim().split("\n").pop());

const t0 = Date.now();
const { ticket } = manage("admin", { method: "POST", path: "/api/admin/diag-ticket", body: { note: "e2e diagnostics" } });
console.log("ticket", ticket);

const app = spawn(TECH, [], { env: { ...process.env, TETHER_PROFILE: "diag-e2e", WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9232" }, stdio: "ignore" });
let ok = false;
try {
  let b;
  for (let i = 0; i < 60 && !b; i++) b = await chromium.connectOverCDP("http://127.0.0.1:9232").catch(() => sleep(500));
  let page;
  for (let i = 0; i < 60 && !page; i++) {
    page = b.contexts().flatMap((c) => c.pages()).find((p) => p.url().includes("index.html") || p.url().startsWith("tauri") || p.url().startsWith("http://tauri"));
    if (!page) await sleep(500);
  }
  await page.waitForFunction(() => !!window.__TAURI_INTERNALS__);
  await sleep(4000); // let the app reach the relay once so the bundle has route attempts
  await page.evaluate((t) => window.__TAURI_INTERNALS__.invoke("diag_upload", { ticket: t }), ticket);
  const got = manage("diag_open", { ticket });
  const names = (got.files || []).map((f) => f.name);
  // Manage returns diagnostics.json as text; search the raw text for secret-looking fields.
  const diag = typeof got.diagnostics === "string" ? got.diagnostics : JSON.stringify(got.diagnostics ?? "");
  const leaks = /"(password|permanent_password|key_pair|salt|token|unlock_pin)"\s*:\s*"(?!<redacted>)[^"]{6,}/i.test(diag);
  console.log("files:", names.length, names.slice(0, 8).join(", "));
  console.log("diagnostics.json:", diag.length, "chars");
  ok = names.some((n) => n.endsWith("diagnostics.json")) && !leaks;
  console.log(`${ok ? "PASS" : "FAIL"}  diagnostics upload with ticket -> visible in Tether Manage`, JSON.stringify({ ticket, files: names.length, secrets_in_json: leaks, ms: Date.now() - t0 }));
  await b.close();
} catch (e) {
  console.log("FAIL  diagnostics round trip", e.message);
} finally {
  try { execFileSync("taskkill", ["/PID", String(app.pid), "/T", "/F"], { stdio: "ignore" }); } catch {}
  process.exit(ok ? 0 : 1);
}

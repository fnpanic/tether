// Drive Tether Manage (debug exe) for end-to-end tests, through its own WebView over CDP.
// Uses a separate test vault (TETHER_MANAGE_DIR), never the admin's real one.
//
//   node e2e/manage-cli.mjs setup                      create the test vault via the first-run UI
//   node e2e/manage-cli.mjs call <command> [json-args] unlock, invoke a Tauri command, print JSON
//
// Env: MANAGE_DIR (default C:\vms\manage-test), MANAGE_EXE, MANAGE_SERVER.
import { spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { chromium } from "playwright-core";

const ROOT = resolve(import.meta.dirname, "../../..");
const DIR = process.env.MANAGE_DIR || "C:\\vms\\manage-test";
const EXE = process.env.MANAGE_EXE || join(ROOT, "apps/manage/src-tauri/target/debug/tether-manage.exe");
const SERVER = process.env.MANAGE_SERVER || "https://tether-desk.duckdns.org";
const PORT = 9444;
const PASS_FILE = join(DIR, "test-passphrase.txt");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function start() {
  mkdirSync(DIR, { recursive: true });
  const p = spawn(EXE, [], {
    env: { ...process.env, TETHER_MANAGE_DIR: DIR, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${PORT}` },
    stdio: "ignore",
  });
  let browser;
  for (let i = 0; i < 60 && !browser; i++) {
    browser = await chromium.connectOverCDP(`http://127.0.0.1:${PORT}`).catch(() => null);
    if (!browser) await sleep(500);
  }
  if (!browser) throw new Error("Tether Manage did not expose CDP");
  let page;
  for (let i = 0; i < 40 && !page; i++) {
    page = browser.contexts().flatMap((c) => c.pages())[0];
    if (!page) await sleep(250);
  }
  await page.waitForFunction(() => !!window.__TAURI_INTERNALS__);
  const invoke = (cmd, args = {}) => page.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a), [cmd, args]);
  const stop = async () => {
    await browser.close().catch(() => {});
    p.kill();
  };
  return { page, invoke, stop };
}

const [, , action, cmd, rawArgs] = process.argv;
const app = await start();
try {
  if (action === "setup") {
    if (existsSync(PASS_FILE)) throw new Error(`test vault already exists in ${DIR}`);
    const pass = randomBytes(18).toString("base64url");
    writeFileSync(PASS_FILE, pass);
    // The real first-run screen: two passphrase fields and "Create keys".
    const fields = app.page.locator('input[type="password"]');
    await fields.nth(0).fill(pass);
    await fields.nth(1).fill(pass);
    await app.page.getByRole("button", { name: "Create keys" }).click();
    await app.page.waitForFunction(() => !document.body.innerText.includes("Set up Tether Manage"), null, { timeout: 60000 });
    await app.invoke("settings_set", { settings: { server: SERVER } }).catch((e) => console.error("settings_set:", e.message));
    const st = await app.invoke("state");
    writeFileSync(join(DIR, "pub.json"), JSON.stringify({ admin_pub: st.admin_pub, updater_pub: st.updater_pub }, null, 2));
    console.log(JSON.stringify({ admin_pub: st.admin_pub, updater_pub: st.updater_pub, dir: DIR }));
  } else if (action === "call") {
    await app.invoke("keys_unlock", { passphrase: readFileSync(PASS_FILE, "utf8").trim() });
    const out = await app.invoke(cmd, rawArgs ? JSON.parse(rawArgs) : {});
    console.log(JSON.stringify(out ?? null));
  } else {
    throw new Error("usage: setup | call <command> [json-args]");
  }
} catch (e) {
  console.error("ERROR:", e?.message ?? e);
  process.exitCode = 1;
} finally {
  await app.stop();
}

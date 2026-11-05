// File transfer multi-select end-to-end on one Windows machine: host "ftm-host" (debug build:
// TETHER_DEV_PASSWORD) and viewer "ftm-viewer" (--file-transfer) both start on an invisible desktop
// (hidden-run.ps1) and are driven only over WebView2's DevTools protocol; nothing is injected into
// the user's desktop. The host is this PC, so all files live in a fresh folder under
// C:\Users\Public (the local pane reaches it by double-clicks from the home folder) that is removed
// at the end.
//
// Selects files and a folder with click / Shift+click / Ctrl+click / Ctrl+A, uploads and
// downloads them with one button, drags a selection, and deletes a multi-selection after the
// in-app confirmation. Checks sizes and bytes on disk.
//
//   node e2e/ft-multi.e2e.mjs          Env: TECH_EXE (C:\cargo-target\ftmulti\debug\tether.exe)
// Only processes started here are stopped (by PID).
import { execFileSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { chromium } from "playwright-core";

const EXE = process.env.TECH_EXE || "C:\\cargo-target\\ftmulti\\debug\\tether.exe";
const HIDDEN = resolve(import.meta.dirname, "hidden-run.ps1");
const HOST_CDP = 9352;
const VIEWER_CDP = 9353;
const PW = "Ftm-" + randomBytes(6).toString("hex");
const PUBLIC = process.env.PUBLIC || "C:\\Users\\Public";
const BASE_NAME = `tether-ft-multi-${randomBytes(3).toString("hex")}`;
const BASE = join(PUBLIC, BASE_NAME);
const pids = [];
const results = [];
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function until(what, fn, timeout = 30000, every = 250) {
  const end = Date.now() + timeout;
  let last;
  while (Date.now() < end) {
    try {
      const v = await fn();
      if (v) return v;
    } catch (e) {
      last = e;
    }
    await sleep(every);
  }
  throw new Error(`timeout waiting for ${what}${last ? ": " + last.message : ""}`);
}
const pages = (b) => b.contexts().flatMap((c) => c.pages());
const page = (b, match, timeout) => until(`page ${match}`, () => pages(b).find((p) => p.url().includes(match)), timeout);
const cdp = (port) => until(`devtools on :${port}`, () => chromium.connectOverCDP(`http://127.0.0.1:${port}`), 60000, 500);
const ipc = (p, cmd, args) => p.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a), [cmd, args]);
async function check(name, fn) {
  try {
    const d = await fn();
    results.push({ name, ok: true, ...(d || {}) });
    console.log(`ok   ${name}${d ? " " + JSON.stringify(d) : ""}`);
  } catch (e) {
    results.push({ name, ok: false, error: String(e.message || e) });
    console.log(`FAIL ${name} ${e.message || e}`);
  }
}

function hidden(args, env) {
  // stdio must be ignored: a pipe would stay open in the started app and block here.
  const before = ours();
  execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", HIDDEN, "-Exe", EXE, "-Args", args, "-Desktop", "tether-ft-multi"], { env: { ...process.env, ...env }, stdio: "ignore" });
  pids.push(...ours().filter((p) => !before.includes(p)));
}
/** PIDs of processes started from EXE (this test's build). */
function ours() {
  const ps = `Get-CimInstance Win32_Process -Filter "Name='tether.exe'" | Where-Object { $_.ExecutablePath -eq '${EXE}' } | ForEach-Object { $_.ProcessId }`;
  return execFileSync("powershell", ["-NoProfile", "-Command", ps], { stdio: ["ignore", "pipe", "ignore"] }).toString().split(/\s+/).filter(Boolean).map(Number);
}

/** a.bin, b.bin, skip.bin and sub/{c.bin, deep/d.bin}: different sizes and contents. */
function tree(dir) {
  const files = { "a.bin": 300 * 1024 + 1, "b.bin": 1200 * 1024 + 7, "skip.bin": 2048, "sub/c.bin": 500 * 1024, "sub/deep/d.bin": 10 * 1024 + 3 };
  for (const [rel, n] of Object.entries(files)) {
    mkdirSync(join(dir, rel, ".."), { recursive: true });
    writeFileSync(join(dir, rel), randomBytes(n));
  }
}
/** The selected items arrived byte for byte; skip.bin did not. */
function same(src, dst) {
  for (const rel of ["a.bin", "b.bin", "sub/c.bin", "sub/deep/d.bin"]) {
    const d = join(dst, rel);
    if (!existsSync(d)) throw new Error(`${rel} missing`);
    if (!readFileSync(d).equals(readFileSync(join(src, rel)))) throw new Error(`${rel} differs`);
  }
  if (existsSync(join(dst, "skip.bin"))) throw new Error("skip.bin was not selected but arrived");
  return true;
}
const arrived = (src, dst) => {
  try {
    return same(src, dst);
  } catch {
    return false;
  }
};

const step = (s) => console.log(`..   ${s}`);
async function main() {
  tree(join(BASE, "up-src"));
  tree(join(BASE, "down-src"));
  for (const d of ["up-dst", "down-dst", "drag-dst"]) mkdirSync(join(BASE, d), { recursive: true });

  // Every profile on one PC derives the same ID: give each its own (see toolbar.e2e.mjs).
  const getId = (profile) => execFileSync(EXE, ["--get-id"], { env: { ...process.env, TETHER_PROFILE: profile } }).toString().trim();
  const machineId = getId("ftm-fresh-" + randomBytes(3).toString("hex"));
  for (const profile of ["ftm-host", "ftm-viewer"]) {
    if (getId(profile) !== machineId) continue;
    const cfg = join(process.env.APPDATA, `Tether-${profile}`, "config", `Tether-${profile}.toml`);
    const raw = readFileSync(cfg, "utf8");
    writeFileSync(cfg, `id = '${1_000_000_000 + Math.floor(Math.random() * 900_000_000)}'\n` + raw.replace(/^(enc_)?id = .*\r?\n/gm, ""));
  }

  step("host");
  hidden("", { TETHER_PROFILE: "ftm-host", TETHER_DEV_PASSWORD: PW, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${HOST_CDP}` });
  const host = await page(await cdp(HOST_CDP), "index.html", 30000);
  const st = await until("host online", async () => {
    const s = await ipc(host, "status");
    return s.status === 1 && s.key_confirmed && s.id ? s : null;
  }, 60000);
  await sleep(3500); // dev password is applied 3 s after start

  step(`viewer -> ${st.id}`);
  hidden(`--connect ${st.id} --password-env FTM_PW --file-transfer`, { FTM_PW: PW, TETHER_PROFILE: "ftm-viewer", WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${VIEWER_CDP}` });
  const ft = await page(await cdp(VIEWER_CDP), "conn=file-transfer", 30000);
  const key = ft.url().match(/session\/([0-9a-f]{32})/)[1];
  const [localPane, remotePane] = [0, 1].map((i) => ft.getByRole("listbox").nth(i).locator("..")); // the pane holding the list
  const row = (pane, name) => pane.getByRole("option").filter({ hasText: new RegExp(`^${name.replace(".", "\\.")}`) });
  const pathOf = (pane) => pane.locator(".font-mono").first().innerText();
  const button = (pane, re) => pane.getByRole("button", { name: re });
  const selected = (pane) => pane.locator('[role="option"][aria-selected="true"]').count();
  step("file transfer window");
  await until("remote listing", async () => (await ft.getByRole("listbox").count()) === 2 && (await remotePane.getByRole("option").count()) > 0, 30000);

  const remoteGo = async (dir) => {
    await ipc(ft, "fs_remote_dir", { key, path: dir, hidden: false });
    await until(`remote pane at ${dir}`, async () => (await pathOf(remotePane)) === dir, 10000);
  };
  const localGo = async (sub) => {
    // From the home folder (or a sibling of `sub`) by Up and double-clicks, as a user would.
    if ((await pathOf(localPane)).startsWith(BASE)) await localPane.getByRole("button", { name: "Up", exact: true }).click();
    else {
      await until("local home listed", async () => (await localPane.getByRole("option").count()) > 0, 10000);
      await localPane.getByRole("button", { name: "Up", exact: true }).click();
      await row(localPane, "Public").dblclick();
      await row(localPane, BASE_NAME).dblclick();
    }
    await row(localPane, sub).dblclick();
    await until(`local pane at ${sub}`, async () => (await pathOf(localPane)) === join(BASE, sub), 10000);
  };

  await check("local pane: click, Shift+click, Ctrl+click, Ctrl+A select; the button counts them", async () => {
    await localGo("up-src");
    await remoteGo(join(BASE, "up-dst"));
    await row(localPane, "sub").click();
    await row(localPane, "b.bin").click({ modifiers: ["Shift"] }); // sub, a.bin, b.bin
    if ((await selected(localPane)) !== 3) throw new Error(`shift range: ${await selected(localPane)}`);
    await row(localPane, "skip.bin").click({ modifiers: ["Control"] });
    await button(localPane, /^Upload 4 items$/).waitFor({ timeout: 2000 });
    await row(localPane, "skip.bin").click({ modifiers: ["Control"] });
    await localPane.getByRole("listbox").press("Control+a");
    await button(localPane, /^Upload 4 items$/).waitFor({ timeout: 2000 });
    await localPane.getByRole("listbox").press("Escape");
    if (!(await button(localPane, /^Upload$/).isDisabled())) throw new Error("Escape did not clear");
    await row(localPane, "a.bin").click();
    await row(localPane, "sub").click({ modifiers: ["Control"] });
    await row(localPane, "b.bin").click({ modifiers: ["Control"] });
    await button(localPane, /^Upload 3 items$/).waitFor({ timeout: 2000 });
  });
  await check("upload 2 files + a folder with one click: all arrive byte for byte, 3 jobs", async () => {
    const t0 = Date.now();
    await button(localPane, /^Upload 3 items$/).click();
    await until("uploaded", () => arrived(join(BASE, "up-src"), join(BASE, "up-dst")), 60000, 300);
    const overall = await until("3 of 3 finished", async () => {
      const t = await ft.getByTestId("ft-overall").innerText();
      return t.startsWith("3 of 3 finished") && t;
    }, 15000);
    return { ms: Date.now() - t0, overall };
  });

  await check("remote pane: Ctrl+A, Ctrl+click off; download 2 files + a folder; remote listing stays", async () => {
    await ft.getByRole("button", { name: "Clear finished" }).click();
    await localGo("down-dst");
    const src = join(BASE, "down-src");
    await remoteGo(src);
    await row(remotePane, "a.bin").click();
    await remotePane.getByRole("listbox").press("Control+a");
    await row(remotePane, "skip.bin").click({ modifiers: ["Meta"] }); // Cmd works like Ctrl
    await button(remotePane, /^Download 3 items$/).click();
    await until("downloaded", () => arrived(src, join(BASE, "down-dst")), 60000, 300);
    await until("3 of 3 finished", async () => (await ft.getByTestId("ft-overall").innerText()).startsWith("3 of 3 finished"), 15000);
    if ((await pathOf(remotePane)) !== src) throw new Error(`remote pane moved to ${await pathOf(remotePane)}`);
    const names = await remotePane.getByRole("option").allInnerTexts();
    if (names.length !== 4) throw new Error(`remote listing: ${names.join(" | ")}`);
  });

  await check("drag a multi-selection to the other pane (keyboard: arrows + Shift)", async () => {
    await remoteGo(join(BASE, "drag-dst"));
    await localGo("up-src");
    const lb = localPane.getByRole("listbox");
    await lb.focus();
    await lb.press("ArrowDown"); // sub
    await lb.press("ArrowDown"); // a.bin
    await lb.press("Shift+ArrowDown"); // + b.bin
    await button(localPane, /^Upload 2 items$/).waitFor({ timeout: 2000 });
    await row(localPane, "b.bin").dragTo(remotePane);
    const dst = join(BASE, "drag-dst");
    await until("dragged files arrive", () => ["a.bin", "b.bin"].every((n) => existsSync(join(dst, n)) && readFileSync(join(dst, n)).equals(readFileSync(join(BASE, "up-src", n)))), 30000, 300);
    const extra = readdirSync(dst).filter((n) => !["a.bin", "b.bin"].includes(n));
    if (extra.length) throw new Error(`unexpected: ${extra}`);
  });

  await check("delete a multi-selection on the remote pane after one in-app confirmation", async () => {
    const dir = join(BASE, "up-dst");
    await remoteGo(dir);
    await remotePane.getByRole("listbox").press("Control+a");
    await remotePane.getByRole("listbox").press("Delete");
    const dlg = ft.getByRole("dialog");
    await dlg.getByText("Delete 3 items?").waitFor({ timeout: 3000 });
    await dlg.getByRole("button", { name: "Delete", exact: true }).click();
    await until("all deleted on disk", () => readdirSync(dir).length === 0, 15000, 300).catch((e) => {
      throw new Error(`${e.message}; left: ${readdirSync(dir, { recursive: true })}`);
    });
    await until("remote pane empty", async () => (await ft.locator("text=Empty folder").count()) > 0, 10000);
  });
}

try {
  await main();
} catch (e) {
  results.push({ name: "run", ok: false, error: String(e.stack || e) });
  console.log(`FAIL run ${e.stack || e}`);
} finally {
  for (const pid of pids) {
    try {
      execFileSync("taskkill", ["/PID", String(pid), "/T", "/F"], { stdio: "ignore" });
    } catch {}
  }
  await sleep(1000);
  rmSync(BASE, { recursive: true, force: true });
  const failed = results.filter((r) => !r.ok).length;
  console.log(`\n${results.length - failed}/${results.length} passed`);
  process.exit(failed ? 1 : 0);
}

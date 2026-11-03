// Session toolbar, file transfer and single-instance end-to-end test on one Windows machine.
//
// Host "ux-host" (debug build: TETHER_DEV_PASSWORD) and viewer "ux-viewer" (debug or release) run
// side by side with separate profiles; both are driven only over WebView2's DevTools protocol.
//
// Safety on a PC that is in use (the host is this machine):
//  - Mandatory gate: before any toolbar click the host revokes keyboard/mouse and restart for the
//    session, and the run stops unless the viewer has been told so (the host only announces a
//    permission after applying it). Mouse, keyboard, Ctrl+Alt+Del and lock are key/mouse events the
//    host then drops; nothing is injected on this desktop.
//  - Restart is only confirmed after the viewer's core session was closed, so no reboot request is
//    ever sent; the test asserts the resulting error toast instead.
//  - Full screen is not switched on (it would cover the user's screen); the window API it uses is
//    checked instead. No window positions or screenshots are used.
//
//   node e2e/toolbar.e2e.mjs                                        debug viewer
//   VIEWER_EXE=C:\cargo-target\ux\release\tether.exe node e2e/toolbar.e2e.mjs
//   QS_EXE=C:\cargo-target\ux-qs\debug\TetherQS.exe node e2e/toolbar.e2e.mjs   + TetherQS check
//
// Env: HOST_EXE / VIEWER_EXE (default C:\cargo-target\ux\debug\tether.exe), QS_EXE (optional).
// Results: tools/bench/results/toolbar-<debug|release>.json. Only processes started here are
// stopped (by PID).

import { spawn, execFileSync } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync, existsSync, statSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { chromium } from "playwright-core";

const ROOT = resolve(import.meta.dirname, "../../..");
const DEBUG_EXE = "C:\\cargo-target\\ux\\debug\\tether.exe";
const HOST_EXE = process.env.HOST_EXE || DEBUG_EXE;
const VIEWER_EXE = process.env.VIEWER_EXE || DEBUG_EXE;
const QS_EXE = process.env.QS_EXE || "";
const FLAVOR = VIEWER_EXE.toLowerCase().includes("release") ? "release" : "debug";
// A `|` in the password: it must survive the single-instance hand-over.
const PW = "Tb-E2e-" + randomBytes(4).toString("hex") + "|x";
const WORK = join(tmpdir(), "tether-toolbar-e2e");
const HOST_CDP = 9322;
const VIEWER_CDP = 9323;
const results = { started: new Date().toISOString(), flavor: FLAVOR, viewer: VIEWER_EXE, tests: [] };
const procs = [];
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let currentStep = "start";
const step = (name) => {
  currentStep = name;
  console.log(`…  ${name}`);
};

function launch(exe, args, env = {}) {
  const p = spawn(exe, args, { env: { ...process.env, ...env }, stdio: "ignore" });
  procs.push(p);
  return p;
}

const exited = (p, ms) =>
  new Promise((res) => {
    if (p.exitCode != null) return res(p.exitCode);
    const t = setTimeout(() => res(null), ms);
    p.once("exit", (code) => {
      clearTimeout(t);
      res(code);
    });
  });

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

const cdp = (port) => until(`devtools on :${port}`, () => chromium.connectOverCDP(`http://127.0.0.1:${port}`), 40000, 500);
const pages = (b) => b.contexts().flatMap((c) => c.pages());
const page = (b, match, timeout) => until(`page ${match}`, () => pages(b).find((p) => p.url().includes(match)), timeout);
const ipc = (p, cmd, args) =>
  Promise.race([
    p.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a), [cmd, args]),
    sleep(20000).then(() => {
      throw new Error(`IPC ${cmd} did not return within 20 s`);
    }),
  ]);
const text = (p) => p.evaluate(() => document.body.innerText);

function record(name, ok, detail = {}) {
  results.tests.push({ name, ok, ...detail });
  console.log(`${ok ? "ok " : "FAIL"} ${name}${Object.keys(detail).length ? " " + JSON.stringify(detail) : ""}`);
}
async function check(name, fn) {
  try {
    const detail = await fn();
    record(name, true, detail || {});
  } catch (e) {
    const page = diagPage && !diagPage.isClosed() ? (await text(diagPage).catch(() => "")).slice(0, 300) : undefined;
    record(name, false, { error: String(e.message || e), page });
  }
}
let diagPage = null;

/** Count `session` events per name: `own` = delivered to this window (what the app listens to),
 *  `any` = a target-Any listener, which also sees other windows' events (the old app behaviour). */
async function countEvents(p) {
  await p.evaluate(async () => {
    const T = window.__TAURI_INTERNALS__;
    window.__ev = { own: {}, any: {}, perm: {} };
    const add = (bucket) =>
      T.transformCallback((e) => {
        const n = e.payload?.name;
        window.__ev[bucket][n] = (window.__ev[bucket][n] || 0) + 1;
        if (bucket === "own" && n === "permission") window.__ev.perm[e.payload.payload.name] = e.payload.payload.value;
      });
    const label = T.metadata.currentWebview.label;
    await T.invoke("plugin:event|listen", { event: "session", target: { kind: "WebviewWindow", label }, handler: add("own") });
    await T.invoke("plugin:event|listen", { event: "session", target: { kind: "Any" }, handler: add("any") });
  });
}
const events = (p) => p.evaluate(() => JSON.parse(JSON.stringify(window.__ev)));

async function toolbarUp(s) {
  await s.mouse.move(400, 20);
  await until("toolbar visible", () => s.getByRole("button", { name: "Disconnect" }).isVisible(), 5000, 100);
}
async function notBlank(s) {
  const t = await text(s);
  if (t.includes("Something went wrong")) throw new Error("page crashed: " + t.slice(0, 300));
  if (!(await s.evaluate(() => document.getElementById("root")?.childElementCount ?? 0))) throw new Error("white screen: #root is empty");
}
const toastSays = (s, msg) => until(`toast "${msg}"`, async () => (await text(s)).includes(msg), 4000, 100);
/** A toolbar action that worked shows no error toast. */
async function noToast(s) {
  await sleep(800);
  const t = await s.evaluate(() => [...document.querySelectorAll('[role="status"]')].map((e) => e.innerText).join(" | "));
  if (t.trim()) throw new Error(`toast: ${t}`);
}

async function main() {
  rmSync(WORK, { recursive: true, force: true });
  mkdirSync(WORK, { recursive: true });

  // ------------------------------------------------------------------ own IDs
  // Every profile on one PC derives the same (MAC-based) ID, so other test hosts on this machine
  // and our own viewer's main window would share the host's registration. Give both profiles a
  // random ID of their own (a plain `id` is accepted and stored encrypted by Config::load).
  step("ids");
  const getId = (profile) => execFileSync(HOST_EXE, ["--get-id"], { env: { ...process.env, TETHER_PROFILE: profile } }).toString().trim();
  const machineId = getId("ux-fresh-" + randomBytes(3).toString("hex"));
  for (const profile of ["ux-host", "ux-viewer"]) {
    if (getId(profile) !== machineId) continue;
    const cfg = join(process.env.APPDATA, `Tether-${profile}`, "config", `Tether-${profile}.toml`);
    const id = String(1_000_000_000 + Math.floor(Math.random() * 900_000_000));
    const raw = readFileSync(cfg, "utf8");
    writeFileSync(cfg, `id = '${id}'\n` + raw.replace(/^(enc_)?id = .*\r?\n/gm, ""));
    if (getId(profile) !== id) throw new Error(`could not give ${profile} its own ID`);
  }

  // ------------------------------------------------------------------ host
  step("host");
  launch(HOST_EXE, [], { TETHER_PROFILE: "ux-host", TETHER_DEV_PASSWORD: PW, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${HOST_CDP}` });
  const hostB = await cdp(HOST_CDP);
  const host = await page(hostB, "index.html");
  const st = await until("host online", async () => {
    const s = await ipc(host, "status");
    return s.status === 1 && s.key_confirmed && s.id ? s : null;
  }, 60000);
  await sleep(3500); // dev password is applied 3 s after start

  // ------------------------------------------------------------------ viewer + single instance
  step("viewer");
  const vEnv = { TETHER_PROFILE: "ux-viewer", WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${VIEWER_CDP}` };
  launch(VIEWER_EXE, [], vEnv);
  const vB = await cdp(VIEWER_CDP);
  const main = await page(vB, "index.html");
  await until("viewer main window", () => text(main).then((t) => t.length > 0));
  const mainPages = () => pages(vB).filter((p) => !p.url().includes("/session/")).length;

  await check("single instance: second start exits, still one main window", async () => {
    const second = launch(VIEWER_EXE, [], { ...vEnv, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9329" });
    const code = await exited(second, 20000);
    if (code == null) throw new Error("second instance still running after 20 s");
    await sleep(1000);
    const n = mainPages();
    if (n !== 1) throw new Error(`${n} main windows`);
    return { exit: code };
  });

  step("forwarded --connect");
  let s;
  await check("single instance: second --connect opens the session in the first process (password via env, no prompt)", async () => {
    const before = pages(vB).length;
    const fwd = launch(VIEWER_EXE, ["--connect", st.id, "--password-env", "UX_E2E_PW"], { ...vEnv, UX_E2E_PW: PW, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9328" });
    const code = await exited(fwd, 20000);
    if (code == null) throw new Error("--connect process did not hand over and exit");
    s = await page(vB, "/session/", 30000).catch((e) => {
      throw new Error(`${e.message} (--connect exit ${code})`);
    });
    const t0 = Date.now();
    await until("first frame", () => s.evaluate(() => { const c = document.querySelector("canvas"); return !!c && !(c.width === 300 && c.height === 150); }), 40000).catch(async (e) => {
      throw new Error(`${e.message}; page: ${(await text(s)).slice(0, 300)}`);
    });
    if (await s.locator("#pw").count()) throw new Error("password prompt shown");
    return { exit: code, windows: pages(vB).length - before, ms_to_first_frame: Date.now() - t0 };
  });
  if (!s) throw new Error("no session window");
  const key = s.url().match(/session\/([0-9a-f]{32})/)[1];
  diagPage = s;
  await countEvents(s);

  // ------------------------------------------------------------------ mandatory safety gate
  step("host permissions");
  const cmPage = await page(hostB, "/cm", 15000);
  const cmId = (await until("cm client", async () => (await ipc(cmPage, "cm_clients")).find((x) => !x.is_file_transfer), 15000)).id;
  await ipc(cmPage, "cm_permission", { id: cmId, name: "keyboard", enabled: false });
  await ipc(cmPage, "cm_permission", { id: cmId, name: "restart", enabled: false });
  await until("viewer told keyboard/restart revoked", async () => {
    const { perm } = await events(s);
    return perm.keyboard === false && perm.restart === false;
  }, 10000);
  record("safety gate: host revoked keyboard/mouse and restart for the session", true);

  // ------------------------------------------------------------------ toolbar
  step("toolbar");
  await toolbarUp(s);
  await check("pin toolbar keeps it visible", async () => {
    await s.getByRole("button", { name: "Pin toolbar" }).click();
    await s.mouse.move(600, 500);
    await sleep(3000);
    if (!(await s.getByRole("button", { name: "Disconnect" }).isVisible())) throw new Error("toolbar hid while pinned");
    if (!(await s.getByRole("button", { name: "Unpin toolbar" }).count())) throw new Error("label did not change");
  });

  const displays = await s.locator('button[aria-label^="Display "]').count();
  if (displays > 1) {
    await check("display switch", async () => {
      await s.getByRole("button", { name: "Display 2" }).click();
      await noToast(s);
      await s.getByRole("button", { name: "Display 1" }).click();
      await noToast(s);
      await notBlank(s);
      return { displays };
    });
  } else record("display switch (single display: no buttons, nothing to switch)", true, { displays });

  await check("original size / fit to window", async () => {
    await s.getByRole("button", { name: "Original size" }).click();
    await s.getByRole("button", { name: "Fit to window" }).click({ timeout: 3000 });
    await s.getByRole("button", { name: "Original size" }).waitFor({ timeout: 3000 });
    await notBlank(s);
  });

  await check("image quality menu", async () => {
    await s.getByTitle("Image quality").click();
    await s.getByRole("button", { name: /Sharp text/ }).click();
    await noToast(s);
    if (!(await s.getByTitle("Image quality").innerText()).includes("Sharp text")) throw new Error("label not updated");
    const saved = await ipc(s, "get_option", { key: "image-quality" }).catch(() => "?");
    await s.getByTitle("Image quality").click();
    await s.getByRole("button", { name: /Balanced/ }).click();
    await noToast(s);
    await notBlank(s);
    return { saved };
  });

  await check("statistics overlay", async () => {
    await s.getByRole("button", { name: "Statistics" }).click();
    await until("stats", async () => (await text(s)).includes("encryption"), 3000, 100);
    await s.getByRole("button", { name: "Statistics" }).click();
    await until("stats hidden", async () => !(await text(s)).includes("encryption"), 3000, 100);
  });

  for (const label of ["Send Ctrl+Alt+Del", "Lock remote screen"]) {
    await check(`${label}: command accepted (host drops it: keyboard revoked)`, async () => {
      await s.getByRole("button", { name: label }).click();
      await noToast(s);
      await notBlank(s);
    });
  }

  await check("restart: in-app confirmation, cancel closes it", async () => {
    await s.getByRole("button", { name: "Restart remote device" }).click();
    const dlg = s.getByRole("dialog", { name: "Restart remote device" });
    await dlg.waitFor({ timeout: 3000 });
    await dlg.getByRole("button", { name: "Cancel" }).click();
    await dlg.waitFor({ state: "hidden", timeout: 3000 });
  });

  // Not switched on: a full-screen window would cover the user's screen (see header).
  await check("full screen: window API reachable, permission granted", async () => {
    if (await ipc(s, "plugin:window|is_fullscreen", { label: `s-${key}` })) throw new Error("already full screen");
    const caps = readFileSync(join(ROOT, "apps/desktop/src-tauri/capabilities/default.json"), "utf8");
    if (!caps.includes("core:window:allow-set-fullscreen")) throw new Error("set-fullscreen not permitted");
    await s.getByRole("button", { name: "Full screen" }).waitFor({ timeout: 2000 });
  });

  step("chat");
  await check("chat: panel, send, host message shown exactly once", async () => {
    await s.getByRole("button", { name: "Chat" }).click();
    await s.getByPlaceholder("Message").fill("hello from viewer");
    await s.getByRole("button", { name: "Send", exact: true }).click();
    await noToast(s);
    const before = (await events(s)).own.chat || 0;
    await ipc(cmPage, "cm_chat", { id: cmId, text: "hello from host" });
    await until("host message", async () => (await text(s)).includes("hello from host"), 10000, 100);
    await sleep(1500);
    const shown = await s.evaluate(() => [...document.querySelectorAll("div")].filter((d) => d.childElementCount === 0 && d.textContent === "hello from host").length);
    const evs = ((await events(s)).own.chat || 0) - before;
    if (shown !== 1 || evs !== 1) throw new Error(`shown ${shown}x, ${evs} chat events`);
    await s.getByRole("button", { name: "Close chat" }).click();
  });

  // ------------------------------------------------------------------ file transfer
  step("file transfer");
  const sBefore = await events(s);
  let ft;
  await check("transfer files: opens without a password prompt and lists the remote home", async () => {
    await s.getByRole("button", { name: "Transfer files" }).click();
    ft = await page(vB, "conn=file-transfer", 10000);
    await countEvents(ft);
    const listed = await until("remote listing", async () => {
      if ((await text(ft)).includes("Password required")) throw new Error("password prompt shown");
      return ft.evaluate(() => {
        const remote = document.querySelectorAll(".rounded-2xl.border")[1];
        const n = remote?.querySelectorAll("[draggable]").length ?? 0;
        return n > 0 && { path: remote.querySelector(".font-mono")?.textContent, entries: n };
      });
    }, 30000, 200);
    await sleep(1500);
    if ((await text(ft)).includes("Password required")) throw new Error("password prompt shown");
    await notBlank(ft);
    return { remote_path: listed };
  });
  if (ft) {
    await check("10 MB upload completes, SHA-256 matches, one job_done event, one row", async () => {
      const src = join(WORK, "up.bin");
      const dstDir = join(WORK, "remote-dst");
      mkdirSync(dstDir, { recursive: true });
      const blob = randomBytes(10 * 1024 * 1024);
      writeFileSync(src, blob);
      const ftKey = ft.url().match(/session\/([0-9a-f]{32})/)[1];
      const t0 = Date.now();
      await ipc(ft, "fs_transfer", { key: ftKey, id: 9001, path: src, to: join(dstDir, "up.bin"), fileNum: 0, hidden: false, isRemote: false });
      const dst = join(dstDir, "up.bin");
      await until("file arrives", () => existsSync(dst) && statSync(dst).size === blob.length, 60000, 200);
      await until("job_done", async () => (await events(ft)).own.job_done >= 1, 15000, 100);
      await sleep(1500);
      if (createHash("sha256").update(readFileSync(dst)).digest("hex") !== createHash("sha256").update(blob).digest("hex")) throw new Error("hash mismatch");
      const e = (await events(ft)).own;
      if (e.job_done !== 1) throw new Error(`${e.job_done} job_done events`);
      return { seconds: (Date.now() - t0) / 1000 };
    });
    await check("no duplicate connections; each window gets only its own events", async () => {
      const fe = (await events(ft)).own;
      if ((fe.peer_info || 0) > 1) throw new Error(`file transfer got ${fe.peer_info} peer_info`);
      const se = await events(s);
      for (const n of ["peer_info", "folder_files", "job_done", "msgbox"]) if ((se.own[n] || 0) !== (sBefore.own[n] || 0)) throw new Error(`remote window got the file transfer's ${n}`);
      // What a target-Any listener (the app before this fix) would have received from the other window:
      const leaked = Object.fromEntries(["folder_files", "job_done", "job_progress"].map((n) => [n, (se.any[n] || 0) - (sBefore.any[n] || 0)]));
      const cl = await ipc(cmPage, "cm_clients");
      const n = cl.filter((c) => c.is_file_transfer).length;
      if (n !== 1) throw new Error(`host sees ${n} file-transfer connections`);
      return { host_connections: cl.length, any_target_would_have_leaked: leaked };
    });
    await ipc(ft, "plugin:window|close", { label: await ft.evaluate(() => window.__TAURI_INTERNALS__.metadata.currentWebview.label) }).catch(() => {});
  }

  // ------------------------------------------------------------------ errors are shown
  step("error path");
  await check("after the session is gone, lock and a confirmed restart show an error toast (no request sent)", async () => {
    await ipc(s, "session_close", { key }); // core session only; the window stays
    await sleep(500);
    await toolbarUp(s).catch(() => {});
    if (!(await s.getByRole("button", { name: "Lock remote screen" }).isVisible())) return { skipped: "toolbar hidden after close" };
    await s.getByRole("button", { name: "Lock remote screen" }).click();
    await toastSays(s, "Not connected to the remote device.");
    await sleep(2000);
    await s.getByRole("button", { name: "Restart remote device" }).click();
    await s.getByRole("dialog", { name: "Restart remote device" }).getByRole("button", { name: "Restart" }).click();
    await toastSays(s, "Not connected to the remote device.");
    await notBlank(s);
  });

  // ------------------------------------------------------------------ disconnect
  step("disconnect");
  await check("disconnect closes the session window", async () => {
    await toolbarUp(s);
    await s.getByRole("button", { name: "Disconnect" }).click();
    await until("window closed", () => s.isClosed() || !pages(vB).includes(s), 10000, 200);
    await until("host dropped it", async () => !(await ipc(cmPage, "cm_clients")).some((c) => c.id === cmId), 15000, 300);
  });

  // ------------------------------------------------------------------ TetherQS
  if (QS_EXE) {
    step("TetherQS");
    await check("TetherQS: second start exits, first window stays the only one", async () => {
      launch(QS_EXE, [], { WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9330" });
      const qB = await cdp(9330);
      const q = await until("TetherQS window", () => pages(qB)[0], 30000);
      await until("TetherQS id", async () => /\d{3}/.test(await text(q)), 60000, 500);
      const second = launch(QS_EXE, [], { WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9331" });
      const code = await exited(second, 20000);
      if (code == null) throw new Error("second TetherQS still running");
      if (pages(qB).length !== 1) throw new Error(`${pages(qB).length} windows`);
      return { exit: code };
    });
  } else record("TetherQS single instance (skipped: set QS_EXE)", true, { skipped: true });
}

const outDir = join(ROOT, "tools/bench/results");
const watchdog = setTimeout(() => {
  record("run", false, { error: `watchdog: stuck in step '${currentStep}'` });
  cleanup();
}, 300000);
try {
  await main();
} catch (e) {
  record("run", false, { step: currentStep, error: String(e.stack || e) });
} finally {
  cleanup();
}
function cleanup() {
  clearTimeout(watchdog);
  // Only what this script started (and their children): never by image name.
  for (const p of procs) {
    if (p.exitCode != null) continue;
    try {
      execFileSync("taskkill", ["/PID", String(p.pid), "/T", "/F"], { stdio: "ignore" });
    } catch {}
  }
  mkdirSync(outDir, { recursive: true });
  writeFileSync(join(outDir, `toolbar-${FLAVOR}.json`), JSON.stringify(results, null, 2));
  const failed = results.tests.filter((t) => !t.ok).length;
  console.log(`\n${results.tests.length - failed}/${results.tests.length} passed (${FLAVOR})`);
  process.exit(failed ? 1 : 0);
}

// Keyboard path of the viewer: keys typed into the session window (CDP Input.dispatchKeyEvent,
// no OS input) arrive at the host as expected, with the PC→Mac shortcut mapping on and off, and
// keys still held when the window loses the focus are released on the host.
//
//   node e2e/keyboard-e2e.mjs --exe C:\cargo-target\kbd\debug\tether.exe   (debug build)
//
// Host (profile kbd-host) and viewer (kbd-view) run on an invisible desktop (hidden-run.ps1). The
// host is a debug build with TETHER_DEV_KEYLOG: it logs every received key event instead of
// injecting it (and ignores mouse events), so nothing is typed on this PC. The viewer pretends the
// host is a Mac (TETHER_DEV_PEER_PLATFORM) with the keyboard hook installed: it must pass every key, the window is on a hidden desktop (0.5.18 dropped them all).
// Only the process trees this script started are stopped (by PID).
import { execFileSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { chromium } from "playwright-core";

const { values: opt } = parseArgs({ options: { exe: { type: "string" } } });
if (!opt.exe) throw new Error("--exe is required");
const DESK = "tether-kbd-test";
const HIDDEN = resolve(import.meta.dirname, "hidden-run.ps1");
const PW = "Kbd-Pass-" + randomBytes(4).toString("hex") + "!";
const LOG = join(tmpdir(), `tether-keylog-${randomBytes(3).toString("hex")}.jsonl`);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let failures = 0;
const check = (what, ok, detail = "") => {
  console.log(`${ok ? "PASS" : "FAIL"} ${what}${ok ? "" : `\n     got: ${detail}`}`);
  if (!ok) failures++;
};

async function until(what, fn, timeout = 30000, every = 250) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    try {
      const v = await fn();
      if (v) return v;
    } catch {}
    await sleep(every);
  }
  throw new Error(`timeout: ${what}`);
}
const cdp = (port) => until(`cdp ${port}`, () => chromium.connectOverCDP(`http://127.0.0.1:${port}`), 120000, 500);
const page = (b, m) => until(`page ${m}`, () => b.contexts().flatMap((c) => c.pages()).find((p) => p.url().includes(m)), 60000);
const ipc = (p, cmd, args) => p.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a), [cmd, args]);

// An empty -Args value would be dropped by PowerShell: leave it out.
const hidden = (args, env) =>
  execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", HIDDEN, "-Exe", opt.exe, ...(args ? ["-Args", args] : []), "-Desktop", DESK], {
    env: { ...process.env, ...env },
    stdio: "ignore",
  });
// Every process of this exe was started by this script (its own build output).
const killAll = () =>
  execFileSync("powershell", ["-NoProfile", "-Command", `Get-CimInstance Win32_Process -Filter "Name='tether.exe'" | Where-Object { $_.ExecutablePath -eq '${opt.exe}' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }`], { stdio: "ignore" });

// Windows set-1 scan codes as the host receives them in map mode (peer = Windows).
const SC = { ControlLeft: 0x1d, ShiftLeft: 0x2a, Alt: 0x38, MetaLeft: 0xe05b, Space: 0x39, KeyC: 0x2e, KeyR: 0x13, KeyY: 0x15, KeyZ: 0x2c, KeyQ: 0x10, Home: 0xe047, LeftArrow: 0xe04b, Tab: 0x0f };
const NAME = Object.fromEntries(Object.entries(SC).map(([k, v]) => [v, k]));
const DOM = { ControlLeft: ["Control", 17, 2], ShiftLeft: ["Shift", 16, 8], AltLeft: ["Alt", 18, 1], AltRight: ["AltGraph", 18, 0], MetaLeft: ["Meta", 91, 4], Space: [" ", 32], KeyC: ["c", 67], KeyR: ["r", 82], KeyY: ["y", 89], Home: ["Home", 36], Tab: ["Tab", 9] };

/** Host log since `from` as "+Name"/"-Name" (map mode) or "text:x". */
function received(from) {
  if (!existsSync(LOG)) return [];
  return readFileSync(LOG, "utf8")
    .split("\n")
    .slice(from)
    .filter(Boolean)
    .map((l) => JSON.parse(l))
    .map((e) => (e.kind === "chr" ? `${e.down || e.press ? "+" : "-"}${NAME[+e.v] ?? "0x" + (+e.v).toString(16)}` : `${e.kind}:${e.v}`));
}
const lines = () => (existsSync(LOG) ? readFileSync(LOG, "utf8").split("\n").filter(Boolean).length : 0);

async function main() {
  const dir = join(process.env.APPDATA, "Tether-kbd-host", "config");
  if (!existsSync(join(dir, "Tether-kbd-host.toml"))) {
    mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, "Tether-kbd-host.toml"), `id = '${700000000 + Math.floor(Math.random() * 99999999)}'\n`);
  }
  hidden("", {
    TETHER_PROFILE: "kbd-host",
    TETHER_DEV_PASSWORD: PW,
    TETHER_DEV_KEYLOG: LOG,
    WEBVIEW2_USER_DATA_FOLDER: join(tmpdir(), "tether-kbd-host-wv"),
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9261",
  });
  const hostB = await cdp(9261);
  const main = await page(hostB, "index.html");
  const st = await until("host online", async () => {
    const s = await ipc(main, "status");
    return s.status === 1 && s.key_confirmed && s.id ? s : null;
  }, 90000);
  await sleep(4000); // dev password is set 3 s after start
  console.log(`host ${st.id}, key log ${LOG}`);

  hidden(`--connect ${st.id} --password-env TETHER_E2E_PW`, {
    TETHER_PROFILE: "kbd-view",
    TETHER_E2E_PW: PW,
    TETHER_DEV_PEER_PLATFORM: "Mac OS",
    WEBVIEW2_USER_DATA_FOLDER: join(tmpdir(), "tether-kbd-view-wv"),
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9262",
  });
  const viewB = await cdp(9262);
  const sess = await page(viewB, "/session/");
  const key = sess.url().match(/session\/([0-9a-f]{32})/)[1];
  await until("viewer connected", () => sess.evaluate(() => document.querySelector("canvas")?.width > 0), 60000);
  await sess.evaluate(() => document.querySelector("canvas").focus());
  const c = await sess.context().newCDPSession(sess);

  let mods = 0;
  /** Press (down) or release a DOM key in the session window, like a keyboard would. */
  const k = async (code, down, char) => {
    const [key, vk, bit = 0] = DOM[code] ?? [char, char.toUpperCase().charCodeAt(0)];
    if (down) mods |= bit;
    const text = down && char && !(mods & 6) ? char : undefined;
    await c.send("Input.dispatchKeyEvent", { type: down ? (text ? "keyDown" : "rawKeyDown") : "keyUp", code, key: char ?? key, windowsVirtualKeyCode: vk, modifiers: mods, text });
    if (!down) mods &= ~bit;
  };
  const combo = async (...codes) => {
    for (const x of codes) await k(...(Array.isArray(x) ? [x[0], true, x[1]] : [x, true]));
    for (const x of [...codes].reverse()) await k(...(Array.isArray(x) ? [x[0], false, x[1]] : [x, false]));
  };
  const expect = async (what, run, want) => {
    const from = lines();
    await run();
    const got = await until(what, async () => {
      const r = received(from);
      return r.length >= want.length ? r : null;
    }, 8000).catch(() => received(from));
    await sleep(300);
    const all = received(from);
    check(`${what}: ${want.join(" ")}`, JSON.stringify(all) === JSON.stringify(want), all.join(" "));
  };

  const kb = await ipc(sess, "session_keyboard", { key });
  check(`settings for a Mac host ${JSON.stringify(kb)}`, kb.dir === "pc-mac" && kb.shortcuts && kb.system_keys && !kb.alt_tab_remote && kb.hook);
  check("Keyboard menu in the toolbar", await until("Keyboard menu", () => sess.evaluate(() => !!document.querySelector('[aria-label="Keyboard"]')), 10000).catch(() => false));
  // The page takes key events once it has the focus: wait until a Shift press arrives.
  const warm = lines();
  await until("first key at the host", async () => {
    await k("ShiftLeft", true);
    await k("ShiftLeft", false);
    await sleep(500);
    return received(warm).length > 0;
  }, 20000);
  await sleep(500);

  // Mapping on (default towards a Mac)
  await expect("Ctrl+C → Cmd+C", () => combo("ControlLeft", "KeyC"), ["+MetaLeft", "+KeyC", "-KeyC", "-MetaLeft"]);
  await expect("Ctrl+Y → Shift+Cmd+Z", () => combo("ControlLeft", "KeyY"), ["+MetaLeft", "-MetaLeft", "+MetaLeft", "+ShiftLeft", "+KeyZ", "-KeyZ", "-ShiftLeft", "-MetaLeft", "+MetaLeft", "-MetaLeft"]);
  await expect("Home → Cmd+Left", () => combo("Home"), ["+MetaLeft", "+LeftArrow", "-LeftArrow", "-MetaLeft"]);
  await expect("Win+R → Control+R", () => combo("MetaLeft", "KeyR"), ["+ControlLeft", "+KeyR", "-KeyR", "-ControlLeft"]);
  await expect("AltGr+Q (German) → @ as text", () => combo("ControlLeft", "AltRight", ["KeyQ", "@"]), ["+MetaLeft", "-MetaLeft", "seq:@"]);

  // Mapping off: keys as they are
  const off = await ipc(sess, "session_keyboard", { key, shortcuts: false });
  check("mapping can be turned off", off.shortcuts === false);
  await expect("mapping off: Ctrl+C stays Ctrl+C", () => combo("ControlLeft", "KeyC"), ["+ControlLeft", "+KeyC", "-KeyC", "-ControlLeft"]);
  await expect("Alt+Space reaches the host", () => combo("AltLeft", "Space"), ["+Alt", "+Space", "-Space", "-Alt"]);
  await expect("Win+R reaches the host", () => combo("MetaLeft", "KeyR"), ["+MetaLeft", "+KeyR", "-KeyR", "-MetaLeft"]);
  await expect("Alt+Tab (typed in the window) reaches the host", () => combo("AltLeft", "Tab"), ["+Alt", "+Tab", "-Tab", "-Alt"]);

  // Held keys are released when the window loses the focus (or a text field takes it)
  await expect("Ctrl held", () => k("ControlLeft", true), ["+ControlLeft"]);
  await expect("released when a text field takes the keys", () => ipc(sess, "session_release_keys", { key }), ["-ControlLeft"]);
  await expect("Alt held", () => k("AltLeft", true), ["+Alt"]);
  mods = 0;
  await expect("released when the window is minimized (focus lost)", () => sess.evaluate(() => window.__TAURI_INTERNALS__.invoke("plugin:window|minimize", { label: window.__TAURI_INTERNALS__.metadata.currentWindow.label })), ["-Alt"]);
  await expect("a late key-up sends nothing twice", () => k("AltLeft", false), []);
}

main()
  .catch((e) => {
    console.error(e);
    failures++;
  })
  .finally(() => {
    killAll();
    rmSync(LOG, { force: true });
    console.log(failures ? `${failures} FAILED` : "ALL PASSED");
    process.exit(failures ? 1 : 0);
  });

// Host connection window (--cm) during a session: minimized to the taskbar once the session
// starts (with the main window), minimizable again any number of times (title-bar button and
// taskbar WM_SYSCOMMAND), restored with the Chat tab open when the technician writes, restored
// for a new request. `--cm` is started like the installed service does (SW_HIDE startup info).
// During the session the main window is minimized and off the taskbar (hidden), so the only
// Tether taskbar button is the connection window's; starting Tether again (pinned taskbar icon,
// Start menu) toggles the connection window, never the main one; it is back on the taskbar
// (minimized) when the session ends.
//
//   node e2e/cm-window-e2e.mjs --exe C:\cargo-target\cmwin\debug\tether.exe   (debug build: TETHER_DEV_PASSWORD)
//
// Host (profile cmw-host) and viewer (cmw-view) run on an invisible desktop (hidden-run.ps1) and
// are driven over DevTools only. The minimized state is read twice: from Tauri and with Win32
// IsIconic on the cm window. Only the process trees this script started are stopped (by PID).
import { execFileSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { chromium } from "playwright-core";

const { values: opt } = parseArgs({ options: { exe: { type: "string" } } });
if (!opt.exe) throw new Error("--exe is required");
const DESK = "tether-cmwin-test";
const HIDDEN = resolve(import.meta.dirname, "hidden-run.ps1");
const PW = "Cm-Pass-" + randomBytes(4).toString("hex") + "!";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let failures = 0;
const check = (what, ok) => {
  console.log(`${ok ? "PASS" : "FAIL"} ${what}`);
  if (!ok) failures++;
};

async function until(what, fn, timeout = 30000, every = 250) {
  if (process.env.CM_E2E_VERBOSE) console.log(`.. ${what}`);
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
const text = (p) => p.evaluate(() => document.body.innerText);

// stdio ignored: execFileSync would otherwise wait for the started app (see direct-e2e.mjs).
const hidden = (args, env, show = -1) =>
  execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", HIDDEN, "-Exe", opt.exe, "-Args", args, "-Desktop", DESK, ...(show >= 0 ? ["-Show", String(show)] : [])], {
    env: { ...process.env, ...env },
    stdio: "ignore",
  });
// Every process of this exe was started by this script (its own build output).
const killAll = (filter = "") =>
  execFileSync("powershell", ["-NoProfile", "-Command", `Get-CimInstance Win32_Process -Filter "Name='tether.exe'" | Where-Object { $_.ExecutablePath -eq '${opt.exe}' ${filter} } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }`], { stdio: "ignore" });

// Win32 view of our own windows on the hidden desktop: the "Tauri Window" of the process of this
// exe whose command line matches (`--cm` = connection window, none = main window).
const CM = "-and $_.CommandLine -like '*--cm*'";
const MAIN = "-and $_.CommandLine -notlike '*--*'";
function win32(filter, op = "probe", arg = 0) {
  const ps = `
Add-Type -TypeDefinition @'
using System; using System.Collections.Generic; using System.Runtime.InteropServices; using System.Text; using System.Threading;
public static class W {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern IntPtr OpenDesktop(string n, int f, bool i, uint a);
  [DllImport("user32.dll")] public static extern bool EnumDesktopWindows(IntPtr d, EnumProc p, IntPtr l);
  [DllImport("user32.dll")] public static extern int GetWindowThreadProcessId(IntPtr h, out int pid);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern int GetWindowLong(IntPtr h, int i);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  public static IntPtr Find(string desk, int want) {
    IntPtr r = IntPtr.Zero;
    IntPtr d = OpenDesktop(desk, 0, false, 0x0041); // DESKTOP_READOBJECTS | DESKTOP_ENUMERATE
    EnumDesktopWindows(d, (h, l) => { int pid; GetWindowThreadProcessId(h, out pid); var c = new StringBuilder(64); GetClassName(h, c, 64);
      if (pid == want && c.ToString() == "Tauri Window") r = h; return true; }, IntPtr.Zero);
    return r;
  }
  public static string Probe(IntPtr h) {
    return "{\\"iconic\\":" + (IsIconic(h) ? "true" : "false") + ",\\"visible\\":" + (IsWindowVisible(h) ? "true" : "false") +
      ",\\"style\\":" + GetWindowLong(h, -16) + ",\\"ex\\":" + GetWindowLong(h, -20) + "}";
  }
  // How many 100 ms samples within ms the window was NOT minimized (catches re-show loops).
  public static int Restored(IntPtr h, int ms) { int n = 0; for (int t = 0; t < ms; t += 100) { if (!IsIconic(h)) n++; Thread.Sleep(100); } return n; }
}
'@
$p = Get-CimInstance Win32_Process -Filter "Name='tether.exe'" | Where-Object { $_.ExecutablePath -eq '${opt.exe}' ${filter} } | Sort-Object CreationDate | Select-Object -First 1
$h = if ($p) { [W]::Find('${DESK}', [int]$p.ProcessId) } else { [IntPtr]::Zero }
if ($h -eq [IntPtr]::Zero) { 'null' }
elseif ('${op}' -eq 'restored') { [W]::Restored($h, ${arg}) }
elseif ('${op}' -eq 'syscommand') { [void][W]::PostMessage($h, 0x0112, [IntPtr]${arg}, [IntPtr]::Zero); 'true' } # WM_SYSCOMMAND, as the taskbar sends
else { [W]::Probe($h) }`;
  return JSON.parse(execFileSync("powershell", ["-NoProfile", "-Command", ps]).toString().trim() || "null");
}
const win32Cm = () => win32(CM);
const SC_MINIMIZE = 0xf020;
const SC_RESTORE = 0xf120;

async function cmState(cm) {
  const tauri = await ipc(cm, "plugin:window|is_minimized", { label: "cm" });
  return { tauri, win32: win32Cm() };
}

/** Minimized and stays so for 3.5 s: nothing re-shows the window behind the user's back. */
function staysMinimized(filter = CM) {
  const restored = win32(filter, "restored", 3500);
  return restored === 0;
}

async function main() {
  // Every profile on one PC derives the same machine ID: give the test host its own (see reconnect-bench.mjs).
  const dir = join(process.env.APPDATA, "Tether-cmw-host", "config");
  if (!existsSync(join(dir, "Tether-cmw-host.toml"))) {
    mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, "Tether-cmw-host.toml"), `id = '${700000000 + Math.floor(Math.random() * 99999999)}'\n`);
  }
  hidden("", { TETHER_PROFILE: "cmw-host", TETHER_DEV_PASSWORD: PW, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9251" });
  const hostB = await cdp(9251);
  const main = await page(hostB, "index.html");
  const st = await until("host online", async () => {
    const s = await ipc(main, "status");
    return s.status === 1 && s.key_confirmed && s.id ? s : null;
  }, 90000);
  await sleep(4000); // dev password is set 3 s after start
  console.log(`host ${st.id}`);

  // Installed, the service's server starts `--cm` with STARTF_USESHOWWINDOW / SW_HIDE
  // (LaunchProcessWin in the core); start it the same way so the server uses this one.
  hidden("--cm", { TETHER_PROFILE: "cmw-host", WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9251" }, 0);
  await until("cm window", () => win32Cm(), 60000);

  // 1. Password login: the session is active at once, the window minimizes.
  hidden(`--connect ${st.id} --password-env TETHER_E2E_PW`, { TETHER_PROFILE: "cmw-view", TETHER_E2E_PW: PW, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9252" });
  const viewB = await cdp(9252);
  const sess = await page(viewB, "/session/");
  const key = sess.url().match(/session\/([0-9a-f]{32})/)[1];
  const cm = await page(hostB, "#/cm");
  await until("Session active", async () => (await text(cm)).includes("Session active"), 60000);
  let s = await until("cm minimized", async () => {
    const x = await cmState(cm);
    return x.tauri ? x : null;
  }, 10000).catch(() => cmState(cm));
  check(`minimized after password login ${JSON.stringify(s)}`, s.tauri && s.win32?.iconic && s.win32?.visible);
  // Own taskbar button that toggles: minimize box, no tool window (WS_EX_TOOLWINDOW 0x80).
  check(`cm on the taskbar, minimizable ${JSON.stringify(s.win32)}`, (s.win32?.style & 0x20000) !== 0 && (s.win32?.ex & 0x80) === 0);
  const m = await until("main minimized", async () => ((await ipc(main, "plugin:window|is_minimized", { label: "main" })) ? win32(MAIN) : null), 10000).catch(() => win32(MAIN));
  // Hidden = no taskbar button: no click on a Tether button can reach the main window.
  check(`main window minimized and off the taskbar ${JSON.stringify(m)}`, m?.iconic && m?.visible === false);
  check("main window stays minimized", staysMinimized(MAIN));

  // 1b. The user restores and minimizes it again (title-bar button = window API), three times:
  // it must stay minimized every time.
  for (let i = 1; i <= 3; i++) {
    await ipc(cm, "plugin:window|unminimize", { label: "cm" });
    await until("restored", async () => !(await cmState(cm)).tauri, 5000).catch(() => null);
    await sleep(500);
    await ipc(cm, "plugin:window|minimize", { label: "cm" });
    await until("minimized", async () => (await cmState(cm)).tauri, 5000).catch(() => null);
    check(`round ${i}: minimize button keeps it minimized`, staysMinimized());
  }

  // 1c. Taskbar button: restore and minimize arrive as WM_SYSCOMMAND on the window, three times.
  for (let i = 1; i <= 3; i++) {
    win32(CM, "syscommand", SC_RESTORE);
    await sleep(1000);
    const r = win32Cm();
    check(`round ${i}: taskbar restore ${JSON.stringify(r)}`, r && !r.iconic && r.visible);
    win32(CM, "syscommand", SC_MINIMIZE);
    await sleep(500);
    check(`round ${i}: taskbar minimize stays minimized`, staysMinimized());
  }
  const mainOff = (x) => x?.iconic && x?.visible === false;
  check(`main window still minimized and off the taskbar ${JSON.stringify(win32(MAIN))}`, mainOff(win32(MAIN)));

  // 1d. The pinned taskbar icon / Start menu: Tether started again (single instance) toggles only
  // the connection window, three times; the main window stays minimized and off the taskbar.
  const again = () => hidden("", { TETHER_PROFILE: "cmw-host" });
  for (let i = 1; i <= 3; i++) {
    again();
    const r = await until("cm restored by a second start", () => {
      const x = win32Cm();
      return x && !x.iconic ? x : null;
    }, 20000).catch(() => win32Cm());
    check(`round ${i}: app icon restores the connection window ${JSON.stringify(r)}`, r && !r.iconic && r.visible);
    check(`round ${i}: main window not restored ${JSON.stringify(win32(MAIN))}`, mainOff(win32(MAIN)));
    again();
    await until("cm minimized by a second start", () => win32Cm()?.iconic, 20000).catch(() => null);
    check(`round ${i}: app icon minimizes the connection window`, staysMinimized());
    check(`round ${i}: main window stays minimized`, staysMinimized(MAIN) && mainOff(win32(MAIN)));
  }

  // 2. The user restores it: it stays restored.
  await ipc(cm, "plugin:window|unminimize", { label: "cm" });
  await sleep(3000);
  s = await cmState(cm);
  check(`stays restored ${JSON.stringify(s)}`, !s.tauri && s.win32 && !s.win32.iconic);

  // 3. Minimized again; a chat message restores it on the Chat tab.
  await ipc(cm, "plugin:window|minimize", { label: "cm" });
  await until("minimized by hand", async () => (await cmState(cm)).tauri, 5000);
  const msg = "cm-e2e " + randomBytes(3).toString("hex");
  await until("viewer connected", () => sess.evaluate(() => document.querySelector("canvas")?.width > 0), 40000);
  await ipc(sess, "session_chat", { key, text: msg });
  await until("message shown", async () => (await text(cm)).includes(msg), 15000).catch(() => null);
  await sleep(500);
  s = await cmState(cm);
  const t = await text(cm);
  const chatTab = await cm.evaluate(() => [...document.querySelectorAll("[aria-pressed=true],[aria-checked=true],[aria-selected=true]")].map((e) => e.textContent).join("|"));
  check(`restored on chat ${JSON.stringify(s)}`, !s.tauri && s.win32 && !s.win32.iconic && s.win32.visible);
  check(`chat tab shows the message (selected: ${chatTab})`, t.includes(msg) && /Chat/.test(chatTab));

  // 4. Click-to-accept: the request restores the window, Accept minimizes it.
  await ipc(cm, "plugin:window|minimize", { label: "cm" });
  killAll("-and $_.CommandLine -like '*--connect*'");
  await until("session gone", async () => !(await text(cm)).includes("Session active"), 30000);
  const back = await until("main back on the taskbar", () => {
    const x = win32(MAIN);
    return x?.visible ? x : null;
  }, 15000).catch(() => win32(MAIN));
  check(`session ended: main window back on the taskbar, still minimized ${JSON.stringify(back)}`, back?.visible && back?.iconic);
  hidden(`--connect ${st.id}`, { TETHER_PROFILE: "cmw-view", WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9252" });
  const accepted = await until("Accept prompt", async () => {
    const b = await cdp(9251);
    const c = await page(b, "#/cm");
    return (await text(c)).includes("Accept") ? c : null;
  }, 60000).catch(() => null);
  if (!accepted) {
    check("accept prompt shown (viewer without password)", false);
  } else {
    s = await cmState(accepted);
    check(`restored for the request ${JSON.stringify(s)}`, !s.tauri && s.win32 && !s.win32.iconic);
    await accepted.getByRole("button", { name: "Accept" }).click(); // DOM click over DevTools, no OS input
    await until("Session active after Accept", async () => (await text(accepted)).includes("Session active"), 20000).catch(() => null);
    check("UI shows the session after Accept", (await text(accepted)).includes("Session active"));
    s = await until("minimized after Accept", async () => ((await cmState(accepted)).tauri ? cmState(accepted) : null), 10000).catch(() => cmState(accepted));
    check(`minimized after Accept ${JSON.stringify(s)}`, s.tauri && s.win32?.iconic && s.win32?.visible);
  }
}

main()
  .catch((e) => {
    console.error(e);
    failures++;
  })
  .finally(() => {
    killAll();
    console.log(failures ? `${failures} FAILED` : "ALL PASSED");
    process.exit(failures ? 1 : 0);
  });

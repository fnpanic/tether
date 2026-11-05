// Technician session window: a maximized or full screen window comes back maximized or full screen
// after it was minimized, restored through Tauri (unminimize) and through the taskbar path
// (Win32 WM_SYSCOMMAND SC_RESTORE, what a taskbar click sends).
//
//   node e2e/session-maxrestore-e2e.mjs --exe C:\cargo-target\maxrestore\debug\tether.exe   (debug build: TETHER_DEV_PASSWORD)
//
// Host (profile mxr-host) and viewer (mxr-view) run on an invisible desktop (hidden-run.ps1) and
// are driven over DevTools only; Win32 calls go to the viewer's own session HWND. Only processes
// of this exe (this script's own build output) are stopped.
import { execFileSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { chromium } from "playwright-core";

const { values: opt } = parseArgs({ options: { exe: { type: "string" } } });
if (!opt.exe) throw new Error("--exe is required");
const DESK = "tether-maxrestore-test";
const HIDDEN = resolve(import.meta.dirname, "hidden-run.ps1");
const PW = "Mx-Pass-" + randomBytes(4).toString("hex") + "!";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let failures = 0;
const check = (what, ok) => {
  console.log(`${ok ? "PASS" : "FAIL"} ${what}`);
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

const hidden = (args, env) =>
  execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", HIDDEN, "-Exe", opt.exe, "-Args", args, "-Desktop", DESK], {
    env: { ...process.env, ...env },
    stdio: "ignore",
  });
const killAll = () =>
  execFileSync("powershell", ["-NoProfile", "-Command", `Get-CimInstance Win32_Process -Filter "Name='tether.exe'" | Where-Object { $_.ExecutablePath -eq '${opt.exe}' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }`], { stdio: "ignore" });

// The viewer's (--connect) top-level window: IsZoomed / IsIconic / rect; `act` "restore" posts
// WM_SYSCOMMAND SC_RESTORE to it first (the taskbar click path).
function win32(act = "") {
  const ps = `
Add-Type -TypeDefinition @'
using System; using System.Collections.Generic; using System.Runtime.InteropServices;
public static class W {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [DllImport("user32.dll")] public static extern IntPtr OpenDesktop(string n, int f, bool i, uint a);
  [DllImport("user32.dll")] public static extern bool EnumDesktopWindows(IntPtr d, EnumProc p, IntPtr l);
  [DllImport("user32.dll")] public static extern int GetWindowThreadProcessId(IntPtr h, out int pid);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsZoomed(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern int GetWindowTextLength(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetWindow(IntPtr h, uint c);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  public static string Probe(string desk, int want, string act) {
    var res = new List<string>();
    IntPtr d = OpenDesktop(desk, 0, false, 0x0041);
    EnumDesktopWindows(d, (h, l) => { int pid; GetWindowThreadProcessId(h, out pid);
      if (pid == want && GetWindow(h, 4) == IntPtr.Zero && GetWindowTextLength(h) > 0) {
        if (act == "restore") { PostMessage(h, 0x0112, (IntPtr)0xF120, IntPtr.Zero); System.Threading.Thread.Sleep(1500); }
        RECT r; GetWindowRect(h, out r);
        res.Add("{\\"zoomed\\":" + (IsZoomed(h) ? "true" : "false") + ",\\"iconic\\":" + (IsIconic(h) ? "true" : "false") +
          ",\\"rect\\":[" + r.L + "," + r.T + "," + r.R + "," + r.B + "]}");
      }
      return true; }, IntPtr.Zero);
    return "[" + string.Join(",", res) + "]";
  }
}
'@
$p = Get-CimInstance Win32_Process -Filter "Name='tether.exe'" | Where-Object { $_.ExecutablePath -eq '${opt.exe}' -and $_.CommandLine -like '*--connect*' } | Select-Object -First 1
if ($p) { [W]::Probe('${DESK}', [int]$p.ProcessId, '${act}') } else { '[]' }`;
  return JSON.parse(execFileSync("powershell", ["-NoProfile", "-Command", ps]).toString().trim() || "[]")[0] ?? null;
}

async function main() {
  const dir = join(process.env.APPDATA, "Tether-mxr-host", "config");
  if (!existsSync(join(dir, "Tether-mxr-host.toml"))) {
    mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, "Tether-mxr-host.toml"), `id = '${700000000 + Math.floor(Math.random() * 99999999)}'\n`);
  }
  hidden("", { TETHER_PROFILE: "mxr-host", TETHER_DEV_PASSWORD: PW, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9261" });
  const hostB = await cdp(9261);
  const main = await page(hostB, "index.html");
  const st = await until("host online", async () => {
    const s = await ipc(main, "status");
    return s.status === 1 && s.key_confirmed && s.id ? s : null;
  }, 90000);
  await sleep(4000); // dev password is set 3 s after start
  console.log(`host ${st.id}`);

  hidden(`--connect ${st.id} --password-env TETHER_E2E_PW`, { TETHER_PROFILE: "mxr-view", TETHER_E2E_PW: PW, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9262" });
  const viewB = await cdp(9262);
  const s = await page(viewB, "/session/");
  const label = "s-" + s.url().match(/session\/([0-9a-f]{32})/)[1];
  await until("viewer connected", () => s.evaluate(() => document.querySelector("canvas")?.width > 0), 60000);
  const w = (cmd, args = { label }) => ipc(s, `plugin:window|${cmd}`, args);
  // The toolbar hides itself; a mousemove near the top (a DOM event, no OS input) brings it back.
  const click = async (name) => {
    await s.evaluate(() => document.body.dispatchEvent(new MouseEvent("mousemove", { bubbles: true, clientX: 200, clientY: 10 })));
    await s.getByRole("button", { name, exact: true }).click({ timeout: 5000 });
  };

  for (const via of ["unminimize", "taskbar"]) {
    for (const how of ["toolbar", "api"]) {
      // Maximized -> minimized -> restored: maximized again, same rect.
      await w("toggle_maximize"); // maximize/unmaximize are not in the capability, toggle is
      const max = await until("maximized", async () => ((await w("is_maximized")) && win32()?.zoomed ? win32() : null), 5000);
      if (how === "toolbar") await click("Minimize");
      else await w("minimize");
      await until("minimized", () => w("is_minimized"), 5000);
      await sleep(500);
      const r = via === "taskbar" ? win32("restore") : (await w("unminimize"), await sleep(1500), win32());
      await sleep(500);
      const tauri = await w("is_maximized");
      const r2 = win32();
      check(`maximized, minimize (${how}), restore (${via}): maximized ${JSON.stringify({ tauri, r, r2, max: max.rect })}`,
        tauri && r2?.zoomed && !r2.iconic && JSON.stringify(r2.rect) === JSON.stringify(max.rect));
      await w("toggle_maximize");
      await until("unmaximized", async () => !(await w("is_maximized")) && !win32()?.zoomed, 5000);
      // Normal stays normal.
      await w("minimize");
      await until("minimized", () => w("is_minimized"), 5000);
      if (via === "taskbar") win32("restore");
      else await w("unminimize");
      await sleep(1500);
      const n = win32();
      check(`normal, minimize, restore (${via}): normal ${JSON.stringify(n)}`, !n.zoomed && !n.iconic && !(await w("is_maximized")));
    }
    // Full screen (toolbar) -> Minimize (toolbar) -> restored: full screen again.
    await click("Full screen");
    await until("full screen", () => w("is_fullscreen"), 5000);
    await sleep(500);
    const fs = win32();
    await click("Minimize");
    await until("minimized", () => w("is_minimized"), 5000);
    await sleep(500);
    if (via === "taskbar") win32("restore");
    else await w("unminimize");
    const back = await until("full screen again", () => w("is_fullscreen"), 5000).catch(() => false);
    await sleep(500);
    const f2 = win32();
    const label2 = await s.evaluate(() => [...document.querySelectorAll("button[aria-label]")].map((b) => b.getAttribute("aria-label")).find((l) => /full screen/i.test(l)));
    check(`full screen, minimize, restore (${via}): full screen ${JSON.stringify({ back, f2, fs: fs.rect, label2 })}`,
      back && !f2.iconic && JSON.stringify(f2.rect) === JSON.stringify(fs.rect) && label2 === "Exit full screen");
    if (label2 === "Exit full screen") await click("Exit full screen");
    else if (back) await w("set_fullscreen", { label, value: false }); // UI out of sync: still leave it
    await until("left full screen", async () => !(await w("is_fullscreen")), 5000);
    await sleep(500);
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

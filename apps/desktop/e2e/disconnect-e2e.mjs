// Disconnect from a session's toolbar keeps every Tether process running, viewer and host, and
// a technician app asked to connect to its own ID refuses with a message (0.5.27 crashed there:
// BEX64 0xc0000409, every tether.exe gone).
//
//   node e2e/disconnect-e2e.mjs --exe C:\cargo-target\r8\debug\tether.exe [--port 22216] [--rounds 3]
//
// The exe is a debug build (TETHER_DEV_PASSWORD) with TETHER_SERVER=127.0.0.1:<port> and
// TETHER_PUB_KEY = key A (C:\vms\night\build-e2e.sh). This script starts hbbs/hbbr (SERVER_BIN,
// default C:\cargo-target\keyrot-e2e\debug) on <port>/<port+1> with key A (KEY_A). Host (profile
// dc-host) and viewer (dc-view) main windows run on an invisible desktop (hidden-run.ps1) and are
// driven over DevTools only. Only processes of this exe and the relay started here are stopped.
import { execFileSync, spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { chromium } from "playwright-core";

const { values: opt } = parseArgs({ options: { exe: { type: "string" }, port: { type: "string", default: "22216" }, rounds: { type: "string", default: "3" } } });
if (!opt.exe) throw new Error("--exe is required");
const PORT = +opt.port;
const SBIN = process.env.SERVER_BIN || "C:\\cargo-target\\keyrot-e2e\\debug";
const KEY_A = process.env.KEY_A || "C:\\cargo-target\\keyrot\\e2e\\keyA";
const DESK = "tether-disconnect-test";
const HIDDEN = resolve(import.meta.dirname, "hidden-run.ps1");
const PW = "Dc-Pass-" + randomBytes(4).toString("hex") + "!";
const work = mkdtempSync(join(tmpdir(), "disconnect-e2e-"));
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
const pages = (b) => b.contexts().flatMap((c) => c.pages());
const page = (b, m) => until(`page ${m}`, () => pages(b).find((p) => p.url().includes(m)), 60000);
const ipc = (p, cmd, args) => p.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a), [cmd, args]);

/** Starts the exe on the hidden desktop; returns its PID. */
function hidden(args, env) {
  const out = join(work, `pid-${randomBytes(3).toString("hex")}.txt`);
  execFileSync("powershell", ["-NoProfile", "-Command", `& '${HIDDEN}' -Exe '${opt.exe}' -Args '${args}' -Desktop ${DESK} | Out-File -Encoding ascii '${out}'`], { env: { ...process.env, ...env }, stdio: ["ignore", "ignore", process.env.DC_VERBOSE ? "inherit" : "ignore"] });
  return +readFileSync(out, "utf8").trim().split(/\s+/).pop();
}
const procs = () =>
  JSON.parse(execFileSync("powershell", ["-NoProfile", "-Command", `@(Get-CimInstance Win32_Process -Filter "Name='tether.exe'" | Where-Object { $_.ExecutablePath -eq '${opt.exe}' } | ForEach-Object { $_.ProcessId }) | ConvertTo-Json -Compress`]).toString().trim() || "[]");
const alive = (pid) => [].concat(procs()).includes(pid);
const killAll = () =>
  execFileSync("powershell", ["-NoProfile", "-Command", `Get-CimInstance Win32_Process -Filter "Name='tether.exe'" | Where-Object { $_.ExecutablePath -eq '${opt.exe}' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }`], { stdio: "ignore" });

/** For `ms`, sends `profile`'s single-instance window what a plain second start sends (WM_COPYDATA:
 *  show the main window), back to back, so one lands while a closing session window is torn down. */
function startRequests(profile, ms) {
  const ps = `
Add-Type -TypeDefinition @'
using System; using System.Runtime.InteropServices; using System.Text;
public static class S {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern IntPtr OpenDesktop(string n, int f, bool i, uint a);
  [DllImport("user32.dll")] public static extern bool EnumDesktopWindows(IntPtr d, EnumProc p, IntPtr l);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  [StructLayout(LayoutKind.Sequential)] public struct CDS { public IntPtr dwData; public int cbData; public IntPtr lpData; }
  [DllImport("user32.dll")] public static extern IntPtr SendMessageTimeout(IntPtr h, uint m, IntPtr w, ref CDS l, uint f, uint t, out IntPtr r);
  public static int Run(string desk, string cls, int ms) {
    IntPtr w = IntPtr.Zero;
    EnumDesktopWindows(OpenDesktop(desk, 0, false, 0x0041), (h, l) => { var c = new StringBuilder(128); GetClassName(h, c, 128); if (c.ToString() == cls) w = h; return true; }, IntPtr.Zero);
    if (w == IntPtr.Zero) return -1;
    byte[] b = Encoding.UTF8.GetBytes("|tether" + (char)0); // the plugin's "cwd|argv0" with no arguments
    IntPtr p = Marshal.AllocHGlobal(b.Length); Marshal.Copy(b, 0, p, b.Length);
    var cds = new CDS { dwData = (IntPtr)1542, cbData = b.Length, lpData = p };
    int n = 0; var end = DateTime.Now.AddMilliseconds(ms); IntPtr r;
    while (DateTime.Now < end) { SendMessageTimeout(w, 0x004A, IntPtr.Zero, ref cds, 2, 2000, out r); n++; System.Threading.Thread.Sleep(1); } // WM_COPYDATA, SMTO_ABORTIFHUNG
    return n;
  }
}
'@
[S]::Run('${DESK}', 'app.tether.desktop.${profile}-sic', ${ms})`;
  const p = spawn("powershell", ["-NoProfile", "-Command", ps], { stdio: ["ignore", "pipe", "ignore"] });
  let out = "";
  p.stdout.on("data", (d) => (out += d));
  return new Promise((r) => p.on("exit", () => r(out.trim())));
}

function profile(name) {
  const dir = join(process.env.APPDATA, `Tether-${name}`, "config");
  if (!existsSync(join(dir, `Tether-${name}.toml`))) {
    // Every profile on one PC derives the same machine ID: give each its own.
    mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, `Tether-${name}.toml`), `id = '${600000000 + Math.floor(Math.random() * 99999999)}'\n`);
  }
}

const relay = [];
async function startRelay() {
  const dir = join(work, "relay");
  mkdirSync(dir);
  for (const f of ["id_ed25519", "id_ed25519.pub"]) copyFileSync(join(KEY_A, f), join(dir, f));
  let ready = "";
  for (const [bin, args] of [["hbbr", ["-p", String(PORT + 1), "-k", "_"]], ["hbbs", ["-p", String(PORT), "-r", `127.0.0.1:${PORT + 1}`, "-k", "_"]]]) {
    const p = spawn(join(SBIN, `${bin}.exe`), args, { cwd: dir, env: { ...process.env, HEALTH_PORT: "0" }, stdio: ["ignore", "pipe", "pipe"] });
    p.stdout.on("data", (d) => (ready += d));
    p.stderr.on("data", (d) => (ready += d));
    relay.push(p);
  }
  await until("hbbs listening", () => ready.includes("Listening on websocket"), 20000);
}

async function online(p) {
  let last;
  return until("online", async () => {
    const s = (last = await ipc(p, "status"));
    return s.status === 1 && s.key_confirmed && s.id ? s : null;
  }, 90000).catch((e) => {
    throw new Error(`${e.message}: ${JSON.stringify(last)}`);
  });
}

// The toolbar's Disconnect button, clicked in the DOM (the toolbar may be slid away; no OS input).
const disconnect = (p) => p.evaluate(() => document.querySelector('button[aria-label="Disconnect"]').click());

/** One session from `viewerProfile`'s running main window to `id`, ended with the toolbar's
 *  Disconnect (`busy`: while start requests arrive). */
async function sessionAndDisconnect(viewerB, viewerProfile, id, label, busy = false) {
  const before = new Set(pages(viewerB).map((p) => p));
  hidden(`--connect ${id} --password-env TETHER_E2E_PW`, { TETHER_PROFILE: viewerProfile, TETHER_E2E_PW: PW });
  const sess = await until("session window", () => pages(viewerB).find((p) => !before.has(p) && p.url().includes("/session/")), 60000);
  const shown = await until("remote screen", () => sess.evaluate(() => document.querySelector("canvas")?.width > 0), 60000).catch(() => false);
  check(`${label}: remote screen shown`, shown);
  await sleep(1500);
  const sent = busy && startRequests(viewerProfile, 4000);
  if (busy) await sleep(1000);
  await disconnect(sess);
  if (busy) console.log(`  ${await sent} start requests around Disconnect`);
  await until("session window closed", () => !pages(viewerB).some((p) => p.url().includes("/session/")), 20000).catch(() => null);
  await sleep(6000); // the crash came about a second after the window closed
}

async function main() {
  await startRelay();
  profile("dc-host");
  profile("dc-view");
  const hostPid = hidden("", { TETHER_PROFILE: "dc-host", TETHER_DEV_PASSWORD: PW, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9471" });
  const hostB = await cdp(9471);
  const hostMain = await page(hostB, "index.html");
  const st = await online(hostMain);
  await sleep(4000); // dev password is set 3 s after start
  console.log(`host ${st.id} pid ${hostPid}`);

  // 1. Normal disconnect: viewer and host are separate processes and profiles.
  const viewPid = hidden("", { TETHER_PROFILE: "dc-view", WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9472" });
  const viewB = await cdp(9472);
  await online(await page(viewB, "index.html"));
  for (let i = 1; i <= +opt.rounds; i++) {
    await sessionAndDisconnect(viewB, "dc-view", st.id, `normal ${i}`);
    check(`normal ${i}: viewer still running`, alive(viewPid));
    check(`normal ${i}: host still running`, alive(hostPid));
  }
  // 1b. Disconnect while the viewer's process gets start requests: 0.5.27 aborted when one arrived
  // while the session window was being destroyed (the loopback session's "host session ended").
  for (let i = 1; i <= +opt.rounds; i++) {
    await sessionAndDisconnect(viewB, "dc-view", st.id, `busy ${i}`, true);
    check(`busy ${i}: viewer still running`, alive(viewPid));
    check(`busy ${i}: host still running`, alive(hostPid));
  }

  // 2. Loopback: the technician app connects to its own ID.
  const hostBefore = pages(hostB).length;
  hidden(`--connect ${st.id} --password-env TETHER_E2E_PW`, { TETHER_PROFILE: "dc-host", TETHER_E2E_PW: PW });
  await sleep(8000);
  const loop = pages(hostB).find((p) => p.url().includes("/session/"));
  check(`loopback: no session window to itself (pages ${hostBefore} -> ${pages(hostB).length})`, !loop);
  if (loop) {
    await disconnect(loop).catch(() => null);
    await sleep(6000);
  }
  // Same request from the main window's Connect box: refused there with a message.
  const err = await ipc(hostMain, "open_session", { peer: st.id }).then(() => "", (e) => String(e));
  check(`loopback from the Connect box refused: "${err}"`, /own ID|this device/i.test(err));
  await sleep(3000);
  check("loopback: host still running", alive(hostPid));
  check("loopback: viewer still running", alive(viewPid));
}

main()
  .catch((e) => {
    console.error(e);
    failures++;
  })
  .finally(() => {
    killAll();
    for (const r of relay) r.kill();
    console.log(failures ? `${failures} FAILED` : "ALL PASSED");
    process.exit(failures ? 1 : 0);
  });

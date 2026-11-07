// Drive a technician session for the unattended-access (service) test, from this PC, without touching
// the real desktop: the viewer runs on a hidden desktop (hidden-run.ps1), everything goes through its
// WebView2 devtools. Used for docs/unattended.md.
//   node unattended-drive.mjs start <id> <pw.json>   start the viewer (password via --password-env, never argv)
//   node unattended-drive.mjs <step>...              act on the open session, steps:
//     shot:<file.png>  click:fx,fy  dbl:fx,fy  combo:KeyA+KeyB  text:<s>  pwfile:<json>[,enter]  restart  wait:ms  state
// fx,fy are fractions of the remote screen. pwfile types {"password"} from a JSON file (never printed).
// Env: PORT (devtools, default 9397), VIEWER (tether.exe, default C:\vms\night\viewer0523\tether.exe),
//      PROF (TETHER_PROFILE, default vm-svc-view), DESK (hidden desktop, default tether-svc-test).
import { createRequire } from "module";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
const require = createRequire(import.meta.url);
const { chromium } = require("playwright-core");
const PORT = process.env.PORT || "9397";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const readPw = (f) => JSON.parse(fs.readFileSync(f, "utf8").replace(/^\uFEFF/, "")).password;
const [cmd, ...rest] = process.argv.slice(2);

async function session() {
  for (let i = 0; i < 200; i++) {
    try {
      const b = await chromium.connectOverCDP(`http://127.0.0.1:${PORT}`);
      const p = b.contexts().flatMap((c) => c.pages()).find((x) => x.url().includes("conn=default"));
      if (p) return [b, p, p.url().match(/session\/([0-9a-f]{32})/)[1]];
      await b.close();
    } catch {}
    await sleep(300);
  }
  throw new Error("no session window");
}

if (cmd === "start") {
  const [id, pwfile] = rest;
  if (!/^\d{6,12}$/.test(id || "")) throw new Error("usage: start <id> <pw.json>");
  const env = { ...process.env, TETHER_PROFILE: process.env.PROF || "vm-svc-view", WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${PORT}`, TETHER_E2E_PW: readPw(pwfile) };
  const hid = new URL("./hidden-run.ps1", import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, "$1");
  const exe = process.env.VIEWER || "C:/vms/night/viewer0523/tether.exe";
  console.log("viewer pid", execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", hid, "-Exe", exe, "-Desktop", process.env.DESK || "tether-svc-test", "-Args", `--connect ${id} --password-env TETHER_E2E_PW`], { env }).toString().trim());
  const [b, p] = await session();
  await sleep(4000);
  console.log("state:", (await p.evaluate(() => document.body.innerText)).replace(/\s+/g, " ").slice(0, 200));
  await b.close();
} else {
  const [b, p, key] = await session();
  const inv = (c, a) => p.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a), [c, a]);
  const box = async () => p.locator("canvas").boundingBox();
  for (const s of [cmd, ...rest]) {
    const i = s.indexOf(":"), op = i < 0 ? s : s.slice(0, i), v = i < 0 ? "" : s.slice(i + 1);
    if (op === "wait") await sleep(+v);
    else if (op === "click" || op === "dbl") {
      const r = await box(), [fx, fy] = v.split(",").map(Number), x = r.x + r.width * fx, y = r.y + r.height * fy;
      await p.mouse.move(x, y, { steps: 4 });
      await sleep(300);
      await (op === "dbl" ? p.mouse.dblclick(x, y) : p.mouse.click(x, y));
      await sleep(800);
    } else if (op === "combo") {
      const ks = v.split("+");
      for (const c of ks) await inv("session_key", { key, code: c, down: true, lockModes: 0, character: "" });
      for (const c of ks.reverse()) await inv("session_key", { key, code: c, down: false, lockModes: 0, character: "" });
      await sleep(600);
    } else if (op === "text") { await inv("session_type_text", { key, text: v }); await sleep(300 + v.length * 30); }
    else if (op === "pwfile") {
      const [f, enter] = v.split(",");
      const pw = readPw(f);
      await inv("session_type_text", { key, text: pw });
      await sleep(400 + pw.length * 30);
      if (enter === "enter") for (const down of [true, false]) await inv("session_key", { key, code: "Enter", down, lockModes: 0, character: "" });
    } else if (op === "restart") console.log("restart:", await inv("session_restart_remote", { key }).then(() => "ok", (e) => "ERR " + e));
    else if (op === "state") console.log("state:", (await p.evaluate(() => document.body.innerText)).replace(/\s+/g, " ").slice(0, 200));
    else if (op === "shot") {
      const data = await p.evaluate(() => { const c = document.querySelector("canvas"); const o = document.createElement("canvas"); o.width = 1152; o.height = Math.round(1152 * c.height / c.width); o.getContext("2d").drawImage(c, 0, 0, o.width, o.height); return o.toDataURL("image/png"); });
      fs.writeFileSync(v, Buffer.from(data.split(",")[1], "base64"));
      console.log("saved", v);
    } else throw new Error(`unknown step ${s}`);
  }
  await b.close();
}

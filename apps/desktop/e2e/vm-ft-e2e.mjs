// End-to-end: technician app on this PC -> TetherQS in the Hyper-V VM, the customer accepting by
// click (no password typed). Checks what the customer and the technician see, chat both ways, and
// file transfer opened from the session (one card, no second accept, upload + download).
//
//   node e2e/vm-ft-e2e.mjs            technician on this PC (opens windows here)
//   TECH_IN_VM=1 node e2e/vm-ft-e2e.mjs   technician inside the VM too (nothing opens on this PC;
//                                         copy the build first: run-elevated.ps1 vm-tech-run.ps1 -Exe …)
// Env: VM_IP (172.30.252.30), QS_CDP (9334), TECH_EXE.
import { spawn, execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { chromium } from "playwright-core";

const ROOT = resolve(import.meta.dirname, "../../..");
const VM_IP = process.env.VM_IP || "172.30.252.30";
const QS_CDP = `http://${VM_IP}:${process.env.QS_CDP || 9334}`;
const TECH = process.env.TECH_EXE || "C:\\cargo-target\\tether\\release\\tether.exe";
const OUT = join(ROOT, "tools/bench/results/vm-ft-e2e.json");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const results = [];
const procs = [];
const IN_VM = process.env.TECH_IN_VM === "1";
const broker = (...a) => execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "C:\\vms\\broker\\run-elevated.ps1", "vm-tech-run.ps1", ...a]).toString();
const TECH_CDP = IN_VM ? `http://${VM_IP}:9235` : "http://127.0.0.1:9231";

function record(name, ok, detail = {}) {
  results.push({ name, ok, ...detail });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}`, Object.keys(detail).length ? JSON.stringify(detail) : "");
}
async function until(what, fn, timeout = 30000, every = 300) {
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
  throw new Error(`timeout waiting for ${what}${last ? `: ${last.message}` : ""}`);
}
const pages = (b) => b.contexts().flatMap((c) => c.pages());
const ipc = (p, cmd, args = {}) => p.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a), [cmd, args]);
const text = (p) => p.evaluate(() => document.body.innerText);
const check = async (name, fn) => {
  try {
    const d = await fn();
    record(name, true, d ?? {});
  } catch (e) {
    record(name, false, { error: e.message });
  }
};

async function main() {
  const qsB = await until("TetherQS devtools in the VM", () => chromium.connectOverCDP(QS_CDP), 60000, 1000);
  const qs = await until("TetherQS page", () => pages(qsB).find((p) => p.url().includes("index.html") || p.url().startsWith("tauri") || p.url().startsWith("http")), 20000);
  await until("TetherQS ready", async () => (await qs.locator('[data-testid="qs-status"]').innerText()).includes("Ready"), 90000, 1000);
  const id = (await qs.locator('[data-testid="qs-id"]').innerText()).replace(/\s/g, "");

  await check("QS diagnostics list every server, each checked", async () => {
    const d = await ipc(qs, "diagnostics");
    const probes = await Promise.all(d.servers.map((s) => ipc(qs, "diagnostics_probe", { server: s })));
    if (d.servers.length < 2) throw new Error(`servers: ${JSON.stringify(d.servers)}`);
    return { servers: d.servers, reachable: probes.map((p) => p.reachable) };
  });

  // 1. Connect without a password: the customer accepts by click.
  if (IN_VM) console.log(broker("-Connect", id).trim().split(/\r?\n/).pop());
  else procs.push(spawn(TECH, ["--connect", id], { env: { ...process.env, TETHER_PROFILE: "vm-tech", WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9231" }, stdio: "ignore" }));
  const vB = await until("technician devtools", () => chromium.connectOverCDP(TECH_CDP), 60000, 500);
  const v = await until("session window", () => pages(vB).find((p) => p.url().includes("/session/")), 40000);
  const key = v.url().match(/session\/([0-9a-f]{32})/)[1];

  await check("request on TetherQS says Accept needs no password; technician told either works", async () => {
    await until("Accept button", () => qs.getByRole("button", { name: "Accept" }).isVisible(), 30000);
    const q = await text(qs);
    if (!/Accept lets them in right away/.test(q)) throw new Error(q.slice(0, 300));
    const tech = await until("technician waiting text", async () => {
      const x = await text(v);
      return /click Accept/i.test(x) ? x : null;
    }, 15000);
    const audio = await qs.getByRole("switch", { name: /sound/i }).getAttribute("aria-checked");
    return { technician: tech.replace(/\s+/g, " ").slice(0, 160), audio_default: audio };
  });
  await check("audio is off by default", async () => {
    const a = await qs.getByRole("switch", { name: /sound/i }).getAttribute("aria-checked");
    if (a !== "false") throw new Error(`audio switch ${a}`);
  });
  await check("window docks bottom right during the request", async () => {
    const r = await qs.evaluate(() => ({ x: screenX, y: screenY, w: outerWidth, h: outerHeight, sw: screen.availWidth, sh: screen.availHeight }));
    if (r.sw - (r.x + r.w) > 80 || r.sh - (r.y + r.h) > 80) throw new Error(JSON.stringify(r));
    return r;
  });
  await qs.getByRole("button", { name: "Accept" }).click();
  await check("after Accept: confirmation on TetherQS, session card, first frame for the technician", async () => {
    const toast = await until("accepted toast", async () => ((await text(qs)).includes("Accepted:") ? true : null), 5000, 100);
    await until("session active", async () => (await qs.locator('[data-testid="qs-status"]').innerText()).includes("Session active"), 15000);
    await until("first frame", () => v.evaluate(() => { const c = document.querySelector("canvas"); return !!c && !(c.width === 300 && c.height === 150); }), 30000);
    return { toast };
  });

  // 2. Chat both ways. The toolbar auto-hides: its handle at the top centre reveals it.
  const toolbarUp = async () => {
    const handle = v.getByRole("button", { name: "Show toolbar" });
    if (await handle.isVisible()) await handle.click();
  };
  const techChat = async () => {
    await toolbarUp();
    await v.getByRole("button", { name: /^Chat/ }).click();
  };
  const techSay = async (t) => {
    await v.getByPlaceholder("Message").fill(t);
    await v.getByPlaceholder("Message").press("Enter");
  };
  const qsMsg = (t) => qs.locator('[data-testid="qs-chat-msg"]', { hasText: t });
  await check("chat: technician's message shows on TetherQS (chat opens, toast, attention requested)", async () => {
    const hello = `Hello from your technician ${Date.now()}`;
    await techChat();
    await techSay(hello);
    await qsMsg(hello).waitFor({ timeout: 10000 });
    const toast = await until("toast", async () => ((await text(qs)).includes("New message from your technician") ? true : null), 3000, 100);
    const chatTab = await qs.locator('[data-testid="qs-tab-chat"]').getAttribute("aria-pressed");
    const focused = await qs.evaluate(() => document.hasFocus());
    let log = "";
    if (IN_VM) {
      log = execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "C:\\vms\\broker\\run-elevated.ps1", "vm-qslog4.ps1", "-Pattern", "attention", "-Last", "3", "-Skip", "0"]).toString();
      if (!/attention requested \(Ok/.test(log)) throw new Error(`QS log: ${log.slice(-400)}`);
    }
    // `focused` is informational: TetherQS already took the focus when the request arrived.
    if (chatTab !== "true") throw new Error(JSON.stringify({ chatTab, focused }));
    return { toast, chatTab, focused, attention: IN_VM ? log.match(/attention requested \([^)]*\)+/)?.[0] : "not checked (technician on this PC)" };
  });
  await check("chat: a later message while Permissions is shown raises an unread badge on TetherQS", async () => {
    await qs.locator('[data-testid="qs-tab-perm"]').click();
    const again = `Second message ${Date.now()}`;
    await techSay(again);
    const badge = await until("QS unread badge", async () => {
      const b = qs.locator('[data-testid="qs-chat-unread"]');
      return (await b.count()) ? b.innerText() : null;
    }, 10000);
    await qs.locator('[data-testid="qs-tab-chat"]').click();
    await qsMsg(again).waitFor({ timeout: 3000 });
    const gone = (await qs.locator('[data-testid="qs-chat-unread"]').count()) === 0;
    if (badge.trim() !== "1" || !gone) throw new Error(JSON.stringify({ badge, gone }));
    return { badge: badge.trim() };
  });
  await check("chat: customer's reply reaches the technician (unread badge while the panel is closed)", async () => {
    await v.getByRole("button", { name: "Close chat" }).click();
    const reply = `Reply from the customer ${Date.now()}`;
    await qs.locator('[data-testid="qs-chat-input"]').fill(reply);
    await qs.locator('[data-testid="qs-chat-input"]').press("Enter");
    await qsMsg(reply).waitFor({ timeout: 3000 });
    const badge = await until("technician unread badge", async () => {
      const b = v.locator('[data-testid="chat-unread"]');
      return (await b.count()) && (await b.isVisible()) ? b.innerText() : null;
    }, 10000);
    await techChat();
    const shown = await until("reply in the technician's chat panel", async () => ((await text(v)).includes(reply) ? true : null), 5000);
    const cleared = (await v.locator('[data-testid="chat-unread"]').count()) === 0;
    await v.getByRole("button", { name: "Close chat" }).click();
    if (badge.trim() !== "1" || !cleared) throw new Error(JSON.stringify({ badge, cleared }));
    return { badge: badge.trim(), shown };
  });
  // Later checks look for the permission switches on the session card.
  await qs.locator('[data-testid="qs-tab-perm"]').click().catch(() => {});

  // 3. Remote actions: TetherQS runs without the service.
  await check("remote actions: Ctrl+Alt+Del disabled with the reason, lock asks and warns (not sent)", async () => {
    await v.getByRole("button", { name: "Remote actions" }).click();
    const cad = v.getByRole("menu").getByRole("button", { name: /^Send Ctrl\+Alt\+Del/ });
    const disabled = await cad.isDisabled();
    const hint = await cad.innerText();
    await v.getByRole("menu").getByRole("button", { name: /^Lock screen/ }).click();
    const dlg = v.getByRole("dialog", { name: "Lock the remote screen?" });
    await dlg.waitFor({ timeout: 3000 });
    const warn = await dlg.innerText();
    await dlg.getByRole("button", { name: "Cancel" }).click();
    if (!disabled || !/Needs Tether installed/.test(hint) || !/cannot sign back in/.test(warn)) throw new Error(JSON.stringify({ disabled, hint, warn }));
    return { hint: hint.replace(/\s+/g, " "), button: (await dlg.count()) === 0 };
  });

  // 4. File transfer from the session: no second request, one card, upload and download.
  await v.getByRole("button", { name: "Transfer files" }).click();
  const ft = await until("file transfer window", () => pages(vB).find((p) => p.url().includes("/session/") && p.url().includes("file-transfer")) ?? pages(vB).find((p) => p !== v && p.url().includes("/session/")), 20000);
  await check("file transfer opens without a second accept on TetherQS; one card there", async () => {
    const t0 = Date.now();
    let sawRequest = false;
    const remote = await until("remote home folder listed (not the password screen)", async () => {
      if ((await text(qs)).includes("wants to transfer files")) sawRequest = true;
      const x = await text(ft);
      if (x.includes("Password required")) throw new Error("password screen shown");
      // Both panes may show that path (technician in the VM): the remote one must be past "Connecting…".
      return x.includes("C:\\Users\\customer") && !/Connecting…|Waiting for the person/.test(x) ? x : null;
    }, 30000, 200);
    const files = await until("Transferring files on the session card", async () => (await text(qs)).includes("Transferring files"), 5000).catch(() => false);
    const cards = await qs.getByRole("button", { name: "Disconnect" }).count();
    if (sawRequest || cards !== 1 || !files) throw new Error(JSON.stringify({ sawRequest, cards, files }));
    return { ms: Date.now() - t0, cards, indicator: files, listed: remote.includes("Desktop") };
  });

  // Within one live session nothing asks again: Refresh, a reloaded window.
  await check("Refresh in the file-transfer window: no password, no new request", async () => {
    await ft.getByRole("button", { name: "Refresh" }).nth(1).click();
    await calm("file transfer", ft, qs, () => ft.getByText("Password required").count(), 5000);
    const x = await text(ft);
    if (!x.includes("C:\\Users\\customer")) throw new Error(x.slice(0, 300));
  });
  await check("reloading the file-transfer window (F5): no password, no new request", async () => {
    await ft.reload();
    await calm("file transfer", ft, qs, () => ft.getByText("Password required").count(), 8000);
    await until("remote folder listed again", async () => {
      const x = await text(ft);
      return x.includes("C:\\Users\\customer") && !/Connecting…|Waiting for the person/.test(x);
    }, 20000);
  });

  // Source file: on this PC, or (technician in the VM) the VM's own copy of the technician app.
  const VM_DIR = "C:\\Users\\Public\\TetherTech";
  const dir = IN_VM ? VM_DIR : join(tmpdir(), `tether-ft-${Date.now()}`);
  const name = IN_VM ? "tether.exe" : `tether-ft-${Date.now()}.bin`;
  const up = `tether-ft-${Date.now()}.bin`;
  let body = null;
  if (!IN_VM) {
    mkdirSync(dir, { recursive: true });
    body = Buffer.alloc(3 * 1024 * 1024 + 123, 7);
    for (let i = 0; i < body.length; i += 4096) body[i] = i & 255;
    writeFileSync(join(dir, name), body);
  }
  const localSize = async (path, file) => (await ipc(ft, "fs_local_dir", { path, hidden: false })).entries.find((e) => e.name === file)?.size ?? -1;
  const srcSize = IN_VM ? await localSize(dir, name) : body.length;
  const remoteHome = "C:\\Users\\customer";
  await check(`upload a ${(srcSize / 1048576).toFixed(1)} MB file to the customer's home folder`, async () => {
    await ipc(ft, "fs_transfer", { key: ftKey(ft), id: 9001, path: `${dir}\\${name}`, to: `${remoteHome}\\${up}`, fileNum: 0, hidden: false, isRemote: false });
    // Done = listed with its full size (the file shows up as soon as the upload starts).
    const want = fmtSize(srcSize);
    await until(`uploaded file listed with ${want}`, async () => {
      await ipc(ft, "fs_remote_dir", { key: ftKey(ft), path: remoteHome, hidden: false });
      await sleep(700);
      const row = ft.locator("div", { hasText: up }).last();
      return (await row.count()) > 0 && (await row.innerText()).includes(want);
    }, 240000, 1000);
  });
  await check(IN_VM ? "download it back: same size" : "download it back: same bytes", async () => {
    const backName = `back-${up}`;
    await ipc(ft, "fs_transfer", { key: ftKey(ft), id: 9002, path: `${remoteHome}\\${up}`, to: `${dir}\\${backName}`, fileNum: 0, hidden: false, isRemote: true });
    if (IN_VM) {
      await until("downloaded", async () => (await localSize(dir, backName)) === srcSize, 240000, 1000);
      await ipc(ft, "fs_remove", { key: ftKey(ft), id: 9004, path: `${dir}\\${backName}`, isRemote: false, isDir: false }).catch(() => {});
    } else {
      const back = join(dir, backName);
      await until("downloaded", () => existsSync(back) && readFileSync(back).length === body.length, 60000, 500);
      if (!readFileSync(back).equals(body)) throw new Error("content differs");
    }
  });
  await ipc(ft, "fs_remove", { key: ftKey(ft), id: 9003, path: `${remoteHome}\\${up}`, isRemote: true, isDir: false }).catch(() => {});
  if (!IN_VM) rmSync(dir, { recursive: true, force: true });

  await check("closing file transfer leaves the session card on TetherQS", async () => {
    await ipc(ft, "plugin:window|close", { label: `s-${ftKey(ft)}` }).catch(() => ft.close());
    await until("indicator gone", async () => !(await text(qs)).includes("Transferring files"), 15000);
    if ((await qs.getByRole("button", { name: "Disconnect" }).count()) !== 1) throw new Error("session card gone");
  });

  await check("reconnect after a dropped connection: no password, no Accept", async () => {
    await ipc(v, "session_reconnect", { key, forceRelay: false });
    await calm("session", v, qs, () => v.locator("#pw").count(), 10000);
    await until("session active again", async () => (await qs.locator('[data-testid="qs-status"]').innerText()).includes("Session active") && !/Reconnecting|Connecting…/.test(await text(v)), 20000);
  });
  await check("reloading the session window (F5): no password, no Accept", async () => {
    await v.reload();
    await calm("session", v, qs, () => v.locator("#pw").count(), 10000);
    await until("first frame after the reload", () => v.evaluate(() => { const c = document.querySelector("canvas"); return !!c && !(c.width === 300 && c.height === 150); }), 30000);
  });

  await check("Files switched off on the session card: file transfer needs a new accept (declined)", async () => {
    const sw = qs.getByRole("switch", { name: "Transfer files" });
    if ((await sw.getAttribute("aria-checked")) === "true") await sw.click();
    await sleep(800);
    await toolbarUp(); // the toolbar hides itself after the reload above
    await v.getByRole("button", { name: "Transfer files" }).click();
    await until("a new file transfer request on TetherQS", async () => (await text(qs)).includes("wants to transfer files"), 20000);
    await qs.getByRole("button", { name: "Decline" }).click();
    await until("request gone", async () => !(await text(qs)).includes("wants to transfer files"), 10000);
    for (const p of pages(vB).filter((p) => p.url().includes("conn=file-transfer"))) await p.close().catch(() => {});
    await sw.click(); // back on
  });

  await check("Disconnect on TetherQS ends the session; window centred again", async () => {
    await qs.getByRole("button", { name: "Disconnect" }).click();
    await until("idle", async () => (await qs.locator('[data-testid="qs-status"]').innerText()).includes("Ready"), 15000);
    // Twice: a technician that lost the close reason reconnects a few seconds later and re-docks the window.
    for (const wait of [500, 8000]) {
      await sleep(wait);
      const r = await qs.evaluate(() => ({ x: screenX, w: outerWidth, sw: screen.availWidth }));
      if (Math.abs(r.x + r.w / 2 - r.sw / 2) > 60) throw new Error(`after ${wait} ms: ${JSON.stringify(r)}`);
      const st = await qs.locator('[data-testid="qs-status"]').innerText();
      if (!st.includes("Ready")) throw new Error(`after ${wait} ms: ${st}`);
    }
  });

  const ok = results.every((r) => r.ok);
  writeFileSync(OUT, JSON.stringify({ when: new Date().toISOString(), vm: VM_IP, key, results }, null, 2));
  console.log(`${results.filter((r) => r.ok).length}/${results.length} passed`);
  process.exitCode = ok ? 0 : 1;
}

/** For `ms`: neither a password screen on the technician's side nor a request on TetherQS. */
async function calm(what, page, qs, passwordShown, ms) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (await passwordShown().catch(() => 0)) throw new Error(`${what} shows the password screen`);
    const q = await text(qs).catch(() => "");
    if (/wants to (see and control|transfer files)/.test(q)) throw new Error(`TetherQS asks again: ${q.slice(0, 200)}`);
    await sleep(200);
  }
}

/** Same as FileTransfer.tsx fmtSize. */
function fmtSize(n) {
  const u = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) {
    n /= 1024;
    i++;
  }
  return `${n.toFixed(i ? 1 : 0)} ${u[i]}`;
}

function ftKey(p) {
  return p.url().match(/session\/([0-9a-f]{32})/)[1];
}

main()
  .catch((e) => {
    record("run", false, { error: e.message });
    process.exitCode = 1;
  })
  .finally(() => {
    if (IN_VM) {
      try {
        broker("-Stop");
      } catch {}
    }
    for (const p of procs) {
      try {
        execFileSync("taskkill", ["/PID", String(p.pid), "/T", "/F"], { stdio: "ignore" });
      } catch {}
    }
    setTimeout(() => process.exit(), 500);
  });

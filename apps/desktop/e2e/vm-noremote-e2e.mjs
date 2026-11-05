// End-to-end: remote input never reaches Tether's own windows on the host (0.5.6).
// Technician app on this PC on an invisible desktop (no window or focus here) -> TetherQS in the
// Hyper-V VM. Real remote clicks and keys go to TetherQS (permission switches, Disconnect, the
// recording consent's Allow) and must change nothing; the same input on Notepad in the VM must
// work, and Alt+Tab away from TetherQS too.
//
//   node e2e/vm-noremote-e2e.mjs
// Needs: the new TetherQS in the VM (run-elevated.ps1 vm-p2p.ps1 -Qs <exe>), the broker, the corp
// proxy if the VM is in corporate mode, C:\vms\vm-noremote.ps1 (Notepad / foreground helpers).
// Env: TECH_EXE (C:\cargo-target\noremote\release\tether.exe), VM_IP (172.30.252.30), QS_CDP (9334).
import { execFileSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { chromium } from "playwright-core";

const ROOT = resolve(import.meta.dirname, "../../..");
const TECH = process.env.TECH_EXE || "C:\\cargo-target\\noremote\\release\\tether.exe";
const VM_IP = process.env.VM_IP || "172.30.252.30";
const QS_CDP = `http://${VM_IP}:${process.env.QS_CDP || 9334}`;
const HIDDEN = resolve(import.meta.dirname, "hidden-run.ps1");
const CDP_PORT = 9243;
const OUT = join(ROOT, "tools/bench/results/vm-noremote-e2e.json");
const NOTICE = "can only be used by the person at that computer";
const MOUSE = { MOVE: 0, DOWN: 1, UP: 2, WHEEL: 3 };
const LEFT = 1 << 3;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const pages = (b) => b.contexts().flatMap((c) => c.pages());
const ipc = (p, cmd, args = {}) => p.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a), [cmd, args]);
const text = (p) => p.evaluate(() => document.body.innerText);
const results = [];

function record(name, ok, detail = {}) {
  results.push({ name, ok, ...detail });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}`, Object.keys(detail).length ? JSON.stringify(detail) : "");
}
const check = async (name, fn) => {
  try {
    record(name, true, (await fn()) ?? {});
  } catch (e) {
    record(name, false, { error: e.message });
  }
};
async function until(what, fn, timeout = 30000, every = 200) {
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
function vm(...a) {
  const out = execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "C:\\vms\\broker\\run-elevated.ps1", "vm-noremote.ps1", ...a]).toString();
  const line = out.split(/\r?\n/).find((l) => l.startsWith("RESULT ") || l.startsWith("FAILED"));
  if (!line || line.startsWith("FAILED") || line.includes("FAILED")) throw new Error(`vm-noremote ${a.join(" ")}: ${out.slice(-300)}`);
  return line.slice(7);
}
function killTech() {
  const ps = `Get-CimInstance Win32_Process -Filter "Name='tether.exe'" | Where-Object { $_.ExecutablePath -eq '${TECH}' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }`;
  execFileSync("powershell", ["-NoProfile", "-Command", ps], { stdio: "ignore" });
}

async function main() {
  const qsB = await until("TetherQS devtools in the VM", () => chromium.connectOverCDP(QS_CDP), 60000, 1000);
  const qs = await until("TetherQS page", () => pages(qsB).find((p) => !p.url().startsWith("devtools")), 20000);
  await until("TetherQS ready", async () => (await qs.locator('[data-testid="qs-status"]').innerText()).includes("Ready"), 90000, 1000);
  const id = (await qs.locator('[data-testid="qs-id"]').innerText()).replace(/\s/g, "");
  const pw = ((await qs.locator('[data-testid="qs-password"]').getAttribute("data-password")) ?? "").trim();
  console.log(`host ${id}, TetherQS ${await ipc(qs, "app_version").catch(() => "?")}`);

  killTech();
  const env = { ...process.env, TETHER_PROFILE: "noremote-tech", WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${CDP_PORT}`, TETHER_E2E_PW: pw };
  execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", HIDDEN, "-Exe", TECH, "-Args", `--connect ${id} --password-env TETHER_E2E_PW`], { env, stdio: "ignore" });
  const vB = await until("technician devtools", () => chromium.connectOverCDP(`http://127.0.0.1:${CDP_PORT}`), 120000, 500);
  const v = await until("session window", () => pages(vB).find((p) => p.url().includes("/session/")), 40000);
  const key = v.url().match(/session\/([0-9a-f]{32})/)[1];
  await until("session active", async () => {
    const accept = qs.getByRole("button", { name: "Accept" });
    if (await accept.isVisible().catch(() => false)) await accept.click();
    return (await qs.locator('[data-testid="qs-status"]').innerText()).includes("Session active");
  }, 40000, 300);
  await until("first frame", () => v.evaluate(() => { const c = document.querySelector("canvas"); return !!c && !(c.width === 300 && c.height === 150); }), 40000);
  await qs.locator('[data-testid="qs-tab-perm"]').click().catch(() => {});
  await sleep(1500); // TetherQS docks bottom right

  // Remote input, exactly what the session window sends.
  const move = (x, y) => ipc(v, "session_mouse", { key, mask: MOUSE.MOVE, x, y, alt: false, ctrl: false, shift: false, command: false });
  const click = async (x, y) => {
    await move(x, y);
    await sleep(150);
    for (const t of [MOUSE.DOWN, MOUSE.UP]) {
      await ipc(v, "session_mouse", { key, mask: t | LEFT, x, y, alt: false, ctrl: false, shift: false, command: false });
      await sleep(60);
    }
  };
  const keyEv = (code, down) => ipc(v, "session_key", { key, code, down, lockModes: 0, character: "" });
  const tap = async (code, ...mods) => {
    for (const m of mods) await keyEv(m, true);
    await keyEv(code, true);
    await keyEv(code, false);
    for (const m of mods.reverse()) await keyEv(m, false);
    await sleep(120);
  };
  const type = async (s) => {
    for (const ch of s) await tap(`Key${ch.toUpperCase()}`);
  };
  // Centre of an element of the TetherQS page in screen pixels (= remote coordinates, one display).
  const centre = (loc) =>
    loc.evaluate((el) => {
      const r = el.getBoundingClientRect();
      const bx = (outerWidth - innerWidth) / 2;
      const top = outerHeight - innerHeight - bx;
      return { x: Math.round((screenX + bx + r.left + r.width / 2) * devicePixelRatio), y: Math.round((screenY + top + r.top + r.height / 2) * devicePixelRatio) };
    });
  const switches = async () => {
    const s = qs.getByRole("switch");
    const out = {};
    for (let i = 0; i < (await s.count()); i++) out[await s.nth(i).getAttribute("aria-label")] = await s.nth(i).getAttribute("aria-checked");
    return out;
  };
  const active = async () => (await qs.locator('[data-testid="qs-status"]').innerText()).includes("Session active");
  const hovered = () => qs.evaluate(() => [...document.querySelectorAll(":hover")].pop()?.closest('[role="switch"],button')?.getAttribute("aria-label") ?? [...document.querySelectorAll(":hover")].pop()?.closest("button")?.innerText ?? null);
  const before = await switches();
  if (Object.keys(before).length < 2) throw new Error(`permission switches on TetherQS: ${JSON.stringify(before)}`);

  // 1. Other apps keep working: click into Notepad, type, save.
  const np = vm("-Action", "notepad");
  const [l, t, r, b] = np.match(/rect=(-?\d+),(-?\d+),(-?\d+),(-?\d+)/).slice(1).map(Number);
  const npText = { x: Math.round((l + r) / 2), y: Math.round(t + (b - t) * 0.6) };
  await check("Notepad in the VM: remote click and typing work, Ctrl+S saves", async () => {
    await click(npText.x, npText.y);
    await type("abc");
    await tap("KeyS", "ControlLeft");
    const c = await until("saved text", () => {
      const x = vm("-Action", "read");
      return x.includes("abc") ? x : null;
    }, 15000, 1000);
    return { notepad: np, file: c };
  });

  // 2. Clicks on TetherQS change nothing. The pointer still gets there (moves pass).
  await check("pointer moves still reach the TetherQS window (hover)", async () => {
    const sw = qs.getByRole("switch").first();
    const p = await centre(sw);
    await move(p.x, p.y);
    const h = await until("hover on the switch", async () => ((await hovered()) === (await sw.getAttribute("aria-label")) ? hovered() : null), 5000);
    return { at: p, hovered: h };
  });
  await v.evaluate((n) => {
    window.__notices = 0;
    new MutationObserver(() => {
      if (document.body.innerText.includes(n)) window.__notices++;
    }).observe(document.body, { childList: true, subtree: true, characterData: true });
  }, NOTICE);
  await check("remote clicks on every permission switch of TetherQS change nothing", async () => {
    const s = qs.getByRole("switch");
    for (let i = 0; i < (await s.count()); i++) {
      const p = await centre(s.nth(i));
      await click(p.x, p.y);
      await sleep(250);
    }
    await sleep(1500);
    const after = await switches();
    if (JSON.stringify(after) !== JSON.stringify(before)) throw new Error(JSON.stringify({ before, after }));
    return { switches: after };
  });
  await check("technician is told why (notice in the session window)", async () => {
    const n = await until("notice", () => v.evaluate(() => window.__notices), 5000, 100);
    return { notice_mutations: n };
  });
  await check("remote click on Disconnect (and the window's Close) does not end the session", async () => {
    const d = await centre(qs.getByRole("button", { name: "Disconnect" }));
    await click(d.x, d.y);
    const c = await centre(qs.getByRole("button", { name: "Close" }));
    await click(c.x, c.y);
    await sleep(2500);
    if (!(await active())) throw new Error("session ended");
    return { disconnect: d, close: c };
  });
  await check("remote click on Allow of the recording consent grants nothing", async () => {
    await ipc(v, "session_record", { key, start: true });
    const dlg = qs.getByRole("dialog");
    await until("consent dialog on TetherQS", () => dlg.isVisible(), 15000);
    const a = await centre(dlg.getByRole("button", { name: "Allow", exact: true }));
    await click(a.x, a.y);
    vm("-Action", "activate", "-Name", "TetherQS"); // Enter goes to the window in front
    await keyEv("Enter", true);
    await keyEv("Enter", false);
    await sleep(2000);
    const still = await dlg.isVisible();
    const rec = await qs.getByText("Recording allowed", { exact: true }).isVisible().catch(() => false);
    // Answer it locally (the person at the computer can).
    if (still) await dlg.getByRole("button", { name: "Don't allow" }).click();
    if (!still || rec) throw new Error(JSON.stringify({ dialogStillOpen: still, rec }));
    return { allow: a };
  });

  // 3. Keys while TetherQS is in front: nothing reaches it; Alt+Tab leaves it.
  await check("keys on TetherQS in front (Enter, Space, Esc, Tab, letters, Alt+F4) change nothing", async () => {
    const fg = vm("-Action", "activate", "-Name", "TetherQS");
    if (!fg.includes("fg=TetherQS")) throw new Error(`could not bring TetherQS to the front: ${fg}`);
    await qs.getByRole("button", { name: "Disconnect" }).focus();
    for (const k of ["Enter", "Space", "Escape", "Tab", "Space", "Enter", "KeyX"]) await tap(k);
    const s1 = await switches();
    const ok = (await active()) && JSON.stringify(before) === JSON.stringify(s1);
    if (!ok) throw new Error(JSON.stringify({ active: await active(), before, s1 }));
    // Alt+F4: F4 is dropped and the window stays; the switches are compared again after Alt+Tab.
    await tap("F4", "AltLeft");
    await sleep(2000);
    // Asked from the page: the helper process `vm -Action fg` starts takes the foreground itself.
    const focused = await Promise.race([qs.evaluate(() => document.hasFocus()), sleep(3000).then(() => "stalled")]);
    if (focused !== true) throw new Error(JSON.stringify({ focused }));
    return { tetherqs_focused: focused };
  });
  await check("Alt+Tab away from TetherQS works; Notepad gets keys again", async () => {
    await tap("Tab", "AltLeft");
    const fg = await until("another window in front", () => {
      const x = vm("-Action", "fg");
      return x.includes("TetherQS") ? null : x;
    }, 10000, 1000);
    // Whatever Alt+Tab picked: bring Notepad up (locally) and type remotely.
    if (!fg.includes("Notepad")) vm("-Action", "activate", "-Name", "Notepad");
    await type("d");
    await tap("KeyS", "ControlLeft");
    const c = await until("saved text", () => {
      const x = vm("-Action", "read");
      return x.includes("abcd") ? x : null;
    }, 15000, 1000);
    return { after_alt_tab: fg, file: c };
  });
  // Before 0.5.6's fix a lone Alt (Alt+F4: F4 dropped) put TetherQS into keyboard menu mode: its
  // WebView (and DevTools) stalled until the next key or click.
  await check("lone Alt and Alt+F4 on TetherQS do not stall it: it repaints, one remote click on Notepad works", async () => {
    const repaints = () =>
      Promise.race([qs.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(() => r(true))))), sleep(2000).then(() => false)]);
    const out = {};
    let want = "abcd";
    for (const [name, keys, letter] of [["Alt", ["AltLeft"], "e"], ["Alt+F4", ["F4", "AltLeft"], "f"]]) {
      const fg = vm("-Action", "activate", "-Name", "TetherQS");
      if (!fg.includes("fg=TetherQS")) throw new Error(`${name}: could not bring TetherQS to the front: ${fg}`);
      await tap(...keys);
      await sleep(300);
      if (!(await repaints())) throw new Error(`${name}: TetherQS stopped repainting`);
      // One click, then type at once: the letter lands in Notepad only if that click activated it.
      await click(npText.x, npText.y);
      await type(letter);
      await tap("KeyS", "ControlLeft");
      want += letter;
      out[name] = await until(`${name}: "${letter}" typed into Notepad after one click`, () => {
        const x = vm("-Action", "read");
        return x.includes(want) ? x : null;
      }, 15000, 1000);
    }
    return out;
  });
  await check("session still active, permissions as before", async () => {
    const after = await switches();
    if (!(await active()) || JSON.stringify(after) !== JSON.stringify(before)) throw new Error(JSON.stringify({ before, after }));
  });

  try {
    vm("-Action", "close");
  } catch {}
  await qs.getByRole("button", { name: "Disconnect" }).click().catch(() => {});
  const ok = results.every((x) => x.ok);
  writeFileSync(OUT, JSON.stringify({ when: new Date().toISOString(), vm: VM_IP, results }, null, 2));
  console.log(`${results.filter((x) => x.ok).length}/${results.length} passed`);
  process.exitCode = ok ? 0 : 1;
}

main()
  .catch((e) => {
    record("run", false, { error: e.message });
    process.exitCode = 1;
  })
  .finally(() => {
    try {
      killTech();
    } catch {}
    setTimeout(() => process.exit(), 500);
  });

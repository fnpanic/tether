// End-to-end: UAC prompts on a TetherQS host (portable, standard user) never strand the viewer.
// Technician app on this PC on an invisible desktop -> TetherQS in the Hyper-V VM. The UAC prompt on
// the VM's secure desktop is answered with Hyper-V console keys (C:\vms\vm-uac-key.ps1: password +
// Enter = Yes, Esc = No); nothing touches this PC's desktop.
//
//   node e2e/vm-uac-e2e.mjs [--timeout-s 180]
// Order: No to "Request admin access", unanswered prompt (timeout), unrelated prompt (No), Yes,
// then an unrelated prompt with admin access on (visible to the technician, answered No).
// Needs: the broker, C:\vms\vm-uac-key.ps1 and vm-uac-qs.ps1, TetherQS started with CDP (vm-uac-qs.ps1 -Action restart).
// Env: TECH_EXE (C:\cargo-target\uac\debug\tether.exe), VM_IP (172.30.252.30), QS_CDP (9334).
import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { chromium } from "playwright-core";

const ROOT = resolve(import.meta.dirname, "../../..");
const TECH = process.env.TECH_EXE || "C:\\cargo-target\\uac\\debug\\tether.exe";
const VM_IP = process.env.VM_IP || "172.30.252.30";
const QS_CDP = `http://${VM_IP}:${process.env.QS_CDP || 9334}`;
const HIDDEN = resolve(import.meta.dirname, "hidden-run.ps1");
const CDP_PORT = 9244;
const OUT = join(ROOT, "tools/bench/results/vm-uac-e2e.json");
const SHOTS = join(ROOT, "tools/bench/results/vm-uac");
const ti = process.argv.indexOf("--timeout-s");
const TIMEOUT_S = ti > 0 ? Number(process.argv[ti + 1]) : 180;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const pages = (b) => b.contexts().flatMap((c) => c.pages());
const ipc = (p, cmd, args = {}) => p.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a), [cmd, args]);
const results = [];
mkdirSync(SHOTS, { recursive: true });

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
  throw new Error(`timeout waiting for ${what}${last ? `: ${last.message}` : ""}`);
}
const broker = (script, ...a) => execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "C:\\vms\\broker\\run-elevated.ps1", script, ...a]).toString();
const key = (action, shot) => broker("vm-uac-key.ps1", "-Action", action, "-Out", join(SHOTS, `${shot}.png`));
const consent = async () => /proc \d+ consent\.exe/.test(broker("vm-uac-qs.ps1", "-Action", "state"));
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
  console.log(`host ${id}, TetherQS ${await ipc(qs, "app_version").catch(() => "?")}, elevated=${(await ipc(qs, "state")).elevated}`);

  killTech();
  const env = { ...process.env, TETHER_PROFILE: "uac-tech", WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${CDP_PORT}`, TETHER_E2E_PW: pw };
  execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", HIDDEN, "-Exe", TECH, "-Args", `--connect ${id} --password-env TETHER_E2E_PW`], { env, stdio: "ignore" });
  const vB = await until("technician devtools", () => chromium.connectOverCDP(`http://127.0.0.1:${CDP_PORT}`), 120000, 500);
  const v = await until("session window", () => pages(vB).find((p) => p.url().includes("/session/")), 40000);
  await until("session active", async () => {
    const accept = qs.getByRole("button", { name: "Accept" });
    if (await accept.isVisible().catch(() => false)) await accept.click();
    return (await qs.locator('[data-testid="qs-status"]').innerText()).includes("Session active");
  }, 40000, 300);
  await until("first frame", () => v.evaluate(() => { const c = document.querySelector("canvas"); return !!c && !(c.width === 300 && c.height === 150); }), 40000);
  // Session events as the window gets them (msgbox, cancel_msgbox, portable_service), with times.
  await v.evaluate(() => {
    const T = window.__TAURI_INTERNALS__;
    window.__ev = [];
    const t0 = Date.now();
    return T.invoke("plugin:event|listen", { event: "session", target: { kind: "Any" }, handler: T.transformCallback((e) => { const n = e.payload?.name; if (/msgbox|portable/.test(n)) window.__ev.push(`${((Date.now() - t0) / 1000).toFixed(1)}s ${n} ${JSON.stringify(e.payload.payload).slice(0, 120)}`); }) });
  });
  const body = () => v.evaluate(() => document.body.innerText);
  const notes = () => v.evaluate(() => [...document.querySelectorAll('[data-testid="host-note"]')].map((e) => e.textContent));
  // Still a working session: no error overlay, the picture shown, the toolbar there.
  const alive = async () => {
    const t = await body();
    if (/Please wait for confirmation of UAC|Prompt\s*\n/.test(t)) throw new Error("UAC dialog overlay shown");
    const ok = await v.evaluate(() => getComputedStyle(document.querySelector("canvas")).opacity === "1" && !!document.querySelector('[data-testid="toolbar"]'));
    if (!ok) throw new Error("picture or toolbar gone");
    return true;
  };
  const reveal = () => v.evaluate(() => document.querySelector('[aria-label="Show toolbar"]')?.click());
  const request = async () => {
    await reveal();
    await v.getByRole("button", { name: "Remote actions" }).click();
    const item = v.getByRole("button", { name: /Request admin access/ });
    const hint = await item.innerText();
    if (await item.isDisabled()) return { disabled: true, hint };
    await item.click();
    return { disabled: false, hint };
  };
  const shot = (n) => v.screenshot({ path: join(SHOTS, `${n}.png`) });
  const uacUp = async (what) => {
    await until(`${what}: consent.exe`, consent, 40000, 1000);
    const n = await until(`${what}: UAC notice`, async () => (await notes()).find((x) => x.includes("admin prompt (UAC) is open")), 15000);
    await alive();
    return n;
  };

  // 1. Request admin access, the person clicks No.
  await check("No: request -> notice (no dialog) -> No -> notice gone, 'declined' toast, session goes on", async () => {
    const r = await request();
    if (r.disabled) throw new Error(`item disabled: ${r.hint}`);
    await until("'requested' toast", async () => (await body()).includes("Admin access requested"), 10000);
    const n = await uacUp("No");
    await shot("1-uac-open");
    const k = key("no", "1-vm-after-no");
    const t0 = Date.now();
    await until("notice gone", async () => (await notes()).length === 0, 15000);
    const gone = Date.now() - t0;
    const toast = await until("declined toast", async () => (await body()).match(/Admin access was declined[^\n]*/)?.[0], 15000);
    await shot("1-after-no");
    await alive();
    return { notice: n, gone_ms: gone, toast, vm: k.trim().split(/\r?\n/).filter((l) => l.startsWith("consent")) };
  });

  // 2. Nobody answers: the session must stay usable; does Windows close the prompt by itself?
  await check(`Timeout: unanswered prompt for up to ${TIMEOUT_S} s, session stays usable`, async () => {
    await sleep(6000);
    const r = await request();
    if (r.disabled) throw new Error(`item disabled: ${r.hint}`);
    await uacUp("timeout");
    const t0 = Date.now();
    let closedBy = "windows";
    while (await consent()) {
      await alive();
      if (Date.now() - t0 > TIMEOUT_S * 1000) {
        closedBy = "esc after the wait";
        key("no", "2-vm-timeout");
        break;
      }
      await sleep(10000);
    }
    const s = Math.round((Date.now() - t0) / 1000);
    await until("notice gone", async () => (await notes()).length === 0, 15000);
    const toast = await until("declined toast", async () => (await body()).match(/Admin access was declined[^\n]*/)?.[0], 15000);
    await alive();
    return { open_s: s, closed_by: closedBy, toast };
  });

  // 3. A prompt Tether did not ask for (cmd.exe "Run as administrator"), answered No.
  await check("Unrelated prompt: notice, No -> notice gone, neutral toast", async () => {
    await sleep(6000);
    broker("vm-uac-qs.ps1", "-Action", "trigger");
    await uacUp("unrelated");
    key("no", "3-vm-after-no");
    await until("notice gone", async () => (await notes()).length === 0, 15000);
    const toast = await until("neutral toast", async () => (await body()).match(/admin prompt on the remote computer was answered or closed[^\n]*/)?.[0], 15000);
    await alive();
    return { toast };
  });

  // 4. Request again, the person confirms (over-the-shoulder admin credentials + Enter).
  await check("Yes: request again -> Yes -> 'admin access is on', item disabled, TetherQS elevated", async () => {
    await sleep(6000);
    await request();
    await uacUp("Yes");
    key("yes", "4-vm-after-yes");
    const t0 = Date.now();
    await until("notice gone", async () => (await notes()).length === 0, 15000);
    const toast = await until("admin-on toast", async () => (await body()).match(/Admin access is on[^\n]*/)?.[0], 20000);
    const ms = Date.now() - t0;
    await shot("4-after-yes");
    await alive();
    if ((await body()).includes("declined")) throw new Error("declined toast after Yes");
    const r = await request();
    await v.getByRole("button", { name: "Remote actions" }).click(); // close the menu
    const qsElevated = await until("TetherQS reports admin access", async () => (await ipc(qs, "state")).elevated, 15000, 1000);
    return { toast, after_ms: ms, item: r, qs_elevated: qsElevated };
  });

  // 5. With admin access the technician sees the secure desktop; no notice, no dialog.
  await check("With admin access: an unrelated prompt is visible in the picture, no notice", async () => {
    await sleep(3000);
    broker("vm-uac-qs.ps1", "-Action", "trigger");
    await until("consent.exe", consent, 40000, 1000);
    await sleep(4000);
    await shot("5-uac-visible");
    const n = await notes();
    await alive();
    key("no", "5-vm-after-no");
    if (n.length) throw new Error(`notice shown: ${n}`);
    return {};
  });

  const ev = await v.evaluate(() => window.__ev);
  console.log(ev.join("\n"));
  writeFileSync(OUT, JSON.stringify({ at: new Date().toISOString(), host: id, results, events: ev }, null, 1));
  await v.evaluate(() => window.close()).catch(() => {});
  await sleep(2000);
  killTech();
  await qsB.close().catch(() => {});
  await vB.close().catch(() => {});
}

main()
  .catch((e) => record("run", false, { error: e.message }))
  .finally(() => {
    const failed = results.filter((r) => !r.ok).length;
    console.log(`${results.length - failed}/${results.length} passed`);
    process.exit(failed ? 1 : 0);
  });

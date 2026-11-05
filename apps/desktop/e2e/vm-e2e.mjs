// End-to-end: technician app on this PC  ->  TetherQS in the Hyper-V VM (corporate mode:
// outbound blocked except a TLS-inspecting proxy, TetherQS running as a standard user).
// Drives only the apps' own webviews over CDP (no OS-level input on either machine).
//
//   node e2e/vm-e2e.mjs [--skip-reconnect] [--skip-recording]
// Env: VM_IP (172.30.252.30), QS_CDP (9334, port-proxied in the VM), TECH_EXE, SSH_KEY.
import { spawn, execFileSync } from "node:child_process";
import { existsSync, readdirSync, statSync, writeFileSync, mkdirSync } from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";
import { chromium } from "playwright-core";

const ROOT = resolve(import.meta.dirname, "../../..");
const VM_IP = process.env.VM_IP || "172.30.252.30";
const QS_CDP = `http://${VM_IP}:${process.env.QS_CDP || 9334}`;
const TECH = process.env.TECH_EXE || "C:\\cargo-target\\tether\\release\\tether.exe";
const SSH_KEY = process.env.SSH_KEY || join(homedir(), ".ssh", "tether_oracle");
const OUT = join(ROOT, "tools/bench/results/vm-e2e.json");
const REC_DIR = join(homedir(), "Videos", "Tether");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const results = [];
const procs = [];

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
const text = (p) => p.evaluate(() => document.body.textContent);

async function main() {
  // 1. TetherQS in the VM: ready, ID + one-time password, branding from the signed policy.
  const qsB = await until("TetherQS devtools in the VM", () => chromium.connectOverCDP(QS_CDP), 60000, 1000);
  const qs = await until("TetherQS page", () => pages(qsB).find((p) => p.url().includes("index.html") || p.url().startsWith("tauri") || p.url().startsWith("http")), 20000);
  await until("TetherQS ready", async () => (await qs.locator('[data-testid="qs-status"]').innerText()).includes("Ready"), 90000, 1000);
  const id = (await qs.locator('[data-testid="qs-id"]').innerText()).replace(/\s/g, "");
  const pw = ((await qs.locator('[data-testid="qs-password"]').getAttribute("data-password")) ?? "").trim();
  const st = await ipc(qs, "state");
  record("TetherQS ready behind TLS-inspecting proxy as standard user", true, { id, title: st.branding?.title, elevated: st.elevated, policy_serial: st.policy_serial ?? st.policy?.serial });

  // 2. Technician connects from this PC via the CLI, password over stdin (not in the process list).
  const t = spawn(TECH, ["--connect", id, "--password-stdin"], {
    env: { ...process.env, TETHER_PROFILE: "vm-tech", WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9231" },
    stdio: ["pipe", "ignore", "ignore"],
  });
  procs.push(t);
  t.stdin.end(pw + "\n");
  const vB = await until("technician devtools", () => chromium.connectOverCDP("http://127.0.0.1:9231"), 40000, 500);
  const v = await until("session window", () => pages(vB).find((p) => p.url().includes("/session/")), 40000);
  const key = v.url().match(/session\/([0-9a-f]{32})/)[1];
  const t0 = Date.now();
  // approve-mode "both": the customer may still need to click Accept.
  const accepted = await until("first frame (accepting on TetherQS if asked)", async () => {
    const accept = qs.getByRole("button", { name: "Accept" });
    if (await accept.isVisible().catch(() => false)) await accept.click();
    return (await qs.locator('[data-testid="qs-status"]').innerText()).includes("Session active") && (await v.evaluate(() => { const c = document.querySelector("canvas"); return !!c && !(c.width === 300 && c.height === 150); }));
  }, 60000, 500);
  record("CLI connect (--password-stdin) + first frame", !!accepted, { ms: Date.now() - t0 });
  await ipc(v, "plugin:window|minimize", { label: `s-${key}` }).catch(() => {});

  if (!process.argv.includes("--skip-recording")) {
    // 3. Recording needs the customer's consent: Deny first, then Allow.
    const before = new Set(existsSync(REC_DIR) ? readdirSync(REC_DIR) : []);
    const newFiles = () => (existsSync(REC_DIR) ? readdirSync(REC_DIR) : []).filter((f) => !before.has(f));
    // The technician's own toolbar button (DOM click: works while the window is minimised).
    const toolbar = (name) =>
      v.getByRole("button", { name, exact: true, includeHidden: true }).dispatchEvent("click", undefined, { timeout: 15000 }).catch(async (e) => {
        const labels = await v.evaluate(() => [...document.querySelectorAll("button")].map((b) => b.getAttribute("aria-label") || b.title || b.textContent.trim()));
        console.log("viewer buttons:", JSON.stringify(labels));
        throw e;
      });
    await toolbar("Record session");
    await until("consent prompt on TetherQS", () => qs.getByRole("button", { name: "Don't allow" }).isVisible(), 20000).catch(async (e) => {
      console.log("QS window:", (await text(qs)).replace(/\n+/g, " | ").slice(0, 400));
      console.log("viewer:", (await text(v)).replace(/\n+/g, " | ").slice(0, 300));
      throw e;
    });
    await qs.getByRole("button", { name: "Don't allow" }).click();
    const declined = await until("viewer shows declined", async () => (await text(v)).includes("declined"), 15000).catch(() => false);
    await sleep(3000);
    record("recording denied by customer: nothing recorded", !!declined && newFiles().length === 0, { declined_shown: !!declined, files: newFiles().length });
    await toolbar("Record session");
    await until("consent prompt again", () => qs.getByRole("button", { name: "Allow", exact: true }).isVisible(), 20000);
    await qs.getByRole("button", { name: "Allow", exact: true }).click();
    await until("REC indicator", async () => (await text(v)).includes("REC"), 20000);
    await sleep(8000);
    await toolbar("Stop recording");
    // The "Recording saved" card (after the recorder closed and renamed the file).
    const card = await until("Recording saved card", () => v.locator('[data-testid="recording-saved"]').innerText({ timeout: 500 }).catch(() => ""), 20000).catch(() => "");
    const listed = await ipc(v, "recordings_list").catch((e) => ({ error: String(e) }));
    const name = (card.match(/\d{4}-\d{2}-\d{2}_\d{2}-\d{2}-\d{2}_\S+\.(webm|mp4)/) ?? [""])[0];
    record("recording saved card with Open / Show in folder, file in the list", !!name && /Open/.test(card) && /Show in folder/.test(card) && !!listed.items?.some((r) => r.name === name && r.peer_id === id), {
      card: card.replace(/\n+/g, " | "),
      folder: listed.folder,
      listed: listed.items?.slice(0, 3).map((r) => `${r.name} ${r.duration_s}s ${r.size}B`),
    });
    const files = (existsSync(REC_DIR) ? readdirSync(REC_DIR) : []).filter((f) => !before.has(f));
    const file = files.map((f) => join(REC_DIR, f)).sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs)[0];
    let dur = 0;
    if (file) {
      try {
        dur = parseFloat(execFileSync("ffprobe", ["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", file]).toString());
      } catch {}
    }
    record("recording allowed: file written and playable", !!file && dur > 4, { file, seconds: dur, size: file ? statSync(file).size : 0 });
  }

  if (!process.argv.includes("--skip-reconnect")) {
    // 4. Relay failures mid-session. Interruption = failure -> first picture drawn afterwards
    // (viewer clock; a hook timestamps every canvas draw). A session that was not on the failed
    // relay is simply not interrupted, which is what high availability should give.
    const RELAYS = ["opc@158.101.205.185", "opc@92.5.231.71"];
    const ssh = (host, cmd) => execFileSync("ssh", ["-i", SSH_KEY, "-o", "ConnectTimeout=20", host, cmd], { stdio: "ignore" });
    await v.evaluate(() => {
      if (window.__dh) return;
      window.__dh = 1;
      window.__draws = [];
      const o = CanvasRenderingContext2D.prototype.drawImage;
      CanvasRenderingContext2D.prototype.drawImage = function (...a) {
        window.__draws.push(performance.now());
        return o.apply(this, a);
      };
    });
    const outage = async (label, fail, restore) => {
      const t0 = await v.evaluate(() => performance.now());
      fail();
      const tFail = await v.evaluate(() => performance.now());
      const saw = await until("reconnecting overlay", async () => (await text(v)).includes("Reconnecting"), 8000, 100).then(() => true, () => false);
      const tSeen = await v.evaluate(() => performance.now());
      let first = null;
      if (saw) {
        await until("session resumed", async () => !(await text(v)).includes("Reconnecting"), 60000, 100).catch(() => {});
        first = await until("first picture after the failure", () => v.evaluate((tf) => window.__draws.find((t) => t > tf) ?? null, tFail), 20000, 100).catch(() => null);
      }
      if (restore) restore();
      // upper bound: from before the SSH command that causes the failure (includes its round trip);
      // recovery: from the moment the viewer saw the drop (100 ms polling) to the next picture.
      const upper = first != null ? Math.round(first - t0) : null;
      const recovery = first != null ? Math.round(first - tSeen) : null;
      record(label, !saw || (recovery != null && recovery <= 2000), saw ? { recovery_ms: recovery, upper_bound_ms: upper, ssh_cmd_ms: Math.round(tFail - t0) } : { interrupted: false });
      await sleep(6000);
    };
    // Which relay carries the session: each hbbr reports its open sessions at /readyz/relay.
    const NAMES = ["tether-desk.duckdns.org", "tether-desk2.duckdns.org"];
    const carrier = async () => {
      for (let i = 0; i < NAMES.length; i++) {
        const body = await fetch(`https://${NAMES[i]}/readyz/relay`).then((r) => r.text()).catch(() => "");
        if (/sessions=([1-9]\d*)/.test(body)) return i;
      }
      return -1;
    };
    let c = await carrier();
    if (c < 0) throw new Error("no relay reports an open session");
    await outage(`relay crash (hbbr SIGKILL on ${NAMES[c]}, the session's relay): recovers within 2 s via the other relay`, () => ssh(RELAYS[c], "sudo systemctl kill -s SIGKILL tether-hbbr"));
    await sleep(5000);
    c = await carrier();
    if (c < 0) throw new Error("no relay reports an open session after the first recovery");
    await outage(
      `relay node down (${NAMES[c]}: hbbs + hbbr killed and kept down): session continues via the other relay`,
      () => ssh(RELAYS[c], "sudo systemctl kill -s SIGKILL tether-hbbs tether-hbbr; sudo systemctl stop tether-hbbs tether-hbbr"),
      () => ssh(RELAYS[c], "sudo systemctl start tether-hbbs tether-hbbr"),
    );
  }

  const ok = results.every((r) => r.ok);
  mkdirSync(join(ROOT, "tools/bench/results"), { recursive: true });
  writeFileSync(OUT, JSON.stringify({ when: new Date().toISOString(), vm: VM_IP, results }, null, 2));
  console.log(`${results.filter((r) => r.ok).length}/${results.length} passed`);
  process.exitCode = ok ? 0 : 1;
}

main()
  .catch((e) => {
    record("run", false, { error: e.message });
    process.exitCode = 1;
  })
  .finally(() => {
    for (const p of procs) {
      try {
        execFileSync("taskkill", ["/PID", String(p.pid), "/T", "/F"], { stdio: "ignore" });
      } catch {}
    }
    setTimeout(() => process.exit(), 500);
  });

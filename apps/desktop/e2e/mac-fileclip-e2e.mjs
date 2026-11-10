// File copy-paste between macOS and Windows through Tether, all four directions, end to end. Mac side: a
// runner job puts a file on the pasteboard (file URL, like Finder Cmd+C) or pastes like Finder Cmd+V (reads
// the pasteboard's file and copies it into a folder under ~ at once; the core swaps the placeholder copy for
// the real file, fsevent) and checks the content. Windows side (the VM, C:\vms\vm-fileclip.ps1 through the
// elevated broker, in the console session): CF_HDROP on the clipboard (Explorer Ctrl+C), or the shell
// "Paste" verb into a folder (Explorer Ctrl+V).
//  (2) the Mac's Tether views the VM's TetherQS (Mac = viewer);
//  (3) the technician app inside the VM (C:\vms\vm-tech-run.ps1) views the Mac (Mac = host). The Finder
//      paste comes PASTE_LATE s (default 45) after the copy: the placeholder must outlive 30 s.
// The viewer runs on the VM's normal desktop on purpose: on a hidden desktop (hidden-run.ps1) Explorer
// never asks the viewer's OLE clipboard for the file contents.
//
//   node e2e/mac-fileclip-e2e.mjs            (PART=2 or PART=3 for one half)
// Env: TECH (copied into the VM), MAC_ID, MAC_PW_FILE, VM_CDP (TetherQS devtools), VM_TECH_CDP, PASTE_LATE, OUT.
import { execFile } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { promisify } from "node:util";
import { chromium } from "playwright-core";

const run = promisify(execFile);
const TECH = process.env.TECH || "C:\\Program Files\\Tether\\tether.exe";
const MAC_ID = process.env.MAC_ID || "128974683";
const PW = readFileSync(process.env.MAC_PW_FILE || "C:/vms/mac.pw", "utf8").split(/\r?\n/)[0].trim();
const VM_CDP = process.env.VM_CDP || "http://172.30.252.30:9334";
const PART = process.env.PART || "23";
const VM_TECH_CDP = process.env.VM_TECH_CDP || "http://172.30.252.30:9235";
const OUT = resolve(process.env.OUT || `C:/vms/signtest/fileclip-${Date.now()}`);
const ROOT = resolve(import.meta.dirname, "../../..");
const MB = join(ROOT, "tools", "macbuild", "macbuild.py");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const until = async (t) => { while (Date.now() / 1000 < t) await sleep(200); };
const tok = (k) => `${k}-${Math.random().toString(36).slice(2, 10)}`;
const results = [];
const check = (name, ok, detail = "") => {
  results.push({ name, ok: !!ok });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? `  (${detail})` : ""}`);
};
mkdirSync(join(OUT, "job"), { recursive: true });

// Windows side: C:\vms\vm-fileclip.ps1 (set / paste) in the VM, through the elevated broker.
const vm = async (action, t) => (await run("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "C:/vms/broker/run-elevated.ps1", "vm-fileclip.ps1", "-Action", action, "-Token", t], { timeout: 600000 })).stdout.toString().trim();

// Mac side: the pasteboard through AppKit (JXA).
writeFileSync(join(OUT, "job", "fclip.js"), `ObjC.import('AppKit');
function run(argv) {
  const pb = $.NSPasteboard.generalPasteboard;
  if (argv[0] === 'set') { pb.clearContents; return pb.writeObjects($([$.NSURL.fileURLWithPath(argv[1])])) ? 'ok' : 'fail'; }
  const a = pb.readObjectsForClassesOptions($([$.NSURL]), $({}));
  const r = []; if (a) for (let i = 0; i < a.count; i++) r.push(a.objectAtIndex(i).path.js);
  if (argv[0] !== 'paste') return r.join('\\n');
  // Finder's Cmd+V: copy the pasteboard's files right away (the core then swaps the placeholder copy for the real file).
  const fm = $.NSFileManager.defaultManager, out = [];
  for (const src of r) {
    const err = $();
    out.push(fm.copyItemAtPathToPathError(src, argv[1] + '/' + src.split('/').pop(), err) ? 'copied ' + src : 'copy failed ' + src + ': ' + (err.localizedDescription ? err.localizedDescription.js : '?'));
  }
  return out.join('; ');
}
`);
writeFileSync(join(OUT, "job", "fclip.sh"), `#!/bin/bash
# START; T_SET (put OUT_TOK's file on the pasteboard); T_IN (expect IN_TOK's file); VM_ID/VM_PW: connect first.
D=$HOME/tether-fclip-e2e; mkdir -p "$D/src"
at() { while [ "$(date +%s)" -lt "$1" ]; do sleep 0.2; done; }
at "$START"
if [ -n "$VM_ID" ]; then /Applications/Tether.app/Contents/MacOS/tether --connect "$VM_ID" "$VM_PW" >/dev/null 2>&1 & echo "connect $(date +%T)"; fi
at "$T_SET"
f="$D/src/$OUT_TOK.txt"; printf 'content-%s' "$OUT_TOK" >"$f"
echo "mac set $(osascript -l JavaScript fclip.js set "$f") $(date +%T)"
at "$T_IN"
u=""
for i in $(seq 1 60); do u=$(osascript -l JavaScript fclip.js get 2>/dev/null); case "$u" in ""|*"$OUT_TOK"*) sleep 0.5 ;; *) break ;; esac; done
echo "mac pasteboard at $(date +%T): $u"; for x in /tmp/.rustdesk_*; do [ -e "$x" ] && echo "placeholder $x born $(stat -f %SB -t %T "$x")"; done
dst="$D/dst-$IN_TOK"; mkdir -p "$dst"
for i in 1 2 3 4 5; do r=$(osascript -l JavaScript fclip.js paste "$dst" 2>&1); echo "paste: $r"; case "$r" in *copied*) break ;; esac; sleep 1; done
for i in $(seq 1 60); do [ "$(cat "$dst"/* 2>/dev/null)" = "content-$IN_TOK" ] && { echo "mac got file $(ls "$dst") at $(date +%T)"; break; }; sleep 0.5; done
echo "mac dst: $(ls "$dst" | head -3 | tr '\\n' ' ') content: $(head -c 60 "$dst"/* 2>/dev/null)"
for L in ~/Library/Logs/Tether/tether_rCURRENT.log ~/Library/Logs/Tether/*/tether_rCURRENT.log; do
  echo "log $L:"; grep -iE "clip|paste|rustdesk_|fsevent|file_contents|format" "$L" 2>/dev/null | tail -12 | cut -c1-260
done
rm -rf "$D"
`);
const submit = async (envs) => {
  for (let t = 0; t < 3; t++) {
    const out = await run("python", [MB, "submit", "--user", "tether", "--work", join(OUT, "job"), "--timeout", "300", ...envs.flatMap((e) => ["--env", e]), "--", "bash", "fclip.sh"], { timeout: 600000 }).then((r) => r.stdout.toString(), (e) => String(e));
    const id = (out.trim().match(/\d{8}T\d{6}Z-[0-9a-f]+$/) || [""])[0];
    if (id) return id;
  }
  return "";
};
const jobLog = async (id) => {
  for (let i = 0; i < 60; i++) {
    const s = await run("python", [MB, "status", "--user", "tether", id]).then((r) => r.stdout.toString(), () => "");
    if (/"state": "(ok|failed|error|timeout)"/.test(s)) break;
    await sleep(5000);
  }
  return run("python", [MB, "log", "--user", "tether", id]).then((r) => r.stdout.toString(), (e) => String(e));
};

if (PART.includes("2")) {
  let vb, qs;
  try {
    vb = await chromium.connectOverCDP(VM_CDP, { timeout: 8000 });
    const vp = vb.contexts().flatMap((c) => c.pages())[0];
    qs = (cmd, args = {}) => vp.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a), [cmd, args]);
    const vmId = (await vp.locator('[data-testid="qs-id"]').innerText()).replace(/\s/g, "");
    const vmPw = await vp.locator('[data-testid="qs-password"]').getAttribute("data-password");
    const m2v = tok("mac2vm"), v2m = tok("vm2mac");
    const start = Math.floor(Date.now() / 1000) + 100;
    const id = await submit([`START=${start}`, `T_SET=${start + 25}`, `T_IN=${start + 75}`, `OUT_TOK=${m2v}`, `IN_TOK=${v2m}`, `VM_ID=${vmId}`, `VM_PW=${vmPw}`]);
    check("Mac job (Mac viewer) submitted", id, `${id}, VM ${vmId}`);
    if (!id) throw new Error("runner submit failed 3 times");
    let st;
    for (let i = 0; i < 120; i++) { st = await qs("state"); if (st.clients?.some((c) => c.authorized)) break; await sleep(500); }
    const c = st.clients?.find((x) => x.authorized);
    check("Mac viewer connected to the VM", c, c ? `peer ${c.peer_id} clipboard=${c.clipboard} file=${c.file_transfer ?? c.file}` : "");
    await until(start + 30);
    const got = await vm("paste", m2v);
    check("Mac (viewer) -> VM (host): file pasted in Explorer", got.includes(`pasted ${m2v}.txt`) && got.includes(`content-${m2v}`), got.replace(/\r?\n/g, " | "));
    await until(start + 68);
    console.log("vm", await vm("set", v2m));
    const log = await jobLog(id);
    console.log("mac:", log.trim().replace(/\n/g, " | "));
    check("VM (host) -> Mac (viewer): file pasted in Finder", log.includes(`mac got file ${v2m}.txt`));
  } catch (e) {
    check("part 2 without error", false, String(e?.stack ?? e).slice(0, 400));
  } finally {
    if (qs) for (const x of (await qs("state").catch(() => ({}))).clients || []) await qs("cm_close", { id: x.id }).catch(() => {});
    await vb?.close().catch(() => {});
  }
}

// (3) the technician app inside the VM (its normal desktop, via C:\vms\vm-tech-run.ps1) views the Mac.
if (PART.includes("3")) {
  const vmTech = (...a) => run("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "C:/vms/broker/run-elevated.ps1", "vm-tech-run.ps1", ...a], { timeout: 600000 }).then((r) => r.stdout.toString().trim(), (e) => String(e));
  let tb;
  try {
    console.log("vm tech:", (await vmTech("-Exe", TECH)).replace(/\r?\n/g, " | "));
    console.log("vm tech:", (await vmTech("-Connect", MAC_ID, "-Password", PW)).replaceAll(PW, "***").replace(/\r?\n/g, " | "));
    let page;
    for (let i = 0; i < 120 && !page; i++) {
      try { tb = await chromium.connectOverCDP(VM_TECH_CDP, { timeout: 5000 }); page = tb.contexts().flatMap((c) => c.pages()).find((p) => p.url().includes("/session/")); if (!page) { await tb.close(); tb = null; } } catch {}
      if (!page) await sleep(1000);
    }
    if (!page) throw new Error("no session page in the VM's technician app");
    for (let i = 0; i < 150; i++) { if (await page.evaluate(() => { const c = document.querySelector("canvas"); return !!c && c.width > 300; })) break; await sleep(200); }
    const m2v = tok("mac2vmv"), v2m = tok("vmv2mac");
    const start = Math.floor(Date.now() / 1000) + 100;
    const id = await submit([`START=${start}`, `T_SET=${start}`, `T_IN=${start + 60 + (+(process.env.PASTE_LATE ?? 45))}`, `OUT_TOK=${m2v}`, `IN_TOK=${v2m}`]);
    check("Mac job (Mac host, VM viewer) submitted", id, id);
    if (!id) throw new Error("runner submit failed 3 times");
    await until(start + 4);
    const got = await vm("paste", m2v);
    check("Mac (host) -> VM (viewer): file pasted in Explorer", got.includes(`pasted ${m2v}.txt`) && got.includes(`content-${m2v}`), got.replace(/\r?\n/g, " | "));
    await until(start + 45);
    console.log("vm", await vm("set", v2m));
    const log = await jobLog(id);
    console.log("mac:", log.trim().replace(/\n/g, " | "));
    check("VM (viewer) -> Mac (host): file pasted in Finder", log.includes(`mac got file ${v2m}.txt`));
  } catch (e) {
    check("part 3 without error", false, String(e?.stack ?? e).slice(0, 400));
  } finally {
    await tb?.close().catch(() => {});
    await vmTech("-Stop");
  }
}

console.log(`\nfiles: ${OUT}`);
const failed = results.filter((x) => !x.ok);
console.log(`${results.length - failed.length}/${results.length} passed`);
process.exit(failed.length ? 1 : 0);

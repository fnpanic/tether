// Layout audit (docs/ui-audit.md): node tools/ui-audit.mjs <desktop|quick|manage> <label> [scenario-regex]
// Every window state x window size x light/dark in Edge (mock mode; TetherQS with a Tauri IPC stand-in).
// Screenshots and findings.json -> $UIAUDIT_OUT (default C:/vms/uiaudit)/<app>/<label>/. Needs `npm ci` per app.
import { createRequire } from "node:module";
import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
const CLONE = process.env.CLONE ?? new URL("..", import.meta.url).pathname.replace(/^\/([A-Z]:)/, "$1");
const require = createRequire(`${CLONE}/apps/desktop/package.json`);
const { chromium } = require("playwright-core");

const [app, phase, filter] = process.argv.slice(2);
const OUT = `${process.env.UIAUDIT_OUT ?? "C:/vms/uiaudit"}/${app}/${phase}`;
mkdirSync(OUT, { recursive: true });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ---- window size sets: [name, w, h, dpr]
const common = [["1280x720", 1280, 720, 1], ["1366@125", 1093, 614, 1.25], ["1366@150", 911, 512, 1.5]];
const SIZES = {
  main: [["default", 1000, 660, 1], ["min", 820, 560, 1], ...common],
  adv: [["default", 1280, 800, 1], ["min", 1000, 600, 1], ...common],
  session: [["default", 1280, 800, 1], ["min", 640, 400, 1], ...common],
  cm: [["fixed", 380, 560, 1], ["small-screen", 380, 512, 1.5]],
  quick: [["fixed", 420, 560, 1], ["small-screen", 420, 512, 1.5]],
  manage: [["default", 1180, 800, 1], ["min", 900, 600, 1], ...common],
};

// ---- helpers for actions
const btn = (p, name) => p.getByRole("button", { name, exact: typeof name === "string" }).first().click({ timeout: 4000 });
const txt = (p, re) => p.getByText(re).first().click({ timeout: 4000 });

// ---- TetherQS stand-in for Tauri IPC
function fakeTauri(state) {
  let n = 0;
  window.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: "main" }, currentWebview: { windowLabel: "main", label: "main" } },
    transformCallback: (cb) => { const id = ++n; (window.__cbs ??= {})[id] = cb; return id; },
    unregisterCallback: () => {},
    convertFileSrc: (s) => s,
    invoke: async (cmd, args) => {
      if (cmd === "state") return state;
      if (cmd === "plugin:event|listen") { (window.__listeners ??= {})[args.event] = args.handler; return ++n; }
      if (cmd === "diagnostics") return { version: "0.5.26", id: state.id, servers: ["https://relay-eu-west-1.support.example-company-name.com", "https://relay-us-east-2.support.example-company-name.com"], proxy: "http://proxy.corp.example-company-name.internal:3128", elevated: false, policy_serial: 12, policy_group: "Reception desks Hamburg" };
      if (cmd === "diagnostics_probe") return args.server.includes("us") ? { reachable: false, ms: null, error: "connection timed out after 5 s (tcp connect to 203.0.113.9:21116)" } : { reachable: true, ms: 23, error: null };
      if (cmd === "update_check") return "TetherQS 0.5.26 is up to date";
      return cmd.startsWith("plugin:") ? (cmd.endsWith("listen") ? ++n : false) : null;
    },
  };
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
}
const PERMS = [["keyboard", "Control mouse and keyboard"], ["clipboard", "Use the clipboard"], ["file", "Transfer files"], ["audio", "Hear this computer's sound"], ["restart", "Restart this computer"], ["block_input", "Block my mouse and keyboard"]].map(([name, label]) => ({ name, label, allowed: true, locked: name === "restart" }));
const qclient = (id, o = {}) => ({ id, peer_id: String(100000000 + id), name: "Thomas", authorized: true, is_file_transfer: false, keyboard: true, clipboard: true, file: true, audio: true, restart: true, block_input: false, ...o });
const qstate = (o = {}, b = {}, u = {}) => ({
  id: "731204986", password: "k7x9q2", status: 1, version: "0.5.26", elevated: false, blocked: false, mac: null,
  branding: { title: "Tether Support", accent: "#0071e3", welcome: "Tell your technician the ID and password below.", elevate: true, approve_mode: "", consent_text: "Your technician wants to record this session.", diagnostics: true, perms: PERMS, custom_app: null, logo: "", ...b },
  update: { available: null, notes: "", required: false, forced_at: null, installing: false, error: null, last_check: "Checked today 09:12: up to date", progress: null, serial: 1, stale: false, phase: "", waiting_idle: false, ...u },
  clients: [], ...o,
});
const LONG = "Praxis Dr. med. Hans-Joachim Müller-Lüdenscheidt und Kollegen Gemeinschaftspraxis";

// ---- scenarios: [name, sizeSet, path, opts]
const APPS = {
  desktop: {
    dir: "desktop", port: 5231,
    scenarios: [
      ["home", "main", "/#/"],
      ["home-many", "main", "/?hosts=200#/"],
      ["home-more-menu", "main", "/#/", { act: (p) => btn(p, "More") }],
      ["home-longid", "main", "/#/", { act: (p) => p.getByRole("textbox", { name: "Remote device ID" }).fill("1234567890123456789012345678901234567890") }],
      ["home-search-none", "main", "/?hosts=200#/", { act: (p) => p.getByRole("textbox", { name: "Search recent devices" }).fill("zzzzzz") }],
      ...["Security", "Unattended access", "Recording", "Appearance", "Network", "Diagnostics", "About"].map((s) => [`settings-${s.split(" ")[0].toLowerCase()}`, "main", "/#/", { act: async (p) => { await btn(p, "Settings"); await p.getByRole("dialog", { name: "Settings" }).getByRole("button", { name: s, exact: true }).click(); } }]),
      ...[["settings-diag-sent", "K7Q2M9XD"], ["settings-diag-error", "ZZZZZZZZ"]].map(([n, code]) => [n, "main", "/#/", { act: async (p) => { await btn(p, "Settings"); await p.getByRole("dialog", { name: "Settings" }).getByRole("button", { name: "Diagnostics", exact: true }).click(); await p.getByRole("textbox", { name: "Support ticket code" }).fill(code); await btn(p, "Send to support"); await sleep(400); await p.getByRole("button", { name: "Send to support" }).scrollIntoViewIfNeeded(); } }]),
      ["recordings", "main", "/#/", { act: (p) => btn(p, "Recordings") }],
      ["hosts", "adv", "/?hosts=5000#/hosts"],
      ["hosts-empty", "adv", "/?hosts=0#/hosts"],
      ["hosts-none-found", "adv", "/?hosts=5000#/hosts", { act: (p) => p.getByRole("textbox", { name: "Search hosts" }).fill("zzzzzzzz") }],
      ["hosts-detail", "adv", "/?hosts=5000#/hosts", { act: async (p) => { await p.getByRole("grid", { name: "Hosts" }).getByRole("row").nth(1).click(); } }],
      ["hosts-selection", "adv", "/?hosts=5000#/hosts", { act: async (p) => { await p.getByRole("checkbox", { name: "Select all shown" }).click(); } }],
      ["hosts-palette", "adv", "/?hosts=5000#/hosts", { act: (p) => btn(p, "Open command palette") }],
      ["hosts-tag-rules", "adv", "/?hosts=5000#/hosts", { act: (p) => btn(p, "Tag rules") }],
      ["hosts-tag-colours", "adv", "/?hosts=5000#/hosts", { act: (p) => btn(p, "Tag colours") }],
      ["hosts-team", "adv", "/?hosts=5000#/hosts", { act: (p) => btn(p, "Team") }],
      ["hosts-open-sessions", "adv", "/?hosts=5000#/hosts", { act: (p) => btn(p, "Open sessions") }],
      ["hosts-shortcuts", "adv", "/?hosts=5000#/hosts", { act: (p) => btn(p, "Keyboard shortcuts") }],
      ["hosts-settings", "adv", "/?hosts=5000#/hosts", { act: (p) => btn(p, "Settings") }],
      ["cm", "cm", "/#/cm"],
      ["cm-second", "cm", "/#/cm", { act: (p) => p.locator("button", { hasText: /@/ }).nth(1).click() }],
      ["session", "session", "/#/session/mock?peer=184392571"],
      ["session-keyboard", "session", "/#/session/mock?peer=184392571", { act: (p) => btn(p, "Keyboard") }],
      ["session-actions", "session", "/#/session/mock?peer=184392571", { act: (p) => btn(p, "Remote actions") }],
      ["session-lock-confirm", "session", "/#/session/mock?peer=184392571", { act: async (p) => { await btn(p, "Remote actions"); await p.getByText("Lock screen…").click(); } }],
      ["session-quality", "session", "/#/session/mock?peer=184392571", { act: (p) => p.locator('button[title="Image quality"]').click() }],
      ["session-stats", "session", "/#/session/mock?peer=184392571", { act: (p) => btn(p, "Statistics") }],
      ["session-chat", "session", "/#/session/mock?peer=184392571", { act: (p) => btn(p, /^Chat/) }],
      ["filetransfer", "session", "/#/session/mock?peer=184392571&conn=file-transfer"],
    ],
  },
  quick: {
    dir: "quick", port: 5232,
    scenarios: [
      ["idle", "quick", "/", { state: qstate() }],
      ["idle-click-only", "quick", "/", { state: qstate({}, { approve_mode: "click" }) }],
      ["idle-long-branding", "quick", "/", { state: qstate({ status: 0 }, { title: LONG, welcome: LONG + ". " + LONG, custom_app: LONG, logo: "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='200' height='40'%3E%3Crect width='200' height='40' fill='%23e30'/%3E%3C/svg%3E" }) }],
      ["idle-update-mac", "quick", "/", { state: qstate({ mac: { screen: false, accessibility: false } }, {}, { available: "0.5.27", forced_at: Math.floor(Date.now() / 1000) + 600, error: "Download failed: the server answered 503 Service Unavailable" }) }],
      ["idle-blocked", "quick", "/", { state: qstate({ blocked: true, status: 2 }, {}, { available: "0.5.27", installing: true, progress: [3, 10] }) }],
      ["request", "quick", "/", { state: qstate({ clients: [qclient(1, { authorized: false, name: LONG })] }) }],
      ["request-update-mac", "quick", "/", { state: qstate({ mac: { screen: true, accessibility: false }, clients: [qclient(1, { authorized: false })] }, {}, { available: "0.5.27" }) }],
      ["session", "quick", "/", { state: qstate({ clients: [qclient(1, { name: LONG })] }) }],
      ["session-chat", "quick", "/", { state: qstate({ clients: [qclient(1)] }), emit: [{ name: "chat", payload: { id: 1, text: "Hi, I am connecting now. " + "z".repeat(200) } }], act: async (p) => { await p.getByTestId("qs-tab-chat").click(); for (const m of ["Hello", "x".repeat(300), "https://example.com/a/very/long/link/without/spaces/" + "y".repeat(120)]) { await p.getByTestId("qs-chat-input").fill(m); await p.getByTestId("qs-chat-input").press("Enter"); } } }],
      ["session-two", "quick", "/", { state: qstate({ clients: [qclient(1), qclient(2, { name: "Second technician" })] }, {}, { available: "0.5.27" }) }],
      ["session-two-chat", "quick", "/", { state: qstate({ clients: [qclient(1), qclient(2)] }), act: (p) => p.getByTestId("qs-tab-chat").nth(1).click() }],
      ["help", "quick", "/", { state: qstate({}, { custom_app: LONG }, { stale: true }), act: async (p) => { await btn(p, "Help"); await sleep(300); } }],
      ["consent", "quick", "/", { state: qstate({ clients: [qclient(1)] }), emit: [{ name: "record_consent_request", payload: { conn_id: 1, peer_name: LONG, text: LONG + ". " + LONG + ". " + LONG } }] }],
    ],
  },
  manage: {
    dir: "manage", port: 5233,
    scenarios: [
      ["gate-unlock", "manage", "/", { noUnlock: true }],
      ["gate-error", "manage", "/", { noUnlock: true, act: async (p) => { await p.locator("input[type=password]").first().fill("wrong"); await p.locator("input[type=password]").first().press("Enter"); await p.getByRole("alert").waitFor(); } }],
      ...["Status and relays", "Policy", "Devices", "Sessions", "Team", "Releases", "Download page", "Custom apps", "Diagnostics", "Keys"].map((n) => [`page-${n.split(" ")[0].toLowerCase()}`, "manage", "/", { page: n }]),
      ["page-devices-many", "manage", "/?many", { page: "Devices" }],
      ["page-sessions-many", "manage", "/?many", { page: "Sessions" }],
      ["policy-new-group", "manage", "/", { page: "Policy", act: (p) => btn(p, "New group") }],
      ["team-add", "manage", "/", { page: "Team", act: (p) => btn(p, "Add technician") }],
      ["custom-new", "manage", "/", { page: "Custom apps", act: (p) => btn(p, "New custom app") }],
      ["diag-created", "manage", "/", { page: "Diagnostics", act: async (p) => { const t = p.locator("main input, main textarea").first(); await t.fill(LONG); await btn(p, "Create ticket"); } }],
      ["diag-open", "manage", "/", { page: "Diagnostics", act: (p) => p.getByRole("button", { name: /^Open the upload/ }).first().click({ timeout: 4000 }) }],
      ["publish-sheet", "manage", "/", { act: (p) => p.getByRole("button", { name: /^Publish/ }).first().click() }],
      ["releases-publish", "manage", "/", { page: "Releases", act: (p) => p.getByRole("button", { name: /^Publish \d+ new build/ }).click({ timeout: 4000 }) }],
      ["releases-publish-silent", "manage", "/?silent", { page: "Releases", act: async (p) => { await p.getByRole("button", { name: /^Publish \d+ new build/ }).click({ timeout: 4000 }); await p.getByRole("dialog").getByRole("checkbox").check({ timeout: 4000 }); } }],
      ["releases-result-mixed", "manage", "/?mixed", { page: "Releases", act: async (p) => { await p.getByRole("button", { name: /^Publish \d+ new build/ }).click({ timeout: 4000 }); const d = p.getByRole("dialog"); await d.getByRole("button", { name: /^Sign and publish ·/ }).click({ timeout: 4000 }); await d.getByRole("button", { name: "Done" }).waitFor({ timeout: 8000 }); } }],
      // Publishing in progress (mock progress events): the progress is the sheet's content, the review form is gone, nothing cut at the edge of its scroll area, Cancel in view.
      ["releases-publishing", "manage", "/?uploading", { page: "Releases", act: async (p) => { await p.getByRole("button", { name: /^Publish \d+ new build/ }).click({ timeout: 4000 }); const d = p.getByRole("dialog"); await d.getByRole("button", { name: /^Sign and publish ·/ }).click({ timeout: 4000 }); await d.getByRole("progressbar").first().waitFor({ timeout: 4000 }); }, expect: publishing }],
      ["cleanup-silent", "manage", "/?silent", { page: "Releases", act: async (p) => { await p.getByRole("button", { name: /Clean up old releases/ }).click({ timeout: 4000 }); await p.getByText("Nothing can be deleted while a relay does not answer.").waitFor({ timeout: 4000 }); } }],
      ["download-review", "manage", "/", { page: "Download page", act: (p) => p.getByRole("button", { name: /review|publish/i }).first().click({ timeout: 4000 }) }],
    ],
  },
};

/** In-page check for a publish sheet while publishing: what the generic audit cannot tell (a
 *  heading cut at the bottom of a scroll area is normal scrolling there). */
function publishing() {
  const d = document.querySelector("[role=dialog]");
  if (!d) return [["publishing", "no dialog"]];
  const out = [], shown = (el) => el.getClientRects().length > 0;
  if ([...d.querySelectorAll("textarea,input,[role=radio],[role=switch]")].some(shown)) out.push(["publishing-form", "the review form is still shown while publishing (it can no longer change)"]);
  if (![...d.querySelectorAll("[role=progressbar]")].some(shown)) out.push(["publishing-progress", "no upload progress shown"]);
  const cancel = [...d.querySelectorAll("button")].find((b) => b.innerText.trim() === "Cancel");
  if (!cancel || cancel.getBoundingClientRect().bottom > innerHeight) out.push(["publishing-cancel", "Cancel not in view"]);
  for (const el of d.querySelectorAll("h2,h3,p,li")) {
    if (!shown(el)) continue;
    for (let a = el.parentElement; a && a !== d; a = a.parentElement) {
      if (!/auto|scroll/.test(getComputedStyle(a).overflowY)) continue;
      const r = el.getBoundingClientRect(), s = a.getBoundingClientRect();
      if ((r.top < s.bottom - 1 && r.bottom > s.bottom + 1) || (r.top < s.top - 1 && r.bottom > s.top + 1)) out.push(["publishing-cut", `"${el.innerText.slice(0, 50)}" cut at the edge of the sheet's scroll area`]);
    }
  }
  return out;
}

// ---- in-page audit
function audit() {
  const vw = innerWidth, vh = innerHeight, out = [];
  const label = (el) => {
    const t = (el.getAttribute("aria-label") || el.getAttribute("title") || el.innerText || el.getAttribute("placeholder") || "").trim().replace(/\s+/g, " ").slice(0, 40);
    const c = typeof el.className === "string" ? el.className.split(" ").filter(Boolean).slice(0, 4).join(".") : "";
    return `${t} <${el.tagName.toLowerCase()}${c ? "." + c : ""}>`;
  };
  const shown = (el) => { for (let a = el; a; a = a.parentElement) { const s = getComputedStyle(a); if (s.display === "none" || s.visibility === "hidden" || +s.opacity < 0.05) return false; } return true; };
  const layer = (el) => { for (let a = el; a; a = a.parentElement) { const s = getComputedStyle(a); if (s.position === "fixed" || a.getAttribute("role") === "dialog" || a.getAttribute("role") === "menu") return a; } return document.body; };
  const IA = "button,a[href],input:not([type=hidden]),select,textarea,[role=switch],[role=tab],[role=menuitem],[role=checkbox],[role=radio]";
  const els = [...document.querySelectorAll("body *")].filter((el) => !(el.closest("svg") && el.tagName.toLowerCase() !== "svg"));
  const de = document.scrollingElement;
  if (de.scrollWidth > vw + 1) out.push(["page-hscroll", `document scrollWidth ${de.scrollWidth} > ${vw}`]);
  // Scrollers: a table or a control that only fits by scrolling sideways is cut in view; a scroll area squeezed to (almost) nothing hides its content.
  for (const el of els) {
    const s = getComputedStyle(el);
    const sx = /auto|scroll/.test(s.overflowX) && el.scrollWidth > el.clientWidth + 1;
    const sy = /auto|scroll/.test(s.overflowY) && el.scrollHeight > el.clientHeight + 1;
    if ((!sx && !sy) || !shown(el) || el.getBoundingClientRect().width < 2) continue;
    if (sx && el.clientWidth > 0 && el !== de && el.querySelector(`table,${IA}`)) out.push(["hscroll", `${label(el)} scrolls sideways ${el.clientWidth}/${el.scrollWidth}: content cut at its edge`]);
    if (sy && el.clientHeight < 60 && el.querySelector(`${IA},p,li,span`)) out.push(["scroll-tiny", `${label(el)} scroll area only ${el.clientHeight}px tall for ${el.scrollHeight}px of content`]);
  }
  for (const el of els) {
    const r = el.getBoundingClientRect();
    if (r.width < 2 || r.height < 2) continue;
    const ia = el.matches(IA);
    const hasText = [...el.childNodes].some((n) => n.nodeType === 3 && n.textContent.trim());
    if (!ia && !hasText && !el.matches("svg,img")) continue;
    if (!shown(el)) continue;
    const cs = getComputedStyle(el);
    // clipped by a non-scrolling ancestor, or by the viewport
    let handled = false;
    for (let a = el.parentElement; a && a !== document.documentElement; a = a.parentElement) {
      const s = getComputedStyle(a);
      if (s.overflowX === "visible" && s.overflowY === "visible") { if (s.position === "fixed") break; continue; }
      const b = a.getBoundingClientRect();
      const outX = r.left < b.left - 1 || r.right > b.right + 1, outY = r.top < b.top - 1 || r.bottom > b.bottom + 1;
      if (!outX && !outY) { if (s.position === "fixed") break; continue; }
      if (s.textOverflow === "ellipsis") { handled = true; break; }
      const scrollY = /auto|scroll/.test(s.overflowY) && a.scrollHeight > a.clientHeight + 1;
      const scrollX = /auto|scroll/.test(s.overflowX) && a.scrollWidth > a.clientWidth + 1;
      if ((outY && !scrollY) || (outX && !scrollX)) {
        // fully outside = hidden on purpose (off-canvas) unless it is a scroller's content; only partial clipping counts
        const inter = Math.max(0, Math.min(r.right, b.right) - Math.max(r.left, b.left)) * Math.max(0, Math.min(r.bottom, b.bottom) - Math.max(r.top, b.top));
        if (inter > 0) out.push(["clipped", `${label(el)} clipped by ${label(a)} (el ${Math.round(r.left)},${Math.round(r.top)},${Math.round(r.right)},${Math.round(r.bottom)} box ${Math.round(b.left)},${Math.round(b.top)},${Math.round(b.right)},${Math.round(b.bottom)})`]);
      }
      handled = true;
      break;
    }
    if (!handled && (r.right > vw + 1 || r.bottom > vh + 1 || r.left < -1 || r.top < -1)) {
      const docScroll = de.scrollHeight > vh + 1 && getComputedStyle(document.body).overflowY !== "hidden";
      if (!(docScroll && r.bottom > vh && r.left >= 0 && r.right <= vw)) out.push(["offscreen", `${label(el)} at ${Math.round(r.left)},${Math.round(r.top)},${Math.round(r.right)},${Math.round(r.bottom)} viewport ${vw}x${vh}`]);
    }
    // text that does not fit its own box
    if (hasText && el.clientWidth > 0 && el.scrollWidth > el.clientWidth + 1 && cs.display !== "inline") {
      // Cut with "…" is fine when a tooltip shows the whole text; without one the text is lost.
      if (cs.textOverflow === "ellipsis") out.push([el.title || el.closest("[title]") ? "ellipsis" : "ellipsis-notitle", `${label(el)} ${el.scrollWidth}>${el.clientWidth}`]);
      else if (cs.overflowX !== "visible") out.push(["text-cut", `${label(el)} ${el.scrollWidth}>${el.clientWidth}`]);
      else out.push(["text-spill", `${label(el)} ${el.scrollWidth}>${el.clientWidth}`]);
    }
    // placeholder wider than its empty field
    if (el.matches("input[placeholder]:placeholder-shown")) {
      const ps = getComputedStyle(el, "::placeholder");
      const ctx = (audit.c ??= document.createElement("canvas").getContext("2d"));
      ctx.font = `${ps.fontStyle} ${ps.fontWeight} ${ps.fontSize} ${ps.fontFamily}`;
      const room = el.clientWidth - parseFloat(cs.paddingLeft) - parseFloat(cs.paddingRight);
      if (ctx.measureText(el.placeholder).width > room + 1) out.push(["placeholder-cut", `${label(el)} placeholder ${Math.round(ctx.measureText(el.placeholder).width)}>${Math.round(room)}`]);
    }
    if (hasText && el.clientHeight > 0 && el.scrollHeight > el.clientHeight + 2 && cs.overflowY !== "visible" && !/auto|scroll/.test(cs.overflowY)) out.push(["text-cut-v", `${label(el)} ${el.scrollHeight}>${el.clientHeight}`]);
    // a control whose label wraps past its own (fixed) height: the extra line spills out of the button
    if (ia && hasText && el.clientHeight > 0 && el.scrollHeight > el.clientHeight + 2 && cs.overflowY === "visible") out.push(["label-spill", `${label(el)} label ${el.scrollHeight}px tall in a ${el.clientHeight}px control`]);
    // interactive element covered by something else in the same layer
    if (ia && r.left >= 0 && r.top >= 0 && r.right <= vw && r.bottom <= vh) {
      const x = r.left + r.width / 2, y = r.top + r.height / 2;
      let inView = true;
      for (let a = el.parentElement; a; a = a.parentElement) { const s = getComputedStyle(a); if (s.overflowY !== "visible" || s.overflowX !== "visible") { const b = a.getBoundingClientRect(); if (x < b.left || x > b.right || y < b.top || y > b.bottom) { inView = false; break; } } }
      const hit = inView && document.elementFromPoint(x, y);
      if (hit && hit !== el && !el.contains(hit) && !hit.contains(el) && !(el.labels && [...el.labels].some((l) => l.contains(hit))) && layer(hit) === layer(el) && cs.pointerEvents !== "none") out.push(["covered", `${label(el)} covered by ${label(hit)}`]);
    }
  }
  return out;
}

// ---- run
const cfg = APPS[app];
const cwd = `${CLONE}/apps/${cfg.dir}`;
if (process.env.UIAUDIT_PORT) cfg.port = +process.env.UIAUDIT_PORT; // a second run next to another one
const vite = spawn(process.execPath, [`${cwd}/node_modules/vite/bin/vite.js`, "--port", String(cfg.port), "--strictPort"], { cwd, stdio: ["ignore", "pipe", "pipe"] });
await new Promise((ok, fail) => { vite.stdout.on("data", (d) => String(d).includes("Local") && ok()); setTimeout(() => fail(new Error("vite timeout")), 60000); });
const browser = await chromium.launch({ channel: "msedge" });
const findings = [];
try {
  for (const [name, set, path, o = {}] of cfg.scenarios) {
    if (filter && !new RegExp(filter).test(name)) continue;
    const combos = SIZES[set].flatMap((s) => ["light", "dark"].map((t) => [...s, t]));
    await Promise.all(combos.map(async ([sz, w, h, dpr, theme]) => {
        const ctx = await browser.newContext({ viewport: { width: w, height: h }, deviceScaleFactor: dpr, colorScheme: theme, reducedMotion: "reduce" });
        await ctx.addInitScript(() => { window.__mockCalls = []; window.open = () => null; try { localStorage.setItem("tether.mock.seen", "1"); } catch {} });
        if (o.state) await ctx.addInitScript(fakeTauri, o.state);
        const p = await ctx.newPage();
        p.setDefaultTimeout(6000);
        const errs = [];
        p.on("pageerror", (e) => errs.push(String(e).slice(0, 200)));
        let note = "";
        try {
          await p.goto(`http://localhost:${cfg.port}${path}`, { timeout: 120000 });
          await sleep(app === "desktop" && set === "adv" ? 2500 : 900);
          if (app === "manage" && !o.noUnlock) {
            await p.locator("input[type=password]").first().fill("x");
            await p.locator("input[type=password]").first().press("Enter");
            await sleep(500);
          }
          if (o.page) { await p.getByRole("navigation", { name: "Sections" }).getByRole("button", { name: new RegExp(`^${o.page}`) }).click(); await sleep(700); }
          if (o.emit) for (const ev of o.emit) { await p.evaluate((ev) => window.__cbs[window.__listeners.cm]({ event: "cm", id: 1, payload: ev }), ev); await sleep(200); }
          if (o.act) await o.act(p);
          await sleep(600);
        } catch (e) { note = `act failed: ${String(e.message).split("\n")[0]}`; }
        const file = `${name}__${sz}__${theme}.png`;
        await p.screenshot({ path: `${OUT}/${file}` });
        const issues = await p.evaluate(audit).catch((e) => [["audit-error", String(e)]]);
        if (o.expect) issues.push(...(await p.evaluate(o.expect).catch((e) => [["audit-error", String(e)]])));
        findings.push({ name, size: sz, theme, file, note, errs, issues });
        const bad = issues.filter((i) => i[0] !== "ellipsis");
        console.log(`${name} ${sz} ${theme}: ${bad.length} issues${note ? " | " + note : ""}${errs.length ? " | pageerror " + errs[0] : ""}`);
        await ctx.close();
    }));
  }
} finally {
  writeFileSync(`${OUT}/findings${filter ? "-" + filter.replace(/\W/g, "_") : ""}.json`, JSON.stringify(findings, null, 1));
  await browser.close();
  vite.kill();
}

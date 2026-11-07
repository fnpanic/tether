// Advanced mode (Hosts window) in mock mode with 5,000 hosts, in Chromium and WebKit: search,
// filters, saved searches, bulk tagging, persistence, keyboard flow, palette, export, and that the
// connect actions invoke the right IPC. Also measures search latency per keystroke.
//
//   node e2e/hosts-mock.e2e.mjs            both engines
//   node e2e/hosts-mock.e2e.mjs webkit     one engine
//
// Browsers: `npx playwright-core install chromium-headless-shell webkit` (PLAYWRIGHT_BROWSERS_PATH
// may point to a cache dir). Everything runs inside the headless browser.
import { spawn } from "node:child_process";
import { resolve } from "node:path";
import { chromium, webkit } from "playwright-core";

const ROOT = resolve(import.meta.dirname, "..");
const PORT = 14820 + Math.floor(Math.random() * 150);
const engines = process.argv[2] ? [process.argv[2]] : ["chromium", "webkit"];
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let failures = 0;
const check = (what, ok, detail = "") => {
  console.log(`${ok ? "PASS" : "FAIL"} ${what}${detail ? `  (${detail})` : ""}`);
  if (!ok) failures++;
};
async function until(fn, timeout = 10000, every = 50) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    if (await fn().catch(() => false)) return true;
    await sleep(every);
  }
  return false;
}

const vite = spawn(process.execPath, [resolve(ROOT, "node_modules/vite/bin/vite.js"), "--port", String(PORT), "--strictPort"], { cwd: ROOT, stdio: ["ignore", "pipe", "pipe"] });
const ready = new Promise((ok) => vite.stdout.on("data", (d) => String(d).includes("Local") && ok()));

async function run(name) {
  const browser = await { chromium, webkit }[name].launch();
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  await ctx.addInitScript(() => {
    window.__mockCalls = [];
    window.open = () => null; // the mock's open_session opens a window; not needed here
  });
  const p = await ctx.newPage();
  const errors = [];
  p.on("pageerror", (e) => errors.push(String(e)));
  const tag = (s) => `[${name}] ${s}`;
  const base = `http://localhost:${PORT}/`;
  const calls = (cmd) => p.evaluate((c) => window.__mockCalls.filter((x) => x.cmd === c).map((x) => x.args), cmd);


  // ---- Mode: Simple home by default, Advanced loaded lazily, switch both ways, policy off/forced.
  const hostsModule = [];
  p.on("request", (r) => /\/pages\/Hosts\.tsx/.test(r.url()) && hostsModule.push(r.url()));
  await p.goto(`${base}?hosts=5000#/`, { timeout: 120000 });
  await p.evaluate(() => localStorage.clear());
  await p.reload();
  const hostsBtn = p.getByRole("button", { name: "Hosts", exact: true });
  check(tag("simple home view by default"), await until(() => p.getByRole("textbox", { name: "Remote device ID" }).isVisible(), 30000));
  check(tag("advanced bundle not loaded in simple mode"), hostsModule.length === 0, hostsModule.join(" "));
  await hostsBtn.click();
  check(tag("Hosts button switches the home view to Advanced"), await until(() => p.getByRole("textbox", { name: "Search hosts" }).isVisible(), 30000));
  check(tag("advanced bundle loaded on demand"), hostsModule.length > 0);
  check(tag("window asked to fit the inventory"), (await calls("main_window_fit")).some((c) => c.advanced === true));
  await p.getByRole("button", { name: "Simple view" }).click();
  check(tag("Simple view switches back"), await until(() => p.getByRole("textbox", { name: "Remote device ID" }).isVisible()));
  await p.goto(`${base}?hosts=50&ui=off#/`);
  await p.evaluate(() => localStorage.setItem("tether.mode", "advanced"));
  await p.reload();
  await until(() => p.getByRole("textbox", { name: "Remote device ID" }).isVisible(), 30000);
  await sleep(300);
  check(tag("policy ui.advanced=off: simple only, no Hosts button"), (await p.getByRole("textbox", { name: "Remote device ID" }).isVisible()) && !(await hostsBtn.isVisible()));
  await p.goto(`${base}?hosts=50&ui=forced#/`);
  await p.evaluate(() => localStorage.setItem("tether.mode", "simple"));
  await p.reload();
  check(tag("policy ui.advanced=forced: Advanced without a way back"), (await until(() => p.getByRole("textbox", { name: "Search hosts" }).isVisible(), 30000)) && !(await p.getByRole("button", { name: "Simple view" }).isVisible()));

  await p.goto(`${base}?hosts=5000#/`);
  await p.evaluate(() => {
    localStorage.clear();
    localStorage.setItem("tether.mode", "advanced");
  });
  await p.reload();
  const search = p.getByRole("textbox", { name: "Search hosts" });
  const total = async () => Number((await p.getByTestId("result-count").textContent()).replace(/[^\d ]/g, "").trim().split(/\s+/).pop());
  const shown = async () => Number((await p.getByTestId("result-count").textContent()).replace(/[,.]/g, "").match(/\d+/)[0]);
  const rows = () => p.locator("[role=row][data-id]");
  const clearCalls = () => p.evaluate(() => (window.__mockCalls.length = 0));
  const setQuery = async (q) => {
    await search.fill(q);
    await sleep(60);
  };

  check(tag("list loads"), await until(async () => (await rows().count()) > 10, 60000));
  // Org devices arrive after the team check.
  await until(async () => (await total()) > 4000, 10000);
  const all = await total();
  check(tag("5,000-host fleet listed"), all > 4000, `${all} hosts`);
  check(tag("list is virtualised (few rows in the DOM)"), (await rows().count()) < 80, `${await rows().count()} rows`);

  // ---- Search latency: type a query one key at a time.
  await search.click();
  await p.evaluate(() => (window.__hostsPerf.samples.length = 0));
  const e2e = [];
  for (const ch of "ber srv") {
    const t0 = Date.now();
    await p.keyboard.type(ch);
    await p.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
    e2e.push(Date.now() - t0);
  }
  const samples = await p.evaluate(() => window.__hostsPerf.samples.slice());
  const worst = Math.max(...samples);
  check(tag("search < 16 ms per keystroke on 5,000 hosts"), samples.length >= 7 && worst < 16, `search ms ${samples.map((x) => x.toFixed(1)).join(" ")}`);
  const sorted = [...e2e].sort((a, b) => a - b);
  console.log(`  [${name}] keystroke to painted frame (incl. Playwright round trip): median ${sorted[3]} ms, max ${sorted[6]} ms`);
  const n1 = await shown();
  check(tag("fuzzy words narrow the list"), n1 > 0 && n1 < all, `${n1} of ${all}`);
  const firstName = await rows().first().locator("[role=gridcell]").nth(2).textContent();
  check(tag("best match first"), /BER.*SRV/i.test(firstName), firstName);

  // ---- Tag and field filters with AND / NOT.
  await setQuery("#site:berlin is:online");
  const berlin = await shown();
  const dots = await rows().evaluateAll((rs) => rs.map((r) => r.querySelector("[role=img]")?.getAttribute("aria-label")));
  check(tag("#tag AND is:online"), berlin > 0 && dots.every((d) => d === "Online"), `${berlin} hosts`);
  await setQuery("#site:berlin -is:online");
  const off = await shown();
  await setQuery("#site:berlin");
  check(tag("NOT splits a tag filter"), berlin + off === (await shown()), `${berlin} + ${off}`);
  await setQuery("#customer:acme | #customer:globex");
  const or = await shown();
  await setQuery("#customer:acme");
  const a = await shown();
  await setQuery("#customer:globex");
  check(tag("OR of two customers"), or === a + (await shown()), `${or}`);
  await setQuery("(#site:munich OR #site:hamburg) AND NOT #env:prod");
  check(tag("parentheses"), (await shown()) > 0);

  // ---- Sidebar: library, tag tree (click = filter, Alt+click = exclude), saved search.
  await setQuery("");
  await p.getByRole("navigation", { name: "Library" }).getByRole("button", { name: /^Online/ }).click();
  check(tag("library: Online"), (await search.inputValue()) === "is:online");
  await p.getByRole("button", { name: /^site\s/ }).click();
  check(tag("tag tree click adds #site"), (await search.inputValue()) === "is:online #site");
  await p.getByRole("button", { name: /^env\s/ }).click({ modifiers: ["Alt"] });
  check(tag("tag tree Alt+click excludes"), (await search.inputValue()) === "is:online #site -#env");
  await p.getByRole("button", { name: /^Acme servers/ }).click();
  check(tag("saved search applies its query"), (await search.inputValue()) === "#customer:acme #role:server" && (await shown()) > 0);

  // ---- Keyboard flow: search, arrows, Enter / Shift+Enter / Alt+Enter → open_session.
  await setQuery("");
  await search.click();
  await search.fill("BER-SRV");
  await sleep(60);
  await p.keyboard.press("ArrowDown");
  const cursorId = await p.locator("[role=row][data-cursor]").getAttribute("data-id");
  const secondId = await rows().nth(1).getAttribute("data-id");
  check(tag("ArrowDown moves the cursor"), cursorId === secondId, cursorId);
  await clearCalls();
  await p.keyboard.press("Enter");
  await p.keyboard.press("Shift+Enter");
  await p.keyboard.press("Alt+Enter");
  await until(async () => (await calls("open_session")).length === 3, 3000);
  const os = await calls("open_session");
  check(
    tag("Enter / Shift+Enter / Alt+Enter: connect, files, view only"),
    os.length === 3 && os.every((c) => c.peer === cursorId) && os[0].conn === "default" && !os[0].viewOnly && os[1].conn === "file-transfer" && os[2].conn === "default" && os[2].viewOnly === true,
    JSON.stringify(os),
  );
  await p.getByRole("button", { name: "View only" }).click();
  check(tag("detail panel View only button"), (await calls("open_session")).at(-1)?.viewOnly === true);

  // ---- Command palette.
  await clearCalls();
  await p.keyboard.press("Control+k");
  const pal = p.getByRole("combobox", { name: "Command" });
  check(tag("Ctrl+K opens the palette"), await pal.isVisible());
  const target = (await rows().first().locator("[role=gridcell]").nth(2).textContent()).trim();
  await pal.fill(target);
  await sleep(80);
  await p.keyboard.press("Enter");
  await until(async () => (await calls("open_session")).length === 1, 3000);
  check(tag("palette: name + Enter connects"), (await calls("open_session"))[0]?.peer === (await rows().first().getAttribute("data-id")));

  // ---- Bulk tagging: Ctrl+A on a filtered list, T, add a tag to all.
  await setQuery("#customer:globex #site:berlin");
  const nsel = await shown();
  await rows().first().click(); // focus leaves the search box
  await p.keyboard.press("Control+a");
  check(tag("Ctrl+A selects every shown host"), Number((await p.getByTestId("selection-count").textContent()).replace(/\D/g, "")) === nsel, `${nsel}`);
  await p.keyboard.press("t");
  const dlg = p.getByRole("dialog");
  check(tag("T opens bulk tags"), await dlg.getByText(`Tags for ${nsel} hosts`).isVisible());
  await dlg.getByRole("textbox").fill("campaign:q4");
  await p.keyboard.press("Enter");
  check(tag("bulk tag applied to all"), await dlg.getByText("all", { exact: true }).first().isVisible());
  await dlg.getByRole("button", { name: "Done" }).click();
  await p.keyboard.press("Escape");
  await setQuery("#campaign:q4");
  check(tag("search finds the bulk-tagged hosts"), (await shown()) === nsel, `${await shown()} vs ${nsel}`);
  await setQuery("#campaign");
  check(tag("namespace query includes children"), (await shown()) === nsel);

  // Bulk remove through the same dialog.
  await rows().first().click();
  await p.keyboard.press("Control+a");
  await p.keyboard.press("t");
  await p.getByRole("dialog").locator("div.flex.h-8", { hasText: "campaign:q4" }).getByRole("button", { name: "Remove" }).click();
  await p.getByRole("dialog").getByRole("button", { name: "Done" }).click();
  await setQuery("#campaign:q4");
  check(tag("bulk remove"), (await p.getByText("No host matches").isVisible()));

  // ---- Persistence (debounced save, survives a reload).
  await setQuery("");
  await rows().first().click();
  await p.keyboard.press("e");
  await p.keyboard.type("Reception PC");
  await p.keyboard.press("Enter");
  const renamedId = await p.locator("[role=row][data-cursor]").getAttribute("data-id");
  await until(async () => (await calls("hosts_store_save")).length > 0, 3000);
  check(tag("store saved after edits (debounced)"), (await calls("hosts_store_save")).length > 0);
  await p.reload();
  await search.waitFor();
  await until(async () => (await rows().count()) > 10, 30000);
  await setQuery("reception pc");
  check(tag("rename persisted across reload"), (await rows().first().getAttribute("data-id")) === renamedId);

  // ---- Export leaves notes out by default.
  await setQuery("");
  await p.keyboard.press("Control+k");
  await pal.fill("export hosts");
  await p.keyboard.press("Enter");
  await p.getByRole("button", { name: "Export…" }).click();
  const exp = await p.evaluate(() => window.__lastExport);
  check(tag("export: JSON without notes"), !!exp && exp.name.endsWith(".json") && exp.content.includes('"tether-hosts"') && !exp.content.includes("front desk before restarting"));

  // ---- Help and sessions dialogs open from the keyboard / title bar.
  await rows().first().click();
  await p.keyboard.press("?");
  check(tag("? shows shortcuts"), await p.getByRole("dialog").getByText("Search syntax").isVisible());
  await p.keyboard.press("Escape");
  await p.getByRole("button", { name: "Open sessions" }).click();
  check(tag("sessions dialog lists open sessions"), await until(() => p.getByRole("button", { name: "Send Ctrl+Alt+Del" }).isVisible()));
  await clearCalls();
  await p.getByRole("button", { name: "Send Ctrl+Alt+Del" }).click();
  check(tag("Ctrl+Alt+Del goes to that session"), await until(async () => (await calls("session_ctrl_alt_del"))[0]?.key === "k1"));
  await p.keyboard.press("Escape");

  check(tag("no page errors"), errors.length === 0, errors.join(" | "));
  await browser.close();
}

try {
  await ready;
  for (const e of engines) await run(e);
} catch (e) {
  console.error(e);
  failures++;
} finally {
  vite.kill();
}
console.log(failures ? `${failures} FAILED` : "all passed");
process.exit(failures ? 1 : 0);

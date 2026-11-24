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
    // The clipboard as the page wrote it (no permission prompt in either engine).
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: async (t) => void (window.__clip = t), readText: async () => window.__clip ?? "" } });
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

  // ---- Health: built-in view, column sort, a search term, the Health section, raw report, rules, export.
  const SHOTS = process.env.SHOTS; // a folder: screenshots of the health views
  const shot = async (n) => SHOTS && (await p.screenshot({ path: `${SHOTS}/${name}-${n}.png` }));
  const nav = p.getByRole("navigation", { name: "Library" });
  const attention = nav.getByRole("button", { name: /^Needs attention/ });
  const navCount = Number((await attention.textContent()).replace(/\D/g, ""));
  await attention.click();
  check(tag("Needs attention applies health:crit | health:stale | health:conflict"), (await search.inputValue()) === "health:crit | health:stale | health:conflict");
  check(tag("Needs attention count equals its result count"), navCount > 0 && navCount === (await shown()), `${navCount} vs ${await shown()}`);
  const dotsOf = () => rows().evaluateAll((rs) => rs.map((r) => r.querySelector("[role=img][aria-label^='Health:']")?.getAttribute("aria-label")));
  check(tag("every row of the view is critical, not reporting or in a link conflict"), (await dotsOf()).every((d) => d === "Health: Critical" || d === "Health: Not reporting" || d === "Health: Link conflict"));
  await setQuery("health:crit");
  check(tag("health:crit finds only critical hosts"), (await shown()) > 0 && (await dotsOf()).every((d) => d === "Health: Critical"));
  await setQuery("check:db-service");
  const db = await shown();
  check(tag("check:db-service finds hosts with that failing check"), db > 0 && db <= navCount, `${db}`);
  await setQuery("");
  await p.getByRole("columnheader", { name: "Health" }).click();
  check(tag("sorting by Health puts the link conflict, then a critical row first"), (await dotsOf())[0] === "Health: Link conflict" && (await dotsOf())[1] === "Health: Critical", (await dotsOf()).slice(0, 2).join());
  check(tag("Health column says it in words"), /Link conflict/.test(await rows().first().locator("[role=gridcell]").nth(5).textContent()) && /critical/.test(await rows().nth(1).locator("[role=gridcell]").nth(5).textContent()));
  // The conflicted host: nothing joined, the conflict explained.
  await rows().first().click();
  check(tag("a host with two linked devices shows the link conflict"), await until(() => p.getByTestId("health-conflict-host").isVisible()));
  await shot("conflict-host");
  await rows().nth(1).click();
  const sec = p.getByTestId("health-section");
  check(tag("detail shows the Health section"), await until(() => sec.locator("li[data-check]").first().isVisible()));
  check(tag("detail shows the customer reference"), /Customer 471[12]/.test(await sec.textContent()), (await sec.textContent()).slice(0, 120));
  // Every fault (all groups) and the setup items come before the first "checks passed" fold.
  const order = await sec.evaluate((s) => {
    const items = [...s.querySelectorAll("li[data-check]")];
    const passed = [...s.querySelectorAll("button")].filter((b) => /checks? passed/.test(b.textContent));
    const groups = [...s.querySelectorAll("li[data-check]")].map((li) => li.closest("div").firstElementChild?.textContent);
    return { ok: items.length > 1 && passed.length > 0 && items.every((li) => !!(li.compareDocumentPosition(passed[0]) & Node.DOCUMENT_POSITION_FOLLOWING)), n: items.length, groups: new Set(groups).size };
  });
  check(tag("all faults are listed before checks passed"), order.ok && order.groups > 1, JSON.stringify(order));
  await shot("detail");
  await sec.getByRole("button", { name: "History (90 days)" }).click();
  check(tag("history lists changes"), (await sec.getByRole("list", { name: "History" }).locator("li").count()) > 0);
  await sec.getByRole("button", { name: "Raw report" }).click();
  await sec.getByRole("button", { name: "Copy raw report" }).click();
  const clip = await p.evaluate(() => window.__clip ?? "");
  let raw = null;
  try {
    raw = JSON.parse(clip);
  } catch {}
  check(tag("raw report Copy writes the report JSON"), raw?.v === 1 && Array.isArray(raw?.body?.checks), clip.slice(0, 60));
  await shot("raw");
  await p.keyboard.press("Escape");
  // Agent-only rows: no connect actions, Enter only explains.
  await setQuery("id:m:");
  check(tag("agent-only rows are listed (two of them in a conflict)"), (await shown()) === 7);
  await rows().first().click();
  await clearCalls();
  check(tag("an m: row has no Connect button"), (await p.getByText("No remote access: Tether is not installed").first().isVisible()) && !(await p.getByRole("button", { name: /^Connect/ }).isVisible()));
  const det = p.getByRole("complementary", { name: "Host details" });
  check(tag("an m: row has no name, tags, notes, pin or Remove host"), !(await det.locator("#host-name").count()) && !(await det.locator("#host-notes").count()) && !(await det.getByRole("button", { name: /^(Pin|Unpin|Remove host)$/ }).count()));
  await p.keyboard.press("Delete");
  await sleep(100);
  check(tag("Delete on an m: row opens no Remove dialog"), !(await p.getByRole("dialog").count()));
  await p.keyboard.press("Enter");
  await sleep(100);
  check(tag("Enter on an m: row opens nothing"), (await calls("open_session")).length === 0);
  // Software list (SBOM): collapsed until opened, then the table, the filter, the device line and the export.
  await setQuery("LAB-ANALYZER-01");
  await rows().first().click();
  const sw = p.getByTestId("software-section");
  check(tag("Software is collapsed and says when the list is from"), (await until(() => sw.isVisible())) && /as of /.test(await sw.textContent()) && (await sw.getByRole("table").count()) === 0 && (await calls("team_status_sbom")).length === 0);
  await sw.getByRole("button", { name: /^Software/ }).click();
  check(tag("opening Software asks the relay and lists the programs"), (await until(() => sw.getByRole("table").isVisible())) && (await calls("team_status_sbom")).length === 1 && (await sw.getByRole("row").count()) === 201, `${await sw.getByRole("row").count()} rows`);
  check(tag("Software header counts the programs"), /226 programs, as of /.test(await sw.getByRole("button", { name: /^Software/ }).textContent()));
  check(tag("the product comes first, then by name"), /^Records Server/.test(await sw.getByRole("row").nth(1).textContent()) && /^Aster Notes/.test(await sw.getByRole("row").nth(2).textContent()));
  check(tag("Software shows the device line and the truncated line"), (await sw.getByText("Example Devices Analyzer Station 7010, serial 7XK2LAB0001, 16 GB").isVisible()) && (await sw.getByText("12 more programs are not in the list").isVisible()));
  check(tag("Software draws 200 rows and offers Show all"), await sw.getByRole("button", { name: "Show all 226" }).isVisible());
  await sw.getByRole("button", { name: "Show all 226" }).click();
  check(tag("Show all draws every program"), (await sw.getByRole("row").count()) === 227 && !(await sw.getByRole("button", { name: /^Show all/ }).isVisible()), `${await sw.getByRole("row").count()} rows`);
  const swFilter = sw.getByRole("textbox", { name: "Filter software by name or publisher" });
  await swFilter.fill("oAk");
  check(tag("the filter matches the name, case-insensitive"), (await sw.getByRole("row").count()) === 2 && (await sw.getByRole("cell", { name: "Oakham Image Editor" }).isVisible()));
  await swFilter.fill("ravenscar security");
  check(tag("the filter matches the publisher"), (await sw.getByRole("row").count()) === 2 && (await sw.getByRole("cell", { name: "Ravenscar Antivirus" }).isVisible()));
  await swFilter.fill("");
  await sw.getByRole("button", { name: "Export SBOM" }).click();
  await until(async () => /\.cdx\.json$/.test((await p.evaluate(() => window.__lastExport?.name)) ?? ""));
  const sx = await p.evaluate(() => window.__lastExport);
  check(tag("Export SBOM saves the document as <host>-<date>.cdx.json, unchanged"), /^LAB-ANALYZER-01-\d{4}-\d{2}-\d{2}\.cdx\.json$/.test(sx?.name ?? "") && JSON.parse(sx.content).bomFormat === "CycloneDX" && sx.content.includes('"Aster Notes"'), sx?.name);
  await shot("software");
  await setQuery("SCAN-STATION-02");
  await rows().first().click();
  await p.getByTestId("software-section").getByRole("button", { name: /^Software/ }).click();
  check(tag("a list the relay dropped meanwhile says so, no export"), (await until(() => p.getByText("No software list on the server.").isVisible())) && !(await p.getByRole("button", { name: "Export SBOM" }).isVisible()));
  await setQuery("NAS-BACKUP-01");
  await rows().first().click();
  await p.getByTestId("software-section").getByRole("button", { name: /^Software/ }).click();
  check(tag("a document that is not a software list says so and still offers the export"), (await until(() => p.getByText("The software list could not be read.").isVisible())) && (await p.getByRole("button", { name: "Export SBOM" }).isVisible()));
  // A report without body: an empty state, no error.
  await setQuery("NAS-BACKUP-01");
  await rows().first().click();
  check(tag("a report without checks shows an empty state"), await until(() => p.getByText("This report lists no checks.").isVisible()));
  // The other device of the conflict: its own row, marked, with Unlink and Remove.
  await setQuery("SHADOW-PC");
  await rows().first().click();
  check(tag("a conflicting agent row says which host it is in conflict for"), await p.getByText(/^In conflict with another agent for host /).isVisible());
  check(tag("a conflicting agent is its own row with the conflict line and Unlink"), (await rows().first().getAttribute("data-id")) === "m:agent-dup-1" && (await until(() => p.getByTestId("health-conflict").isVisible())) && (await until(() => p.getByTestId("health-section").getByRole("button", { name: "Unlink" }).isVisible())));
  await shot("conflict-agent");
  // A claim to a host whose health agent is linked already cannot be linked (it would hide that host's health).
  await setQuery("IMPOSTER-PC");
  await rows().first().click();
  check(tag("Link is refused for a host that already has a linked health agent"), (await until(() => p.getByText(/This host already has a linked health agent \(.+\)\. Unlink that one first/).isVisible())) && (await p.getByTestId("health-claim").getByRole("button", { name: "Link to this host" }).isDisabled()));
  await shot("link-blocked");
  // Verified links: an agent that only claims a Tether ID keeps its own row until a member confirms it.
  await setQuery("FRONT-DESK-AGENT");
  await rows().first().click();
  const claimLine = p.getByTestId("health-claim");
  // The enrolment time comes with the host's report (asked for once the cursor rests).
  const claimText = (await until(async () => /enrolled/.test(await claimLine.textContent()))) ? await claimLine.textContent() : await claimLine.textContent().catch(() => "");
  check(tag("a claimed agent stays an m: row with the claim line"), (await rows().first().getAttribute("data-id")) === "m:agent-only-4" && /Claims to be \d{9} \(.+\)\. Confirm only if this is that computer\./.test(claimText) && /enrolled/.test(claimText), claimText);
  await shot("claim");
  await claimLine.getByRole("button", { name: "Link to this host" }).click();
  const ldlg = p.getByRole("dialog");
  check(tag("Link asks for one explicit confirmation"), await until(() => ldlg.getByText("The agent's claim is not proof").isVisible()));
  if (SHOTS) await sleep(500); // past the dialog's fade-in
  await shot("link-dialog");
  await clearCalls();
  await ldlg.getByRole("button", { name: "Link", exact: true }).click();
  await until(async () => (await calls("team_status_link")).length > 0);
  const la = (await calls("team_status_link"))[0];
  check(tag("Link calls team_status_link with the claimed ID"), la?.uuid === "agent-only-4" && claimText.includes(`Claims to be ${la?.tetherId} `), JSON.stringify(la));
  await setQuery(`id:${la?.tetherId}`);
  await rows().first().click();
  check(tag("the linked host shows who linked it"), await until(() => p.getByText("Health linked by Anna (Helpdesk)").isVisible()));
  await setQuery("id:m:");
  check(tag("the linked agent no longer has its own row"), (await shown()) === 6);
  // An agent Tether installed and linked on that computer (relay proof): said so, no member named.
  await setQuery("id:327660915");
  await rows().first().click();
  check(tag("a proof-linked host says it was linked by Tether on that computer"), await until(() => p.getByText(/^Health linked by Tether on this computer, /).isVisible()));
  await setQuery(`id:${la?.tetherId}`);
  await rows().first().click();
  await p.getByTestId("health-section").getByRole("button", { name: "Unlink" }).click();
  await clearCalls();
  await p.getByRole("dialog").getByRole("button", { name: "Unlink" }).click();
  check(tag("Unlink calls team_status_unlink"), await until(async () => (await calls("team_status_unlink"))[0]?.uuid === "agent-only-4"));
  await setQuery("FRONT-DESK-AGENT");
  check(tag("an unlinked agent is its own row again, link dismissed, no claim"), (await until(async () => (await rows().first().getAttribute("data-id")) === "m:agent-only-4")) && (await rows().first().click(), await until(() => p.getByTestId("health-dismissed").isVisible())) && !(await claimLine.isVisible()));
  check(tag("a dismissed row says so in its banner"), await p.getByText("Link dismissed. This agent is not linked to a host.").isVisible());
  await shot("dismissed");
  await clearCalls();
  await p.getByRole("button", { name: "Allow a new claim" }).click();
  check(tag("Allow a new claim calls team_status_allow_claim"), await until(async () => (await calls("team_status_allow_claim"))[0]?.uuid === "agent-only-4"));
  await rows().first().click();
  await p.getByRole("button", { name: "Remove from health monitoring…" }).click();
  const rmdlg = p.getByRole("dialog");
  check(tag("Remove names the host and asks to uninstall the agent"), (await rmdlg.getByText("Remove FRONT-DESK-AGENT from health monitoring?").isVisible()) && (await rmdlg.getByText(/enrols again by itself.*Uninstall the agent there, or ask an admin to revoke the token/).isVisible()) && (await rmdlg.getByText(/Tether on that computer enrols it again while its policy says install; to remove the agent, use the session menu or change the policy\./).isVisible()));
  await clearCalls();
  await rmdlg.getByRole("button", { name: "Remove", exact: true }).click();
  check(tag("Remove calls team_status_remove"), await until(async () => (await calls("team_status_remove"))[0]?.uuid === "agent-only-4"));
  check(tag("a removed agent leaves the list"), await until(async () => (await setQuery("FRONT-DESK-AGENT"), (await shown()) === 0)));
  // In tag rules health terms are plain words, as older desktops read them; the dialog says so.
  await setQuery("");
  await p.getByRole("button", { name: "Tag rules" }).click();
  const rdlg = p.getByRole("dialog");
  await rdlg.getByRole("textbox", { name: "Rule tag" }).fill("plain");
  await rdlg.getByRole("textbox", { name: "Rule search" }).fill("health:crit");
  check(tag("rules dialog says health terms are not available in rules"), (await rdlg.getByText("are not available in rules: there they are plain words").isVisible()) && !(await rdlg.getByRole("button", { name: "Add rule" }).isDisabled()));
  await rdlg.getByRole("button", { name: "Close" }).click();
  // Export: failing checks, one row per host and failing check.
  await setQuery("health:crit");
  const ncrit = await shown();
  await p.keyboard.press("Control+k");
  await pal.fill("export hosts");
  await p.keyboard.press("Enter");
  const edlg = p.getByRole("dialog");
  await edlg.getByRole("radio", { name: "Failing checks (CSV)" }).click();
  await edlg.getByRole("radio", { name: /^Shown/ }).click();
  await shot("export");
  await edlg.getByRole("button", { name: "Export…" }).click();
  const fx = await p.evaluate(() => window.__lastExport);
  const lines = fx?.content.trim().split("\r\n") ?? [];
  check(tag("export: failing checks CSV"), /^tether-health-faults-\d{4}-\d{2}-\d{2}\.csv$/.test(fx?.name ?? "") && lines[0] === "id,name,uuid,check,state,since" && lines.length - 1 >= ncrit && lines.some((l) => l.includes(",app/db-service,crit,")), `${fx?.name} ${lines.length - 1} rows, ${ncrit} hosts`);
  await setQuery("");
  // Team dialog: the notification switch.
  await p.getByRole("button", { name: "Team", exact: true }).click();
  check(tag("Team dialog has the notification switch (on)"), (await p.getByRole("switch", { name: "Notify me when a host turns critical or stops reporting" }).getAttribute("aria-checked")) === "true");
  await p.keyboard.press("Escape");

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

  // A member without write sees the claim, but cannot link, unlink or remove.
  await p.goto(`${base}?hosts=5000&team=ro#/`);
  await search.waitFor({ timeout: 60000 });
  await until(async () => (await total()) > 4000, 30000);
  await setQuery("FRONT-DESK-AGENT");
  await rows().first().click();
  await until(() => claimLine.isVisible());
  check(tag("read-only member: Link is disabled with a reason, no Remove"), (await claimLine.getByRole("button", { name: "Link to this host" }).isDisabled()) && (await claimLine.getByText("Only team members who may edit shared tags can link").isVisible()) && !(await p.getByRole("button", { name: "Remove from health monitoring…" }).isVisible()));
  await setQuery("health:crit");
  await rows().first().click();
  await until(() => p.getByTestId("health-section").locator("li[data-check]").first().isVisible());
  check(tag("read-only member: no Unlink or Remove on a linked host"), !(await p.getByRole("button", { name: "Unlink" }).isVisible()) && !(await p.getByRole("button", { name: "Remove from health monitoring…" }).isVisible()));

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

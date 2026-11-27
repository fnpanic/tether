import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { buildHosts, emptyStore, type Host, type StatusSummary } from "./model";
import { parse, search } from "./query";
import { asOf, chipLines, faultsHint, lookupFor, NEGATED_NOTE, negatedSoftware, resetSoftwareCache, runSoftware, searcher, softwareTerms, termKey, type Call } from "./software";

const NOW = 1_800_000_000;
const sum = (uuid: string, p: Partial<StatusSummary> = {}): StatusSummary => ({
  uuid,
  tether_id: null,
  host: uuid.toUpperCase(),
  agent: "0.1.0",
  overall: "ok",
  counts: { ok: 3, warn: 0, crit: 0, unknown: 0, setup: 0 },
  products: [],
  faults: [],
  last_seen: NOW - 60,
  next: 300,
  stale: false,
  relay: "a.example",
  ...p,
});
const list = (at = NOW - 3600) => ({ sbom: { at, sha: "00" } });
// dev-1 and dev-2 have lists on relay a, dev-3 on relay b (its newest list), dev-4 none.
const STATUS = [sum("dev-0001", { ...list(), sbomRelay: "a.example" }), sum("dev-0002", { ...list(), sbomRelay: "a.example" }), sum("dev-0003", { relay: "a.example", ...list(), sbomRelay: "b.example" }), sum("dev-0004")];
const fleet = (): Host[] => buildHosts(emptyStore(), [], [], null, NOW, STATUS);
const RELAYS = ["a.example", "b.example"];

/** A stand-in for `team_status_software`: per relay an answer or an error text. */
function relays(per: Record<string, unknown | ((q: string, v: string) => unknown)>) {
  const calls: { relay: string; q: string; v: string }[] = [];
  const call: Call = async (cmd, args) => {
    expect(cmd).toBe("team_status_software");
    const { relay, q, v } = args as { relay: string; q: string; v: string };
    calls.push({ relay, q, v });
    const a = per[relay];
    const r = typeof a === "function" ? a(q, v) : a;
    if (typeof r === "string") throw r;
    return r;
  };
  return { call, calls };
}
const found = (uuids: string[], extra = {}) => ({ sbom: true, built: NOW, hosts: uuids.map((uuid) => ({ uuid, name: "Garnet Browser", version: "126.0.2" })), more: 0, pending: 0, ...extra });

async function run(q: string, per: Parameters<typeof relays>[0]) {
  const r = relays(per);
  const terms = softwareTerms(parse(q).node);
  const res = await runSoftware(terms, RELAYS, r.call);
  const hs = fleet();
  const ids = search(hs, q, NOW, false, lookupFor(res))
    .map((h) => h.id)
    .sort();
  return { ids, lines: chipLines(terms, res, hs, RELAYS), calls: r.calls, res };
}

beforeEach(() => resetSoftwareCache());
afterEach(() => vi.useRealTimers());

describe("software terms", () => {
  it("collects each distinct (name, version) once, also under NOT, OR and brackets", () => {
    expect(softwareTerms(parse('software:chrome -software:chrome (software:"google chrome"@120 | software:x@1) software:Chrome').node)).toEqual([
      { q: "chrome", v: "" },
      { q: "google chrome", v: "120" },
      { q: "x", v: "1" },
    ]);
    expect(softwareTerms(parse("check:backup customer:47").node)).toEqual([]);
  });
});

describe("runSoftware", () => {
  it("asks every relay; a host matches when the relay holding its newest list reports it", async () => {
    // Relay a also reports dev-3, but dev-3's newest list is on b, which does not: no match.
    const r = await run("software:garnet", { "a.example": found(["dev-0001", "dev-0003"]), "b.example": found([]) });
    expect(r.calls.map((c) => c.relay).sort()).toEqual(RELAYS);
    expect(r.calls[0]).toMatchObject({ q: "garnet", v: "" });
    expect(r.ids).toEqual(["m:dev-0001"]);
    expect(r.lines).toHaveLength(1);
    expect(r.lines[0]).toMatch(/^Installed software “garnet”: 1 computer, as of \d[^()]* \(lists as of each computer's last upload; 1 computer without a list\)$/);
  });
  it("-software: only hosts that have a list without a match", async () => {
    const r = await run("-software:garnet", { "a.example": found(["dev-0001"]), "b.example": found([]) });
    expect(r.ids).toEqual(["m:dev-0002", "m:dev-0003"]);
  });
  it("a uuid the summary does not have is ignored: the relay cannot add hosts", async () => {
    const r = await run("software:garnet", { "a.example": found(["dev-0001", "not-in-the-list-1", "__proto__"]), "b.example": found([]) });
    expect(r.ids).toEqual(["m:dev-0001"]);
    expect(r.lines[0]).toContain(": 1 computer,");
  });
  it("sends the version prefix", async () => {
    const r = await run('software:"garnet browser"@126', { "a.example": (q: string, v: string) => found(q === "garnet browser" && v === "126" ? ["dev-0001"] : []), "b.example": found([]) });
    expect(r.ids).toEqual(["m:dev-0001"]);
  });
  it("an old relay (404): not supported, its hosts match nothing, the others still answer", async () => {
    const r = await run("software:garnet", { "a.example": found(["dev-0001"]), "b.example": "unsupported: This server does not offer software search." });
    expect(r.ids).toEqual(["m:dev-0001"]);
    expect(r.lines).toContain("b.example does not support software search (needs the relay update of release 0.7.0)");
    expect(r.lines).toContain("Results from a.example only.");
    // dev-3's list is on b: unknown, so -software: does not list it either.
    expect((await run("-software:garnet", { "a.example": found(["dev-0001"]), "b.example": "unsupported: x" })).ids).toEqual(["m:dev-0002"]);
  });
  it("names every outcome in words", async () => {
    const one = async (b: unknown) => (resetSoftwareCache(), await run("software:garnet", { "a.example": found([]), "b.example": b })).lines;
    expect(await one({ sbom: false, hosts: [] })).toContain("software search is switched off on b.example");
    expect(await one("forbidden: Your team membership does not allow software search.")).toContain("your team membership does not allow software search on b.example");
    expect(await one("limit:42: Too many software searches.")).toContain("you ran more than 6 software searches a minute on b.example: wait 42 s, then press Enter");
    expect(await one("reportoff: Health reporting is switched off on this server.")).toContain("health reporting is switched off on b.example: it holds no software lists to search");
    expect(await one("unavailable:5: The server is not ready: status summary not ready")).toContain("b.example is not ready to search yet (it is preparing its status data): press Enter to try again in 5 s");
    expect(await one("overloaded: The server is overloaded; try again shortly.")).toContain("b.example: the server is overloaded; try again shortly");
    expect(await one("offline: connection timed out")).toContain("b.example could not be reached");
    expect(await one("The server answered 500: oops")).toContain("b.example: The server answered 500: oops");
    expect(await one(found([], { more: 120 }))).toContain("more than 2000 computers match on b.example: refine the search");
    expect(await one(found([], { pending: 3400 }))).toContain("b.example is still indexing 3,400 lists: until it is done, its computers that it did not name are left out, with or without -");
    expect(await one(found([], { partial: true }))).toContain("b.example stopped before it searched every list: the result is incomplete");
    const plain = await one(found([], { unknown_field: 1 }));
    expect(plain).toHaveLength(1);
    expect(plain[0]).toMatch(/^Installed software “garnet”: 0 computers, as of .* \(lists as of each computer's last upload; 1 computer without a list\)$/);
  });
  it("a capped, partial or not fully indexed answer leaves the hosts it did not name unknown", async () => {
    expect((await run("-software:garnet", { "a.example": found(["dev-0001"], { more: 3 }), "b.example": found([]) })).ids).toEqual(["m:dev-0003"]);
    resetSoftwareCache();
    expect((await run("-software:garnet", { "a.example": found(["dev-0001"], { partial: true }), "b.example": found([]) })).ids).toEqual(["m:dev-0003"]);
    // The relay does not say which lists are not indexed yet: dev-2 may have it, so it is not "without".
    resetSoftwareCache();
    const r = await run("-software:garnet", { "a.example": found(["dev-0001"], { pending: 5 }), "b.example": found([]) });
    expect(r.ids).toEqual(["m:dev-0003"]);
    resetSoftwareCache();
    expect((await run("software:garnet", { "a.example": found(["dev-0001"], { pending: 5 }), "b.example": found([]) })).ids).toEqual(["m:dev-0001"]);
  });
  it("asks one relay one term after the other (it runs one search at a time), the relays in parallel", async () => {
    const running = new Set<string>();
    const log: string[] = [];
    const call: Call = async (_c, a) => {
      const { relay, q } = a as { relay: string; q: string };
      if (running.has(relay)) throw "busy:5: The server is busy with another search.";
      running.add(relay);
      log.push(`${relay} ${q}`);
      await new Promise((ok) => setTimeout(ok, 5));
      running.delete(relay);
      return found(q === "garnet" ? ["dev-0001"] : []);
    };
    const terms = softwareTerms(parse("software:garnet -software:aster").node);
    const res = await runSoftware(terms, RELAYS, call);
    expect([...res.values()].flatMap((m) => [...m.values()]).every((a) => a.ok)).toBe(true);
    expect(log.sort()).toEqual(["a.example aster", "a.example garnet", "b.example aster", "b.example garnet"]);
    expect(search(fleet(), "software:garnet -software:aster", NOW, false, lookupFor(res)).map((h) => h.id)).toEqual(["m:dev-0001"]);
  });
  it("a relay busy with another search is asked again after Retry-After, at most 3 times, and says so", async () => {
    vi.useFakeTimers();
    let busyFor = 2;
    const r = relays({ "a.example": () => (busyFor-- > 0 ? "busy:5: The server is busy with another search." : found(["dev-0001"])), "b.example": found([]) });
    const waits: string[] = [];
    const p = runSoftware(softwareTerms(parse("software:garnet").node), RELAYS, r.call, (relay, w) => waits.push(`${relay} ${w}`));
    await vi.advanceTimersByTimeAsync(4_999);
    expect(r.calls.filter((c) => c.relay === "a.example").length).toBe(1);
    expect(chipLines(softwareTerms(parse("software:garnet").node), null, fleet(), RELAYS, ["a.example"])).toEqual([
      "Searching installed software on 2 servers… Until it answers, software terms match no computer.",
      "a.example is busy with another search; trying again…",
    ]);
    await vi.advanceTimersByTimeAsync(10_001);
    const res = await p;
    expect(r.calls.filter((c) => c.relay === "a.example").length).toBe(3);
    expect(waits).toEqual(["a.example true", "a.example true", "a.example false"]);
    expect(res.get(termKey({ q: "garnet", v: "" }))?.get("a.example")?.ok).toBe(true);
    // Always busy: the first try and 3 more, then a line that offers to try again.
    resetSoftwareCache();
    const always = relays({ "a.example": "busy:5: busy", "b.example": found([]) });
    const q = runSoftware(softwareTerms(parse("software:garnet").node), RELAYS, always.call);
    await vi.advanceTimersByTimeAsync(60_000);
    const gave = await q;
    expect(always.calls.filter((c) => c.relay === "a.example").length).toBe(4);
    expect(chipLines(softwareTerms(parse("software:garnet").node), gave, fleet(), RELAYS)).toContain("a.example stayed busy with other searches: press Enter to try again");
    // This member's own limit is not retried: the wait is shown.
    resetSoftwareCache();
    const lim = relays({ "a.example": "limit:42: Too many software searches.", "b.example": found([]) });
    const l = runSoftware(softwareTerms(parse("software:garnet").node), RELAYS, lim.call);
    await vi.advanceTimersByTimeAsync(60_000);
    await l;
    expect(lim.calls.filter((c) => c.relay === "a.example").length).toBe(1);
  });
  it("after a failure that covers the whole relay, its other terms get the same answer without a request", async () => {
    for (const bad of ["offline: timed out", "limit:42: Too many software searches.", "forbidden: no", "reportoff: off", "unsupported: no", { sbom: false, hosts: [] }]) {
      resetSoftwareCache();
      const r = relays({ "a.example": bad, "b.example": found([]) });
      const res = await runSoftware(softwareTerms(parse("software:garnet software:aster software:opal").node), RELAYS, r.call);
      expect(r.calls.filter((c) => c.relay === "a.example").map((c) => c.q)).toEqual(["garnet"]);
      expect(r.calls.filter((c) => c.relay === "b.example").length).toBe(3);
      const got = ["garnet", "aster", "opal"].map((q) => res.get(termKey({ q, v: "" }))!.get("a.example"));
      expect(got[1]).toBe(got[0]);
      expect(got[2]).toBe(got[0]);
    }
    // An error that is not relay-wide is asked term after term.
    resetSoftwareCache();
    const r = relays({ "a.example": "boom", "b.example": found([]) });
    await runSoftware(softwareTerms(parse("software:garnet software:aster").node), RELAYS, r.call);
    expect(r.calls.filter((c) => c.relay === "a.example").length).toBe(2);
  });
  it("the wait before a retry after busy gets 0 to 2 s of jitter", async () => {
    vi.useFakeTimers();
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    let busyFor = 1;
    const r = relays({ "a.example": () => (busyFor-- > 0 ? "busy:5: busy" : found([])) });
    const p = runSoftware(softwareTerms(parse("software:garnet").node), ["a.example"], r.call);
    await vi.advanceTimersByTimeAsync(5_999);
    expect(r.calls.length).toBe(1);
    await vi.advanceTimersByTimeAsync(1);
    await p;
    expect(r.calls.length).toBe(2);
    vi.restoreAllMocks();
  });
  it("a dropped search sends no further request and does not retry", async () => {
    vi.useFakeTimers();
    let release: (v: unknown) => void = () => {};
    const sent: string[] = [];
    const call: Call = (_c, a) => {
      sent.push(String((a as { q: string }).q));
      return new Promise((ok) => (release = ok));
    };
    const s = searcher(call, () => ["a.example"], () => {});
    s.enter("software:garnet software:aster");
    expect(sent).toEqual(["garnet"]);
    s.typed("software:opal"); // the query changes while the first term is on its way
    release(found([]));
    await vi.advanceTimersByTimeAsync(700);
    expect(sent).toEqual(["garnet"]);
    // A busy answer to a search that is closed meanwhile is not asked again.
    resetSoftwareCache();
    sent.length = 0;
    s.enter("software:ruby");
    s.dispose();
    release("busy:5: busy");
    await vi.advanceTimersByTimeAsync(60_000);
    expect(sent).toEqual(["ruby"]);
  });
  it("one request per (term, relay), cached 60 s; failures that pass are asked again", async () => {
    let now = 1000;
    vi.spyOn(Date, "now").mockImplementation(() => now);
    const r = relays({ "a.example": found(["dev-0001"]), "b.example": "offline: timed out" });
    const terms = softwareTerms(parse("software:garnet").node);
    await Promise.all([runSoftware(terms, RELAYS, r.call), runSoftware(terms, RELAYS, r.call)]);
    expect(r.calls.map((c) => c.relay)).toEqual(["a.example", "b.example"]);
    now += 59_000;
    await runSoftware(terms, RELAYS, r.call);
    expect(r.calls.map((c) => c.relay)).toEqual(["a.example", "b.example", "b.example"]);
    now += 2_000;
    await runSoftware(terms, RELAYS, r.call);
    expect(r.calls.filter((c) => c.relay === "a.example").length).toBe(2);
    vi.restoreAllMocks();
  });
  it("without a relay it says so", async () => {
    const terms = softwareTerms(parse("software:garnet").node);
    expect(chipLines(terms, await runSoftware(terms, [], relays({}).call), fleet(), [])).toContain("Software search needs team access to a server.");
  });
});

describe("searcher", () => {
  it("starts on Enter or after 800 ms without typing, never for a term under 2 characters, and drops a stale answer", async () => {
    vi.useFakeTimers();
    let release: (v: unknown) => void = () => {};
    const sent: string[] = [];
    const call: Call = (_c, a) => {
      sent.push(String((a as { q: string }).q));
      return new Promise((ok) => (release = ok));
    };
    const runs: { key: string; done: boolean }[] = [];
    const s = searcher(call, () => ["a.example"], (r) => runs.push({ key: r.key, done: !!r.results }));
    s.typed("software:g");
    await vi.advanceTimersByTimeAsync(2000);
    expect(sent).toEqual([]); // under 2 characters
    s.typed("software:ga");
    await vi.advanceTimersByTimeAsync(700);
    s.typed("software:gar");
    await vi.advanceTimersByTimeAsync(700);
    expect(sent).toEqual([]); // still typing
    await vi.advanceTimersByTimeAsync(100);
    expect(sent).toEqual(["gar"]);
    expect(runs).toEqual([{ key: termKey({ q: "gar", v: "" }), done: false }]);
    // The query changes while the answer is on its way: that answer is not applied.
    s.typed("software:garnet");
    release(found(["dev-0001"]));
    await vi.advanceTimersByTimeAsync(0);
    expect(runs.filter((r) => r.done)).toEqual([]);
    s.enter("software:garnet");
    expect(sent).toEqual(["gar", "garnet"]);
    release(found(["dev-0001"]));
    await vi.advanceTimersByTimeAsync(0);
    expect(runs.at(-1)).toEqual({ key: termKey({ q: "garnet", v: "" }), done: true });
    // The 800 ms timer of the last typing was cleared by Enter: no second request.
    await vi.advanceTimersByTimeAsync(1000);
    expect(sent).toEqual(["gar", "garnet"]);
    s.dispose();
  });
  it("a term too short or too long is named, not sent", () => {
    const hs = fleet();
    expect(chipLines(softwareTerms(parse("software:g").node), new Map(), hs, RELAYS)).toEqual(["software: needs at least 2 characters (“g”), so it matches no computer, with or without -"]);
    expect(chipLines(softwareTerms(parse(`software:${"x".repeat(65)}`).node), new Map(), hs, RELAYS)[0]).toMatch(/^software: at most 64 characters/);
    expect(chipLines(softwareTerms(parse(`software:ab@${"1".repeat(33)}`).node), new Map(), hs, RELAYS)[0]).toMatch(/version at most 32/);
  });
});

describe("unknown until answered", () => {
  it("before the answer, and for a term that cannot be sent, a software term matches nothing, plain or negated; an OR's other branch still matches", async () => {
    const hs = fleet();
    const ids = (q: string, res?: Map<string, Map<string, never>>) =>
      search(hs, q, NOW, false, res && lookupFor(res))
        .map((h) => h.id)
        .sort();
    for (const res of [undefined, new Map()]) {
      expect(ids("software:garnet", res)).toEqual([]);
      expect(ids("-software:garnet", res)).toEqual([]);
      expect(ids("-software:g", res)).toEqual([]);
      expect(ids("software:garnet | host:dev-0004", res)).toEqual(["m:dev-0004"]);
      expect(ids("-software:garnet | host:dev-0004", res)).toEqual(["m:dev-0004"]);
    }
    // Answered for the sendable term only: the unsendable one stays unknown.
    const r = await run("-software:garnet -software:g", { "a.example": found(["dev-0001"]), "b.example": found([]) });
    expect(r.ids).toEqual([]);
    expect(r.calls.map((c) => c.q)).toEqual(["garnet", "garnet"]);
  });
  it("trims the term as the relay does and refuses the characters it refuses, with a plain line", async () => {
    const r = await run('software:"  garnet "', { "a.example": found(["dev-0001"]), "b.example": found([]) });
    expect(r.calls.map((c) => c.q)).toEqual(["garnet", "garnet"]);
    expect(r.ids).toEqual(["m:dev-0001"]);
    expect((await run('software:" g "', {})).calls).toEqual([]);
    for (const c of ["\u0007", "\u200B", "\u200F", "\u202A", "\u202E", "\u2060", "\u2069", "\uFEFF"]) {
      const x = await run(`software:"gar${c}net"`, {});
      expect(x.calls).toEqual([]);
      expect(x.lines[0]).toMatch(/^software: “gar.net” contains an invisible or control character the server refuses; type it again\. Until then it matches no computer$/su);
    }
    expect((await run(`software:garnet@1\u200E`, {})).calls).toEqual([]);
    // U+2028 ends the field in the parser: the text is a plain word, and nothing is sent either.
    expect((await run(`software:"gar\u2028net"`, {})).calls).toEqual([]);
  });
  it("says when the result is from, and that a negated term leaves out what it cannot say", async () => {
    const r = await run("software:garnet", { "a.example": found(["dev-0001"]), "b.example": found([]) });
    expect(asOf(r.res)).toBe(NOW * 1000); // the relay's `built`
    const res = await runSoftware(softwareTerms(parse("software:aster").node), ["a.example"], relays({ "a.example": { ...found([]), built: undefined } }).call);
    expect(Math.abs((asOf(res) ?? 0) - Date.now())).toBeLessThan(1000); // without `built`, the arrival time
    expect(asOf(new Map())).toBeNull();
    expect(negatedSoftware(parse("-software:garnet customer:47").node)).toBe(true);
    expect(negatedSoftware(parse("-(software:a | is:online)").node)).toBe(true);
    expect(negatedSoftware(parse("--software:a").node)).toBe(false);
    expect(negatedSoftware(parse("software:a -customer:47").node)).toBe(false);
    expect(NEGATED_NOTE).toBe("Computers without a software list, or whose list is not indexed yet or could not be read, are not shown either way.");
  });
});

describe("failing checks hint", () => {
  it("counts hosts with more failing checks than the summary holds, only for a check: term", () => {
    const hs = buildHosts(emptyStore(), [], [], null, NOW, [sum("dev-0001", { more_faults: 2 }), sum("dev-0002", { more_faults: 1 }), sum("dev-0003")]);
    expect(faultsHint(parse("check:backup").node, hs)).toBe("2 computers have more failing checks than the summary holds; open them to see all");
    expect(faultsHint(parse("-check:backup=crit customer:47").node, hs)).toBe("2 computers have more failing checks than the summary holds; open them to see all");
    expect(faultsHint(parse("health:crit").node, hs)).toBeNull();
    expect(faultsHint(parse("check:backup").node, hs.slice(2))).toBeNull();
  });
});

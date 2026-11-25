import { describe, expect, it } from "vitest";
import { buildHosts, emptyStore, normTag, type Host, type StatusSummary } from "./model";
import { cmpVersion, fuzzy, parse, search, withoutSoftware } from "./query";
import { applyRules } from "./rules";

const NOW = 1_800_000_000;
function hosts(status: StatusSummary[] = []): Host[] {
  const store = emptyStore();
  store.hosts = {
    "111111111": { name: "Front desk", tags: ["site:berlin", "customer:acme", "env:prod"], notes: "printer jams" },
    "222222222": { tags: ["site:munich", "customer:acme"], pinned: true },
    "333333333": { tags: ["site:berlin:lab", "customer:globex", "env:test", "role:server"] },
    "444444444": { added: NOW - 10 },
  };
  const peers = [
    { id: "111111111", hostname: "BER-WS-001", username: "anna", platform: "Windows", last: NOW - 3600 },
    { id: "222222222", hostname: "MUC-SRV-01", username: "svc", platform: "Linux", last: NOW - 40 * 86400 },
    { id: "333333333", hostname: "ber-mac-07", username: "ben", platform: "Mac OS", last: NOW - 2 * 86400 },
  ];
  const org = [
    { uuid: "u1", id: "222222222", profile: "customer", version: "0.5.20", os: "ubuntu 24.04", last_seen: NOW - 120 },
    { uuid: "u5", id: "555555555", profile: "customer", version: "0.5.21", os: "windows 10.0.26100", last_seen: NOW - 9 * 86400 },
  ];
  const online = new Map([
    ["111111111", true],
    ["222222222", false],
    ["333333333", true],
  ]);
  const h = buildHosts(store, peers, org, online, NOW, status);
  applyRules(h, [{ id: "r", tag: "platform:mac", when: "os:mac" }], NOW);
  return h;
}
const sum = (p: Partial<StatusSummary>): StatusSummary => ({
  uuid: "u0",
  tether_id: null,
  host: "PC",
  agent: "0.1.0",
  overall: "ok",
  counts: { ok: 3, warn: 0, crit: 0, unknown: 0, setup: 0 },
  products: [],
  faults: [],
  last_seen: NOW - 60,
  next: 300,
  stale: false,
  relay: "r",
  ...p,
});
const STATUS = [
  sum({
    uuid: "u1",
    tether_id: "111111111",
    agent: "0.2.1",
    customer: "4711",
    overall: "crit",
    counts: { ok: 3, warn: 0, crit: 1, unknown: 0, setup: 0 },
    faults: [{ id: "app/db-service", state: "crit", since: new Date((NOW - 4 * 86400) * 1000).toISOString() }],
    products: [{ name: "Records Server", version: "26.3.2", role: "server" }],
  }),
  sum({ uuid: "u2", tether_id: "222222222", agent: "0.1.0", customer: "0815" }),
  sum({ uuid: "u9", tether_id: null, host: "LAB-PC", agent: "0.2.0", stale: true }),
];
const ids = (q: string, status: StatusSummary[] = []) =>
  search(hosts(status), q, NOW)
    .map((h) => h.id)
    .sort();

describe("query parser", () => {
  it("reads words, tags, fields and operators", () => {
    expect(parse("berlin #site:ber* -os:mac (a | b)").node).toMatchObject({ k: "and" });
    expect(parse("x OR y").node).toEqual({ k: "or", c: [{ k: "word", v: "x" }, { k: "word", v: "y" }] });
    expect(parse("NOT #env:prod").node).toEqual({ k: "not", c: { k: "tag", v: "env:prod", prefix: false } });
    expect(parse('name:"front desk"').node).toEqual({ k: "field", f: "name", op: "", v: "front desk" });
    expect(parse("ver:>=0.5.20").node).toEqual({ k: "field", f: "ver", op: ">=", v: "0.5.20" });
    expect(parse("tag:customer:*").node).toEqual({ k: "tag", v: "customer:", prefix: true });
    expect(parse("unknown:field").node).toEqual({ k: "word", v: "unknown:field" });
    expect(parse("").node).toBeNull();
  });
  it("never throws on broken input and says what it ignored", () => {
    for (const q of ["(", ")", "((a", "a)", "|", "a |", "-", "NOT", '"open', "#", "a & & b", ")))(((", "--x"]) expect(() => search(hosts(), q, NOW)).not.toThrow();
    expect(parse("(a").error).toBe("Missing )");
    expect(parse("a)").error).toBe("Unmatched )");
    expect(parse("a OR").error).toBe("Nothing after OR");
  });
});

describe("search", () => {
  it("matches fuzzy words across fields", () => {
    expect(ids("front")).toEqual(["111111111"]);
    expect(ids("printer")).toEqual(["111111111"]); // notes
    expect(ids("bws001")).toEqual(["111111111"]); // subsequence of BER-WS-001
    expect(ids("2222")).toEqual(["222222222"]);
    expect(ids("anna berlin")).toEqual(["111111111"]); // every word must match
  });
  it("does not match letters spread over different fields", () => {
    const h = hosts().find((x) => x.id === "444444444")!;
    expect(fuzzy(h.hay, "4x")).toBe(0);
    expect(fuzzy("abc\u0001xbyc", "bc")).toBeGreaterThan(0);
    expect(fuzzy("ab\u0001c", "abc")).toBe(0);
  });
  it("tags match their children and prefixes", () => {
    expect(ids("#site:berlin")).toEqual(["111111111", "333333333"]);
    expect(ids("#site")).toEqual(["111111111", "222222222", "333333333"]);
    expect(ids("#customer:ac*")).toEqual(["111111111", "222222222"]);
    expect(ids("#site:ber")).toEqual([]); // not a prefix without *
  });
  it("combines with AND / OR / NOT and parentheses", () => {
    expect(ids("#customer:acme -#env:prod")).toEqual(["222222222"]);
    expect(ids("#site:munich | #env:test")).toEqual(["222222222", "333333333"]);
    expect(ids("(#site:munich OR #site:berlin) AND NOT #env:test")).toEqual(["111111111", "222222222"]);
    expect(ids("!#site")).toEqual(["444444444", "555555555"]);
  });
  it("filters by status, source, OS, version and age", () => {
    expect(ids("is:online")).toEqual(["111111111", "333333333"]);
    expect(ids("is:offline")).toEqual(["222222222"]);
    expect(ids("is:pinned")).toEqual(["222222222"]);
    expect(ids("is:org")).toEqual(["222222222", "555555555"]);
    expect(ids("is:manual")).toEqual(["444444444"]);
    expect(ids("is:recent")).toEqual(["111111111", "333333333"]);
    expect(ids("os:mac")).toEqual(["333333333"]);
    expect(ids("os:linux")).toEqual(["222222222"]);
    expect(ids("ver:<0.5.21")).toEqual(["222222222"]);
    expect(ids("ver:0.5")).toEqual(["222222222", "555555555"]);
    expect(ids("seen:<1d")).toEqual(["111111111", "222222222"]);
    expect(ids("seen:>7d")).toEqual(["555555555"]);
    expect(ids("has:notes")).toEqual(["111111111"]);
    expect(ids("-has:tags")).toEqual(["444444444", "555555555"]);
    expect(ids("host:ber-")).toEqual(["111111111", "333333333"]);
    expect(ids("user:ben")).toEqual(["333333333"]);
  });
  it("rule tags are searchable and do not repeat stored tags", () => {
    expect(ids("#platform:mac")).toEqual(["333333333"]);
    expect(hosts().find((h) => h.id === "333333333")!.auto).toEqual(["platform:mac"]);
    expect(ids("platform")).toEqual(["333333333"]);
  });
  it("ranks word starts over inner matches over scattered letters", () => {
    const a = fuzzy("x\u0001berlin", "ber");
    const b = fuzzy("x\u0001amber", "ber");
    const c = fuzzy("x\u0001b-e-r", "ber");
    expect(a).toBeGreaterThan(b);
    expect(b).toBeGreaterThan(c);
    expect(c).toBeGreaterThan(0);
    // Pinned and online hosts come first among equal matches.
    const r = search(hosts(), "acme", NOW).map((h) => h.id);
    expect(r[0]).toBe("222222222");
  });
});

describe("health terms", () => {
  const hids = (q: string) => ids(q, STATUS);
  it("health terms", () => {
    expect(hids("health:crit")).toEqual(["111111111"]);
    expect(hids("health:crit | health:stale")).toEqual(["111111111", "m:u9"]);
    expect(hids("health:none")).toEqual(["333333333", "444444444", "555555555"]);
    expect(hids("product:records version:26.3 role:server")).toEqual(["111111111"]);
    expect(hids("version:26.4")).toEqual([]);
    expect(hids("role:client")).toEqual([]);
    expect(hids("check:db-service")).toEqual(["111111111"]);
    expect(hids("agent:0.1")).toEqual(["222222222"]);
    expect(hids("agent:none")).toEqual(["333333333", "444444444", "555555555"]);
    expect(hids("customer:47")).toEqual(["111111111"]);
    expect(hids("customer:4711 | customer:0815")).toEqual(["111111111", "222222222"]);
    expect(hids("customer:0815")).toEqual(["222222222"]);
    expect(hids("customer:711")).toEqual([]); // starts with, not contains
    expect(hids("customer:none")).toEqual(["m:u9"]); // reports health, has no customer; hosts without an agent are agent:none
    expect(hids("since:3d")).toEqual(["111111111"]);
    expect(hids("since:5d")).toEqual([]);
    expect(hids("since:96h")).toEqual(["111111111"]);
    expect(hids("records")).toContain("111111111"); // product names are free text
    expect(hids("#role:server")).toEqual(["333333333"]); // the tag form stays a tag
    expect(hids("role:server")).toEqual(["111111111"]); // the field is the detected role, not the tag
  });
  it("version: pairs with product: in the same AND group", () => {
    const two = [sum({ uuid: "u1", tether_id: "111111111", products: [{ name: "Records Server", version: "26.4.1" }, { name: "Imaging", version: "26.3" }] })];
    expect(ids("product:records version:26.3", two)).toEqual([]);
    expect(ids("product:imaging version:26.3", two)).toEqual(["111111111"]);
    expect(ids("version:26.3", two)).toEqual(["111111111"]);
    expect(ids("product:records version:26.4 os:win", two)).toEqual(["111111111"]);
    expect(ids("product:records (version:26.3 | version:9)", two)).toEqual(["111111111"]); // inside an OR: any product
  });
  it("a rule reads health words as plain text, exactly as before health terms existed", () => {
    // Three rule texts from before: role:, version:, health: and a product name are plain words in a
    // rule (older desktops of the team read them so), not the health fields of the search box.
    const rules = [
      { id: "a", tag: "srv", when: "role:server" },
      { id: "b", tag: "new", when: "version:26.3 | os:mac" },
      { id: "c", tag: "hot", when: "health:crit | records" },
      { id: "d", tag: "cust", when: "customer:47" },
    ];
    const auto = (h: Host[]) => Object.fromEntries(h.filter((x) => x.auto.length).map((x) => [x.id, x.auto]));
    const [withHealth, without] = [hosts(STATUS), hosts()];
    applyRules(withHealth, rules, NOW);
    applyRules(without, rules, NOW);
    expect(auto(withHealth)).toEqual({ "333333333": ["srv", "new"] });
    expect(auto(without)).toEqual(auto(withHealth));
    // The search box keeps the health fields, and product names as free text.
    expect(ids("role:server", STATUS)).toEqual(["111111111"]);
    expect(ids("records", STATUS)).toContain("111111111");
  });
});

describe("software and failure terms", () => {
  const sw = (q: string, at?: string) => ({ k: "field", f: "software", op: "", v: q, ...(at ? { at } : {}) });
  it("parses software:<name>[@<version>], quoted names, negation, OR and brackets", () => {
    expect(parse("software:Chrome").node).toEqual(sw("chrome"));
    expect(parse('software:"Google Chrome"@120').node).toEqual(sw("google chrome", "120"));
    expect(parse("software:7-zip@19").node).toEqual(sw("7-zip", "19"));
    expect(parse('software:"name@home"').node).toEqual(sw("name@home")); // an @ inside the quotes is part of the name
    expect(parse("software:x@").node).toEqual(sw("x"));
    expect(parse("-software:x").node).toEqual({ k: "not", c: sw("x") });
    expect(parse("software:a | software:b").node).toEqual({ k: "or", c: [sw("a"), sw("b")] });
    expect(parse("(software:a | is:online) customer:47").node).toEqual({ k: "and", c: [{ k: "or", c: [sw("a"), { k: "field", f: "is", op: "", v: "online" }] }, { k: "field", f: "customer", op: "", v: "47" }] });
    expect(parse("software:chrome").words).toEqual([]);
  });
  it("a tag rule reads software: as a plain word, as the other health terms", () => {
    expect(parse("software:chrome", true).node).toEqual({ k: "word", v: "software:chrome" });
    const h = hosts(STATUS);
    applyRules(h, [{ id: "s", tag: "sw", when: "software:chrome" }], NOW);
    expect(h.filter((x) => x.auto.includes("sw"))).toEqual([]);
  });
  it("matches through the lookup; hosts without a list never match, in either form", () => {
    const st = STATUS.map((s) => (s.uuid === "u9" ? s : { ...s, sbom: { at: NOW - 60, sha: "00" } }));
    const look = (n: { v: string }, h: Host) => (h.status?.sbom ? h.status.uuid === "u1" && n.v === "chrome" : null);
    const hit = (q: string) =>
      search(hosts(st), q, NOW, false, look)
        .map((h) => h.id)
        .sort();
    expect(hit("software:chrome")).toEqual(["111111111"]);
    expect(hit("-software:chrome")).toEqual(["222222222"]); // has a list without a match; m:u9 and hosts without an agent have none
    expect(hit("software:chrome | is:offline")).toEqual(["111111111", "222222222"]);
    expect(hit("-(software:chrome | software:x)")).toEqual(["222222222"]);
    expect(hit("software:chrome customer:0815")).toEqual([]);
    expect(hit("-software:chrome | health:stale")).toEqual(["222222222", "m:u9"]);
    // Without a lookup (not searched yet) the term matches nothing, either way.
    expect(ids("software:chrome", st)).toEqual([]);
    expect(ids("-software:chrome", st)).toEqual([]);
  });
  it("withoutSoftware drops the software terms the lookup cannot answer yet", () => {
    expect(withoutSoftware(parse("software:a customer:47").node)).toEqual({ k: "field", f: "customer", op: "", v: "47" });
    expect(withoutSoftware(parse("software:a").node)).toBeNull();
    expect(withoutSoftware(parse("-software:a | (software:b is:online)").node)).toEqual({ k: "field", f: "is", op: "", v: "online" });
    expect(withoutSoftware(parse("software:a software:b").node, (n) => n.v === "b")).toEqual(sw("b"));
  });
  it("check:<text>=crit or =warn matches only that state", () => {
    const st = [sum({ uuid: "u1", tether_id: "111111111", faults: [{ id: "backup/age", state: "warn" }] }), sum({ uuid: "u2", tether_id: "222222222", faults: [{ id: "backup/age", state: "crit" }, { id: "sys/disk", state: "warn" }] })];
    expect(ids("check:backup=crit", st)).toEqual(["222222222"]);
    expect(ids("check:backup=warn", st)).toEqual(["111111111"]);
    expect(ids("check:backup", st)).toEqual(["111111111", "222222222"]);
    expect(ids("check:disk=crit", st)).toEqual([]);
    expect(ids("check:=crit", st)).toEqual([]); // no check id contains "=crit"
  });
});

describe("helpers", () => {
  it("normalises tags", () => {
    expect(normTag(" Site : Berlin HQ ")).toBe("site:berlin-hq");
    expect(normTag("#env:prod")).toBe("env:prod");
    expect(normTag("::")).toBeNull();
    expect(normTag("a".repeat(65))).toBeNull();
    expect(normTag("Kunde:Müller&Co")).toBe("kunde:müllerco");
  });
  it("compares versions", () => {
    expect(cmpVersion("0.5.21", "0.5.3")).toBeGreaterThan(0);
    expect(cmpVersion("0.5", "0.5.0")).toBe(0);
    expect(cmpVersion("1.0.0-beta", "1.0.0")).toBe(0);
  });
});

describe("deep nesting", () => {
  it("never throws: nesting past the limit is ignored with an error", () => {
    for (const q of ["(".repeat(50_000), "!".repeat(50_000) + "x", "(".repeat(300) + "berlin" + ")".repeat(300)]) {
      const p = parse(q);
      expect(p.error).toBe("Too many nested brackets");
    }
    expect(parse("(".repeat(150) + "berlin" + ")".repeat(150))).toEqual({ node: { k: "word", v: "berlin" }, words: ["berlin"], error: undefined });
  });
});

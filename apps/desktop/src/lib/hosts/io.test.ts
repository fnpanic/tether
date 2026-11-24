import { describe, expect, it } from "vitest";
import { buildHosts, emptyStore, type StatusSummary } from "./model";
import { exportCsv, exportFaultsCsv, exportJson, mergeImport, parseCsv, parseImport } from "./io";

const FORMULA = "=cmd|' /C calc'!A0";
const store = () => {
  const s = emptyStore();
  s.hosts = {
    "111111111": { name: FORMULA, tags: ["site:berlin", "env:prod"], notes: "admin pw is hunter2", pinned: true },
    "222222222": { tags: ["site:munich"] },
  };
  s.tags = { site: { color: "teal" } };
  s.rules = [{ id: "r1", tag: "platform:mac", when: "os:mac" }];
  s.searches = [{ id: "s1", name: "Berlin", query: "#site:berlin" }];
  return s;
};

describe("export", () => {
  it("leaves notes out unless asked", () => {
    expect(exportJson(store(), { notes: false })).not.toContain("hunter2");
    expect(exportCsv(store(), { notes: false })).not.toContain("hunter2");
    expect(exportJson(store(), { notes: true })).toContain("hunter2");
    expect(exportCsv(store(), { notes: true })).toContain("hunter2");
  });
  it("defuses spreadsheet formulas", () => {
    const csv = exportCsv(store(), { notes: false });
    expect(csv.split("\r\n")[0]).toBe("id,name,tags,pinned");
    expect(csv).toContain(`'${FORMULA}`);
    expect(csv).not.toMatch(/(^|,)=cmd/m);
  });
  it("exports a selection, including hosts without an entry", () => {
    const j = JSON.parse(exportJson(store(), { notes: false, ids: ["222222222", "999999999"] }));
    expect(Object.keys(j.hosts)).toEqual(["222222222", "999999999"]);
  });
  it("round-trips JSON", () => {
    const back = parseImport(exportJson(store(), { notes: true }));
    expect(back.hosts["111111111"]).toEqual({ name: FORMULA, tags: ["site:berlin", "env:prod"], notes: "admin pw is hunter2", pinned: true });
    expect(back.tags).toEqual({ site: { color: "teal" } });
    expect(back.rules.map((r) => [r.tag, r.when])).toEqual([["platform:mac", "os:mac"]]);
    expect(back.searches.map((s) => s.query)).toEqual(["#site:berlin"]);
  });
  it("round-trips CSV, undoing the formula guard", () => {
    const back = parseImport(exportCsv(store(), { notes: true }));
    expect(back.hosts["111111111"].name).toBe(FORMULA);
    expect(back.hosts["111111111"].tags).toEqual(["site:berlin", "env:prod"]);
    expect(back.hosts["111111111"].pinned).toBe(true);
    expect(back.hosts["111111111"].notes).toBe("admin pw is hunter2");
  });
});

describe("health export", () => {
  const NOW = 1_800_000_000;
  const since = "2026-11-19T07:55:00Z";
  const s = emptyStore();
  s.hosts = { "111111111": { name: "Front desk", tags: ["site:berlin"] } };
  const status: StatusSummary[] = [
    {
      uuid: "u1",
      tether_id: "111111111",
      host: "BER-WS-001",
      agent: "0.1.0",
      customer: "4711",
      overall: "crit",
      counts: { ok: 9, warn: 1, crit: 1, unknown: 0, setup: 0 },
      products: [{ name: "Records Server", version: "26.3.2", role: "server" }, { name: "Imaging" }],
      faults: [
        { id: "app/db-service", state: "crit", since },
        { id: "=backup/age", state: "warn" },
      ],
      last_seen: NOW,
      next: 300,
      stale: false,
      relay: "r",
    },
    { uuid: "u9", tether_id: null, host: "LAB-PC", agent: "0.1.0", overall: "ok", counts: { ok: 3, warn: 0, crit: 0, unknown: 0, setup: 0 }, products: [], faults: [], last_seen: NOW, next: 300, stale: true, relay: "r" },
  ];
  const byId = new Map(buildHosts(s, [], [], null, NOW, status).map((h) => [h.id, h]));
  it("exports health fields and one row per failing check", () => {
    const csv = exportCsv(s, { notes: false, ids: ["111111111", "m:u9"], hosts: byId }).split("\r\n");
    expect(csv[0]).toBe("id,name,tags,pinned,health,crit,warn,unknown,setup,products,agent,customer,reported");
    expect(csv[1]).toBe("111111111,Front desk,site:berlin,,crit,1,1,0,0,\"Records Server 26.3.2 (server); Imaging\",0.1.0,4711,2027-01-15T08:00:00.000Z");
    expect(csv[2]).toBe("m:u9,,,,stale,0,0,0,0,,0.1.0,,2027-01-15T08:00:00.000Z");
    const faults = exportFaultsCsv([byId.get("111111111")!, byId.get("m:u9")!]).split("\r\n");
    expect(faults).toEqual(["id,name,uuid,check,state,since", `111111111,Front desk,u1,app/db-service,crit,${since}`, "111111111,Front desk,u1,'=backup/age,warn,", ""]);
  });
  it("JSON carries a health object per host, only with health data", () => {
    const j = JSON.parse(exportJson(s, { notes: false, ids: ["111111111"], hosts: byId }));
    expect(j.hosts["111111111"].health).toEqual({ state: "crit", counts: { ok: 9, warn: 1, crit: 1, unknown: 0, setup: 0 }, products: status[0].products, agent: "0.1.0", customer: "4711", reported: "2027-01-15T08:00:00.000Z", faults: status[0].faults });
    expect(JSON.parse(exportJson(s, { notes: false })).hosts["111111111"].health).toBeUndefined();
    // Health data never comes back in through an import.
    expect(parseImport(exportJson(s, { notes: false, ids: ["111111111"], hosts: byId })).hosts["111111111"]).toEqual({ name: "Front desk", tags: ["site:berlin"] });
  });
});

describe("import", () => {
  it("parses quoted CSV and semicolon CSV", () => {
    expect(parseCsv('a,b\r\n"x, ""y""",z\n')).toEqual([
      ["a", "b"],
      ['x, "y"', "z"],
    ]);
    const imp = parseImport("ID;Name;Tags\n123 456 789;Kasse 1;Site:Berlin, env:prod\n");
    expect(imp.hosts["123456789"]).toEqual({ name: "Kasse 1", tags: ["site:berlin", "env:prod"] });
  });
  it("skips invalid IDs and refuses files without an id column", () => {
    const imp = parseImport("id,name\n../etc,x\n555555555,ok\n");
    expect(Object.keys(imp.hosts)).toEqual(["555555555"]);
    expect(imp.skipped[0]).toContain("row 2");
    expect(() => parseImport("name,tags\nx,y\n")).toThrow(/id/);
    expect(() => parseImport("{nope")).toThrow(/JSON/);
  });
  it("accepts a plain JSON list and drops unknown fields", () => {
    const imp = parseImport(JSON.stringify([{ id: "777777777", name: "x".repeat(200), tags: ["A", 5, "a"], password: "p" }]));
    expect(imp.hosts["777777777"]).toEqual({ name: "x".repeat(64), tags: ["a"] });
  });
  it("merges: tags added, names replaced, rules and searches not duplicated", () => {
    const s = store();
    const merged = mergeImport(s, parseImport(exportJson(s, { notes: false })), 1000);
    expect(merged.rules).toHaveLength(1);
    expect(merged.searches).toHaveLength(1);
    expect(merged.hosts["111111111"].notes).toBe("admin pw is hunter2");
    const m2 = mergeImport(s, parseImport("id,name,tags\n222222222,Munich server,env:prod\n333333333,,new\n"), 1000);
    expect(m2.hosts["222222222"]).toMatchObject({ name: "Munich server", tags: ["site:munich", "env:prod"] });
    expect(m2.hosts["333333333"]).toEqual({ tags: ["new"], added: 1000 });
  });
});

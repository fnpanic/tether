import { describe, expect, it, vi } from "vitest";
import { clean, exportName, filterPrograms, MAX_PROGRAMS, parse, SOFTWARE_ROWS, sbomPlan, visibleRows } from "./sbom";

const prop = (name: string, value: string) => ({ name, value });
const app = (name: string, version = "1.0", publisher = "Example Software", properties?: unknown) => ({ type: "application", name, version, publisher, ...(properties === undefined ? {} : { properties }) });
const doc = (components: unknown[], machine: unknown[] = [], extra: Record<string, unknown> = {}) =>
  JSON.stringify({
    bomFormat: "CycloneDX",
    specVersion: "1.6",
    serialNumber: "urn:uuid:5b0b8a52-0000-4000-8000-000000000001",
    version: 3,
    metadata: { timestamp: "2026-11-24T06:12:00Z", tools: { components: [{ type: "application", name: "tether-monitor", version: "0.1.2" }] }, component: { type: "device", name: "PC-01", properties: machine }, ...extra },
    components: [{ type: "operating-system", name: "Example OS Pro", version: "10.0.22631" }, ...components],
  });

describe("sbom.parse", () => {
  it("reads a document as the agent builds it", () => {
    const s = parse(
      doc(
        [app("Zeta Editor", "2.1", "Zeta Ltd"), app("Records Server", "26.3.2", "Example Health Software", [prop("tether:product", "true"), prop("tether:role", "server")]), app("Alpha Viewer", "9", "Alpha Inc")],
        [prop("tether:hw.manufacturer", "Example Devices"), prop("tether:hw.model", "Station 7010"), prop("tether:hw.serial", "7XK2ABC"), prop("tether:hw.cpu", "Example Core 5"), prop("tether:hw.memory_bytes", "17179869184"), prop("tether:truncated", "12")],
      ),
    );
    expect(s.readable).toBe(true);
    expect(s.os).toEqual({ name: "Example OS Pro", version: "10.0.22631" });
    expect(s.tool).toBe("tether-monitor 0.1.2");
    expect(s.device).toEqual({ manufacturer: "Example Devices", model: "Station 7010", serial: "7XK2ABC", memory: 17179869184, cpu: "Example Core 5" });
    expect([s.truncated, s.omitted, s.capped]).toEqual([true, 12, false]);
    // The operating system is not a program; the product is marked and first, the rest by name.
    expect(s.programs.map((p) => [p.name, p.product, p.role])).toEqual([["Records Server", true, "server"], ["Alpha Viewer", false, ""], ["Zeta Editor", false, ""]]);
    expect(s.programs[0]).toMatchObject({ version: "26.3.2", publisher: "Example Health Software" });
  });

  it("sorts case-insensitively within products and within the rest", () => {
    const p = (n: string) => app(n, "1", "x", [prop("tether:product", "true")]);
    const s = parse(doc([app("b-tool"), p("zed"), app("A-tool"), p("Alpha"), app("c-tool")]));
    expect(s.programs.map((x) => x.name)).toEqual(["Alpha", "zed", "A-tool", "b-tool", "c-tool"]);
  });

  it("no hardware, no truncation: no device, not truncated", () => {
    const s = parse(doc([app("A")]));
    expect([s.device, s.truncated, s.omitted]).toEqual([undefined, false, 0]);
    expect(parse(doc([], [prop("tether:truncated", "many"), prop("tether:hw.memory_bytes", "lots")])).device).toBeUndefined();
  });

  it("memory: only a finite number from 1 to 2^50 bytes is shown", () => {
    const mem = (v: string) => parse(doc([], [prop("tether:hw.model", "M"), prop("tether:hw.memory_bytes", v)])).device!.memory;
    expect([mem("1"), mem(String(2 ** 50)), mem("17179869184")]).toEqual([1, 2 ** 50, 17179869184]);
    for (const v of ["0", "-5", "0.5", String(2 ** 50 + 1), "1e300", "Infinity", "NaN", "", "lots"]) expect([v, mem(v)]).toEqual([v, 0]);
  });

  it("anything that is not a document with components is unreadable, and never throws", () => {
    for (const raw of ["", "not json", "{", "null", "[]", "42", '"text"', "true", "{}", '{"components":"x"}', '{"components":{"length":3}}']) {
      const s = parse(raw);
      expect([raw, s.readable, s.programs.length]).toEqual([raw, false, 0]);
    }
  });

  it("JSON.parse throwing (stack, memory) is unreadable", () => {
    const spy = vi.spyOn(JSON, "parse").mockImplementation(() => {
      throw new RangeError("Maximum call stack size exceeded");
    });
    try {
      expect(parse("{}")).toMatchObject({ readable: false, programs: [] });
    } finally {
      spy.mockRestore();
    }
  });

  it("a document nested 100 000 levels deep does not crash the parser", () => {
    const n = 100_000;
    const deep = '{"a":'.repeat(n) + "1" + "}".repeat(n);
    expect(() => parse(deep)).not.toThrow();
    expect(parse(deep).readable).toBe(false);
    // The same depth inside a field that is read: still no throw, and the readable part is kept.
    const nested = `{"components":[{"type":"application","name":"Kept","properties":${"[".repeat(n)}${"]".repeat(n)}},{"type":"application","name":"Other","publisher":${"[".repeat(n)}${"]".repeat(n)}}]}`;
    const s = parse(nested);
    expect(s.programs.map((p) => p.name).sort()).toEqual(["Kept", "Other"]);
    expect(parse(`{"components":[],"metadata":${"[".repeat(n)}${"]".repeat(n)}}`).readable).toBe(true);
  });

  it("keeps at most 5000 programs out of 100 000 components, and says it cut", () => {
    const comps = Array.from({ length: 100_000 }, (_, i) => app(`P${i}`));
    const s = parse(doc(comps));
    expect(s.programs).toHaveLength(MAX_PROGRAMS);
    expect(MAX_PROGRAMS).toBe(5000);
    expect(s.capped).toBe(true);
    // Exactly 5000 is not a cut.
    expect(parse(doc(comps.slice(0, 5000))).capped).toBe(false);
    expect(parse(doc(comps.slice(0, 5001))).capped).toBe(true);
  });

  it("cuts every string: a name of 1 MB gives 128, a version 64, a publisher 128", () => {
    const big = "x".repeat(1_000_000);
    const s = parse(doc([app(big, big, big, [prop("tether:product", "true"), prop("tether:role", big)])], [prop("tether:hw.serial", big), prop("tether:hw.model", big)]));
    const p = s.programs[0];
    expect([p.name.length, p.version.length, p.publisher.length, p.role.length]).toEqual([128, 64, 128, 128]);
    expect([s.device!.serial.length, s.device!.model.length]).toEqual([128, 128]);
  });

  it("HTML stays text", () => {
    const s = parse(doc([app('<img src=x onerror="alert(1)">', "<b>1</b>", "<script>x</script>")]));
    expect(s.programs[0]).toMatchObject({ name: '<img src=x onerror="alert(1)">', version: "<b>1</b>", publisher: "<script>x</script>" });
  });

  it("skips entries it cannot use: no name, a name that is not text, not objects, other types", () => {
    const s = parse(doc([{ type: "application", version: "1" }, { type: "application", name: 42 }, { type: "application", name: "" }, { type: "application", name: "‮​" }, null, 7, "x", [], { type: "library", name: "Lib" }, { name: "No type" }, app("Fine", 7 as never, 8 as never)]));
    expect(s.programs).toEqual([{ name: "Fine", version: "", publisher: "", product: false, role: "" }]);
  });

  it("properties that are not an array, or hold odd entries, are ignored", () => {
    const odd = [null, 5, "x", { name: 1, value: "tether:product" }, { name: "tether:product", value: true }, { name: "tether:product" }];
    for (const properties of [{ "tether:product": "true" }, "tether:product", 7, null, odd]) {
      const s = parse(doc([app("A", "1", "P", properties)], [], {}));
      expect(s.programs[0].product).toBe(false);
    }
    const m = JSON.stringify({ components: [], metadata: { component: { properties: { "tether:hw.model": "X" } } } });
    expect(parse(m).device).toBeUndefined();
  });

  it("components of the wrong shape never reach the list: components itself, the OS entry, the machine", () => {
    expect(parse(JSON.stringify({ components: [], metadata: null })).readable).toBe(true);
    expect(parse(JSON.stringify({ components: [], metadata: "x" })).device).toBeUndefined();
    expect(parse(JSON.stringify({ components: [{ type: "operating-system" }], metadata: { component: 3, tools: 5 } })).os).toBeUndefined();
  });

  it("an older tools array is read too", () => {
    expect(parse(JSON.stringify({ components: [], metadata: { tools: [{ name: "agent", version: "1" }] } })).tool).toBe("agent 1");
  });

  it("removes control, bidi and zero-width characters from every string it takes", () => {
    const s = parse(doc([app("Pay‮exe.txt", "1\u0000.‍2", "Pub⁦lisher \u0007­﻿")], [prop("tether:hw.serial", "AB‮​CD")]));
    expect(s.programs[0]).toMatchObject({ name: "Payexe.txt", version: "1.2", publisher: "Publisher" });
    expect(s.device!.serial).toBe("ABCD");
  });
});

describe("sbom.clean", () => {
  it("cuts on code points, never inside a surrogate pair", () => {
    const e = "\u{1F600}";
    // 130 emoji: 260 UTF-16 units; the cut keeps 127 whole ones and the mark.
    const out = clean(e.repeat(130), 128);
    expect([...out].length).toBe(128);
    expect(out).toBe(e.repeat(127) + "…");
    expect(out).not.toMatch(/[\ud800-\udbff](?![\udc00-\udfff])|(?<![\ud800-\udbff])[\udc00-\udfff]/);
    // An emoji lands on the cut: 126 letters, an emoji, more letters.
    const at = "a".repeat(126) + e + "b".repeat(10);
    expect(clean(at, 128)).toBe("a".repeat(126) + e + "…");
    const edge = "a".repeat(127) + e + "b";
    expect(clean(edge, 128)).toBe("a".repeat(127) + "…");
    // Within the limit in code points (but over it in units): untouched.
    expect(clean(e.repeat(100), 128)).toBe(e.repeat(100));
    expect(clean("a".repeat(128), 128)).toBe("a".repeat(128));
    expect(clean("a".repeat(129), 128)).toBe("a".repeat(127) + "…");
  });
  it("removes what hides, then cuts", () => {
    expect(clean("‮".repeat(500) + "abc", 128)).toBe("abc");
    expect(clean("a‍b\u0000c‮d", 128)).toBe("abcd");
    expect(clean("ab", 128)).toBe("ab");
  });
});

describe("sbom.filterPrograms", () => {
  const list = parse(doc([app("Alpine Runtime 8 Update 201", "8.0", "Northwind Corporation"), app("Elm Temurin Kit 17", "17.0", "Elm Foundation"), app("Pad", "1", "Sam Example"), app("A.B (x)", "1", "C+D")])).programs;
  it("matches name or publisher, case-insensitive, as plain text", () => {
    expect(filterPrograms(list, "ALPINE").map((p) => p.name)).toEqual(["Alpine Runtime 8 Update 201"]);
    expect(filterPrograms(list, "foundation").map((p) => p.name)).toEqual(["Elm Temurin Kit 17"]);
    expect(filterPrograms(list, " elm ").length).toBe(1);
    expect(filterPrograms(list, "a.b (x)").length).toBe(1);
    expect(filterPrograms(list, "c+d").length).toBe(1);
    expect(filterPrograms(list, ".*")).toEqual([]);
    expect(filterPrograms(list, "")).toBe(list);
    expect(filterPrograms(list, "   ")).toBe(list);
    expect(filterPrograms(list, "nothing")).toEqual([]);
  });
});

describe("sbom.visibleRows (Show all)", () => {
  it("draws 200 of 201 rows until all are asked for", () => {
    const rows = Array.from({ length: 201 }, (_, i) => i);
    expect(SOFTWARE_ROWS).toBe(200);
    expect(visibleRows(rows, false)).toHaveLength(200);
    expect(visibleRows(rows, true)).toHaveLength(201);
    expect(visibleRows(rows.slice(0, 200), false)).toHaveLength(200);
    expect(visibleRows([], false)).toEqual([]);
  });
});

describe("sbom.sbomPlan", () => {
  it("a document belongs to its sha: shown only for that sha; loaded when open and not held", () => {
    // Closed, nothing loaded: nothing to show, nothing to load.
    expect(sbomPlan(false, "a1", null)).toEqual({ show: false, load: false });
    // Opened: load. Loaded for this sha: shown, no second load (also when closed and opened again).
    expect(sbomPlan(true, "a1", null)).toEqual({ show: false, load: true });
    expect(sbomPlan(true, "a1", "a1")).toEqual({ show: true, load: false });
    expect(sbomPlan(false, "a1", "a1")).toEqual({ show: true, load: false });
    // The summary moves on to a new sha while the section is closed: the old document is not shown (no count,
    // no Export) and nothing loads until it is opened; then the new one loads.
    expect(sbomPlan(false, "b2", "a1")).toEqual({ show: false, load: false });
    expect(sbomPlan(true, "b2", "a1")).toEqual({ show: false, load: true });
  });
});

describe("sbom.exportName", () => {
  it("is <host>-<date>.cdx.json with the host reduced to A-Za-z0-9._-", () => {
    expect(exportName("PRAXIS-SRV.01", "2026-11-24")).toBe("PRAXIS-SRV.01-2026-11-24.cdx.json");
    expect(exportName("../Büro PC\\x:y", "2026-11-24")).toBe(".._B_ro_PC_x_y-2026-11-24.cdx.json");
    expect(exportName("", "2026-11-24")).toBe("host-2026-11-24.cdx.json");
    expect(exportName("h".repeat(500), "2026-11-24")).toMatch(/^h{64}-2026-11-24\.cdx\.json$/);
    expect(exportName("a/b", "2026-11-24")).not.toMatch(/[\\/]/);
  });
  it("a name Windows reserves for a device (before the first dot) gets a _ in front", () => {
    for (const h of ["CON.lab", "con.x", "Prn.a.b", "AUX.", "nul.txt", "COM1.x", "com9.y", "LPT1.z", "lpt9.q", "Con", "COM5"]) expect([h, exportName(h, "2026-11-24")]).toEqual([h, `_${h}-2026-11-24.cdx.json`]);
    for (const h of ["COM0.x", "COM10.x", "LPT.x", "CONSOLE.x", "x.CON", "ACON.y", "NULL.x"]) expect([h, exportName(h, "2026-11-24").startsWith("_")]).toEqual([h, false]);
  });
});

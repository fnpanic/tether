import { describe, expect, it, vi } from "vitest";
import { exportName, filterPrograms, MAX_PROGRAMS, parse } from "./sbom";

const prop = (name: string, value: string) => ({ name, value });
const app = (name: string, version = "1.0", publisher = "Example Software", properties?: unknown) => ({ type: "application", name, version, publisher, ...(properties === undefined ? {} : { properties }) });
const doc = (components: unknown[], machine: unknown[] = [], extra: Record<string, unknown> = {}) =>
  JSON.stringify({
    bomFormat: "CycloneDX",
    specVersion: "1.6",
    serialNumber: "urn:uuid:5b0b8a52-0000-4000-8000-000000000001",
    version: 3,
    metadata: { timestamp: "2026-11-24T06:12:00Z", tools: { components: [{ type: "application", name: "tether-monitor", version: "0.1.2" }] }, component: { type: "device", name: "PC-01", properties: machine }, ...extra },
    components: [{ type: "operating-system", name: "Windows 11 Pro", version: "10.0.22631" }, ...components],
  });

describe("sbom.parse", () => {
  it("reads a document as the agent builds it", () => {
    const s = parse(
      doc(
        [app("Zeta Editor", "2.1", "Zeta Ltd"), app("Records Server", "26.3.2", "Example Health Software", [prop("tether:product", "true"), prop("tether:role", "server")]), app("Alpha Viewer", "9", "Alpha Inc")],
        [prop("tether:hw.manufacturer", "Dell Inc."), prop("tether:hw.model", "OptiPlex 7010"), prop("tether:hw.serial", "7XK2ABC"), prop("tether:hw.cpu", "Intel i5"), prop("tether:hw.memory_bytes", "17179869184"), prop("tether:truncated", "12")],
      ),
    );
    expect(s.readable).toBe(true);
    expect(s.os).toEqual({ name: "Windows 11 Pro", version: "10.0.22631" });
    expect(s.tool).toBe("tether-monitor 0.1.2");
    expect(s.device).toEqual({ manufacturer: "Dell Inc.", model: "OptiPlex 7010", serial: "7XK2ABC", memory: 17179869184, cpu: "Intel i5" });
    expect([s.truncated, s.omitted]).toEqual([true, 12]);
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

  it("keeps at most 5000 programs out of 100 000 components", () => {
    const comps = Array.from({ length: 100_000 }, (_, i) => app(`P${i}`));
    const s = parse(doc(comps));
    expect(s.programs).toHaveLength(MAX_PROGRAMS);
    expect(MAX_PROGRAMS).toBe(5000);
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
    const s = parse(doc([{ type: "application", version: "1" }, { type: "application", name: 42 }, { type: "application", name: "" }, null, 7, "x", [], { type: "library", name: "Lib" }, { name: "No type" }, app("Fine", 7 as never, 8 as never)]));
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
});

describe("sbom.filterPrograms", () => {
  const list = parse(doc([app("Java 8 Update 201", "8.0", "Oracle Corporation"), app("Eclipse Temurin JDK 17", "17.0", "Eclipse Adoptium"), app("Notepad", "1", "Don Ho"), app("A.B (x)", "1", "C+D")])).programs;
  it("matches name or publisher, case-insensitive, as plain text", () => {
    expect(filterPrograms(list, "JAVA").map((p) => p.name)).toEqual(["Java 8 Update 201"]);
    expect(filterPrograms(list, "adoptium").map((p) => p.name)).toEqual(["Eclipse Temurin JDK 17"]);
    expect(filterPrograms(list, " eclipse ").length).toBe(1);
    expect(filterPrograms(list, "a.b (x)").length).toBe(1);
    expect(filterPrograms(list, "c+d").length).toBe(1);
    expect(filterPrograms(list, ".*")).toEqual([]);
    expect(filterPrograms(list, "")).toBe(list);
    expect(filterPrograms(list, "   ")).toBe(list);
    expect(filterPrograms(list, "nothing")).toEqual([]);
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
});

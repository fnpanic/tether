import { describe, expect, it } from "vitest";
import { applyShared, emptyStore, shared, type SharedDoc } from "./model";
import { merge3, mergeSet, readShared } from "./sync";

const doc = (hosts: SharedDoc["hosts"], extra: Partial<SharedDoc> = {}): SharedDoc => ({ v: 1, hosts, tags: {}, rules: [], searches: [], ...extra });

describe("three-way merge", () => {
  it("keeps tag additions and removals from both sides", () => {
    expect(mergeSet(["a", "b"], ["a", "b", "c"], ["b", "d"])).toEqual(["b", "d", "c"]);
    const base = doc({ "1": { tags: ["site:berlin", "env:prod"] } });
    const local = doc({ "1": { tags: ["site:berlin", "env:prod", "customer:acme"] } });
    const remote = doc({ "1": { tags: ["site:berlin"] }, "2": { tags: ["x"] } });
    expect(merge3(base, local, remote).hosts).toEqual({ "1": { tags: ["site:berlin", "customer:acme"] }, "2": { tags: ["x"] } });
  });
  it("same field changed on both sides: local wins; one side changed: that side wins", () => {
    const base = doc({ "1": { name: "old" }, "2": { name: "two" } });
    const local = doc({ "1": { name: "mine" }, "2": { name: "two" } });
    const remote = doc({ "1": { name: "theirs" }, "2": { name: "renamed" } });
    expect(merge3(base, local, remote).hosts).toEqual({ "1": { name: "mine" }, "2": { name: "renamed" } });
  });
  it("deletions survive unless the other side changed the entry", () => {
    const base = doc({ "1": { tags: ["a"] }, "2": { tags: ["b"] } }, { rules: [{ id: "r", tag: "t", when: "x" }] });
    const local = doc({ "2": { tags: ["b"] } }, { rules: [] });
    const remote = doc({ "1": { tags: ["a"] }, "2": { tags: ["b", "c"] } }, { rules: [{ id: "r", tag: "t", when: "x" }] });
    const m = merge3(base, local, remote);
    expect(m.hosts).toEqual({ "2": { tags: ["b", "c"] } });
    expect(m.rules).toEqual([]);
    const remote2 = doc({ "1": { tags: ["a", "z"] }, "2": { tags: ["b"] } });
    expect(merge3(base, local, remote2).hosts["1"]).toEqual({ tags: ["a", "z"] });
  });
  it("first sync (no base) unions both sides", () => {
    const m = merge3(null, doc({ "1": { tags: ["a"] } }), doc({ "1": { tags: ["b"] }, "2": { name: "n" } }));
    expect(m.hosts).toEqual({ "1": { tags: ["b", "a"] }, "2": { name: "n" } });
  });
  it("merging an already merged doc changes nothing", () => {
    const base = doc({ "1": { tags: ["a"] } });
    const once = merge3(base, doc({ "1": { tags: ["a", "b"] } }), base);
    expect(merge3(base, once, once)).toEqual(once);
    expect(merge3(once, once, once)).toEqual(once);
  });
});

describe("shared doc", () => {
  it("keeps notes and pins out of the shared part and back in after a merge", () => {
    const s = emptyStore();
    s.hosts = { "1": { name: "n", tags: ["a"], notes: "secret", pinned: true }, "2": { notes: "only" } };
    const sh = shared(s);
    expect(JSON.stringify(sh)).not.toContain("secret");
    expect(sh.hosts).toEqual({ "1": { name: "n", tags: ["a"] } });
    const back = applyShared(s, doc({ "1": { tags: ["b"] }, "3": { name: "new" } }));
    expect(back.hosts).toEqual({ "1": { notes: "secret", pinned: true, tags: ["b"] }, "2": { notes: "only" }, "3": { name: "new" } });
  });
  it("reads a doc from the relay defensively", () => {
    expect(readShared(null)).toBeNull();
    const d = readShared({ hosts: { "../x": { tags: ["a"] }, "123456789": { tags: ["A B", 3], name: 7 } }, rules: [{ id: "r", tag: "t", when: "x" }, { bad: 1 }], searches: "no" });
    expect(d!.hosts).toEqual({ "123456789": { tags: ["a-b"] } });
    expect(d!.rules).toHaveLength(1);
    expect(d!.searches).toEqual([]);
  });
});

describe("team sync between two clients", () => {
  // The relay: a client pulls, merges, and puts only when the merge differs from what it pulled (store.ts teamSync).
  const relay = { doc: null as unknown, puts: 0 };
  const client = (local: SharedDoc) => {
    let base: SharedDoc | null = null;
    return () => {
      const remote = readShared(relay.doc);
      const merged = merge3(base, local, remote);
      if (JSON.stringify(merged) !== JSON.stringify(remote)) {
        relay.doc = JSON.parse(JSON.stringify(merged));
        relay.puts++;
      }
      base = local = merged;
    };
  };
  it("stops writing once both have each other's additions (hosts with names as IDs, rules, searches, colours)", () => {
    const a = client(doc({ zeta: { name: "Z" }, "99999999999": { tags: ["a"] } }, { tags: { site: { color: "red" } }, rules: [{ id: "ra", tag: "t", when: "x" }], searches: [{ id: "sa", name: "A", query: "a" }] }));
    const b = client(doc({ alpha: { tags: ["b"] } }, { tags: { env: { color: "blue" } }, rules: [{ id: "rb", tag: "u", when: "y" }], searches: [{ id: "sb", name: "B", query: "b" }] }));
    a();
    b();
    a();
    expect(relay.puts).toBe(2);
    for (let round = 0; round < 3; round++) {
      a();
      b();
    }
    expect(relay.puts).toBe(2);
    expect(Object.keys((relay.doc as SharedDoc).hosts).sort()).toEqual(["99999999999", "alpha", "zeta"]);
    expect((relay.doc as SharedDoc).rules.map((r) => r.id)).toEqual(["ra", "rb"]);
  });
});

describe("a shared doc is limited like an import", () => {
  it("drops over-long queries, unknown colours, invalid tags and __proto__ keys", () => {
    const raw = JSON.parse(`{"hosts":{"__proto__":{"name":"x"},"1":{"name":"ok"}},"tags":{"__proto__":{"color":"red"},"Site A":{"color":"red"},"env":{"color":"javascript:1"}},
      "rules":[{"id":"r1","tag":"T 1","when":"os:mac","off":true,"future":{"x":1}},{"id":"r0","tag":"t","when":""},{"id":"r2","tag":"t","when":"${"(".repeat(5000)}"},{"id":"r3","tag":"!!","when":"x"},{"id":"__proto__","tag":"t","when":"x"}],
      "searches":[{"id":"s1","name":"n","query":"q"},{"id":"s2","name":"n","query":"${"(".repeat(1001)}"},{"id":"s3","name":"","query":"q"},{"id":"s4","name":"empty","query":""}]}`);
    const d = readShared(raw)!;
    expect(d.hosts).toEqual({ "1": { name: "ok" } });
    expect(d.tags).toEqual({ "site-a": { color: "red" }, env: {} });
    expect(Object.getPrototypeOf(d.tags)).toBe(Object.prototype);
    // Fields of a newer app stay; an empty query is as valid here as in an import; a too long one is cut like there.
    expect(d.rules).toEqual([{ id: "r1", tag: "t-1", when: "os:mac", off: true, future: { x: 1 } }, { id: "r0", tag: "t", when: "" }]);
    expect(d.searches).toEqual([{ id: "s1", name: "n", query: "q" }, { id: "s2", name: "n", query: "(".repeat(1000) }, { id: "s4", name: "empty", query: "" }]);
    expect(readShared({ tags: { a: { color: "red", icon: "x" }, b: { color: "nope", icon: "y" } } })!.tags).toEqual({ a: { icon: "x", color: "red" }, b: { icon: "y" } });
    expect(readShared({ rules: Array.from({ length: 2000 }, (_, i) => ({ id: `r${i}`, tag: "t", when: "x" })) })!.rules).toHaveLength(500);
  });
});

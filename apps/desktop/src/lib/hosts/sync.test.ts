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

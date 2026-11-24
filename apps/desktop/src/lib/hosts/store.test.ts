import { beforeEach, describe, expect, it, vi } from "vitest";

// The backend: what the host list file holds (or that reading it fails) and every command the page sent.
const backend = { file: "" as string | Error, calls: [] as string[] };
vi.mock("../ipc", () => ({
  invoke: async (cmd: string) => {
    backend.calls.push(cmd);
    if (cmd !== "hosts_store_load") return undefined;
    if (backend.file instanceof Error) throw backend.file.message;
    return backend.file;
  },
}));
vi.stubGlobal("window", globalThis);

const { actions, flush, readStore, useHosts } = await import("./store");

const saved = async () => {
  actions.setNotes("1", "edited");
  await flush();
  return backend.calls.includes("hosts_store_save");
};

beforeEach(() => {
  backend.calls = [];
  useHosts.setState({ team: {} });
});

describe("host list file", () => {
  it("only a missing or empty file is a fresh store", () => {
    expect(readStore("")).toEqual({ store: { v: 1, hosts: {}, tags: {}, rules: [], searches: [] } });
    expect(readStore('{"v":1,"hosts":{"1":{"name":"n"}}}')).toEqual({ store: { v: 1, hosts: { "1": { name: "n" } }, tags: {}, rules: [], searches: [] } });
    expect(readStore('{"v":1,"hosts":{"1":').block).toMatch(/damaged/);
    expect(readStore("[]").block).toMatch(/damaged/);
    const newer = readStore('{"v":2,"hosts":{"1":{"name":"n"}}}');
    expect(newer.block).toMatch(/newer version/);
    expect(newer.store.hosts).toEqual({ "1": { name: "n" } });
  });
  it("is saved after a normal load", async () => {
    backend.file = '{"v":1,"hosts":{"1":{"name":"n"}}}';
    await actions.load();
    expect(useHosts.getState().saveError).toBeUndefined();
    expect(await saved()).toBe(true);
  });
  it.each([
    ["damaged", '{"v":1,"hosts":{"1":'],
    ["from a newer version", '{"v":2,"hosts":{}}'],
    ["unreadable", new Error("Access is denied.")],
  ])("is never written over when it is %s", async (_, file) => {
    backend.file = file;
    await actions.load();
    expect(useHosts.getState().loaded).toBe(true);
    expect(useHosts.getState().saveError).toMatch(/hosts\.json.*not saved.*(delete it|Update Tether|try again)/);
    expect(await saved()).toBe(false);
  });
});

describe("team member without write permission", () => {
  it("keeps the team's names, tags, rules and searches; notes and pins stay editable", async () => {
    backend.file = '{"v":1,"hosts":{"1":{"name":"team","tags":["a"]}},"rules":[{"id":"r","tag":"t","when":"x"}],"team":{"enabled":true}}';
    await actions.load();
    useHosts.setState({ team: { me: { name: "Ro", write: false, audit: false } } });
    actions.setName("1", "mine");
    actions.addTags(["1"], ["b"]);
    actions.removeTags(["1"], ["a"]);
    actions.setNotes("1", "note");
    actions.setPinned(["1"], true);
    actions.addHost("2", "new", ["c"]);
    actions.setStore((s) => ({ ...s, rules: [], searches: [{ id: "s", name: "n", query: "q" }], tags: { a: { color: "red" } } }));
    await actions.remove(["1"]);
    const s = useHosts.getState().store;
    expect(s.hosts["1"]).toEqual({ name: "team", tags: ["a"] });
    expect(Object.keys(s.hosts["2"])).toEqual(["added"]);
    expect(s.rules).toHaveLength(1);
    expect(s.searches).toEqual([]);
    expect(s.tags).toEqual({});
    await flush();
  });
});

describe("prototype names", () => {
  it("an edit never writes a store entry for __proto__ or constructor", () => {
    useHosts.setState({ store: { v: 1, hosts: {}, tags: {}, rules: [], searches: [] } });
    actions.addTags(["__proto__", "constructor", "111111111"], ["x"]);
    const after = useHosts.getState().store.hosts;
    expect(Object.getPrototypeOf(after)).toBe(Object.prototype);
    expect(Object.keys(after)).toEqual(["111111111"]);
  });
});

describe("agent-only rows (m:<uuid>)", () => {
  it("are never written to the store or the shared document, and are dropped on load", async () => {
    useHosts.setState({ store: { v: 1, hosts: {}, tags: {}, rules: [], searches: [] } });
    actions.addTags(["m:dev-0001", "111111111"], ["x"]);
    actions.setName("m:dev-0001", "agent");
    actions.setNotes("m:dev-0001", "n");
    actions.setPinned(["m:dev-0001"], true);
    expect(Object.keys(useHosts.getState().store.hosts)).toEqual(["111111111"]);
    const { readShared } = await import("./sync");
    const { shared } = await import("./model");
    expect(Object.keys(readStore('{"v":1,"hosts":{"m:dev-0001":{"name":"a"},"1":{"name":"n"}}}').store.hosts)).toEqual(["1"]);
    expect(Object.keys(readShared({ hosts: { "m:dev-0001": { name: "a" }, "1": { name: "n" } } })!.hosts)).toEqual(["1"]);
    expect(Object.keys(shared({ v: 1, hosts: { "m:dev-0001": { name: "a" }, "1": { name: "n" } }, tags: {}, rules: [], searches: [] }).hosts)).toEqual(["1"]);
  });
});

describe("stored host IDs from before the health view", () => {
  it("stay editable, also outside the Tether ID characters (a direct IPv6 peer)", () => {
    const id = "[2001:db8::1]:21118";
    useHosts.setState({ store: { v: 1, hosts: { [id]: { name: "lab" } }, tags: {}, rules: [], searches: [] } });
    actions.addTags([id], ["site:lab"]);
    actions.setName(id, "lab printer");
    expect(useHosts.getState().store.hosts[id]).toEqual({ name: "lab printer", tags: ["site:lab"] });
  });
});

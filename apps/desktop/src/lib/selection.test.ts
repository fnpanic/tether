import { describe, expect, it } from "vitest";
import { clickSelect, emptySelection, moveSelect, selectAll } from "./selection";

const L = ["a", "b", "c", "d", "e"];

describe("selection", () => {
  it("plain click selects one", () => {
    const s = clickSelect(L, clickSelect(L, emptySelection, "b"), "d");
    expect(s).toEqual({ names: ["d"], anchor: "d", cursor: "d" });
  });
  it("ctrl+click toggles and keeps list order", () => {
    let s = clickSelect(L, emptySelection, "d");
    s = clickSelect(L, s, "a", { toggle: true });
    expect(s.names).toEqual(["a", "d"]);
    s = clickSelect(L, s, "d", { toggle: true });
    expect(s.names).toEqual(["a"]);
  });
  it("shift+click selects the range from the anchor, either direction", () => {
    let s = clickSelect(L, emptySelection, "b");
    expect(clickSelect(L, s, "d", { range: true }).names).toEqual(["b", "c", "d"]);
    s = clickSelect(L, s, "d", { range: true });
    expect(clickSelect(L, s, "a", { range: true }).names).toEqual(["a", "b"]);
  });
  it("shift without an anchor selects one; ctrl+shift adds the range", () => {
    expect(clickSelect(L, emptySelection, "c", { range: true }).names).toEqual(["c"]);
    let s = clickSelect(L, emptySelection, "a");
    s = clickSelect(L, s, "d", { toggle: true });
    expect(clickSelect(L, s, "e", { toggle: true, range: true }).names).toEqual(["a", "d", "e"]);
  });
  it("an anchor that left the list is ignored", () => {
    const s = { names: ["x"], anchor: "x", cursor: "x" };
    expect(clickSelect(L, s, "c", { range: true }).names).toEqual(["c"]);
  });
  it("select all", () => {
    expect(selectAll(L, emptySelection)).toEqual({ names: L, anchor: "a", cursor: "a" });
    expect(selectAll([], emptySelection).names).toEqual([]);
  });
  it("arrow keys move and shift+arrow extends", () => {
    let s = moveSelect(L, emptySelection, 1);
    expect(s.names).toEqual(["a"]);
    s = moveSelect(L, s, 1, true);
    s = moveSelect(L, s, 1, true);
    expect(s.names).toEqual(["a", "b", "c"]);
    expect(moveSelect(L, s, -1).names).toEqual(["b"]);
    expect(moveSelect(L, { names: ["e"], anchor: "e", cursor: "e" }, 1).names).toEqual(["e"]);
    expect(moveSelect(L, emptySelection, -1).names).toEqual(["e"]);
    expect(moveSelect([], emptySelection, 1)).toBe(emptySelection);
  });
});

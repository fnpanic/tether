import { describe, expect, it } from "vitest";
import { agentLine, cmAdd, cmEmpty, cmNote, cmRemove, cmShow, requestText, type CmClient, type CmState } from "./cm";

const c = (id: number, extra: Partial<CmClient> = {}): CmClient => ({ id, peer_id: `${id}00000000`, name: `user${id}`, authorized: false, is_file_transfer: false, ...extra });

describe("connection manager: shown client", () => {
  it("a second request does not replace the one on screen; it gets a mark", () => {
    let s: CmState<CmClient> = cmAdd(cmEmpty, c(1));
    expect(s.active).toBe(1);
    s = cmAdd(s, c(2));
    expect(s.active).toBe(1);
    expect(s.attention).toEqual([2]);
    // Accepting the first (the core announces it again) keeps the order and the marks.
    s = cmAdd(s, c(1, { authorized: true }));
    expect(s.clients.map((x) => [x.id, x.authorized])).toEqual([[1, true], [2, false]]);
    expect(s.attention).toEqual([2]);
    s = cmShow(s, 2);
    expect([s.active, s.attention]).toEqual([2, []]);
  });
  it("a message from another client marks it; removing the shown one shows who is left", () => {
    let s = cmAdd(cmAdd(cmEmpty, c(1, { authorized: true })), c(2, { authorized: true }));
    expect(s.attention).toEqual([]);
    s = cmNote(cmNote(s, 2), 2);
    expect([s.active, s.attention]).toEqual([1, [2]]);
    expect(cmNote(s, 1)).toBe(s);
    s = cmRemove(s, 1);
    expect([s.active, s.clients.length]).toEqual([2, 1]);
    expect(cmRemove(s, 2)).toEqual(cmEmpty);
    // Only a request is left: it is not shown by itself (the user picks it), and the next request does not take over either.
    s = cmRemove(cmAdd(cmAdd(cmEmpty, c(1, { authorized: true })), c(2)), 1);
    expect([s.active, s.attention]).toEqual([null, [2]]);
  });
});

describe("connection manager: request text", () => {
  it("names the ID, says the name is unverified, and what is asked for", () => {
    expect(requestText(c(1), "100 000 000")).toBe("user1 (ID 100 000 000) asks to see and control this screen. The name is chosen by the other side and is not verified.");
    expect(requestText(c(1, { name: "" }), "1")).toBe("The device with ID 1 asks to see and control this screen.");
    expect(requestText(c(1, { is_file_transfer: true }), "1")).toContain("send and receive files");
    expect(requestText(c(1, { is_view_camera: true }), "1")).toContain("camera");
    expect(requestText(c(1, { is_terminal: true }), "1")).toContain("terminal");
    expect(requestText(c(1, { port_forward: "10.0.0.5:3389" }), "1")).toContain("reach 10.0.0.5:3389 through this computer");
  });
});

describe("connection manager: the technician's agent action", () => {
  it("installed or removed: one plain line", () => {
    expect(agentLine({ id: 1, action: "install", outcome: "installed" })).toBe("The technician installed the health monitoring agent.");
    expect(agentLine({ id: 1, action: "remove", outcome: "removed-by-tether" })).toBe("The technician removed the health monitoring agent.");
  });
  it("another outcome: a neutral line that does not repeat it", () => {
    for (const outcome of ["failed", "not-installed", "installed-by-hand", "<b>x</b>", "", 5, null]) {
      expect(agentLine({ id: 1, action: "install", outcome })).toBe("The technician asked to install the health monitoring agent.");
      expect(agentLine({ id: 1, action: "remove", outcome })).toBe("The technician asked to remove the health monitoring agent.");
    }
    expect(agentLine({ id: 1, action: "remove", outcome: "installed" })).toBe("The technician asked to remove the health monitoring agent.");
  });
  it("an action outside install and remove, or no payload: nothing", () => {
    for (const p of [{ id: 1, action: "status", outcome: "installed" }, { id: 1, action: "run", outcome: "installed" }, { id: 1, action: ["install"], outcome: "installed" }, null, "install", undefined]) expect(agentLine(p)).toBeNull();
  });
});

import { describe, expect, it } from "vitest";
import { lockModes, mouseMask, MOUSE, remoteKey, wheelSteps } from "./input";
import { formatId } from "./ipc";

describe("remoteKey (keyboard local to the session toolbar)", () => {
  it("every press sent to the remote gets its release, and nothing typed while local is sent", () => {
    const held = new Set<string>();
    const sent: string[] = [];
    const key = (local: boolean, code: string, down: boolean) => remoteKey(held, local, code, down) && sent.push(code + (down ? "+" : "-"));
    // Ctrl+Alt down go out, Home (the chord) starts local mode: Home is local, Ctrl/Alt still get their release.
    key(false, "ControlLeft", true);
    key(false, "AltLeft", true);
    key(true, "Home", true);
    key(true, "Home", false);
    key(true, "AltLeft", false);
    key(true, "ControlLeft", false);
    // Shift+Tab and arrows on the toolbar.
    for (const [c, d] of [["ShiftLeft", true], ["Tab", true], ["Tab", false], ["ShiftLeft", false], ["ArrowRight", true], ["ArrowRight", false]] as const) key(true, c, d);
    // Escape ends local mode while still down; its release comes after.
    key(true, "Escape", true);
    key(false, "Escape", false);
    expect(held.size).toBe(0);
    // Back on the remote: the same keys are sent whole.
    const again = ["ShiftLeft", "Tab", "ArrowRight", "KeyX", "Home", "Escape"];
    for (const c of again) (key(false, c, true), key(false, c, false));
    expect(sent).toEqual(["ControlLeft+", "AltLeft+", "AltLeft-", "ControlLeft-", ...again.flatMap((c) => [c + "+", c + "-"])]);
  });
  it("a release missed while local does not swallow the next real one", () => {
    const held = new Set(["KeyA"]); // pressed while local, released while another window had the focus
    expect(remoteKey(held, false, "KeyA", true)).toBe(true);
    expect(remoteKey(held, false, "KeyA", false)).toBe(true);
  });
});
import { parsePacket, renderSize, widenTo } from "./video";

describe("input encoding", () => {
  it("encodes buttons as type | bits << 3", () => {
    expect(mouseMask(MOUSE.DOWN, 0)).toBe(1 | (1 << 3));
    expect(mouseMask(MOUSE.UP, 2)).toBe(2 | (2 << 3));
    expect(mouseMask(MOUSE.DOWN, 1)).toBe(1 | (4 << 3));
  });

  it("turns one mouse notch down into y = -1", () => {
    const r = wheelSteps({ deltaX: 0, deltaY: 100, deltaMode: 0 }, { x: 0, y: 0 });
    expect(r.y).toBe(-1);
    expect(r.acc.y).toBe(0);
  });

  it("accumulates small trackpad deltas", () => {
    let acc = { x: 0, y: 0 };
    let total = 0;
    for (let i = 0; i < 10; i++) {
      const r = wheelSteps({ deltaX: 0, deltaY: -25, deltaMode: 0 }, acc);
      acc = r.acc;
      total += r.y;
    }
    expect(total).toBe(2);
  });

  it("reports caps and num lock", () => {
    expect(lockModes({ getModifierState: (k) => k === "CapsLock" })).toBe(2);
    expect(lockModes({ getModifierState: () => true })).toBe(6);
  });
});

describe("formatting and packets", () => {
  it("groups IDs in threes", () => {
    expect(formatId("123456789")).toBe("123 456 789");
    expect(formatId("12 34567")).toBe("1 234 567");
    expect(formatId("abc")).toBe("abc");
  });

  it("parses the 16-byte video header", () => {
    const buf = new ArrayBuffer(20);
    const v = new DataView(buf);
    v.setUint8(0, 0);
    v.setUint8(1, 4);
    v.setUint8(2, 1);
    v.setUint8(3, 2);
    v.setUint32(12, 1234, true);
    const p = parsePacket(buf);
    expect(p).toMatchObject({ kind: 0, codec: 4, key: true, display: 2, pts: 1234 });
    expect(p.data.byteLength).toBe(4);
  });

  it("draws full frames when the decoder reports a bogus half-size visible rect", () => {
    expect(renderSize({ codedWidth: 2560, codedHeight: 1440, visibleRect: { width: 1280, height: 720 } })).toEqual({ w: 2560, h: 1440 });
    expect(renderSize({ codedWidth: 1920, codedHeight: 1088, visibleRect: { width: 1920, height: 1080 } })).toEqual({ w: 1920, h: 1080 });
  });

  it("Retina Mac: a bogus 1280×720 visible rect in a 3456×2240 frame draws the host's 3456×2234 display", () => {
    const f = { codedWidth: 3456, codedHeight: 2240, visibleRect: { x: 0, y: 0, width: 1280, height: 720 } };
    const r = renderSize(f, { w: 3456, h: 2234 });
    expect(r).toEqual({ w: 3456, h: 2234 });
    expect(widenTo(f, r.w, r.h)?.visibleRect).toEqual({ x: 0, y: 0, width: 3456, height: 2234 });
    // Without the display size: the whole coded frame (far larger than the visible rect).
    expect(renderSize(f)).toEqual({ w: 3456, h: 2240 });
    // A display size that does not match the frame (switch in progress) is ignored.
    expect(renderSize({ codedWidth: 1920, codedHeight: 1088, visibleRect: { width: 1920, height: 1080 } }, { w: 3456, h: 2234 })).toEqual({ w: 1920, h: 1080 });
  });

  it("draws the whole coded picture when the visible rect is wrongly half of it (Retina Mac)", () => {
    const half = { codedWidth: 3024, codedHeight: 1964, visibleRect: { x: 0, y: 0, width: 1512, height: 982 } };
    const { w, h } = renderSize(half);
    expect(widenTo(half, w, h)).toEqual({ visibleRect: { x: 0, y: 0, width: 3024, height: 1964 }, displayWidth: 3024, displayHeight: 1964 });
    // Real cropping (alignment padding) is left alone.
    const pad = { codedWidth: 1920, codedHeight: 1088, visibleRect: { x: 0, y: 0, width: 1920, height: 1080 } };
    const r = renderSize(pad);
    expect(widenTo(pad, r.w, r.h)).toBeNull();
  });
});

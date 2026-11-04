import { describe, expect, it } from "vitest";
import { lockModes, mouseMask, MOUSE, wheelSteps } from "./input";
import { formatId } from "./ipc";
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

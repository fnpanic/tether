import { describe, expect, it } from "vitest";
import { CODEC, decoderConfig, KEY_RETRY_MS, KeyRecovery, NATIVE_MS, STALL_FRAMES, STALL_MS, watchdog, WATCH0, watchNative, type Watch } from "./video";

describe("decoderConfig", () => {
  it("decodes the software codecs with the host's BT.601 limited-range matrix", () => {
    for (const c of [CODEC.vp8, CODEC.vp9, CODEC.av1]) {
      expect(decoderConfig(c).colorSpace).toEqual({ primaries: "bt709", transfer: "iec61966-2-1", matrix: "smpte170m", fullRange: false });
    }
    for (const c of [CODEC.h264, CODEC.h265]) expect(decoderConfig(c).colorSpace).toBeUndefined();
    expect(decoderConfig(CODEC.av1).codec).toBe("av01.0.08M.08");
  });
});

describe("KeyRecovery", () => {
  it("after a reconnect waits for the stream's key frame and asks only if it is late", () => {
    const k = new KeyRecovery();
    k.expect(0);
    expect(k.waiting).toBe(true);
    expect(k.poll(KEY_RETRY_MS - 1)).toBe(false); // no refresh (it restarts the host's video)
    expect(k.poll(KEY_RETRY_MS)).toBe(true); // late: ask now
    expect(k.requests).toBe(1);
    expect(k.gotKey(KEY_RETRY_MS + 50)).toBe(KEY_RETRY_MS + 50);
    expect(k.waiting).toBe(false);
  });

  it("requests a key frame at once and repeats until one arrives", () => {
    const k = new KeyRecovery();
    expect(k.need(0)).toBe(true);
    expect(k.need(10)).toBe(false); // already asked
    expect(k.poll(KEY_RETRY_MS - 1)).toBe(false);
    expect(k.poll(KEY_RETRY_MS)).toBe(true); // first request presumably lost: ask again
    expect(k.poll(KEY_RETRY_MS + 5)).toBe(false);
    expect(k.poll(2 * KEY_RETRY_MS)).toBe(true);
    expect(k.requests).toBe(3);
    expect(k.gotKey(2500)).toBe(2500);
    expect(k.waiting).toBe(false);
    expect(k.poll(10_000)).toBe(false); // nothing pending any more
  });

  it("ignores key frames nobody waited for", () => {
    const k = new KeyRecovery();
    expect(k.gotKey(100)).toBe(0);
    expect(k.requests).toBe(0);
  });

  it("starts a new wait after recovering", () => {
    const k = new KeyRecovery();
    k.need(0);
    k.gotKey(50);
    expect(k.need(60)).toBe(true);
    expect(k.gotKey(160)).toBe(100);
  });
});

describe("watchdog", () => {
  /** Run once-a-second checks; `fps` frames fed per second, `draws` whether they are drawn. */
  function run(w: Watch, secs: number, fps: number, draws: boolean, t0 = 0, active = true) {
    const acts: string[] = [];
    let fed = w.fed;
    let drawn = w.drawn;
    for (let i = 1; i <= secs; i++) {
      fed += fps;
      if (draws) drawn += fps;
      const [n, a] = watchdog(w, fed, drawn, t0 + i * 1000, active);
      w = n;
      if (a) acts.push(`${i}:${a}`);
    }
    return { w, acts };
  }

  it("leaves a healthy stream and a still screen alone", () => {
    expect(run(WATCH0, 120, 30, true).acts).toEqual([]);
    // Still screen: the host sends nothing, for minutes.
    const live = run(WATCH0, 10, 30, true).w;
    expect(run(live, 300, 0, false, 10_000).acts).toEqual([]);
  });

  it("falls back to native decoding when fed frames are not drawn, then asks for another codec", () => {
    const { w, acts } = run(WATCH0, STALL_MS / 1000 + 1, 30, false);
    expect(acts).toEqual([`${STALL_MS / 1000 + 1}:native`]);
    expect(w.stage).toBe(1);
    // Native decoding draws nothing either (no frames reach the page at all).
    const t0 = (STALL_MS / 1000 + 1) * 1000;
    const r = run(w, NATIVE_MS / 1000, 0, false, t0);
    expect(r.acts).toEqual([`${NATIVE_MS / 1000}:codec`]);
    // Then nothing more, whatever happens.
    expect(run(r.w, 60, 30, false, t0 + NATIVE_MS).acts).toEqual([]);
  });

  it("is healthy again once native decoding draws", () => {
    const w = run(WATCH0, 4, 30, false).w;
    const r = run(w, 30, 30, true, 4000);
    expect(r.acts).toEqual([]);
    expect(r.w.stage).toBe(0);
  });

  it("needs enough frames, not just time", () => {
    // A frame or two fed with nothing drawn and then a still screen: below STALL_FRAMES.
    const [w] = watchdog(WATCH0, STALL_FRAMES - 1, 0, 1000, true);
    expect(run(w, 30, 0, false, 1000).acts).toEqual([]);
  });

  it("does nothing while inactive (connecting, reconnecting, view-only) and restarts its clocks", () => {
    expect(run(WATCH0, 30, 30, false, 0, false).acts).toEqual([]);
    const native = watchNative(WATCH0, 0);
    // Reconnecting long after the fallback: the native picture gets its full NATIVE_MS again.
    const off = run(native, 20, 0, false, 0, false).w;
    expect(run(off, NATIVE_MS / 1000 - 1, 0, false, 20_000).acts).toEqual([]);
  });

  it("escalates a decoder error's native fallback too, but never after the codec request", () => {
    expect(run(watchNative(WATCH0, 0), NATIVE_MS / 1000, 0, false).acts).toEqual([`${NATIVE_MS / 1000}:codec`]);
    const done: Watch = { ...WATCH0, stage: 2 };
    expect(watchNative(done, 0)).toBe(done);
  });
});

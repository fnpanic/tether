import { describe, expect, it } from "vitest";
import { CODEC, decoderConfig, KEY_RETRY_MS, ExactRows, KeyRecovery, NATIVE_MS, STALL_FRAMES, STALL_MS, watchdog, WATCH0, watchNative, type Watch } from "./video";

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

describe("ExactRows", () => {
  // A picture 2 px wide and 16 rows high: two strips of 8 rows.
  const rows = (fill: number) => new Uint8Array(2 * 8 * 4).fill(fill);
  const list = (...pairs: number[]) => new DataView(new Uint32Array(pairs).buffer);
  const drawn = (e: ExactRows, flush = true) => e.after(flush).map((r) => [r.y, r.rows[0]]);
  const fresh = () => {
    const e = new ExactRows();
    e.size(2, 16);
    return e;
  };

  it("draws new rows at once, or after the frame that is still in the decoder", () => {
    const e = fresh();
    expect(e.strip(2, 0, 8, 11, rows(1), false)).toMatchObject({ y: 0, h: 8 });
    expect(e.strip(2, 8, 8, 12, rows(2), true)).toBeNull();
    expect(drawn(e, false)).toEqual([]); // another frame is still in the decoder
    expect(drawn(e)).toEqual([[8, 2]]);
    expect(drawn(e)).toEqual([]); // once
  });

  it("puts back over every frame only what the host's last list names with the copy's hash", () => {
    const e = fresh();
    e.strip(2, 0, 8, 11, rows(1), false);
    e.strip(2, 8, 8, 12, rows(2), false);
    // Not listed yet: a frame replaces the picture and nothing goes back over it.
    e.video(false);
    expect(drawn(e)).toEqual([]);
    e.list(2, list(0, 11, 8, 12));
    e.video(false);
    expect(drawn(e)).toEqual([[0, 1], [8, 2]]);
    e.video(false);
    expect(drawn(e)).toEqual([[0, 1], [8, 2]]);
    // Row 8 changed on the host: the list without it comes before the frame that shows the change.
    e.list(2, list(0, 11));
    e.video(false);
    expect(drawn(e)).toEqual([[0, 1]]);
    // A hash that is not the copy's (a strip that never arrived here): not drawn.
    e.list(2, list(0, 11, 8, 99));
    e.video(false);
    expect(drawn(e)).toEqual([[0, 1]]);
    // A list for another width names nothing.
    e.list(3, list(0, 11));
    e.video(false);
    expect(drawn(e)).toEqual([]);
  });

  it("drops the list at a key frame that no list preceded, and on reconnect, display or size change", () => {
    const held = () => {
      const e = fresh();
      e.strip(2, 0, 8, 11, rows(1), false);
      e.list(2, list(0, 11));
      e.video(false);
      expect(drawn(e)).toEqual([[0, 1]]);
      return e;
    };
    // The host's video service started anew (its "nothing is held" may not have arrived).
    let e = held();
    e.video(true);
    expect(drawn(e)).toEqual([]);
    // A key frame right after a list keeps that list.
    e = held();
    e.list(2, list(0, 11));
    e.video(true);
    expect(drawn(e)).toEqual([[0, 1]]);
    // ... but only for that frame's stream: the next key frame without a list drops it.
    e.video(true);
    expect(drawn(e)).toEqual([]);
    e = held();
    e.reconnect();
    e.video(false);
    expect(drawn(e)).toEqual([]);
    // After a reconnect the copy is still there for a list of the new connection.
    e.list(2, list(0, 11));
    e.video(false);
    expect(drawn(e)).toEqual([[0, 1]]);
    e = held();
    e.clear();
    e.list(2, list(0, 11));
    e.video(false);
    expect(drawn(e)).toEqual([]);
    e = held();
    e.size(2, 24);
    e.list(2, list(0, 11));
    e.video(false);
    expect(drawn(e)).toEqual([]);
    // The same size again (a new canvas) keeps everything.
    e = held();
    e.size(2, 16);
    e.video(false);
    expect(drawn(e)).toEqual([[0, 1]]);
  });

  it("refuses rows that are not for this picture and forgets rows that waited when a new frame comes", () => {
    const e = fresh();
    expect(e.strip(3, 0, 8, 1, rows(1), false)).toBeNull(); // another width
    expect(e.strip(2, 16, 8, 1, rows(1), false)).toBeNull(); // below the picture
    expect(e.strip(2, 0, 8, 1, new Uint8Array(5), false)).toBeNull(); // wrong size
    expect(e.strip(2, 0, 0, 1, new Uint8Array(0), false)).toBeNull();
    e.list(2, list(0, 1, 16, 1));
    e.video(false);
    expect(drawn(e)).toEqual([]);
    // Rows that waited are not drawn over a newer frame, but they are kept for the list.
    e.strip(2, 8, 8, 12, rows(2), true);
    e.video(false);
    expect(drawn(e)).toEqual([]);
    e.list(2, list(8, 12));
    e.video(false);
    expect(drawn(e)).toEqual([[8, 2]]);
  });
});

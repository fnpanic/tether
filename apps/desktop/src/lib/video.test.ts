import { describe, expect, it } from "vitest";
import { KEY_RETRY_MS, KeyRecovery } from "./video";

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

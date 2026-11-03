import { describe, expect, it } from "vitest";
import { classify, delayMs, onFailure, reconnectMinutes, type Series } from "./reconnect";

const err = (text: string, retry = true) => ({ type: "error", title: "Connection Error", text, retry });

describe("reconnect", () => {
  it("backs off 0.5, 1, 2, 4, 8 s then every 10 s, relay after two failures", () => {
    let s: Series | null = null;
    const delays: number[] = [];
    const relay: boolean[] = [];
    for (let i = 0; i < 8; i++) {
      const step = onFailure(s, "retryable", 1000 + i, 10);
      if (step.kind !== "retry") throw new Error("gave up");
      s = step.series;
      delays.push(step.delayMs);
      relay.push(step.forceRelay);
    }
    expect(delays).toEqual([500, 1000, 2000, 4000, 8000, 10000, 10000, 10000]);
    expect(relay).toEqual([false, false, true, true, true, true, true, true]);
    expect(delayMs(100)).toBe(10000);
  });

  it("gives up after the policy limit", () => {
    const first = onFailure(null, "retryable", 0, 10);
    if (first.kind !== "retry") throw new Error();
    expect(onFailure(first.series, "retryable", 9 * 60_000, 10).kind).toBe("retry");
    expect(onFailure(first.series, "retryable", 10 * 60_000, 10).kind).toBe("give-up");
    expect(onFailure(null, "retryable", 0, 0).kind).toBe("give-up");
  });

  it("never retries decisions by the other side or by policy", () => {
    for (const m of [
      { type: "re-input-password", title: "", text: "Wrong Password", retry: false },
      err("Closed manually by the peer"),
      err("Connection rejected"),
      err("Connection not allowed"),
      err("Blocked by policy"),
      err("Update required"),
      err("Some unknown failure", false),
    ]) {
      expect(classify(m), m.text).toBe("fatal");
      expect(onFailure(null, classify(m), 0, 10).kind).toBe("give-up");
    }
  });

  it("retries transport failures", () => {
    expect(classify(err("Reset by the peer"))).toBe("retryable");
    expect(classify(err("Timeout"))).toBe("retryable");
  });

  it("offline ends a working session but not a running reconnect series", () => {
    const offline = classify(err("Remote desktop is offline", false));
    expect(offline).toBe("transient");
    expect(onFailure(null, offline, 0, 10).kind).toBe("give-up");
    expect(onFailure({ attempt: 3, startedAt: 0 }, offline, 1000, 10).kind).toBe("retry");
    const blocked = classify({ type: "error", title: "Connection blocked", text: "This connection could not be end-to-end encrypted, so it was refused.", retry: false });
    expect(onFailure({ attempt: 3, startedAt: 0 }, blocked, 1000, 10).kind).toBe("retry");
  });

  it("parses the policy value", () => {
    expect(reconnectMinutes("")).toBe(10);
    expect(reconnectMinutes("abc")).toBe(10);
    expect(reconnectMinutes("0")).toBe(0);
    expect(reconnectMinutes("3")).toBe(3);
  });
});

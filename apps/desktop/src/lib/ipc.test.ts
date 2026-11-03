import { afterEach, describe, expect, it, vi } from "vitest";
import { invoke, subscribe } from "./ipc";

afterEach(() => vi.useRealTimers());

describe("subscribe", () => {
  it("off before the listener is registered leaks nothing (StrictMode effect cleanup)", async () => {
    vi.useFakeTimers();
    const early = vi.fn();
    const late = vi.fn();
    const a = subscribe("session", early);
    a.off();
    await expect(a.ready).rejects.toThrow();
    const b = subscribe("session", late);
    await b.ready;
    await invoke("session_start");
    vi.advanceTimersByTime(1000);
    expect(early).not.toHaveBeenCalled();
    expect(late.mock.calls.filter(([e]) => e.name === "peer_info")).toHaveLength(1);
    b.off();
    await invoke("session_start");
    vi.advanceTimersByTime(1000);
    expect(late.mock.calls.filter(([e]) => e.name === "peer_info")).toHaveLength(1);
  });
});

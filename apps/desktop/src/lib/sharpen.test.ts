import { afterEach, describe, expect, it, vi } from "vitest";
import { AMOUNT, loadSharpen, saveSharpen, sharpenAmount, tapStep } from "./sharpen";

describe("sharpen setting", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("defaults to Auto and round-trips the user's choice", () => {
    const m = new Map<string, string>();
    vi.stubGlobal("localStorage", { getItem: (k: string) => m.get(k) ?? null, setItem: (k: string, v: string) => void m.set(k, v) });
    expect(loadSharpen()).toBe("auto");
    saveSharpen("off");
    expect(loadSharpen()).toBe("off");
    saveSharpen("strong");
    expect(loadSharpen()).toBe("strong");
    m.set("tether.sharpen", "bogus");
    expect(loadSharpen()).toBe("auto");
  });

  it("falls back to Auto when storage is blocked", () => {
    vi.stubGlobal("localStorage", {
      getItem: () => {
        throw new Error("blocked");
      },
      setItem: () => {
        throw new Error("blocked");
      },
    });
    expect(loadSharpen()).toBe("auto");
    expect(() => saveSharpen("off")).not.toThrow();
  });
});

describe("sharpenAmount", () => {
  const fhd = { w: 1920, h: 1080 };
  it("off is off, strong is strong", () => {
    expect(sharpenAmount("off", fhd, { w: 1280, h: 720 })).toBe(0);
    expect(sharpenAmount("strong", fhd, fhd)).toBe(AMOUNT.strong);
  });
  it("auto: skipped at 1:1 (also when the box rounds by a pixel)", () => {
    expect(sharpenAmount("auto", fhd, fhd)).toBe(0);
    expect(sharpenAmount("auto", fhd, { w: 1921, h: 1079 })).toBe(0);
  });
  it("auto: scaled pictures, either way; Strong more", () => {
    expect(sharpenAmount("auto", { w: 2560, h: 1440 }, { w: 1600, h: 900 })).toBe(AMOUNT.scaled);
    expect(sharpenAmount("auto", { w: 1280, h: 720 }, fhd)).toBe(AMOUNT.scaled);
    expect(0).toBeLessThan(AMOUNT.scaled);
    expect(AMOUNT.scaled).toBeLessThan(AMOUNT.strong);
  });
  it("nothing to draw yet: no sharpening", () => {
    expect(sharpenAmount("auto", { w: 0, h: 0 }, fhd)).toBe(0);
    expect(sharpenAmount("strong", fhd, { w: 0, h: 0 })).toBe(0);
  });
});

describe("tapStep", () => {
  it("one output pixel when shrinking, one texel otherwise", () => {
    expect(tapStep({ w: 2560, h: 1440 }, { w: 1280, h: 720 })).toBe(2);
    expect(tapStep({ w: 1280, h: 720 }, { w: 2560, h: 1440 })).toBe(1);
    expect(tapStep({ w: 1920, h: 1080 }, { w: 0, h: 0 })).toBe(1);
  });
});

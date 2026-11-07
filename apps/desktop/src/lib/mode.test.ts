import { describe, expect, it } from "vitest";
import { modePolicy, resolveMode } from "./mode";

describe("home view mode", () => {
  it("policy off hides, forced forces, available follows the user", () => {
    expect(resolveMode("off", "advanced")).toBe("simple");
    expect(resolveMode("forced", "simple")).toBe("advanced");
    expect(resolveMode("available", "advanced")).toBe("advanced");
    expect(resolveMode("available", "simple")).toBe("simple");
  });
  it("reads the policy value, defaulting to available", () => {
    const eff = (v?: string) => ({ profile: "technician", serial: 1, group: null, values: (v ? { "ui.advanced": v } : {}) as Record<string, string>, locked: [], hidden: [], sources: [] });
    expect(modePolicy(null)).toBe("available");
    expect(modePolicy(eff())).toBe("available");
    expect(modePolicy(eff("forced"))).toBe("forced");
    expect(modePolicy(eff("off"))).toBe("off");
    expect(modePolicy(eff("bogus"))).toBe("available");
  });
});

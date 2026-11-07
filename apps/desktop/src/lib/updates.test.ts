import { describe, expect, it } from "vitest";
import { updateLine } from "./updates";

const base = { current: "0.5.23", version: null, notes: "", mandatory: false, forced: false, blocked: null, enabled: true, restartSeconds: 60 };

describe("updateLine", () => {
  it("says up to date, rolling out, staged, halted and out of date", () => {
    expect(updateLine(base)).toBe("Tether 0.5.23 is up to date.");
    expect(updateLine({ ...base, rollout: { version: "0.5.24", percent: 10 } })).toBe("Tether 0.5.23 is up to date. 0.5.24 is rolling out (10 % of devices so far).");
    expect(updateLine({ ...base, version: "0.5.24", phase: "staged" })).toBe("Version 0.5.24 is available. Downloaded and verified.");
    expect(updateLine({ ...base, phase: "halted" })).toContain("halted");
    expect(updateLine({ ...base, stale: true })).toContain("Update information is out of date.");
  });
});

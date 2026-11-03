import { describe, expect, it } from "vitest";
import { errorText, hidden, isOn, locked, usePolicyStore, type Effective } from "./policy";

const eff: Effective = {
  profile: "technician",
  serial: 4,
  group: "helpdesk",
  values: { "permission.clipboard": "N", "connect.outgoing": "Y", "diagnostics.page": "N" },
  locked: ["permission.clipboard", "diagnostics.page"],
  hidden: ["diagnostics.page"],
  sources: ["defaults", "profile", "group:helpdesk"],
};

describe("policy store", () => {
  it("nothing is locked or hidden before the policy is known", () => {
    usePolicyStore.getState().set(null);
    expect(locked("permission.clipboard")).toBe(false);
    expect(hidden("diagnostics.page")).toBe(false);
    expect(isOn(null, "connect.outgoing")).toBe(true);
  });

  it("reports locked and hidden features", () => {
    usePolicyStore.getState().set(eff);
    expect(locked("permission.clipboard")).toBe(true);
    expect(hidden("permission.clipboard")).toBe(false);
    expect(locked("diagnostics.page")).toBe(true);
    expect(hidden("diagnostics.page")).toBe(true);
    expect(locked("connect.outgoing")).toBe(false);
    expect(isOn(eff, "diagnostics.page")).toBe(false);
  });

  it("shows only the human text of policy errors", () => {
    expect(errorText("locked:update-required: Install the required Tether update to start sessions.")).toBe(
      "Install the required Tether update to start sessions.",
    );
    expect(errorText("invalid device ID")).toBe("invalid device ID");
  });
});

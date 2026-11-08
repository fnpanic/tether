import { describe, expect, it } from "vitest";
import { CLOSED, DECLINED, elevationError, hostNote, uacOutcome } from "./elevation";
import { classify } from "./reconnect";

describe("elevation", () => {
  it("recognises the core's UAC notices and the tags that withdraw them", () => {
    // io_loop.rs (non-flutter): msgbox(msgtype, "Prompt", text, "") and cancel_msgbox("<type>-<title>-<text>-<link>")
    expect(hostNote("custom-uac-nocancel")).toBe("uac");
    expect(hostNote("custom-uac-nocancel-Prompt-Please wait for confirmation of UAC...-")).toBe("uac");
    expect(hostNote("custom-elevated-foreground-nocancel-Prompt-elevated_foreground_window_tip-")).toBe("elevated-window");
    expect(hostNote("error")).toBeNull();
    // Without this mapping the notice became a fatal session error ("Prompt" + Close) that never cleared.
    expect(classify({ type: "custom-uac-nocancel", title: "Prompt", text: "Please wait for confirmation of UAC...", retry: false })).toBe("fatal");
  });

  it("says declined only for Tether's own request that brought no admin access", () => {
    expect(uacOutcome(false, true)).toBe(DECLINED);
    expect(uacOutcome(false, false)).toBe(CLOSED);
    expect(uacOutcome(true, true)).toBeNull();
    expect(uacOutcome(true, false)).toBeNull();
  });

  it("words the host's elevation errors", () => {
    expect(elevationError("No need to elevate")).toMatch(/already on/);
    expect(elevationError("No permission")).toMatch(/turned off/);
    expect(elevationError("Failed to run portable service process")).toMatch(/could not be requested: Failed/);
    // TetherQS with policy customer.elevate off (connection.rs handle_elevation_request).
    expect(elevationError("it is turned off by policy on that computer.")).toBe("Admin access could not be requested: it is turned off by policy on that computer.");
  });
});

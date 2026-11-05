import { describe, expect, it } from "vitest";
import { routeLabel } from "./route";

describe("routeLabel", () => {
  it("names the winning path", () => {
    expect(routeLabel(false, "WebSocket")).toBe("Relay");
    expect(routeLabel(true, "LAN")).toBe("Direct (LAN)");
    expect(routeLabel(true, "WebRTC/IPv6")).toBe("Direct (IPv6)");
    expect(routeLabel(true, "TCP")).toBe("P2P");
    expect(routeLabel(false, "WebRTC")).toBe("Relay (UDP)");
    expect(routeLabel(false, "TCP")).toBe("Relay");
  });
});

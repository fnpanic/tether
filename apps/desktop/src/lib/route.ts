/** Path badge from the core's connection_type: which transport won the race (core client.rs). */
export function routeLabel(direct: boolean, streamType: string): string {
  // A relayed WebRTC session runs over hbbs's TURN (UDP), the others over the WebSocket relay.
  if (!direct) return streamType === "WebRTC" ? "Relay (UDP)" : "Relay";
  if (streamType === "LAN") return "Direct (LAN)";
  if (streamType.includes("IPv6")) return "Direct (IPv6)";
  return "P2P";
}

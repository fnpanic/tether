/** Path badge from the core's connection_type: which transport won the race (core client.rs). */
export function routeLabel(direct: boolean, streamType: string): string {
  // A relayed WebRTC session runs over hbbs's TURN (UDP), the others over the WebSocket relay.
  if (!direct) return streamType === "WebRTC" ? "Relay (UDP)" : "Relay";
  // "WebRTC/LAN": ICE nominated a pair between two private addresses (core webrtc.rs pair_is_lan).
  if (streamType === "LAN" || streamType.endsWith("/LAN")) return "Direct (LAN)";
  if (streamType.includes("IPv6")) return "Direct (IPv6)";
  return "P2P";
}

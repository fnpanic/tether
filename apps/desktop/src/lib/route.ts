/** Path badge from the core's connection_type: which transport won the race (core client.rs). */
export function routeLabel(direct: boolean, streamType: string): string {
  if (!direct) return "Relay";
  if (streamType === "LAN") return "Direct (LAN)";
  if (streamType.includes("IPv6")) return "Direct (IPv6)";
  return "P2P";
}

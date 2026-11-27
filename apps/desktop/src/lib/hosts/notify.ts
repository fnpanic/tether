// Desktop notifications for health (spec 10.5): one per host and change into critical or stale,
// at most one per host per hour. Pure; Hosts.tsx sends them after each team refresh.
import { healthText } from "./health";
import { label, type HealthState, type Host } from "./model";

export interface Sent {
  /** The last known state per host. A host without a summary (`none`, or no row) keeps its old
   *  entry, so a relay that was briefly unreachable does not count as a change when it is back. */
  state: Map<string, HealthState>;
  /** When each host was last notified (unix s). */
  at: Map<string, number>;
}

const HOUR = 3600;
/** More alerts than this in one refresh become one summary toast. */
const BURST = 3;

/** `prev` null = the first refresh since start: it only records the states. */
export function healthAlerts(prev: Sent | null, hosts: Host[], now: number): { alerts: { key: string; title: string; body: string }[]; next: Sent } {
  const next: Sent = { state: new Map(prev?.state ?? []), at: new Map(prev?.at ?? []) };
  const due: Host[] = [];
  for (const h of hosts) {
    if (h.health === "none") continue;
    const before = next.state.get(h.id);
    next.state.set(h.id, h.health);
    if (!prev || (h.health !== "crit" && h.health !== "stale") || before === h.health) continue;
    if (now - (prev.at.get(h.id) ?? -Infinity) < HOUR) continue;
    next.at.set(h.id, now);
    due.push(h);
  }
  if (due.length > BURST) {
    const names = due.slice(0, 5).map(label).join(", ");
    return { alerts: [{ key: "many", title: `${due.length} hosts need attention`, body: due.length > 5 ? `${names} and ${due.length - 5} more` : names }], next };
  }
  return { alerts: due.map((h) => ({ key: h.id, title: `${label(h)} ${h.health === "crit" ? "is critical" : "stopped reporting"}`, body: healthText(h, now) })), next };
}

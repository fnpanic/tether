// What the live policy check (policy_live) says, in the words of the bar, Status and the Live tab.
import type { Live, LiveRelay } from "./ipc";

const host = (server: string) => server.replace(/^https?:\/\//, "").replace(/\/.*$/, "");

/** policy_live's note for a relay that answered 404 (src-tauri/src/main.rs NO_POLICY). */
const NO_POLICY = "No policy published";

/** The relay's policy could not be read (no answer, an error, or not a valid policy): "nothing
 *  published" cannot be told from it. */
export const unread = (r: LiveRelay) => r.serial == null && r.note !== NO_POLICY && r.note != null;

/** Relays that serve the newest verified serial, out of all configured. */
export function liveCount(live: Live | null) {
  const relays = live?.relays ?? [];
  return { on: relays.filter((r) => r.verified && r.serial === live?.serial).length, of: relays.length };
}

/** Why the check is not complete ("host: reason; …"), or null when every relay answered. */
export function unchecked(live: Live | null): string | null {
  const bad = (live?.relays ?? []).filter(unread);
  return bad.length ? bad.map((r) => `${host(r.server)}: ${r.note}`).join("; ") : null;
}

/** Worth asking again soon: the last check failed or a relay did not answer it. */
export const incomplete = (live: Live | null, error: string | null) => !!error || !!live?.relays.some((r) => !r.reachable || unread(r));

/** The policy bar's live part. Nothing found on relays that could not be read is "could not
 *  check", never "nothing published yet". */
export function liveLine(live: Live | null, error: string | null): { tone: "ok" | "warn" | "danger" | "muted"; text: string } {
  if (!live) return error ? { tone: "danger", text: `Live: could not check (${error})` } : { tone: "muted", text: "Live: checking…" };
  const { on, of } = liveCount(live);
  const why = unchecked(live);
  if (live.serial == null && why) return { tone: "danger", text: `Live: could not check (${why})` };
  const text = live.serial == null ? "Live: nothing published yet" : `Live: serial ${live.serial} on ${on}/${of} relay${of === 1 ? "" : "s"}`;
  return { tone: error ? "danger" : live.serial != null && on < of ? "warn" : "ok", text };
}

// Enrolment grants for agents Tether installs (health.enrol-grant), what the editor shows and does
// with them; the backend (grants.rs) makes, checks and revokes them.
import type { Layer, RelayResult } from "./ipc";

export const AGENT = "health.agent";
export const GRANT = "health.enrol-grant";

/** A grant as grants_info describes it. */
export interface GrantInfo {
  /** replaced: renewed, but the newer grant is not on every relay yet; still valid everywhere. */
  status: "valid" | "replaced" | "expired" | "revoked" | "foreign" | "invalid";
  id: string | null;
  exp: number | null;
  customer: string | null;
  made_here: boolean;
  renew: boolean;
  why: string | null;
}
/** A renewed or revoked grant not yet revoked on every relay. */
export interface PendingRevoke {
  id: string;
  layer: string;
  state: "replaced" | "revoked";
  /** Relays still to revoke on. */
  relays: string[];
  /** Replaced: relays not yet known to serve a policy without it (it is revoked nowhere until none is left). */
  waiting: string[];
}
export interface GrantsInfo {
  layers: Record<string, GrantInfo>;
  pending: PendingRevoke[];
  /** Layers whose grant in the published policy is revoked here. */
  live_revoked?: string[];
}

/** The backend's name of a layer: `profile:<name>` or `group:<name>`. */
export const layerKey = (kind: "profile" | "group", name: string) => `${kind}:${name}`;

const active = (layer: Layer, id: string) => {
  const r = layer[id];
  return r && (r.state === "on" || r.state === "locked") ? r.value ?? "" : "";
};

/** `health.agent` of the layer when it needs a grant (on-request, install). */
export function needsGrant(layer: Layer): string | null {
  const v = active(layer, AGENT);
  return v === "on-request" || v === "install" ? v : null;
}

/** The grant token the layer carries ("" when none). */
export const grantOf = (layer: Layer) => active(layer, GRANT);

/** The layer with `token` as its grant. */
export const withGrant = (layer: Layer, token: string): Layer => ({ ...layer, [GRANT]: { state: "on", value: token } });

/** Revoke without Renew: the grant leaves the draft and a health.agent that needed it becomes
 * manual (as before the feature; nothing installed is removed), so the draft stays publishable. */
export function afterRevoke(layer: Layer): Layer {
  const next = { ...layer };
  delete next[GRANT];
  if (needsGrant(layer)) next[AGENT] = { ...layer[AGENT], value: "manual" };
  return next;
}

const day = 86400;

/** Badge text and tone for a grant (`now` in unix seconds). */
export function grantBadge(g: GrantInfo, now: number): { text: string; tone: "ok" | "warn" | "danger" } {
  if (g.status === "valid") {
    const left = Math.floor(((g.exp ?? now) - now) / day);
    return g.renew ? { text: `Expires in ${left} day${left === 1 ? "" : "s"}: renew`, tone: "warn" } : { text: "Valid", tone: "ok" };
  }
  if (g.status === "replaced") return { text: "Replaced by a newer grant (not published)", tone: "warn" };
  const text = { expired: "Expired", revoked: "Revoked", foreign: "Does not verify with this app's admin key: renew", invalid: "Not valid" }[g.status];
  return { text, tone: "danger" };
}

/** A layer without its own grant: the line and badge it shows. A group uses its profile's grant
 * when it has none itself (clients resolve per feature, group over profile). */
export function noGrant(layer: Layer, profile?: Layer): { line: string; badge: string | null } {
  if (profile && grantOf(profile)) return { line: "no grant of its own: uses the profile's grant", badge: null };
  const needs = needsGrant(layer) ?? (profile && !layer[AGENT] ? needsGrant(profile) : null);
  return { line: "no grant", badge: needs ? `Needed: health.agent is ${needs}` : null };
}

/** The editor's line for a grant still to revoke; `name` is the layer as shown. */
export function pendingText(p: PendingRevoke, host: (s: string) => string, name?: string): string {
  const of = name ? ` of ${name}` : "";
  if (p.state === "replaced" && p.waiting.length) return `Old grant ${p.id}${of} is still valid: it is revoked on every relay once all of them serve the published policy. The new policy is missing on ${p.waiting.map(host).join(", ")}.`;
  if (p.state === "replaced") return `Old grant ${p.id}${of}: still to revoke on ${p.relays.map(host).join(", ")}, once every relay serves the published policy without it.`;
  return `Revoked grant ${p.id}${of} not yet revoked on ${p.relays.map(host).join(", ")}.`;
}

/** What a revoke did, per grant and relay. */
export function revokeOutcome(rs: RelayResult[], host: (s: string) => string, publishNote = false): string {
  if (!rs.length) return "Nothing left to revoke on the relays.";
  const ids = [...new Set(rs.map((r) => r.detail?.id as string))];
  const lines = ids.map((id) => {
    const mine = rs.filter((r) => r.detail?.id === id);
    const ok = mine.filter((r) => r.ok).map((r) => host(r.server));
    const bad = mine.filter((r) => !r.ok).map((r) => `${host(r.server)} (${(r.error ?? "").replace(/^Grant \S+ not revoked: /, "")})`);
    return `Grant ${id}: ${ok.length ? `revoked on ${ok.join(", ")}` : "revoked nowhere"}${bad.length ? `; not on ${bad.join(", ")}: use Revoke again` : ""}.`;
  });
  return lines.join(" ") + (publishNote ? " The published policy still sends it to machines: publish the draft so machines stop trying." : "");
}

/** Grants in `info` that should be renewed or replaced (for the reminder after unlocking). */
export const toRenew = (info: GrantsInfo) => Object.entries(info.layers).filter(([, g]) => g.renew || (g.status !== "valid" && g.status !== "replaced"));

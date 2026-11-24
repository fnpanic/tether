// Enrolment grants for agents Tether installs (health.enrol-grant), what the editor shows and does
// with them; the backend (grants.rs) makes, checks and revokes them.
import type { Layer } from "./ipc";

export const AGENT = "health.agent";
export const GRANT = "health.enrol-grant";

/** A grant as grants_info describes it. */
export interface GrantInfo {
  status: "valid" | "expired" | "revoked" | "foreign" | "invalid";
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
  relays: string[];
}
export interface GrantsInfo {
  layers: Record<string, GrantInfo>;
  pending: PendingRevoke[];
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
  const text = { expired: "Expired", revoked: "Revoked", foreign: "Does not verify with this app's admin key: renew", invalid: "Not valid" }[g.status];
  return { text, tone: "danger" };
}

/** Grants in `info` that should be renewed or replaced (for the reminder after unlocking). */
export const toRenew = (info: GrantsInfo) => Object.entries(info.layers).filter(([, g]) => g.renew || g.status !== "valid");

// The signed team document (tether_trust::team::TeamDoc, docs/api.md) as the Team page edits it:
// types, the words for what a publish changes (what a key holder reviews), and the defaults of a
// first document. The checks themselves are the relay's (backend team_check). teamdoc.test.ts.
import type { Role } from "./roles";

export interface Oidc {
  issuer: string;
  tenant: string;
  /** The API registration's client id (v2 access tokens carry it as `aud`). */
  audience: string;
  /** The client id of the desktop app registration (public client, redirect http://localhost). */
  client_id: string;
  scope: string;
}
export interface Grants {
  manage?: Role;
  write?: boolean;
  audit?: boolean;
  sbom?: boolean;
  agent?: boolean;
  customers?: string[];
}
export interface DocMember {
  name: string;
  key: string;
  write: boolean;
  audit: boolean;
  sbom: boolean;
  agent: boolean;
  customers: string[];
  service: boolean;
}
export interface TeamDoc {
  serial: number;
  oidc?: Oidc | null;
  roles: Record<string, Grants>;
  deny: string[];
  oidc_only: boolean;
  members: DocMember[];
}

export const emptyOidc = (): Oidc => ({ issuer: "https://login.microsoftonline.com/<tenant id>/v2.0", tenant: "", audience: "", client_id: "", scope: "api://<API client id>/access" });

/** The app roles of design 12.4 with what they grant (a starting point; the administrator decides). */
export function standardRoles(): Record<string, Grants> {
  return {
    "Tether.Technician": {},
    "Tether.Technician.Write": { write: true },
    "Tether.Software": { sbom: true },
    "Tether.Agent": { agent: true },
    "Tether.SessionAudit": { audit: true },
    "Tether.Manage.Auditor": { manage: "auditor" },
    "Tether.Manage.Operator": { manage: "operator" },
    "Tether.Manage.PolicyEditor": { manage: "policy-editor" },
    "Tether.Manage.Publisher": { manage: "publisher" },
    "Tether.Manage.KeyHolder": { manage: "key-holder" },
  };
}

/** What a role grants, in words. */
export function grantText(g: Grants): string {
  if (g.manage) return `Tether Manage: ${g.manage}`;
  const p = [g.write && "write", g.audit && "audit", g.sbom && "sbom", g.agent && "agent"].filter(Boolean);
  const c = g.customers?.length ? `customers ${g.customers.join(", ")}` : "";
  return [p.length ? p.join(", ") : "team member", c].filter(Boolean).join("; ");
}

function memberText(m: DocMember): string {
  const p = [m.write && "write", m.audit && "audit", m.sbom && "sbom", m.agent && "agent"].filter(Boolean).join(", ") || "read only";
  return `${p}; customers ${m.customers.join(", ") || "none"}${m.service ? "; service" : ""}`;
}

/** Everyone who loses team access with `oidc_only`: roster entries that are not service entries. */
export const lockedOut = (d: TeamDoc) => (d.oidc_only ? d.members.filter((m) => !m.service).map((m) => m.name) : []);

/**
 * One change a publish makes. `text`: values between backticks (shown in a monospace font).
 * `risk`: security-relevant, listed first and shown as a warning. `confirm`: the sign-in settings
 * or a KeyHolder mapping change, which the key holder must tick before signing.
 */
export interface TeamChange {
  text: string;
  risk: boolean;
  confirm: boolean;
}

const v = (s: unknown) => `\`${s}\``;

/** What an entry may reach: the flags that matter and its customers (a role with none: every customer). */
type Reach = { write?: boolean; sbom?: boolean; agent?: boolean; customers: string[] };
const roleReach = (g: Grants): Reach => (g.manage ? { customers: [] } : { ...g, customers: g.customers?.length ? g.customers : ["*"] });

/** What `b` may do that `a` (null: nothing) may not: write, sbom, agent, every customer or a customer pattern not covered before. */
export function gained(a: Reach | null, b: Reach): string[] {
  const out: string[] = (["write", "sbom", "agent"] as const).filter((k) => b[k] && !a?.[k]);
  if (a?.customers.includes("*")) return out;
  if (b.customers.includes("*")) return [...out, "every customer"];
  const more = b.customers.filter((p) => !a?.customers.includes(p));
  return more.length ? [...out, `customers ${more.join(", ")}`] : out;
}

/** What publishing `to` changes against `from` (null: nothing signed yet: everything is added), security-relevant changes first. */
export function teamChanges(from: TeamDoc | null, to: TeamDoc): TeamChange[] {
  const out: TeamChange[] = [];
  const add = (text: string, risk = false, confirm = false) => out.push({ text, risk: risk || confirm, confirm });
  const f = from ?? { serial: 0, oidc: null, roles: {}, deny: [], oidc_only: false, members: [] };
  const o1 = f.oidc ?? null;
  const o2 = to.oidc ?? null;
  if (!o1 && o2) add(`Sign-in on: issuer ${v(o2.issuer)}, tenant ${v(o2.tenant)}, audience ${v(o2.audience)}, client ${v(o2.client_id)}, scope ${v(o2.scope)}`, true, true);
  else if (o1 && !o2) add("Sign-in off: every signed-in session ends on the relays", true, true);
  else if (o1 && o2) {
    for (const k of ["issuer", "tenant", "audience", "client_id", "scope"] as const) if (o1[k] !== o2[k]) add(`Sign-in ${k}: ${v(o1[k])} → ${v(o2[k])} (every signed-in session ends)`, true, true);
  }
  for (const [r, g] of Object.entries(to.roles)) {
    const was = f.roles[r];
    const more = gained(was ? roleReach(was) : null, roleReach(g));
    const holder = (g.manage === "key-holder") !== (was?.manage === "key-holder");
    const why = more.length ? ` (gains ${more.join(", ")})` : "";
    if (!was) add(`Role ${v(r)} added: ${grantText(g)}${why}`, !!g.manage || more.length > 0, holder);
    else if (grantText(was) !== grantText(g)) add(`Role ${v(r)}: ${grantText(was)} → ${grantText(g)}${why}`, !!(g.manage || was.manage) || more.length > 0, holder);
  }
  for (const [r, was] of Object.entries(f.roles)) if (!(r in to.roles)) add(`Role ${v(r)} removed: sessions with only this role end`, !!was.manage, was.manage === "key-holder");
  for (const d of to.deny) if (!f.deny.includes(d)) add(`Denied: ${v(d)} (their sessions end at once)`, true);
  for (const d of f.deny) if (!to.deny.includes(d)) add(`No longer denied: ${v(d)} (may sign in again)`, true);
  if (f.oidc_only !== to.oidc_only) add(to.oidc_only ? `Sign-in only: keys work only for service entries (${lockedOut(to).length} entries lose access)` : "Sign-in only off: every entry's key works again", true);
  const old = new Map(f.members.map((m) => [m.key, m]));
  const now = new Map(to.members.map((m) => [m.key, m]));
  for (const m of to.members) {
    const o = old.get(m.key);
    const more = gained(o ?? null, m);
    const svc = !!o && o.service !== m.service ? (m.service ? " (now a service entry: keeps working with sign-in only)" : " (no longer a service entry)") : "";
    const why = more.length ? ` (gains ${more.join(", ")})` : "";
    if (!o) add(`Add ${m.name} (${memberText(m)})${why}`, more.length > 0 || m.service);
    else if (o.name !== m.name || memberText(o) !== memberText(m)) add(`${o.name === m.name ? m.name : `${o.name} → ${m.name}`}: ${memberText(m)}${why}${svc}`, more.length > 0 || !!svc);
  }
  for (const m of f.members) if (!now.has(m.key)) add(`Remove ${m.name}: their key stops working on every relay that gets this document`);
  // Security-relevant first; the order within each part stays.
  return [...out.filter((c) => c.risk), ...out.filter((c) => !c.risk)];
}

/** The document as the relay compares it (field order and absent optionals do not matter). */
export const sameDoc = (a: TeamDoc, b: TeamDoc) => teamChanges(a, b).length === 0;

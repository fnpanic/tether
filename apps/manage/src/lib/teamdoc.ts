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

/** What publishing `to` changes against `from` (null: nothing signed yet), one line each. */
export function teamChanges(from: TeamDoc | null, to: TeamDoc): string[] {
  const out: string[] = [];
  const f = from ?? { serial: 0, oidc: null, roles: {}, deny: [], oidc_only: false, members: [] };
  const o1 = f.oidc ?? null;
  const o2 = to.oidc ?? null;
  if (!o1 && o2) out.push(`Sign-in on: issuer ${o2.issuer}, tenant ${o2.tenant}, audience ${o2.audience}, client ${o2.client_id}, scope ${o2.scope}`);
  else if (o1 && !o2) out.push("Sign-in off: every signed-in session ends on the relays");
  else if (o1 && o2) {
    for (const k of ["issuer", "tenant", "audience", "client_id", "scope"] as const) if (o1[k] !== o2[k]) out.push(`Sign-in ${k}: ${o1[k]} → ${o2[k]} (every signed-in session ends)`);
  }
  for (const [r, g] of Object.entries(to.roles)) {
    const was = f.roles[r];
    if (!was) out.push(`Role ${r} added: ${grantText(g)}`);
    else if (grantText(was) !== grantText(g)) out.push(`Role ${r}: ${grantText(was)} → ${grantText(g)}`);
  }
  for (const r of Object.keys(f.roles)) if (!(r in to.roles)) out.push(`Role ${r} removed: sessions with only this role end`);
  for (const d of to.deny) if (!f.deny.includes(d)) out.push(`Denied: ${d} (their sessions end at once)`);
  for (const d of f.deny) if (!to.deny.includes(d)) out.push(`No longer denied: ${d}`);
  if (f.oidc_only !== to.oidc_only) out.push(to.oidc_only ? `Sign-in only: keys work only for service entries (${lockedOut(to).length} entries lose access)` : "Sign-in only off: every entry's key works again");
  const old = new Map(f.members.map((m) => [m.key, m]));
  const now = new Map(to.members.map((m) => [m.key, m]));
  for (const m of to.members) {
    const o = old.get(m.key);
    if (!o) out.push(`Add ${m.name} (${memberText(m)})`);
    else if (o.name !== m.name || memberText(o) !== memberText(m)) out.push(`${o.name === m.name ? m.name : `${o.name} → ${m.name}`}: ${memberText(m)}`);
  }
  for (const m of f.members) if (!now.has(m.key)) out.push(`Remove ${m.name}: their key stops working on every relay that gets this document`);
  return out;
}

/** The document as the relay compares it (field order and absent optionals do not matter). */
export const sameDoc = (a: TeamDoc, b: TeamDoc) => teamChanges(a, b).length === 0;

// Device health from the team summary (`team_status`, one answer per relay): pure helpers.
import type { CheckState, HealthState, Host, ReportCheck, StatusHostReply, StatusSummary, Text2 } from "./model";

type RelayAnswer = { relay: string; hosts?: Omit<StatusSummary, "relay">[]; error?: string };

/** One summary per uuid over all relays; the newest `last_seen` wins, on a tie the smaller relay
 *  name (stable between refreshes). A relay that was not reached comes with its last hosts and an error.
 *  The software list is decided on its own: the entry with the newest `sbom.at` names the relay to ask (`sbomRelay`). */
export function mergeStatus(perRelay: RelayAnswer[]): StatusSummary[] {
  const by = new Map<string, StatusSummary>();
  const sb = new Map<string, { at: number; sha: string; relay: string }>();
  for (const r of perRelay ?? []) {
    for (const s of r.hosts ?? []) {
      const old = by.get(s.uuid);
      if (!old || s.last_seen > old.last_seen || (s.last_seen === old.last_seen && r.relay < old.relay)) by.set(s.uuid, { ...s, relay: r.relay });
      const b = s.sbom;
      const o = sb.get(s.uuid);
      if (b && typeof b.at === "number" && typeof b.sha === "string" && (!o || b.at > o.at || (b.at === o.at && r.relay < o.relay))) sb.set(s.uuid, { at: b.at, sha: b.sha, relay: r.relay });
    }
  }
  return [...by.values()].map(({ sbom: _x, ...s }) => {
    const b = sb.get(s.uuid);
    return b ? { ...s, sbom: { at: b.at, sha: b.sha }, sbomRelay: b.relay } : s;
  });
}

const reason = (e: string) => (e.startsWith("unsupported: ") ? "this relay does not report health" : e.replace(/^[a-z-]+: /, ""));

/** The footer's health note for relays that failed (also the whole call failing: one entry with `error`). */
export function statusNote(perRelay: RelayAnswer[]): string | undefined {
  const bad = (perRelay ?? []).filter((r) => r.error);
  if (!bad.length) return undefined;
  // Every relay failed: the reason alone; else name the relays that did.
  return bad.length === perRelay.length && !bad.some((r) => r.hosts) ? reason(bad[0].error!) : bad.map((r) => `${r.relay}: ${reason(r.error!)}`).join(" · ");
}

export function healthOf(s: StatusSummary | undefined): HealthState {
  if (!s) return "none";
  if (s.stale) return "stale";
  if (s.overall === "crit" || s.overall === "warn") return s.overall;
  // UNKNOWN is never a fault; checks only waiting to be configured show as `setup`.
  return s.counts.setup > 0 ? "setup" : "ok";
}

const RANK: Record<HealthState, number> = { conflict: 6, crit: 5, stale: 4, warn: 3, setup: 2, ok: 1, none: 0 };
export const healthRank = (h: HealthState): number => RANK[h];

/** Not OK since: the earliest `since` of the warn and crit faults (unix s), null when none has one. */
export function notSince(s: StatusSummary): number | null {
  let min: number | null = null;
  for (const f of s.faults) {
    const t = f.state !== "unknown" && f.since ? Date.parse(f.since) / 1000 : NaN;
    if (!Number.isNaN(t) && (min === null || t < min)) min = t;
  }
  return min === null ? null : Math.floor(min);
}

const str = (v: unknown): string => (typeof v === "string" ? v : "");

/** A report text in `lang`, else English, else German. */
export const pick = (t: Text2 | undefined, lang: "de" | "en"): string => str(t?.[lang]) || str(t?.en) || str(t?.de);

export const uiLang = (): "de" | "en" => (navigator.language.toLowerCase().startsWith("de") ? "de" : "en");

/** Device text cut for display (title 120, hint 600, value 80). */
export const clip = (s: string | undefined, n: number): string => (!s ? "" : s.length > n ? s.slice(0, n - 1) + "…" : s);

const STATES = new Set<CheckState>(["ok", "warn", "crit", "unknown"]);

/** The report's checks that can be shown; the relay never reads `body`, so anything malformed is dropped. */
export function checksOf(r: StatusHostReply | string | null): ReportCheck[] {
  const list = r && typeof r === "object" ? r.report?.body?.checks : undefined;
  if (!Array.isArray(list)) return [];
  const seen = new Set<string>();
  // A repeated id keeps its first entry (ids are React keys).
  return list.filter((c): c is ReportCheck => !!c && typeof c === "object" && typeof c.id === "string" && STATES.has(c.state) && !seen.has(c.id) && !!seen.add(c.id));
}

export interface CheckGroup {
  name: string;
  checks: ReportCheck[];
}

/** The Health section in the order it is shown (spec 10.2): first the faults of every group
 *  (groups by worst state then name, crit before warn inside), then setup items, then the other
 *  UNKNOWN checks (never a fault), then the passing checks per group (by name). */
export function groupChecks(checks: ReportCheck[]): { faults: CheckGroup[]; setup: ReportCheck[]; unknown: ReportCheck[]; passed: CheckGroup[] } {
  const bad = new Map<string, ReportCheck[]>();
  const ok = new Map<string, ReportCheck[]>();
  const setup: ReportCheck[] = [];
  const unknown: ReportCheck[] = [];
  for (const c of checks) {
    if (c.state === "unknown") {
      (c.setup ? setup : unknown).push(c);
      continue;
    }
    const m = c.state === "ok" ? ok : bad;
    const name = str(c.group) || "Other";
    m.set(name, [...(m.get(name) ?? []), c]);
  }
  const crit = (l: ReportCheck[]) => l.some((c) => c.state === "crit");
  const faults = [...bad].map(([name, l]) => ({ name, checks: [...l].sort((a, b) => Number(b.state === "crit") - Number(a.state === "crit")) }));
  faults.sort((a, b) => Number(crit(b.checks)) - Number(crit(a.checks)) || a.name.localeCompare(b.name));
  const passed = [...ok].map(([name, l]) => ({ name, checks: l })).sort((a, b) => a.name.localeCompare(b.name));
  return { faults, setup, unknown, passed };
}

const n = (k: number, one: string) => `${k} ${one}${k === 1 ? "" : "s"}`;
const dur = (s: number) => (s < 5400 ? `${Math.max(1, Math.round(s / 60))} min` : s < 2 * 86400 ? `${Math.round(s / 3600)} h` : `${Math.round(s / 86400)} d`);

/** The Health column's words (spec 10.1), so a state never rests on colour alone. */
export function healthText(h: Pick<Host, "health" | "status">, now: number): string {
  const c = h.status?.counts;
  switch (h.health) {
    case "crit":
    case "warn":
      return [c?.crit ? n(c.crit, "critical") : "", c?.warn ? n(c.warn, "warning") : ""].filter(Boolean).join(", ") || (h.health === "crit" ? "Critical" : "Warning");
    case "stale":
      return `No health report for ${dur(now - (h.status?.last_seen ?? now))}`;
    case "setup":
      return "Setup needed";
    case "ok":
      return "OK";
    case "none":
      return "No agent";
    case "conflict":
      return "Link conflict";
  }
}

/** What holds a host row's health link: its joined agent, or `conflict` when two devices are
 *  linked to it on different relays; nothing for an agent-only row or no row. */
export function linkHolder(h: Host | undefined): StatusSummary | "conflict" | undefined {
  if (!h || h.agentOnly) return undefined;
  return h.conflict && !h.status ? "conflict" : h.status;
}

/** What the Health section offers about an agent's link to a host row. Only members with write may
 *  link (shown disabled to others), unlink, allow a new claim or remove. `linked`: who confirmed it
 *  (null by: a token issued for that ID); also on the rows of a conflict. `dismissed`: a member
 *  unlinked it, it claims nothing until one allows a new claim (the summary says so; the per-host
 *  answer too). */
export function linkState(h: Pick<Host, "agentOnly" | "status" | "conflict">, rep: StatusHostReply | null, write: boolean, holder?: StatusSummary | "conflict") {
  const s = h.status;
  const claim = h.agentOnly && s?.claimed_id ? s.claimed_id : null;
  const linked = s?.tether_id && (!h.agentOnly || h.conflict) ? { by: rep?.linked_by ?? null, at: rep?.linked_at ?? null } : null;
  const dismissed = !!s && !s.tether_id && !s.claimed_id && (!!s.no_claim || !!rep?.no_claim);
  // The claimed host has a health agent joined already (from any relay), or is in a link conflict:
  // linking this one would only make a conflict that hides that host's health (`linkHolder`).
  const blocked = !claim || !holder ? null : holder === "conflict" ? { host: "", at: null, conflict: true } : { host: holder.host, at: holder.linked_at ?? null };
  const link = !claim ? ("none" as const) : blocked ? ("blocked" as const) : write ? ("on" as const) : ("disabled" as const);
  return { claim, linked, link, blocked, unlink: !!linked && write, remove: !!s && write, dismissed, allow: dismissed && write, conflict: !!h.conflict };
}

/** The line under a linked host: who made the link. A member's confirmation names the member; a
 *  link the relay made from Tether's proof on that computer (`linked_proof`, only ever `true`; an
 *  older relay sends no field) says so; else it came from an enrolment token issued for that ID. */
export function linkedText(h: Pick<Host, "status">, rep: StatusHostReply | null, st: Pick<ReturnType<typeof linkState>, "linked" | "conflict">, day: (t: number | null | undefined) => string): string {
  const l = st.linked;
  if (!l) return "";
  const at = l.at ? `, ${day(l.at)}` : "";
  if (st.conflict) return `Linked to ${clip(h.status?.tether_id ?? "", 32)} on its relay${at}`;
  if (l.by) return `Health linked by ${clip(l.by, 64)}, ${day(l.at)}`;
  if (rep?.linked_proof === true || h.status?.linked_proof === true) return `Health linked by Tether on that computer${at}`;
  return `Health linked by the enrolment token issued for this host${at}`;
}

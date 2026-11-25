// Advanced mode data model (docs/advanced.md#data-model). The store is the technician's own data;
// peers (the core's recent list) and org devices (team endpoint) are merged in at runtime, never saved.

import { healthOf } from "./health";

export const STORE_VERSION = 1;

export type TagColor = "gray" | "red" | "orange" | "yellow" | "green" | "teal" | "blue" | "purple" | "pink";
export const TAG_COLORS: TagColor[] = ["gray", "red", "orange", "yellow", "green", "teal", "blue", "purple", "pink"];

export interface HostEntry {
  /** Display name the technician chose (shared with the team). */
  name?: string;
  tags?: string[];
  /** Personal; never synced, exported only on request. */
  notes?: string;
  pinned?: boolean;
  /** Added by hand (address book), unix seconds. */
  added?: number;
  /** Sessions opened from this app, newest first, capped. */
  history?: { t: number; conn: string }[];
}

export interface Rule {
  id: string;
  tag: string;
  /** A query (query.ts): matching hosts get `tag` automatically. */
  when: string;
  off?: boolean;
}

export interface SavedSearch {
  id: string;
  name: string;
  query: string;
}

export interface TeamState {
  enabled: boolean;
  /** Shared document as last synced (the merge base) and its revision. */
  base?: SharedDoc | null;
  rev?: number;
  synced?: number;
  showOrg?: boolean;
  /** Health notifications (personal, never shared); on unless false. */
  healthNotify?: boolean;
}

export interface Store {
  v: number;
  hosts: Record<string, HostEntry>;
  tags: Record<string, { color?: TagColor }>;
  rules: Rule[];
  searches: SavedSearch[];
  team?: TeamState;
}

/** The part of the store a team shares (team sync, export without notes). */
export interface SharedDoc {
  v: number;
  hosts: Record<string, { name?: string; tags?: string[] }>;
  tags: Record<string, { color?: TagColor }>;
  rules: Rule[];
  searches: SavedSearch[];
}

export const emptyStore = (): Store => ({ v: STORE_VERSION, hosts: {}, tags: {}, rules: [], searches: [] });

export interface Peer {
  id: string;
  hostname: string;
  username: string;
  platform: string;
  /** Last connection (peer config mtime), unix seconds. */
  last: number;
}

export interface OrgDevice {
  uuid: string;
  id: string;
  profile: string;
  version: string;
  os: string;
  last_seen: number;
}

export type Online = "on" | "off" | "unknown";

// Device health (docs/api.md, `/api/team/status`): as the relay sends it, plus the relay it came from.
export type CheckState = "ok" | "warn" | "crit" | "unknown";
/** A row's health: `setup` = nothing failing but checks not configured; `none` = no report. */
export type HealthState = "ok" | "warn" | "crit" | "stale" | "setup" | "none" | "conflict";
export interface StatusCounts {
  ok: number;
  warn: number;
  crit: number;
  unknown: number;
  setup: number;
}
export interface StatusProduct {
  name: string;
  version?: string;
  role?: "server" | "client" | "single";
}
export interface StatusFault {
  id: string;
  state: "warn" | "crit" | "unknown";
  since?: string;
}
/** One host of the team summary. */
export interface StatusSummary {
  uuid: string;
  /** Verified: from a token issued for this ID, or confirmed by a team member. The only way a summary joins a host row. */
  tether_id: string | null;
  /** What the agent says its Tether ID is, not yet confirmed (then `tether_id` is null). */
  claimed_id?: string | null;
  /** When the verified link was made. */
  linked_at?: number | null;
  /** The relay linked it from Tether's proof on that computer (sent only when true; an older relay sends none). */
  linked_proof?: boolean;
  /** A member dismissed its link: it claims nothing until a member allows a new claim. */
  no_claim?: boolean;
  host: string;
  agent: string;
  /** The customer reference the relay holds for this device (1 to 32 of A-Za-z0-9._-); a relay that predates it sends no field. */
  customer?: string | null;
  overall: CheckState;
  counts: StatusCounts;
  products: StatusProduct[];
  faults: StatusFault[];
  /** Failing checks left out of `faults` (the summary holds at most 10); the report has them all. */
  more_faults?: number;
  last_seen: number;
  next: number;
  stale: boolean;
  /** The relay this entry came from (the newest of all relays). */
  relay: string;
  /** A software list (SBOM) is stored for this device: when (unix s) and the first 16 hex digits of its SHA-256. A relay without the feature sends none. */
  sbom?: { at: number; sha: string };
  /** The relay with the newest `sbom` of this device (not always `relay`); set by `mergeStatus`. */
  sbomRelay?: string;
}
export interface Text2 {
  de?: string;
  en?: string;
}
export interface ReportCheck {
  id: string;
  group?: string;
  state: CheckState;
  since?: string;
  value?: string;
  /** An UNKNOWN check that is only "not configured". */
  setup?: boolean;
  title: Text2;
  hint?: Text2;
}
/** `at`: when the relay stored it (history only). */
export interface StatusChange {
  id: string;
  from: CheckState;
  to: CheckState;
  time: string;
  at?: number;
}
/** `team_status_host`: the last full report (null before the first) and 90 days of history, oldest first.
 *  `last_seen`/`next` are null for an enrolled device that never posted. */
export interface StatusHostReply {
  uuid: string;
  tether_id: string | null;
  claimed_id?: string | null;
  /** When the agent enrolled (unix s). */
  enrolled?: number | null;
  /** The member who confirmed the link and when; null for a link from a token issued for that ID. */
  linked_by?: string | null;
  linked_at?: number | null;
  /** The link was made from Tether's proof on that computer (sent only when true). */
  linked_proof?: boolean;
  /** A member dismissed its link: it claims nothing until a member allows a new claim. */
  no_claim?: boolean;
  last_seen: number | null;
  next: number | null;
  stale: boolean;
  customer?: string | null;
  report:
    | (Omit<StatusSummary, "relay" | "last_seen" | "stale" | "tether_id" | "products"> & {
        tether_id?: string;
        v: 1;
        seq: number;
        time: string;
        packs: Record<string, number>;
        // A report may leave these out (the relay stores it byte-exact).
        products?: StatusProduct[];
        changes?: StatusChange[];
        body?: { checks?: ReportCheck[] };
      })
    | null;
  history: StatusChange[];
}

/** One row of the inventory. `hay` is the lowercased search text, built once per change. */
export interface Host {
  id: string;
  name: string;
  hostname: string;
  user: string;
  os: string;
  version: string;
  tags: string[];
  /** Tags from rules (not stored on the host). */
  auto: string[];
  notes: string;
  pinned: boolean;
  /** Last connection from this PC (unix s, 0 = never). */
  last: number;
  /** Last heartbeat the org saw (unix s, 0 = not an org device). */
  seen: number;
  online: Online;
  recent: boolean;
  manual: boolean;
  org: boolean;
  profile: string;
  health: HealthState;
  status?: StatusSummary;
  /** Reported by a monitor agent only: no Tether ID, so no remote access (id `m:<uuid>`). */
  agentOnly: boolean;
  /** Two devices are linked to this Tether ID on different relays: none of them joins the host
   *  row (health `conflict`), each is an agent-only row marked so. */
  conflict?: boolean;
  hay: string;
  /** `hay` without the product names, for tag rules (unset: the same as `hay`). */
  rhay?: string;
}

export function osFamily(s: string): string {
  const p = s.toLowerCase();
  if (p.includes("win")) return "Windows";
  if (p.includes("mac") || p.includes("darwin")) return "macOS";
  if (p.includes("linux") || p.includes("ubuntu") || p.includes("debian") || p.includes("fedora")) return "Linux";
  if (p.includes("android")) return "Android";
  return s ? "Other" : "";
}

/** `site:Berlin HQ` → `site:berlin-hq`; null when nothing valid is left. */
export function normTag(t: string): string | null {
  const s = t
    .trim()
    .replace(/^#/, "")
    .toLowerCase()
    .replace(/\s*:\s*/g, ":")
    .replace(/\s+/g, "-")
    .replace(/[^\p{L}\p{N}:._/-]/gu, "")
    .replace(/:{2,}/g, ":")
    .replace(/^:+|:+$/g, "");
  return s && s.length <= 64 ? s : null;
}

export const MAX_TAGS = 32;
export const MAX_NOTES = 4000;
export const MAX_NAME = 64;

export const MAX_QUERY = 1000;

// A rule, saved search or tag colour as every path takes it (typed here, imported, shared by the
// team): the same limits, and fields this version does not know stay (a newer app wrote them).
export function cleanRule(raw: unknown): Omit<Rule, "id"> | null {
  const r = raw as Partial<Rule> | null;
  const tag = r && typeof r === "object" && typeof r.tag === "string" ? normTag(r.tag) : null;
  return r && tag && typeof r.when === "string" && r.when.length <= MAX_QUERY ? { ...r, tag, when: r.when } : null;
}

export function cleanSearch(raw: unknown): Omit<SavedSearch, "id"> | null {
  const s = raw as Partial<SavedSearch> | null;
  const name = s && typeof s === "object" && typeof s.name === "string" ? s.name.trim().slice(0, MAX_NAME) : "";
  return s && name && typeof s.query === "string" ? { ...s, name, query: s.query.slice(0, MAX_QUERY) } : null;
}

export function cleanTagMeta(raw: unknown): { color?: TagColor } {
  const { color, ...rest } = (raw && typeof raw === "object" ? raw : {}) as { color?: unknown };
  return TAG_COLORS.includes(color as TagColor) ? { ...rest, color: color as TagColor } : rest;
}

export function cleanTags(tags: unknown): string[] {
  if (!Array.isArray(tags)) return [];
  const out: string[] = [];
  for (const t of tags) {
    const n = typeof t === "string" ? normTag(t) : null;
    if (n && !out.includes(n)) out.push(n);
    if (out.length >= MAX_TAGS) break;
  }
  return out;
}

/** Deterministic default colour per namespace, so `site:*` tags share one. */
export function tagColor(tag: string, meta: Store["tags"]): TagColor {
  const own = meta[tag]?.color;
  if (own) return own;
  const ns = tag.includes(":") ? tag.slice(0, tag.indexOf(":")) : tag;
  const nsColor = meta[ns]?.color;
  if (nsColor) return nsColor;
  let h = 0;
  for (let i = 0; i < ns.length; i++) h = (h * 31 + ns.charCodeAt(i)) | 0;
  return TAG_COLORS[1 + (Math.abs(h) % (TAG_COLORS.length - 1))];
}

const DAY = 86400;

/** An ID from status data that may key a row: the Tether ID rule, and never an `Object.prototype`
 *  name (`__proto__`, `constructor`), which a plain-object store would resolve to the prototype. */
export const safeId = (id: string): boolean => /^[A-Za-z0-9_.@:-]{1,64}$/.test(id) && !(id in Object.prototype);
/** An ID the store (and the team's shared document) may hold: any the technician had before (a
 *  direct IPv6 peer too), but never an agent-only row (`m:<uuid>`) or a prototype name. */
export const storeId = (id: string): boolean => !id.startsWith("m:") && !(id in Object.prototype);
const own = (store: Store, id: string): HostEntry | undefined => (Object.hasOwn(store.hosts, id) ? store.hosts[id] : undefined);

/** All hosts: store entries ∪ recent peers ∪ org devices (if shown) ∪ health summaries, one row per ID.
 *  A summary joins its Tether ID's row; one without a (verified) Tether ID gets its own row `m:<uuid>`,
 *  also when it claims one: only a confirmed `tether_id` joins (`claimed_id` never does). */
export function buildHosts(store: Store, peers: Peer[], org: OrgDevice[], online: Map<string, boolean> | null, now: number, status: StatusSummary[] = []): Host[] {
  const byId = new Map<string, Host>();
  const get = (id: string): Host => {
    let h = byId.get(id);
    if (!h) {
      const e = own(store, id);
      h = {
        id,
        name: e?.name ?? "",
        hostname: "",
        user: "",
        os: "",
        version: "",
        tags: e?.tags ?? [],
        auto: [],
        notes: e?.notes ?? "",
        pinned: !!e?.pinned,
        last: e?.history?.[0]?.t ?? 0,
        seen: 0,
        online: "unknown",
        recent: false,
        manual: !!e?.added,
        org: false,
        profile: "",
        health: "none",
        agentOnly: false,
        hay: "",
      };
      byId.set(id, h);
    }
    return h;
  };
  for (const id of Object.keys(store.hosts)) {
    const e = store.hosts[id];
    // An entry with only history of a removed peer still counts: the technician kept it.
    if (e.added || e.tags?.length || e.name || e.notes || e.pinned) get(id);
  }
  for (const p of peers) {
    const h = get(p.id);
    h.hostname = p.hostname;
    h.user = p.username;
    h.os = h.os || p.platform;
    h.last = Math.max(h.last, p.last);
    h.recent = true;
  }
  for (const d of org) {
    const h = get(d.id);
    h.org = true;
    h.profile = d.profile;
    h.version = d.version;
    if (d.os) h.os = d.os;
    h.seen = Math.max(h.seen, d.last_seen);
  }
  // A verified ID that could collide with a synthetic `m:` key or a prototype name is ignored.
  const tidOf = (s: StatusSummary) => (s.tether_id && safeId(s.tether_id) && !s.tether_id.startsWith("m:") ? s.tether_id : null);
  // Summaries are one per uuid: two for one ID are two devices linked to it (on different relays).
  const linked = new Map<string, number>();
  for (const s of status) {
    const t = tidOf(s);
    if (t) linked.set(t, (linked.get(t) ?? 0) + 1);
  }
  for (const s of status) {
    let tid = tidOf(s);
    if (tid && linked.get(tid)! > 1) {
      // Nothing joins the host; each device is a row of its own, marked as conflicting.
      get(tid).conflict = true;
      tid = null;
      get(`m:${s.uuid}`).conflict = true;
    }
    const h = get(tid ?? `m:${s.uuid}`);
    // Two agents naming one Tether ID (different relays): the newest report wins.
    if (h.status && h.status.last_seen >= s.last_seen) continue;
    h.status = s;
    h.agentOnly = !tid;
    h.hostname ||= s.host;
  }
  const out = [...byId.values()];
  for (const h of out) {
    h.health = h.conflict && !h.status ? "conflict" : healthOf(h.status);
    h.recent = h.recent && now - h.last < 30 * DAY;
    if (online) h.online = online.get(h.id) ? "on" : online.has(h.id) ? "off" : "unknown";
  }
  return out;
}

/** Display name: chosen name, else hostname, else user, else the ID. */
export const label = (h: Pick<Host, "name" | "hostname" | "user" | "id">) => h.name || h.hostname || h.user || h.id;

export function indexHost(h: Host) {
  const base = [h.id, h.name, h.hostname, h.user, h.os, osFamily(h.os), h.version, ...h.tags, ...h.auto, h.notes].join("\u0001").toLowerCase();
  const products = h.status?.products.map((p) => p.name) ?? [];
  // Tag rules read the text they read before health data existed; the search box also product names.
  h.rhay = products.length ? base : undefined;
  h.hay = products.length ? base + "\u0001" + products.join("\u0001").toLowerCase() : base;
}

export function shared(store: Store): SharedDoc {
  const hosts: SharedDoc["hosts"] = {};
  for (const [id, e] of Object.entries(store.hosts)) {
    if ((e.name || e.tags?.length) && !id.startsWith("m:")) hosts[id] = { ...(e.name ? { name: e.name } : {}), ...(e.tags?.length ? { tags: e.tags } : {}) };
  }
  return { v: STORE_VERSION, hosts, tags: store.tags, rules: store.rules, searches: store.searches };
}

/** Put a merged shared doc back into the store, keeping personal fields. */
export function applyShared(store: Store, doc: SharedDoc): Store {
  const hosts: Store["hosts"] = {};
  for (const [id, e] of Object.entries(store.hosts)) {
    const { name: _n, tags: _t, ...personal } = e;
    if (Object.keys(personal).length) hosts[id] = personal;
  }
  for (const [id, e] of Object.entries(doc.hosts)) {
    hosts[id] = { ...hosts[id], ...(e.name ? { name: e.name } : {}), ...(e.tags?.length ? { tags: e.tags } : {}) };
  }
  return { ...store, hosts, tags: doc.tags, rules: doc.rules, searches: doc.searches };
}

export function newId(): string {
  return Math.random().toString(36).slice(2, 10);
}

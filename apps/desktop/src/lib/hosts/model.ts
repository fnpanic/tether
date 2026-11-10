// Advanced mode data model (docs/advanced.md#data-model). The store is the technician's own data;
// peers (the core's recent list) and org devices (team endpoint) are merged in at runtime, never saved.

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
  hay: string;
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

/** All hosts: store entries ∪ recent peers ∪ org devices (if shown), one row per ID. */
export function buildHosts(store: Store, peers: Peer[], org: OrgDevice[], online: Map<string, boolean> | null, now: number): Host[] {
  const byId = new Map<string, Host>();
  const get = (id: string): Host => {
    let h = byId.get(id);
    if (!h) {
      const e = store.hosts[id];
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
  const out = [...byId.values()];
  for (const h of out) {
    h.recent = h.recent && now - h.last < 30 * DAY;
    if (online) h.online = online.get(h.id) ? "on" : online.has(h.id) ? "off" : "unknown";
  }
  return out;
}

/** Display name: chosen name, else hostname, else user, else the ID. */
export const label = (h: Pick<Host, "name" | "hostname" | "user" | "id">) => h.name || h.hostname || h.user || h.id;

export function indexHost(h: Host) {
  h.hay = [h.id, h.name, h.hostname, h.user, h.os, osFamily(h.os), h.version, ...h.tags, ...h.auto, h.notes].join("\u0001").toLowerCase();
}

export function shared(store: Store): SharedDoc {
  const hosts: SharedDoc["hosts"] = {};
  for (const [id, e] of Object.entries(store.hosts)) {
    if (e.name || e.tags?.length) hosts[id] = { ...(e.name ? { name: e.name } : {}), ...(e.tags?.length ? { tags: e.tags } : {}) };
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

// Team sync: three-way merge of the shared doc (docs/advanced.md#team-sync). base = what this app
// last synced, local = now, remote = what the relay has. Changes on both sides survive; for the same
// field changed on both sides the local edit wins (the user who saves last sees the result at once).
import { cleanTags, STORE_VERSION, type Rule, type SavedSearch, type SharedDoc } from "./model";

const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

/** Scalar: the side that changed wins; both changed → local. */
function pick<T>(base: T, local: T, remote: T): T {
  return same(local, base) ? remote : local;
}

/** Sets: remote + what local added − what local removed. */
export function mergeSet(base: string[] = [], local: string[] = [], remote: string[] = []): string[] {
  const added = local.filter((t) => !base.includes(t));
  const removed = new Set(base.filter((t) => !local.includes(t)));
  return cleanTags([...remote.filter((t) => !removed.has(t)), ...added]);
}

/** Maps keyed by id; an entry deleted on one side and unchanged on the other is deleted. */
function mergeMap<V>(base: Record<string, V>, local: Record<string, V>, remote: Record<string, V>, value: (b: V | undefined, l: V | undefined, r: V | undefined) => V | undefined): Record<string, V> {
  const out: Record<string, V> = {};
  for (const k of new Set([...Object.keys(base), ...Object.keys(local), ...Object.keys(remote)])) {
    const b = base[k];
    const l = local[k];
    const r = remote[k];
    let v: V | undefined;
    if (l === undefined && b !== undefined) v = same(r, b) ? undefined : r; // deleted here
    else if (r === undefined && b !== undefined) v = same(l, b) ? undefined : l; // deleted there
    else v = value(b, l, r);
    if (v !== undefined) out[k] = v;
  }
  return out;
}

const byId = <T extends { id: string }>(a: T[] = []) => Object.fromEntries(a.map((x) => [x.id, x]));

export function merge3(base: SharedDoc | null | undefined, local: SharedDoc, remote: SharedDoc | null | undefined): SharedDoc {
  const b: SharedDoc = base ?? { v: STORE_VERSION, hosts: {}, tags: {}, rules: [], searches: [] };
  const r: SharedDoc = remote ?? b;
  const hosts = mergeMap(b.hosts ?? {}, local.hosts ?? {}, r.hosts ?? {}, (bh, lh, rh) => {
    const name = pick(bh?.name, lh?.name, rh?.name);
    const tags = mergeSet(bh?.tags, lh?.tags, rh?.tags);
    if (!name && !tags.length) return undefined;
    return { ...(name ? { name } : {}), ...(tags.length ? { tags } : {}) };
  });
  const tags = mergeMap(b.tags ?? {}, local.tags ?? {}, r.tags ?? {}, (bt, lt, rt) => pick(bt, lt, rt) ?? lt ?? rt);
  const order = <T extends { id: string }>(l: T[], rr: T[], m: Record<string, T>) => {
    const ids = [...l.map((x) => x.id), ...rr.map((x) => x.id).filter((id) => !l.some((x) => x.id === id))];
    return ids.filter((id) => m[id]).map((id) => m[id]);
  };
  const rules = mergeMap<Rule>(byId(b.rules), byId(local.rules), byId(r.rules), (bb, l, rr) => pick(bb, l, rr) ?? l ?? rr);
  const searches = mergeMap<SavedSearch>(byId(b.searches), byId(local.searches), byId(r.searches), (bb, l, rr) => pick(bb, l, rr) ?? l ?? rr);
  return { v: STORE_VERSION, hosts, tags, rules: order(local.rules ?? [], r.rules ?? [], rules), searches: order(local.searches ?? [], r.searches ?? [], searches) };
}

/** A doc from the relay, reduced to what this version understands (it may come from a newer app). */
export function readShared(raw: unknown): SharedDoc | null {
  if (!raw || typeof raw !== "object") return null;
  const d = raw as Partial<SharedDoc>;
  const hosts: SharedDoc["hosts"] = {};
  for (const [id, e] of Object.entries(d.hosts ?? {})) {
    if (!/^[A-Za-z0-9_.@:-]{1,64}$/.test(id) || !e || typeof e !== "object") continue;
    const tags = cleanTags(e.tags);
    const name = typeof e.name === "string" ? e.name.slice(0, 64) : "";
    if (name || tags.length) hosts[id] = { ...(name ? { name } : {}), ...(tags.length ? { tags } : {}) };
  }
  const arr = <T,>(x: unknown, ok: (v: T) => boolean): T[] => (Array.isArray(x) ? (x as T[]).filter((v) => v && typeof v === "object" && ok(v)) : []);
  return {
    v: STORE_VERSION,
    hosts,
    tags: d.tags && typeof d.tags === "object" ? d.tags : {},
    rules: arr<Rule>(d.rules, (r) => typeof r.id === "string" && typeof r.tag === "string" && typeof r.when === "string"),
    searches: arr<SavedSearch>(d.searches, (s) => typeof s.id === "string" && typeof s.name === "string" && typeof s.query === "string"),
  };
}

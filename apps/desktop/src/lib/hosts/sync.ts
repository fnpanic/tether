// Team sync: three-way merge of the shared doc (docs/advanced.md#team-sync). base = what this app
// last synced, local = now, remote = what the relay has. Changes on both sides survive; for the same
// field changed on both sides the local edit wins (the user who saves last sees the result at once).
// The merged doc lists keys and entries in the relay's order, then what is new here: a merge that
// changes nothing is then byte-identical to the relay's doc, and no client writes it back.
import { cleanRule, cleanSearch, cleanTagMeta, cleanTags, normTag, STORE_VERSION, type Rule, type SavedSearch, type SharedDoc } from "./model";

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
  for (const k of new Set([...Object.keys(remote), ...Object.keys(local), ...Object.keys(base)])) {
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
  const order = <T extends { id: string }>(first: T[], then: T[], m: Record<string, T>) => [...new Set([...first, ...then].map((x) => x.id))].filter((id) => m[id]).map((id) => m[id]);
  const rules = mergeMap<Rule>(byId(b.rules), byId(local.rules), byId(r.rules), (bb, l, rr) => pick(bb, l, rr) ?? l ?? rr);
  const searches = mergeMap<SavedSearch>(byId(b.searches), byId(local.searches), byId(r.searches), (bb, l, rr) => pick(bb, l, rr) ?? l ?? rr);
  return { v: STORE_VERSION, hosts, tags, rules: order(r.rules ?? [], local.rules ?? [], rules), searches: order(r.searches ?? [], local.searches ?? [], searches) };
}

const MAX_LIST = 500;
const MAX_TAG_META = 5000;
// `__proto__` as a key would set the prototype of the map it is put in.
const okId = (x: unknown): x is string => typeof x === "string" && /^[A-Za-z0-9_.@:-]{1,64}$/.test(x) && x !== "__proto__";

/** A doc from the relay, reduced to what this version understands (it may come from a newer app)
 *  and to the limits an import has (io.ts): every team member renders what one member wrote. */
export function readShared(raw: unknown): SharedDoc | null {
  if (!raw || typeof raw !== "object") return null;
  const d = raw as Partial<SharedDoc>;
  const hosts: SharedDoc["hosts"] = {};
  for (const [id, e] of Object.entries(d.hosts ?? {})) {
    // Agent-only rows (`m:<uuid>`) are never shared; one an earlier version wrote is dropped.
    if (!okId(id) || id.startsWith("m:") || !e || typeof e !== "object") continue;
    const tags = cleanTags(e.tags);
    const name = typeof e.name === "string" ? e.name.slice(0, 64) : "";
    if (name || tags.length) hosts[id] = { ...(name ? { name } : {}), ...(tags.length ? { tags } : {}) };
  }
  const tags: SharedDoc["tags"] = {};
  for (const [k, v] of Object.entries(d.tags && typeof d.tags === "object" ? d.tags : {}).slice(0, MAX_TAG_META)) {
    const n = normTag(k);
    if (n && n !== "__proto__") tags[n] = cleanTagMeta(v);
  }
  // The entry's own limits are the import's and the dialogs' (model.ts); here also a valid id.
  const list = <T extends { id: string }>(x: unknown, clean: (v: unknown) => Omit<T, "id"> | null): T[] => {
    const out: T[] = [];
    for (const v of Array.isArray(x) ? x : []) {
      const r = clean(v);
      const id = (v as { id?: unknown } | null)?.id;
      if (r && okId(id) && out.push({ ...r, id } as T) >= MAX_LIST) break;
    }
    return out;
  };
  return {
    v: STORE_VERSION,
    hosts,
    tags,
    rules: list<Rule>(d.rules, cleanRule),
    searches: list<SavedSearch>(d.searches, cleanSearch),
  };
}

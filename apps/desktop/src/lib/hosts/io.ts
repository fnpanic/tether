// Import/export of the host store (docs/advanced.md#import-and-export). Exports leave out notes
// unless asked (they may hold passwords or customer details); nothing else in the store is secret.
import { cleanRule, cleanSearch, cleanTagMeta, cleanTags, label, MAX_NAME, MAX_NOTES, newId, normTag, STORE_VERSION, type Host, type HostEntry, type Store } from "./model";

export interface ExportOpts {
  notes: boolean;
  /** Only these IDs (a selection); all when absent. */
  ids?: string[];
  /** The host rows: adds their health fields (export only; an import ignores them). */
  hosts?: Map<string, Host>;
}

const iso = (t: number) => new Date(t * 1000).toISOString();
const products = (h: Host) => (h.status?.products ?? []).map((p) => [p.name, p.version, p.role && `(${p.role})`].filter(Boolean).join(" ")).join("; ");

function pick(store: Store, o: ExportOpts): [string, HostEntry][] {
  return o.ids ? o.ids.map((id) => [id, (Object.hasOwn(store.hosts, id) && store.hosts[id]) || {}]) : Object.entries(store.hosts);
}

export function exportJson(store: Store, o: ExportOpts): string {
  const hosts: Record<string, HostEntry> = {};
  for (const [id, e] of pick(store, o)) {
    const out: HostEntry = {};
    if (e.name) out.name = e.name;
    if (e.tags?.length) out.tags = e.tags;
    if (e.pinned) out.pinned = true;
    if (e.added) out.added = e.added;
    if (o.notes && e.notes) out.notes = e.notes;
    const st = o.hosts?.get(id)?.status;
    const health = st && { state: o.hosts!.get(id)!.health, counts: st.counts, products: st.products, agent: st.agent, customer: st.customer ?? undefined, reported: iso(st.last_seen), faults: st.faults };
    if (Object.keys(out).length || o.ids) hosts[id] = health ? { ...out, health } as HostEntry : out;
  }
  return JSON.stringify({ format: "tether-hosts", v: STORE_VERSION, hosts, tags: store.tags, rules: store.rules, searches: store.searches }, null, 2);
}

/** A cell a spreadsheet would run as a formula gets a leading `'` (as Tether Manage's CSV). */
function cell(s: string): string {
  const v = /^[=+\-@\t\r]/.test(s) ? `'${s}` : s;
  return /[",\r\n;]/.test(v) ? `"${v.replace(/"/g, '""')}"` : v;
}

export function exportCsv(store: Store, o: ExportOpts): string {
  const cols = ["id", "name", "tags", "pinned", ...(o.notes ? ["notes"] : []), ...(o.hosts ? ["health", "crit", "warn", "unknown", "setup", "products", "agent", "customer", "reported"] : [])];
  const rows = [cols.join(",")];
  for (const [id, e] of pick(store, o)) {
    const r = [id, e.name ?? "", (e.tags ?? []).join(" "), e.pinned ? "yes" : ""];
    if (o.notes) r.push(e.notes ?? "");
    if (o.hosts) {
      const h = o.hosts.get(id);
      const st = h?.status;
      r.push(h?.health ?? "", ...(st ? [st.counts.crit, st.counts.warn, st.counts.unknown, st.counts.setup].map(String) : ["", "", "", ""]), h ? products(h) : "", st?.agent ?? "", st?.customer ?? "", st ? iso(st.last_seen) : "");
    }
    rows.push(r.map(cell).join(","));
  }
  return rows.join("\r\n") + "\r\n";
}

/** One row per failing check of the given hosts (the summary's `faults`). */
export function exportFaultsCsv(hosts: Host[]): string {
  const rows = ["id,name,uuid,check,state,since"];
  for (const h of hosts) for (const f of h.status?.faults ?? []) rows.push([h.id, label(h), h.status!.uuid, f.id, f.state, f.since ?? ""].map(cell).join(","));
  return rows.join("\r\n") + "\r\n";
}

/** RFC 4180 CSV; also `;`-separated (German Excel). */
export function parseCsv(text: string): string[][] {
  const first = text.split(/\r?\n/, 1)[0] ?? "";
  const sep = (first.match(/;/g)?.length ?? 0) > (first.match(/,/g)?.length ?? 0) ? ";" : ",";
  const rows: string[][] = [];
  let row: string[] = [];
  let f = "";
  let q = false;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (q) {
      if (c === '"' && text[i + 1] === '"') {
        f += '"';
        i++;
      } else if (c === '"') q = false;
      else f += c;
    } else if (c === '"' && f === "") q = true;
    else if (c === sep) {
      row.push(f);
      f = "";
    } else if (c === "\n" || c === "\r") {
      if (c === "\r" && text[i + 1] === "\n") i++;
      row.push(f);
      if (row.some((x) => x !== "")) rows.push(row);
      row = [];
      f = "";
    } else f += c;
  }
  row.push(f);
  if (row.some((x) => x !== "")) rows.push(row);
  return rows;
}

export interface Imported {
  hosts: Record<string, HostEntry>;
  tags: Store["tags"];
  rules: Store["rules"];
  searches: Store["searches"];
  /** Rows / entries that were skipped, with the reason. */
  skipped: string[];
}

const ID_RE = /^[A-Za-z0-9_.@:-]{1,64}$/;
const unq = (s: string) => s.replace(/^'(?=[=+\-@])/, "");

function entry(raw: Record<string, unknown>): HostEntry {
  const e: HostEntry = {};
  if (typeof raw.name === "string" && raw.name.trim()) e.name = raw.name.trim().slice(0, MAX_NAME);
  const tags = cleanTags(typeof raw.tags === "string" ? raw.tags.split(/[\s,;|]+/) : raw.tags);
  if (tags.length) e.tags = tags;
  if (raw.pinned === true || raw.pinned === "yes" || raw.pinned === "true" || raw.pinned === "1") e.pinned = true;
  if (typeof raw.notes === "string" && raw.notes) e.notes = raw.notes.slice(0, MAX_NOTES);
  if (typeof raw.added === "number" && raw.added > 0) e.added = Math.floor(raw.added);
  return e;
}

/** JSON (our export, or `[{id, name, tags}]`) or CSV with a header row containing `id`. */
export function parseImport(text: string): Imported {
  const out: Imported = { hosts: {}, tags: {}, rules: [], searches: [], skipped: [] };
  const t = text.replace(/^﻿/, "").trim();
  if (t.length > 16 * 1024 * 1024) throw new Error("The file is too large (16 MB).");
  if (t.startsWith("{") || t.startsWith("[")) {
    let doc: unknown;
    try {
      doc = JSON.parse(t);
    } catch {
      throw new Error("The file is not valid JSON.");
    }
    const list: [string, Record<string, unknown>][] = Array.isArray(doc)
      ? doc.filter((x) => x && typeof x === "object").map((x) => [String((x as { id?: unknown }).id ?? ""), x as Record<string, unknown>])
      : Object.entries(((doc as { hosts?: unknown }).hosts ?? {}) as Record<string, Record<string, unknown>>);
    for (const [rawId, raw] of list) {
      const id = rawId.replace(/\s/g, "");
      if (!ID_RE.test(id) || typeof raw !== "object" || !raw) {
        out.skipped.push(`invalid ID "${rawId.slice(0, 40)}"`);
        continue;
      }
      out.hosts[id] = entry(raw);
    }
    if (!Array.isArray(doc)) {
      const d = doc as Partial<Store>;
      for (const [k, v] of Object.entries(d.tags ?? {})) {
        const n = normTag(k);
        if (n && n !== "__proto__") out.tags[n] = cleanTagMeta(v);
      }
      for (const raw of Array.isArray(d.rules) ? d.rules : []) {
        const r = cleanRule(raw);
        if (r) out.rules.push({ ...r, id: newId() });
      }
      for (const raw of Array.isArray(d.searches) ? d.searches : []) {
        const s = cleanSearch(raw);
        if (s) out.searches.push({ ...s, id: newId() });
      }
    }
    return out;
  }
  const rows = parseCsv(t);
  const head = (rows.shift() ?? []).map((h) => h.trim().toLowerCase());
  const col = (n: string) => head.indexOf(n);
  const idc = col("id") >= 0 ? col("id") : col("tether id");
  if (idc < 0) throw new Error('The CSV needs a header row with an "id" column.');
  rows.forEach((r, i) => {
    const id = (r[idc] ?? "").replace(/\s/g, "");
    if (!ID_RE.test(id)) {
      out.skipped.push(`row ${i + 2}: invalid ID "${id.slice(0, 40)}"`);
      return;
    }
    const get = (n: string) => (col(n) >= 0 ? unq(r[col(n)] ?? "") : undefined);
    out.hosts[id] = entry({ name: get("name"), tags: get("tags"), pinned: get("pinned"), notes: get("notes") });
  });
  return out;
}

/** Merge an import into the store: tags are added, a name/notes in the file replace the old one. */
export function mergeImport(store: Store, imp: Imported, now = Math.floor(Date.now() / 1000)): Store {
  const hosts = { ...store.hosts };
  for (const [id, e] of Object.entries(imp.hosts)) {
    const old = hosts[id] ?? {};
    const tags = cleanTags([...(old.tags ?? []), ...(e.tags ?? [])]);
    hosts[id] = { ...old, ...e, ...(tags.length ? { tags } : {}), added: old.added ?? e.added ?? now };
  }
  const known = new Set(store.rules.map((r) => `${r.tag}\u0001${r.when}`));
  const ksearch = new Set(store.searches.map((s) => s.name));
  return {
    ...store,
    hosts,
    tags: { ...store.tags, ...imp.tags },
    rules: [...store.rules, ...imp.rules.filter((r) => !known.has(`${r.tag}\u0001${r.when}`))],
    searches: [...store.searches, ...imp.searches.filter((s) => !ksearch.has(s.name))],
  };
}

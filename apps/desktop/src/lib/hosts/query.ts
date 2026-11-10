// Host query language (docs/advanced.md#query-syntax): fuzzy words, #tags, field filters,
// AND (space) / OR (|) / NOT (- or !), parentheses. Never throws: a broken query still filters by
// what could be read, and `error` says what was ignored.
import { osFamily, type Host } from "./model";

export type Node =
  | { k: "and" | "or"; c: Node[] }
  | { k: "not"; c: Node }
  | { k: "word"; v: string }
  | { k: "tag"; v: string; prefix: boolean }
  | { k: "field"; f: string; op: string; v: string };

export interface Parsed {
  node: Node | null;
  /** Free-text words, for ranking and highlighting. */
  words: string[];
  error?: string;
}

const FIELDS = new Set(["id", "name", "host", "os", "ver", "user", "note", "tag", "is", "has", "seen", "src"]);

type Tok = { t: "(" | ")" | "or" | "and" | "not" } | { t: "term"; v: string; quoted: boolean };

function tokenize(q: string): Tok[] {
  const out: Tok[] = [];
  let i = 0;
  while (i < q.length) {
    const c = q[i];
    if (/\s/.test(c)) {
      i++;
    } else if (c === "(" || c === ")") {
      out.push({ t: c });
      i++;
    } else if (c === "|") {
      out.push({ t: "or" });
      i++;
    } else if (c === "&") {
      out.push({ t: "and" });
      i++;
    } else if ((c === "-" || c === "!") && i + 1 < q.length && !/[\s)]/.test(q[i + 1])) {
      out.push({ t: "not" });
      i++;
    } else {
      // A term; a quoted part (also after `field:`) may contain spaces.
      let v = "";
      let quoted = false;
      while (i < q.length && !/[\s()|]/.test(q[i])) {
        if (q[i] === '"') {
          quoted = true;
          const end = q.indexOf('"', i + 1);
          v += q.slice(i + 1, end < 0 ? q.length : end);
          i = end < 0 ? q.length : end + 1;
        } else v += q[i++];
      }
      if (!quoted && v === "OR") out.push({ t: "or" });
      else if (!quoted && v === "AND") out.push({ t: "and" });
      else if (!quoted && v === "NOT") out.push({ t: "not" });
      else if (v) out.push({ t: "term", v, quoted });
    }
  }
  return out;
}

function term(v: string, quoted: boolean): Node {
  if (!quoted || !v.includes(" ") || v.includes(":")) {
    if (v.startsWith("#") && v.length > 1) {
      const prefix = v.endsWith("*");
      return { k: "tag", v: v.slice(1, prefix ? -1 : undefined).toLowerCase(), prefix };
    }
    const m = /^([a-z]+):(>=|<=|>|<|=)?(.*)$/i.exec(v);
    if (m && FIELDS.has(m[1].toLowerCase()) && m[3]) {
      const f = m[1].toLowerCase();
      if (f === "tag") {
        const prefix = m[3].endsWith("*");
        return { k: "tag", v: m[3].slice(0, prefix ? -1 : undefined).toLowerCase(), prefix };
      }
      return { k: "field", f, op: m[2] ?? "", v: m[3].toLowerCase() };
    }
  }
  return { k: "word", v: v.toLowerCase() };
}

/** Brackets and NOTs nest by recursion; past this depth the rest of the nesting is ignored (a
 *  query from a shared team doc must not overflow the stack while the page renders). */
const MAX_DEPTH = 200;

export function parse(q: string): Parsed {
  const toks = tokenize(q);
  let pos = 0;
  let depth = 0;
  let error: string | undefined;
  const words: string[] = [];

  const or = (): Node | null => {
    const parts: Node[] = [];
    const a = and();
    if (a) parts.push(a);
    while (toks[pos]?.t === "or") {
      pos++;
      const b = and();
      if (b) parts.push(b);
      else error ??= "Nothing after OR";
    }
    return parts.length === 0 ? null : parts.length === 1 ? parts[0] : { k: "or", c: parts };
  };
  const and = (): Node | null => {
    const parts: Node[] = [];
    for (;;) {
      const t = toks[pos];
      if (!t || t.t === ")" || t.t === "or") break;
      if (t.t === "and") {
        pos++;
        continue;
      }
      const u = unary();
      if (u) parts.push(u);
    }
    return parts.length === 0 ? null : parts.length === 1 ? parts[0] : { k: "and", c: parts };
  };
  const unary = (): Node | null => {
    const t = toks[pos];
    if ((t?.t === "not" || t?.t === "(") && depth >= MAX_DEPTH) {
      error ??= "Too many nested brackets";
      pos++;
      return null;
    }
    if (t?.t === "not") {
      pos++;
      depth++;
      const u = unary();
      depth--;
      if (!u) error ??= "Nothing after NOT";
      return u ? { k: "not", c: u } : null;
    }
    if (t?.t === "(") {
      pos++;
      depth++;
      const e = or();
      depth--;
      if (toks[pos]?.t === ")") pos++;
      else error ??= "Missing )";
      return e;
    }
    pos++;
    if (t?.t !== "term") return null;
    const n = term(t.v, t.quoted);
    if (n.k === "word") words.push(n.v);
    return n;
  };

  let node = or();
  while (pos < toks.length) {
    // A stray `)`: skip it and keep reading, AND-ing the rest.
    error ??= "Unmatched )";
    pos++;
    const rest = or();
    if (rest) node = node ? { k: "and", c: [node, rest] } : rest;
  }
  return { node, words, error };
}

/** Subsequence match of `w` in `s`; 0 = no match, higher = tighter / earlier / on a word start. */
export function fuzzy(s: string, w: string): number {
  const at = s.indexOf(w);
  if (at >= 0) {
    const start = at === 0 || /[\u0001\s:._/-]/.test(s[at - 1]);
    return 1000 + (start ? 500 : 0) - Math.min(at, 400);
  }
  // Letters in order within one field (fields are joined by \u0001); a match that would cross a
  // field restarts in the next field.
  let from = 0;
  outer: while (from < s.length) {
    let si = from;
    let gaps = 0;
    let prev = -1;
    for (let wi = 0; wi < w.length; wi++) {
      const c = w.charCodeAt(wi);
      while (si < s.length && s.charCodeAt(si) !== c) {
        if (prev >= 0 && s.charCodeAt(si) === 1) {
          from = si + 1;
          continue outer;
        }
        si++;
      }
      if (si >= s.length) return 0;
      if (prev >= 0) gaps += si - prev - 1;
      prev = si++;
    }
    if (gaps <= w.length * 3) return Math.max(1, 400 - gaps * 20);
    const sep = s.indexOf("\u0001", from);
    if (sep < 0) return 0;
    from = sep + 1;
  }
  return 0;
}

const UNIT: Record<string, number> = { m: 60, h: 3600, d: 86400, w: 7 * 86400 };

function age(v: string): number | null {
  const m = /^(\d+)([mhdw])?$/.exec(v);
  return m ? Number(m[1]) * UNIT[m[2] ?? "d"] : null;
}

export function cmpVersion(a: string, b: string): number {
  const pa = a.split(/[.-]/).map((x) => parseInt(x, 10) || 0);
  const pb = b.split(/[.-]/).map((x) => parseInt(x, 10) || 0);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const d = (pa[i] ?? 0) - (pb[i] ?? 0);
    if (d) return d;
  }
  return 0;
}

function hasTag(h: Host, v: string, prefix: boolean): boolean {
  const all = h.auto.length ? h.tags.concat(h.auto) : h.tags;
  for (const t of all) {
    if (prefix ? t.startsWith(v) : t === v || t.startsWith(v + ":")) return true;
  }
  return false;
}

function field(h: Host, f: string, op: string, v: string, now: number): boolean {
  switch (f) {
    case "id":
      return h.id.includes(v.replace(/\s/g, ""));
    case "name":
      return (h.name || h.hostname).toLowerCase().includes(v);
    case "host":
      return h.hostname.toLowerCase().includes(v);
    case "user":
      return h.user.toLowerCase().includes(v);
    case "note":
      return h.notes.toLowerCase().includes(v);
    case "os":
      return h.os.toLowerCase().includes(v) || osFamily(h.os).toLowerCase().startsWith(v);
    case "ver": {
      if (!h.version) return false;
      if (!op) return h.version.startsWith(v);
      const c = cmpVersion(h.version, v);
      return op === ">" ? c > 0 : op === ">=" ? c >= 0 : op === "<" ? c < 0 : op === "<=" ? c <= 0 : c === 0;
    }
    case "is":
      return v === "online" ? h.online === "on" : v === "offline" ? h.online === "off" : v === "pinned" ? h.pinned : v === "org" ? h.org : v === "recent" ? h.recent : v === "manual" ? h.manual : false;
    case "has":
      return v === "notes" ? !!h.notes : v === "tags" ? h.tags.length + h.auto.length > 0 : v === "name" ? !!h.name : false;
    case "src":
      return v === "org" ? h.org : v === "recent" ? h.last > 0 : v === "manual" ? h.manual : false;
    case "seen": {
      const s = age(v);
      const last = Math.max(h.last, h.seen);
      if (s === null || !last) return false;
      const ago = now - last;
      return op === ">" || op === ">=" ? ago >= s : ago <= s;
    }
  }
  return false;
}

export function matches(n: Node, h: Host, now: number): boolean {
  switch (n.k) {
    case "and":
      return n.c.every((c) => matches(c, h, now));
    case "or":
      return n.c.some((c) => matches(c, h, now));
    case "not":
      return !matches(n.c, h, now);
    case "word":
      return fuzzy(h.hay, n.v) > 0;
    case "tag":
      return hasTag(h, n.v, n.prefix);
    case "field":
      return field(h, n.f, n.op, n.v, now);
  }
}

/** Hosts matching `q`; with free-text words, best match first (else input order). */
export function search(hosts: Host[], q: string | Parsed, now = Date.now() / 1000): Host[] {
  const p = typeof q === "string" ? parse(q) : q;
  if (!p.node) return hosts;
  const node = p.node;
  if (!p.words.length) return hosts.filter((h) => matches(node, h, now));
  const scored: { h: Host; s: number }[] = [];
  for (const h of hosts) {
    if (!matches(node, h, now)) continue;
    let s = 0;
    for (const w of p.words) s += fuzzy(h.hay, w);
    scored.push({ h, s: s + (h.pinned ? 50 : 0) + (h.online === "on" ? 20 : 0) });
  }
  scored.sort((a, b) => b.s - a.s);
  return scored.map((x) => x.h);
}

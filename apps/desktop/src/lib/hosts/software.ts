// The `software:` search term (spec 10.4): every relay is asked (`team_status_software`) which hosts
// have a matching program in their last uploaded software list; a host counts when the relay that
// holds its newest list (`sbomRelay`, by `sbom.at`) names it. Each answer is kept 60 s.
import { invoke } from "../ipc";
import type { Host } from "./model";
import { parse, type Node, type SoftwareField, type SoftwareLookup } from "./query";

export type Call = (cmd: string, args: Record<string, unknown>) => Promise<unknown>;
export type SwTerm = { q: string; v: string };
type Hit = { name: string; version: string };
export type Answer =
  | { ok: true; hits: Map<string, Hit>; more: number; pending: number; partial: boolean; at: number }
  | { ok: false; why: Why; text: string; retry?: number };
/** busy: the relay runs another search (relay-wide); limit: this member's 6 searches a minute;
 *  reportoff / unavailable: the relay's 503 (health reporting off / its status data not ready). */
type Why = "unsupported" | "off" | "forbidden" | "busy" | "limit" | "reportoff" | "unavailable" | "offline" | "error";
/** Per term (`termKey`), per relay. */
export type Results = Map<string, Map<string, Answer>>;

export const termKey = (t: SwTerm) => JSON.stringify([t.q, t.v]);
export const termsKey = (ts: SwTerm[]) => ts.map(termKey).join("\n");
// Trimmed as the relay trims them, so the lengths are counted as it counts them.
const termOf = (n: SoftwareField): SwTerm => ({ q: n.v.trim(), v: (n.at ?? "").trim() });

/** The query's software terms, each distinct (name, version) once, in order. */
export function softwareTerms(n: Node | null): SwTerm[] {
  const out = new Map<string, SwTerm>();
  const walk = (x: Node | null): void => {
    if (!x) return;
    if (x.k === "and" || x.k === "or") x.c.forEach(walk);
    else if (x.k === "not") walk(x.c);
    else if (x.k === "field" && x.f === "software") out.set(termKey(termOf(x)), termOf(x));
  };
  walk(n);
  return [...out.values()];
}

/** What the relay accepts (docs/api.md): 2 to 64 characters, a version prefix of at most 32. */
const tooShort = (t: SwTerm) => [...t.q].length < 2;
const tooLong = (t: SwTerm) => [...t.q].length > 64 || [...t.v].length > 32;
/** Characters the relay refuses: control, zero-width, line separators and direction marks. */
const hidden = (t: SwTerm) => /[\p{Cc}\u200B-\u200F\u2028-\u202E\u2060-\u2069\uFEFF]/u.test(t.q + t.v);
export const runnable = (ts: SwTerm[]) => ts.filter((t) => !tooShort(t) && !tooLong(t) && !hidden(t));

const TTL = 60_000;
const cache = new Map<string, { at: number; a: Answer }>();
const inflight = new Map<string, Promise<Answer>>();
export const resetSoftwareCache = () => (cache.clear(), inflight.clear());

const num = (x: unknown) => (typeof x === "number" && Number.isFinite(x) && x > 0 ? Math.floor(x) : 0);

function answerOf(r: unknown): Answer {
  const o = (r && typeof r === "object" ? r : {}) as Record<string, unknown>;
  if (o.sbom !== true) return { ok: false, why: "off", text: "" };
  const hits = new Map<string, Hit>();
  for (const h of Array.isArray(o.hosts) ? o.hosts : []) {
    const x = (h && typeof h === "object" ? h : {}) as Record<string, unknown>;
    if (typeof x.uuid === "string" && !hits.has(x.uuid)) hits.set(x.uuid, { name: typeof x.name === "string" ? x.name : "", version: typeof x.version === "string" ? x.version : "" });
  }
  // `partial`: the relay's time ran out before every list was read.
  return { ok: true, hits, more: num(o.more), pending: num(o.pending), partial: !!o.partial, at: Date.now() };
}

function failureOf(e: unknown): Answer {
  const s = String(e);
  const m = /^(unsupported|forbidden|offline|busy|limit|reportoff|unavailable)(?::(\d+))?: ?(.*)$/s.exec(s);
  if (!m) return { ok: false, why: "error", text: s };
  return { ok: false, why: m[1] as Why, text: m[3], ...(m[2] ? { retry: Number(m[2]) } : {}) };
}

/** One relay's answer for one term: from the cache, the request already on its way, or a new one.
 *  Passing failures (busy, not reached, other errors) are not kept: the next search asks again. */
function ask(relay: string, t: SwTerm, call: Call): Promise<Answer> {
  const key = `${relay}\n${termKey(t)}`;
  const c = cache.get(key);
  if (c && Date.now() - c.at < TTL) return Promise.resolve(c.a);
  let p = inflight.get(key);
  if (!p) {
    p = call("team_status_software", { relay, q: t.q, v: t.v }).then(answerOf, failureOf);
    inflight.set(key, p);
    void p.then((a) => {
      inflight.delete(key);
      if (a.ok || a.why === "off" || a.why === "unsupported" || a.why === "forbidden" || a.why === "reportoff") cache.set(key, { at: Date.now(), a });
    });
  }
  return p;
}

/** Tries a relay that is busy with another search (relay-wide, not this member's fault) again after
 *  its Retry-After, at most `BUSY_TRIES` times; `onBusy(relay, true)` while it waits. */
const BUSY_TRIES = 3;
async function askRetrying(r: string, t: SwTerm, call: Call, onBusy?: (relay: string, waiting: boolean) => void): Promise<Answer> {
  for (let i = 0; ; i++) {
    const a = await ask(r, t, call);
    if (a.ok || a.why !== "busy" || i === BUSY_TRIES) {
      if (i) onBusy?.(r, false);
      return a;
    }
    onBusy?.(r, true);
    await new Promise((ok) => setTimeout(ok, Math.min(a.retry ?? 5, 60) * 1000));
  }
}

export async function runSoftware(terms: SwTerm[], relays: string[], call: Call = (c, a) => invoke(c, a), onBusy?: (relay: string, waiting: boolean) => void): Promise<Results> {
  const ok = runnable(terms);
  const out: Results = new Map(ok.map((t) => [termKey(t), new Map<string, Answer>()]));
  // A relay runs one search at a time: per relay one term after the other, the relays in parallel.
  await Promise.all(relays.map(async (r) => {
    for (const t of ok) out.get(termKey(t))!.set(r, await askRetrying(r, t, call, onBusy));
  }));
  return out;
}

/** When the oldest answer shown was received (ms), or null without one. */
export function asOf(res: Results): number | null {
  const ats = [...res.values()].flatMap((m) => [...m.values()]).flatMap((a) => (a.ok ? [a.at] : []));
  return ats.length ? Math.min(...ats) : null;
}

/** The answers as a lookup for `search`: true/false from the relay holding the host's newest list,
 *  null (never matches, in either form) without a list, without an answer from that relay, or when
 *  the relay left hosts out (more than 2000, its time ran out, or lists not indexed yet: it does not
 *  say which hosts those are). */
export function lookupFor(res: Results): SoftwareLookup {
  return (n, h) => {
    const s = h.status;
    if (!s?.sbom) return null;
    const a = res.get(termKey(termOf(n)))?.get(s.sbomRelay ?? s.relay);
    if (!a?.ok) return null;
    if (a.hits.has(s.uuid)) return true;
    return a.more || a.partial || a.pending ? null : false;
  };
}

/** Starts the search on Enter or after `delay` ms without typing; an answer for a query that changed
 *  meanwhile is dropped. `onRun`: results null while it runs. */
export type SwState = { key: string; results: Results | null; busy?: string[] };
export function searcher(call: Call, relays: () => string[], onRun: (r: SwState) => void, delay = 800) {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let gen = 0;
  /** The software terms of the search started last. */
  let cur = "";
  const keyOf = (q: string) => termsKey(runnable(softwareTerms(parse(q).node)));
  const start = (q: string) => {
    clearTimeout(timer);
    const my = ++gen;
    const terms = runnable(softwareTerms(parse(q).node));
    cur = termsKey(terms);
    if (!terms.length) return;
    onRun({ key: cur, results: null });
    const key = cur;
    const busy = new Set<string>();
    const onBusy = (r: string, waiting: boolean) => {
      if (waiting) busy.add(r);
      else busy.delete(r);
      if (my === gen) onRun({ key, results: null, busy: [...busy] });
    };
    void runSoftware(terms, relays(), call, onBusy).then((results) => my === gen && onRun({ key, results }));
  };
  return {
    /** Typing that leaves the software terms as they are (another term, a chosen query) changes nothing. */
    typed(q: string) {
      clearTimeout(timer);
      if (keyOf(q) === cur) return;
      gen++;
      cur = "";
      timer = setTimeout(() => start(q), delay);
    },
    enter: start,
    dispose() {
      clearTimeout(timer);
      gen++;
    },
  };
}

const n = (x: number, one: string, many = one + "s") => `${x.toLocaleString("en-US")} ${x === 1 ? one : many}`;
const label = (t: SwTerm) => `“${t.q}”${t.v ? ` @${t.v}` : ""}`;

/** The lines under the search box (spec 10.4). `res` null: the search is still running (`busy`: the
 *  relays it waits for because they run another search). Then per term the count, then every relay
 *  that did not give a full answer, in words. A term not sent, or not answered yet, matches nothing. */
export function chipLines(terms: SwTerm[], res: Results | null, hosts: Host[], relays: string[], busy: string[] = []): string[] {
  const out: string[] = [];
  const none = "so it matches no computer, with or without -";
  for (const t of terms) {
    if (tooShort(t)) out.push(`software: needs at least 2 characters (${label(t)}), ${none}`);
    else if (tooLong(t)) out.push(`software: at most 64 characters, version at most 32, ${none}`);
    else if (hidden(t)) out.push(`software: ${label(t)} contains an invisible or control character the server refuses; type it again. Until then it matches no computer`);
  }
  const ok = runnable(terms);
  if (!ok.length) return out;
  if (!relays.length) return [...out, "Software search needs team access to a server."];
  if (!res) {
    const n = relays.length;
    return [
      `Searching installed software on ${n} ${n === 1 ? "server" : "servers"}… Until it answers, software terms match no computer.`,
      ...busy.map((r) => `${r} is busy with another search; trying again…`),
      ...out,
    ];
  }
  const at = asOf(res);
  const time = at === null ? "" : `, as of ${new Date(at).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}`;
  const look = lookupFor(res);
  const without = hosts.filter((h) => !h.status?.sbom).length;
  for (const t of ok) {
    const f: SoftwareField = { k: "field", f: "software", op: "", v: t.q, ...(t.v ? { at: t.v } : {}) };
    const hit = hosts.filter((h) => look(f, h) === true).length;
    out.push(`Installed software ${label(t)}: ${n(hit, "computer")}${time} (lists as of each computer's last upload; ${n(without, "computer")} without a list)`);
  }
  const notes = new Set<string>();
  const answered = new Set<string>();
  for (const t of ok) {
    for (const r of relays) {
      const a = res.get(termKey(t))?.get(r);
      if (!a) continue;
      if (a.ok) {
        answered.add(r);
        if (a.more) notes.add(`more than 2000 computers match on ${r}: refine the search`);
        if (a.pending) notes.add(`${r} is still indexing ${n(a.pending, "list")}: until it is done, its computers that it did not name are left out, with or without -`);
        if (a.partial) notes.add(`${r} stopped before it searched every list: the result is incomplete`);
        continue;
      }
      notes.add(
        a.why === "unsupported"
          ? `${r} does not support software search (needs tether-api 0.7.0)`
          : a.why === "off"
            ? `software search is switched off on ${r}`
            : a.why === "forbidden"
              ? `your team membership does not allow software search on ${r}`
              : a.why === "busy"
                ? `${r} stayed busy with other searches: press Enter to try again`
                : a.why === "limit"
                  ? `you ran more than 6 software searches a minute on ${r}: wait ${a.retry ?? 60} s, then press Enter`
                  : a.why === "reportoff"
                    ? `health reporting is switched off on ${r}: it holds no software lists to search`
                    : a.why === "unavailable"
                      ? `${r} is not ready to search yet (it is preparing its status data): press Enter to try again in ${a.retry ?? 30} s`
                      : a.why === "offline"
                        ? `${r} could not be reached`
                        : `${r}: ${a.text}`,
      );
    }
  }
  const failed = relays.filter((r) => !answered.has(r));
  if (failed.length && answered.size) notes.add(`Results from ${[...answered].join(", ")} only.`);
  return [...out, ...notes];
}

/** For a query that negates a software term (spec 10.4: unknown is never "does not have it"). */
export function negatedSoftware(node: Node | null, neg = false): boolean {
  if (!node) return false;
  if (node.k === "and" || node.k === "or") return node.c.some((c) => negatedSoftware(c, neg));
  if (node.k === "not") return negatedSoftware(node.c, !neg);
  return neg && node.k === "field" && node.f === "software";
}
export const NEGATED_NOTE = "Computers without a software list, or whose list is not indexed yet or could not be read, are not shown either way.";

/** With a `check:` term: the hosts whose summary left failing checks out, so the filter may miss them. */
export function faultsHint(node: Node | null, hosts: Host[]): string | null {
  const has = (x: Node | null): boolean => !!x && (x.k === "and" || x.k === "or" ? x.c.some(has) : x.k === "not" ? has(x.c) : x.k === "field" && x.f === "check");
  if (!has(node)) return null;
  const c = hosts.filter((h) => (h.status?.more_faults ?? 0) > 0).length;
  return c ? `${n(c, "computer")} ${c === 1 ? "has" : "have"} more failing checks than the summary holds; open ${c === 1 ? "it" : "them"} to see all` : null;
}

// A host's software list: the CycloneDX document the agent builds (docs/health.md), read tolerantly.
// The relay stores any JSON object a device sent, so the text may be deeply nested, huge or not a
// CycloneDX document at all: `parse` never throws, looks only at the fields it needs (with explicit
// type checks, never iterating the whole tree), cuts every string and keeps at most 5000 programs.
export interface SbomProgram {
  name: string;
  version: string;
  publisher: string;
  /** A product the agent detected (`tether:product`). */
  product: boolean;
  role: string;
}
export interface SbomDevice {
  manufacturer: string;
  model: string;
  serial: string;
  /** Bytes; 0 when not given. */
  memory: number;
  cpu: string;
}
export interface Sbom {
  /** The text is a JSON object with a `components` array; false: only the export is offered. */
  readable: boolean;
  device?: SbomDevice;
  os?: { name: string; version: string };
  /** Detected products first, then by name. */
  programs: SbomProgram[];
  /** The agent left programs out (`tether:truncated`). */
  truncated: boolean;
  omitted: number;
  /** The document holds more programs than `MAX_PROGRAMS`: only the first ones are kept. */
  capped: boolean;
  tool?: string;
}

export const MAX_PROGRAMS = 5000;
const MAX_PROPS = 64;
const NAME = 128;
const VERSION = 64;

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);
// Control, format (bidi overrides, zero-width) and line/paragraph separator characters: a name could use
// them to look like another one. Removed before a string is cut.
const HIDDEN = /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/gu;

/** Device text for display: hidden characters removed, cut to `n` code points (a pair is never split), "…" marks a cut. */
export function clean(s: string, n: number): string {
  const t = s.replace(HIDDEN, "");
  if (t.length <= n) return t;
  // n code points are at most 2n units: nothing beyond that is looked at.
  const cps = [...t.slice(0, 2 * n)];
  return cps.length <= n && t.length <= 2 * n ? t : cps.slice(0, n - 1).join("") + "…";
}
const text = (v: unknown, n: number): string => (typeof v === "string" ? clean(v, n) : "");

/** `properties` as name -> value (strings only, first of a name wins); anything but an array gives none. */
function props(v: unknown): Map<string, string> {
  const m = new Map<string, string>();
  if (!Array.isArray(v)) return m;
  for (let i = 0; i < v.length && i < MAX_PROPS; i++) {
    const p: unknown = v[i];
    if (isObj(p) && typeof p.name === "string" && typeof p.value === "string" && !m.has(p.name)) m.set(p.name, clean(p.value, NAME));
  }
  return m;
}

const collator = new Intl.Collator(undefined, { sensitivity: "base", numeric: true });
const byProduct = (a: SbomProgram, b: SbomProgram) => Number(b.product) - Number(a.product) || collator.compare(a.name, b.name);

const unreadable = (): Sbom => ({ readable: false, programs: [], truncated: false, omitted: 0, capped: false });

export function parse(raw: string): Sbom {
  let doc: unknown;
  try {
    doc = JSON.parse(raw);
  } catch {
    // Not JSON, or nested so deep that the parser ran out of stack.
    return unreadable();
  }
  if (!isObj(doc) || !Array.isArray(doc.components)) return unreadable();
  const out: Sbom = { readable: true, programs: [], truncated: false, omitted: 0, capped: false };

  const meta = isObj(doc.metadata) ? doc.metadata : {};
  const machine = isObj(meta.component) ? props(meta.component.properties) : new Map<string, string>();
  const bytes = Number(machine.get("tether:hw.memory_bytes"));
  // A real machine has 1 byte to 1 PiB (2^50); anything else is not shown.
  const hw: SbomDevice = {
    manufacturer: machine.get("tether:hw.manufacturer") ?? "",
    model: machine.get("tether:hw.model") ?? "",
    serial: machine.get("tether:hw.serial") ?? "",
    memory: Number.isFinite(bytes) && bytes >= 1 && bytes <= 2 ** 50 ? bytes : 0,
    cpu: machine.get("tether:hw.cpu") ?? "",
  };
  if (hw.manufacturer || hw.model || hw.serial || hw.memory || hw.cpu) out.device = hw;
  const omitted = /^\d{1,9}$/.test(machine.get("tether:truncated") ?? "") ? Number(machine.get("tether:truncated")) : 0;
  out.omitted = omitted;
  out.truncated = omitted > 0;

  // The agent: `tools.components[0]` (CycloneDX 1.5 and later) or the older `tools[0]`.
  const tools = meta.tools;
  const t: unknown = Array.isArray(tools) ? tools[0] : isObj(tools) && Array.isArray(tools.components) ? tools.components[0] : undefined;
  if (isObj(t) && typeof t.name === "string") out.tool = [text(t.name, NAME), text(t.version, VERSION)].filter(Boolean).join(" ");

  const list: unknown[] = doc.components;
  for (let i = 0; i < list.length; i++) {
    const c: unknown = list[i];
    if (!isObj(c)) continue;
    const name = text(c.name, NAME);
    if (!name) continue;
    if (c.type === "operating-system") {
      out.os ??= { name, version: text(c.version, VERSION) };
    } else if (c.type === "application") {
      if (out.programs.length >= MAX_PROGRAMS) {
        out.capped = true;
        break;
      }
      const p = props(c.properties);
      out.programs.push({ name, version: text(c.version, VERSION), publisher: text(c.publisher, NAME), product: p.get("tether:product") === "true", role: p.get("tether:role") ?? "" });
    }
  }
  out.programs.sort(byProduct);
  return out;
}

/** The programs whose name or publisher contains `q` (case-insensitive, plain text). */
export function filterPrograms(list: SbomProgram[], q: string): SbomProgram[] {
  const f = q.trim().toLowerCase();
  return f ? list.filter((p) => p.name.toLowerCase().includes(f) || p.publisher.toLowerCase().includes(f)) : list;
}

/** `<host>-<yyyy-mm-dd>.cdx.json` with the host name reduced to A-Za-z0-9._- (at most 64 of them); a name Windows
 *  reserves for a device (CON, NUL, COM1 ... as the part before the first dot) gets a `_` in front. */
export function exportName(host: string, day: string): string {
  const h = host.replace(/[^A-Za-z0-9._-]/g, "_").slice(0, 64) || "host";
  return `${/^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])$/i.test(h.split(".")[0]) ? "_" : ""}${h}-${day}.cdx.json`;
}

/** What the section shows for the loaded text, and whether it must load: a document belongs to the `sha` it was
 *  loaded for, so one for another `sha` than the summary's is never shown or exported; it loads when the section is
 *  open and holds no document for the summary's `sha`. */
export function sbomPlan(open: boolean, sha: string, loadedSha: string | null): { show: boolean; load: boolean } {
  const show = loadedSha === sha;
  return { show, load: open && !show };
}

export const SOFTWARE_ROWS = 200;
/** The rows to draw: at most `SOFTWARE_ROWS` until the user asks for all. */
export const visibleRows = <T,>(rows: T[], all: boolean): T[] => (all ? rows : rows.slice(0, SOFTWARE_ROWS));

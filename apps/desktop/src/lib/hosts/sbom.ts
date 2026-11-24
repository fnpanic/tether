// A host's software list: the CycloneDX document the agent builds (docs/health.md), read tolerantly.
// The relay stores any JSON object a device sent, so the text may be deeply nested, huge or not a
// CycloneDX document at all: `parse` never throws, looks only at the fields it needs (with explicit
// type checks, never iterating the whole tree), cuts every string and keeps at most 5000 programs.
import { clip } from "./health";

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
  tool?: string;
}

export const MAX_PROGRAMS = 5000;
const MAX_PROPS = 64;
const NAME = 128;
const VERSION = 64;

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);
const text = (v: unknown, n: number): string => (typeof v === "string" ? clip(v, n) : "");

/** `properties` as name -> value (strings only, first of a name wins); anything but an array gives none. */
function props(v: unknown): Map<string, string> {
  const m = new Map<string, string>();
  if (!Array.isArray(v)) return m;
  for (let i = 0; i < v.length && i < MAX_PROPS; i++) {
    const p: unknown = v[i];
    if (isObj(p) && typeof p.name === "string" && typeof p.value === "string" && !m.has(p.name)) m.set(p.name, clip(p.value, NAME));
  }
  return m;
}

const collator = new Intl.Collator(undefined, { sensitivity: "base", numeric: true });
const byProduct = (a: SbomProgram, b: SbomProgram) => Number(b.product) - Number(a.product) || collator.compare(a.name, b.name);

const unreadable = (): Sbom => ({ readable: false, programs: [], truncated: false, omitted: 0 });

export function parse(raw: string): Sbom {
  let doc: unknown;
  try {
    doc = JSON.parse(raw);
  } catch {
    // Not JSON, or nested so deep that the parser ran out of stack.
    return unreadable();
  }
  if (!isObj(doc) || !Array.isArray(doc.components)) return unreadable();
  const out: Sbom = { readable: true, programs: [], truncated: false, omitted: 0 };

  const meta = isObj(doc.metadata) ? doc.metadata : {};
  const machine = isObj(meta.component) ? props(meta.component.properties) : new Map<string, string>();
  const bytes = Number(machine.get("tether:hw.memory_bytes"));
  const hw: SbomDevice = {
    manufacturer: machine.get("tether:hw.manufacturer") ?? "",
    model: machine.get("tether:hw.model") ?? "",
    serial: machine.get("tether:hw.serial") ?? "",
    memory: Number.isFinite(bytes) && bytes > 0 ? bytes : 0,
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
  for (let i = 0; i < list.length && out.programs.length < MAX_PROGRAMS; i++) {
    const c: unknown = list[i];
    if (!isObj(c) || typeof c.name !== "string" || !c.name) continue;
    if (c.type === "operating-system") {
      out.os ??= { name: text(c.name, NAME), version: text(c.version, VERSION) };
    } else if (c.type === "application") {
      const p = props(c.properties);
      out.programs.push({ name: text(c.name, NAME), version: text(c.version, VERSION), publisher: text(c.publisher, NAME), product: p.get("tether:product") === "true", role: p.get("tether:role") ?? "" });
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

/** `<host>-<yyyy-mm-dd>.cdx.json` with the host name reduced to A-Za-z0-9._- (and at most 64 of them). */
export function exportName(host: string, day: string): string {
  return `${host.replace(/[^A-Za-z0-9._-]/g, "_").slice(0, 64) || "host"}-${day}.cdx.json`;
}

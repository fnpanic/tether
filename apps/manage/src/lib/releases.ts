/** A file on a relay below /download/<channel>/ (GET /api/admin/releases). */
export interface ReleaseFile {
  channel: string;
  path: string;
  size: number;
  modified: number;
}

/** Every file of one release (artifact, .sig, mac .dmg + .sig), or a folder's latest.json. */
export interface ReleaseGroup {
  key: string;
  channel: string;
  /** `technician/windows-x86_64` */
  dir: string;
  /** null: the folder's manifest or a file no release name matches. */
  version: string | null;
  files: ReleaseFile[];
  size: number;
  modified: number;
}

// Must match tether_trust::release::file_version (artifact, .sig, TetherQS .exe.gz, mac .dmg + .sig).
const NAME = /^(?:Tether_(.+?)_(?:x64-setup\.exe|universal\.app\.tar\.gz|universal\.dmg)(?:\.sig)?|TetherQS-(.+?)(?:-universal\.zip(?:\.sig)?|\.exe(?:\.sig|\.gz)?))$/;

/** `MAJOR.MINOR.PATCH[-pre]`, numbers only in the core (tether_trust::version::parse). */
function parseVersion(v: string): { core: number[]; pre: string | null } | null {
  v = v.trim().replace(/^v+/, "");
  const i = v.indexOf("-");
  const core = (i < 0 ? v : v.slice(0, i)).split(".");
  if (core.length > 4 || !core.every((p) => /^\d+$/.test(p))) return null;
  return { core: core.map(Number), pre: i < 0 ? null : v.slice(i + 1) };
}

const sign = (n: number) => (n < 0 ? -1 : n > 0 ? 1 : 0);

/** Semver pre-release order, as tether_trust::version::compare_pre. */
function comparePre(x: string, y: string): number {
  const a = x.split("."), b = y.split(".");
  for (let i = 0; ; i++) {
    if (i >= a.length || i >= b.length) return sign(a.length - b.length);
    const [p, q] = [a[i], b[i]];
    const [pn, qn] = [/^\d+$/.test(p), /^\d+$/.test(q)];
    const o = pn && qn ? sign(Number(p) - Number(q)) : pn ? -1 : qn ? 1 : p < q ? -1 : p > q ? 1 : 0;
    if (o) return o;
  }
}

/** Version order of the clients and tether_trust::version::compare (`0.5.0-test.9 < 0.5.0-test.10 <
 *  0.5.0`). Strings that are no version sort after versions, among themselves numeric-aware. */
export function compareVersion(a: string, b: string): number {
  const va = parseVersion(a), vb = parseVersion(b);
  if (!va || !vb) return va ? -1 : vb ? 1 : a.localeCompare(b, undefined, { numeric: true });
  for (let i = 0; i < Math.max(va.core.length, vb.core.length); i++) {
    const o = sign((va.core[i] ?? 0) - (vb.core[i] ?? 0));
    if (o) return o;
  }
  if (va.pre === null || vb.pre === null) return va.pre === vb.pre ? 0 : va.pre === null ? 1 : -1;
  return comparePre(va.pre, vb.pre);
}

/** One line per release, newest version first within a channel and app. */
export function groupReleases(files: ReleaseFile[]): ReleaseGroup[] {
  const groups = new Map<string, ReleaseGroup>();
  for (const f of files) {
    const i = f.path.lastIndexOf("/");
    const dir = i < 0 ? "" : f.path.slice(0, i);
    const m = f.path.slice(i + 1).match(NAME);
    const version = m ? (m[1] ?? m[2]) : null;
    const key = `${f.channel}/${dir}/${version ?? f.path.slice(i + 1)}`;
    const g = groups.get(key) ?? { key, channel: f.channel, dir, version, files: [], size: 0, modified: 0 };
    g.files.push(f);
    g.size += f.size;
    g.modified = Math.max(g.modified, f.modified);
    groups.set(key, g);
  }
  return [...groups.values()]
    .map((g) => ({ ...g, files: g.files.sort((a, b) => a.path.localeCompare(b.path)) }))
    .sort((a, b) => a.channel.localeCompare(b.channel) || a.dir.localeCompare(b.dir) || (a.version === null ? -1 : b.version === null ? 1 : compareVersion(b.version, a.version)));
}

/** Ramp steps `[offset seconds, percent]`; must match tether_trust::release::RAMP_* (D4). */
export const rampPresets = {
  standard: [[0, 1], [3600, 10], [21600, 50], [86400, 100]],
  fast: [[0, 10], [3600, 100]],
  now: [[0, 100]],
} as const satisfies Record<string, readonly (readonly [number, number])[]>;

const after = (s: number) => (s === 0 ? "at once" : s % 86400 === 0 ? `after ${s / 86400} d` : s % 3600 === 0 ? `after ${s / 3600} h` : `after ${Math.round(s / 60)} min`);

/** "1 % at once → 10 % after 1 h → …" */
export function describeRamp(steps: readonly (readonly [number, number])[] | undefined): string {
  if (!steps?.length) return "";
  if (steps.length === 1 && steps[0][1] === 100 && steps[0][0] === 0) return "Every device at once";
  return steps.map(([s, p]) => `${p} % ${after(s)}`).join(" → ");
}

/** One release in the signed index (release_index command). */
export interface IndexEntry {
  key: string;
  version: string;
  percent: number;
  halted: boolean;
  security: boolean;
  mandatory: boolean;
  forced: boolean;
  start: number;
  steps: [number, number][];
  cdn: boolean;
}

export interface IndexView {
  channel: string;
  serial: number | null;
  expires?: number;
  days_left?: number;
  stale?: boolean;
  /** `error`: the relay serves an index no trusted admin key verifies (publishing is refused). */
  relays: { server: string; serial: number | null; error?: string | null }[];
  entries: IndexEntry[];
}

/** Where a rollout stands, for the table. */
export function rolloutState(e: IndexEntry): string {
  if (e.halted) return "Halted";
  if (e.security) return "Security release: everyone";
  if (e.percent >= 100) return "Everyone";
  if (e.percent === 0) return "Not started";
  return `${e.percent} % of devices`;
}

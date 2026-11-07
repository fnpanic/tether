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

// Must match release::NAMES in src-tauri/src/release.rs (plus the mac disk image).
const NAME = /^(?:Tether_(.+?)_(?:x64-setup\.exe|universal\.app\.tar\.gz|universal\.dmg)|TetherQS-(.+?)(?:-universal\.zip|\.exe))(?:\.sig)?$/;

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
  const byVersion = (a: string, b: string) => a.localeCompare(b, undefined, { numeric: true });
  return [...groups.values()]
    .map((g) => ({ ...g, files: g.files.sort((a, b) => a.path.localeCompare(b.path)) }))
    .sort((a, b) => a.channel.localeCompare(b.channel) || a.dir.localeCompare(b.dir) || (a.version === null ? -1 : b.version === null ? 1 : byVersion(b.version, a.version)));
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
  relays: { server: string; serial: number | null }[];
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

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

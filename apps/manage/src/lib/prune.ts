/** Release clean-up (docs/release-cleanup.md): the merged plan of release_prune_plan. */

export interface PruneVersion {
  version: string;
  files: string[];
  bytes: number;
  /** Why it is kept (union over the relays); empty when every relay would drop it. */
  why: string[];
  /** Per relay: "drop" or "keep" (relays without the version are absent). */
  relays: Record<string, "drop" | "keep">;
  /** Every relay that has it would drop it: offered for deletion. */
  offer: boolean;
}

export interface PruneGroup {
  channel: string;
  /** `customer/windows-x86_64` */
  dir: string;
  published: { server: string; version: string | null }[];
  versions: PruneVersion[];
}

export interface PrunePlan {
  keep: number;
  relays: string[];
  /** The relays hold different release indexes: a lagging one protects its older version. */
  index_differs: boolean;
  groups: PruneGroup[];
  offer: { versions: number; files: number; bytes: number };
  kept_versions: number;
}

export interface PruneVer {
  channel: string;
  dir: string;
  version: string;
}

export const verKey = (channel: string, dir: string, version: string) => `${channel}/${dir}/${version}`;

const REASONS: Record<string, string> = {
  index: "in the release index",
  "latest.json": "published (latest.json)",
  newer: "newer than published",
  previous: "kept as an older version",
  "download page": "download page",
  pinned: "on the CDN",
  recent: "uploaded in the last 24 h",
};

/** "in the release index · published (latest.json)" */
export function reasons(why: string[]): string {
  return why.map((w) => REASONS[w] ?? w).join(" · ");
}

/** The keys of every offered version (the sheet starts with all of them ticked). */
export function offeredKeys(plan: PrunePlan): Set<string> {
  return new Set(plan.groups.flatMap((g) => g.versions.filter((v) => v.offer).map((v) => verKey(g.channel, g.dir, v.version))));
}

/** The ticked versions as the endpoint takes them, with their totals. */
export function selection(plan: PrunePlan, ticked: Set<string>): { delete: PruneVer[]; files: number; bytes: number } {
  const out = { delete: [] as PruneVer[], files: 0, bytes: 0 };
  for (const g of plan.groups)
    for (const v of g.versions)
      if (v.offer && ticked.has(verKey(g.channel, g.dir, v.version))) {
        out.delete.push({ channel: g.channel, dir: g.dir, version: v.version });
        out.files += v.files.length;
        out.bytes += v.bytes;
      }
  return out;
}

/** Relays that keep a version while others drop it ("kept on hetzner…"), for the per-relay note. */
export function keptOn(v: PruneVersion): string[] {
  const states = Object.values(v.relays);
  return states.includes("drop") ? Object.entries(v.relays).filter(([, s]) => s === "keep").map(([sv]) => sv) : [];
}

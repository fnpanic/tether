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

export type Steps = readonly (readonly [number, number])[];

/** The custom ramp's fields: first wave, then hours from the start until 10 %, 50 % and 100 %. */
export interface CustomRamp {
  first: number;
  h10: number;
  h50: number;
  h100: number;
}
export const defaultCustom: CustomRamp = { first: 5, h10: 2, h50: 12, h100: 48 };

/** `custom:<hours>=<percent>,…` for release::rollout; steps the first wave already reaches are left out. */
export function customRollout(c: CustomRamp): string {
  const steps: [number, number][] = [[0, c.first]];
  for (const [p, h] of [
    [10, c.h10],
    [50, c.h50],
    [100, c.h100],
  ] as const)
    if (p > c.first) steps.push([h, p]);
  return "custom:" + steps.map(([h, p]) => `${h}=${p}`).join(",");
}

/** Fields of a `custom:` rollout made by [customRollout]; null for any other shape. */
export function customFields(rollout: string): CustomRamp | null {
  const s = rampSteps(rollout);
  if (!s || !rollout.startsWith("custom:")) return null;
  const at = (p: number) => s.find((x) => x[1] === p)?.[0];
  const first = s[0][1];
  const c = { first, h10: (at(10) ?? 0) / 3600, h50: (at(50) ?? 0) / 3600, h100: (at(100) ?? 0) / 3600 };
  return customRollout(c) === rollout ? c : null;
}

/** Hours of a ramp step, as release::rollout reads them (whole minutes, at most 30 days). */
const MAX_HOURS = 720;

/** Why release::rollout would refuse `rollout`, or null. Must match it (release.rs). */
export function rolloutProblem(rollout: string): string | null {
  if (!rollout.startsWith("custom:")) return rollout in rampPresets || rollout === "" ? null : `unknown rollout ${rollout}`;
  const steps: [number, number][] = [];
  for (const part of rollout.slice(7).split(",")) {
    const [h, p, extra] = part.split("=");
    // Plain decimals only, like release.rs (no 1e2, 0x10, 100.0, Infinity).
    if (extra !== undefined || !/^(\d+\.?\d*|\.\d+)$/.test(h.trim()) || !/^\d+$/.test(p?.trim() ?? "")) return "Each step needs hours and a whole percent.";
    const hours = Number(h),
      percent = Number(p);
    if (hours < 0 || hours > MAX_HOURS) return "Hours must be 0 to 720 (30 days).";
    if (percent < 1 || percent > 100) return "Percent must be 1 to 100.";
    const secs = Math.round(hours * 60) * 60;
    const last = steps[steps.length - 1];
    if (!last && secs !== 0) return "The first step starts at once.";
    if (last && (secs <= last[0] || percent <= last[1])) return "Each step must come later and reach more devices than the one before.";
    steps.push([secs, percent]);
  }
  if (steps.length > 16) return "At most 16 steps.";
  if (steps[steps.length - 1][1] !== 100) return "The last step must reach 100 %.";
  return null;
}

/** The steps of a preset or a valid `custom:` rollout; null when release::rollout would refuse it. */
export function rampSteps(rollout: string): Steps | null {
  if (rolloutProblem(rollout)) return null;
  if (!rollout.startsWith("custom:")) return rampPresets[(rollout || "standard") as keyof typeof rampPresets];
  return rollout
    .slice(7)
    .split(",")
    .map((x) => x.split("=").map(Number))
    .map(([h, p]) => [Math.round(h * 60) * 60, p] as const);
}

const after = (s: number) => (s === 0 ? "at once" : s % 86400 === 0 ? `after ${s / 86400} d` : s % 3600 === 0 ? `after ${s / 3600} h` : `after ${Math.round(s / 60)} min`);

/** "1 % at once → 10 % after 1 h → …" */
export function describeRamp(steps: Steps | null | undefined): string {
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

// ---- "Publish the latest builds": what each relay got, build by build ----

/** One relay's (or "CDN"/"Cleanup"/"Cancel") outcome, as the commands return it (ipc RelayResult). */
export interface Row {
  server: string;
  ok: boolean;
  error?: string;
  detail: any;
}

/** A failed or left-out relay that will not answer the next build either. */
const SILENT = /^skipped|not reachable|No answer from the server|Could not reach the server|stalled|stopped sending/;

/** `"<relay>: <error>; <relay>: <error>"` (a build that failed on every relay) split per relay;
 *  null when `msg` is not that (one error for the whole build). */
export function perRelay(msg: string, servers: string[]): Map<string, string> | null {
  const at = servers.map((sv) => [sv, msg.indexOf(`${sv}: `)] as const).filter(([, i]) => i >= 0).sort((a, b) => a[1] - b[1]);
  if (!at.length || at[0][1] !== 0) return null;
  return new Map(at.map(([sv, i], k) => [sv, msg.slice(i + sv.length + 2, k + 1 < at.length ? at[k + 1][1] - 2 : undefined)]));
}

interface Got {
  live: string[];
  errs: string[];
  cancelled: string[];
  warn: string[];
  /** Builds this target left to the relays (CDN: file over its size limit). */
  notes: string[];
}

/** Per relay what went live and what did not (and why), over several builds. `skip` grows with
 *  the relays that went silent: the later builds and the page leave them out. */
export class Tally {
  per = new Map<string, Got>();
  late: string[] = [];
  labels: string[] = [];
  cancelled = false;
  servers: string[];
  skip: string[];
  constructor(servers: string[], skip: string[]) {
    this.servers = servers;
    this.skip = skip;
    servers.forEach((sv) => this.at(sv));
  }
  private at(sv: string) {
    if (!this.per.has(sv)) this.per.set(sv, { live: [], errs: [], cancelled: [], warn: [], notes: [] });
    return this.per.get(sv)!;
  }
  /** The relays the next step goes to. */
  targets() {
    return this.servers.filter((sv) => !this.skip.includes(sv));
  }
  private fail(sv: string, what: string, e: string) {
    this.at(sv).errs.push(`${what}: ${e}`);
    if (this.servers.includes(sv) && SILENT.test(e) && !this.skip.includes(sv)) this.skip.push(sv);
  }
  private skipped(what: string) {
    this.servers.filter((sv) => this.skip.includes(sv)).forEach((sv) => this.at(sv).errs.push(`${what}: skipped (not reachable)`));
  }
  /** A step's per-relay results (`what`: "Customer module Windows 0.9.3", "Download page"). */
  results(what: string, rows: Row[]) {
    if (!this.labels.includes(what)) this.labels.push(what);
    for (const x of rows) {
      if (x.server === "Cancel") this.late.push(`${what}: ${x.error}`);
      else if (x.ok && x.detail?.relaysOnly) this.at(x.server).notes.push(`${what}: ${x.detail.note}`);
      else if (x.ok) {
        this.at(x.server).live.push(what);
        if (x.detail?.warning) this.at(x.server).warn.push(`${what}: ${x.detail.warning}`);
        if (x.detail?.note) this.at(x.server).notes.push(`${what}: ${x.detail.note}`);
      } else this.fail(x.server, what, x.error ?? "failed");
    }
  }
  /** A step that failed as a whole: its error split per relay when it names them. */
  error(what: string, msg: string) {
    if (!this.labels.includes(what)) this.labels.push(what);
    if (msg.startsWith("Cancelled.")) return this.cancel(what);
    const to = this.targets();
    const own = perRelay(msg, to);
    this.skipped(what);
    // A whole-build error that names no relay is not a relay's silence: do not skip on it.
    to.forEach((sv) => (own ? this.fail(sv, what, own.get(sv) ?? msg) : this.at(sv).errs.push(`${what}: ${msg}`)));
  }
  /** Not started: Cancel came first, or every relay is left out. */
  cancel(what: string) {
    if (!this.labels.includes(what)) this.labels.push(what);
    this.cancelled = true;
    this.servers.forEach((sv) => this.at(sv).cancelled.push(what));
  }
  none(what: string) {
    if (!this.labels.includes(what)) this.labels.push(what);
    this.skipped(what);
  }
  /** One row per relay: what is live there, what is not (with why); "Cancel" for a too-late Cancel. */
  rows(): Row[] {
    const out: Row[] = [...this.per].map(([server, g]) => {
      // Cancelled builds are no failure where something went live; where nothing did, they are the result.
      const errs = g.live.length ? g.errs : [...g.errs, ...g.cancelled.map((c) => `${c}: cancelled, not published`)];
      const warn = [...g.warn, ...(g.live.length && g.cancelled.length ? [`Not published (cancelled): ${g.cancelled.join(", ")}`] : [])];
      // One line per build.
      return { server, ok: !errs.length, error: errs.join("\n") || undefined, detail: { published: g.live, warning: warn.join("\n") || undefined, note: g.notes.join("\n") || undefined } };
    });
    if (this.late.length) out.push({ server: "Cancel", ok: false, error: this.late.join("; "), detail: null });
    return out;
  }
  /** Where each step is live: "X is live on every relay" / "on a, b" / nothing for none. */
  live(): string[] {
    const host = (u: string) => u.replace(/^https?:\/\//, "");
    return this.labels.flatMap((l) => {
      const on = this.servers.filter((sv) => this.per.get(sv)!.live.includes(l));
      return on.length ? [`${l} is live on ${on.length === this.servers.length ? "every relay" : on.map(host).join(", ")}`] : [];
    });
  }
  /** The last step that went live on some relay. */
  lastLive() {
    return [...this.labels].reverse().find((l) => this.servers.some((sv) => this.per.get(sv)!.live.includes(l)));
  }
  /** The result sheet's summary line. */
  summary(): string {
    const live = this.live();
    const rows = this.rows().filter((r) => this.servers.includes(r.server));
    if (this.cancelled) {
      const last = this.lastLive();
      if (!last) return "Cancelled. Nothing was published; files that already went out stay unused until the next publish or Clean up.";
      return `Cancelled after ${last}${this.late.length ? " (too late to stop it: its release index was already being written)" : ""}; ${live.join("; ")}. The builds after it were not published.`;
    }
    if (this.late.length) return `Cancel came too late: the release index was already being written. ${live.join("; ")}.`;
    if (rows.every((r) => r.ok)) return rows.some((r) => r.detail?.warning) ? "Live on every relay. Some need attention: see the warnings below." : "Published on every relay.";
    return `Not every relay has it. ${live.length ? `${live.join("; ")}. ` : "Nothing went live. "}Fix the relay and publish again, or bring it up to date from the header.`;
  }
  /** The toast: never a plain success after a Cancel or a failure. */
  toast(versions: string): string {
    if (this.cancelled) return this.lastLive() ? `Cancelled after ${this.lastLive()}` : "Cancelled. Nothing was published";
    if (this.late.length) return "Cancel came too late: it is live where marked done";
    const rows = this.rows().filter((r) => this.servers.includes(r.server));
    return rows.every((r) => r.ok) ? `Published ${versions}` : `Published ${versions} on some relays only: see the results`;
  }
}

/** The toast after one release (`version` to `channel`): matches the result, not just "released". */
export function releaseToast(version: string, channel: string, rows: Row[]): string {
  const relays = rows.filter((r) => !["Cancel", "CDN", "Cleanup"].includes(r.server));
  const live = relays.filter((r) => r.ok).length;
  if (rows.some((r) => r.server === "Cancel")) return `Too late to cancel: ${version} is live on ${live} of ${relays.length} relays`;
  if (live < relays.length) return `Released ${version} to ${channel} on ${live} of ${relays.length} relays only`;
  if (relays.some((r) => r.detail?.warning)) return `Released ${version} to ${channel}; some relays need attention`;
  return `Released ${version} to ${channel}`;
}

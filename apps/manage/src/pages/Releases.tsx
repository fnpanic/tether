import { Fragment, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { ask, open } from "@tauri-apps/plugin-dialog";
import { Check, ChevronRight, RefreshCw, Trash2, X } from "lucide-react";
import { useApp } from "../App";
import { api, bytes, host, inTauri, invoke, target, when, type RelayResult } from "../lib/ipc";
import { idle, percent, track, type Progress } from "../lib/progress";
import { pageHtml } from "../lib/page-template";
import { PublishSheet } from "../components/publish";
import { CleanupSheet } from "../components/cleanup";
import { describeRamp, groupReleases, rampPresets, rolloutState, type IndexView, type ReleaseGroup } from "../lib/releases";
import { Badge, Button, Card, ErrorText, IconButton, Label, PageHeader, Segmented, Table, TextArea, TextField, Toggle } from "../components/ui";

interface Release {
  channel: string;
  path: string;
  size: number;
  modified: number;
}
type Kind = "technician" | "customer";
type Platform = "windows-x86_64" | "darwin-universal";

interface OverviewRow {
  channel: string;
  kind: Kind;
  platform: Platform;
  /** Highest version uploaded to any relay. */
  latest: string;
  relays: { server: string; published: string | null; has_latest: boolean | null; error: string | null }[];
}

/** Remote file name; must match release::target in src-tauri/src/release.rs. */
const fileName = (kind: Kind, platform: Platform, v: string) =>
  platform === "windows-x86_64"
    ? kind === "technician"
      ? `Tether_${v}_x64-setup.exe`
      : `TetherQS-${v}.exe`
    : kind === "technician"
      ? `Tether_${v}_universal.app.tar.gz`
      : `TetherQS-${v}-universal.zip`;

const APP: Record<Kind, string> = { technician: "Technician app", customer: "Customer module" };
const OS: Record<Platform, string> = { "windows-x86_64": "Windows", "darwin-universal": "macOS" };

const release = (p: Progress) => {
  const [kind, platform] = p.dir.split("/") as [Kind, Platform];
  return `${APP[kind] ?? kind} for ${OS[platform] ?? platform} ${p.version}`;
};

/** Live view of a release upload (events from release_publish / release_sync). */
function UploadProgress() {
  const { state } = useApp();
  const [s, setS] = useState(idle);
  useEffect(() => {
    if (!inTauri) return;
    const un = listen<Progress>("release-progress", (e) => setS((s) => track(s, e.payload)));
    return () => void un.then((f) => f());
  }, []);
  const c = s.current;
  const n = state.servers.length;
  return (
    <div className="mt-4 rounded-xl border border-hairline p-3" aria-live="polite">
      {!c ? (
        <p className="text-caption text-muted">Working…</p>
      ) : (
        <>
          <p className="text-callout font-semibold">{release(c)}</p>
          <p className="mt-1 text-caption text-muted">
            {!c.server
              ? `Signing ${c.file}…`
              : c.server === "CDN"
                ? `Cloudflare CDN · ${c.file || "working"}…`
                : `Relay ${state.servers.indexOf(c.server) + 1} of ${n} · ${host(c.server)} · ${c.file === "latest.json" ? "writing latest.json" : `uploading ${c.file}`}`}
          </p>
          {c.server && c.total > 0 && (
            <>
              <div className="mt-2 h-1.5 overflow-hidden rounded-full bg-tile-2" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent(c)}>
                <div className="h-full rounded-full bg-accent" style={{ width: `${percent(c)}%` }} />
              </div>
              <p className="mt-1 text-footnote text-muted">
                {bytes(c.sent)} of {bytes(c.total)} · {percent(c)}%
              </p>
            </>
          )}
        </>
      )}
      {s.finished.length > 0 && (
        <ul className="mt-3 flex flex-col gap-1">
          {s.finished.map((f, i) => (
            <li key={i} className="flex items-start gap-2 text-caption">
              {f.error ? <X size={16} className="mt-0.5 shrink-0 text-danger" aria-label="Failed" /> : <Check size={16} className="mt-0.5 shrink-0 text-ok" aria-label="Done" />}
              <span>
                {release(f)} · <span className="font-semibold">{target(f.server)}</span>
              </span>
              {f.error && <span className="min-w-0 flex-1 select-text text-danger">{f.error}</span>}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/** What a relay publishes for one release row, as a badge. */
function Published({ r, latest }: { r: OverviewRow["relays"][number]; latest: string }) {
  if (r.published === latest) return <Badge tone="ok">Published · latest</Badge>;
  if (r.published) return <Badge tone="warn" title={`Devices on this relay get ${r.published}; ${latest} is uploaded${r.has_latest ? " here" : " elsewhere"} but not published here.`}>Older · {r.published}</Badge>;
  return <Badge tone="danger" title={r.error ?? "No latest.json for this app and platform"}>Missing on {host(r.server)}</Badge>;
}

function Overview() {
  const { toast } = useApp();
  const [data, setData] = useState<{ rows: OverviewRow[]; errors: string[] } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [sync, setSync] = useState<OverviewRow | null>(null);
  const load = () => invoke<{ rows: OverviewRow[]; errors: string[] }>("releases_overview").then(setData, (e) => setError(e.message));
  useEffect(() => void load(), []);
  const servers = data?.rows[0]?.relays.map((r) => r.server) ?? [];

  return (
    <section className="mb-10">
      <div className="mb-3 flex items-end justify-between">
        <div>
          <h2 className="text-headline font-semibold">What devices get</h2>
          <p className="mt-1 text-caption text-muted">Published = the version in each relay's public latest.json. Latest = the highest version uploaded to any relay.</p>
        </div>
        <IconButton label="Refresh" onClick={load}>
          <RefreshCw size={16} />
        </IconButton>
      </div>
      <ErrorText error={error} />
      {data?.errors.map((e) => <ErrorText key={e} error={e} />)}
      <Table head={["Channel", "App", "Platform", "Latest", ...servers.map(host), ""]} empty={data && !data.rows.length ? "No releases yet." : false}>
        {data?.rows.map((row) => {
          const missing = row.relays.filter((r) => r.published !== row.latest);
          const source = row.relays.some((r) => r.published === row.latest);
          return (
            <tr key={`${row.channel}/${row.kind}/${row.platform}`}>
              <td>{row.channel}</td>
              <td>{APP[row.kind]}</td>
              <td>{OS[row.platform]}</td>
              <td className="font-semibold">{row.latest}</td>
              {row.relays.map((r) => (
                <td key={r.server}>
                  <Published r={r} latest={row.latest} />
                </td>
              ))}
              <td className="text-right">
                {missing.length > 0 &&
                  (source ? (
                    <Button size="sm" variant="secondary" onClick={() => setSync(row)}>
                      Publish to missing relays
                    </Button>
                  ) : (
                    <span className="text-footnote text-muted">Not published anywhere: use Sign and publish</span>
                  ))}
              </td>
            </tr>
          );
        })}
      </Table>
      {sync && (
        <PublishSheet
          title={`Publish ${sync.latest} to the missing relays`}
          action="Copy and publish"
          progress={<UploadProgress />}
          onClose={() => {
            setSync(null);
            load();
          }}
          run={async () => {
            const r = await invoke<RelayResult[]>("release_sync", { channel: sync.channel, kind: sync.kind, platform: sync.platform, version: sync.latest });
            toast(`${sync.latest} published on every relay`);
            return r;
          }}
        >
          <p className="text-callout">
            {APP[sync.kind]} for {OS[sync.platform]} {sync.latest}, channel {sync.channel}.
          </p>
          <p className="mt-2 text-caption text-muted">
            The files and their signatures are copied from a relay that publishes {sync.latest} (nothing is signed again). Each relay gets its own latest.json pointing at itself, files first, latest.json last. Relays that already publish it are left alone.
          </p>
        </PublishSheet>
      )}
    </section>
  );
}

interface Build {
  kind: Kind;
  platform: Platform;
  version: string;
  file: string;
  /** Highest version the relays publish on the channel; null = nothing yet. */
  published: string | null;
  newer: boolean;
  problem: string | null;
}

/**
 * One click for the usual release: the newest build of each app in the build folder (dist), signed
 * and published where it is newer than what the relays publish, then the download page rebuilt.
 */
function LatestBuilds({ onDone }: { onDone: () => void }) {
  const { state, reload, toast } = useApp();
  const s = state.settings;
  const [folder, setFolder] = useState<string>(s.release_folder ?? "");
  const [channel, setChannel] = useState("stable");
  const [builds, setBuilds] = useState<Build[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notes, setNotes] = useState("");
  const [page, setPage] = useState(true);
  const [reviewing, setReviewing] = useState(false);
  const stable = channel.trim() === "stable";

  const scan = (f = folder) => {
    if (!f) return;
    setError(null);
    setBuilds(null);
    invoke<Build[]>("release_scan", { folder: f, channel: channel.trim() }).then(setBuilds, (e) => setError(e.message));
  };
  useEffect(() => scan(), [folder]);

  const choose = async () => {
    const f = await open({ directory: true, defaultPath: folder || undefined });
    if (typeof f !== "string") return;
    setFolder(f);
    await api.saveSettings({ ...s, release_folder: f });
    await reload();
  };

  const todo = (builds ?? []).filter((b) => b.newer && !b.problem);

  /** Every release, then the page; one result row per relay, failed if anything failed there. */
  const publishAll = async (): Promise<RelayResult[]> => {
    const per = new Map<string, { errs: string[]; done: string[] }>(state.servers.map((sv) => [sv, { errs: [], done: [] }]));
    const add = (server: string, what: string, err?: string) => {
      if (!per.has(server)) per.set(server, { errs: [], done: [] });
      if (err) per.get(server)!.errs.push(`${what}: ${err}`);
      else per.get(server)!.done.push(what);
    };
    for (const b of todo) {
      const label = `${APP[b.kind]} ${OS[b.platform]} ${b.version}`;
      try {
        const r = await invoke<RelayResult[]>("release_publish", {
          args: { file: b.file, kind: b.kind, platform: b.platform, channel: channel.trim(), version: b.version, notes, mandatory: false, minimum_version: "", forced: false },
        });
        r.forEach((x) => add(x.server, label, x.ok ? undefined : x.error));
      } catch (e: any) {
        state.servers.forEach((sv) => add(sv, label, e.message));
      }
    }
    if (page && stable) {
      try {
        // The saved design: Standard (built-in) or the custom template (docs/download-page.md).
        const html = await pageHtml(s, state.servers, invoke);
        const r = await invoke<RelayResult[]>("page_publish", { html });
        r.forEach((x) => add(x.server, "Download page", x.ok ? undefined : x.error));
      } catch (e: any) {
        state.servers.forEach((sv) => add(sv, "Download page", e.message));
      }
    }
    // Failed relays list every error; the others what they now publish.
    const results = [...per].map(([server, { errs, done }]): RelayResult => ({ server, ok: !errs.length, error: errs.join("; ") || undefined, detail: { published: done } }));
    if (results.every((r) => r.ok)) toast(`Published ${[...new Set(todo.map((b) => b.version))].join(", ")}`);
    return results;
  };

  return (
    <section className="mb-10">
      <h2 className="mb-3 text-headline font-semibold">Publish the latest builds</h2>
      <Card>
        <div className="grid grid-cols-[minmax(0,1fr)_160px] gap-x-6 gap-y-4">
          <Label text="Build folder" hint="Searched two levels deep, e.g. dist with windows, quick and macos inside. Remembered for next time.">
            <div className="flex gap-2">
              <TextField readOnly value={folder} placeholder="No folder chosen" />
              <Button onClick={choose}>Choose…</Button>
              <IconButton label="Scan again" onClick={() => scan()}>
                <RefreshCw size={16} />
              </IconButton>
            </div>
          </Label>
          <Label text="Channel">
            <TextField value={channel} onChange={(e) => setChannel(e.target.value)} onBlur={() => scan()} />
          </Label>
        </div>
        {folder && (
          <div className="mt-4">
            <ErrorText error={error} />
            {!error && (
              <Table head={["App", "Platform", "In the folder", "Relays publish", ""]} empty={!builds ? "Scanning…" : !builds.length ? "No Tether builds in this folder." : false}>
                {builds?.map((b) => (
                  <tr key={b.kind + b.platform}>
                    <td>{APP[b.kind]}</td>
                    <td>{OS[b.platform]}</td>
                    <td className="font-semibold" title={b.file}>
                      {b.version}
                    </td>
                    <td>{b.published ?? "—"}</td>
                    <td className="text-right">
                      {b.problem ? <Badge tone="danger">{b.problem}</Badge> : b.newer ? <Badge tone="warn">Will be published</Badge> : <Badge tone="ok">Up to date</Badge>}
                    </td>
                  </tr>
                ))}
              </Table>
            )}
          </div>
        )}
        <div className="mt-4 flex items-center justify-end gap-6">
          {stable && (
            <div className="flex items-center gap-3 text-caption">
              Then update the download page
              <Toggle label="Then update the download page" checked={page} onChange={setPage} />
            </div>
          )}
          <Button variant="primary" size="lg" disabled={!todo.length} onClick={() => setReviewing(true)}>
            {todo.length ? `Publish ${todo.length} new build${todo.length > 1 ? "s" : ""}…` : "Nothing new to publish"}
          </Button>
        </div>
      </Card>
      {reviewing && (
        <PublishSheet
          title="Publish the latest builds"
          action="Sign and publish"
          progress={<UploadProgress />}
          cdn
          onClose={() => {
            setReviewing(false);
            scan();
            onDone();
          }}
          run={publishAll}
        >
          <ul className="flex flex-col gap-1 text-callout">
            {todo.map((b) => (
              <li key={b.kind + b.platform}>
                {APP[b.kind]} for {OS[b.platform]} <span className="font-semibold">{b.version}</span>
                <span className="text-muted"> (now {b.published ?? "none"})</span>
              </li>
            ))}
          </ul>
          <div className="mt-3">
            <Label text="Release notes (optional)">
              <TextArea rows={2} value={notes} onChange={(e) => setNotes(e.target.value)} />
            </Label>
          </div>
          <p className="mt-3 text-caption text-muted">
            Each build is signed with the update key and uploaded to every relay on channel {channel.trim()}: not mandatory, not forced, no minimum version (use Sign and publish below for those).
            {page && stable && " Then the download page is rebuilt with its saved title and text."}
          </p>
        </PublishSheet>
      )}
    </section>
  );
}

/** A DELETE the relay refused because the file is published (409, docs/release-cleanup.md §4.3). */
const published409 = (e: any) => /published release/.test(e?.message ?? "");

/** Clean-up settings (settings.prune): older versions kept, and the automatic run after each publish. */
function CleanupSettings({ onOpen }: { onOpen: () => void }) {
  const { state, reload, toast } = useApp();
  const s = state.settings;
  const [keep, setKeep] = useState<number>(s.prune?.keep ?? 2);
  const save = async (prune: { keep: number; auto: boolean }) => {
    await api.saveSettings({ ...s, prune });
    await reload();
    toast(prune.auto ? `Clean-up after each publish is on, keeping ${prune.keep} older version(s)` : "Clean-up settings saved");
  };
  const auto = !!s.prune?.auto;
  return (
    <Card className="mb-3">
      <div className="flex flex-wrap items-center justify-between gap-4">
        <div className="flex items-center gap-2 text-caption">
          Keep
          <TextField
            className="w-16"
            type="number"
            min={0}
            max={20}
            value={keep}
            aria-label="Older versions kept besides the published one"
            onChange={(e) => setKeep(Math.max(0, Math.min(20, Number(e.target.value) || 0)))}
            onBlur={() => keep !== (s.prune?.keep ?? 2) && save({ keep, auto })}
          />
          older version(s) besides the published one
        </div>
        <div className="flex items-center gap-3 text-caption">
          Clean up after each publish
          <Toggle label="Clean up after each publish" checked={auto} onChange={(v) => save({ keep, auto: v })} />
          <Button size="sm" variant="secondary" onClick={onOpen}>
            Clean up old releases…
          </Button>
        </div>
      </div>
      <p className="mt-2 text-footnote text-muted">
        Always kept: the version in the release index (also halted), the one in latest.json, newer ones, the older ones above, versions on the download page or the CDN, and files uploaded in the last 24 h.
      </p>
    </Card>
  );
}

export function Releases() {
  const { toast, state } = useApp();
  const [cleanup, setCleanup] = useState(false);
  const [file, setFile] = useState("");
  const [kind, setKind] = useState<Kind>("technician");
  const [platform, setPlatform] = useState<Platform>("windows-x86_64");
  const [channel, setChannel] = useState("stable");
  const [version, setVersion] = useState("");
  const [notes, setNotes] = useState("");
  const [mandatory, setMandatory] = useState(false);
  const [minimumVersion, setMinimumVersion] = useState("");
  const [forced, setForced] = useState(false);
  const [rollout, setRollout] = useState("standard");
  const [security, setSecurity] = useState(false);
  const [reviewing, setReviewing] = useState(false);
  const [list, setList] = useState<Release[] | null>(null);
  const [listError, setListError] = useState<string | null>(null);
  const [round, setRound] = useState(0);

  const load = () => api.admin<Release[]>("GET", "/api/admin/releases").then(setList, (e) => setListError(e.message));
  useEffect(() => void load(), []);

  const pick = async () => {
    const f = await open({ filters: [{ name: "Release file", extensions: ["exe", "gz", "zip"] }] });
    if (typeof f !== "string") return;
    setFile(f);
    const name = f.split(/[\\/]/).pop() ?? "";
    const known: [RegExp, Kind, Platform][] = [
      [/^Tether_(.+)_x64-setup\.exe$/i, "technician", "windows-x86_64"],
      [/^Tether_(.+)_universal\.app\.tar\.gz$/i, "technician", "darwin-universal"],
      [/^TetherQS-(.+)-universal\.zip$/i, "customer", "darwin-universal"],
      [/^TetherQS(?:-(.+))?\.exe$/i, "customer", "windows-x86_64"],
    ];
    for (const [re, k, p] of known) {
      const m = name.match(re);
      if (!m) continue;
      setKind(k);
      setPlatform(p);
      if (m[1]) setVersion(m[1]);
      break;
    }
  };

  const [openGroups, setOpenGroups] = useState<Set<string>>(new Set());
  const removeGroup = async (g: ReleaseGroup) => {
    const what = `${APP[g.dir.split("/")[0] as Kind] ?? g.dir} ${g.version} (${g.channel}, ${g.files.length} files)`;
    if (!(await ask(`Delete ${what} from every relay? Devices that still get this version from latest.json can no longer download it.`, { title: "Delete release", kind: "warning" }))) return;
    // Every file is tried; a failure does not leave the rest of the release behind.
    const failed: string[] = [];
    const guarded: string[] = [];
    const del = (path: string, force = false) => api.admin("DELETE", `/api/admin/release/${path}${force ? "?force=1" : ""}`);
    for (const f of g.files)
      await del(`${f.channel}/${f.path}`).catch((e: any) => {
        if (published409(e)) guarded.push(`${f.channel}/${f.path}`);
        else failed.push(`${f.path}: ${e.message}`);
      });
    if (guarded.length && (await ask(`${g.version} is the published release (in the release index or latest.json). Devices that update or reinstall now get a missing file until you publish again. Delete it anyway?`, { title: "Delete the published release", kind: "warning", okLabel: "Delete anyway" })))
      for (const p of guarded) await del(p, true).catch((e: any) => failed.push(`${p}: ${e.message}`));
    setListError(failed.length ? `Not deleted: ${failed.join("; ")}` : null);
    load();
  };

  const remove = async (r: Release) => {
    if (!(await ask(`Delete ${r.channel}/${r.path} from every relay?`, { title: "Delete file", kind: "warning" }))) return;
    const path = `/api/admin/release/${r.channel}/${r.path}`;
    try {
      await api.admin("DELETE", path).catch(async (e: any) => {
        if (!published409(e)) throw e;
        if (!(await ask(`${r.path.split("/").pop()} is what devices get now (the published release, latest.json or the release index). Deleting it breaks updates until you publish again. Delete it anyway?`, { title: "Delete a published file", kind: "warning", okLabel: "Delete anyway" }))) return;
        await api.admin("DELETE", `${path}?force=1`);
      });
      load();
    } catch (e: any) {
      setListError(e.message);
    }
  };

  const target = fileName(kind, platform, version || "<version>");

  return (
    <>
      <PageHeader title="Releases" sub="What each relay publishes, and signing new installers with the update key." />
      <LatestBuilds onDone={() => setRound((r) => r + 1)} />
      <Overview key={round} />
      <Rollouts key={`r${round}`} />

      <h2 className="mb-3 text-headline font-semibold">Sign and publish a new release</h2>
      <Card>
        <div className="grid grid-cols-2 gap-x-6 gap-y-4">
          <Label
            text="File"
            hint={
              file
                ? `Uploaded as ${channel}/${kind}/${platform}/${target}${kind === "technician" && platform === "darwin-universal" ? " with the .dmg next to it" : ""}`
                : "Windows: Tether_<version>_x64-setup.exe or TetherQS-<version>.exe. macOS: Tether_<version>_universal.app.tar.gz (its .dmg must be in the same folder) or TetherQS-<version>-universal.zip"
            }
          >
            <div className="flex gap-2">
              <TextField readOnly value={file} placeholder="No file chosen" />
              <Button onClick={pick}>Choose…</Button>
            </div>
          </Label>
          <Label text="App">
            <Segmented<Kind> value={kind} onChange={setKind} options={[{ value: "technician", label: "Technician app" }, { value: "customer", label: "Customer module" }]} />
          </Label>
          <Label text="Platform">
            <Segmented<Platform> value={platform} onChange={setPlatform} options={[{ value: "windows-x86_64", label: "Windows" }, { value: "darwin-universal", label: "macOS" }]} />
          </Label>
          <Label text="Version">
            <TextField value={version} onChange={(e) => setVersion(e.target.value)} placeholder="0.3.1" />
          </Label>
          <Label text="Channel" hint="Devices follow the policy setting update.channel.">
            <TextField value={channel} onChange={(e) => setChannel(e.target.value)} />
          </Label>
          <div className="col-span-2">
            <Label text="Release notes">
              <TextArea rows={3} value={notes} onChange={(e) => setNotes(e.target.value)} />
            </Label>
          </div>
          <div className="flex items-center justify-between gap-4">
            <div>
              <p className="text-caption font-semibold">Mandatory</p>
              <p className="text-footnote text-muted">Must be installed before the app can be used for sessions.</p>
            </div>
            <Toggle label="Mandatory" checked={mandatory} onChange={setMandatory} />
          </div>
          <div className="flex items-center justify-between gap-4">
            <div>
              <p className="text-caption font-semibold">Forced</p>
              <p className="text-footnote text-muted">Install without asking after the policy countdown.</p>
            </div>
            <Toggle label="Forced" checked={forced} onChange={setForced} />
          </div>
          <Label text="Minimum version" hint="Older versions are blocked. Leave empty for none.">
            <TextField value={minimumVersion} onChange={(e) => setMinimumVersion(e.target.value)} placeholder="0.3.0" />
          </Label>
          <Label text="Rollout" hint={describeRamp(rampPresets[rollout as keyof typeof rampPresets])}>
            <Segmented<string> value={rollout} onChange={setRollout} options={[{ value: "standard", label: "Standard" }, { value: "fast", label: "Fast" }, { value: "now", label: "Immediately" }]} />
          </Label>
          <div className="flex items-center justify-between gap-4">
            <div>
              <p className="text-caption font-semibold">Security release</p>
              <p className="text-footnote text-muted">Skips the rollout: every device gets it at once.</p>
            </div>
            <Toggle label="Security release" checked={security} onChange={setSecurity} />
          </div>
        </div>
        <div className="mt-6 flex justify-end">
          <Button variant="primary" size="lg" disabled={!file || !version.trim()} onClick={() => setReviewing(true)}>
            Review and publish…
          </Button>
        </div>
      </Card>
      {reviewing && (
        <PublishSheet
          title="Sign and publish release"
          action="Sign and publish"
          progress={<UploadProgress />}
          cdn
          onClose={() => {
            setReviewing(false);
            setRound((r) => r + 1);
            load();
          }}
          run={async () => {
            const r = await invoke<RelayResult[]>("release_publish", {
              args: { file, kind, platform, channel: channel.trim(), version: version.trim(), notes, mandatory, minimum_version: minimumVersion.trim(), forced, rollout, security },
            });
            toast(`Released ${version} to ${channel}`);
            setFile("");
            return r;
          }}
        >
          <dl className="grid grid-cols-[140px_1fr] gap-x-4 gap-y-1.5 text-caption">
            <dt className="text-muted">Release</dt>
            <dd className="font-semibold">
              {APP[kind]} {version.trim()} for {OS[platform]}
            </dd>
            <dt className="text-muted">Channel</dt>
            <dd>{channel.trim()}</dd>
            <dt className="text-muted">File</dt>
            <dd className="break-all font-mono text-footnote">{target}</dd>
            <dt className="text-muted">Flags</dt>
            <dd>{[mandatory && "mandatory", forced && "forced", minimumVersion.trim() && `minimum ${minimumVersion.trim()}`].filter(Boolean).join(", ") || "none"}</dd>
            <dt className="text-muted">Rollout</dt>
            <dd>{security ? "security release: everyone at once" : describeRamp(rampPresets[rollout as keyof typeof rampPresets])}</dd>
          </dl>
          <p className="mt-3 text-caption text-muted">
            Signed with the update key, then uploaded to each relay (and the CDN, if set up): files first, then the signed release index, the legacy latest.json last. Online devices on the{" "}
            {channel.trim()} channel hear of it within about a minute.
          </p>
        </PublishSheet>
      )}

      <div className="mb-3 mt-10 flex items-center justify-between">
        <div>
          <h2 className="text-headline font-semibold">Releases on the first relay</h2>
          <p className="mt-1 text-caption text-muted">One line per release; open it for its files (installer, signature, disk image, updater archive).</p>
        </div>
        <IconButton label="Refresh" onClick={load}>
          <RefreshCw size={16} />
        </IconButton>
      </div>
      <CleanupSettings onOpen={() => setCleanup(true)} />
      {cleanup && (
        <CleanupSheet
          keepDefault={state.settings.prune?.keep ?? 2}
          onClose={() => {
            setCleanup(false);
            setRound((r) => r + 1);
            load();
          }}
        />
      )}
      <div className="mb-3">
        <ErrorText error={listError} />
      </div>
      <Table head={["", "Channel", "App", "Platform", "Version", "Files", "Size", "Modified", ""]} empty={list && !list.length ? "No releases yet." : false}>
        {groupReleases(list ?? []).map((g) => {
          const [kind, platform] = g.dir.split("/") as [Kind, Platform];
          const isOpen = openGroups.has(g.key);
          const toggle = () => setOpenGroups((o) => (o.has(g.key) ? (o.delete(g.key), new Set(o)) : new Set(o).add(g.key)));
          return (
            <Fragment key={g.key}>
              <tr className="cursor-pointer" onClick={toggle}>
                <td className="w-8">
                  <button aria-label={isOpen ? "Hide files" : "Show files"} aria-expanded={isOpen} className="grid h-7 w-7 place-items-center rounded-md text-muted hover:bg-tile-2" onClick={(e) => (e.stopPropagation(), toggle())}>
                    <ChevronRight size={15} className={`transition-transform ${isOpen ? "rotate-90" : ""}`} />
                  </button>
                </td>
                <td>{g.channel}</td>
                <td>{APP[kind] ?? kind}</td>
                <td>{OS[platform] ?? platform}</td>
                <td className="font-semibold">{g.version ?? <span className="font-normal text-muted">latest.json (what devices get)</span>}</td>
                <td>{g.files.length}</td>
                <td>{bytes(g.size)}</td>
                <td>{when(g.modified)}</td>
                <td className="w-12">
                  {g.version && (
                    <IconButton label={`Delete release ${g.version}`} onClick={(e) => (e.stopPropagation(), removeGroup(g))}>
                      <Trash2 size={15} />
                    </IconButton>
                  )}
                </td>
              </tr>
              {isOpen &&
                g.files.map((r) => (
                  <tr key={r.path} className="bg-tile-2/50">
                    <td />
                    <td colSpan={4} className="select-text pl-6 font-mono text-footnote">
                      {r.path.split("/").pop()}
                    </td>
                    <td />
                    <td>{bytes(r.size)}</td>
                    <td>{when(r.modified)}</td>
                    <td className="w-12">
                      <IconButton label={`Delete ${r.path}`} onClick={() => remove(r)}>
                        <Trash2 size={15} />
                      </IconButton>
                    </td>
                  </tr>
                ))}
            </Fragment>
          );
        })}
      </Table>
    </>
  );
}

/** The signed release index per channel: rollout progress, halt and resume (docs/updates.md). */
function Rollouts() {
  const { toast } = useApp();
  const [channel, setChannel] = useState("stable");
  const [view, setView] = useState<IndexView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState("");
  const load = (ch = channel) => invoke<IndexView>("release_index", { channel: ch.trim() }).then(setView, (e) => setError(e.message));
  useEffect(() => void load(), []);
  const control = async (key: string, action: string, preset?: string) => {
    const what = action === "halt" ? "Halt" : action === "resume" ? "Resume" : "Restart the rollout of";
    if (!(await ask(`${what} ${key} on ${channel}? A new signed release index goes to every relay. Halting reaches devices in about 1 minute; devices that installed it keep it.`, { title: "Rollout", kind: "warning" }))) return;
    setBusy(key);
    try {
      await invoke<RelayResult[]>("release_control", { channel: channel.trim(), key, action, preset });
      // The download page is built when it is published: it keeps linking a halted release.
      toast(action === "halt" && channel.trim() === "stable" ? `Halt ${key}: done. Publish the download page again so it no longer offers this release.` : `${what} ${key}: done`);
      await load();
    } catch (e: any) {
      setError(e.message);
    } finally {
      setBusy("");
    }
  };
  return (
    <section className="mb-10">
      <div className="mb-3 flex items-end justify-between gap-4">
        <div>
          <h2 className="text-headline font-semibold">Rollouts</h2>
          <p className="mt-1 text-caption text-muted">
            The signed release index devices follow. Serial {view?.serial ?? "–"}
            {view?.days_left !== undefined && ` · valid for ${view.days_left} more days`}
            {view?.relays.map((r) => ` · ${host(r.server)}: ${r.error ? "not verified" : (r.serial ?? "none")}`).join("")}
          </p>
          {view?.stale && <ErrorText error="The release index has expired: devices show 'update information is out of date'. Unlock the keys to re-sign it." />}
          {view?.days_left !== undefined && view.days_left < 7 && !view.stale && <p className="text-caption text-warn">The release index expires in {view.days_left} days; it is re-signed when you unlock the keys.</p>}
        </div>
        <div className="flex items-center gap-2">
          <TextField className="w-28" value={channel} onChange={(e) => setChannel(e.target.value)} onKeyDown={(e) => e.key === "Enter" && load()} aria-label="Channel" />
          <IconButton label="Refresh" onClick={() => load()}>
            <RefreshCw size={16} />
          </IconButton>
        </div>
      </div>
      <ErrorText error={error} />
      <Table head={["App", "Platform", "Version", "Rollout", "Ramp", ""]} empty={view && !view.entries.length ? "No signed release index on this channel yet: the next publish creates it." : false}>
        {view?.entries.map((e) => {
          const [kind, platform] = e.key.split("/") as [Kind, Platform];
          return (
            <tr key={e.key}>
              <td>{APP[kind] ?? kind}</td>
              <td>{OS[platform] ?? platform}</td>
              <td className="font-semibold">{e.version}</td>
              <td>
                <Badge tone={e.halted ? "danger" : e.percent >= 100 || e.security ? "ok" : "warn"}>{rolloutState(e)}</Badge>
                {e.cdn && <span className="ml-2 text-footnote text-muted">CDN</span>}
              </td>
              <td className="text-footnote text-muted">{e.security ? "–" : `${describeRamp(e.steps)} from ${when(e.start)}`}</td>
              <td className="whitespace-nowrap text-right">
                {e.halted ? (
                  <Button size="sm" disabled={!!busy} onClick={() => control(e.key, "resume")}>
                    Resume
                  </Button>
                ) : (
                  <Button size="sm" disabled={!!busy} onClick={() => control(e.key, "halt")}>
                    Halt
                  </Button>
                )}{" "}
                {!e.security && e.percent < 100 && (
                  <Button size="sm" variant="ghost" disabled={!!busy} onClick={() => control(e.key, "ramp", "now")}>
                    Everyone now
                  </Button>
                )}
              </td>
            </tr>
          );
        })}
      </Table>
      <CdnSettings />
    </section>
  );
}

/** Cloudflare Workers Static Assets for downloads (D1). The token is sealed in the key vault. */
function CdnSettings() {
  const { toast } = useApp();
  const [s, setS] = useState<{ enabled: boolean; base: string; worker: string; credentials: string | null | undefined } | null>(null);
  const [envFile, setEnvFile] = useState("");
  const [pass, setPass] = useState("");
  const [error, setError] = useState<string | null>(null);
  useEffect(() => void invoke<typeof s>("cdn_state").then(setS, () => {}), []);
  if (!s) return null;
  const save = async (enabled: boolean) => {
    setError(null);
    try {
      await invoke("cdn_configure", { enabled, base: s.base.trim(), worker: s.worker.trim(), envFile: envFile.trim() || null, passphrase: pass || null });
      setS(await invoke("cdn_state"));
      setPass("");
      setEnvFile(""); // stored now: the toggle must not re-send the file without the passphrase
      toast(enabled ? "CDN on: the next publish deploys to Cloudflare" : "CDN settings saved");
    } catch (e: any) {
      setError(e.message);
    }
  };
  return (
    <Card className="mt-4">
      <div className="flex items-center justify-between gap-4">
        <div>
          <p className="text-caption font-semibold">Download CDN (Cloudflare Workers, free plan)</p>
          <p className="text-footnote text-muted">
            Each publish deploys the update files (max. 25 MiB each; TetherQS as .gz) as a new version of the Worker. Devices verify every byte and fall back to the relays. Credentials:{" "}
            {s.credentials ? `account ${s.credentials.slice(0, 6)}…` : "not stored"}
          </p>
        </div>
        <Toggle label="CDN" checked={s.enabled} onChange={(v) => save(v)} />
      </div>
      <div className="mt-3 grid grid-cols-2 gap-x-6 gap-y-3">
        <Label text="Address">
          <TextField value={s.base} onChange={(e) => setS({ ...s, base: e.target.value })} />
        </Label>
        <Label text="Worker">
          <TextField value={s.worker} onChange={(e) => setS({ ...s, worker: e.target.value })} />
        </Label>
        <Label text="Credentials file" hint="cf.env with CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID; stored sealed with the key passphrase.">
          <div className="flex gap-2">
            <TextField readOnly value={envFile} placeholder="No file chosen" />
            <Button onClick={async () => { const f = await open({ filters: [{ name: "Env file", extensions: ["env", "txt"] }] }); if (typeof f === "string") setEnvFile(f); }}>Choose…</Button>
          </div>
        </Label>
        <Label text="Key passphrase">
          <TextField type="password" value={pass} onChange={(e) => setPass(e.target.value)} />
        </Label>
      </div>
      <ErrorText error={error} />
      <div className="mt-3 flex justify-end">
        <Button onClick={() => save(s.enabled)}>Save</Button>
      </div>
    </Card>
  );
}

import { useEffect, useMemo, useState } from "react";
import { ask } from "@tauri-apps/plugin-dialog";
import { Lock, EyeOff, Plus, RotateCcw, Trash2 } from "lucide-react";
import { useApp } from "../App";
import { api, host, invoke, PROFILES, when, type Bundle, type Device, type Effective, type Feature, type Layer, type Rule } from "../lib/ipc";
import { byArea, FeatureRow, sameRule, shown, type Lang } from "../components/features";
import { ChangeList } from "../components/publish";
import { GrantRow, PendingLines } from "../components/grant";
import { GRANT, layerKey, type GrantsInfo } from "../lib/grants";
import { liveCount, unchecked } from "../lib/live";
import { Badge, Button, Card, ErrorText, IconButton, Label, Modal, PageHeader, SELECTED, Segmented, Select, StatusDot, Table, TextField } from "../components/ui";

type Scope = { kind: "profile"; name: string } | { kind: "group"; name: string };
type Tab = "editor" | "live" | "history";

function Preview({ draft, profile, devices }: { draft: Bundle; profile: string; devices: Device[] }) {
  const [device, setDevice] = useState("");
  const [eff, setEff] = useState<Effective | null>(null);
  const own = devices.filter((d) => d.profile === profile);
  useEffect(() => {
    invoke<Effective>("policy_preview", { bundle: draft, profile, device }).then(setEff, () => setEff(null));
  }, [draft, profile, device]);
  return (
    <Card className="sticky top-0">
      <h2 className="text-headline font-semibold">Effective draft</h2>
      <p className="mb-3 mt-1 text-caption text-muted">What a {profile} device gets once this draft is published.</p>
      <Select value={device} onChange={(e) => setDevice(e.target.value)} aria-label="Device for preview">
        <option value="">Any device without a group</option>
        {own.map((d) => (
          <option key={d.uuid} value={d.uuid}>
            {d.id || d.uuid} {draft.assignments[d.uuid] ? `· ${draft.assignments[d.uuid]}` : ""}
          </option>
        ))}
      </Select>
      {eff && (
        <>
          <p className="mt-3 text-footnote text-muted">Layers: {eff.sources.join(" → ")}</p>
          <ul className="mt-2 max-h-[60vh] overflow-auto">
            {Object.entries(eff.values).map(([id, v]) => (
              <li key={id} className="flex items-center gap-2 py-1 text-footnote">
                <span className={`min-w-0 flex-1 truncate font-mono ${eff.hidden.includes(id) ? "text-faint line-through" : ""}`}>{id}</span>
                <span className="font-mono">{shown(v)}</span>
                {eff.hidden.includes(id) ? <EyeOff size={12} className="text-muted" /> : eff.locked.includes(id) ? <Lock size={12} className="text-muted" /> : <span className="w-3" />}
              </li>
            ))}
          </ul>
        </>
      )}
    </Card>
  );
}

/** Read-only: what the relays serve now, per profile, with sources, and the draft beside it. */
function LiveView({ features, lang }: { features: Feature[]; lang: Lang }) {
  const { live, liveError, draft, changes, reloadLive } = useApp();
  const [profile, setProfile] = useState<string>("technician");
  const [onlyDiff, setOnlyDiff] = useState<"all" | "diff">("all");
  const [draftEff, setDraftEff] = useState<Effective | null>(null);
  useEffect(() => {
    invoke<Effective>("policy_preview", { bundle: draft, profile, device: "" }).then(setDraftEff, () => setDraftEff(null));
  }, [draft, profile]);
  if (!live)
    return liveError ? (
      <div className="flex items-center gap-3">
        <ErrorText error={`Could not read the live policy: ${liveError}`} />
        <Button size="sm" onClick={reloadLive}>
          Check again
        </Button>
      </div>
    ) : (
      <p className="text-caption text-muted">Reading the live policy from every relay…</p>
    );
  const { on, of } = liveCount(live);
  const name = (id: string) => features.find((f) => f.id === id)?.[lang] ?? id;
  const rows = (live.effective[profile] ?? []).filter((r) => onlyDiff === "all" || draftEff?.values[r.id] !== r.value);

  return (
    <div className="flex flex-col gap-8">
      <section>
        <div className="mb-3 flex items-end justify-between">
          <div>
            <h2 className="text-headline font-semibold">{live.serial != null ? `Serial ${live.serial} · live on ${on} of ${of} relays` : unchecked(live) ? "Could not check: no relay answered with a policy" : "Nothing published yet"}</h2>
            <p className="mt-1 text-caption text-muted">Read from each relay's public GET /api/policy and verified with this app's admin key{live.issued_at ? ` · issued ${when(live.issued_at)}` : ""}.</p>
          </div>
          <Button size="sm" onClick={reloadLive}>
            Check again
          </Button>
        </div>
        <Table head={["Relay", "Reachable", "Serves serial", "Issued", "Signature"]}>
          {live.relays.map((r) => (
            <tr key={r.server}>
              <td className="font-semibold">{host(r.server)}</td>
              <td>
                <span className="flex items-center gap-2" title={r.error ?? undefined}>
                  <StatusDot tone={r.reachable ? "ok" : "danger"} />
                  {r.reachable ? "Yes" : "No"}
                </span>
              </td>
              <td>
                {r.serial ?? "–"}{" "}
                {r.serial != null && r.serial === live.serial && r.verified ? <Badge tone="ok">Live</Badge> : live.serial != null && <Badge tone="warn">Behind</Badge>}
              </td>
              <td>{when(r.issued_at)}</td>
              <td className="max-w-[320px]">{r.verified ? <Badge tone="ok">Verified</Badge> : <span className="text-caption text-warn">{r.note ?? "–"}</span>}</td>
            </tr>
          ))}
        </Table>
      </section>

      <section>
        <h2 className="mb-1 text-headline font-semibold">Draft compared with live</h2>
        <p className="mb-3 text-caption text-muted">What Publish would change.</p>
        <ChangeList changes={changes} empty="The draft is the live policy." />
      </section>

      <section>
        <div className="mb-3 flex flex-wrap items-end justify-between gap-3">
          <div>
            <h2 className="text-headline font-semibold">Effective live values</h2>
            <p className="mt-1 text-caption text-muted">What a device without a group gets now (groups and machine policy can override).</p>
          </div>
          <div className="flex gap-2">
            <Segmented label="Profile" value={profile} onChange={setProfile} options={PROFILES.map((p) => ({ value: p, label: p === "technician" ? "Technician" : "Customer" }))} />
            <Segmented<"all" | "diff"> label="Rows" value={onlyDiff} onChange={setOnlyDiff} options={[{ value: "all", label: "All" }, { value: "diff", label: "Differs from draft" }]} />
          </div>
        </div>
        <Table head={["Feature", "Live value", "Source", "Draft"]} empty={rows.length ? false : "Draft and live agree for this profile."}>
          {rows.map((r) => {
            const d = draftEff?.values[r.id];
            return (
              <tr key={r.id}>
                <td>
                  <p className={r.hidden ? "text-faint line-through" : ""}>{name(r.id)}</p>
                  <p className="font-mono text-footnote text-muted">{r.id}</p>
                </td>
                <td>
                  <span className="flex items-center gap-1.5 font-mono">
                    {shown(r.value)}
                    {r.hidden ? <EyeOff size={12} className="text-muted" aria-label="Hidden" /> : r.locked && <Lock size={12} className="text-muted" aria-label="Locked" />}
                  </span>
                </td>
                <td>{r.source === "profile" ? <Badge tone="ink">Profile</Badge> : <Badge>Default</Badge>}</td>
                <td>{d === undefined || d === r.value ? <span className="text-muted">same</span> : <span className="font-mono font-semibold">{shown(d)}</span>}</td>
              </tr>
            );
          })}
        </Table>
      </section>
    </div>
  );
}

interface HistoryRow {
  serial: number;
  issued_at: number | null;
  server: boolean;
  local: boolean;
}

function History() {
  const { setDraft, live, toast } = useApp();
  const [history, setHistory] = useState<HistoryRow[]>([]);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    invoke<HistoryRow[]>("policy_history").then(setHistory, (e) => setError(e.message));
  }, [live?.serial]);
  const load = async (serial: number) => {
    if (!(await ask(`Load the content of version ${serial} into the draft? Unpublished changes in the draft are replaced. Nothing is published until you press Publish.`, { title: "Load version" }))) return;
    try {
      setDraft(await invoke<Bundle>("policy_local", { serial }));
      toast(`Version ${serial} loaded into the draft. Review and publish.`);
    } catch (e: any) {
      setError(e.message);
    }
  };
  return (
    <>
      <ErrorText error={error} />
      <Card className="py-2">
        {history.map((h) => (
          <div key={h.serial} className="flex items-center gap-4 border-b border-hairline py-2 text-caption last:border-0">
            <span className="w-24 font-semibold">Version {h.serial}</span>
            {h.serial === live?.serial && <Badge tone="ok">Live</Badge>}
            <span className="flex-1 text-muted">{when(h.issued_at)}</span>
            <span className="text-footnote text-muted">{h.server ? "on the relay" : "only on this PC"}</span>
            <Button size="sm" disabled={!h.server && !h.local} onClick={() => load(h.serial)}>
              <RotateCcw size={14} /> Load into draft
            </Button>
          </div>
        ))}
        {!history.length && <p className="py-4 text-center text-caption text-muted">Nothing published yet.</p>}
      </Card>
    </>
  );
}

export function Policy() {
  const { draft, setDraft, live, invalid, dirty, changes, toast } = useApp();
  const [features, setFeatures] = useState<Feature[]>([]);
  const [devices, setDevices] = useState<Device[]>([]);
  const [tab, setTab] = useState<Tab>("editor");
  const [scope, setScope] = useState<Scope>({ kind: "profile", name: "technician" });
  const [lang, setLang] = useState<Lang>("en");
  const [only, setOnly] = useState<"all" | "set" | "unpublished">("all");
  const [newGroup, setNewGroup] = useState<{ name: string; profile: string } | null>(null);
  const [grants, setGrants] = useState<GrantsInfo>({ layers: {}, pending: [] });
  const reloadGrants = () => invoke<GrantsInfo>("grants_info", { bundle: draft, live: live?.bundle ?? null }).then(setGrants, () => {});
  useEffect(() => void reloadGrants(), [draft, live]);
  // Still to revoke for a group that was deleted since: shown once, at the top of the health area.
  const orphans = grants.pending.filter((p) => p.layer.startsWith("group:") && !draft.groups[p.layer.slice(6)]);

  useEffect(() => {
    invoke<Feature[]>("features").then(setFeatures);
    api.devices().then((r) => setDevices(r.devices), () => setDevices([]));
  }, []);

  const group = scope.kind === "group" ? draft.groups[scope.name] : undefined;
  const profile = scope.kind === "profile" ? scope.name : group?.profile ?? "technician";
  const layer: Layer = (scope.kind === "profile" ? draft.profiles[scope.name] : group?.overrides) ?? {};
  const liveLayer: Layer = (scope.kind === "profile" ? live?.bundle?.profiles[scope.name] : live?.bundle?.groups[scope.name]?.overrides) ?? {};
  const liveRule = (id: string): Rule | null => liveLayer[id] ?? null;

  const areas = useMemo(
    () => byArea(features, profile, only === "all" ? undefined : only === "set" ? (f) => !!layer[f.id] : (f) => !sameRule(layer[f.id], liveLayer[f.id])),
    [features, profile, only, layer, liveLayer],
  );
  const setCount = Object.keys(layer).length;
  const unpublishedCount = features.filter((f) => f.profiles.includes(profile) && !sameRule(layer[f.id], liveLayer[f.id])).length;

  const setRule = (id: string, rule?: Rule) => {
    const next: Layer = { ...layer };
    if (rule) next[id] = rule;
    else delete next[id];
    setLayer(next);
  };
  const setLayer = (next: Layer) => {
    if (scope.kind === "profile") setDraft({ ...draft, profiles: { ...draft.profiles, [scope.name]: next } });
    else setDraft({ ...draft, groups: { ...draft.groups, [scope.name]: { ...group!, overrides: next } } });
  };

  const removeGroup = async (name: string) => {
    const n = Object.values(draft.assignments).filter((g) => g === name).length;
    if (!(await ask(`Delete group "${name}"${n ? ` and unassign ${n} device(s)` : ""}?`, { title: "Delete group", kind: "warning" }))) return;
    const groups = { ...draft.groups };
    delete groups[name];
    const assignments = Object.fromEntries(Object.entries(draft.assignments).filter(([, g]) => g !== name));
    setDraft({ ...draft, groups, assignments });
    setScope({ kind: "profile", name: "technician" });
  };

  const scopeBtn = (s: Scope, label: string, sub: string) => {
    const active = s.kind === scope.kind && s.name === scope.name;
    return (
      <button
        key={`${s.kind}:${s.name}`}
        role="tab"
        aria-selected={active}
        onClick={() => setScope(s)}
        className={`flex h-8 items-center gap-2 rounded-full border px-3.5 text-caption ${active ? `border-accent ${SELECTED}` : "border-hairline text-ink-80 hover:bg-tile-2"}`}
      >
        {label}
        <span className={`text-footnote ${active ? "text-on-accent/80" : "text-muted"}`}>{sub}</span>
      </button>
    );
  };

  return (
    <>
      <PageHeader title="Policy" sub="Edit the draft, compare it with what is live, publish from the bar above.">
        <Segmented<Lang> label="Label language" value={lang} onChange={setLang} options={[{ value: "en", label: "EN" }, { value: "de", label: "DE" }]} />
      </PageHeader>
      <div className="mb-6 flex items-center gap-4">
        <Segmented<Tab>
          label="View"
          value={tab}
          onChange={setTab}
          options={[
            { value: "editor", label: "Draft editor", count: changes.length || undefined },
            { value: "live", label: live?.serial != null ? `Live policy · serial ${live.serial}` : "Live policy" },
            { value: "history", label: "History" },
          ]}
        />
        {invalid ? (
          <ErrorText error={`Not publishable: ${invalid}`} />
        ) : (
          <p className="flex items-center gap-2 text-caption text-muted">
            <StatusDot tone={dirty ? "warn" : "ok"} />
            {dirty ? "Valid draft with unpublished changes" : "Draft matches the live policy"}
          </p>
        )}
      </div>

      {tab === "live" && <LiveView features={features} lang={lang} />}
      {tab === "history" && <History />}
      {tab === "editor" && (
        <>
          <div className="mb-4 flex flex-wrap items-center gap-2" role="tablist" aria-label="Layer">
            {PROFILES.map((p) => scopeBtn({ kind: "profile", name: p }, p === "technician" ? "Technician profile" : "Customer profile", `${Object.keys(draft.profiles[p] ?? {}).length} set`))}
            <span className="mx-1 h-5 w-px bg-hairline" />
            {Object.entries(draft.groups).map(([name, g]) => scopeBtn({ kind: "group", name }, name, `${g.profile} · ${Object.values(draft.assignments).filter((x) => x === name).length} devices`))}
            <Button size="sm" variant="ghost" onClick={() => setNewGroup({ name: "", profile: "technician" })}>
              <Plus size={14} /> New group
            </Button>
          </div>

          <div className="grid grid-cols-[minmax(0,1fr)_300px] items-start gap-6">
            <Card className="py-3">
              <div className="flex flex-wrap items-center justify-between gap-3 pb-2">
                <div>
                  <h2 className="text-headline font-semibold">{scope.kind === "profile" ? `${profile === "technician" ? "Technician" : "Customer"} profile` : `Group ${scope.name}`}</h2>
                  <p className="text-caption text-muted">{scope.kind === "group" ? `Overrides for assigned ${profile} devices. Assign devices on the Devices page.` : `Applies to every ${profile} device.`}</p>
                </div>
                <div className="flex items-center gap-2">
                  <Segmented
                    label="Show features"
                    value={only}
                    onChange={setOnly}
                    options={[
                      { value: "all", label: "All" },
                      { value: "set", label: "Changed only", count: setCount },
                      { value: "unpublished", label: "Not published", count: unpublishedCount },
                    ]}
                  />
                  {scope.kind === "group" && (
                    <IconButton label="Delete group" onClick={() => removeGroup(scope.name)}>
                      <Trash2 size={16} />
                    </IconButton>
                  )}
                </div>
              </div>
              {areas.map(([area, list]) => (
                <div key={area} className="mt-4">
                  <h3 className="mb-1 text-caption font-semibold capitalize text-muted">{area}</h3>
                  {area === "health" && <PendingLines pending={orphans} named onReload={reloadGrants} toast={toast} />}
                  {list.map((f) =>
                    f.id === GRANT ? (
                      <GrantRow
                        key={`${scope.kind}:${scope.name}`}
                        f={f}
                        lang={lang}
                        layerKey={layerKey(scope.kind, scope.name)}
                        layer={layer}
                        profile={scope.kind === "group" ? draft.profiles[profile] ?? {} : undefined}
                        info={grants.layers[layerKey(scope.kind, scope.name)]}
                        pending={grants.pending.filter((p) => p.layer === layerKey(scope.kind, scope.name))}
                        liveRevoked={!!grants.live_revoked?.includes(layerKey(scope.kind, scope.name))}
                        onChange={setLayer}
                        onReload={reloadGrants}
                        toast={toast}
                      />
                    ) : (
                      <FeatureRow key={f.id} f={f} rule={layer[f.id]} lang={lang} live={live ? liveRule(f.id) : undefined} onChange={(r) => setRule(f.id, r)} />
                    ),
                  )}
                </div>
              ))}
              {!areas.length && <p className="py-8 text-center text-caption text-muted">{only === "set" ? "Nothing set in this layer: every feature uses the default." : "This layer matches the live policy."}</p>}
            </Card>

            <Preview draft={draft} profile={profile} devices={devices} />
          </div>
        </>
      )}

      {newGroup && (
        <Modal onClose={() => setNewGroup(null)}>
          <h2 className="text-headline font-semibold">New group</h2>
          <form
            className="mt-4 flex flex-col gap-4"
            onSubmit={(e) => {
              e.preventDefault();
              const name = newGroup.name.trim();
              if (!name || name.length > 64 || draft.groups[name]) return;
              setDraft({ ...draft, groups: { ...draft.groups, [name]: { profile: newGroup.profile, overrides: {} } } });
              setScope({ kind: "group", name });
              setNewGroup(null);
            }}
          >
            <Label text="Name" hint={draft.groups[newGroup.name.trim()] ? "A group with this name exists." : "1–64 characters."}>
              <TextField data-autofocus maxLength={64} value={newGroup.name} onChange={(e) => setNewGroup({ ...newGroup, name: e.target.value })} />
            </Label>
            <Label text="Profile">
              <Select value={newGroup.profile} onChange={(e) => setNewGroup({ ...newGroup, profile: e.target.value })}>
                {PROFILES.map((p) => (
                  <option key={p}>{p}</option>
                ))}
              </Select>
            </Label>
            <div className="flex justify-end gap-2">
              <Button type="button" onClick={() => setNewGroup(null)}>
                Cancel
              </Button>
              <Button variant="primary" disabled={!newGroup.name.trim() || !!draft.groups[newGroup.name.trim()]}>
                Create
              </Button>
            </div>
          </form>
        </Modal>
      )}
    </>
  );
}

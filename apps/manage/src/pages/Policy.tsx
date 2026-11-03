import { useEffect, useMemo, useState } from "react";
import { ask } from "@tauri-apps/plugin-dialog";
import { Lock, EyeOff, Plus, RotateCcw, Trash2 } from "lucide-react";
import { useApp } from "../App";
import { api, invoke, PROFILES, when, type Bundle, type Device, type Effective, type Feature, type Layer, type Rule, type RuleState } from "../lib/ipc";
import { Button, Card, ErrorText, IconButton, Label, Modal, PageHeader, Segmented, Select, StatusDot, TextField, Toggle } from "../components/ui";

type Scope = { kind: "profile"; name: string } | { kind: "group"; name: string };
type Lang = "en" | "de";

const STATES: { value: RuleState | "default"; label: string }[] = [
  { value: "default", label: "Default" },
  { value: "on", label: "On" },
  { value: "off", label: "Off" },
  { value: "locked", label: "Locked" },
  { value: "hidden", label: "Hidden" },
];

function ValueEditor({ f, value, onChange }: { f: Feature; value: string; onChange: (v: string) => void }) {
  if (f.kind === "bool") return <Toggle label={f.en} checked={value === "Y"} onChange={(v) => onChange(v ? "Y" : "N")} />;
  if (f.kind === "choice")
    return (
      <Select value={value} onChange={(e) => onChange(e.target.value)} aria-label={f.en}>
        {f.choices.map((c) => (
          <option key={c}>{c}</option>
        ))}
      </Select>
    );
  return <TextField aria-label={f.en} type={f.kind === "number" ? "number" : "text"} min={0} value={value} onChange={(e) => onChange(e.target.value)} />;
}

function FeatureRow({ f, rule, lang, onChange }: { f: Feature; rule?: Rule; lang: Lang; onChange: (r?: Rule) => void }) {
  const state = rule?.state ?? "default";
  const showValue = rule && (rule.state === "on" || rule.state === "locked");
  return (
    <div className="flex flex-wrap items-center gap-x-4 gap-y-2 border-b border-hairline py-2.5 last:border-0">
      <div className="min-w-[180px] flex-1">
        <p className="text-caption" title={lang === "en" ? f.de : f.en}>
          {lang === "en" ? f.en : f.de}
        </p>
        <p className="font-mono text-footnote text-faint">
          {f.id} · default {f.kind === "bool" ? (f.default === "Y" ? "on" : "off") : f.default || "empty"}
        </p>
      </div>
      <div className="ml-auto flex items-center gap-3">
        {showValue && (
          <div className={f.kind === "bool" ? "" : "w-40"}>
            <ValueEditor f={f} value={rule.value ?? (f.kind === "bool" ? "Y" : f.default)} onChange={(value) => onChange({ ...rule, value })} />
          </div>
        )}
        <Segmented
          label={`State of ${f.en}`}
          value={state}
          options={STATES}
          onChange={(s) => onChange(s === "default" ? undefined : { state: s, ...(s === "on" || s === "locked" ? { value: rule?.value ?? (f.kind === "bool" ? "Y" : f.default) } : {}) })}
        />
      </div>
    </div>
  );
}

function Preview({ draft, profile, devices }: { draft: Bundle; profile: string; devices: Device[] }) {
  const [device, setDevice] = useState("");
  const [eff, setEff] = useState<Effective | null>(null);
  const own = devices.filter((d) => d.profile === profile);
  useEffect(() => {
    invoke<Effective>("policy_preview", { bundle: draft, profile, device }).then(setEff, () => setEff(null));
  }, [draft, profile, device]);
  return (
    <Card className="sticky top-0">
      <h2 className="text-headline font-semibold">Effective policy</h2>
      <p className="mb-3 mt-1 text-caption text-muted">What a {profile} device gets from this draft.</p>
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
                <span className="font-mono">{v === "" ? "–" : v === "Y" ? "on" : v === "N" ? "off" : v}</span>
                {eff.hidden.includes(id) ? <EyeOff size={12} className="text-muted" /> : eff.locked.includes(id) ? <Lock size={12} className="text-muted" /> : <span className="w-3" />}
              </li>
            ))}
          </ul>
        </>
      )}
    </Card>
  );
}

interface HistoryRow {
  serial: number;
  issued_at: number | null;
  server: boolean;
  local: boolean;
}

export function Policy() {
  const { draft, setDraft, dirty, markPublished, toast } = useApp();
  const [features, setFeatures] = useState<Feature[]>([]);
  const [devices, setDevices] = useState<Device[]>([]);
  const [scope, setScope] = useState<Scope>({ kind: "profile", name: "technician" });
  const [lang, setLang] = useState<Lang>("en");
  const [invalid, setInvalid] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [history, setHistory] = useState<HistoryRow[]>([]);
  const [newGroup, setNewGroup] = useState<{ name: string; profile: string } | null>(null);

  const loadHistory = () => invoke<HistoryRow[]>("policy_history").then(setHistory, (e) => setError(e.message));
  useEffect(() => {
    invoke<Feature[]>("features").then(setFeatures);
    api.devices().then(setDevices, () => setDevices([]));
    loadHistory();
  }, []);
  useEffect(() => {
    invoke("policy_check", { bundle: draft }).then(() => setInvalid(null), (e) => setInvalid(e.message));
  }, [draft]);

  const group = scope.kind === "group" ? draft.groups[scope.name] : undefined;
  const profile = scope.kind === "profile" ? scope.name : group?.profile ?? "technician";
  const layer: Layer = (scope.kind === "profile" ? draft.profiles[scope.name] : group?.overrides) ?? {};

  const areas = useMemo(() => {
    const m = new Map<string, Feature[]>();
    for (const f of features.filter((f) => f.profiles.includes(profile))) m.set(f.area, [...(m.get(f.area) ?? []), f]);
    return [...m];
  }, [features, profile]);

  const setRule = (id: string, rule?: Rule) => {
    const next: Layer = { ...layer };
    if (rule) next[id] = rule;
    else delete next[id];
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

  const publish = async (b: Bundle, what: string) => {
    setBusy(true);
    setError(null);
    try {
      const serial = await api.publish(b);
      markPublished({ ...b, serial, issued_at: Math.floor(Date.now() / 1000) });
      toast(`${what} as version ${serial}`);
      loadHistory();
    } catch (e: any) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  };

  const restore = async (serial: number) => {
    if (!(await ask(`Publish the content of version ${serial} again as a new version? Unpublished changes in the editor are replaced.`, { title: "Restore policy" }))) return;
    try {
      publish(await invoke<Bundle>("policy_local", { serial }), `Restored version ${serial}`);
    } catch (e: any) {
      setError(e.message);
    }
  };

  const scopeBtn = (s: Scope, label: string, sub: string) => {
    const active = s.kind === scope.kind && s.name === scope.name;
    return (
      <button
        key={`${s.kind}:${s.name}`}
        role="tab"
        aria-selected={active}
        onClick={() => setScope(s)}
        className={`flex h-8 items-center gap-2 rounded-full border px-3.5 text-caption ${active ? "border-accent bg-accent/12 text-ink" : "border-hairline text-ink-80 hover:bg-tile-2"}`}
      >
        {label}
        <span className="text-footnote text-muted">{sub}</span>
      </button>
    );
  };

  return (
    <>
      <PageHeader title="Policy" sub={`Last published version ${draft.serial || "–"} · ${when(draft.issued_at)}`}>
        <Segmented<Lang> label="Label language" value={lang} onChange={setLang} options={[{ value: "en", label: "EN" }, { value: "de", label: "DE" }]} />
        <Button variant="primary" disabled={busy || !!invalid || !dirty} onClick={() => publish(draft, "Published")}>
          {busy ? "Publishing…" : "Publish"}
        </Button>
      </PageHeader>
      <div className="mb-4 flex flex-col gap-2">
        {invalid ? <ErrorText error={`Not publishable: ${invalid}`} /> : <p className="flex items-center gap-2 text-caption text-muted"><StatusDot tone={dirty ? "warn" : "ok"} />{dirty ? "Valid draft with unpublished changes" : "Draft matches the published policy"}</p>}
        <ErrorText error={error} />
      </div>

      <div className="mb-4 flex flex-wrap items-center gap-2" role="tablist" aria-label="Layer">
        {PROFILES.map((p) => scopeBtn({ kind: "profile", name: p }, p === "technician" ? "Technician profile" : "Customer profile", `${Object.keys(draft.profiles[p] ?? {}).length}`))}
        <span className="mx-1 h-5 w-px bg-hairline" />
        {Object.entries(draft.groups).map(([name, g]) => scopeBtn({ kind: "group", name }, name, `${g.profile} · ${Object.values(draft.assignments).filter((x) => x === name).length} devices`))}
        <Button size="sm" variant="ghost" onClick={() => setNewGroup({ name: "", profile: "technician" })}>
          <Plus size={14} /> New group
        </Button>
      </div>

      <div className="grid grid-cols-[minmax(0,1fr)_300px] items-start gap-6">
        <Card className="py-3">
          <div className="flex items-center justify-between pb-2">
            <div>
              <h2 className="text-headline font-semibold">{scope.kind === "profile" ? `${profile === "technician" ? "Technician" : "Customer"} profile` : `Group ${scope.name}`}</h2>
              <p className="text-caption text-muted">{scope.kind === "group" ? `Overrides for assigned ${profile} devices. Assign devices on the Devices page.` : `Applies to every ${profile} device.`}</p>
            </div>
            {scope.kind === "group" && (
              <IconButton label="Delete group" onClick={() => removeGroup(scope.name)}>
                <Trash2 size={16} />
              </IconButton>
            )}
          </div>
          {areas.map(([area, list]) => (
            <div key={area} className="mt-4">
              <h3 className="text-caption font-semibold capitalize text-muted">{area}</h3>
              {list.map((f) => (
                <FeatureRow key={f.id} f={f} rule={layer[f.id]} lang={lang} onChange={(r) => setRule(f.id, r)} />
              ))}
            </div>
          ))}
        </Card>

        <Preview draft={draft} profile={profile} devices={devices} />
      </div>

      <h2 className="mb-3 mt-10 text-headline font-semibold">History</h2>
      <Card className="py-2">
        {history.map((h) => (
          <div key={h.serial} className="flex items-center gap-4 border-b border-hairline py-2 text-caption last:border-0">
            <span className="w-24 font-semibold">Version {h.serial}</span>
            <span className="flex-1 text-muted">{when(h.issued_at)}</span>
            <span className="text-footnote text-muted">{h.server ? "on server" : "local only"}</span>
            <Button size="sm" disabled={busy || !h.server && !h.local} onClick={() => restore(h.serial)}>
              <RotateCcw size={14} /> Restore
            </Button>
          </div>
        ))}
        {!history.length && <p className="py-4 text-center text-caption text-muted">Nothing published yet.</p>}
      </Card>

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
              <TextField autoFocus maxLength={64} value={newGroup.name} onChange={(e) => setNewGroup({ ...newGroup, name: e.target.value })} />
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

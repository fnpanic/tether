// Advanced mode dialogs: bulk tags, rules, import/export, team, add host, sessions, help.
import { useEffect, useMemo, useState } from "react";
import { Copy, Keyboard, Lock, Plus, Power, RefreshCw, Trash2, Users } from "lucide-react";
import { Button, IconButton, Modal, Segmented, TextField, Toggle } from "../ui";
import { formatId, inTauri, invoke } from "../../lib/ipc";
import { label, newId, normTag, TAG_COLORS, tagColor, type Host, type Store, type TagColor } from "../../lib/hosts/model";
import { exportCsv, exportFaultsCsv, exportJson, mergeImport, parseImport, type Imported } from "../../lib/hosts/io";
import { parse, search } from "../../lib/hosts/query";
import { actions, teamSync, useHosts, useReadOnly } from "../../lib/hosts/store";
import { errorText } from "../../lib/policy";
import { ago, Kbd, TagChip, TagDot, TagInput } from "./bits";

const H = ({ children }: { children: React.ReactNode }) => <h2 className="text-headline font-semibold">{children}</h2>;
const P = ({ children }: { children: React.ReactNode }) => <p className="mt-1 text-caption text-muted">{children}</p>;
/** Shown instead of the controls for what a team shares, to a member who may not change it. */
const Managed = ({ what }: { what: string }) => <p className="mt-4 rounded-xl bg-parchment px-4 py-3 text-caption text-muted">Managed by your team: {what} can only be changed by members with write permission.</p>;

/** Add/remove tags on many hosts. Shows how many of the selection carry each tag now. */
export function BulkTags({ hosts, meta, known, onClose }: { hosts: Host[]; meta: Store["tags"]; known: string[]; onClose: () => void }) {
  const ids = hosts.map((h) => h.id);
  const ro = useReadOnly();
  const counts = useMemo(() => {
    const m = new Map<string, number>();
    for (const h of hosts) for (const t of h.tags) m.set(t, (m.get(t) ?? 0) + 1);
    return [...m.entries()].sort((a, b) => b[1] - a[1]);
  }, [hosts]);
  return (
    <Modal onClose={onClose} width={480}>
      <H>Tags for {ids.length.toLocaleString()} hosts</H>
      <P>Added tags go on every selected host. Remove takes a tag off all of them.</P>
      {ro ? (
        <Managed what="tags" />
      ) : (
        <div className="mt-4">
          <TagInput autoFocus known={known} onAdd={(t) => actions.addTags(ids, [t])} placeholder="Add tag to all (customer:acme)" />
        </div>
      )}
      <div className="mt-4 max-h-[280px] space-y-1 overflow-y-auto" aria-label="Tags in the selection">
        {counts.length === 0 && <div className="text-caption text-muted">The selected hosts have no tags yet.</div>}
        {counts.map(([t, n]) => (
          <div key={t} className="flex h-8 items-center gap-2 rounded-md px-1 hover:bg-tile-2">
            <TagChip tag={t} meta={meta} />
            <span className="text-footnote text-muted">{n === ids.length ? "all" : `${n.toLocaleString()} of ${ids.length.toLocaleString()}`}</span>
            <span className="flex-1" />
            {n < ids.length && !ro && (
              <Button size="sm" variant="ghost" onClick={() => actions.addTags(ids, [t])}>
                Add to all
              </Button>
            )}
            {!ro && (
              <Button size="sm" variant="ghost" className="!text-danger" onClick={() => actions.removeTags(ids, [t])}>
                Remove
              </Button>
            )}
          </div>
        ))}
      </div>
      <div className="mt-6 flex justify-end">
        <Button variant="primary" onClick={onClose}>
          Done
        </Button>
      </div>
    </Modal>
  );
}

export function TagColors({ tags, meta, onClose }: { tags: string[]; meta: Store["tags"]; onClose: () => void }) {
  const nss = [...new Set(tags.map((t) => (t.includes(":") ? t.slice(0, t.indexOf(":")) : t)))].sort();
  const setColor = (k: string, color: TagColor) => actions.setStore((s) => ({ ...s, tags: { ...s.tags, [k]: { ...s.tags[k], color } } }));
  const ro = useReadOnly();
  return (
    <Modal onClose={onClose} width={460}>
      <H>Tag colours</H>
      <P>A colour set on a namespace (site) applies to all its tags (site:berlin, site:munich).</P>
      {ro && <Managed what="tag colours" />}
      <div className="mt-4 max-h-[360px] space-y-1 overflow-y-auto">
        {nss.map((k) => (
          <div key={k} className="flex h-9 items-center gap-3">
            <span className="w-32 truncate text-caption">{k}</span>
            <div className="flex gap-1" role="radiogroup" aria-label={`Colour of ${k}`}>
              {TAG_COLORS.map((c) => (
                <button
                  key={c}
                  role="radio"
                  aria-checked={tagColor(k, meta) === c}
                  aria-label={c}
                  disabled={ro}
                  onClick={() => setColor(k, c)}
                  className={`grid h-6 w-6 place-items-center rounded-full ${tagColor(k, meta) === c ? "ring-2 ring-focus" : ""}`}
                >
                  <span className="h-3 w-3 rounded-full" style={{ background: `var(--tag-${c})` }} />
                </button>
              ))}
            </div>
          </div>
        ))}
      </div>
      <div className="mt-6 flex justify-end">
        <Button variant="primary" onClick={onClose}>
          Done
        </Button>
      </div>
    </Modal>
  );
}

export function Rules({ hosts, store, onClose }: { hosts: Host[]; store: Store; onClose: () => void }) {
  const [tag, setTag] = useState("");
  const [when, setWhen] = useState("");
  const ro = useReadOnly();
  // As a rule reads it: health terms are plain words there (older desktops of the team read them so).
  const p = useMemo(() => parse(when, true), [when]);
  const n = useMemo(() => (p.node ? search(hosts, p, undefined, true).length : 0), [hosts, p]);
  const add = () => {
    const t = normTag(tag);
    if (!t || !p.node) return;
    actions.setStore((s) => ({ ...s, rules: [...s.rules, { id: newId(), tag: t, when: when.trim() }] }));
    setTag("");
    setWhen("");
  };
  const counts = useMemo(() => new Map(store.rules.map((r) => [r.id, search(hosts, `#${r.tag}`).length])), [store.rules, hosts]);
  return (
    <Modal onClose={onClose} width={620}>
      <H>Tag rules</H>
      <P>Hosts that match a search get its tag automatically (dashed chip). Rules use the same syntax as the search box and update as hosts change.</P>
      <div className="mt-4 space-y-1">
        {store.rules.length === 0 && <div className="text-caption text-muted">No rules yet. Example: hosts whose name starts with BER- get site:berlin.</div>}
        {store.rules.map((r) => (
          <div key={r.id} className="flex h-10 items-center gap-3 rounded-md px-2 hover:bg-tile-2">
            <Toggle
              label={`Rule ${r.tag} on`}
              disabled={ro}
              checked={!r.off}
              onChange={(v) => actions.setStore((s) => ({ ...s, rules: s.rules.map((x) => (x.id === r.id ? { ...x, off: !v } : x)) }))}
            />
            <TagChip tag={r.tag} meta={store.tags} />
            <span className="text-footnote text-muted">when</span>
            <code className="min-w-0 flex-1 truncate font-mono text-footnote">{r.when}</code>
            <span className="text-footnote text-muted">{r.off ? "off" : `${(counts.get(r.id) ?? 0).toLocaleString()} hosts`}</span>
            <IconButton label={`Delete rule ${r.tag}`} className="disabled:opacity-40" disabled={ro} onClick={() => actions.setStore((s) => ({ ...s, rules: s.rules.filter((x) => x.id !== r.id) }))}>
              <Trash2 size={15} />
            </IconButton>
          </div>
        ))}
      </div>
      {ro && <Managed what="tag rules" />}
      <form
        hidden={ro}
        className="mt-5 grid grid-cols-[160px_1fr_auto] items-start gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          add();
        }}
      >
        <TextField className="!h-9 !text-caption" placeholder="Tag (site:berlin)" aria-label="Rule tag" value={tag} onChange={(e) => setTag(e.target.value.slice(0, 64))} />
        <div>
          <TextField className="!h-9 font-mono !text-caption" placeholder="e.g. host:BER- or os:mac" title="Search, e.g. host:BER- or os:mac -#env:prod" aria-label="Rule search" value={when} onChange={(e) => setWhen(e.target.value.slice(0, 1000))} />
          <div className="mt-1 text-footnote text-muted">{when ? (p.error ? `${p.error}. ` : "") + `${n.toLocaleString()} hosts match now` : " "}</div>
        </div>
        <Button type="submit" variant="primary" size="md" disabled={!normTag(tag) || !p.node}>
          <Plus size={15} /> Add rule
        </Button>
      </form>
      <div className="mt-2 text-footnote text-muted">Health and software terms (health:, product:, check:, software: …) are not available in rules: there they are plain words. Use them in the search box or a saved search.</div>
      <div className="mt-4 flex justify-end">
        <Button variant="pearl" onClick={onClose}>
          Close
        </Button>
      </div>
    </Modal>
  );
}

export async function saveFile(name: string, content: string, type: string): Promise<boolean> {
  if (inTauri) return invoke<boolean>("hosts_export", { name, content });
  const a = document.createElement("a");
  a.href = URL.createObjectURL(new Blob([content], { type }));
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(a.href), 1000);
  (globalThis as { __lastExport?: unknown }).__lastExport = { name, content };
  return true;
}

export function Export({
  store,
  selection,
  shown = [],
  hosts,
  health = false,
  onClose,
  onToast,
}: {
  store: Store;
  selection: string[];
  /** The rows of the current search, in list order. */
  shown?: Host[];
  hosts?: Map<string, Host>;
  /** Health data exists: health fields and the failing-checks CSV are offered. */
  health?: boolean;
  onClose: () => void;
  onToast: (m: string) => void;
}) {
  const [fmt, setFmt] = useState<"json" | "csv" | "faults">("json");
  // With health data, "All with tags or names" would silently leave out agent-only and untagged rows.
  const [scope, setScope] = useState<"all" | "shown" | "sel">(selection.length > 1 ? "sel" : health ? "shown" : "all");
  const [notes, setNotes] = useState(false);
  const go = async () => {
    const ids = scope === "sel" ? selection : scope === "shown" ? shown.map((h) => h.id) : undefined;
    const o = { notes, ids, hosts: health ? hosts : undefined };
    // Failing checks: "All" means every host with health data, not only those with tags or names.
    const list = ids ? ids.map((id) => hosts?.get(id)).filter((h): h is Host => !!h) : [...(hosts?.values() ?? [])];
    const content = fmt === "json" ? exportJson(store, o) : fmt === "csv" ? exportCsv(store, o) : exportFaultsCsv(list);
    const day = new Date().toISOString().slice(0, 10);
    try {
      if (await saveFile(fmt === "faults" ? `tether-health-faults-${day}.csv` : `tether-hosts-${day}.${fmt}`, content, fmt === "json" ? "application/json" : "text/csv")) {
        onToast("Hosts exported");
        onClose();
      }
    } catch (e) {
      onToast(errorText(e));
    }
  };
  return (
    <Modal onClose={onClose} width={580}>
      <H>Export hosts</H>
      <P>Names, tags, pins, tag colours, rules and saved searches{health ? ", and each host's health: state, counts, products, agent and last report" : ""}. Passwords are never part of the host list.</P>
      <div className="mt-5 space-y-4">
        <div className="flex items-center justify-between">
          <span className="text-caption">Format</span>
          <Segmented
            value={fmt}
            onChange={setFmt}
            options={[{ value: "json", label: "JSON (everything)" }, { value: "csv", label: "CSV (spreadsheet)" }, ...(health ? [{ value: "faults" as const, label: "Failing checks (CSV)" }] : [])]}
          />
        </div>
        <div className="flex items-center justify-between">
          <span className="text-caption">Hosts</span>
          <Segmented
            value={scope}
            onChange={setScope}
            options={[
              { value: "all", label: fmt === "faults" ? "All" : "All with tags or names" },
              { value: "shown" as const, label: `Shown (${shown.length.toLocaleString()})` },
              ...(selection.length ? [{ value: "sel" as const, label: `Selected (${selection.length.toLocaleString()})` }] : []),
            ]}
          />
        </div>
        <div className="flex items-center justify-between gap-4">
          <div>
            <div className="text-caption">Include notes</div>
            <div className="text-footnote text-muted">Notes can contain customer details. Off by default.</div>
          </div>
          <Toggle label="Include notes" checked={notes && fmt !== "faults"} disabled={fmt === "faults"} onChange={setNotes} />
        </div>
      </div>
      <div className="mt-7 flex justify-end gap-2">
        <Button variant="pearl" onClick={onClose}>
          Cancel
        </Button>
        <Button variant="primary" onClick={go}>
          Export…
        </Button>
      </div>
    </Modal>
  );
}

export function Import({ store, onClose, onToast }: { store: Store; onClose: () => void; onToast: (m: string) => void }) {
  const [imp, setImp] = useState<Imported | null>(null);
  const [err, setErr] = useState("");
  const ro = useReadOnly();
  const read = (text: string) => {
    try {
      setImp(parseImport(text));
      setErr("");
    } catch (e) {
      setErr((e as Error).message);
    }
  };
  const pick = async () => {
    if (inTauri) {
      const t = await invoke<string | null>("hosts_import").catch((e) => (setErr(errorText(e)), null));
      if (t != null) read(t);
    }
  };
  const n = imp ? Object.keys(imp.hosts).length : 0;
  const isNew = imp ? Object.keys(imp.hosts).filter((id) => !store.hosts[id]).length : 0;
  return (
    <Modal onClose={onClose} width={480}>
      <H>Import hosts</H>
      <P>A Tether export (JSON) or a CSV with a header row: id, name, tags (space separated), pinned, notes. Tags are added to the ones a host has; names and notes in the file replace the old ones.</P>
      {ro && <Managed what="names, tags, rules and saved searches (only pins and notes are imported here)" />}
      <div className="mt-5">
        {inTauri ? (
          <Button variant="secondary" onClick={pick}>
            Choose file…
          </Button>
        ) : (
          <input type="file" accept=".json,.csv,.txt" aria-label="Import file" onChange={(e) => e.target.files?.[0]?.text().then(read)} className="text-caption" />
        )}
      </div>
      {err && <div className="mt-3 text-caption text-danger">{err}</div>}
      {imp && (
        <div className="mt-4 rounded-xl bg-parchment px-4 py-3 text-caption" data-testid="import-preview">
          <div>
            <b>{n.toLocaleString()}</b> hosts ({isNew.toLocaleString()} new), {imp.rules.length} rules, {imp.searches.length} saved searches
          </div>
          {imp.skipped.length > 0 && (
            <div className="mt-1 text-footnote text-warn">
              Skipped {imp.skipped.length}: {imp.skipped.slice(0, 3).join("; ")}
              {imp.skipped.length > 3 ? " …" : ""}
            </div>
          )}
        </div>
      )}
      <div className="mt-7 flex justify-end gap-2">
        <Button variant="pearl" onClick={onClose}>
          Cancel
        </Button>
        <Button
          variant="primary"
          disabled={!imp || (!n && !imp.rules.length && !imp.searches.length)}
          onClick={() => {
            actions.setStore((s) => mergeImport(s, imp!));
            onToast(`${n.toLocaleString()} hosts imported`);
            onClose();
          }}
        >
          Import
        </Button>
      </div>
    </Modal>
  );
}

export function Team({ onClose, onToast }: { onClose: () => void; onToast: (m: string) => void }) {
  const team = useHosts((s) => s.team);
  const cfg = useHosts((s) => s.store.team);
  const org = useHosts((s) => s.org.length);
  useEffect(() => {
    if (!team.identity) invoke<{ key: string; fingerprint: string }>("team_identity").then((identity) => useHosts.setState({ team: { ...useHosts.getState().team, identity } }), () => {});
  }, [team.identity]);
  const enable = (v: boolean) => {
    actions.setTeam({ enabled: v });
    if (v) void actions.teamRefresh();
    else useHosts.setState({ org: [] });
  };
  return (
    <Modal onClose={onClose} width={520}>
      <div className="flex items-center gap-3">
        <Users size={20} className="text-muted" />
        <H>Team</H>
      </div>
      <P>Share tags, names, rules and saved searches with your team, and see your organisation's devices. Your administrator adds this app's key in Tether Manage (Team). Notes stay on this PC.</P>
      <div className="mt-5 rounded-xl bg-parchment px-4 py-3">
        <div className="text-footnote text-muted">This app's key</div>
        <div className="mt-1 flex items-center gap-2">
          <code className="min-w-0 flex-1 truncate font-mono text-footnote">{team.identity?.key ?? "…"}</code>
          <IconButton label="Copy key" onClick={() => team.identity && navigator.clipboard.writeText(team.identity.key).then(() => onToast("Key copied"), () => {})}>
            <Copy size={14} />
          </IconButton>
        </div>
        <div className="mt-1 text-footnote text-muted">
          Fingerprint <span className="font-mono text-ink">{team.identity?.fingerprint ?? "…"}</span> · read it out to your administrator to compare.
        </div>
      </div>
      <div className="mt-5 space-y-3">
        <div className="flex items-center justify-between gap-4">
          <div>
            <div className="text-caption">Sync with my team</div>
            <div className="text-footnote text-muted" data-testid="team-status">
              {!cfg?.enabled
                ? "Off: everything stays on this PC."
                : team.me
                  ? `Signed in as ${team.me.name}${team.me.write ? "" : " (read only)"}${cfg.synced ? ` · synced ${ago(cfg.synced / 1000)}` : ""}${team.syncing ? " · syncing…" : ""}`
                  : (team.error ?? "Checking…")}
            </div>
          </div>
          <Toggle label="Sync with my team" checked={!!cfg?.enabled} onChange={enable} />
        </div>
        <div className="flex items-center justify-between gap-4">
          <div>
            <div className="text-caption">Show organisation devices</div>
            <div className="text-footnote text-muted">{org ? `${org.toLocaleString()} devices from the relay's heartbeats` : "Devices that report to your relay (ID, OS, version, last seen; no addresses)."}</div>
          </div>
          <Toggle
            label="Show organisation devices"
            disabled={!cfg?.enabled}
            checked={!!cfg?.enabled && cfg.showOrg !== false}
            onChange={(v) => {
              actions.setTeam({ showOrg: v });
              if (v) void actions.teamRefresh();
              else useHosts.setState({ org: [] });
            }}
          />
        </div>
        <div className="flex items-center justify-between gap-4">
          <div>
            <div className="text-caption">Notify me when a host turns critical or stops reporting</div>
            <div className="text-footnote text-muted">Windows only. At most once an hour per host, while the host list is open. Only on this PC.</div>
          </div>
          <Toggle label="Notify me when a host turns critical or stops reporting" disabled={!cfg?.enabled} checked={!!cfg?.enabled && cfg.healthNotify !== false} onChange={(v) => actions.setTeam({ healthNotify: v })} />
        </div>
      </div>
      <div className="mt-7 flex justify-end gap-2">
        <Button variant="pearl" disabled={!cfg?.enabled || team.syncing} onClick={() => void actions.teamRefresh().then(() => teamSync())}>
          <RefreshCw size={14} /> Sync now
        </Button>
        <Button variant="primary" onClick={onClose}>
          Done
        </Button>
      </div>
    </Modal>
  );
}

export function AddHost({ known, onClose, onAdded }: { known: string[]; onClose: () => void; onAdded: (id: string) => void }) {
  const [id, setId] = useState("");
  const [name, setName] = useState("");
  const [tags, setTags] = useState<string[]>([]);
  const ro = useReadOnly();
  const clean = id.replace(/\s/g, "");
  const ok = /^[A-Za-z0-9_.@:-]{6,64}$/.test(clean);
  return (
    <Modal onClose={onClose} width={440}>
      <H>Add host</H>
      <P>Keep a device in your list before you first connect to it.</P>
      <form
        className="mt-5 space-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          if (!ok) return;
          actions.addHost(clean, name, tags);
          onAdded(clean);
          onClose();
        }}
      >
        <TextField autoFocus placeholder="ID" aria-label="Host ID" className="font-mono" value={/^\d[\d\s]*$/.test(id) ? formatId(id) : id} onChange={(e) => setId(e.target.value.slice(0, 64))} />
        {ro ? (
          <Managed what="names and tags" />
        ) : (
          <>
            <TextField placeholder="Name (optional)" aria-label="Host name" value={name} onChange={(e) => setName(e.target.value.slice(0, 64))} />
            <div className="flex flex-wrap gap-1">
              {tags.map((t) => (
                <TagChip key={t} tag={t} meta={{}} onRemove={() => setTags(tags.filter((x) => x !== t))} />
              ))}
            </div>
            <TagInput known={known} onAdd={(t) => !tags.includes(t) && setTags([...tags, t])} placeholder="Tags (optional)" />
          </>
        )}
        <div className="flex justify-end gap-2 pt-4">
          <Button type="button" variant="pearl" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" variant="primary" disabled={!ok}>
            Add host
          </Button>
        </div>
      </form>
    </Modal>
  );
}

export function SaveSearch({ query, onClose }: { query: string; onClose: () => void }) {
  const [name, setName] = useState("");
  return (
    <Modal onClose={onClose} width={420}>
      <H>Save search</H>
      <P>
        <code className="font-mono text-footnote">{query}</code>
      </P>
      <form
        className="mt-4"
        onSubmit={(e) => {
          e.preventDefault();
          if (!name.trim()) return;
          actions.setStore((s) => ({ ...s, searches: [...s.searches, { id: newId(), name: name.trim().slice(0, 64), query }] }));
          onClose();
        }}
      >
        <TextField autoFocus placeholder="Name (Berlin servers)" aria-label="Search name" value={name} onChange={(e) => setName(e.target.value)} />
        <div className="mt-6 flex justify-end gap-2">
          <Button type="button" variant="pearl" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" variant="primary" disabled={!name.trim()}>
            Save
          </Button>
        </div>
      </form>
    </Modal>
  );
}

interface OpenSession {
  key: string;
  peer: string;
  conn: string;
}

/** Open session windows with what can be sent to each without switching to it. */
export function Sessions({ hosts, onClose, onToast }: { hosts: Map<string, Host>; onClose: () => void; onToast: (m: string) => void }) {
  const [list, setList] = useState<OpenSession[]>([]);
  // "lock:<key>" or "restart:<key>": the action that waits for its second click.
  const [confirm, setConfirm] = useState<string | null>(null);
  useEffect(() => {
    const load = () => invoke<OpenSession[]>("sessions_open").then((l) => setList(l ?? []), () => {});
    load();
    const t = setInterval(load, 2000);
    return () => clearInterval(t);
  }, []);
  const act = (p: Promise<unknown>, ok: string) => p.then(() => onToast(ok), (e) => onToast(errorText(e)));
  return (
    <Modal onClose={onClose} width={600}>
      <H>Open sessions</H>
      <P>Every session window of this app. Keys go to the remote computer at once.</P>
      <div className="mt-4 space-y-1" aria-label="Open sessions">
        {list.length === 0 && <div className="rounded-xl bg-parchment px-4 py-6 text-center text-caption text-muted">No open sessions.</div>}
        {list.map((s) => {
          const h = hosts.get(s.peer);
          const remote = s.conn === "default";
          return (
            <div key={s.key} className="flex h-11 items-center gap-3 rounded-md px-2 hover:bg-tile-2">
              <div className="min-w-0 flex-1">
                <div className="truncate text-caption font-semibold" title={h ? label(h) : formatId(s.peer)}>
                  {h ? label(h) : formatId(s.peer)}
                </div>
                <div className="font-mono text-footnote text-muted">
                  {formatId(s.peer)} · {remote ? "Remote control" : "File transfer"}
                </div>
              </div>
              <Button size="sm" variant="pearl" onClick={() => invoke<boolean>("session_focus_window", { key: s.key }).then((ok) => !ok && onToast("That window has closed."))}>
                Show
              </Button>
              {remote && (
                <>
                  <IconButton label="Send Ctrl+Alt+Del" onClick={() => act(invoke("session_ctrl_alt_del", { key: s.key }), "Ctrl+Alt+Del sent")}>
                    <Keyboard size={15} />
                  </IconButton>
                  {confirm === `lock:${s.key}` ? (
                    <Button size="sm" variant="danger" title="If Tether is not installed as a service there (TetherQS), you cannot see the lock screen or sign back in." onClick={() => (setConfirm(null), act(invoke("session_lock_screen", { key: s.key }), "Screen locked"))}>
                      Lock now?
                    </Button>
                  ) : (
                    <IconButton label="Lock remote screen" onClick={() => setConfirm(`lock:${s.key}`)}>
                      <Lock size={15} />
                    </IconButton>
                  )}
                  {confirm === `restart:${s.key}` ? (
                    <Button size="sm" variant="danger" onClick={() => (setConfirm(null), act(invoke("session_restart_remote", { key: s.key }), "Restart requested"))}>
                      Restart now?
                    </Button>
                  ) : (
                    <IconButton label="Restart remote computer" onClick={() => setConfirm(`restart:${s.key}`)}>
                      <Power size={15} />
                    </IconButton>
                  )}
                </>
              )}
            </div>
          );
        })}
      </div>
      <div className="mt-6 flex justify-end">
        <Button variant="primary" onClick={onClose}>
          Done
        </Button>
      </div>
    </Modal>
  );
}

const SHORTCUTS: [string, string][] = [
  ["Ctrl K", "Command palette: connect to any host, run any command"],
  ["/  or  Ctrl F", "Search"],
  ["↑ ↓  Page Up/Down  Home End", "Move"],
  ["Shift ↑ ↓", "Extend the selection"],
  ["Space", "Select / unselect"],
  ["Ctrl A", "Select all shown"],
  ["Enter", "Connect"],
  ["Shift Enter", "Transfer files"],
  ["Alt Enter", "View only"],
  ["T", "Tags of the selection"],
  ["P", "Pin / unpin"],
  ["N", "Edit notes"],
  ["E", "Rename"],
  ["Delete", "Remove from the list"],
  ["Esc", "Clear selection, then search"],
  ["?", "This help"],
];

const SYNTAX: [string, string][] = [
  ["berlin srv", "Fuzzy words in ID, name, hostname, user, OS, version, tags, notes (all must match)"],
  ["#site:berlin", "Tag, including its children (#site matches site:berlin, site:munich)"],
  ["#customer:ac*", "Tag prefix"],
  ["a | b,  a OR b", "Either"],
  ["-#env:prod,  NOT x", "Not"],
  ["( … )", "Grouping: (#site:berlin | #site:munich) -#env:test"],
  ["is:online  is:offline  is:pinned  is:org  is:recent  is:manual", "Status and source"],
  ["os:mac  host:BER-  user:anna  id:1234  name:\"front desk\"", "Fields"],
  ["ver:<0.5.21  ver:0.5", "Version (compare or prefix)"],
  ["seen:<7d  seen:>30d", "Last activity (m, h, d, w)"],
  ["has:notes  has:tags  has:name", "Has a value"],
  ["health:crit  health:stale  health:none", "Health: crit, warn, stale, setup, ok, none (no monitor agent), conflict (two devices linked to one host)"],
  ["product:records  version:26.3  role:server", "Detected products, version prefix, role (server, client, single)"],
  ["check:backup  check:backup=crit", "A check whose id contains the text is not OK; =crit or =warn: in that state"],
  ["software:garnet  software:\"garnet browser\"@126", "An installed program (name or publisher contains the text, @ a version prefix), as of each computer's last upload; Enter searches the servers"],
  ["agent:0.1  agent:none", "Monitor agent version"],
  ["since:3d", "Not OK for at least that long (m, h, d, w). Health terms are plain words in tag rules"],
];

export function Help({ onClose }: { onClose: () => void }) {
  return (
    <Modal onClose={onClose} width={720}>
      <H>Keyboard and search</H>
      {/* Scrolls on a short window (1366x768 at 150 %), so Done stays on screen. */}
      <div className="mt-5 grid max-h-[calc(100vh-200px)] grid-cols-2 gap-8 overflow-y-auto">
        <div>
          <div className="text-footnote font-semibold text-muted">Shortcuts</div>
          <dl className="mt-2 space-y-1.5">
            {SHORTCUTS.map(([k, v]) => (
              <div key={k} className="flex items-center justify-between gap-3 text-caption">
                <dt className="flex gap-1">
                  {k.split("  ").map((x) => (
                    <Kbd key={x}>{x}</Kbd>
                  ))}
                </dt>
                <dd className="text-right text-muted">{v}</dd>
              </div>
            ))}
          </dl>
        </div>
        <div>
          <div className="text-footnote font-semibold text-muted">Search syntax</div>
          <dl className="mt-2 space-y-2">
            {SYNTAX.map(([k, v]) => (
              <div key={k}>
                <dt className="font-mono text-footnote text-ink">{k}</dt>
                <dd className="text-footnote text-muted">{v}</dd>
              </div>
            ))}
          </dl>
        </div>
      </div>
      <div className="mt-6 flex justify-end">
        <Button variant="primary" onClick={onClose}>
          Done
        </Button>
      </div>
    </Modal>
  );
}

export { TagDot };

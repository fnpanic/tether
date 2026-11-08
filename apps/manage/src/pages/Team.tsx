import { useEffect, useState } from "react";
import { Plus, RefreshCw, Trash2 } from "lucide-react";
import { useApp } from "../App";
import { host, invoke, type RelayResult } from "../lib/ipc";
import { fingerprint, keyProblem, nameProblem, parseKey, sameRoster, type Member, type Roster } from "../lib/team";
import { PublishSheet } from "../components/publish";
import { Badge, Button, ErrorText, IconButton, Label, Modal, PageHeader, Table, TextArea, TextField, Toggle } from "../components/ui";

interface Loaded {
  roster: Roster;
  /** Relays whose roster is not the first relay's. */
  differ: string[];
  failed: string[];
}

/** Fingerprints of the given keys, computed as they appear. */
function useFingerprints(keys: string[]) {
  const [fps, setFps] = useState<Record<string, string>>({});
  useEffect(() => {
    const missing = keys.filter((k) => !(k in fps));
    if (!missing.length) return;
    Promise.all(missing.map(async (k) => [k, parseKey(k) ? await fingerprint(parseKey(k)!) : "invalid key"] as const)).then((pairs) =>
      setFps((f) => ({ ...f, ...Object.fromEntries(pairs) })),
    );
  }, [keys.join(",")]);
  return fps;
}

/** What publishing changes, in words. */
function changes(from: Roster, to: Roster): string[] {
  const out: string[] = [];
  const old = new Map(from.members.map((m) => [m.key, m]));
  const now = new Map(to.members.map((m) => [m.key, m]));
  const rights = (m: Member) => [m.write && "edit shared tags", m.audit && "read session history"].filter(Boolean).join(", ") || "read only";
  for (const m of to.members) {
    const o = old.get(m.key);
    if (!o) out.push(`Add ${m.name} (${rights(m)})`);
    else if (o.name !== m.name || o.write !== m.write || o.audit !== m.audit) out.push(`${o.name === m.name ? m.name : `${o.name} → ${m.name}`}: ${rights(m)}`);
  }
  for (const m of from.members) if (!now.has(m.key)) out.push(`Remove ${m.name}: their key stops working on every relay that gets this roster`);
  if (!out.length && !sameRoster(from, to)) out.push("New order");
  return out;
}

function AddMember({ members, onAdd, onClose }: { members: Member[]; onAdd: (m: Member) => void; onClose: () => void }) {
  const [name, setName] = useState("");
  const [key, setKey] = useState("");
  const [fp, setFp] = useState<string | null>(null);
  const nameErr = name ? nameProblem(name.trim()) : null;
  const keyErr = key.trim() ? keyProblem(key, members) : null;
  useEffect(() => {
    const raw = parseKey(key);
    setFp(null);
    if (raw) fingerprint(raw).then(setFp);
  }, [key]);
  const ok = name.trim() && !nameErr && key.trim() && !keyErr;
  return (
    <Modal
      onClose={onClose}
      width={560}
      title={
        <>
          <h2 className="text-headline font-semibold">Add technician</h2>
          <p className="mt-1 text-caption text-muted">
            The technician app shows its public key and fingerprint under Team. Paste the key and compare the fingerprint with the one the technician reads out.
          </p>
        </>
      }
      footer={
        <div className="flex justify-end gap-2">
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" disabled={!ok} onClick={() => onAdd({ name: name.trim(), key: key.replace(/\s+/g, ""), write: false, audit: false })}>
            Add
          </Button>
        </div>
      }
    >
      <div className="flex flex-col gap-4">
        <Label text="Name" hint="Shown to the team as the author of tag changes. 1–64 characters.">
          <TextField autoFocus maxLength={64} value={name} onChange={(e) => setName(e.target.value)} placeholder="Anna Berger" />
        </Label>
        <ErrorText error={nameErr} />
        <Label text="Public key" hint="44 characters, ends with =.">
          <TextArea rows={2} className="font-mono" value={key} onChange={(e) => setKey(e.target.value)} placeholder="AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=" />
        </Label>
        <ErrorText error={keyErr} />
        {fp && !keyErr && (
          <p className="text-caption">
            Fingerprint <code className="select-text font-mono font-semibold">{fp}</code>
          </p>
        )}
      </div>
    </Modal>
  );
}

export function Team() {
  const { state } = useApp();
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [draft, setDraft] = useState<Roster | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [publishing, setPublishing] = useState(false);
  const fps = useFingerprints(draft?.members.map((m) => m.key) ?? []);

  const load = () => {
    setError(null);
    invoke<Loaded>("team_load").then(
      (l) => {
        setLoaded(l);
        setDraft(l.roster);
      },
      (e) => setError(e.message),
    );
  };
  useEffect(load, [state.settings.server]);

  const dirty = !!(loaded && draft && !sameRoster(loaded.roster, draft));
  const set = (i: number, m: Partial<Member>) => draft && setDraft({ members: draft.members.map((x, j) => (j === i ? { ...x, ...m } : x)) });

  return (
    <>
      <PageHeader title="Team" sub="Technicians who sign in to the relays with their own key: device list without IP addresses, shared tags, and session history if allowed.">
        <IconButton label="Reload from the relays" onClick={load}>
          <RefreshCw size={16} />
        </IconButton>
        <Button onClick={() => setAdding(true)} disabled={!draft}>
          <Plus size={16} /> Add technician
        </Button>
        <Button variant="primary" disabled={!dirty} onClick={() => setPublishing(true)}>
          Publish…
        </Button>
      </PageHeader>
      <ErrorText error={error} />
      {loaded && (loaded.differ.length > 0 || loaded.failed.length > 0) && (
        <div className="mb-4 flex flex-col gap-1">
          {loaded.differ.map((s) => (
            <p key={s} className="text-caption text-warn">
              {host(s)} has a different roster. Publish to bring every relay to the list below.
            </p>
          ))}
          {loaded.failed.map((f) => (
            <p key={f} className="select-text text-caption text-danger">
              {f}
            </p>
          ))}
        </div>
      )}
      {dirty && (
        <p className="mb-3 text-caption text-muted">
          <Badge tone="accent">Draft</Badge> Changes stay on this PC until you publish them.
        </p>
      )}
      {draft && (
        <Table head={["Name", "Fingerprint", "Can edit shared tags", "Can read session history", ""]} empty={!draft.members.length && "No technicians yet. Team access is off on the relays."}>
          {draft.members.map((m, i) => (
            <tr key={m.key}>
              <td className="font-semibold">{m.name}</td>
              <td>
                <code className="select-text font-mono text-footnote" title={m.key}>
                  {fps[m.key] ?? "…"}
                </code>
              </td>
              <td>
                <Toggle label={`${m.name} can edit shared tags`} checked={m.write} onChange={(v) => set(i, { write: v })} />
              </td>
              <td>
                <Toggle label={`${m.name} can read session history`} checked={m.audit} onChange={(v) => set(i, { audit: v })} />
              </td>
              <td className="text-right">
                <IconButton label={`Remove ${m.name}`} onClick={() => setDraft({ members: draft.members.filter((_, j) => j !== i) })}>
                  <Trash2 size={16} />
                </IconButton>
              </td>
            </tr>
          ))}
        </Table>
      )}
      {adding && draft && (
        <AddMember
          members={draft.members}
          onClose={() => setAdding(false)}
          onAdd={(m) => {
            setDraft({ members: [...draft.members, m] });
            setAdding(false);
          }}
        />
      )}
      {publishing && loaded && draft && (
        <PublishSheet
          title="Publish team roster"
          action="Publish"
          run={async () => {
            const r = await invoke<RelayResult[]>("team_publish", { roster: draft });
            if (r.every((x) => x.ok)) setLoaded({ roster: draft, differ: [], failed: [] });
            return r;
          }}
          onClose={() => setPublishing(false)}
          warning={<p className="text-caption text-muted">Each relay only knows the roster it was given: a technician removed here keeps access on a relay that does not get this roster.</p>}
        >
          <ul className="flex flex-col gap-1 text-caption">
            {changes(loaded.roster, draft).map((c, i) => (
              <li key={i}>{c}</li>
            ))}
          </ul>
        </PublishSheet>
      )}
    </>
  );
}

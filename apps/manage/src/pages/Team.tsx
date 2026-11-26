import { useEffect, useRef, useState } from "react";
import { ask } from "@tauri-apps/plugin-dialog";
import { Plus, RefreshCw, Trash2 } from "lucide-react";
import { useApp, type Proposal } from "../App";
import { host, inTauri, invoke, when, type RelayResult } from "../lib/ipc";
import { fingerprint, keyProblem, nameProblem, parseKey } from "../lib/team";
import { emptyOidc, grantText, lockedOut, sameDoc, standardRoles, teamChanges, type DocMember, type Grants, type Oidc, type TeamDoc } from "../lib/teamdoc";
import { ROLES, ROLE_TEXT, type Role } from "../lib/roles";
import { PublishSheet } from "../components/publish";
import { Proposals, ProposeSheet, removeProposal, useProposals } from "../components/proposals";
import { Badge, Button, Card, ErrorText, IconButton, Label, Modal, PageHeader, Select, Table, TextArea, TextField, Toggle } from "../components/ui";

interface Loaded {
  doc: TeamDoc;
  /** The relays serve a signed document (else a plain roster, shown as the first document it becomes). */
  signed: boolean;
  /** Relays whose document is not the newest one. */
  differ: string[];
  failed: string[];
}

/** Fingerprints of the given keys, computed as they appear. */
function useFingerprints(keys: string[]) {
  const [fps, setFps] = useState<Record<string, string>>({});
  useEffect(() => {
    const missing = keys.filter((k) => !(k in fps));
    if (!missing.length) return;
    Promise.all(missing.map(async (k) => [k, parseKey(k) ? await fingerprint(parseKey(k)!) : "invalid key"] as const)).then((pairs) => setFps((f) => ({ ...f, ...Object.fromEntries(pairs) })));
  }, [keys.join(",")]);
  return fps;
}

const patterns = (t: string) => t.split(",").map((s) => s.trim()).filter(Boolean);

function AddMember({ members, onAdd, onClose }: { members: DocMember[]; onAdd: (m: DocMember) => void; onClose: () => void }) {
  const [name, setName] = useState("");
  const [key, setKey] = useState("");
  const [service, setService] = useState(false);
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
          <h2 className="text-headline font-semibold">Add a key entry</h2>
          <p className="mt-1 text-caption text-muted">A service (the bridge, a script) or a break-glass technician who works with a key instead of signing in. The app shows its public key and fingerprint under Team; compare the fingerprint.</p>
        </>
      }
      footer={
        <div className="flex justify-end gap-2">
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" disabled={!ok} onClick={() => onAdd({ name: name.trim(), key: key.replace(/\s+/g, ""), write: false, audit: false, sbom: true, agent: false, customers: ["*"], service })}>
            Add
          </Button>
        </div>
      }
    >
      <div className="flex flex-col gap-4">
        <Label text="Name" hint="1–64 characters.">
          <TextField data-autofocus maxLength={64} value={name} onChange={(e) => setName(e.target.value)} placeholder="Monitoring bridge" />
        </Label>
        <ErrorText error={nameErr} />
        <Label text="Public key" hint="44 characters, ends with =.">
          <TextArea rows={2} className="font-mono" value={key} onChange={(e) => setKey(e.target.value)} />
        </Label>
        <ErrorText error={keyErr} />
        {fp && !keyErr && (
          <p className="text-caption">
            Fingerprint <code className="select-text font-mono font-semibold">{fp}</code>
          </p>
        )}
        <Toggle label="Service entry (keeps working with sign-in only)" checked={service} onChange={setService} />
      </div>
    </Modal>
  );
}

const OIDC_FIELDS: [keyof Oidc, string, string][] = [
  ["issuer", "Issuer", "Entra ID: https://login.microsoftonline.com/<tenant id>/v2.0 (v2 tokens: set requestedAccessTokenVersion to 2 in the Tether API registration's manifest)."],
  ["tenant", "Tenant (directory) id", "The tid every token must carry."],
  ["audience", "Audience: the Tether API registration's client id", "A v2 token's aud is always the API's client id, not its App ID URI."],
  ["client_id", "Client id of the Tether desktop app registration", "A public client with the redirect URI http://localhost (any port); Tether Manage signs in with it too."],
  ["scope", "Scope", "The API's scope, e.g. api://<API client id>/access; openid and offline_access are added."],
];

function RoleRow({ name, g, set, remove }: { name: string; g: Grants; set: (g: Grants) => void; remove: () => void }) {
  const flag = (k: "write" | "audit" | "sbom" | "agent") => <Toggle label={`${name}: ${k}`} checked={!!g[k]} disabled={!!g.manage} onChange={(v) => set({ ...g, [k]: v })} />;
  return (
    <tr>
      <td className="select-text font-mono text-footnote">{name}</td>
      <td>
        <Select aria-label={`${name}: Tether Manage role`} value={g.manage ?? ""} onChange={(e) => set(e.target.value ? { manage: e.target.value as Role } : {})}>
          <option value="">Team (desktop)</option>
          {ROLES.map((r) => (
            <option key={r} value={r}>
              Tether Manage: {ROLE_TEXT[r]}
            </option>
          ))}
        </Select>
      </td>
      <td>{flag("write")}</td>
      <td>{flag("audit")}</td>
      <td>{flag("sbom")}</td>
      <td>{flag("agent")}</td>
      <td>
        <TextField aria-label={`${name}: customers`} disabled={!!g.manage} value={(g.customers ?? []).join(", ")} placeholder="every customer" onChange={(e) => set({ ...g, customers: patterns(e.target.value) })} />
      </td>
      <td className="text-right">
        <IconButton label={`Remove role ${name}`} onClick={remove}>
          <Trash2 size={16} />
        </IconButton>
      </td>
    </tr>
  );
}

export function Team() {
  const { state, can, toast } = useApp();
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  // In the app's state: unpublished changes are still here after a visit to another page.
  const { teamDraft: draft, setTeamDraft: setDraft } = useApp();
  const [error, setError] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [newRole, setNewRole] = useState("");
  const [newDeny, setNewDeny] = useState("");
  const [sheet, setSheet] = useState<"publish" | "propose" | null>(null);
  const [reviewing, setReviewing] = useState<Proposal | null>(null);
  const proposals = useProposals("team", can("read-proposals-policy"));
  const fps = useFingerprints(draft?.members.map((m) => m.key) ?? []);

  const load = (discard = false) => {
    setError(null);
    invoke<Loaded>("team_load").then(
      (l) => {
        setLoaded(l);
        setDraft((d) => (discard || !d ? l.doc : d));
      },
      (e) => setError(e.message),
    );
  };
  // Another server list: the kept draft belongs to the old one.
  const server = useRef(state.settings.server);
  useEffect(() => {
    load(server.current !== state.settings.server);
    server.current = state.settings.server;
  }, [state.settings.server]);
  // The relay's checks of the draft (tether_trust's TeamDoc::validate).
  useEffect(() => {
    if (draft) invoke("team_check", { doc: draft }).then(() => setProblem(null), (e) => setProblem(e.message));
  }, [draft]);

  const dirty = !!(loaded && draft && (!sameDoc(loaded.doc, draft) || !loaded.signed));
  const changed = useRef(false);
  changed.current = !!draft && (!loaded || dirty);
  useEffect(() => () => void (changed.current || setDraft(null)), []);
  const reloadAll = async () => {
    if (dirty && inTauri && !(await ask("Load the team document from the relays? Your unpublished changes are discarded.", { title: "Reload", kind: "warning" }))) return;
    setReviewing(null);
    load(true);
  };
  const edit = (f: (d: TeamDoc) => TeamDoc) => draft && setDraft(f(draft));
  const setMember = (i: number, m: Partial<DocMember>) => edit((d) => ({ ...d, members: d.members.map((x, j) => (j === i ? { ...x, ...m } : x)) }));
  const changes = loaded && draft ? [...(!loaded.signed ? ["First signed team document: the relays then refuse plain rosters"] : []), ...teamChanges(loaded.doc, draft)] : [];
  const out = draft ? lockedOut(draft) : [];
  const editable = can("sign") || can("propose-team");

  return (
    <>
      <PageHeader title="Team" sub="The signed team document: staff sign-in, what each directory role may do, the deny list, and key entries for services and break-glass.">
        <IconButton label="Reload from the relays" onClick={reloadAll}>
          <RefreshCw size={16} />
        </IconButton>
        {can("sign") ? (
          <Button variant="primary" disabled={!dirty || !!problem} onClick={() => setSheet("publish")} title={problem ?? undefined}>
            Sign and publish…
          </Button>
        ) : (
          can("propose-team") && (
            <Button variant="primary" disabled={!dirty || !!problem} onClick={() => setSheet("propose")} title={problem ?? "Stored for a key holder to sign and publish"}>
              Propose…
            </Button>
          )
        )}
      </PageHeader>
      <ErrorText error={error} />
      <Proposals
        kind="team"
        list={proposals.list}
        error={proposals.error}
        reload={proposals.load}
        review={
          can("sign")
            ? (p) => {
                setDraft(p.doc as TeamDoc);
                setReviewing(p);
                toast(`Loaded the proposal of ${p.author.name}: its changes against the live document are listed when you publish.`);
              }
            : undefined
        }
      />
      {reviewing && (
        <p className="mb-4 text-caption" role="status">
          <Badge tone="ink">Reviewing</Badge> the proposal of {reviewing.author.name} ({when(reviewing.created)})
          {reviewing.stale.length > 0 && <span className="text-warn"> · the live document changed since it was made</span>}
          {state.signin.upn && reviewing.author.upn === state.signin.upn && <span className="text-warn"> · you proposed it yourself</span>}
        </p>
      )}
      {loaded && !loaded.signed && (
        <p className="mb-3 text-caption text-warn">The relays still use a plain roster. Below is the first team document it becomes (every entry keeps today's rights); signing and publishing it switches the relays to signed documents for good.</p>
      )}
      {loaded?.differ.map((s) => (
        <p key={s} className="text-caption text-warn">
          {host(s)} serves an older team document. Publish to bring every relay to the one below.
        </p>
      ))}
      {loaded?.failed.map((f) => (
        <p key={f} className="select-text text-caption text-danger">
          {f}
        </p>
      ))}
      <ErrorText error={problem} />
      {draft && (
        <fieldset disabled={!editable} className="flex flex-col gap-6">
          <Card>
            <div className="flex items-center justify-between">
              <h2 className="text-headline font-semibold">Staff sign-in</h2>
              <Toggle label="Staff sign in with an identity provider" checked={!!draft.oidc} onChange={(v) => edit((d) => ({ ...d, oidc: v ? (d.oidc ?? emptyOidc()) : null, oidc_only: v && d.oidc_only }))} />
            </div>
            {draft.oidc && (
              <div className="mt-4 grid grid-cols-2 gap-4">
                {OIDC_FIELDS.map(([k, label, hint]) => (
                  <Label key={k} text={label} hint={hint}>
                    <TextField value={draft.oidc![k]} onChange={(e) => edit((d) => ({ ...d, oidc: { ...d.oidc!, [k]: e.target.value.trim() } }))} />
                  </Label>
                ))}
              </div>
            )}
            {draft.oidc && (
              <div className="mt-4">
                <Toggle label="Sign-in only: keys work only for service entries" checked={draft.oidc_only} onChange={(v) => edit((d) => ({ ...d, oidc_only: v }))} />
                {draft.oidc_only && out.length > 0 && (
                  <p className="mt-2 text-caption text-warn" role="alert">
                    Every key entry that is not a service loses team access on every relay that gets this document: {out.join(", ")}. They must sign in instead. Keep at least one break-glass way in (a key holder's keys).
                  </p>
                )}
              </div>
            )}
          </Card>
          <Card>
            <div className="flex items-center justify-between">
              <h2 className="text-headline font-semibold">Directory roles</h2>
              <Button size="sm" onClick={() => edit((d) => ({ ...d, roles: { ...standardRoles(), ...d.roles } }))}>
                Add the standard roles
              </Button>
            </div>
            <p className="mt-1 text-caption text-muted">App role values of the Tether API registration and what they grant: team permissions in the desktop app (customers: references, 47* or *; empty: every customer), or a Tether Manage role (no team access with it). Grants of a person's roles add up.</p>
            <Table head={["App role value", "Grants", "write", "audit", "sbom", "agent", "Customers", ""]} empty={!Object.keys(draft.roles).length && "No roles: nobody can sign in."}>
              {Object.entries(draft.roles).map(([name, g]) => (
                <RoleRow
                  key={name}
                  name={name}
                  g={g}
                  set={(ng) => edit((d) => ({ ...d, roles: { ...d.roles, [name]: ng } }))}
                  remove={() =>
                    edit((d) => {
                      const roles = { ...d.roles };
                      delete roles[name];
                      return { ...d, roles };
                    })
                  }
                />
              ))}
            </Table>
            <div className="mt-3 flex gap-2">
              <TextField aria-label="New app role value" placeholder="Tether.Customer.4711" value={newRole} onChange={(e) => setNewRole(e.target.value.trim())} />
              <Button
                disabled={!newRole || newRole in draft.roles}
                onClick={() => {
                  edit((d) => ({ ...d, roles: { ...d.roles, [newRole]: {} } }));
                  setNewRole("");
                }}
              >
                <Plus size={16} /> Add role
              </Button>
            </div>
            <p className="mt-2 text-footnote text-muted">{Object.entries(draft.roles).map(([r, g]) => `${r}: ${grantText(g)}`).join(" · ")}</p>
          </Card>
          <Card>
            <h2 className="text-headline font-semibold">Deny list</h2>
            <p className="mt-1 text-caption text-muted">Object ids (oid) refused a sign-in; their sessions end on each relay as soon as it gets this document.</p>
            <ul className="mt-2 flex flex-col gap-1">
              {draft.deny.map((o) => (
                <li key={o} className="flex items-center gap-2 text-caption">
                  <code className="select-text font-mono">{o}</code>
                  <IconButton label={`Remove ${o} from the deny list`} onClick={() => edit((d) => ({ ...d, deny: d.deny.filter((x) => x !== o) }))}>
                    <Trash2 size={14} />
                  </IconButton>
                </li>
              ))}
            </ul>
            <div className="mt-2 flex gap-2">
              <TextField aria-label="Object id to deny" placeholder="00000000-0000-0000-0000-000000000000" value={newDeny} onChange={(e) => setNewDeny(e.target.value.trim())} />
              <Button
                disabled={!newDeny || draft.deny.includes(newDeny)}
                onClick={() => {
                  edit((d) => ({ ...d, deny: [...d.deny, newDeny] }));
                  setNewDeny("");
                }}
              >
                Deny
              </Button>
            </div>
          </Card>
          <Card>
            <div className="flex items-center justify-between">
              <h2 className="text-headline font-semibold">Key entries</h2>
              <Button size="sm" onClick={() => setAdding(true)}>
                <Plus size={16} /> Add key entry
              </Button>
            </div>
            <p className="mt-1 text-caption text-muted">Apps that work with their own key: services (the bridge, scripts) and, until everyone signs in, technicians.</p>
            <Table head={["Name", "Fingerprint", "write", "audit", "sbom", "agent", "Customers", "Service", ""]} empty={!draft.members.length && "No key entries."}>
              {draft.members.map((m, i) => (
                <tr key={m.key}>
                  <td className="font-semibold">{m.name}</td>
                  <td>
                    <code className="select-text font-mono text-footnote" title={m.key}>
                      {fps[m.key] ?? "…"}
                    </code>
                  </td>
                  {(["write", "audit", "sbom", "agent"] as const).map((k) => (
                    <td key={k}>
                      <Toggle label={`${m.name}: ${k}`} checked={m[k]} onChange={(v) => setMember(i, { [k]: v })} />
                    </td>
                  ))}
                  <td>
                    <TextField aria-label={`${m.name}: customers`} value={m.customers.join(", ")} onChange={(e) => setMember(i, { customers: patterns(e.target.value) })} />
                  </td>
                  <td>
                    <Toggle label={`${m.name} is a service`} checked={m.service} onChange={(v) => setMember(i, { service: v })} />
                  </td>
                  <td className="text-right">
                    <IconButton label={`Remove ${m.name}`} onClick={() => edit((d) => ({ ...d, members: d.members.filter((_, j) => j !== i) }))}>
                      <Trash2 size={16} />
                    </IconButton>
                  </td>
                </tr>
              ))}
            </Table>
          </Card>
        </fieldset>
      )}
      {adding && draft && (
        <AddMember
          members={draft.members}
          onClose={() => setAdding(false)}
          onAdd={(m) => {
            edit((d) => ({ ...d, members: [...d.members, m] }));
            setAdding(false);
          }}
        />
      )}
      {sheet === "propose" && draft && <ProposeSheet kind="team" doc={draft} changes={changes} onClose={() => setSheet(null)} onDone={proposals.load} />}
      {sheet === "publish" && loaded && draft && (
        <PublishSheet
          title="Sign and publish the team document"
          action="Sign and publish"
          run={async (skip) => {
            const r = await invoke<{ serial: number; relays: RelayResult[] }>("team_publish", { doc: draft, skip });
            if (r.relays.some((x) => x.ok) && reviewing) {
              removeProposal(reviewing).then(proposals.load, () => {});
              setReviewing(null);
            }
            load(true);
            return r.relays;
          }}
          onClose={() => setSheet(null)}
          warning={
            <>
              {reviewing && <p className="text-caption">Proposed by {reviewing.author.name}{reviewing.note ? `: “${reviewing.note}”` : ""}.</p>}
              {out.length > 0 && <p className="text-caption text-warn">Sign-in only: {out.join(", ")} lose team access.</p>}
              <p className="text-caption text-muted">Each relay only knows the document it was given: a relay that does not get this one keeps the old entries and roles.</p>
            </>
          }
        >
          <ul className="flex flex-col gap-1 text-caption">
            {changes.map((c, i) => (
              <li key={i}>{c}</li>
            ))}
          </ul>
        </PublishSheet>
      )}
    </>
  );
}

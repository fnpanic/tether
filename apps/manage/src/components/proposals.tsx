// Proposals (design 12.4): the list a key holder reviews (author, time, note, stale where the
// relays serve something else now) and the sheet in which a policy editor or publisher stores one.
import { useEffect, useState } from "react";
import { useApp, type Proposal } from "../App";
import { ago, host, invoke, visible, when, type RelayResult } from "../lib/ipc";
import { RelayResults } from "./publish";
import { Badge, Button, ErrorText, Label, Modal, TextArea } from "./ui";

export function useProposals(kind: Proposal["kind"], enabled: boolean) {
  const [list, setList] = useState<Proposal[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const load = () => {
    if (!enabled) return;
    invoke<{ proposals: Proposal[]; failed: string[] }>("proposals_list", { kind }).then(
      (r) => {
        setList(r.proposals.map((p) => ({ ...p, kind })));
        setError(r.failed.length ? r.failed.join("; ") : null);
      },
      (e) => setError(e.message),
    );
  };
  useEffect(load, [kind, enabled]);
  return { list, error, load };
}

export const removeProposal = (p: Proposal) => invoke<RelayResult[]>("proposal_delete", { kind: p.kind, ids: p.ids });

/** The proposals of one kind. `review`: what a key holder does with one (load it to sign). */
export function Proposals({ kind, list, error, reload, review }: { kind: Proposal["kind"]; list: Proposal[] | null; error: string | null; reload: () => void; review?: (p: Proposal) => void }) {
  const { state, toast } = useApp();
  if (!list?.length && !error) return null;
  const me = state.signin.signed_in ? state.signin : null;
  return (
    <section className="mb-6 rounded-xl border border-hairline p-4" aria-label={`${kind} proposals`}>
      <h2 className="text-callout font-semibold">Proposals waiting for a key holder</h2>
      <ErrorText error={error} />
      <ul className="mt-2 flex flex-col gap-2">
        {list?.map((p) => (
          <li key={Object.values(p.ids).join()} className="flex flex-wrap items-center gap-x-3 gap-y-1 text-caption">
            <span className="font-semibold">{visible(p.author.name)}</span>
            <span className="text-muted" title={when(p.created)}>
              {ago(p.created)}
            </span>
            {p.serial != null && <Badge tone="muted">based on serial {p.serial}</Badge>}
            {p.stale.length > 0 && (
              <Badge tone="warn" title="What these relays serve changed since the proposal was made: review it against what is live now.">
                stale on {p.stale.map(host).join(", ")}
              </Badge>
            )}
            {me?.upn && p.author.upn === me.upn && <Badge tone="muted">yours</Badge>}
            {p.note && <span className="w-full select-text text-muted">“{visible(p.note)}”</span>}
            <span className="ml-auto flex gap-2">
              {review && (
                <Button size="sm" variant="primary" onClick={() => review(p)}>
                  Review…
                </Button>
              )}
              <Button
                size="sm"
                variant="ghost"
                onClick={() =>
                  removeProposal(p).then(
                    () => reload(),
                    (e) => toast(e.message),
                  )
                }
              >
                Remove
              </Button>
            </span>
          </li>
        ))}
      </ul>
    </section>
  );
}

/** The sheet that stores a draft as a proposal on every relay. */
export function ProposeSheet({ kind, doc, changes, onClose, onDone }: { kind: Proposal["kind"]; doc: unknown; changes: string[]; onClose: () => void; onDone?: () => void }) {
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [results, setResults] = useState<RelayResult[] | null>(null);
  const go = async () => {
    setBusy(true);
    setError(null);
    try {
      setResults(await invoke<RelayResult[]>("proposal_put", { kind, doc, note }));
      onDone?.();
    } catch (e: any) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  };
  return (
    <Modal
      onClose={onClose}
      width={600}
      title={
        <>
          <h2 className="text-headline font-semibold">Propose {kind === "team" ? "team document" : kind} change</h2>
          <p className="mt-1 text-caption text-muted">Stored on the relays unsigned, with your name; nothing changes for devices or staff until a key holder reviews, signs and publishes it.</p>
        </>
      }
      footer={
        <div className="flex justify-end gap-2">
          <Button onClick={onClose}>{results ? "Done" : "Cancel"}</Button>
          {!results && (
            <Button variant="primary" disabled={busy || !changes.length} onClick={go}>
              {busy ? "Storing…" : "Propose"}
            </Button>
          )}
        </div>
      }
    >
      {results ? (
        <RelayResults results={results} ok={() => "proposal stored"} />
      ) : (
        <div className="flex flex-col gap-3">
          <ul className="flex max-h-64 flex-col gap-1 overflow-auto text-caption">
            {changes.length ? changes.map((c, i) => <li key={i}>{c}</li>) : <li>No changes against what is live.</li>}
          </ul>
          <Label text="Note for the key holder" hint="Why, ticket number. At most 500 characters.">
            <TextArea rows={3} maxLength={500} value={note} onChange={(e) => setNote(e.target.value)} />
          </Label>
          <ErrorText error={error} />
        </div>
      )}
    </Modal>
  );
}

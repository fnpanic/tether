import { useEffect, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { RefreshCw } from "lucide-react";
import { useApp } from "../App";
import { api, bytes, invoke, when } from "../lib/ipc";
import { Button, Card, ErrorText, IconButton, Modal, PageHeader, Segmented, Table, TextField } from "../components/ui";
import { RelayResults } from "../components/publish";
import type { RelayResult } from "../lib/ipc";

interface Ticket {
  ticket: string;
  note: string;
  created: number;
  uploaded: number | null;
  size: number | null;
  /** open → uploaded → closed (archived); open → expired (24 h without upload). */
  state: "open" | "uploaded" | "expired" | "closed";
  closed?: number | null;
  /** Relays that have the ticket, and the one with the upload (diag_list merges them). */
  relays: string[];
  upload_relay?: string;
}
interface Opened {
  ticket: string;
  size: number;
  files: { name: string; size: number }[];
  diagnostics: string | null;
}

export function Diagnostics() {
  const { toast } = useApp();
  const [note, setNote] = useState("");
  const [created, setCreated] = useState<{ ticket: string; expires: number } | null>(null);
  const [list, setList] = useState<Ticket[] | null>(null);
  const [opened, setOpened] = useState<Opened | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [tab, setTab] = useState<"active" | "archived">("active");
  const [failed, setFailed] = useState<string[]>([]);
  const [confirmDelete, setConfirmDelete] = useState<Ticket | null>(null);
  const [results, setResults] = useState<{ title: string; results: RelayResult[] } | null>(null);

  const load = (t = tab) =>
    invoke<{ tickets: Ticket[]; failed: string[] }>("diag_list", { archived: t === "archived" }).then(
      (r) => {
        setList(r.tickets);
        setFailed(r.failed);
      },
      (e) => setError(e.message),
    );
  useEffect(() => void load(), []);
  const showTab = (t: "active" | "archived") => {
    setTab(t);
    setList(null);
    load(t);
  };

  /** Close (archive) or delete on every relay; per-relay results when one failed. */
  const change = async (t: Ticket, cmd: "diag_close" | "diag_delete") => {
    setError(null);
    try {
      const r = await invoke<RelayResult[]>(cmd, { ticket: t.ticket });
      const what = cmd === "diag_close" ? "closed" : "deleted";
      if (r.some((x) => !x.ok)) setResults({ title: `Ticket ${t.ticket} ${what} on some relays only`, results: r });
      else toast(`Ticket ${t.ticket} ${what}`);
      load();
    } catch (e: any) {
      setError(e.message);
    }
  };

  const create = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);
    try {
      setCreated(await api.admin("POST", "/api/admin/diag-ticket", { note: note.trim() }));
      setNote("");
      load();
    } catch (e: any) {
      setError(e.message);
      load(); // "Saved on some relays": the code is in the list, and the apps try every relay
    }
  };

  const openTicket = async (ticket: string) => {
    setError(null);
    try {
      setOpened({ ticket, ...(await invoke<Omit<Opened, "ticket">>("diag_open", { ticket })) });
    } catch (e: any) {
      setError(e.message);
    }
  };

  const saveZip = async () => {
    const p = await save({ defaultPath: `tether-diagnostics-${opened!.ticket}.zip`, filters: [{ name: "Zip archive", extensions: ["zip"] }] });
    if (!p) return;
    try {
      await invoke("diag_save", { path: p });
      toast("Saved");
    } catch (e: any) {
      setError(e.message);
    }
  };

  return (
    <>
      <PageHeader title="Diagnostics" sub="Open a ticket, read the code to the user, and their app uploads a diagnostics bundle for it." />
      <Card>
        <form onSubmit={create} className="flex items-end gap-2">
          <label className="flex flex-1 flex-col gap-1.5">
            <span className="text-caption font-semibold">Note</span>
            <TextField maxLength={256} value={note} onChange={(e) => setNote(e.target.value)} placeholder="Who and what, e.g. Dr. Weber, black screen after login" />
          </label>
          <Button variant="primary">Create ticket</Button>
        </form>
      </Card>
      <div className="mt-4">
        <ErrorText error={error} />
      </div>

      <div className="mb-3 mt-8 flex items-center justify-between gap-3">
        <h2 className="text-headline font-semibold">Tickets</h2>
        <div className="flex items-center gap-2">
          <Segmented label="Tickets" value={tab} onChange={showTab} options={[{ value: "active", label: "Active" }, { value: "archived", label: "Archived" }]} />
          <IconButton label="Refresh" onClick={() => load()}>
            <RefreshCw size={16} />
          </IconButton>
        </div>
      </div>
      {failed.length > 0 && <p className="mb-2 text-caption text-warn">Not reachable: {failed.join("; ")}</p>}
      <Table
        head={tab === "archived" ? ["Code", "Note", "Created", "Closed", "Upload", ""] : ["Code", "Note", "Created", "Upload", ""]}
        empty={list && !list.length ? (tab === "archived" ? "No closed tickets in the last 90 days." : "No tickets in the last 7 days.") : false}
      >
        {(list ?? [])
          .sort((a, b) => b.created - a.created)
          .map((t) => (
            <tr key={t.ticket}>
              <td className="font-mono font-semibold tracking-wider select-text">{t.ticket}</td>
              <td className="max-w-[320px] truncate" title={t.note}>
                {t.note || "–"}
              </td>
              <td>{when(t.created)}</td>
              {tab === "archived" && <td>{t.closed ? when(t.closed) : "–"}</td>}
              <td className={t.uploaded ? "" : "text-muted"}>{t.uploaded ? `${bytes(t.size)} · ${when(t.uploaded)}` : t.state === "open" ? "Waiting" : t.state === "expired" ? "Expired, no upload" : "No upload"}</td>
              <td className="whitespace-nowrap text-right">
                <span className="inline-flex gap-1.5">
                  <Button size="sm" disabled={!t.uploaded} onClick={() => openTicket(t.ticket)}>
                    Open
                  </Button>
                  {tab === "active" && (
                    <Button size="sm" title="No more uploads; the ticket moves to Archived (kept 90 days)" onClick={() => change(t, "diag_close")}>
                      Close
                    </Button>
                  )}
                  <Button size="sm" title="Remove the ticket and its upload for good" onClick={() => setConfirmDelete(t)}>
                    Delete
                  </Button>
                </span>
              </td>
            </tr>
          ))}
      </Table>

      {created && (
        <Modal onClose={() => setCreated(null)} width={520}>
          <p className="text-callout text-muted">Read this code to the user. They enter it under Settings › Diagnostics › Send to support (technician app) or Help › Send to support (TetherQS).</p>
          <p className="my-8 select-text text-center font-mono text-[56px] font-semibold leading-none tracking-[0.12em]" aria-label={created.ticket.split("").join(" ")}>
            {created.ticket.slice(0, 4)} {created.ticket.slice(4)}
          </p>
          <p className="text-center text-caption text-muted">Valid until {when(created.expires)}. One upload per code.</p>
          <div className="mt-6 flex justify-end">
            <Button variant="primary" onClick={() => setCreated(null)}>
              Done
            </Button>
          </div>
        </Modal>
      )}

      {confirmDelete && (
        <Modal onClose={() => setConfirmDelete(null)} width={460}>
          <h2 className="text-headline font-semibold">Delete ticket {confirmDelete.ticket}?</h2>
          <p className="mt-2 text-callout text-muted">
            {confirmDelete.uploaded ? "The ticket and its diagnostics upload are removed from every relay for good." : "The ticket is removed from every relay for good."} This cannot be undone.
          </p>
          <div className="mt-6 flex justify-end gap-2">
            <Button onClick={() => setConfirmDelete(null)}>Cancel</Button>
            <Button
              variant="danger"
              onClick={() => {
                const t = confirmDelete;
                setConfirmDelete(null);
                change(t, "diag_delete");
              }}
            >
              Delete
            </Button>
          </div>
        </Modal>
      )}

      {results && (
        <Modal onClose={() => setResults(null)} width={560}>
          <h2 className="mb-3 text-headline font-semibold">{results.title}</h2>
          <RelayResults results={results.results} ok={(r) => (typeof r.detail === "string" ? r.detail : "Done")} />
          <div className="mt-6 flex justify-end">
            <Button variant="primary" onClick={() => setResults(null)}>
              Done
            </Button>
          </div>
        </Modal>
      )}

      {opened && (
        <Modal onClose={() => setOpened(null)} width={760}>
          <div className="flex items-start justify-between gap-4">
            <div>
              <h2 className="text-headline font-semibold">Ticket {opened.ticket}</h2>
              <p className="text-caption text-muted">
                {opened.files.length} files · {bytes(opened.size)}
              </p>
            </div>
            <Button onClick={saveZip}>Save zip…</Button>
          </div>
          <ul className="mt-4 max-h-40 overflow-auto rounded-xl bg-parchment px-4 py-2">
            {opened.files.map((f) => (
              <li key={f.name} className="flex justify-between py-0.5 text-footnote">
                <span className="font-mono">{f.name}</span>
                <span className="text-muted">{bytes(f.size)}</span>
              </li>
            ))}
          </ul>
          {opened.diagnostics ? (
            <>
              <h3 className="mb-2 mt-4 text-caption font-semibold">diagnostics.json</h3>
              <pre className="max-h-[40vh] select-text overflow-auto rounded-xl bg-parchment p-4 font-mono text-footnote">{opened.diagnostics}</pre>
            </>
          ) : (
            <p className="mt-4 text-caption text-muted">This upload has no diagnostics.json.</p>
          )}
        </Modal>
      )}
    </>
  );
}

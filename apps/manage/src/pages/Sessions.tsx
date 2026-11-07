import { useEffect, useState } from "react";
import { Download, RefreshCw } from "lucide-react";
import { save } from "@tauri-apps/plugin-dialog";
import { useApp } from "../App";
import { api, host, inTauri, invoke, type Device } from "../lib/ipc";
import { actionLabel, alarmLabel, duration, matches, sessions, toCsv, type AuditEvent, type Row } from "../lib/sessions";
import { paginate } from "../lib/paging";
import { Badge, Button, ErrorText, IconButton, PageHeader, Pager, Segmented, Table, TextField } from "../components/ui";

type Range = "1" | "7" | "30" | "90";
const LIMIT = 5000;

export function Sessions() {
  const { toast } = useApp();
  const [range, setRange] = useState<Range>("7");
  const [events, setEvents] = useState<AuditEvent[] | null>(null);
  const [failed, setFailed] = useState<string[]>([]);
  const [devices, setDevices] = useState<Device[]>([]);
  const [q, setQ] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [page, setPage] = useState(0);

  const load = () => {
    setError(null);
    setEvents(null);
    const since = Math.floor(Date.now() / 1000) - +range * 86400;
    invoke<{ events: AuditEvent[]; failed: string[] }>("audit_all", { since, limit: LIMIT }).then(
      (r) => {
        setEvents(r.events);
        setFailed(r.failed);
      },
      (e) => setError(e.message),
    );
    api.devices().then((r) => setDevices(r.devices), () => {});
  };
  useEffect(load, [range]);

  const all = events ? sessions(events) : [];
  const rows = all.filter((r) => matches(r, q));
  useEffect(() => setPage(0), [q, range]);
  const pg = paginate(rows, page);
  const os = new Map(devices.map((d) => [d.id.replace(/\s+/g, ""), d]));
  const device = (r: Row) => os.get(r.id.replace(/\s+/g, ""));

  const exportCsv = async () => {
    const name = `tether-sessions-${new Date().toISOString().slice(0, 10)}.csv`;
    const text = toCsv(rows);
    if (!inTauri) {
      const a = Object.assign(document.createElement("a"), { href: URL.createObjectURL(new Blob([text], { type: "text/csv" })), download: name });
      a.click();
      return;
    }
    const p = await save({ defaultPath: name, filters: [{ name: "CSV", extensions: ["csv"] }] });
    if (!p) return;
    try {
      await invoke("text_save", { path: p, text });
      toast(`Saved ${rows.length} rows`);
    } catch (e: any) {
      setError(e.message);
    }
  };

  return (
    <>
      <PageHeader title="Sessions" sub="Connections and file transfers as the hosts reported them to the relays. Hosts post these records without a device key: treat them as a log, not as proof (SECURITY.md).">
        <Button size="sm" onClick={exportCsv} disabled={!rows.length}>
          <Download size={14} /> Export CSV
        </Button>
        <IconButton label="Refresh" onClick={load}>
          <RefreshCw size={16} />
        </IconButton>
      </PageHeader>

      <div className="mb-3 flex flex-wrap items-center gap-2">
        <TextField className="max-w-64 rounded-full" placeholder="Host or technician ID, name, IP, path…" aria-label="Search sessions" value={q} onChange={(e) => setQ(e.target.value)} />
        <Segmented<Range>
          label="Time range"
          value={range}
          onChange={setRange}
          options={[
            { value: "1", label: "24 h" },
            { value: "7", label: "7 days" },
            { value: "30", label: "30 days" },
            { value: "90", label: "90 days" },
          ]}
        />
      </div>
      <p className="mb-4 text-caption text-muted" data-testid="sessions-summary">
        {events ? (
          <>
            Showing <span className="font-semibold text-ink">{rows.length}</span> of {all.length} events
            {all.length >= LIMIT && ` · only the newest ${LIMIT} per relay; pick a shorter range`}
            {q && (
              <button className="ml-2 text-accent" onClick={() => setQ("")}>
                Clear search
              </button>
            )}
          </>
        ) : error ? null : (
          "Reading the session log from every relay…"
        )}
      </p>
      <div className="mb-4 flex flex-col gap-1">
        <ErrorText error={error} />
        {failed.map((f) => (
          <ErrorText key={f} error={`Sessions of one relay are missing: ${f}`} />
        ))}
      </div>
      <Table head={["Time", "Host", "Technician", "IP", "Action", "Duration"]} empty={events && !rows.length ? (q ? "No event matches the search." : "No session in this time range.") : false}>
        {pg.items.map((r) => {
          const d = device(r);
          return (
            <tr key={`${r.nonce ?? ""}|${r.uuid}|${r.conn_id}|${r.action}|${r.ts}`} data-testid="session-row">
              <td className="whitespace-nowrap" title={r.relay ? `Logged by ${host(r.relay)}` : undefined}>
                {new Date(r.ts * 1000).toLocaleString()}
              </td>
              <td>
                <p className="font-mono">{r.id || "–"}</p>
                {d && <p className="text-footnote text-faint">{d.os}</p>}
              </td>
              <td>
                {r.peer_name && <p>{r.peer_name}</p>}
                <p className="font-mono text-footnote text-muted">{r.peer_id || "–"}</p>
              </td>
              <td className="font-mono text-footnote" title={r.src ? `Host posted from ${r.src}` : undefined}>
                {r.ip || "–"}
              </td>
              <td>
                <span className="flex items-center gap-1.5">
                  {actionLabel(r)}
                  {r.conn_type && r.conn_type !== "remote" && <Badge>{r.conn_type.replace("_", " ")}</Badge>}
                  {r.kind === "alarm" && <Badge tone="warn">{alarmLabel(r.typ)}</Badge>}
                </span>
                {r.path != null && (
                  <p className="select-text break-all font-mono text-footnote text-muted" title={r.files?.map((f) => f[0]).join("\n")}>
                    {r.path || "(clipboard)"}
                    {r.num != null && ` · ${r.num} file${r.num === 1 ? "" : "s"}`}
                  </p>
                )}
                {r.kind === "alarm" && r.info && <p className="select-text break-all font-mono text-footnote text-muted">{r.info}</p>}
              </td>
              <td className="whitespace-nowrap">{duration(r.duration)}</td>
            </tr>
          );
        })}
      </Table>
      <Pager {...pg} onPage={setPage} noun="events" />
    </>
  );
}

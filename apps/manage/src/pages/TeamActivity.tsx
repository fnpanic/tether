import { useEffect, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { Download, RefreshCw } from "lucide-react";
import { useApp } from "../App";
import { host, inTauri, invoke, when } from "../lib/ipc";
import { activityCsv, detailText, refused, targetText, who, type ActivityRecord, type RelayState } from "../lib/activity";
import { Badge, Button, ErrorText, IconButton, Label, PageHeader, Select, Table, TextField } from "../components/ui";

const SPANS = [
  { value: "1", label: "Last 24 hours" },
  { value: "7", label: "Last 7 days" },
  { value: "30", label: "Last 30 days" },
  { value: "90", label: "Last 90 days" },
];

/** What staff did on the relays (design 12.5): every relay's team audit log, newest first. */
export function TeamActivity() {
  const { toast } = useApp();
  const [span, setSpan] = useState("7");
  const [actor, setActor] = useState("");
  const [device, setDevice] = useState("");
  const [customer, setCustomer] = useState("");
  const [event, setEvent] = useState("");
  const [data, setData] = useState<{ records: ActivityRecord[]; relays: RelayState[] } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = async () => {
    setBusy(true);
    setError(null);
    try {
      const since = Math.floor(Date.now() / 1000) - Number(span) * 86400;
      setData(await invoke("team_activity", { filter: { since, actor, device, customer, event, limit: 2000 } }));
    } catch (e: any) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  };
  useEffect(() => void load(), [span]);

  const accept = async (server: string) => {
    try {
      await invoke("team_activity_accept", { server });
      await load();
    } catch (e: any) {
      toast(e.message);
    }
  };

  const exportCsv = async () => {
    if (!data) return;
    const path = inTauri ? await save({ defaultPath: "tether-team-activity.csv", filters: [{ name: "CSV", extensions: ["csv"] }] }) : null;
    if (inTauri && !path) return;
    await invoke("text_save", { path, text: activityCsv(data.records) });
    // The export is recorded on the relays (Manage's own record).
    invoke("audit_export", { what: "team-activity", rows: data.records.length }).catch(() => {});
    toast(`Exported ${data.records.length} records.`);
  };

  const warnings = data?.relays.flatMap((r) => r.warnings.map((w) => ({ server: r.server, w, broken: !!r.broken }))) ?? [];

  return (
    <>
      <PageHeader title="Team activity" sub="What staff did on every relay: sign-ins, host opens, searches, changes, Tether Manage actions. Kept on the relays; this PC keeps only each relay's last chain head.">
        <IconButton label="Read again" onClick={load}>
          <RefreshCw size={16} />
        </IconButton>
        <Button onClick={exportCsv} disabled={!data?.records.length}>
          <Download size={16} /> Export CSV…
        </Button>
      </PageHeader>
      <form
        className="mb-4 grid grid-cols-5 items-end gap-3"
        onSubmit={(e) => {
          e.preventDefault();
          load();
        }}
      >
        <Label text="Period">
          <Select value={span} onChange={(e) => setSpan(e.target.value)}>
            {SPANS.map((s) => (
              <option key={s.value} value={s.value}>
                {s.label}
              </option>
            ))}
          </Select>
        </Label>
        <Label text="Person" hint="Name, user name, oid or key id">
          <TextField value={actor} onChange={(e) => setActor(e.target.value)} />
        </Label>
        <Label text="Device" hint="Tether ID or uuid">
          <TextField value={device} onChange={(e) => setDevice(e.target.value)} />
        </Label>
        <Label text="Customer">
          <TextField value={customer} onChange={(e) => setCustomer(e.target.value)} />
        </Label>
        <Label text="Event" hint="e.g. sign-in, report, search, policy-publish">
          <TextField value={event} onChange={(e) => setEvent(e.target.value)} />
        </Label>
        <button type="submit" hidden />
      </form>
      <ErrorText error={error} />
      {warnings.map(({ server, w, broken }, i) => (
        <div key={i} role="alert" className="mb-2 flex items-center gap-3 rounded-lg border border-danger/50 p-3 text-caption text-danger">
          <span className="select-text">{w}</span>
          {broken && (
            <Button size="sm" className="ml-auto" onClick={() => accept(server)}>
              Accept the new state
            </Button>
          )}
        </div>
      ))}
      {data?.relays
        .filter((r) => !r.ok)
        .map((r) => (
          <p key={r.server} className="select-text text-caption text-danger">
            {host(r.server)}: {r.error}
          </p>
        ))}
      {data?.relays.some((r) => r.more) && <p className="mb-2 text-caption text-muted">More records than shown: narrow the period or the filters.</p>}
      {busy && !data && <p className="text-caption text-muted">Reading…</p>}
      {data && (
        <Table head={["Time", "Relay", "Person", "Event", "Device", "Detail", "Result"]} empty={!data.records.length && "No records for these filters."}>
          {data.records.map((r, i) => (
            <tr key={i}>
              <td className="whitespace-nowrap">{when(r.ts)}</td>
              <td>{host(r.relay)}</td>
              <td className="select-text">{who(r)}</td>
              <td>
                {r.event}
                {r.self_reported && (
                  <Badge tone="muted" title="Reported by the person's app, not seen by the relay">
                    self-reported
                  </Badge>
                )}
              </td>
              <td className="select-text">{targetText(r)}</td>
              <td className="max-w-[260px] truncate select-text text-footnote" title={detailText(r)}>
                {detailText(r)}
              </td>
              <td className={refused(r) ? "text-danger" : ""}>{r.result ?? ""}</td>
            </tr>
          ))}
        </Table>
      )}
    </>
  );
}

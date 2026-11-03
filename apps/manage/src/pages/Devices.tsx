import { useEffect, useState } from "react";
import { RefreshCw } from "lucide-react";
import { useApp } from "../App";
import { ago, api, type Device } from "../lib/ipc";
import { Button, Card, ErrorText, IconButton, PageHeader, Select, Table, TextField } from "../components/ui";

export function Devices() {
  const { draft, setDraft, dirty, markPublished, toast, go } = useApp();
  const [devices, setDevices] = useState<Device[] | null>(null);
  const [filter, setFilter] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = () => {
    setError(null);
    api.devices().then(setDevices, (e) => setError(e.message));
  };
  useEffect(load, []);

  const q = filter.trim().toLowerCase();
  const rows = (devices ?? [])
    .filter((d) => !q || [d.id, d.uuid, d.profile, d.version, d.os, d.ip, draft.assignments[d.uuid] ?? ""].some((v) => String(v).toLowerCase().includes(q)))
    .sort((a, b) => b.last_seen - a.last_seen);

  const assign = (uuid: string, group: string) => {
    const assignments = { ...draft.assignments };
    if (group) assignments[uuid] = group;
    else delete assignments[uuid];
    setDraft({ ...draft, assignments });
  };

  const publishNow = async () => {
    setBusy(true);
    setError(null);
    try {
      const serial = await api.publish(draft);
      markPublished({ ...draft, serial, issued_at: Math.floor(Date.now() / 1000) });
      toast(`Published as version ${serial}`);
    } catch (e: any) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <PageHeader title="Devices" sub="Every Tether app that checked in during the last 180 days.">
        <TextField className="w-64 rounded-full" placeholder="Filter" aria-label="Filter devices" value={filter} onChange={(e) => setFilter(e.target.value)} />
        <IconButton label="Refresh" onClick={load}>
          <RefreshCw size={16} />
        </IconButton>
      </PageHeader>
      {dirty && (
        <Card className="mb-4 flex items-center gap-4 py-4">
          <p className="flex-1 text-callout">Group assignments changed. Devices pick them up after you publish the policy.</p>
          <Button onClick={() => go("policy")}>Review</Button>
          <Button variant="primary" disabled={busy} onClick={publishNow}>
            {busy ? "Publishing…" : "Publish now"}
          </Button>
        </Card>
      )}
      <div className="mb-4">
        <ErrorText error={error} />
      </div>
      <Table head={["ID", "Profile", "Version", "OS", "Policy", "Last seen", "IP", "Group"]} empty={devices && !rows.length ? (q ? "No device matches the filter." : "No device has checked in yet.") : false}>
        {rows.map((d) => {
          const groups = Object.entries(draft.groups).filter(([, g]) => g.profile === d.profile);
          const stale = d.serial < draft.serial;
          return (
            <tr key={d.uuid}>
              <td>
                <p className="font-mono">{d.id || "–"}</p>
                <p className="font-mono text-footnote text-faint select-text">{d.uuid}</p>
              </td>
              <td className="capitalize">{d.profile}</td>
              <td>{d.version}</td>
              <td className="text-muted">{d.os}</td>
              <td className={stale ? "text-warn" : ""} title={stale ? "Has not applied the latest published policy yet" : ""}>
                {d.serial}
              </td>
              <td title={new Date(d.last_seen * 1000).toLocaleString()}>{ago(d.last_seen)}</td>
              <td className="font-mono text-footnote">{d.ip}</td>
              <td>
                <Select className="w-40" aria-label={`Group of ${d.id || d.uuid}`} value={draft.assignments[d.uuid] ?? ""} onChange={(e) => assign(d.uuid, e.target.value)}>
                  <option value="">No group</option>
                  {groups.map(([name]) => (
                    <option key={name}>{name}</option>
                  ))}
                </Select>
              </td>
            </tr>
          );
        })}
      </Table>
    </>
  );
}

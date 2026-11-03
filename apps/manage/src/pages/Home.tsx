import { useEffect, useState } from "react";
import { RefreshCw } from "lucide-react";
import { useApp } from "../App";
import { api, bytes } from "../lib/ipc";
import { Card, ErrorText, IconButton, PageHeader, StatusDot } from "../components/ui";

interface Status {
  version: string;
  disk_free: number;
  devices: number;
  policy_serial: number | null;
  uptime: number;
}

function uptime(s: number) {
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  return d ? `${d} d ${h} h` : `${h} h ${Math.floor((s % 3600) / 60)} min`;
}

export function Home() {
  const { state, dirty, go } = useApp();
  const [status, setStatus] = useState<Status | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = () => {
    setError(null);
    api.admin<Status>("GET", "/api/admin/status").then(setStatus, (e) => {
      setStatus(null);
      setError(e.message);
    });
  };
  useEffect(load, []);

  const tiles: [string, string, string?][] = status
    ? [
        ["Policy version", status.policy_serial ? String(status.policy_serial) : "None published"],
        ["Devices", String(status.devices)],
        ["Free disk", bytes(status.disk_free), status.disk_free < 2 * 1024 ** 3 ? "Uploads are refused below 2 GB" : undefined],
        ["Uptime", uptime(status.uptime)],
        ["Relay API", status.version],
      ]
    : [];

  return (
    <>
      <PageHeader title="Status" sub={state.settings.server}>
        <IconButton label="Refresh" onClick={load}>
          <RefreshCw size={16} />
        </IconButton>
      </PageHeader>
      <div className="mb-6 flex items-center gap-2 text-caption text-muted">
        <StatusDot tone={error ? "danger" : status ? "ok" : "muted"} />
        {error ? "Not connected" : status ? "Connected, admin key accepted" : "Connecting…"}
      </div>
      <ErrorText error={error} />
      <div className="grid grid-cols-[repeat(auto-fill,minmax(200px,1fr))] gap-6">
        {tiles.map(([k, v, warn]) => (
          <Card key={k}>
            <p className="text-caption text-muted">{k}</p>
            <p className="mt-2 text-title font-semibold">{v}</p>
            {warn && <p className="mt-1 text-footnote text-warn">{warn}</p>}
          </Card>
        ))}
      </div>
      {dirty && (
        <Card className="mt-6 flex items-center justify-between">
          <p className="text-callout">The policy has changes that are not published yet.</p>
          <button className="text-callout text-accent" onClick={() => go("policy")}>
            Review and publish
          </button>
        </Card>
      )}
    </>
  );
}

import { useEffect, useState } from "react";
import { RefreshCw } from "lucide-react";
import { useApp } from "../App";
import { ago, api, midnight, type Device } from "../lib/ipc";
import { paginate } from "../lib/paging";
import { compareVersion } from "../lib/releases";
import { Badge, ErrorText, IconButton, PageHeader, Pager, Segmented, Select, Table, TextField } from "../components/ui";

/** "windows 10.0.26300" → "windows"; "macos 15.1" / "darwin 24.1" → "macos". */
const family = (os: string) => {
  const f = os.trim().split(/\s+/)[0]?.toLowerCase() ?? "";
  return f === "darwin" || f === "mac" ? "macos" : f || "unknown";
};
const FAMILY: Record<string, string> = { windows: "Windows", macos: "macOS", linux: "Linux" };
const familyName = (f: string) => FAMILY[f] ?? f;

/** Newest version first. */
const byVersion = (a: string, b: string) => compareVersion(b, a);

export function Devices() {
  const { draft, setDraft, live } = useApp();
  const [devices, setDevices] = useState<Device[] | null>(null);
  const [failed, setFailed] = useState<string[]>([]);
  const [q, setQ] = useState("");
  const [when, setWhen] = useState<"all" | "today">("all");
  const [os, setOs] = useState("");
  const [version, setVersion] = useState("");
  const [profile, setProfile] = useState("");
  const [group, setGroup] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [page, setPage] = useState(0);

  const load = () => {
    setError(null);
    api.devices().then(
      (r) => {
        setDevices(r.devices);
        setFailed(r.failed);
      },
      (e) => setError(e.message),
    );
  };
  useEffect(load, []);

  const all = devices ?? [];
  const today = midnight();
  const activeToday = all.filter((d) => d.last_seen >= today).length;
  const families = [...new Set(all.map((d) => family(d.os)))].sort();
  const versions = [...new Set(all.map((d) => d.version))].sort(byVersion);
  const text = q.trim().toLowerCase();
  const rows = all
    .filter((d) => when === "all" || d.last_seen >= today)
    .filter((d) => !os || family(d.os) === os)
    .filter((d) => !version || d.version === version)
    .filter((d) => !profile || d.profile === profile)
    .filter((d) => !group || (group === "-" ? !draft.assignments[d.uuid] : draft.assignments[d.uuid] === group))
    .filter((d) => !text || [d.id, d.uuid, d.profile, d.version, d.os, d.ip, draft.assignments[d.uuid] ?? ""].some((v) => String(v).toLowerCase().includes(text)))
    .sort((a, b) => b.last_seen - a.last_seen);
  const filtered = when !== "all" || os || version || profile || group || text;
  // Back to page 1 whenever the filters change; paginate() clamps if the list shrinks otherwise.
  useEffect(() => setPage(0), [when, os, version, profile, group, text]);
  const pg = paginate(rows, page);

  const assign = (uuid: string, g: string) => {
    const assignments = { ...draft.assignments };
    if (g) assignments[uuid] = g;
    else delete assignments[uuid];
    setDraft({ ...draft, assignments });
  };

  const liveSerial = live?.serial ?? null;

  return (
    <>
      <PageHeader title="Devices" sub="Every Tether app that checked in with any relay during the last 180 days. Group changes go into the policy draft; publish them from the bar above.">
        <IconButton label="Refresh" onClick={load}>
          <RefreshCw size={16} />
        </IconButton>
      </PageHeader>

      <div className="mb-3 flex flex-wrap items-center gap-2">
        <TextField className="max-w-56 rounded-full" placeholder="Search ID, uuid, IP…" aria-label="Search devices" value={q} onChange={(e) => setQ(e.target.value)} />
        <Segmented<"all" | "today">
          label="Last seen"
          value={when}
          onChange={setWhen}
          options={[
            { value: "all", label: "All", count: all.length },
            { value: "today", label: "Active today", count: activeToday },
          ]}
        />
        <Select className={`max-w-40 ${os ? "border-accent! font-semibold" : ""}`} aria-label="Operating system" value={os} onChange={(e) => setOs(e.target.value)}>
          <option value="">Any OS</option>
          {families.map((f) => (
            <option key={f} value={f}>
              {familyName(f)} ({all.filter((d) => family(d.os) === f).length})
            </option>
          ))}
        </Select>
        <Select className={`max-w-40 ${version ? "border-accent! font-semibold" : ""}`} aria-label="App version" value={version} onChange={(e) => setVersion(e.target.value)}>
          <option value="">Any version</option>
          {versions.map((v) => (
            <option key={v} value={v}>
              {v} ({all.filter((d) => d.version === v).length})
            </option>
          ))}
        </Select>
        <Select className={`max-w-40 ${profile ? "border-accent! font-semibold" : ""}`} aria-label="Profile" value={profile} onChange={(e) => setProfile(e.target.value)}>
          <option value="">Any profile</option>
          <option value="technician">Technician</option>
          <option value="customer">Customer</option>
        </Select>
        <Select className={`max-w-40 ${group ? "border-accent! font-semibold" : ""}`} aria-label="Group" value={group} onChange={(e) => setGroup(e.target.value)}>
          <option value="">Any group</option>
          <option value="-">No group</option>
          {Object.keys(draft.groups).map((g) => (
            <option key={g}>{g}</option>
          ))}
        </Select>
      </div>
      <p className="mb-4 text-caption text-muted">
        {devices ? (
          <>
            Showing <span className="font-semibold text-ink">{rows.length}</span> of {all.length} devices · {activeToday} active today (last seen since 00:00 local time)
            {liveSerial != null && ` · ${all.filter((d) => d.serial < liveSerial).length} not on the live policy (serial ${liveSerial}) yet`}
            {filtered && (
              <button
                className="ml-2 text-accent"
                onClick={() => {
                  setQ("");
                  setWhen("all");
                  setOs("");
                  setVersion("");
                  setProfile("");
                  setGroup("");
                }}
              >
                Clear filters
              </button>
            )}
          </>
        ) : (
          "Reading devices from every relay…"
        )}
      </p>
      <div className="mb-4 flex flex-col gap-1">
        <ErrorText error={error} />
        {failed.map((f) => (
          <ErrorText key={f} error={`Devices of one relay are missing: ${f}`} />
        ))}
      </div>
      <Table head={["ID", "Profile", "Version", "OS", "Policy", "Last seen", "IP", "Group"]} empty={devices && !rows.length ? (filtered ? "No device matches the filters." : "No device has checked in yet.") : false}>
        {pg.items.map((d) => {
          const groups = Object.entries(draft.groups).filter(([, g]) => g.profile === d.profile);
          const stale = liveSerial != null && d.serial < liveSerial;
          return (
            <tr key={d.uuid}>
              <td>
                <p className="font-mono">{d.id || "–"}</p>
                <p className="select-text font-mono text-footnote text-faint">{d.uuid}</p>
              </td>
              <td className="capitalize">{d.profile}</td>
              <td>{d.version}</td>
              <td className="text-muted">{d.os}</td>
              <td>
                <span className="flex items-center gap-1.5">
                  {d.serial}
                  {liveSerial != null &&
                    (stale ? (
                      <Badge tone="warn" title={`Live is serial ${liveSerial}. The device picks it up at its next policy check.`}>
                        Stale
                      </Badge>
                    ) : (
                      <Badge tone="ok">Current</Badge>
                    ))}
                </span>
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
      <Pager {...pg} onPage={setPage} noun="devices" />
    </>
  );
}

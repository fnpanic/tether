import { useEffect, useState } from "react";
import { Plus, RefreshCw, Trash2 } from "lucide-react";
import { useApp } from "../App";
import { api, bytes, host, type RelayResult } from "../lib/ipc";
import { liveCount } from "../components/policy-publish";
import { Badge, Button, Card, ErrorText, IconButton, PageHeader, StatusDot, Table, TextField } from "../components/ui";

function uptime(s: number) {
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  return d ? `${d} d ${h} h` : `${h} h ${Math.floor((s % 3600) / 60)} min`;
}

/** The relay list is the most important setting: every publish goes to exactly these relays. */
function Relays() {
  const { state, reload, live, reloadLive, toast } = useApp();
  const [list, setList] = useState<string[]>(state.servers);
  const [add, setAdd] = useState("");
  const [health, setHealth] = useState<RelayResult[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const changed = list.join(",") !== state.servers.join(",");

  const check = () => {
    setHealth(null);
    api.relays().then(setHealth, (e) => setError(e.message));
  };
  useEffect(check, [state.settings.server]);
  useEffect(() => setList(state.servers), [state.settings.server]);

  const save = async () => {
    setError(null);
    try {
      await api.saveSettings({ ...state.settings, server: list.join(", ") });
      await reload();
      reloadLive();
      toast("Relay list saved");
    } catch (e: any) {
      setError(e.message);
    }
  };
  const addOne = () => {
    let v = add.trim().replace(/\/+$/, "");
    if (!v) return;
    if (!/^https:\/\//.test(v)) v = `https://${v.replace(/^\w+:\/\//, "")}`;
    if (!list.includes(v)) setList([...list, v]);
    setAdd("");
  };
  const hp = (sv: string) => health?.find((h) => h.server === sv);
  const pol = (sv: string) => live?.relays.find((r) => r.server === sv);

  return (
    <Card>
      <div className="flex items-start justify-between gap-4">
        <div>
          <h2 className="text-headline font-semibold">Relays</h2>
          <p className="mt-1 max-w-[640px] text-caption text-muted">
            Every publish (policy, releases, download page, custom apps) goes to all of these. Reads use the first one that answers. Each relay keeps its own full copy, so any of them can serve devices alone.
          </p>
        </div>
        <IconButton label="Check again" onClick={check}>
          <RefreshCw size={16} />
        </IconButton>
      </div>
      {live?.unconfigured.length ? (
        <p className="mt-3 rounded-xl border border-warn/60 p-3 text-caption">
          <span className="font-semibold text-warn">The policy sends devices to {live.unconfigured.join(", ")}, which is not in this list.</span>{" "}
          <span className="text-muted">Publishes do not reach it, so its devices keep old policies and releases. Add it below.</span>{" "}
          {live.unconfigured.map((h) => (
            <button key={h} className="text-accent" onClick={() => !list.includes(`https://${h}`) && setList([...list, `https://${h}`])}>
              Add {h}
            </button>
          ))}
        </p>
      ) : null}
      <div className="mt-4">
        <Table head={["Relay", "Health", "Relay API", "Policy", "Devices", "Free disk", "Uptime", ""]}>
          {list.map((sv, i) => {
            const h = hp(sv);
            const p = pol(sv);
            const saved = state.servers.includes(sv);
            return (
              <tr key={sv}>
                <td>
                  <p className="font-semibold">{host(sv)}</p>
                  <p className="text-footnote text-muted">{i === 0 ? "First: used for reads and links" : "Mirror"}</p>
                </td>
                <td>
                  {!saved ? (
                    <Badge tone="accent">Not saved yet</Badge>
                  ) : !health ? (
                    <span className="text-muted">Checking…</span>
                  ) : h?.ok ? (
                    <span className="flex items-center gap-2">
                      <StatusDot tone="ok" /> Admin key accepted
                    </span>
                  ) : (
                    <span className="flex items-center gap-2 text-danger" title={h?.error}>
                      <StatusDot tone="danger" /> <span className="max-w-[220px] truncate">{h?.error ?? "No answer"}</span>
                    </span>
                  )}
                </td>
                <td>{h?.detail?.version ?? "–"}</td>
                <td>
                  {p?.serial ?? h?.detail?.policy_serial ?? "–"}{" "}
                  {p && live?.serial != null && (p.verified && p.serial === live.serial ? <Badge tone="ok">Live</Badge> : <Badge tone="warn">Behind</Badge>)}
                </td>
                <td>{h?.detail?.devices ?? "–"}</td>
                <td className={h?.detail?.disk_free < 2 * 1024 ** 3 ? "text-warn" : ""}>{h?.ok ? bytes(h.detail.disk_free) : "–"}</td>
                <td>{h?.ok ? uptime(h.detail.uptime) : "–"}</td>
                <td className="w-12">
                  <IconButton label={`Remove ${host(sv)}`} disabled={list.length === 1} onClick={() => setList(list.filter((x) => x !== sv))}>
                    <Trash2 size={15} />
                  </IconButton>
                </td>
              </tr>
            );
          })}
        </Table>
      </div>
      <form
        className="mt-4 flex items-center gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          addOne();
        }}
      >
        <TextField className="max-w-sm" placeholder="tether-desk2.duckdns.org" aria-label="Relay to add" value={add} onChange={(e) => setAdd(e.target.value)} />
        <Button type="submit" disabled={!add.trim()}>
          <Plus size={14} /> Add relay
        </Button>
        <span className="flex-1" />
        {changed && (
          <Button type="button" onClick={() => setList(state.servers)}>
            Discard
          </Button>
        )}
        <Button type="button" variant="primary" disabled={!changed} onClick={save}>
          Save relay list
        </Button>
      </form>
      <div className="mt-3">
        <ErrorText error={error} />
      </div>
    </Card>
  );
}

export function Home() {
  const { live, changes, dirty, go, publish, invalid } = useApp();
  const { on, of } = liveCount(live);
  return (
    <>
      <PageHeader title="Status and relays" />
      <div className="flex flex-col gap-6">
        <Relays />
        <Card className="flex flex-wrap items-center gap-6">
          <div className="min-w-[240px] flex-1">
            <p className="text-caption text-muted">Live policy</p>
            <p className="mt-1 text-title font-semibold">{live?.serial != null ? `Serial ${live.serial}` : live ? "None published" : "…"}</p>
            <p className="mt-1 text-caption text-muted">{live?.serial != null ? `On ${on} of ${of} relays` : ""}</p>
          </div>
          <div className="min-w-[240px] flex-1">
            <p className="text-caption text-muted">Draft</p>
            <p className="mt-1 text-title font-semibold">{dirty ? `${changes.length} unpublished change${changes.length === 1 ? "" : "s"}` : "Same as live"}</p>
            <button className="mt-1 text-caption text-accent" onClick={() => go("policy")}>
              Open the policy
            </button>
          </div>
          {dirty && (
            <Button variant="primary" disabled={!!invalid} onClick={publish}>
              Review and publish…
            </Button>
          )}
        </Card>
      </div>
    </>
  );
}

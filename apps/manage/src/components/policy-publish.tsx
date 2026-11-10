// The persistent policy bar (draft vs live) and the policy publish sheet.
import { useEffect, useState } from "react";
import { RefreshCw } from "lucide-react";
import { useApp } from "../App";
import { api, host, invoke, midnight, type Device, type RelayResult } from "../lib/ipc";
import { liveCount, liveLine } from "../lib/live";
import { ChangeList, PublishSheet, RelayResults } from "./publish";
import { Badge, Button, IconButton, Modal, StatusDot } from "./ui";

export function PolicyBar() {
  const { live, liveError, changes, dirty, invalid, publish, go, reloadLive } = useApp();
  const [synced, setSynced] = useState<RelayResult[] | null>(null);
  const [busy, setBusy] = useState(false);
  const { on, of } = liveCount(live);
  const behind = live?.serial != null && on < of;
  const line = liveLine(live, liveError);

  const sync = async () => {
    setBusy(true);
    try {
      setSynced(await invoke<RelayResult[]>("policy_sync"));
    } catch (e: any) {
      setSynced([{ server: "", ok: false, error: e.message, detail: null }]);
    } finally {
      setBusy(false);
      reloadLive();
    }
  };

  return (
    <div className="flex flex-wrap items-center gap-x-4 gap-y-2 border-b border-hairline bg-parchment/60 px-8 py-2.5 text-caption" role="status" aria-label="Policy state">
      <span className="flex items-center gap-2">
        <StatusDot tone={invalid ? "danger" : dirty ? "warn" : "ok"} />
        <span className="font-semibold">{invalid ? "Draft is not publishable" : dirty ? `Draft has ${changes.length} change${changes.length === 1 ? "" : "s"}` : "Draft matches the live policy"}</span>
      </span>
      <span className="h-4 w-px bg-hairline" aria-hidden />
      <span className="flex items-center gap-2">
        <StatusDot tone={line.tone} />
        <span title={liveError ?? undefined}>
          {line.text}
          {/* A later check failed: the value shown is from the check before. */}
          {live && liveError && <span className="text-danger"> (earlier check; checking again failed: {liveError})</span>}
        </span>
      </span>
      {live?.unconfigured.map((h) => (
        <button key={h} onClick={() => go("home")} title="The policy sends devices to this relay, but Tether Manage does not publish to it. Add it on Status and relays.">
          <Badge tone="warn">{h} not in the relay list</Badge>
        </button>
      ))}
      <span className="ml-auto" />
      {behind && (
        <Button size="sm" variant="secondary" disabled={busy} onClick={sync}>
          {busy ? "Updating…" : `Bring ${of - on} relay${of - on === 1 ? "" : "s"} up to date`}
        </Button>
      )}
      <IconButton label="Check the live policy again" onClick={reloadLive} className="h-8 w-8">
        <RefreshCw size={14} />
      </IconButton>
      <Button size="sm" variant="ghost" onClick={() => go("policy")}>
        Review
      </Button>
      <Button size="sm" variant="primary" disabled={!dirty || !!invalid} onClick={publish} title={invalid ?? undefined}>
        Publish…
      </Button>
      {synced && (
        <Modal
          onClose={() => setSynced(null)}
          title={<h2 className="text-headline font-semibold">Relays brought up to date</h2>}
          footer={
            <div className="flex justify-end">
              <Button variant="primary" onClick={() => setSynced(null)}>
                Done
              </Button>
            </div>
          }
        >
          <RelayResults results={synced.filter((r) => r.server)} />
          {synced.some((r) => !r.server) && <p className="text-caption text-danger">{synced.find((r) => !r.server)?.error}</p>}
        </Modal>
      )}
    </div>
  );
}

export function PolicyPublish({ onClose }: { onClose: () => void }) {
  const { draft, setDraft, changes, live, state, reloadLive, go } = useApp();
  const [serial, setSerial] = useState<number | null>(null);
  const missing = live?.unconfigured ?? [];

  const run = async (skip: string[]) => {
    const r = await api.publish(draft, skip);
    setSerial(r.serial);
    setDraft({ ...draft, serial: r.serial, issued_at: Math.floor(Date.now() / 1000) });
    reloadLive();
    return r.relays;
  };

  return (
    <PublishSheet
      title="Publish policy"
      action="Publish"
      run={run}
      onClose={onClose}
      after={() => serial != null && <Adoption serial={serial} />}
      warning={
        missing.length > 0 && (
          <div className="rounded-xl border border-warn/60 p-3 text-caption">
            <p className="font-semibold text-warn">
              {missing.length === 1 ? `${missing[0]} is a relay of this policy but not in Tether Manage's relay list.` : `${missing.join(", ")} are relays of this policy but not in Tether Manage's relay list.`}
            </p>
            <p className="mt-1 text-muted">
              This publishes only to {state.servers.map(host).join(", ")}. Devices that use the other relay keep the old policy. Add it first.
            </p>
            <Button
              size="sm"
              className="mt-2"
              onClick={() => {
                onClose();
                go("home");
              }}
            >
              Open Status and relays
            </Button>
          </div>
        )
      }
    >
      <p className="mb-2 text-caption text-muted">
        {changes.length} change{changes.length === 1 ? "" : "s"} compared with the live policy{live?.serial != null ? ` (serial ${live.serial})` : ""}:
      </p>
      <ChangeList changes={changes} />
    </PublishSheet>
  );
}

/** How many recently active devices already report the new serial; refreshes every 30 s for 10 min. */
function Adoption({ serial }: { serial: number }) {
  const { live } = useApp();
  const [devices, setDevices] = useState<Device[] | null>(null);
  const [at, setAt] = useState<Date | null>(null);
  useEffect(() => {
    const start = Date.now();
    const load = () =>
      api.devices().then(
        (r) => {
          setDevices(r.devices);
          setAt(new Date());
        },
        () => {},
      );
    load();
    const t = window.setInterval(() => (Date.now() - start > 10 * 60_000 ? clearInterval(t) : load()), 30_000);
    return () => clearInterval(t);
  }, [serial]);

  const eff = Object.fromEntries((live?.effective.technician ?? []).map((r) => [r.id, r.value]));
  const minutes = eff["policy.refresh-minutes"] || "15";
  const fast = eff["policy.fast-apply"] === "Y";
  const since = Math.min(midnight(), Math.floor(Date.now() / 1000) - 86400);
  const recent = (devices ?? []).filter((d) => d.last_seen >= since);
  const done = recent.filter((d) => d.serial >= serial).length;

  return (
    <div className="mt-5 rounded-2xl bg-parchment p-4">
      <p className="text-callout font-semibold">
        {devices ? `${done} of ${recent.length} devices active in the last 24 h run serial ${serial}` : "Reading devices…"}
      </p>
      {devices && recent.length > 0 && (
        <div className="mt-2 h-1.5 overflow-hidden rounded-full bg-hairline" aria-hidden>
          <div className="h-full origin-left rounded-full bg-accent transition-transform" style={{ transform: `scaleX(${done / recent.length})` }} />
        </div>
      )}
      <p className="mt-3 text-caption text-muted">
        {fast
          ? "Fast apply is on: devices pick the policy up within about a minute."
          : `Devices pick it up within ${minutes} minutes (policy.refresh-minutes). For a rollout you watch live, turn on Fast apply (policy.fast-apply, checks every minute), publish, and turn it off again afterwards.`}{" "}
        On one device, <code className="font-mono">tether --policy-refresh</code> applies it at once.
      </p>
      <p className="mt-2 text-footnote text-muted">Updates every 30 s for 10 minutes{at ? ` · last ${at.toLocaleTimeString()}` : ""}</p>
    </div>
  );
}

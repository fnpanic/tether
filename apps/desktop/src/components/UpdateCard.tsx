// Update notice. Optional: a card at the bottom right ("Later" hides it). Mandatory (below the
// minimum version, or the release says so): a blocking dialog, no "Later". Forced: a countdown of
// `update.restart-seconds`, then download, install and restart. The backend refuses sessions while
// an update is required; this only explains why.
import { useEffect, useState } from "react";
import { AlertCircle, Download, X } from "lucide-react";
import { inTauri, invoke } from "../lib/ipc";
import { errorText } from "../lib/policy";
import { Button, IconButton } from "./ui";

export interface UpdateStatus {
  current: string;
  version: string | null;
  notes: string;
  mandatory: boolean;
  forced: boolean;
  blocked: string | null;
  enabled: boolean;
  restartSeconds: number;
}

export const updateApi = {
  status: () => invoke<UpdateStatus | undefined>("update_status"),
  check: () => invoke<UpdateStatus>("update_check"),
  install: () => invoke("update_install"),
};

const clock = (s: number) => `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;

export function UpdateCard() {
  const [up, setUp] = useState<UpdateStatus | null>(null);
  const [dismissed, setDismissed] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [progress, setProgress] = useState<{ done: number; total: number | null } | null>(null);
  const [error, setError] = useState("");
  const [left, setLeft] = useState<number | null>(null);

  useEffect(() => {
    updateApi.status().then((s) => s && setUp(s)).catch(() => {});
    if (!inTauri) return;
    const subs = import("@tauri-apps/api/event").then(({ listen }) =>
      Promise.all([listen<UpdateStatus>("update", (e) => setUp(e.payload)), listen<{ done: number; total: number | null }>("update-progress", (e) => setProgress(e.payload))]),
    );
    return () => void subs.then((fs) => fs.forEach((f) => f()));
  }, []);

  const install = async () => {
    setError("");
    setInstalling(true);
    setProgress(null);
    try {
      await updateApi.install(); // restarts the app when done
    } catch (e) {
      setError(errorText(e));
    } finally {
      setInstalling(false);
    }
  };

  // Forced: count down, then install; after an error try again in a minute.
  const forced = !!up?.forced && !!up.version;
  useEffect(() => {
    if (!forced || !up) return;
    setLeft(Math.max(1, up.restartSeconds || 60));
    const t = setInterval(() => setLeft((l) => (l === null || l <= 0 ? l : l - 1)), 1000);
    return () => clearInterval(t);
  }, [forced, up?.version]);
  useEffect(() => {
    if (left !== 0 || installing) return;
    install().then(() => setLeft(60));
  }, [left]);

  if (!up || (!up.version && !up.blocked) || (!up.mandatory && dismissed)) return null;

  const pct = progress?.total ? Math.min(100, Math.round((progress.done / progress.total) * 100)) : null;
  const title = up.forced ? "Tether will update" : up.mandatory ? "Update required" : "Update available";
  const lead = !up.version
    ? `Your organisation requires a newer version than ${up.current}, but none is available yet. Contact your administrator.`
    : !up.enabled
      ? `Version ${up.version} is required. Your organisation installs updates for you; contact your administrator.`
      : up.mandatory
        ? `Install version ${up.version} to keep connecting. Sessions are paused until then.`
        : `Version ${up.version} is ready to install. You have ${up.current}.`;

  const body = (
    <>
      <div className="flex items-start gap-3">
        <span className={`mt-0.5 shrink-0 ${up.mandatory ? "text-warn" : "text-accent"}`}>{up.mandatory ? <AlertCircle size={20} /> : <Download size={20} />}</span>
        <div className="min-w-0 flex-1">
          <h2 className="text-callout font-semibold">{title}</h2>
          <p className="mt-1 text-caption text-muted">{lead}</p>
          {forced && !installing && left !== null && left > 0 && (
            <p className="mt-2 text-callout font-semibold tabular-nums" role="timer">
              Restarting in {clock(left)}
            </p>
          )}
        </div>
        {!up.mandatory && (
          <IconButton label="Later" onClick={() => setDismissed(true)}>
            <X size={16} />
          </IconButton>
        )}
      </div>
      {up.notes && <p className="mt-3 max-h-40 overflow-y-auto whitespace-pre-line rounded-xl bg-parchment px-3 py-2 text-caption">{up.notes}</p>}
      {installing && (
        <div className="mt-4" role="status">
          <div className="h-1.5 overflow-hidden rounded-full bg-parchment">
            <div className="h-full rounded-full bg-accent" style={{ width: pct === null ? "33%" : `${pct}%` }} />
          </div>
          <p className="mt-1.5 text-footnote text-muted">{pct === null ? "Downloading…" : `Downloading… ${pct}%`}</p>
        </div>
      )}
      {error && (
        <p className="mt-3 text-footnote text-danger" role="alert">
          {error}
        </p>
      )}
      <div className="mt-4 flex justify-end gap-2">
        {!up.mandatory && (
          <Button size="sm" variant="ghost" onClick={() => setDismissed(true)} disabled={installing}>
            Later
          </Button>
        )}
        {up.version && up.enabled ? (
          <Button size="sm" variant="primary" onClick={install} disabled={installing}>
            {error ? "Try again" : up.forced ? "Restart now" : "Install and restart"}
          </Button>
        ) : (
          <Button size="sm" onClick={() => updateApi.check().then(setUp).catch((e) => setError(errorText(e)))}>
            Check again
          </Button>
        )}
      </div>
    </>
  );

  return up.mandatory ? (
    <div className="animate-fade fixed inset-0 z-50 grid place-items-center bg-black/40" role="alertdialog" aria-modal aria-label={title}>
      <div className="animate-rise w-[420px] max-w-[92vw] rounded-2xl border border-hairline bg-tile p-8 shadow-float">{body}</div>
    </div>
  ) : (
    <div className="animate-rise fixed bottom-6 right-6 z-40 w-[360px] rounded-2xl border border-hairline bg-tile p-6 shadow-float" role="dialog" aria-label={title}>
      {body}
    </div>
  );
}

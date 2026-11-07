// Releases › "Clean up old releases…" (docs/release-cleanup.md §4.4): the relays' merged plan,
// one checkbox per version that every relay would drop, one confirmation, results per relay.
import { useEffect, useState } from "react";
import { ask } from "@tauri-apps/plugin-dialog";
import { bytes, host, invoke, type RelayResult } from "../lib/ipc";
import { keptOn, offeredKeys, reasons, selection, verKey, type PrunePlan } from "../lib/prune";
import { Button, ErrorText, Modal, TextField } from "./ui";
import { RelayResults } from "./publish";

const APP: Record<string, string> = { technician: "Technician app", customer: "Customer module" };
const OS: Record<string, string> = { "windows-x86_64": "Windows", "darwin-universal": "macOS" };
const groupName = (channel: string, dir: string) => {
  const [app, platform] = dir.split("/");
  return `${channel} › ${APP[app] ?? app} › ${OS[platform] ?? platform}`;
};

function done(r: RelayResult) {
  const d = r.detail ?? {};
  const parts = [`${d.deleted?.length ?? 0} versions deleted, ${bytes(d.freed_bytes ?? 0)} freed`];
  if (d.kept?.length) parts.push(`${d.kept.length} kept (published meanwhile)`);
  if (d.gone?.length) parts.push(`${d.gone.length} already gone`);
  if (d.failed?.length) parts.push(`${d.failed.length} failed: ${d.failed.map((f: any) => `${f.version} ${f.error}`).join("; ")}`);
  return parts.join(" · ");
}

export function CleanupSheet({ keepDefault, onClose }: { keepDefault: number; onClose: () => void }) {
  const [keep, setKeep] = useState(keepDefault);
  const [plan, setPlan] = useState<PrunePlan | null>(null);
  const [ticked, setTicked] = useState<Set<string>>(new Set());
  const [error, setError] = useState<string | null>(null);
  const [phase, setPhase] = useState<"review" | "busy" | "done">("review");
  const [results, setResults] = useState<RelayResult[]>([]);

  const load = (k = keep) => {
    setPlan(null);
    setError(null);
    invoke<PrunePlan>("release_prune_plan", { keep: k }).then((p) => {
      setPlan(p);
      setTicked(offeredKeys(p));
    }, (e) => setError(e.message));
  };
  useEffect(() => load(), []);

  const sel = plan ? selection(plan, ticked) : { delete: [], files: 0, bytes: 0 };
  const n = plan?.relays.length ?? 0;
  const setAll = (keys: string[], on: boolean) => setTicked((t) => {
    const next = new Set(t);
    keys.forEach((k) => (on ? next.add(k) : next.delete(k)));
    return next;
  });
  const all = plan ? [...offeredKeys(plan)] : [];

  const go = async () => {
    if (!(await ask(`Delete ${sel.delete.length} old version${sel.delete.length === 1 ? "" : "s"} (${sel.files} files, ${bytes(sel.bytes)}) from every relay? Each relay checks again and keeps anything published meanwhile.`, { title: "Clean up old releases", kind: "warning" }))) return;
    setPhase("busy");
    setError(null);
    try {
      setResults(await invoke<RelayResult[]>("release_prune", { keep, delete: sel.delete }));
      setPhase("done");
    } catch (e: any) {
      setError(e.message);
      setPhase("review");
    }
  };

  return (
    <Modal onClose={phase === "busy" ? undefined : onClose} width={760}>
      <h2 className="text-headline font-semibold">Clean up old releases</h2>
      {phase === "done" ? (
        <>
          <p className="mb-3 mt-1 text-callout text-muted">{results.every((r) => r.ok) ? "Cleaned up on every relay." : "Not every relay finished. Run the clean-up again once it answers."}</p>
          <RelayResults results={results} ok={done} />
          <div className="mt-6 flex justify-end">
            <Button variant="primary" onClick={onClose}>
              Done
            </Button>
          </div>
        </>
      ) : (
        <>
          <div className="mt-3 flex flex-wrap items-center gap-3 text-caption">
            Keep
            <TextField
              className="w-16"
              type="number"
              min={0}
              max={20}
              value={keep}
              aria-label="Older versions kept"
              onChange={(e) => setKeep(Math.max(0, Math.min(20, Number(e.target.value) || 0)))}
              onBlur={() => load(keep)}
              onKeyDown={(e) => e.key === "Enter" && load(keep)}
            />
            older version(s) besides the published one
          </div>
          {plan && (
            <p className="mt-3 text-callout">
              {sel.delete.length ? `Frees ${bytes(sel.bytes)} on each of ${n} relay${n === 1 ? "" : "s"} (${sel.files} files, ${sel.delete.length} versions).` : "Nothing selected to delete."} Keeps {plan.kept_versions} versions.
            </p>
          )}
          {plan?.index_differs && (
            <p className="mt-2 text-caption text-warn">The relays hold different release indexes. A lagging relay protects its older version (safe, but it stays): publish to the missing relays first (What devices get).</p>
          )}
          <ErrorText error={error} />
          {!plan && !error && <p className="mt-4 text-caption text-muted">Asking every relay for its plan…</p>}
          {plan && (
            <>
              <div className="mt-3 flex gap-2">
                <Button size="sm" onClick={() => setAll(all, true)} disabled={!all.length}>
                  Select all
                </Button>
                <Button size="sm" onClick={() => setAll(all, false)} disabled={!all.length}>
                  Select none
                </Button>
              </div>
              <div className="mt-3 max-h-[45vh] overflow-auto rounded-xl border border-hairline">
                {plan.groups.map((g) => {
                  const keys = g.versions.filter((v) => v.offer).map((v) => verKey(g.channel, g.dir, v.version));
                  return (
                    <section key={g.channel + g.dir} className="border-b border-hairline px-3 py-2 last:border-0">
                      <div className="flex items-center justify-between gap-2">
                        <h3 className="text-caption font-semibold">{groupName(g.channel, g.dir)}</h3>
                        {keys.length > 0 && (
                          <span className="flex gap-1">
                            <Button size="sm" variant="ghost" onClick={() => setAll(keys, true)}>
                              All
                            </Button>
                            <Button size="sm" variant="ghost" onClick={() => setAll(keys, false)}>
                              None
                            </Button>
                          </span>
                        )}
                      </div>
                      <ul className="mt-1 flex flex-col">
                        {g.versions.map((v) => {
                          const key = verKey(g.channel, g.dir, v.version);
                          const others = keptOn(v);
                          return (
                            <li key={key} className={`flex items-center gap-3 py-1 text-caption ${v.offer ? "" : "text-muted"}`}>
                              <input
                                type="checkbox"
                                aria-label={`Delete ${v.version}`}
                                disabled={!v.offer}
                                checked={v.offer && ticked.has(key)}
                                onChange={(e) => setAll([key], e.target.checked)}
                              />
                              <span className="w-24 font-semibold">{v.version}</span>
                              <span className="w-16">{v.files.length} files</span>
                              <span className="w-20">{bytes(v.bytes)}</span>
                              <span className="min-w-0 flex-1 truncate" title={v.files.join("\n")}>
                                {v.offer ? "" : `Kept: ${reasons(v.why)}`}
                                {others.length > 0 && ` (on ${others.map(host).join(", ")}; the other relays would delete it)`}
                              </span>
                            </li>
                          );
                        })}
                      </ul>
                    </section>
                  );
                })}
                {!plan.groups.length && <p className="px-3 py-6 text-center text-caption text-muted">No releases on the relays.</p>}
              </div>
            </>
          )}
          <div className="mt-6 flex justify-end gap-2">
            <Button onClick={onClose} disabled={phase === "busy"}>
              Cancel
            </Button>
            <Button variant="danger" disabled={!sel.delete.length || phase === "busy"} onClick={go}>
              {phase === "busy" ? "Deleting…" : `Delete ${sel.delete.length} version${sel.delete.length === 1 ? "" : "s"}`}
            </Button>
          </div>
        </>
      )}
    </Modal>
  );
}

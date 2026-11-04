// Review-then-publish: one pattern for policy, releases, the download page and custom apps.
// 1. review what will change and check that every configured relay answers, 2. publish to all,
// 3. show the result per relay.
import { type ReactNode, useEffect, useState } from "react";
import { Check, X } from "lucide-react";
import { api, host, type Change, type RelayResult } from "../lib/ipc";
import { Badge, Button, ErrorText, Modal, StatusDot } from "./ui";

export function RelayResults({ results, ok = describe }: { results: RelayResult[]; ok?: (r: RelayResult) => string }) {
  return (
    <ul className="flex flex-col gap-1.5">
      {results.map((r) => (
        <li key={r.server} className="flex items-start gap-2 text-caption">
          {r.ok ? <Check size={16} className="mt-0.5 shrink-0 text-ok" aria-label="Done" /> : <X size={16} className="mt-0.5 shrink-0 text-danger" aria-label="Failed" />}
          <span className="font-semibold">{host(r.server)}</span>
          <span className={`min-w-0 flex-1 select-text ${r.ok ? "text-muted" : "text-danger"}`}>{r.ok ? ok(r) : r.error}</span>
        </li>
      ))}
    </ul>
  );
}

function describe(r: RelayResult) {
  if (r.detail?.already) return "Already up to date";
  if (r.detail?.published?.length) return r.detail.published.join(" · ");
  if (r.detail?.serial != null) return `Serial ${r.detail.serial} is live`;
  return "Done";
}

/** "Technician profile · Allow clipboard sync   off → on" */
export function ChangeList({ changes, empty = "No changes." }: { changes: Change[]; empty?: string }) {
  if (!changes.length) return <p className="text-caption text-muted">{empty}</p>;
  return (
    <ul className="flex flex-col divide-y divide-hairline rounded-xl border border-hairline">
      {changes.map((c, i) => (
        <li key={i} className="flex flex-wrap items-center gap-x-3 gap-y-1 px-3 py-2 text-caption">
          <Badge>{c.scope}</Badge>
          {c.label ? (
            <>
              <span className="min-w-0 flex-1">{c.label}</span>
              <span className="text-muted line-through">{c.from}</span>
              <span className="text-muted">→</span>
              <span className="font-semibold text-accent">{c.to}</span>
            </>
          ) : (
            <span className="min-w-0 flex-1">{c.text.replace(`${c.scope} `, "").replace(/^./, (x) => x.toUpperCase())}</span>
          )}
        </li>
      ))}
    </ul>
  );
}

/**
 * Review sheet. `run` publishes to every relay and returns the per-relay results; `after` adds
 * content below them (e.g. how many devices already applied a policy).
 */
export function PublishSheet({
  title,
  children,
  action,
  run,
  after,
  onClose,
  warning,
  progress,
}: {
  title: string;
  children?: ReactNode;
  action: string;
  run: () => Promise<RelayResult[]>;
  after?: (results: RelayResult[]) => ReactNode;
  onClose: () => void;
  /** Blocks nothing; shown above the targets. */
  warning?: ReactNode;
  /** Shown while publishing (e.g. upload progress); mounted, hidden, while reviewing. */
  progress?: ReactNode;
}) {
  const [check, setCheck] = useState<RelayResult[] | null>(null);
  const [anyway, setAnyway] = useState(false);
  const [phase, setPhase] = useState<"review" | "busy" | "done">("review");
  const [results, setResults] = useState<RelayResult[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.relays().then(setCheck, (e) => setError(e.message));
  }, []);

  const down = check?.filter((r) => !r.ok) ?? [];
  const go = async () => {
    setPhase("busy");
    setError(null);
    try {
      setResults(await run());
      setPhase("done");
    } catch (e: any) {
      setError(e.message);
      setPhase("review");
    }
  };

  return (
    <Modal onClose={phase === "busy" ? undefined : onClose} width={640}>
      <h2 className="text-headline font-semibold">{title}</h2>
      {phase !== "done" ? (
        <>
          <div className="mt-4 max-h-[40vh] overflow-auto">{children}</div>
          {warning && <div className="mt-4">{warning}</div>}
          <h3 className="mb-2 mt-5 text-caption font-semibold">Publishes to every relay</h3>
          {!check ? (
            <p className="text-caption text-muted">Checking the relays…</p>
          ) : (
            <ul className="flex flex-col gap-1.5">
              {check.map((r) => (
                <li key={r.server} className="flex items-center gap-2 text-caption">
                  <StatusDot tone={r.ok ? "ok" : "danger"} />
                  <span className="font-semibold">{host(r.server)}</span>
                  <span className={`min-w-0 flex-1 truncate ${r.ok ? "text-muted" : "text-danger"}`} title={r.error}>
                    {r.ok ? `Reachable · API ${r.detail?.version ?? "?"} · policy serial ${r.detail?.policy_serial ?? "none"}` : r.error}
                  </span>
                </li>
              ))}
            </ul>
          )}
          {down.length > 0 && (
            <label className="mt-3 flex items-start gap-2 text-caption">
              <input type="checkbox" className="mt-0.5" checked={anyway} onChange={(e) => setAnyway(e.target.checked)} />
              <span>
                {down.length === 1 ? "One relay does not answer." : `${down.length} relays do not answer.`} Publish to the others anyway; bring it up to date later (the header shows relays that are behind).
              </span>
            </label>
          )}
          {/* Mounted from the start so it already listens when the first event comes. */}
          {progress && <div hidden={phase !== "busy"}>{progress}</div>}
          <div className="mt-3">
            <ErrorText error={error} />
          </div>
          <div className="mt-6 flex justify-end gap-2">
            <Button onClick={onClose} disabled={phase === "busy"}>
              Cancel
            </Button>
            <Button variant="primary" disabled={!check || phase === "busy" || (down.length > 0 && !anyway)} onClick={go}>
              {phase === "busy" ? "Publishing…" : `${action} · ${check ? check.length - (anyway ? down.length : 0) : "…"} relay${check?.length === 1 ? "" : "s"}`}
            </Button>
          </div>
        </>
      ) : (
        <>
          <p className="mb-3 mt-1 text-callout text-muted">
            {results.every((r) => r.ok) ? "Published on every relay." : "Not every relay has it. Fix the relay and publish again, or bring it up to date from the header."}
          </p>
          <RelayResults results={results} />
          {after?.(results)}
          <div className="mt-6 flex justify-end">
            <Button variant="primary" onClick={onClose}>
              Done
            </Button>
          </div>
        </>
      )}
    </Modal>
  );
}

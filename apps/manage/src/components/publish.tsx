// Review-then-publish: one pattern for policy, releases, the download page and custom apps.
// 1. review what will change and check that every configured relay answers, 2. publish to all,
// 3. show the result per relay.
import { type ReactNode, useEffect, useState } from "react";
import { Check, X } from "lucide-react";
import { api, host, invoke, target, type Change, type RelayResult } from "../lib/ipc";
import { Badge, Button, ErrorText, Modal, StatusDot } from "./ui";

export function RelayResults({ results, ok = describe }: { results: RelayResult[]; ok?: (r: RelayResult) => string }) {
  return (
    <ul className="flex flex-col gap-1.5">
      {results.map((r) => (
        <li key={r.server} className="flex items-start gap-2 text-caption">
          {r.ok ? <Check size={16} className="mt-0.5 shrink-0 text-ok" aria-label="Done" /> : <X size={16} className="mt-0.5 shrink-0 text-danger" aria-label="Failed" />}
          <span className="font-semibold">{target(r.server)}</span>
          <span className="min-w-0 flex-1 select-text">
            {/* A failed row still names what went live there (several builds). */}
            {(r.ok || !!r.detail?.published?.length) && <span className="block text-muted">{r.ok ? ok(r) : `Live: ${r.detail.published.join(" · ")}`}</span>}
            {!r.ok && <span className="block whitespace-pre-line text-danger">{r.error}</span>}
            {r.detail?.warning && <span className="block whitespace-pre-line text-warn">{r.detail.warning}</span>}
          </span>
        </li>
      ))}
    </ul>
  );
}

function describe(r: RelayResult) {
  if (r.detail?.base) return `Update files live on ${host(r.detail.base)}`;
  if (r.detail?.already) return "Already up to date";
  if (r.detail?.published?.length) return r.detail.published.join(" · ");
  if (r.detail?.serial != null) return `Serial ${r.detail.serial} is live`;
  // A release: files, then the signed index (live), then the legacy latest.json.
  if (r.detail?.live) return r.detail.url ? "Live (release index) · latest.json updated" : "Live (release index)";
  if (r.detail?.url) return "Published (latest.json updated)";
  return "Done";
}

/** The result sheet's summary line for `results` (rows "Cancel" = a too-late Cancel). */
export function resultSummary(results: RelayResult[]) {
  if (results.some((r) => r.server === "Cancel")) return "Cancel came too late: the release index was already being written, so it is live where marked done.";
  if (!results.every((r) => r.ok)) return "Not every relay has it. Fix the relay and publish again, or bring it up to date from the header.";
  if (results.some((r) => r.detail?.warning)) return "Live on every relay. Some need attention: see the warnings below.";
  return "Published on every relay.";
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
 * Review sheet. `run` publishes to every relay but those in `skip` (the ones that did not answer
 * the check, when the admin chose to publish to the others anyway) and returns the per-relay
 * results; `after` adds content below them (e.g. how many devices already applied a policy).
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
  cdn: withCdn,
  blocked,
  cancel,
  summary = resultSummary,
}: {
  title: string;
  children?: ReactNode;
  action: string;
  run: (skip: string[]) => Promise<RelayResult[]>;
  after?: (results: RelayResult[]) => ReactNode;
  onClose: () => void;
  /** Blocks nothing; shown above the targets. */
  warning?: ReactNode;
  /** Shown while publishing (e.g. upload progress); mounted, hidden, while reviewing. */
  progress?: ReactNode;
  /** Release uploads: also list the Cloudflare CDN when it is switched on. */
  cdn?: boolean;
  /** The review content is not publishable yet (it shows why). */
  blocked?: boolean;
  /** Stops the publish in progress (release uploads); without it the sheet cannot be left while busy. */
  cancel?: () => void;
  /** The result screen's summary line (default [resultSummary]). */
  summary?: (results: RelayResult[]) => string;
}) {
  const [check, setCheck] = useState<RelayResult[] | null>(null);
  const [anyway, setAnyway] = useState(false);
  const [phase, setPhase] = useState<"review" | "busy" | "done">("review");
  const [results, setResults] = useState<RelayResult[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [cancelling, setCancelling] = useState(false);

  const [cdn, setCdn] = useState<{ enabled: boolean; base: string } | null>(null);
  useEffect(() => {
    api.relays().then(setCheck, (e) => setError(e.message));
    if (withCdn) invoke<{ enabled: boolean; base: string }>("cdn_state").then(setCdn, () => {});
  }, [withCdn]);
  const toCdn = !!cdn?.enabled;

  const down = check?.filter((r) => !r.ok) ?? [];
  const targets = check ? check.length - (anyway ? down.length : 0) : 0;
  const stop = () => {
    setCancelling(true);
    cancel?.();
  };
  const go = async () => {
    setPhase("busy");
    setError(null);
    setCancelling(false);
    try {
      setResults(await run(anyway ? down.map((r) => r.server) : []));
      setPhase("done");
    } catch (e: any) {
      setError(e.message);
      setPhase("review");
    }
  };

  const footer =
    phase !== "done" ? (
      <>
        {/* Mounted from the start so it already listens when the first event comes; in the footer
            so it stays in view. */}
        {progress && <div hidden={phase !== "busy"}>{progress}</div>}
        <ErrorText error={error} />
        <div className="mt-2 flex justify-end gap-2">
          <Button onClick={phase === "busy" ? stop : onClose} disabled={phase === "busy" && (!cancel || cancelling)}>
            {cancelling ? "Cancelling…" : "Cancel"}
          </Button>
          <Button variant="primary" disabled={!check || blocked || phase === "busy" || (down.length > 0 && !anyway) || (!!check && !targets)} onClick={go}>
            {phase === "busy" ? "Publishing…" : `${action} · ${check ? targets : "…"} relay${targets === 1 ? "" : "s"}${toCdn ? " + CDN" : ""}`}
          </Button>
        </div>
      </>
    ) : (
      <div className="flex justify-end">
        <Button variant="primary" onClick={onClose}>
          Done
        </Button>
      </div>
    );

  return (
    <Modal onClose={phase !== "busy" ? onClose : cancel && !cancelling ? stop : undefined} width={640} title={<h2 className="text-headline font-semibold">{title}</h2>} footer={footer}>
      {phase !== "done" ? (
        <>
          <div>{children}</div>
          {warning && <div className="mt-4">{warning}</div>}
          <h3 className="mb-2 mt-5 text-caption font-semibold">{toCdn ? "Publishes to every relay and the Cloudflare CDN" : "Publishes to every relay"}</h3>
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
              {toCdn && (
                <li className="flex items-center gap-2 text-caption">
                  <StatusDot tone="ok" />
                  <span className="font-semibold">Cloudflare CDN</span>
                  <span className="min-w-0 flex-1 truncate text-muted">On · {host(cdn!.base)} · update files, devices fall back to the relays</span>
                </li>
              )}
            </ul>
          )}
          {down.length > 0 && (
            <label className="mt-3 flex items-start gap-2 text-caption">
              <input type="checkbox" className="mt-0.5" checked={anyway} onChange={(e) => setAnyway(e.target.checked)} />
              <span>
                {down.length === 1 ? "One relay does not answer." : `${down.length} relays do not answer.`} Publish to the others anyway: {down.length === 1 ? "it is" : "they are"} left out and listed as skipped; bring {down.length === 1 ? "it" : "them"} up to date later (the header shows relays that are behind).
              </span>
            </label>
          )}
        </>
      ) : (
        <>
          <p className="mb-3 text-callout text-muted">{summary(results)}</p>
          <RelayResults results={results} />
          {after?.(results)}
        </>
      )}
    </Modal>
  );
}

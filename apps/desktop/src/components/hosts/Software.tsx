// The software list (SBOM) of a host, a part of its Health section (docs/health.md, "Software"). Everything
// from the document is plain text, cut by the reader (lib/hosts/sbom.ts).
import { useEffect, useMemo, useRef, useState } from "react";
import { Download } from "lucide-react";
import { Button } from "../ui";
import { invoke } from "../../lib/ipc";
import { errorText } from "../../lib/policy";
import { clip } from "../../lib/hosts/health";
import type { Host } from "../../lib/hosts/model";
import { exportName, filterPrograms, MAX_PROGRAMS, parse, sbomPlan, visibleRows } from "../../lib/hosts/sbom";
import { saveFile } from "./Dialogs";
import { day, Fold } from "./bits";

/** What a load for `sha` came back with: the text, or a line to show. */
type Load = { sha: string; raw: string } | { sha: string; note: string };

/** Asked for from the relay with the newest list when opened, and again when the summary says it changed
 *  (`sbom.sha`). A loaded document belongs to its `sha`: when the summary moves on, it is dropped, so neither the
 *  count nor Export can belong to an older list than the header's date. Export saves the text as the relay sent it. */
export function SoftwarePart({ h }: { h: Host }) {
  const s = h.status!;
  const sb = s.sbom!;
  const relay = s.sbomRelay ?? s.relay;
  const [open, setOpen] = useState(false);
  const [load, setLoad] = useState<Load | null>(null);
  const [filter, setFilter] = useState("");
  const [all, setAll] = useState(false);
  const [saved, setSaved] = useState("");
  const have = useRef<string | null>(null);
  const cur = load?.sha === sb.sha ? load : null;
  const raw = cur && "raw" in cur ? cur.raw : null;
  have.current = raw === null ? null : sb.sha;
  useEffect(() => {
    if (!sbomPlan(open, sb.sha, have.current).load) return;
    let live = true;
    const sha = sb.sha;
    setLoad(null);
    setSaved("");
    invoke<string>("team_status_sbom", { relay, uuid: s.uuid }).then(
      (text) => live && setLoad({ sha, raw: text }),
      (e) => {
        const m = errorText(e);
        if (live) setLoad({ sha, note: m.startsWith("none: ") ? "No software list on the server." : m.startsWith("too-large: ") ? "The software list is too large to show." : clip(m.replace(/^[a-z-]+: /, ""), 300) });
      },
    );
    return () => {
      live = false;
    };
  }, [open, relay, s.uuid, sb.sha]);
  const doc = useMemo(() => (raw === null ? null : parse(raw)), [raw]);
  const rows = useMemo(() => (doc ? filterPrograms(doc.programs, filter) : []), [doc, filter]);
  const shown = visibleRows(rows, all);
  const d = doc?.device;
  const gb = d?.memory ? Math.max(1, Math.round(d.memory / 2 ** 30)) : 0;
  const device = d ? [[d.manufacturer, d.model].filter(Boolean).join(" "), d.serial && `serial ${d.serial}`, gb && `${gb} GB`].filter(Boolean).join(", ") : "";
  const n = doc?.programs.length ?? 0;
  const save = () => {
    setSaved("");
    saveFile(exportName(s.host, new Date().toISOString().slice(0, 10)), raw!, "application/vnd.cyclonedx+json").then(
      (ok) => ok && setSaved("Exported."),
      (e) => setSaved(clip(errorText(e), 200)),
    );
  };
  return (
    <section aria-label="Software" className="mt-3 border-t border-hairline pt-2" data-testid="software-section">
      <div className="flex items-center gap-2">
        <h3 className="min-w-0 flex-1">
          <Fold label="Software" open={open} onToggle={() => setOpen((o) => !o)} extra={<span className="shrink-0 pl-2">{doc?.readable ? `${n} ${n === 1 ? "program" : "programs"}, as of ${day(sb.at)}` : `as of ${day(sb.at)}`}</span>} />
        </h3>
        {raw !== null && (
          <Button size="sm" variant="secondary" className="shrink-0" onClick={save}>
            <Download size={14} /> Export SBOM
          </Button>
        )}
      </div>
      {open && (
        <div className="pb-2">
          {cur === null && <div className="text-footnote text-muted">Loading software list…</div>}
          {cur && "note" in cur && <div className="text-footnote text-warn">{cur.note}</div>}
          {doc && !doc.readable && <div className="text-footnote text-warn">The software list could not be read. It can still be exported.</div>}
          {saved && (
            <div role="status" className="text-footnote text-muted">
              {saved}
            </div>
          )}
          {doc?.readable && (
            <>
              <input
                value={filter}
                onChange={(e) => setFilter(e.target.value.slice(0, 64))}
                onKeyDown={(e) => e.stopPropagation()}
                placeholder="Filter by name or publisher"
                aria-label="Filter software by name or publisher"
                spellCheck={false}
                className="mt-1 h-8 w-full rounded-xl border border-hairline bg-parchment px-3 text-footnote text-ink outline-none placeholder:text-faint focus-visible:border-focus"
              />
              {rows.length === 0 && <div className="mt-1 text-footnote text-muted">{filter.trim() ? "No program matches." : "The list is empty."}</div>}
              {rows.length > 0 && (
                <div role="region" aria-label="Programs" tabIndex={0} className="mt-1 max-h-[320px] overflow-auto rounded-xl border border-hairline">
                  <table className="w-full table-fixed text-left text-footnote">
                    <caption className="sr-only">Installed programs</caption>
                    <thead className="sticky top-0 bg-parchment text-muted">
                      <tr>
                        <th scope="col" className="w-[44%] px-2 py-1 font-semibold">
                          Name
                        </th>
                        <th scope="col" className="w-[20%] px-2 py-1 font-semibold">
                          Version
                        </th>
                        <th scope="col" className="w-[36%] px-2 py-1 font-semibold">
                          Publisher
                        </th>
                      </tr>
                    </thead>
                    <tbody>
                      {shown.map((p, i) => (
                        <tr key={i} className="border-t border-hairline align-top">
                          <td className="break-words px-2 py-1 text-ink">
                            {p.name}
                            {p.product && <span className="ml-1 rounded bg-parchment px-1 text-muted">product{p.role ? `: ${p.role}` : ""}</span>}
                          </td>
                          <td className="break-words px-2 py-1 text-ink-80">{p.version}</td>
                          <td className="break-words px-2 py-1 text-ink-80">{p.publisher}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
              {rows.length > shown.length && (
                <Button size="sm" variant="ghost" className="mt-1" onClick={() => setAll(true)}>
                  Show all {rows.length}
                </Button>
              )}
              {doc.capped && <div className="mt-1 text-footnote text-muted">Only the first {MAX_PROGRAMS} programs are shown.</div>}
              {doc.truncated && (
                <div className="mt-1 text-footnote text-muted">
                  {doc.omitted} more {doc.omitted === 1 ? "program is" : "programs are"} not in the list
                </div>
              )}
              {device && (
                <div className="mt-1 break-words text-footnote text-ink-80">
                  <span className="font-semibold text-muted">Device</span> {device}
                </div>
              )}
            </>
          )}
        </div>
      )}
    </section>
  );
}

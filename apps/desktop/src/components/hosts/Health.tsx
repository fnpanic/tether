// Health section of a host (spec 10.2/10.3): faults by group, setup items, passing checks folded,
// then History, All checks and the raw report. Everything from the device is plain text, clipped.
import { useEffect, useMemo, useState } from "react";
import { ChevronDown, ChevronRight, Copy, Link2, RotateCcw, Trash2, Unlink } from "lucide-react";
import { Button, IconButton, Modal } from "../ui";
import { invoke } from "../../lib/ipc";
import { errorText } from "../../lib/policy";
import { checksOf, clip, groupChecks, healthText, linkState, pick } from "../../lib/hosts/health";
import { label, type CheckState, type Host, type ReportCheck, type StatusHostReply, type StatusSummary } from "../../lib/hosts/model";
import { actions, useHosts } from "../../lib/hosts/store";
import { ago, HealthDot } from "./bits";

type Lang = "de" | "en";
const ICON: Record<CheckState, "crit" | "warn" | "ok" | "setup"> = { crit: "crit", warn: "warn", ok: "ok", unknown: "setup" };
const WORD: Record<CheckState, string> = { crit: "Critical", warn: "Warning", ok: "OK", unknown: "Unknown" };

/** A device time (RFC 3339) as a short local date and time; anything else as clipped text. */
function when(t: string | undefined): string {
  if (!t) return "";
  const d = Date.parse(t);
  return Number.isNaN(d) ? clip(t, 40) : new Date(d).toLocaleString(undefined, { dateStyle: "short", timeStyle: "short" });
}

/** `team_status_host` for this host: on mount (the detail panel is mounted per host) and again when
 *  the summary says a newer report arrived, so an open panel never keeps an old one. */
export function useStatusHost(h: Host): StatusHostReply | string | null {
  const [r, setR] = useState<StatusHostReply | string | null>(null);
  const relay = h.status?.relay;
  const uuid = h.status?.uuid;
  const seen = h.status?.last_seen;
  useEffect(() => {
    if (!relay || !uuid) return;
    let live = true;
    // Arrow keys move through hosts quickly: only a host the cursor rests on is asked for.
    const t = setTimeout(
      () =>
        invoke<StatusHostReply>("team_status_host", { relay, uuid }).then(
          (x) => live && setR(x ?? "No report."),
          (e) => live && setR(errorText(e).replace(/^[a-z-]+: /, "")),
        ),
      250,
    );
    return () => {
      live = false;
      clearTimeout(t);
    };
  }, [relay, uuid, seen]);
  return r;
}

function Fold({ label, open, onToggle, extra }: { label: string; open: boolean; onToggle: () => void; extra?: React.ReactNode }) {
  return (
    <button type="button" aria-expanded={open} onClick={onToggle} className="flex h-7 w-full items-center gap-1 rounded-md text-left text-footnote text-muted hover:text-ink">
      {open ? <ChevronDown size={12} aria-hidden /> : <ChevronRight size={12} aria-hidden />}
      <span className="min-w-0 flex-1 truncate">{label}</span>
      {extra}
    </button>
  );
}

function Check({ c, lang, hint = true }: { c: ReportCheck; lang: Lang; hint?: boolean }) {
  const title = clip(pick(c.title, lang), 120) || clip(c.id, 64);
  const value = clip(typeof c.value === "string" ? c.value : "", 80);
  const since = c.state !== "ok" ? when(c.since) : "";
  const h = hint ? clip(pick(c.hint, lang), 600) : "";
  return (
    <li className="flex gap-2 py-1" data-check={c.id}>
      <span className="pt-0.5">
        <HealthDot s={ICON[c.state]} label={WORD[c.state]} />
      </span>
      <div className="min-w-0 flex-1">
        <div className="break-words text-caption text-ink">{title}</div>
        {(value || since) && (
          <div className="break-words text-footnote text-muted">
            {value}
            {value && since ? " · " : ""}
            {since && `since ${since}`}
          </div>
        )}
        {h && <div className="mt-0.5 whitespace-pre-line break-words text-footnote text-ink-80">{h}</div>}
      </div>
    </li>
  );
}

const day = (t: number | null | undefined) => (t ? new Date(t * 1000).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" }) : "");

type Ask = "link" | "unlink" | "remove";

/** Who this agent is linked to, and the team actions on it: confirm a claim (link), unlink, remove.
 *  Nothing links by itself: a member with write decides, after one explicit confirmation. */
function LinkPart({ h, rep, nameOf, holderOf, ask, setAsk }: { h: Host; rep: StatusHostReply | null; nameOf?: (id: string) => string | undefined; holderOf?: (id: string) => StatusSummary | "conflict" | undefined; ask: Ask | null; setAsk: (a: Ask | null) => void }) {
  const write = useHosts((s) => !!s.team.me?.write);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const claimed = h.agentOnly ? h.status?.claimed_id : null;
  const st = linkState(h, rep, write, claimed ? holderOf?.(claimed) : undefined);
  const s = h.status!;
  const agentHost = clip(s.host, 64);
  const claimName = st.claim ? clip(nameOf?.(st.claim), 64) : "";
  // A device names itself: a host name other than the claimed host's is worth a second look.
  const differs = !!claimName && claimName.toLowerCase() !== agentHost.toLowerCase();
  const close = () => (setAsk(null), setErr(""));
  const run = (cmd: string, args: Record<string, unknown>) => {
    setBusy(true);
    setErr("");
    invoke(cmd, args).then(
      () => (setBusy(false), close(), void actions.teamRefresh()),
      (e) => (setBusy(false), setErr(errorText(e))),
    );
  };
  const what: Record<Ask, { title: string; text: string; warn?: string; button: string; go: () => void }> = {
    link: {
      title: `Link this agent to ${clip(st.claim ?? "", 32)}?`,
      text: `Health reports from ${agentHost} (enrolled ${day(rep?.enrolled) || "at an unknown time"}) will show on ${claimName || clip(st.claim ?? "", 32)}. The agent's claim is not proof: confirm only if this is that computer.`,
      warn: differs ? `The agent calls its computer “${agentHost}”, but this host is “${claimName}”. Check that they are the same computer.` : undefined,
      button: "Link",
      go: () => run("team_status_link", { uuid: s.uuid, tetherId: st.claim }),
    },
    unlink: {
      title: `Unlink health from ${clip(label(h), 64)}?`,
      text: "The link and the agent's claim are removed. The agent keeps reporting, as a host of its own, and cannot claim a Tether ID again until a team member allows a new claim.",
      button: "Unlink",
      go: () => run("team_status_unlink", { uuid: s.uuid }),
    },
    remove: {
      title: `Remove ${clip(label(h), 64)} from health monitoring?`,
      text: "Its enrolment, link and stored reports are deleted on every server. If the agent on that computer still holds a valid enrolment token it enrols again by itself, and comes back as an agent that is not linked to any host. Uninstall the agent there, or ask an admin to revoke the token.",
      button: "Remove",
      go: () => run("team_status_remove", { uuid: s.uuid }),
    },
  };
  return (
    <>
      {st.claim && (
        <div data-testid="health-claim" className="mt-2 rounded-xl bg-parchment px-3 py-2 text-footnote">
          <div className="break-words text-ink">
            Claims to be <span className="font-mono">{clip(st.claim, 32)}</span>
            {claimName && ` (${claimName})`}. Confirm only if this is that computer.
          </div>
          <div className="mt-0.5 break-words text-muted">
            Agent's host name {agentHost}
            {rep?.enrolled ? ` · enrolled ${day(rep.enrolled)}` : ""}
          </div>
          <Button size="sm" variant="secondary" className="mt-2" disabled={st.link !== "on"} onClick={() => setAsk("link")}>
            <Link2 size={14} /> Link to this host
          </Button>
          {st.link === "disabled" && <div className="mt-1 text-muted">Only team members who may edit shared tags can link an agent.</div>}
          {st.blocked && "conflict" in st.blocked && (
            <div className="mt-1 text-muted">This host is in a link conflict: two agents are linked to it on different relays. Resolve that first (unlink or remove the one that is not this computer).</div>
          )}
          {st.blocked && !("conflict" in st.blocked) && (
            <div className="mt-1 text-muted">
              This host already has a linked health agent ({clip(st.blocked.host, 64)}
              {st.blocked.at ? `, linked ${day(st.blocked.at)}` : ""}). Unlink that one first if this is its replacement.
            </div>
          )}
        </div>
      )}
      {st.conflict && st.linked && (
        <div data-testid="health-conflict" role="alert" className="mt-2 rounded-xl bg-parchment px-3 py-2 text-footnote text-danger">
          Link conflict: this agent and another device are both linked to {clip(h.status?.tether_id ?? "", 32)} on different relays, so neither is shown on that host. Unlink or remove the one that is not that computer.
        </div>
      )}
      {st.dismissed && (
        <div data-testid="health-dismissed" className="mt-2 rounded-xl bg-parchment px-3 py-2 text-footnote text-muted">
          <div>Link dismissed: this agent claims no host until a team member allows it again.</div>
          {st.allow && (
            <Button size="sm" variant="secondary" className="mt-2" onClick={() => run("team_status_allow_claim", { uuid: s.uuid })}>
              <RotateCcw size={14} /> Allow a new claim
            </Button>
          )}
        </div>
      )}
      {!ask && err && (
        <p role="alert" className="mt-1 break-words text-footnote text-danger">
          {clip(err, 300)}
        </p>
      )}
      {st.linked && rep && (
        <div className="mt-2 flex items-center justify-between gap-2 text-footnote text-muted">
          <span className="min-w-0 break-words">
            {st.conflict
              ? `Linked to ${clip(h.status?.tether_id ?? "", 32)} on its relay${st.linked.at ? `, ${day(st.linked.at)}` : ""}`
              : st.linked.by
                ? `Health linked by ${clip(st.linked.by, 64)}, ${day(st.linked.at)}`
                : `Health linked by the enrolment token issued for this host${st.linked.at ? `, ${day(st.linked.at)}` : ""}`}
          </span>
          {st.unlink && (
            <Button size="sm" variant="ghost" className="shrink-0" onClick={() => setAsk("unlink")}>
              <Unlink size={14} /> Unlink
            </Button>
          )}
        </div>
      )}
      {ask && (
        <Modal onClose={busy ? undefined : close}>
          <h2 className="text-headline font-semibold">{what[ask].title}</h2>
          <p className="mt-2 text-caption text-muted">{what[ask].text}</p>
          {what[ask].warn && (
            <p role="alert" className="mt-2 rounded-xl bg-parchment px-3 py-2 text-caption font-semibold text-warn">
              {what[ask].warn}
            </p>
          )}
          {err && (
            <p role="alert" className="mt-2 break-words text-caption text-danger">
              {clip(err, 300)}
            </p>
          )}
          <div className="mt-6 flex justify-end gap-2">
            <Button variant="pearl" disabled={busy} onClick={close}>
              Cancel
            </Button>
            <Button variant={ask === "link" ? "primary" : "danger"} data-autofocus={ask === "link" ? undefined : true} disabled={busy} onClick={what[ask].go}>
              {what[ask].button}
            </Button>
          </div>
        </Modal>
      )}
    </>
  );
}

export function HealthSection({ h, reply, lang, nameOf, holderOf }: { h: Host; reply: StatusHostReply | string | null; lang: Lang; nameOf?: (id: string) => string | undefined; holderOf?: (id: string) => StatusSummary | "conflict" | undefined }) {
  const [open, setOpen] = useState<Record<string, boolean>>({});
  const [filter, setFilter] = useState("");
  const [copied, setCopied] = useState(false);
  const [ask, setAsk] = useState<Ask | null>(null);
  const write = useHosts((st) => !!st.team.me?.write);
  const flip = (k: string) => setOpen((o) => ({ ...o, [k]: !o[k] }));
  const checks = useMemo(() => checksOf(reply), [reply]);
  const view = useMemo(() => groupChecks(checks), [checks]);
  const rep = typeof reply === "object" && reply ? reply : null;
  const s = h.status;
  const now = Date.now() / 1000;
  const raw = useMemo(() => (open.raw && rep?.report ? JSON.stringify(rep.report, null, 2) : ""), [open.raw, rep]);
  const history = useMemo(() => {
    const from = Date.now() / 1000 - 90 * 86400;
    const f = filter.trim().toLowerCase();
    const list = Array.isArray(rep?.history) ? rep.history : [];
    // Oldest first from the relay: shown newest first.
    return list
      .filter((x) => {
        const t = x && typeof x.id === "string" ? (x.at ?? Date.parse(x.time) / 1000) : -Infinity;
        return (Number.isNaN(t) || t >= from) && (!f || x.id.toLowerCase().includes(f));
      })
      .reverse();
  }, [rep, filter]);
  if (!s) return null;
  const titleOf = new Map(checks.map((c) => [c.id, clip(pick(c.title, lang), 120)]));

  return (
    <section aria-label="Health" className="mt-5" data-testid="health-section">
      <div className="flex items-baseline justify-between gap-2 text-footnote">
        <span className="font-semibold text-muted">Health</span>
        <span className="min-w-0 truncate text-right text-muted" title={new Date(s.last_seen * 1000).toLocaleString()}>
          reported {ago(s.last_seen, now)}, agent {clip(s.agent, 32)}
        </span>
      </div>
      {s.customer && <div className="mt-0.5 text-right text-footnote text-muted">Customer {clip(s.customer, 32)}</div>}
      <div className="mt-1 flex items-center gap-1.5 text-caption">
        <HealthDot s={h.health} />
        <span className={h.health === "ok" || h.health === "setup" ? "text-ink-80" : "text-ink"}>{healthText(h, now)}</span>
      </div>
      <LinkPart h={h} rep={rep} nameOf={nameOf} holderOf={holderOf} ask={ask} setAsk={setAsk} />

      {reply === null && <div className="mt-2 text-footnote text-muted">Loading report…</div>}
      {typeof reply === "string" && <div className="mt-2 text-footnote text-warn">{clip(reply, 300)}</div>}
      {rep && !rep.report && <div className="mt-2 text-footnote text-muted">No full report yet.</div>}
      {rep?.report && checks.length === 0 && <div className="mt-2 text-footnote text-muted">This report lists no checks.</div>}

      {view.faults.map((g) => (
        <div key={g.name} className="mt-3">
          <div className="truncate text-footnote font-semibold text-ink-80" title={g.name}>
            {clip(g.name, 80)}
          </div>
          <ul>
            {g.checks.map((c) => (
              <Check key={c.id} c={c} lang={lang} />
            ))}
          </ul>
        </div>
      ))}

      {view.setup.length > 0 && (
        <div className="mt-3">
          <div className="text-footnote font-semibold text-ink-80">Setup</div>
          <ul>
            {view.setup.map((c) => (
              <Check key={c.id} c={{ ...c, value: c.value || "not configured" }} lang={lang} />
            ))}
          </ul>
        </div>
      )}
      {view.unknown.length > 0 && (
        <div className="mt-2">
          <Fold label={`${view.unknown.length} unknown (not counted as a fault)`} open={!!open.unknown} onToggle={() => flip("unknown")} />
          {open.unknown && (
            <ul className="pl-4">
              {view.unknown.map((c) => (
                <Check key={c.id} c={c} lang={lang} />
              ))}
            </ul>
          )}
        </div>
      )}

      {view.passed.length > 0 && (
        <div className="mt-3">
          <div className="text-footnote font-semibold text-ink-80">Passed</div>
          {view.passed.map((g) => (
            <div key={g.name}>
              <Fold
                label={`${clip(g.name, 80)}: ${g.checks.length} ${g.checks.length === 1 ? "check" : "checks"} passed`}
                open={!!open[`g:${g.name}`]}
                onToggle={() => flip(`g:${g.name}`)}
                extra={<HealthDot s="ok" size={12} label="" />}
              />
              {open[`g:${g.name}`] && (
                <ul className="pl-4">
                  {g.checks.map((c) => (
                    <Check key={c.id} c={c} lang={lang} hint={false} />
                  ))}
                </ul>
              )}
            </div>
          ))}
        </div>
      )}

      {rep && (
        <div className="mt-3 border-t border-hairline pt-2">
          <Fold label="History (90 days)" open={!!open.history} onToggle={() => flip("history")} />
          {open.history && (
            <div className="pb-2">
              <input
                value={filter}
                onChange={(e) => setFilter(e.target.value.slice(0, 64))}
                onKeyDown={(e) => e.stopPropagation()}
                placeholder="Filter by check"
                aria-label="Filter history by check"
                spellCheck={false}
                className="mt-1 h-8 w-full rounded-xl border border-hairline bg-parchment px-3 font-mono text-footnote text-ink outline-none placeholder:font-sans placeholder:text-faint focus-visible:border-focus"
              />
              {history.length === 0 && <div className="mt-1 text-footnote text-muted">No changes{filter ? " for this check" : ""}.</div>}
              <ul aria-label="History" className="mt-1 max-h-[240px] space-y-1 overflow-y-auto">
                {history.slice(0, 500).map((x, i) => (
                  <li key={i} className="text-footnote">
                    <div className="flex justify-between gap-2">
                      <span className="min-w-0 truncate text-ink-80" title={x.id}>
                        {titleOf.get(x.id) || clip(x.id, 64)}
                      </span>
                      <span className="shrink-0 text-muted">{when(x.time)}</span>
                    </div>
                    <div className="text-muted">
                      {WORD[x.from] ?? clip(String(x.from), 16)} → {WORD[x.to] ?? clip(String(x.to), 16)}
                    </div>
                  </li>
                ))}
              </ul>
            </div>
          )}
          <Fold label={`All checks (${checks.length})`} open={!!open.all} onToggle={() => flip("all")} />
          {open.all && (
            <ul aria-label="All checks" className="pb-2">
              {checks.map((c) => (
                <Check key={c.id} c={c} lang={lang} hint={false} />
              ))}
            </ul>
          )}
          <Fold label="Raw report" open={!!open.raw} onToggle={() => flip("raw")} />
          {open.raw && (
            <div className="relative pb-2">
              {rep.report ? (
                <>
                  <pre aria-label="Raw report" tabIndex={0} className="max-h-[320px] overflow-auto rounded-xl border border-hairline bg-parchment p-3 font-mono text-[11px] leading-[1.45] text-ink-80">
                    {raw}
                  </pre>
                  <IconButton
                    label={copied ? "Raw report copied" : "Copy raw report"}
                    className="!absolute right-1.5 top-1.5 !h-7 !w-7 bg-parchment"
                    onClick={() =>
                      navigator.clipboard.writeText(raw).then(
                        () => (setCopied(true), setTimeout(() => setCopied(false), 1500)),
                        () => {},
                      )
                    }
                  >
                    <Copy size={13} />
                  </IconButton>
                </>
              ) : (
                <div className="text-footnote text-muted">No full report yet.</div>
              )}
            </div>
          )}
        </div>
      )}
      {linkState(h, rep, write).remove && (
        <Button size="sm" variant="danger" className="mt-3" onClick={() => setAsk("remove")}>
          <Trash2 size={14} /> Remove from health monitoring…
        </Button>
      )}
    </section>
  );
}

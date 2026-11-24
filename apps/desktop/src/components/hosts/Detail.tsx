// Right-hand panel for the host under the cursor: actions, name, tags, notes, facts, history.
import { useEffect, useRef, useState } from "react";
import { ArrowRight, Copy, Eye, FolderSync, History, Pin, PinOff, Trash2 } from "lucide-react";
import { Button, IconButton, PlatformIcon } from "../ui";
import { formatId, invoke } from "../../lib/ipc";
import { label, osFamily, type Host, type StatusSummary, type Store } from "../../lib/hosts/model";
import { actions, useHosts, useReadOnly } from "../../lib/hosts/store";
import { errorText } from "../../lib/policy";
import { uiLang } from "../../lib/hosts/health";
import { ago, OnlineDot, TagChip, TagInput } from "./bits";
import { HealthSection, useStatusHost } from "./Health";

interface AuditRow {
  ts: number;
  action: string;
  peer_id?: string;
  peer_name?: string;
  conn_type?: string;
  ip?: string;
}

const CONN: Record<string, string> = { default: "Remote control", "file-transfer": "File transfer", "view-only": "View only", remote: "Remote control", file_transfer: "File transfer", terminal: "Terminal", port_forward: "Port forward", view_camera: "Camera" };

export function Detail({
  h,
  meta,
  known,
  canConnect,
  onConnect,
  onToast,
  onRemove,
  nameOf,
  holderOf,
}: {
  h: Host;
  meta: Store["tags"];
  known: string[];
  canConnect: boolean;
  onConnect: (id: string, kind: "default" | "file-transfer" | "view-only") => void;
  onToast: (m: string) => void;
  onRemove: (ids: string[]) => void;
  /** A host row's name by its ID (the host an agent claims to be). */
  nameOf?: (id: string) => string | undefined;
  /** The health summary already joined to a host row (a claim to it cannot be linked). */
  holderOf?: (id: string) => StatusSummary | "conflict" | undefined;
}) {
  const entry = useHosts((s) => (Object.hasOwn(s.store.hosts, h.id) ? s.store.hosts[h.id] : undefined));
  const me = useHosts((s) => s.team.me);
  const teamOn = useHosts((s) => !!s.store.team?.enabled);
  const [name, setName] = useState(h.name);
  const [notes, setNotes] = useState(h.notes);
  const [audit, setAudit] = useState<AuditRow[] | string | null>(null);
  const ro = useReadOnly();
  const report = useStatusHost(h);
  // Mounted per host (key = id). What was typed and is not saved yet: saved on blur, and on unmount,
  // because clicking another row removes this panel before the field loses the focus.
  const edit = useRef<{ name?: string; notes?: string }>({});
  const save = () => {
    const e = edit.current;
    edit.current = {};
    if (e.name !== undefined) actions.setName(h.id, e.name);
    if (e.notes !== undefined) actions.setNotes(h.id, e.notes);
  };
  useEffect(() => save, []); // eslint-disable-line react-hooks/exhaustive-deps

  const loadAudit = () => {
    setAudit("Loading…");
    invoke<AuditRow[]>("team_audit", { device: h.id, since: Math.floor(Date.now() / 1000) - 90 * 86400 })
      .then((r) => setAudit(r ?? []))
      .catch((e) => setAudit(errorText(e)));
  };
  const why = canConnect ? undefined : "Your organisation does not allow connecting to other devices.";
  const history = entry?.history ?? [];

  return (
    <aside aria-label="Host details" className="flex w-[320px] shrink-0 flex-col overflow-y-auto *:shrink-0 border-l border-hairline bg-canvas px-5 py-5">
      <div className="flex items-start gap-3">
        <div className="grid h-11 w-11 shrink-0 place-items-center rounded-xl bg-parchment text-muted">
          <PlatformIcon platform={h.os} size={22} />
        </div>
        <div className="min-w-0 flex-1">
          <div className="truncate text-callout font-semibold" title={label(h)}>
            {label(h)}
          </div>
          <div className="flex items-center gap-1.5 font-mono text-footnote text-muted">
            <OnlineDot s={h.online} />
            {formatId(h.id)}
            <IconButton
              label="Copy ID"
              className="!h-6 !w-6"
              onClick={() => navigator.clipboard.writeText(h.id).then(() => onToast("ID copied"), () => {})}
            >
              <Copy size={12} />
            </IconButton>
          </div>
        </div>
        {!h.agentOnly && (
          <IconButton label={h.pinned ? "Unpin" : "Pin"} active={h.pinned} onClick={() => actions.setPinned([h.id], !h.pinned)}>
            {h.pinned ? <PinOff size={16} /> : <Pin size={16} />}
          </IconButton>
        )}
      </div>

      {h.agentOnly ? (
        <div className="mt-4 rounded-xl bg-parchment px-3 py-2 text-footnote text-muted">
          {h.conflict
            ? `In conflict with another agent for host ${nameOf?.(h.status?.tether_id ?? "") || h.status?.tether_id || ""}.`
            : h.status?.claimed_id
              ? "This agent is not linked to a host yet. Its health is reported by the monitor agent."
              : h.status?.no_claim || (typeof report === "object" && report?.no_claim)
                ? "Link dismissed. This agent is not linked to a host."
                : "No remote access: Tether is not installed on this machine. Its health is reported by the monitor agent."}
        </div>
      ) : (
        <>
          <div className="mt-4 flex gap-2" title={why}>
            <Button variant="primary" size="md" className="flex-1" disabled={!canConnect} onClick={() => onConnect(h.id, "default")}>
              Connect <ArrowRight size={15} />
            </Button>
            <Button variant="secondary" size="md" className="!px-3" disabled={!canConnect} onClick={() => onConnect(h.id, "file-transfer")} aria-label="Transfer files" title="Transfer files (Shift+Enter)">
              <FolderSync size={16} />
            </Button>
            <Button variant="secondary" size="md" className="!px-3" disabled={!canConnect} onClick={() => onConnect(h.id, "view-only")} aria-label="View only" title="View only (Alt+Enter)">
              <Eye size={16} />
            </Button>
          </div>
          <div className="mt-2 text-footnote text-muted">Enter connects · Shift+Enter files · Alt+Enter view only</div>
        </>
      )}

      {/* An agent-only row is not a host of the list: no name, tags, notes or pin (never stored or shared). */}
      {!h.agentOnly && (
        <>
      <label className="mt-6 flex items-center justify-between text-footnote font-semibold text-muted" htmlFor="host-name">
        Name {ro && <span className="font-normal">Managed by your team</span>}
      </label>
      <input
        id="host-name"
        value={name}
        placeholder={h.hostname || "Name this host"}
        disabled={ro}
        onChange={(e) => setName((edit.current.name = e.target.value.slice(0, 64)))}
        onBlur={save}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Enter" || e.key === "Escape") (e.target as HTMLInputElement).blur();
        }}
        className="mt-1 h-8 rounded-xl border border-hairline bg-parchment px-3 text-caption outline-none placeholder:text-faint focus-visible:border-focus disabled:text-muted"
      />

      <div className="mt-5 flex items-center justify-between text-footnote font-semibold text-muted">
        Tags {ro && <span className="font-normal">Managed by your team</span>}
      </div>
      <div className="mt-1.5 flex flex-wrap gap-1">
        {h.tags.map((t) => (
          <TagChip key={t} tag={t} meta={meta} onRemove={ro ? undefined : () => actions.removeTags([h.id], [t])} />
        ))}
        {h.auto.map((t) => (
          <TagChip key={t} tag={t} meta={meta} auto />
        ))}
      </div>
      {!ro && (
        <div className="mt-2">
          <TagInput inputId="detail-tag" known={known} onAdd={(t) => actions.addTags([h.id], [t])} placeholder="Add tag (site:berlin)" />
        </div>
      )}
        </>
      )}

      {h.conflict && !h.status && (
        <div data-testid="health-conflict-host" role="alert" className="mt-5 rounded-xl bg-parchment px-3 py-2 text-footnote text-danger">
          Health link conflict: two devices are linked to this host on different relays, so no health is shown here. Open the rows of the two agents (search <span className="font-mono">id:m:</span>) and unlink or remove the one that is not this computer.
        </div>
      )}
      {h.status && <HealthSection h={h} reply={report} lang={uiLang()} nameOf={nameOf} holderOf={holderOf} />}

      {!h.agentOnly && (
        <>
      <label className="mt-5 flex items-center justify-between text-footnote font-semibold text-muted" htmlFor="host-notes">
        Notes <span className="font-normal">Only on this PC</span>
      </label>
      <textarea
        id="host-notes"
        value={notes}
        rows={4}
        placeholder="Anything worth remembering. Do not store passwords here."
        onChange={(e) => setNotes((edit.current.notes = e.target.value.slice(0, 4000)))}
        onBlur={save}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Escape") (e.target as HTMLTextAreaElement).blur();
        }}
        className="mt-1 resize-none rounded-xl border border-hairline bg-parchment px-3 py-2 text-caption outline-none placeholder:text-faint focus-visible:border-focus"
      />
        </>
      )}

      <dl className="mt-5 grid grid-cols-[88px_1fr] gap-x-3 gap-y-1.5 text-footnote">
        {(
          [
            ["Hostname", h.hostname],
            ["User", h.user],
            ["OS", h.os && `${osFamily(h.os)} · ${h.os}`],
            ["Version", h.version],
            ["Last session", h.last ? `${ago(h.last)} · ${new Date(h.last * 1000).toLocaleString()}` : ""],
            ["Heartbeat", h.seen ? ago(h.seen) : ""],
            ["Source", [h.recent || h.last ? "Recent" : "", h.manual ? "Added by hand" : "", h.org ? `Organisation (${h.profile})` : ""].filter(Boolean).join(" · ")],
          ] as const
        )
          .filter(([, v]) => v)
          .map(([k, v]) => (
            <div key={k} className="contents">
              <dt className="text-muted">{k}</dt>
              <dd className="min-w-0 truncate text-ink-80" title={String(v)}>
                {v}
              </dd>
            </div>
          ))}
      </dl>

      <div className="mt-5 flex items-center gap-2 text-footnote font-semibold text-muted">
        <History size={13} /> Session history
      </div>
      {history.length === 0 && <div className="mt-1 text-footnote text-muted">No sessions from this app yet.</div>}
      <ul className="mt-1 space-y-0.5">
        {history.slice(0, 8).map((x) => (
          <li key={x.t + x.conn} className="flex justify-between text-footnote">
            <span className="text-ink-80">{CONN[x.conn] ?? x.conn}</span>
            <span className="text-muted">{ago(x.t)}</span>
          </li>
        ))}
      </ul>
      {teamOn && me?.audit && !h.agentOnly && (
        <div className="mt-3">
          {audit === null ? (
            <Button size="sm" variant="pearl" onClick={loadAudit}>
              Load organisation log
            </Button>
          ) : typeof audit === "string" ? (
            <div className="text-footnote text-muted">{audit}</div>
          ) : (
            <ul aria-label="Organisation log" className="space-y-0.5">
              {audit.length === 0 && <li className="text-footnote text-muted">Nothing in the last 90 days.</li>}
              {audit.slice(0, 20).map((r, i) => (
                <li key={i} className="flex justify-between gap-2 text-footnote">
                  <span className="truncate text-ink-80" title={r.ip}>
                    {r.action === "login" ? `${r.peer_name || r.peer_id || "Someone"} · ${CONN[r.conn_type ?? ""] ?? "session"}` : r.action === "disconnect" ? "Disconnected" : r.action}
                  </span>
                  <span className="shrink-0 text-muted">{ago(r.ts)}</span>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}

      {!h.agentOnly && (
        <div className="mt-auto pt-6">
          <Button size="sm" variant="danger" onClick={() => onRemove([h.id])}>
            <Trash2 size={14} /> Remove host
          </Button>
        </div>
      )}
    </aside>
  );
}

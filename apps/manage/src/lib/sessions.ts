/** One record of `GET /api/admin/audit` (docs/api.md "Session audit log"), tagged with its relay. */
export interface AuditEvent {
  ts: number;
  kind: "conn" | "file" | "alarm";
  /** conn: connect | login | disconnect; file: send | receive; alarm: alarm */
  action: string;
  /** Host: Tether ID and base64 machine uuid (not the heartbeat uuid). */
  id: string;
  uuid: string;
  conn_id: number;
  session_id?: string;
  /** Technician address as the host saw it (connect, file). */
  ip?: string;
  /** Address that posted the record (the host's public IP). */
  src?: string;
  peer_id?: string;
  peer_name?: string;
  conn_type?: string;
  primary_auth?: string;
  path?: string;
  is_file?: boolean;
  num?: number;
  files?: [string, number][];
  typ?: number;
  info?: string;
  nonce?: string;
  relay?: string;
}

/** An event plus what the other events of the same connection tell about it. */
export interface Row extends AuditEvent {
  /** Seconds from connect to this disconnect. */
  duration?: number;
}

/** Same record from several relays (or a client retry) once: by nonce, else device+conn_id+action+ts. */
export function dedupe(events: AuditEvent[]): AuditEvent[] {
  const seen = new Set<string>();
  return events.filter((e) => {
    const k = e.nonce || `${e.uuid}|${e.conn_id}|${e.action}|${e.ts}`;
    if (seen.has(k)) return false;
    seen.add(k);
    return true;
  });
}

/**
 * Newest first, with the technician and their IP filled in on every event of a connection (only
 * login and file records name the technician, connect and file records the IP) and the duration
 * on disconnects. A connection is one host uuid + conn_id
 * from a connect to the next connect (conn_id restarts with the host process).
 */
export function sessions(events: AuditEvent[]): Row[] {
  const rows: Row[] = dedupe(events).map((e) => ({ ...e }));
  const asc = [...rows].sort((a, b) => a.ts - b.ts || order(a) - order(b));
  const open = new Map<string, Row[]>();
  const finish = (seg: Row[]) => {
    const who = seg.find((r) => r.peer_id);
    const ip = seg.find((r) => r.ip)?.ip;
    const start = seg.find((r) => r.action === "connect" || r.action === "login");
    for (const r of seg) {
      if (who) {
        r.peer_id ??= who.peer_id;
        r.peer_name ??= who.peer_name;
      }
      r.ip ??= ip;
      if (r.action === "disconnect" && start) r.duration = r.ts - start.ts;
    }
  };
  for (const r of asc) {
    if (r.kind === "alarm") continue;
    const k = `${r.uuid}|${r.conn_id}`;
    if (r.action === "connect" && open.has(k)) {
      finish(open.get(k)!);
      open.delete(k);
    }
    const seg = open.get(k) ?? [];
    seg.push(r);
    open.set(k, seg);
    if (r.action === "disconnect") {
      finish(seg);
      open.delete(k);
    }
  }
  open.forEach(finish);
  return asc.reverse();
}

const ORDER: Record<string, number> = { connect: 0, login: 1, send: 2, receive: 2, disconnect: 3 };
const order = (e: AuditEvent) => ORDER[e.action] ?? 2;

const ACTION: Record<string, string> = {
  connect: "Connected",
  login: "Session started",
  disconnect: "Disconnected",
  send: "Files sent to the technician",
  receive: "Files received from the technician",
  alarm: "Alarm",
};
export const actionLabel = (e: AuditEvent) => ACTION[e.action] ?? e.action;

/** `AlarmAuditType` in vendor/rustdesk/src/server/connection.rs. */
const ALARM: Record<number, string> = {
  0: "blocked by the IP whitelist",
  1: "over 30 failed logins",
  2: "6 failed logins within a minute",
  6: "failed logins from one IPv6 prefix",
  7: "terminal login backoff",
  8: "too many terminal logins",
  9: "session scope violation",
  10: "blocked by the ID whitelist",
};
export const alarmLabel = (typ?: number) => (typ == null ? "" : ALARM[typ] ?? `type ${typ}`);

export function duration(s?: number): string {
  if (s == null) return "";
  if (s < 60) return `${s} s`;
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  return h ? `${h} h ${m} min` : `${m} min ${s % 60} s`;
}

/** Text search over the fields an admin would type: host ID, technician ID/name, IPs, path. */
export function matches(r: Row, q: string): boolean {
  const t = q.trim().toLowerCase().replace(/\s+/g, "");
  if (!t) return true;
  return [r.id, r.uuid, r.peer_id, r.peer_name, r.ip, r.src, r.path].some((v) => v && v.toLowerCase().replace(/\s+/g, "").includes(t));
}

/** Spreadsheet-safe cell: quoted, and text a spreadsheet would run as a formula is prefixed with '. */
function cell(v: unknown): string {
  let s = v == null ? "" : String(v);
  if (/^[=+\-@\t\r]/.test(s)) s = `'${s}`;
  return `"${s.replace(/"/g, '""')}"`;
}

export function toCsv(rows: Row[]): string {
  const head = ["time_utc", "host_id", "host_uuid", "technician_id", "technician_name", "technician_ip", "host_ip", "action", "connection", "path", "files", "duration_s", "relay"];
  const lines = rows.map((r) =>
    [
      new Date(r.ts * 1000).toISOString(),
      r.id,
      r.uuid,
      r.peer_id,
      r.peer_name,
      r.ip,
      r.src,
      r.action,
      r.conn_type,
      r.path,
      r.files ? `${r.num ?? r.files.length}: ${r.files.map((f) => f[0]).join(", ")}` : r.info,
      r.duration,
      r.relay,
    ]
      .map(cell)
      .join(","),
  );
  return [head.join(","), ...lines].join("\r\n") + "\r\n";
}

// Team activity (design 12.5): records of the relays' team audit logs as the page shows and
// exports them. activity.test.ts.

export interface ActivityRecord {
  ts: number;
  event: string;
  actor?: { key_fingerprint: string; name: string; oid?: string; upn?: string };
  ip?: string;
  target?: { uuid?: string; tether_id?: string; customer?: string };
  detail?: unknown;
  result?: number;
  self_reported?: boolean;
  relay: string;
}
export interface RelayState {
  server: string;
  ok: boolean;
  error?: string;
  head?: string;
  more?: boolean;
  broken?: boolean;
  /** The chain check since this PC's stored head (activity.rs): first, ok, partly, undecided, broken. */
  chain?: string;
  warnings: string[];
}

export const who = (r: ActivityRecord) => (r.actor ? (r.actor.upn ? `${r.actor.name} (${r.actor.upn})` : r.actor.name) : "–");
export const targetText = (r: ActivityRecord) => [r.target?.tether_id, r.target?.uuid, r.target?.customer && `customer ${r.target.customer}`].filter(Boolean).join(" · ");
export const detailText = (r: ActivityRecord) => (r.detail == null ? "" : typeof r.detail === "string" ? r.detail : JSON.stringify(r.detail));

/** A refused request (403 and the like) or a sign-in refused. */
export const refused = (r: ActivityRecord) => (r.result != null && r.result >= 400) || r.event === "sign-in-refused" || r.event === "admin-refused";

const cell = (s: string) => (/[",\n\r]/.test(s) || /^[=+\-@]/.test(s) ? `"${(/^[=+\-@]/.test(s) ? "'" : "") + s.replace(/"/g, '""')}"` : s);

/** CSV of the records (one line each, header first); cells that a spreadsheet would run are quoted. */
export function activityCsv(rs: ActivityRecord[]): string {
  const head = ["time (UTC)", "relay", "person", "oid", "event", "target", "detail", "result", "self-reported", "ip"];
  const rows = rs.map((r) => [new Date(r.ts * 1000).toISOString(), r.relay, who(r), r.actor?.oid ?? "", r.event, targetText(r), detailText(r), r.result?.toString() ?? "", r.self_reported ? "yes" : "", r.ip ?? ""]);
  return [head, ...rows].map((row) => row.map(cell).join(",")).join("\r\n") + "\r\n";
}

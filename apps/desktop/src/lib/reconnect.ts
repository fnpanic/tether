/**
 * Viewer reconnect policy. Pure: Session.tsx feeds it failures and runs the timers.
 *
 * - The core's `msgbox.retry` flag (`check_if_retry`) says whether a failure is a transport
 *   problem; texts that mean a decision by the other side or by policy are never retried.
 * - "Transient" outcomes (peer offline, encryption refused because the restarted peer has not
 *   re-registered its key yet) never start reconnecting, but are expected while the peer comes
 *   back, so they do not end a series that is already running. Every attempt still refuses an
 *   unencrypted connection.
 */

/**
 * Delay before reconnect attempt n (1-based): at once, then 0.25, 0.5, 1, 2, 4, 8 s, then every
 * 10 s. A relay restart is over in about a second, so the first attempts are packed into it.
 */
export function delayMs(attempt: number): number {
  const steps = [0, 250, 500, 1000, 2000, 4000, 8000];
  return steps[attempt - 1] ?? 10000;
}

/** After two failed attempts go through the relay (harmless: production is relay-only). */
export const forceRelay = (attempt: number) => attempt > 2;

export type FailureKind = "retryable" | "transient" | "fatal";

/** Transport failures the core does not flag as retryable ("Failed to connect via relay…"). */
const TRANSPORT = /failed|timeout|timed out|reset|refused|handshake|connect|relay|network|unreachable|closed|io error|os error/i;

const FATAL = /password|reject|denied|declin|manually|not allowed|restricted|policy|locked|update|upgrade|version|incoming only|mismatch|2fa/i;

export function classify(m: { type: string; title: string; text: string; retry?: boolean }): FailureKind {
  const t = m.text.toLowerCase();
  // "Restart remote computer": the core ends the session as the peer goes down; bring it back.
  if (m.type.startsWith("restarting")) return "retryable";
  // While the peer restarts it is offline, then briefly has no signed key (E2E refused).
  if (t.includes("offline") || t.includes("not exist") || t.includes("end-to-end encrypted")) return "transient";
  if (m.type.includes("password") || FATAL.test(m.text) || FATAL.test(m.title)) return "fatal";
  if (m.type === "error" && m.retry !== false) return "retryable";
  // Such a failure while reconnecting (the relay is still restarting) must not end the series.
  if (m.type === "error" && TRANSPORT.test(m.text)) return "transient";
  return "fatal";
}

export interface Series {
  attempt: number;
  startedAt: number;
}

export type Step =
  | { kind: "retry"; series: Series; delayMs: number; forceRelay: boolean }
  | { kind: "give-up" };

/**
 * Next step after a failure. `series` is null while the session was working.
 * `limitMinutes` is policy `connect.reconnect-minutes` (0 disables reconnecting).
 */
export function onFailure(series: Series | null, kind: FailureKind, now: number, limitMinutes: number): Step {
  if (kind === "fatal" || limitMinutes <= 0) return { kind: "give-up" };
  if (!series && kind !== "retryable") return { kind: "give-up" };
  const s = series ?? { attempt: 0, startedAt: now };
  if (now - s.startedAt >= limitMinutes * 60_000) return { kind: "give-up" };
  const attempt = s.attempt + 1;
  return { kind: "retry", series: { ...s, attempt }, delayMs: delayMs(attempt), forceRelay: forceRelay(attempt) };
}

/** Policy value in minutes; empty or invalid falls back to the default of 10. */
export function reconnectMinutes(v: string | undefined | null): number {
  const n = Number.parseFloat(v ?? "");
  return Number.isFinite(n) && n >= 0 ? n : 10;
}

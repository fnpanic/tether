/**
 * Viewer reconnect policy. Pure: Session.tsx feeds it failures and runs the timers.
 *
 * - The core's `msgbox.retry` flag (`check_if_retry`) says whether a failure is a transport
 *   problem; texts that mean a decision by the other side or by policy are never retried.
 * - "Offline" ends a working session (never starts reconnecting) but is expected while the
 *   peer comes back, so it does not end a reconnect series that is already running.
 */

/** Delay before reconnect attempt n (1-based): 0.5, 1, 2, 4, 8 s, then every 10 s. */
export function delayMs(attempt: number): number {
  const steps = [500, 1000, 2000, 4000, 8000];
  return steps[attempt - 1] ?? 10000;
}

/** After two failed attempts go through the relay (harmless: production is relay-only). */
export const forceRelay = (attempt: number) => attempt > 2;

export type FailureKind = "retryable" | "offline" | "fatal";

const FATAL = /password|reject|denied|declin|manually|not allowed|restricted|policy|locked|update|upgrade|version|incoming only|mismatch|2fa/i;

export function classify(m: { type: string; title: string; text: string; retry?: boolean }): FailureKind {
  const t = m.text.toLowerCase();
  if (t.includes("offline") || t.includes("not exist")) return "offline";
  if (m.type.includes("password") || FATAL.test(m.text) || FATAL.test(m.title)) return "fatal";
  if (m.type === "error" && m.retry !== false) return "retryable";
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

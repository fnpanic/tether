/**
 * Windows admin prompts (UAC) on a TetherQS host. The core reports them as msgboxes
 * (`custom-uac-nocancel`, `custom-elevated-foreground-nocancel`) and withdraws them with
 * `cancel_msgbox` (tag `<type>-<title>-<text>-<link>`). They are notices: the session goes on.
 */
export type HostNote = "uac" | "elevated-window";

/** The notice a msgbox type or cancel_msgbox tag is about, or null. */
export function hostNote(typeOrTag: string): HostNote | null {
  if (typeOrTag.startsWith("custom-uac-nocancel")) return "uac";
  if (typeOrTag.startsWith("custom-elevated-foreground-nocancel")) return "elevated-window";
  return null;
}

export const NOTE_TEXT: Record<HostNote, string> = {
  uac: "An admin prompt (UAC) is open on the remote computer. You cannot see or answer it from here: the person there has to click Yes or No.",
  "elevated-window":
    "The window in front runs as administrator: Windows ignores your mouse and keyboard in it. Request admin access (Remote actions) or switch to another window.",
};

export const ADMIN_ON = "Admin access is on: admin windows and UAC prompts now work in this session.";
export const DECLINED =
  "Admin access was declined on the remote computer (or its prompt was closed). The session continues without admin rights; you can request it again under Remote actions.";
// Any other prompt (a program, a protected folder): its answer stays on the remote computer.
export const CLOSED = "The admin prompt on the remote computer was answered or closed.";

/** Time for the host to report admin access after a UAC prompt closed (helper start, 1 s polling). */
export const UAC_SETTLE_MS = 5000;

/**
 * What to tell the technician once a UAC prompt closed and settled. Only Tether's own request has a
 * known meaning: no admin access afterwards means No (or closed). The host cannot learn how other
 * prompts were answered (consent.exe runs as SYSTEM).
 */
export function uacOutcome(adminOn: boolean, requested: boolean): string | null {
  if (adminOn) return null; // portable_service says so (ADMIN_ON)
  return requested ? DECLINED : CLOSED;
}

/** The host's answer to "Request admin access" (core ElevationResponse) when it failed. */
export function elevationError(text: string): string {
  if (/no need/i.test(text)) return "Admin access is already on for this remote computer.";
  if (/no permission/i.test(text)) return "Admin access cannot be requested: the person at the remote computer has turned off mouse and keyboard control.";
  return `Admin access could not be requested: ${text}`;
}

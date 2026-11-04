/**
 * Why the password screen is shown, when it is not simply a first login. The core sends
 * `input-password` with text `saved-password-outdated` after the host turned down a remembered
 * password (and has forgotten it).
 */
export function passwordWhy(text: string, wasConnected: boolean): string | null {
  if (wasConnected)
    return "The connection was interrupted for too long to continue by itself, or the person there ended the session. A one-time password changes after each login: enter the one shown there now.";
  if (text === "saved-password-outdated")
    return "The saved password for this device no longer works (TetherQS shows a new one-time password after each session). Enter the one shown there now.";
  return null;
}

/**
 * How long a password request waits for the host to let this client in by itself (a reconnect or
 * a file transfer joining the open session): the core asks first even then.
 */
export const RESUME_GRACE_MS = 2500;

// Who may do what in Tether Manage (design 12.4): the vault (keys unlocked on this PC) and the
// Tether Manage roles of a staff sign-in. The relay enforces the same rows (server/api main.rs
// `session_may`); the vault signs while someone is signed in only for a KeyHolder (main.rs
// `may_sign`). roles.test.ts.

export type Role = "auditor" | "operator" | "policy-editor" | "publisher" | "key-holder";
export const ROLES: Role[] = ["auditor", "operator", "policy-editor", "publisher", "key-holder"];
export const ROLE_TEXT: Record<Role, string> = {
  auditor: "Auditor",
  operator: "Operator",
  "policy-editor": "Policy editor",
  publisher: "Publisher",
  "key-holder": "Key holder",
};

/** The sign-in as the backend reports it (signin_status; the desktop module's status, no token). */
export interface SignIn {
  offered: boolean | null;
  problem: string | null;
  busy: boolean;
  signed_in: boolean;
  name?: string;
  upn?: string;
  roles?: string[];
  exp?: number;
  manage: Role[];
  note: string | null;
  alarm: string | null;
  relays: { relay: string; ok: boolean; text: string }[];
}

export interface Access {
  /** The keys are unlocked on this PC. */
  vault: boolean;
  session: boolean;
  roles: Role[];
}

export function access(unlocked: boolean, s: SignIn | undefined | null): Access {
  const session = !!s?.signed_in;
  return { vault: unlocked, session, roles: session ? (s?.manage ?? []) : [] };
}

const has = (a: Access, r: Role) => a.roles.includes(r);

/** The vault signs: without a sign-in (break-glass, as before) or for a KeyHolder. */
export const canSign = (a: Access) => a.vault && (!a.session || has(a, "key-holder"));

export type Action = "sign" | "operate" | "propose-policy" | "propose-team" | "propose-release" | "read-proposals-policy" | "read-proposals-release";

export function can(a: Access, x: Action): boolean {
  switch (x) {
    case "sign":
      return canSign(a);
    case "operate":
      return canSign(a) || (a.session && has(a, "operator"));
    case "propose-policy":
    case "propose-team":
      return a.session && has(a, "policy-editor");
    case "propose-release":
      return a.session && has(a, "publisher");
    case "read-proposals-policy":
      return canSign(a) || (a.session && (has(a, "policy-editor") || has(a, "key-holder")));
    case "read-proposals-release":
      return canSign(a) || (a.session && (has(a, "publisher") || has(a, "key-holder")));
  }
}

export type Page = "home" | "policy" | "devices" | "sessions" | "activity" | "team" | "releases" | "page" | "custom" | "diag" | "keys";

/** Whether `page` is shown: everything with the vault (as before, plus Team activity); with a
 * sign-in only, what the person's roles read or operate. */
export function pageAllowed(p: Page, a: Access): boolean {
  if (canSign(a)) return true;
  if (!a.session || !a.roles.length) return false;
  switch (p) {
    case "home":
    case "policy":
    case "devices":
    case "sessions":
    case "activity":
    case "releases":
      return true;
    case "team":
      return has(a, "policy-editor") || has(a, "key-holder");
    case "diag":
      return has(a, "operator");
    default:
      return false;
  }
}

// Staff sign-in (design 12.2, 12.4): what the Team dialog says about it, and what this app's team
// permissions allow in any window. The backend (oidc.rs, team.rs) holds the session and the tokens;
// none of them reaches the webview.
import { create } from "zustand";
import { invoke } from "./ipc";

export interface RelayAnswer {
  relay: string;
  ok: boolean;
  text: string;
}

/** `team_signin_status`: no token, only what may be shown. */
export interface SignIn {
  /** Whether the relays offer sign-in (null: not checked yet). */
  offered: boolean | null;
  /** Why sign-in cannot be used (unsigned, older or disagreeing settings, a provider address not allowed). */
  problem?: string | null;
  busy: boolean;
  signed_in: boolean;
  name?: string;
  upn?: string;
  roles?: string[];
  exp?: number;
  write?: boolean;
  audit?: boolean;
  sbom?: boolean;
  agent?: boolean;
  customers?: string[];
  relays: RelayAnswer[];
  /** Why the last sign-in failed or the session ended. */
  note?: string | null;
  /** A relay saw this sign-in used by another key. */
  alarm?: string | null;
}

export type Perm = "write" | "audit" | "sbom" | "agent";
export type Perms = Record<Perm, boolean>;

/** What each permission stands for, and the sentence when it is missing. */
export const PERM_TEXT: Record<Perm, { what: string; missing: string }> = {
  write: { what: "change shared tags and links", missing: "Your team role does not allow changes (write)." },
  audit: { what: "read a computer's session log", missing: "Your team role does not include session logs (audit)." },
  sbom: { what: "software lists and software search", missing: "Your team role does not include software lists (sbom)." },
  agent: { what: "install or remove the health monitoring agent", missing: "Your team role does not allow installing or removing the health monitoring agent (agent)." },
};

/** This app's permissions as the backend knows them (the session's, else the roster entry's; all
 *  allowed without team access). Any window: the session window has no host list of its own. */
export const usePerms = create<Perms>(() => ({ write: true, audit: true, sbom: true, agent: true }));
export const loadPerms = () =>
  invoke<Perms & { signed_in: boolean }>("team_permissions").then(
    (p) => p && usePerms.setState({ write: p.write !== false, audit: p.audit !== false, sbom: p.sbom !== false, agent: p.agent !== false }),
    () => {},
  );

/** A backend sentence without the state prefix it carries (`provider:`, `timeout:`, …). */
const plain = (s: string) => s.replace(/^[a-z-]+: /, "");

const time = (t: number) => new Date(t * 1000).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });

/** "May …; not: …" for a signed-in person, and the customers the relay limits them to. */
export function permissionLine(s: Partial<Perms> & { customers?: string[] }): string {
  const all: Perm[] = ["write", "audit", "sbom", "agent"];
  const may = all.filter((p) => s[p]).map((p) => PERM_TEXT[p].what);
  const not = all.filter((p) => !s[p]).map((p) => PERM_TEXT[p].what);
  const c = s.customers ?? ["*"];
  const scope = c.includes("*") ? "" : ` Only computers of customers ${c.join(", ")}.`;
  return `${may.length ? `May ${may.join(", ")}.` : "May see the host list and health reports."}${not.length ? ` Not: ${not.join(", ")}.` : ""}${scope}`;
}

export interface SignInView {
  /** Show the sign-in part at all (no relay offers sign-in: the team works as before, by key). */
  show: boolean;
  title: string;
  lines: string[];
  /** Sentences that need attention (a refusal, an ended session). */
  warn: string[];
  action: "sign-in" | "sign-out" | "cancel" | null;
}

/** The Team dialog's sign-in part. `member`: this app's key is on the team roster (it works without signing in). */
export function signInView(s: SignIn | undefined, member: boolean): SignInView {
  const none: SignInView = { show: false, title: "", lines: [], warn: [], action: null };
  if (!s || s.offered === null) return none;
  const refused = s.relays.filter((r) => !r.ok).map((r) => `${r.relay}: ${r.text}`);
  if (s.signed_in) {
    const roles = (s.roles ?? []).join(", ");
    return {
      show: true,
      title: `Signed in as ${s.name ?? "?"}${s.upn ? ` (${s.upn})` : ""}`,
      lines: [roles ? `Roles: ${roles}.` : "No roles named.", permissionLine(s), ...(s.exp ? [`Session until ${time(s.exp)}; renewed automatically.`] : [])],
      warn: refused,
      action: "sign-out",
    };
  }
  if (s.busy) return { show: true, title: "Signing in…", lines: ["Finish signing in in your web browser, then come back here."], warn: [], action: "cancel" };
  if (s.problem) return { show: true, title: "Sign-in is not available", lines: [], warn: [s.problem], action: null };
  if (!s.offered) return none;
  const note = s.note ? [plain(s.note)] : [];
  return {
    show: true,
    title: "Not signed in",
    lines: [member ? "This app's key is on your team's list, so team features work without signing in. Sign in to work under your own name." : "Sign in with your organisation's account to use team features.", ...(note[0] === "Signed out." ? note : [])],
    warn: note.length && note[0] !== "Signed out." ? note : refused,
    action: "sign-in",
  };
}

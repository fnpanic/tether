// The health monitoring agent through Tether (spec 7.3, 7.4): what the session dialog and the
// practice's Settings line say. Everything in an answer comes from the other computer: the state
// picks the sentence, `detail` is only an added line, every field is clipped plain text.
import { clean } from "./hosts/sbom";

/** `session_agent` (and the local status): `at` is unix seconds of the last change, 0 for none. */
export interface AgentAnswer {
  state: string;
  version: string;
  installed_by: string;
  detail: string;
  at: number;
}

export type AgentAction = "status" | "install" | "remove";
/** What a dialog button does: ask again (`status`), or open a confirmation. */
export type AgentOffer = "again" | "install" | "reinstall" | "remove";

export const AGENT_MENU = "Health monitoring agent…";

export const OFFER_LABEL: Record<AgentOffer, string> = { again: "Ask again", install: "Install…", reinstall: "Install again…", remove: "Remove…" };

export const PROGRESS: Record<AgentAction, string> = {
  status: "Asking the remote computer…",
  install: "Installing on the remote computer… This can take up to three minutes. Without Tether's service there, the person at the remote computer is asked first.",
  remove: "Removing on the remote computer…",
};

/** Text from the other computer: plain string, hidden characters (control, bidi, zero-width) removed, clipped. */
export const str = (v: unknown, n: number) => (typeof v === "string" ? clean(v, n) : "");

/** A unix time as a date; anything that is not one says so. */
export function agentDate(t: unknown): string {
  const d = typeof t === "number" && t > 0 && t < 1e11 ? new Date(t * 1000) : null;
  return d && !Number.isNaN(d.getTime()) ? d.toLocaleDateString(undefined, { dateStyle: "medium" }) : "";
}

const BY: Record<string, string> = { policy: "policy", session: "technician in a session" };
/** `installed_by` is the other side's word: only an own key of BY counts ("constructor" does not). */
const by = (v: unknown): string => (typeof v === "string" && Object.hasOwn(BY, v) ? BY[v] : "");

export interface AgentView {
  text: string;
  /** The remote computer's own line, clipped; empty when the state's sentence says it all. */
  detail: string;
  /** Not what was asked for (the text says so in words too). */
  problem: boolean;
  offers: AgentOffer[];
}

/** The dialog for an answer; `did`: the action that brought it (an install's own result reads differently). */
export function agentView(a: AgentAnswer, did: AgentAction = "status"): AgentView {
  const v = str(a.version, 32);
  const date = agentDate(a.at);
  const detail = str(a.detail, 300);
  const view = (text: string, offers: AgentOffer[], withDetail = false, problem = false): AgentView => ({ text, detail: withDetail ? detail : "", problem, offers });
  switch (a.state) {
    case "not-installed":
      return view("The health monitoring agent is not installed on the remote computer.", ["install"], did !== "status");
    case "installed": {
      if (did === "install") return view(`Installed${v ? ` ${v}` : ""}, linked to this host; first report within five minutes.`, ["remove"], true);
      const who = by(a.installed_by);
      // The agent's link names another Tether ID than this host's (spec 11.4): only a plain ID is shown.
      const other = /^linked to another ID \(([0-9A-Za-z_-]{1,40})\)$/.exec(detail)?.[1];
      const link = other ? ` It is linked to another ID (${other}), so its health shows on that host's row.` : "";
      return view(`Installed${v ? ` ${v}` : ""} by Tether${date ? ` on ${date}` : ""}${who ? ` (${who})` : ""}.${link}`, ["remove"], did !== "status");
    }
    case "installed-by-hand":
      return view(`Installed by hand${v ? ` ${v}` : ""} (managed outside Tether).`, ["remove"], did !== "status");
    case "removed-locally":
      return view(`Removed on the remote computer${date ? ` on ${date}` : ""} by someone at that computer. Tether does not install it again by itself.`, ["reinstall"]);
    case "removed-by-tether":
      return view(did === "remove" ? "Removed: the agent's service, folders and enrolment are gone." : `Removed by Tether${date ? ` on ${date}` : ""}.`, ["install"], did !== "status");
    case "failed":
      return view("The last action on the remote computer did not succeed.", ["again", "install"], true, true);
    case "running":
      return view("Still running on the remote computer. Ask again in a moment to see the result.", ["again"]);
    case "busy":
      // The host's reason (what runs, or "ask again in N minutes") is the added line.
      return view("Tether on the remote computer is busy with the agent right now. Ask again in a moment.", ["again"], true);
    case "not-allowed":
      return view("Not allowed by the remote computer's policy. The person or administrator of that computer decides (machine policy).", [], true, true);
    case "needs-service":
      return view("Needs Tether's service on the remote computer (Tether installed as a service).", [], false, true);
    case "not-supported":
      return view("The remote computer does not support this: the health monitoring agent is for Windows computers with Tether installed.", [], false, true);
    case "too-old":
      return view("Tether on the remote computer is too old (needs 0.7.0).", [], false, true);
    default:
      return view("The remote computer gave an answer this version of Tether does not know.", ["again"], true, true);
  }
}

export interface AgentConfirm {
  title: string;
  /** The warning that must be read first (install again after the practice removed it). */
  warn?: string;
  text: string;
  button: string;
  action: "install" | "remove";
}

const INSTALL_TEXT =
  "Tether on the remote computer installs your organisation's signed health monitoring agent as a Windows service and links it to this host. It stays after this session ends and reports the computer's health and, if your organisation collects it, the list of installed programs to your organisation's servers. The person at the computer can remove it under Settings › Apps (Tether Monitor).";

/** The confirmation for a dialog button (not for "Ask again"). */
export function agentConfirm(offer: Exclude<AgentOffer, "again">, a: AgentAnswer): AgentConfirm {
  if (offer === "remove")
    return {
      title: "Remove the health monitoring agent?",
      text: `Tether on the remote computer uninstalls the agent: its service, its folders and its enrolment on every server. Health reports for this host stop, and Tether does not install it again by policy.${a.state === "installed-by-hand" ? " It was installed by hand, outside Tether." : ""}`,
      button: "Remove",
      action: "remove",
    };
  if (offer === "reinstall") {
    const date = agentDate(a.at);
    return {
      title: "Install the agent again?",
      warn: `Someone at the remote computer removed the health monitoring agent${date ? ` on ${date}` : ""}. Install it again only if they agree; they see this session.`,
      text: INSTALL_TEXT,
      button: "Install again",
      action: "install",
    };
  }
  return { title: "Install the health monitoring agent?", text: INSTALL_TEXT, button: "Install", action: "install" };
}

/** The session menu item: the host takes the message only with control of mouse and keyboard. */
export function agentMenu(viewOnly: boolean, controlOff: boolean): { disabled: boolean; hint: string } {
  if (viewOnly) return { disabled: true, hint: "Not in a view-only session: it needs control of the remote computer." };
  if (controlOff) return { disabled: true, hint: "Needs control: the person at the remote computer has turned off mouse and keyboard." };
  return { disabled: false, hint: "Install or remove it on the remote computer" };
}

/** Settings on this computer (spec 7.3): read-only, only for an agent Tether installed. */
export function practiceLine(a: AgentAnswer | null | undefined): string | null {
  if (!a || a.state !== "installed") return null;
  const date = agentDate(a.at);
  const who = by(a.installed_by);
  return `Health monitoring agent: installed by Tether${date ? ` on ${date}` : ""} at your organisation's request${who ? ` (${who})` : ""}. What it reports: the health of this computer's software and, if your organisation collects it, the list of installed programs (see Tether's privacy policy). Remove it in Settings › Apps (Tether Monitor).`;
}

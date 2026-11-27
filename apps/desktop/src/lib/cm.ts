// Connection manager window: which requester is shown, and what the request says.
// The shown client never changes by itself: a request or message from someone else must not put
// its Accept button under a click or key press meant for the one on screen. Others get a mark.

export interface CmClient {
  id: number;
  peer_id: string;
  name: string;
  authorized: boolean;
  is_file_transfer: boolean;
  is_view_camera?: boolean;
  is_terminal?: boolean;
  /** Target of a port forward ("host:port"), empty otherwise. */
  port_forward?: string;
}

export interface CmState<C extends CmClient> {
  clients: C[];
  active: number | null;
  /** Clients that asked or wrote while another one was shown. */
  attention: number[];
}

export const cmEmpty = { clients: [], active: null, attention: [] };

/** A new client, or new facts about a known one (accepted, permission changed). */
export function cmAdd<C extends CmClient>(s: CmState<C>, c: C): CmState<C> {
  const known = s.clients.some((x) => x.id === c.id);
  const active = s.active ?? c.id;
  return {
    clients: known ? s.clients.map((x) => (x.id === c.id ? c : x)) : [...s.clients, c],
    active,
    attention: known || active === c.id || c.authorized ? s.attention : [...s.attention, c.id],
  };
}

/** The shown client went away: a running session takes its place, never a waiting request (its
 *  Accept would appear where End session just was); with only requests left the user picks one. */
export function cmRemove<C extends CmClient>(s: CmState<C>, id: number): CmState<C> {
  const clients = s.clients.filter((x) => x.id !== id);
  const active = s.active === id ? ([...clients].reverse().find((x) => x.authorized)?.id ?? null) : s.active;
  return { clients, active, attention: clients.filter((x) => s.attention.includes(x.id) || (active === null && !x.authorized)).map((x) => x.id) };
}

/** A chat message from `id`. */
export function cmNote<C extends CmClient>(s: CmState<C>, id: number): CmState<C> {
  return s.active === id || s.attention.includes(id) ? s : { ...s, attention: [...s.attention, id] };
}

export function cmShow<C extends CmClient>(s: CmState<C>, id: number): CmState<C> {
  return { ...s, active: id, attention: s.attention.filter((x) => x !== id) };
}

/** The request in one sentence: who (the ID is the fact, the name is their claim) and what for. */
export function requestText(c: CmClient, id: string): string {
  const what = c.is_file_transfer
    ? "send and receive files on this computer"
    : c.is_view_camera
      ? "see this computer's camera"
      : c.is_terminal
        ? "open a terminal (command line) on this computer"
        : c.port_forward
          ? `reach ${c.port_forward} through this computer (port forward)`
          : "see and control this screen";
  return c.name ? `${c.name} (ID ${id}) asks to ${what}. The name is chosen by the other side and is not verified.` : `The device with ID ${id} asks to ${what}.`;
}

/** The `health_agent` CM event (`{id, action, outcome}`, sent only when the host acted on a
 *  technician's install or remove): one line for this computer's user. Its fields come from the
 *  core but are not shown: only a known action gives a line, only its known success a specific one. */
export function agentLine(p: unknown): string | null {
  const { action, outcome } = (p && typeof p === "object" ? p : {}) as { action?: unknown; outcome?: unknown };
  if (action === "install") return outcome === "installed" ? "The technician installed the health monitoring agent." : "The technician asked to install the health monitoring agent.";
  if (action === "remove") return outcome === "removed-by-tether" ? "The technician removed the health monitoring agent." : "The technician asked to remove the health monitoring agent.";
  return null;
}

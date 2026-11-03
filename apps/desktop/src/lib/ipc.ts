import { Channel, invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen, type UnlistenFn } from "@tauri-apps/api/event";
import { mock } from "./mock";

export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export function invoke<T = void>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  return inTauri ? tauriInvoke<T>(cmd, args) : mock.invoke<T>(cmd, args);
}

export interface Evt<T = any> {
  name: string;
  payload: T;
}

/** Events the core sent to this window only: a plain `listen` (target Any) also receives every
 *  other session window's events in the same process. */
export async function listen(event: "session" | "cm", cb: (e: Evt) => void): Promise<UnlistenFn> {
  if (!inTauri) return mock.listen(event, cb);
  const { getCurrentWebviewWindow } = await import("@tauri-apps/api/webviewWindow");
  const label = getCurrentWebviewWindow().label;
  return tauriListen<Evt>(event, (e) => cb(e.payload), { target: { kind: "WebviewWindow", label } });
}

/** `listen` for effects: `off` is synchronous and also works before the listener is registered
 *  (React StrictMode runs cleanup right away), so no listener leaks and no event arrives twice.
 *  `ready` resolves once events are delivered; it rejects when `off` came first. */
export function subscribe(event: "session" | "cm", cb: (e: Evt) => void): { off: () => void; ready: Promise<void> } {
  let un: UnlistenFn | null = null;
  let dead = false;
  const ready = listen(event, (e) => !dead && cb(e)).then((u) => {
    if (dead) {
      u();
      throw new Error("unsubscribed");
    }
    un = u;
  });
  ready.catch(() => {});
  return {
    off: () => {
      dead = true;
      un?.();
    },
    ready,
  };
}

export function videoChannel(onPacket: (buf: ArrayBuffer) => void): Channel<ArrayBuffer> | null {
  if (!inTauri) return null;
  const ch = new Channel<ArrayBuffer>();
  ch.onmessage = onPacket;
  return ch;
}

export interface Status {
  id: string;
  password: string;
  status: number;
  key_confirmed: boolean;
  installed: boolean;
  version: string;
}

export interface Peer {
  id: string;
  hostname: string;
  username: string;
  platform: string;
}

export const api = {
  status: () => invoke<Status>("status"),
  refreshPassword: () => invoke("refresh_password"),
  setPermanentPassword: (password: string) => invoke("set_permanent_password", { password }),
  getOption: (key: string) => invoke<string>("get_option", { key }),
  setOption: (key: string, value: string) => invoke("set_option", { key, value }),
  tfaEnabled: () => invoke<boolean>("tfa_enabled"),
  tfaBegin: () => invoke<string>("tfa_begin"),
  tfaConfirm: (code: string) => invoke("tfa_confirm", { code }),
  tfaDisable: () => invoke("tfa_disable"),
  recentPeers: () => invoke<Peer[]>("recent_peers"),
  removePeer: (id: string) => invoke("remove_peer", { id }),
  installService: () => invoke("install_service"),
  /** `from`: live session key whose login a file transfer to the same peer reuses. */
  openSession: (peer: string, conn?: string, from?: string) => invoke<string>("open_session", { peer, conn, from }),
};

/** Display an ID as `123 456 789`. */
export function formatId(id: string): string {
  const d = id.replace(/\s+/g, "");
  if (!/^\d+$/.test(d)) return d;
  return d.replace(/\B(?=(\d{3})+(?!\d))/g, " ");
}

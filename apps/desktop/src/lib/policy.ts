// Admin policy as applied by the backend (src-tauri/src/policy.rs). The backend enforces it; the
// UI only greys out locked controls and leaves out hidden ones.
import { useEffect } from "react";
import { create } from "zustand";
import { inTauri, invoke } from "./ipc";

export interface Effective {
  profile: string;
  serial: number;
  group: string | null;
  values: Record<string, string>;
  locked: string[];
  hidden: string[];
  sources: string[];
}

export const usePolicyStore = create<{ eff: Effective | null; set: (e: Effective | null) => void }>((set) => ({
  eff: null,
  set: (eff) => set({ eff }),
}));

export const isLocked = (eff: Effective | null, id: string) => !!eff?.locked.includes(id);
export const isHidden = (eff: Effective | null, id: string) => !!eff?.hidden.includes(id);
/** Bool feature; true until the policy is known, matching the backend defaults for most features. */
export const isOn = (eff: Effective | null, id: string, def = true) => (eff ? eff.values[id] === "Y" : def);

export const locked = (id: string) => isLocked(usePolicyStore.getState().eff, id);
export const hidden = (id: string) => isHidden(usePolicyStore.getState().eff, id);

let started = false;
/** Load the policy once and follow `policy` events. */
export function startPolicy() {
  if (started) return;
  started = true;
  const set = usePolicyStore.getState().set;
  invoke<Effective | undefined>("policy_state")
    .then((e) => e && set(e))
    .catch(() => {});
  if (inTauri) {
    import("@tauri-apps/api/event").then(({ listen }) => listen<Effective>("policy", (e) => set(e.payload)));
  }
}

export function usePolicy() {
  useEffect(startPolicy, []);
  return usePolicyStore((s) => s.eff);
}

/** Backend errors look like `locked:<feature>: <text>`; show only the text. */
export function errorText(e: unknown): string {
  const s = String(e);
  const m = /^locked:[^:]+: (.*)$/s.exec(s);
  return m ? m[1] : s.replace(/^offline: /, "Cannot reach the server: ");
}

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export interface Perm {
  name: string;
  label: string;
  allowed: boolean;
  locked: boolean;
}

export interface Client {
  id: number;
  peer_id: string;
  name: string;
  authorized: boolean;
  is_file_transfer: boolean;
  [perm: string]: unknown;
}

export interface State {
  id: string;
  password: string;
  status: number;
  version: string;
  elevated: boolean;
  blocked: boolean;
  /** macOS only. */
  mac: { screen: boolean; accessibility: boolean } | null;
  branding: {
    title: string;
    accent: string;
    welcome: string;
    elevate: boolean;
    approve_mode: string;
    consent_text: string;
    diagnostics: boolean;
    perms: Perm[];
    /** Custom app name (signed profile in this exe), null for plain TetherQS. */
    custom_app: string | null;
    /** data: URL of the custom app's logo, "" = none. */
    logo: string;
  };
  update: { available: string | null; notes: string; required: boolean; forced_at: number | null; installing: boolean; error: string | null; last_check: string | null; progress: [number, number] | null; serial: number; stale: boolean; phase: string; waiting_idle: boolean };
  clients: Client[];
}

export interface Diagnostics {
  version: string;
  id: string;
  servers: string[];
  proxy: string;
  elevated: boolean;
  policy_serial: number;
  policy_group: string | null;
}

export interface Probe {
  reachable: boolean;
  ms: number | null;
  error: string | null;
}

export const api = {
  state: () => invoke<State>("state"),
  refreshPassword: () => invoke("refresh_password"),
  authorize: (id: number) => invoke("cm_authorize", { id }),
  close: (id: number) => invoke("cm_close", { id }),
  chat: (id: number, text: string) => invoke("cm_chat", { id, text }),
  permission: (id: number, name: string, enabled: boolean) => invoke("cm_permission", { id, name, enabled }),
  recordConsent: (connId: number, allow: boolean) => invoke("cm_record_consent", { connId, allow }),
  elevate: () => invoke("elevate"),
  macFix: (which: "screen" | "accessibility") => invoke("mac_fix", { which }),
  /** Result text ("… is up to date", "Version … is available"); rejects with the reason. */
  checkUpdate: () => invoke<string>("update_check"),
  installUpdate: () => invoke("update_install"),
  diagnostics: () => invoke<Diagnostics>("diagnostics"),
  diagnosticsProbe: (server: string) => invoke<Probe>("diagnostics_probe", { server }),
};

/** Connection-manager events: add_connection, remove_connection, refused, policy, record_consent_request. */
export function onCm(cb: (name: string, payload: any) => void) {
  return listen<{ name: string; payload: any }>("cm", (e) => cb(e.payload.name, e.payload.payload));
}

/** `123456789` → `123 456 789`. */
export function formatId(id: string): string {
  const d = id.replace(/\s+/g, "");
  return /^\d+$/.test(d) ? d.replace(/\B(?=(\d{3})+(?!\d))/g, " ") : d;
}

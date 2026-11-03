import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { mockInvoke } from "./mock";

export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export function invoke<T = void>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  return (inTauri ? tauriInvoke<T>(cmd, args) : mockInvoke<T>(cmd, args)).catch((e) => {
    throw new Error(typeof e === "string" ? e : e?.message ?? String(e));
  });
}

export type RuleState = "on" | "off" | "locked" | "hidden";
export interface Rule {
  state: RuleState;
  value?: string;
}
export type Layer = Record<string, Rule>;
export interface Group {
  profile: string;
  overrides: Layer;
}
export interface Bundle {
  serial: number;
  issued_at: number;
  profiles: Record<string, Layer>;
  groups: Record<string, Group>;
  assignments: Record<string, string>;
}
export interface Feature {
  id: string;
  area: string;
  en: string;
  de: string;
  kind: "bool" | "choice" | "text" | "number";
  default: string;
  choices: string[];
  off_value: string;
  option: string | null;
  profiles: string[];
}
export interface Effective {
  profile: string;
  serial: number;
  group: string | null;
  values: Record<string, string>;
  locked: string[];
  hidden: string[];
  sources: string[];
}
export interface Device {
  uuid: string;
  id: string;
  profile: string;
  version: string;
  os: string;
  serial: number;
  ip: string;
  last_seen: number;
}
export interface AppState {
  exists: boolean;
  unlocked: boolean;
  admin_pub: string | null;
  updater_pub: string | null;
  settings: Record<string, any> & { server: string };
  data_dir: string;
}

export const PROFILES = ["technician", "customer"] as const;

export const api = {
  state: () => invoke<AppState>("state"),
  admin: <T = any>(method: string, path: string, body?: unknown) => invoke<T>("admin", { method, path, body }),
  devices: () => api.admin<Device[]>("GET", "/api/admin/devices"),
  saveSettings: (settings: Record<string, unknown>) => invoke("settings_set", { settings }),
  publish: (bundle: Bundle) => invoke<number>("policy_publish", { bundle }),
};

export function emptyBundle(): Bundle {
  return { serial: 0, issued_at: 0, profiles: {}, groups: {}, assignments: {} };
}

export function ago(unix?: number | null): string {
  if (!unix) return "–";
  const s = Math.max(0, Date.now() / 1000 - unix);
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
  return `${Math.floor(s / 86400)} d ago`;
}

export function when(unix?: number | null): string {
  return unix ? new Date(unix * 1000).toLocaleString() : "–";
}

export function bytes(n?: number | null): string {
  if (n == null) return "–";
  const u = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) {
    n /= 1024;
    i++;
  }
  return `${n.toFixed(i ? 1 : 0)} ${u[i]}`;
}

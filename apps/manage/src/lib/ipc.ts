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
  /** Rules this build does not know or accept, left out of the result (`<layer>: <why>`). */
  rejected: string[];
}
export interface Device {
  uuid: string;
  id: string;
  profile: string;
  version: string;
  os: string;
  serial: number;
  /** Server key list the device applied (absent: none, or a version before key rotation). */
  server_keys_serial?: number;
  ip: string;
  last_seen: number;
}
export interface AppState {
  exists: boolean;
  unlocked: boolean;
  admin_pub: string | null;
  updater_pub: string | null;
  /** Public halves of the server keys generated here (key rotation), oldest first. */
  server_keys?: string[] | null;
  settings: Record<string, any> & { server: string };
  /** The relay list parsed from settings.server (first = reads). */
  servers: string[];
  data_dir: string;
}
/** Outcome of one operation on one relay. */
export interface RelayResult {
  server: string;
  ok: boolean;
  error?: string;
  detail: any;
}
export interface LiveRelay {
  server: string;
  reachable: boolean;
  error: string | null;
  serial: number | null;
  issued_at: number | null;
  /** Signed with this app's admin key. */
  verified: boolean;
  note: string | null;
}
export interface EffRow {
  id: string;
  value: string;
  locked: boolean;
  hidden: boolean;
  source: "default" | "profile";
}
/** What the relays serve now (policy_live). */
export interface Live {
  relays: LiveRelay[];
  serial: number | null;
  issued_at: number | null;
  effective: Record<string, EffRow[]>;
  /** Relay hosts the policy's servers.list names that are not in this app's relay list. */
  unconfigured: string[];
  bundle: Bundle | null;
}
export interface Change {
  scope: string;
  label: string | null;
  from: string | null;
  to: string | null;
  text: string;
}

export const PROFILES = ["technician", "customer"] as const;

export const api = {
  state: () => invoke<AppState>("state"),
  admin: <T = any>(method: string, path: string, body?: unknown) => invoke<T>("admin", { method, path, body }),
  /** Devices of every relay, merged by uuid. */
  devices: () => invoke<{ devices: Device[]; failed: string[] }>("devices_all"),
  saveSettings: (settings: Record<string, unknown>) => invoke("settings_set", { settings }),
  publish: (bundle: Bundle, skip: string[] = []) => invoke<{ serial: number; relays: RelayResult[] }>("policy_publish", { bundle, skip }),
  live: () => invoke<Live>("policy_live"),
  relays: () => invoke<RelayResult[]>("relays_check"),
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

/** Host of a relay address, for compact labels. */
export const host = (server: string) => server.replace(/^https?:\/\//, "").replace(/\/.*$/, "");
/** A publish target as shown: a relay's host, or "Cloudflare CDN" (target "CDN"). */
export const target = (server: string) => (server === "CDN" ? "Cloudflare CDN" : host(server));

/** Unix seconds of the last local midnight ("active today"). */
export function midnight(): number {
  const d = new Date();
  d.setHours(0, 0, 0, 0);
  return Math.floor(d.getTime() / 1000);
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

// Server key rotation (docs/key-rotation.md): which step comes next, the lists to publish and the
// relay commands. Pure, so it is unit-tested (serverkeys.test.ts).
import type { Device } from "./ipc";

export interface KeyEntry {
  key: string;
  not_before?: number;
  expires?: number;
}
/** One relay's `GET /api/server-keys` (serverkeys_live). */
export interface LiveKeys {
  server: string;
  reachable: boolean;
  verified?: boolean;
  serial?: number | null;
  keys?: KeyEntry[];
  note?: string;
  /** Public key the relay's hbbs signs with (`/rendezvous/pubkey`); absent when it cannot tell. */
  signing?: string;
  signing_note?: string;
}
/** Kept on this PC (server-keys/rotation.json). */
export interface Rotation {
  /** Key installed clients have now (TETHER_PUB_KEY). */
  from?: string;
  /** Key generated for the rotation. */
  to?: string;
  switched?: boolean;
  retired?: boolean;
  /** Folder "Save key files…" wrote id_ed25519 into, until "Delete key files". */
  files?: string;
}

export type Step = "generate" | "announce" | "switch" | "retire" | "relays" | "done";

/** Newest list signed by this app's admin key that any relay serves. */
export function newest(relays: LiveKeys[]): { serial: number; keys: string[] } | null {
  let best: { serial: number; keys: string[] } | null = null;
  for (const r of relays) {
    if (r.verified && r.serial != null && (!best || r.serial > best.serial)) best = { serial: r.serial, keys: (r.keys ?? []).map((k) => k.key) };
  }
  return best;
}

export function step(r: Rotation, live: { keys: string[] } | null): Step {
  if (!r.from || !r.to) return "generate";
  const k = live?.keys ?? [];
  if (k.length === 1 && k[0] === r.to) return r.retired ? "done" : "relays";
  if (k[0] === r.from && k.includes(r.to)) return r.switched ? "retire" : "switch";
  return "announce";
}

/** Relays whose hbbs does not sign with `key` yet, or does not tell: phase 2 is done only when
 * this is empty. */
export const notSwitched = (relays: LiveKeys[], key?: string): LiveKeys[] => relays.filter((r) => !key || r.signing !== key);

/** Phase 1: devices keep presenting the current key and also trust the new one. */
export function announceList(r: Rotation, expires?: number): KeyEntry[] {
  return [{ key: r.from!, ...(expires ? { expires } : {}) }, { key: r.to! }];
}

/** Phase 3: only the new key. */
export const retireList = (r: Rotation): KeyEntry[] => [{ key: r.to! }];

/** Devices active in the last 24 h, and how many of them applied list `serial` or newer. */
export function confirmed(devices: Device[], serial: number, now = Date.now() / 1000): { done: number; of: number } {
  const recent = devices.filter((d) => d.last_seen >= now - 86400);
  return { done: recent.filter((d) => (d.server_keys_serial ?? 0) >= serial).length, of: recent.length };
}

const host = (server: string) => server.replace(/^https?:\/\//, "").replace(/[/:].*$/, "");

/** Phase 2 on VM relays, one relay at a time (server/rotate-key.sh). */
export function switchCommands(servers: string[]): string {
  return servers
    .map(host)
    .map((h) => `scp id_ed25519 id_ed25519.pub rotate-key.sh opc@${h}:/tmp/\nssh opc@${h} 'sudo bash /tmp/rotate-key.sh switch /tmp/id_ed25519 && rm /tmp/id_ed25519 /tmp/id_ed25519.pub'`)
    .join("\n");
}

/** Phase 3b on VM relays: stop accepting the old key. */
export function retireCommands(servers: string[]): string {
  return servers.map(host).map((h) => `ssh opc@${h} 'sudo bash /tmp/rotate-key.sh retire'`).join("\n");
}

export function helmSwitch(oldKey: string): string {
  return `helm upgrade tether deploy/helm/tether-relay --reuse-values --set key.privateKey="$(cat id_ed25519)" --set key.accept="${oldKey}"`;
}

export const HELM_RETIRE = `helm upgrade tether deploy/helm/tether-relay --reuse-values --set key.accept=""`;

/** A base64 Ed25519 public key (32 bytes). */
export const isKey = (k: string) => /^[A-Za-z0-9+/]{43}=$/.test(k.trim());

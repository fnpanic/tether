// Session recordings on this computer (src-tauri/src/recording.rs, docs/recording.md).
import { formatId, inTauri, invoke } from "./ipc";

export interface Recording {
  /** File name inside the recordings folder; the commands below take it. */
  name: string;
  peer_id: string;
  peer_name: string;
  started_ms: number;
  duration_s: number;
  size: number;
  /** Still waiting for upload. */
  pending: boolean;
}

export interface RecordingsState {
  folder: string;
  /** Picked in Settings (else policy or default). */
  custom: boolean;
  /** Policy fixes the folder. */
  locked: boolean;
  items: Recording[];
}

export const recordingsApi = {
  list: () => invoke<RecordingsState>("recordings_list"),
  play: (name: string) => invoke("recordings_open", { name }),
  reveal: (name: string) => invoke("recordings_open", { name, reveal: true }),
  openFolder: () => invoke("recordings_open", {}),
  remove: (name: string) => invoke("recordings_delete", { name }),
  pickFolder: () => invoke<string | null>("recordings_pick_folder"),
  resetFolder: () => invoke("recordings_reset_folder"),
};

/** `recordings` event (all windows): new files closed, then the result of an upload attempt. */
export type RecordingsEvent = { saved: Recording[]; upload: "none" | "unc" | "https-put" } | { uploaded: string[]; failed: [string, string][] };

export function onRecordings(cb: (e: RecordingsEvent) => void): () => void {
  if (!inTauri) return () => {};
  let off: (() => void) | null = null;
  let dead = false;
  import("@tauri-apps/api/event").then(({ listen }) =>
    listen<RecordingsEvent>("recordings", (e) => cb(e.payload)).then((u) => (dead ? u() : (off = u))),
  );
  return () => {
    dead = true;
    off?.();
  };
}

/** What the "Recording saved" card shows. */
export interface SavedCard {
  files: Recording[];
  upload: "none" | "uploading" | "uploaded" | "failed";
  error?: string;
}

/** A new `saved` event replaces the card; an upload result updates it if it is about its files. */
export function reduceCard(card: SavedCard | null, e: RecordingsEvent): SavedCard | null {
  if ("saved" in e) return e.saved.length ? { files: e.saved, upload: e.upload === "none" ? "none" : "uploading" } : card;
  if (!card || card.upload === "none") return card;
  const mine = (n: string) => card.files.some((f) => f.name === n);
  const fail = e.failed.find(([n]) => mine(n));
  if (fail) return { ...card, upload: "failed", error: fail[1] };
  if (card.files.every((f) => e.uploaded.includes(f.name))) return { ...card, upload: "uploaded" };
  return card;
}

export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = bytes / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v < 10 ? v.toFixed(1) : Math.round(v)} ${units[i]}`;
}

/** `0:42`, `12:05`, `1:02:09`; empty when unknown. */
export function formatDuration(s: number): string {
  if (!s) return "";
  const [h, m, sec] = [Math.floor(s / 3600), Math.floor((s % 3600) / 60), s % 60];
  const p = (n: number) => String(n).padStart(2, "0");
  return h ? `${h}:${p(m)}:${p(sec)}` : `${m}:${p(sec)}`;
}

export const peerLabel = (r: Recording) => r.peer_name || formatId(r.peer_id);

/** Policy `recording.upload` + target, in words. */
export function uploadText(mode: string, target: string, keepLocal: boolean): string {
  if (mode !== "unc" && mode !== "https-put") return "Recordings stay on this computer.";
  const where = target || "(no target set)";
  const verb = mode === "unc" ? "Copied to" : "Uploaded to";
  return `${verb} ${where} after each session.${keepLocal ? "" : " The local copy is deleted once uploaded."}`;
}

/** Policy `recording.max-days` in words. */
export function retentionText(days: string): string {
  const n = parseInt(days, 10);
  return n > 0 ? `Deleted automatically after ${n} day${n === 1 ? "" : "s"}.` : "Kept until you delete them.";
}

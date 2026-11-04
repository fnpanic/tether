/** Event `release-progress` from src-tauri/src/main.rs (struct Progress). */
export interface Progress {
  /** `technician/windows-x86_64` */
  dir: string;
  version: string;
  /** "" while signing `file`. */
  server: string;
  file: string;
  sent: number;
  total: number;
  /** This relay is finished with this release; failed if `error`. */
  done: boolean;
  error: string | null;
}

export interface UploadState {
  current: Progress | null;
  /** One entry per release × relay, in order. */
  finished: Progress[];
}

export const idle: UploadState = { current: null, finished: [] };

export const track = (s: UploadState, p: Progress): UploadState => (p.done ? { current: null, finished: [...s.finished, p] } : { ...s, current: p });

export const percent = (p: Progress) => (p.total ? Math.floor((p.sent * 100) / p.total) : 0);

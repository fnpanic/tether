/** Event `release-progress` from src-tauri/src/main.rs (struct Progress). */
export interface Progress {
  /** `technician/windows-x86_64` */
  dir: string;
  version: string;
  /** "" while signing `file`. */
  server: string;
  /** "" with a server: that relay has every file and waits for the others. */
  file: string;
  sent: number;
  total: number;
  /** This relay is finished with this release; failed if `error`. */
  done: boolean;
  error: string | null;
}

export interface UploadState {
  /** What is going on now: the signing step, or one entry per relay (they upload at once). */
  current: Progress[];
  /** One entry per release × relay, in the order they finished. */
  finished: Progress[];
}

export const idle: UploadState = { current: [], finished: [] };

export const track = (s: UploadState, p: Progress): UploadState => {
  if (p.done) return { current: s.current.filter((c) => c.server !== p.server), finished: [...s.finished, p] };
  if (!p.server) return { ...s, current: [p] };
  // A relay's event ends the signing step and replaces that relay's last one, in place.
  const current = s.current.filter((c) => c.server);
  const i = current.findIndex((c) => c.server === p.server);
  return { ...s, current: i < 0 ? [...current, p] : current.map((c, j) => (j === i ? p : c)) };
};

export const percent = (p: Progress) => (p.total ? Math.floor((p.sent * 100) / p.total) : 0);

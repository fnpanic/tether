// Recordings dialog of the main window: the recordings on this computer, with the folder they are in.
import { useCallback, useEffect, useState } from "react";
import { FolderOpen, Play, Trash2, Video, X } from "lucide-react";
import { Button, IconButton } from "../components/ui";
import { formatDuration, formatSize, onRecordings, peerLabel, recordingsApi, type Recording, type RecordingsState } from "../lib/recordings";
import { formatId } from "../lib/ipc";
import { errorText, usePolicy } from "../lib/policy";

/** Recordings list with live updates (focus and the `recordings` event). */
export function useRecordings() {
  const [st, setSt] = useState<RecordingsState | null>(null);
  const load = useCallback(() => recordingsApi.list().then(setSt).catch(() => {}), []);
  useEffect(() => {
    load();
    window.addEventListener("focus", load);
    const off = onRecordings(load);
    return () => {
      window.removeEventListener("focus", load);
      off();
    };
  }, [load]);
  return { st, load };
}

/** Recording turned off by policy and nothing left to look at: no camera button, no dialog. */
export const recordingsHidden = (eff: ReturnType<typeof usePolicy>, st: RecordingsState | null) =>
  !st || (eff?.values["recording.mode"] === "off" && st.items.length === 0);

/** Opened from the camera button in the main window's title bar, the same way as Settings. */
export function Recordings({ onClose, onToast }: { onClose: () => void; onToast: (m: string) => void }) {
  const { st, load } = useRecordings();
  const [ask, setAsk] = useState<string | null>(null);
  useEffect(() => {
    const k = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, [onClose]);
  const act = (p: Promise<unknown>) => p.catch((e) => onToast(errorText(e)));
  return (
    <div className="animate-fade fixed inset-0 z-30 flex bg-black/40" onMouseDown={onClose}>
      <div
        className="animate-rise m-auto flex h-[min(600px,92vh)] w-[min(860px,94vw)] flex-col overflow-hidden rounded-2xl border border-hairline bg-tile shadow-float"
        onMouseDown={(e) => e.stopPropagation()}
        role="dialog"
        aria-label="Recordings"
        data-testid="recordings"
      >
        <div className="flex items-center gap-3 px-7 pb-2 pt-5">
          <h2 className="text-title font-semibold">Recordings</h2>
          <div className="flex-1" />
          <Button size="sm" onClick={() => act(recordingsApi.openFolder())}>
            <FolderOpen size={14} /> Open folder
          </Button>
          <IconButton label="Close recordings" onClick={onClose}>
            <X size={18} />
          </IconButton>
        </div>
        <div className="truncate px-7 font-mono text-footnote text-muted" title={st?.folder}>
          {st?.folder}
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto px-7 pb-7">
          {st && st.items.length === 0 ? (
            <div className="mt-4 rounded-2xl bg-parchment px-6 py-10 text-center">
              <div className="text-callout font-semibold">No recordings yet</div>
              <div className="mt-1 text-caption text-muted">Click the camera button in a session's toolbar to record. The other person is asked first.</div>
            </div>
          ) : (
            <ul className="mt-4 overflow-hidden rounded-2xl border border-hairline bg-tile">
              {st?.items.map((r) => (
                <Row
                  key={r.name}
                  r={r}
                  asking={ask === r.name}
                  onPlay={() => act(recordingsApi.play(r.name))}
                  onReveal={() => act(recordingsApi.reveal(r.name))}
                  onAsk={() => setAsk(r.name)}
                  onCancel={() => setAsk(null)}
                  onDelete={() => {
                    setAsk(null);
                    act(recordingsApi.remove(r.name).then(load));
                  }}
                />
              ))}
            </ul>
          )}
        </div>
      </div>
    </div>
  );
}

function Row(p: { r: Recording; asking: boolean; onPlay: () => void; onReveal: () => void; onAsk: () => void; onCancel: () => void; onDelete: () => void }) {
  const { r } = p;
  const when = new Date(r.started_ms).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
  const meta = [when, formatDuration(r.duration_s), formatSize(r.size)].filter(Boolean).join(" · ");
  return (
    <li data-testid="recording-row" className="flex items-center gap-4 border-b border-hairline px-5 py-3 last:border-0">
      <button onClick={p.onPlay} aria-label={`Play recording of ${peerLabel(r)}`} className="grid h-10 w-14 shrink-0 place-items-center rounded-md bg-parchment text-muted hover:text-accent">
        <Video size={18} strokeWidth={1.5} />
      </button>
      <div className="min-w-0 flex-1">
        <div className="truncate text-caption font-semibold" title={r.name}>
          {peerLabel(r)}
          {r.peer_name && <span className="ml-2 font-mono font-normal text-muted">{formatId(r.peer_id)}</span>}
        </div>
        <div className="text-footnote text-muted">
          {meta}
          {r.pending && <span className="ml-2 text-warn">Waiting for upload</span>}
        </div>
      </div>
      {p.asking ? (
        <div className="flex items-center gap-2">
          <span className="text-footnote text-muted">Delete this recording?</span>
          <Button size="sm" variant="ghost" onClick={p.onCancel}>
            Cancel
          </Button>
          <Button size="sm" variant="danger" onClick={p.onDelete}>
            Delete
          </Button>
        </div>
      ) : (
        <div className="flex items-center">
          <IconButton label="Play" onClick={p.onPlay}>
            <Play size={16} />
          </IconButton>
          <IconButton label="Show in folder" onClick={p.onReveal}>
            <FolderOpen size={16} />
          </IconButton>
          <IconButton label="Delete" onClick={p.onAsk} className="hover:!text-danger">
            <Trash2 size={16} />
          </IconButton>
        </div>
      )}
    </li>
  );
}

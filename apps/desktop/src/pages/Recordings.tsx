// Main window: the recordings on this computer, with the folder they are in.
import { useCallback, useEffect, useState } from "react";
import { FolderOpen, Play, Trash2, Video } from "lucide-react";
import { Button, IconButton } from "../components/ui";
import { RecordingSaved } from "../components/RecordingSaved";
import { formatDuration, formatSize, peerLabel, recordingsApi, type Recording, type RecordingsState } from "../lib/recordings";
import { formatId } from "../lib/ipc";
import { errorText, usePolicy } from "../lib/policy";

export function RecordingsSection({ onToast }: { onToast: (m: string) => void }) {
  const [st, setSt] = useState<RecordingsState | null>(null);
  const [ask, setAsk] = useState<string | null>(null);
  const eff = usePolicy();
  const load = useCallback(() => recordingsApi.list().then(setSt).catch(() => {}), []);
  useEffect(() => {
    load();
    window.addEventListener("focus", load);
    return () => window.removeEventListener("focus", load);
  }, [load]);
  const act = (p: Promise<unknown>) => p.catch((e) => onToast(errorText(e)));
  const off = eff?.values["recording.mode"] === "off";
  if (!st || (off && st.items.length === 0)) return <RecordingSaved onChange={load} />;
  return (
    <section data-testid="recordings">
      <div className="mt-12 flex items-end gap-3">
        <h2 className="text-headline font-semibold">Recordings</h2>
        <div className="flex-1" />
        <Button size="sm" onClick={() => act(recordingsApi.openFolder())}>
          <FolderOpen size={14} /> Open folder
        </Button>
      </div>
      <div className="mt-1 truncate font-mono text-footnote text-muted" title={st.folder}>
        {st.folder}
      </div>
      {st.items.length === 0 ? (
        <div className="mt-4 rounded-2xl bg-parchment px-6 py-10 text-center">
          <div className="text-callout font-semibold">No recordings yet</div>
          <div className="mt-1 text-caption text-muted">Click the camera button in a session's toolbar to record. The other person is asked first.</div>
        </div>
      ) : (
        <ul className="mt-4 overflow-hidden rounded-2xl border border-hairline bg-tile">
          {st.items.map((r) => (
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
      <RecordingSaved onChange={load} />
    </section>
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

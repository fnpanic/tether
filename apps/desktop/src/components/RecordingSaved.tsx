// "Recording saved" card: shown in every open Tether window when a recording file is closed.
import { useEffect, useState } from "react";
import { CheckCircle2, FolderOpen, Loader2, Play, TriangleAlert, X } from "lucide-react";
import { Button, IconButton } from "./ui";
import { onRecordings, recordingsApi, reduceCard, type SavedCard } from "../lib/recordings";
import { errorText, usePolicy } from "../lib/policy";

export function RecordingSaved({ onChange }: { onChange?: () => void }) {
  const [card, setCard] = useState<SavedCard | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const eff = usePolicy();
  useEffect(
    () =>
      onRecordings((e) => {
        setCard((c) => reduceCard(c, e));
        onChange?.();
      }),
    [onChange],
  );
  // Stays while uploading; otherwise goes away on its own after a while.
  useEffect(() => {
    if (!card || card.upload === "uploading") return;
    const t = setTimeout(() => setCard(null), 20_000);
    return () => clearTimeout(t);
  }, [card]);
  if (!card) return null;
  const f = card.files[0];
  const target = eff?.values["recording.upload-target"] || "the server";
  const act = (p: Promise<unknown>) => p.catch((e) => setErr(errorText(e)));
  return (
    <div role="status" data-testid="recording-saved" className="animate-rise fixed bottom-6 right-6 z-50 w-[360px] max-w-[calc(100vw-48px)] rounded-2xl border border-hairline bg-tile p-5 text-ink shadow-float">
      <div className="flex items-start gap-3">
        <CheckCircle2 size={20} className="mt-0.5 shrink-0 text-ok" />
        <div className="min-w-0 flex-1">
          <div className="text-callout font-semibold">{card.files.length > 1 ? `${card.files.length} recordings saved` : "Recording saved"}</div>
          <div className="mt-0.5 truncate font-mono text-footnote text-muted" title={card.files.map((x) => x.name).join("\n")}>
            {f.name}
          </div>
          {card.upload === "uploading" && (
            <div className="mt-2 flex items-center gap-1.5 text-footnote text-muted">
              <Loader2 size={12} className="animate-spin" /> Uploading to {target}…
            </div>
          )}
          {card.upload === "uploaded" && <div className="mt-2 text-footnote text-ok">Uploaded to {target}.</div>}
          {card.upload === "failed" && (
            <div className="mt-2 flex items-start gap-1.5 text-footnote text-warn" title={card.error}>
              <TriangleAlert size={12} className="mt-0.5 shrink-0" /> Upload failed. Tether retries at the next session or start.
            </div>
          )}
          {err && <div className="mt-2 text-footnote text-danger">{err}</div>}
        </div>
        <IconButton label="Dismiss" className="-mr-2 -mt-2 !h-8 !w-8" onClick={() => setCard(null)}>
          <X size={15} />
        </IconButton>
      </div>
      {/* keep-local N: the file is gone once uploaded. */}
      {!(card.upload === "uploaded" && eff?.values["recording.keep-local"] === "N") && (
      <div className="mt-4 flex gap-2 pl-8">
        <Button size="sm" variant="primary" onClick={() => act(recordingsApi.play(f.name))}>
          <Play size={14} /> Open
        </Button>
        <Button size="sm" onClick={() => act(recordingsApi.reveal(f.name))}>
          <FolderOpen size={14} /> Show in folder
        </Button>
      </div>
      )}
    </div>
  );
}

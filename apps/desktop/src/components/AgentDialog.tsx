// Session menu › Health monitoring agent… (spec 7.4): asks the host for the agent's state, offers
// what the state allows, confirms Install and Remove. The host decides; everything it says is plain,
// clipped text (lib/agent.ts). Kept out of Session.tsx so it can be rendered in unit tests.
import { useEffect, useRef, useState } from "react";
import { Button } from "./ui";
import { invoke } from "../lib/ipc";
import { errorText } from "../lib/policy";
import { clip } from "../lib/hosts/health";
import { agentConfirm, agentView, OFFER_LABEL, PROGRESS, type AgentAction, type AgentAnswer, type AgentOffer } from "../lib/agent";

type Ask = Exclude<AgentOffer, "again">;
export type AgentResult = { a: AgentAnswer; did: AgentAction } | null;

export function AgentDialog({ sessionKey: key, onClose }: { sessionKey: string; onClose: () => void }) {
  const [r, setR] = useState<AgentResult>(null);
  const [busy, setBusy] = useState<AgentAction | null>("status");
  const [err, setErr] = useState("");
  const [ask, setAsk] = useState<Ask | null>(null);
  const live = useRef(true);
  const run = (action: AgentAction) => {
    setAsk(null);
    setErr("");
    setBusy(action);
    invoke<AgentAnswer>("session_agent", { key, action }).then(
      (a) => live.current && (setR(a ? { a, did: action } : null), setBusy(null)),
      (e) => live.current && (setErr(errorText(e)), setBusy(null)),
    );
  };
  useEffect(() => {
    live.current = true;
    run("status");
    return () => void (live.current = false);
  }, []); // eslint-disable-line react-hooks/exhaustive-deps
  return <AgentBody r={r} busy={busy} err={err} ask={ask} onOffer={(o) => (o === "again" ? run("status") : setAsk(o))} onConfirm={(a) => run(a)} onBack={() => setAsk(null)} onClose={onClose} />;
}

/** The dialog for one moment: asking (`busy`), an answer, or the confirmation of `ask`. */
export function AgentBody({ r, busy, err, ask, onOffer, onConfirm, onBack, onClose }: { r: AgentResult; busy: AgentAction | null; err: string; ask: Ask | null; onOffer: (o: AgentOffer) => void; onConfirm: (a: "install" | "remove") => void; onBack: () => void; onClose: () => void }) {
  const box = useRef<HTMLDivElement>(null);
  // The button that had focus may be gone (confirmed, answered): focus stays in the dialog.
  useEffect(() => {
    if (!box.current?.contains(document.activeElement)) box.current?.querySelector<HTMLElement>("[data-close]")?.focus();
  });
  const c = ask && r ? agentConfirm(ask, r.a) : null;
  const v = r && !busy ? agentView(r.a, r.did) : null;
  return (
    <div ref={box} role="dialog" aria-label={c ? c.title : "Health monitoring agent"} data-testid="agent-dialog" className="animate-rise absolute left-1/2 top-16 z-30 w-[400px] max-w-[calc(100%-32px)] -translate-x-1/2 rounded-2xl border border-white/10 glass p-4 shadow-float">
      <div className="text-callout font-semibold">{c ? c.title : "Health monitoring agent"}</div>
      {c ? (
        <>
          {c.warn && (
            <p role="alert" className="mt-2 rounded-xl bg-black/30 px-3 py-2 text-caption font-semibold text-warn">
              Warning: {c.warn}
            </p>
          )}
          <p className="mt-2 text-caption text-white/70">{c.text}</p>
          <div className="mt-4 flex justify-end gap-2">
            <Button variant="ghost" size="sm" className="!text-white/70 hover:!bg-white/10" autoFocus onClick={onBack}>
              Cancel
            </Button>
            <Button variant="primary" size="sm" onClick={() => onConfirm(c.action)}>
              {c.button}
            </Button>
          </div>
        </>
      ) : (
        <>
          <div role="status" aria-live="polite" className="mt-2 text-caption">
            {busy ? (
              <p className="text-white/70">{PROGRESS[busy]}</p>
            ) : err ? (
              <p className="break-words text-warn">Could not ask the remote computer: {clip(err, 300)}</p>
            ) : (
              v && (
                <>
                  <p className={v.problem ? "text-warn" : "text-white/85"}>{v.text}</p>
                  {v.detail && <p className="mt-1 break-words text-footnote text-white/55">The remote computer says: “{v.detail}”</p>}
                </>
              )
            )}
          </div>
          <div className="mt-4 flex flex-wrap justify-end gap-2">
            {!busy && err && (
              <Button size="sm" variant="ghost" className="!text-white/70 hover:!bg-white/10" onClick={() => onOffer("again")}>
                Ask again
              </Button>
            )}
            {!busy &&
              !err &&
              v?.offers.map((o) => (
                <Button key={o} size="sm" variant={o === "remove" ? "danger" : o === "again" ? "ghost" : "primary"} className={o === "again" ? "!text-white/70 hover:!bg-white/10" : ""} onClick={() => onOffer(o)}>
                  {OFFER_LABEL[o]}
                </Button>
              ))}
            <Button data-close variant="ghost" size="sm" className="!text-white/70 hover:!bg-white/10" onClick={onClose}>
              Close
            </Button>
          </div>
        </>
      )}
    </div>
  );
}

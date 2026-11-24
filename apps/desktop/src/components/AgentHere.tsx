// The health monitoring agent on THIS computer when Tether runs without its service (spec 7.2,
// 11.2): the pre-dialog before any elevation prompt (asked by a technician in a session or by the
// card below), and the home view's card while the policy asks to install. Remote input never
// reaches Tether's windows (own_ui_guard), so only the person here can answer. The dialog's texts
// come from Tether's own backend (agent_elevated.rs), shown as plain text.
import { useEffect, useState } from "react";
import { inTauri, invoke } from "../lib/ipc";
import { errorText } from "../lib/policy";
import type { AgentAnswer } from "../lib/agent";
import { Button, Modal } from "./ui";

export interface AgentConsent {
  id: number;
  title: string;
  paragraphs: string[];
  yes: string;
  no: string;
}

/** The pre-dialog's content: the decline first and focused (a key typed as it appears must not install). */
export function AgentConsentBody({ d, onAnswer }: { d: AgentConsent; onAnswer: (yes: boolean) => void }) {
  return (
    <>
      <h2 className="text-headline font-semibold">{d.title}</h2>
      <div className="mt-3 flex flex-col gap-2 text-callout text-ink-80">
        {d.paragraphs.map((p, i) => (
          <p key={i}>{p}</p>
        ))}
      </div>
      <div className="mt-6 flex justify-end gap-2">
        <Button data-autofocus onClick={() => onAnswer(false)}>
          {d.no}
        </Button>
        <Button variant="primary" onClick={() => onAnswer(true)}>
          {d.yes}
        </Button>
      </div>
    </>
  );
}

/** The main window's pre-dialog: shown when the backend says one waits (event, or at start). Escape
 *  or a click outside answers no; the backend gives up after 2 minutes on its own. */
export function AgentConsentDialog({ onNotice }: { onNotice: (m: string) => void }) {
  const [d, setD] = useState<AgentConsent | null>(null);
  useEffect(() => {
    if (!inTauri) return;
    const ask = () => invoke<AgentConsent | null>("health_agent_consent_pending").then(setD, () => {});
    ask();
    let off: (() => void) | undefined;
    let dead = false;
    import("@tauri-apps/api/event")
      .then(({ listen }) => listen("health_agent_consent", ask))
      .then((u) => (dead ? u() : (off = u)));
    return () => {
      dead = true;
      off?.();
    };
  }, []);
  if (!d) return null;
  const answer = (yes: boolean) => {
    setD(null);
    invoke<boolean>("health_agent_consent_answer", { id: d.id, yes }).then(
      (ok) => !ok && onNotice("That question is no longer open."),
      (e) => onNotice(errorText(e)),
    );
  };
  return (
    <Modal key={d.id} onClose={() => answer(false)} width={480}>
      <AgentConsentBody d={d} onAnswer={answer} />
    </Modal>
  );
}

export const CARD_TEXT = "Your organisation asks to install its health monitoring program on this computer.";

export function AgentCardBody({ busy, result, onInstall, onLater }: { busy: boolean; result: string; onInstall: () => void; onLater: () => void }) {
  return (
    <section aria-label="Health monitoring program" className="mb-6 flex shrink-0 items-center gap-4 rounded-2xl border border-hairline bg-tile px-6 py-4">
      <div className="min-w-0 flex-1">
        <p className="text-callout">{CARD_TEXT}</p>
        {result && (
          <p className="mt-1 text-caption text-muted" role="status">
            {result}
          </p>
        )}
      </div>
      <Button variant="primary" size="sm" disabled={busy} onClick={onInstall}>
        {busy ? "Installing…" : "Install…"}
      </Button>
      <Button variant="ghost" size="sm" onClick={onLater}>
        Not now
      </Button>
    </section>
  );
}

/** The home view's card (policy `install`, Tether without its service, no agent here): nothing
 *  happens until the person clicks Install… (then the same pre-dialog and prompt). Not now: hidden
 *  until Tether starts again. */
export function AgentCard() {
  const [shown, setShown] = useState(false);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState("");
  useEffect(() => {
    invoke<boolean>("health_agent_offer").then(setShown, () => {});
  }, []);
  if (!shown) return null;
  const install = () => {
    setBusy(true);
    setResult("");
    invoke<AgentAnswer>("health_agent_install").then(
      (a) => {
        setBusy(false);
        if (a.state === "installed") {
          setShown(false);
          return;
        }
        setResult(typeof a.detail === "string" && a.detail ? a.detail : "The health monitoring program was not installed.");
      },
      (e) => {
        setBusy(false);
        setResult(errorText(e));
      },
    );
  };
  return <AgentCardBody busy={busy} result={result} onInstall={install} onLater={() => setShown(false)} />;
}

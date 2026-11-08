import { useEffect, useRef, useState } from "react";
import { Clipboard, FolderSync, Keyboard, MessageSquare, Mic, RotateCcw, Send, ShieldAlert, Video } from "lucide-react";
import { formatId, invoke, subscribe } from "../lib/ipc";
import { Button, Logo, Modal, Segmented, TitleBar, Toggle } from "../components/ui";

interface Client {
  id: number;
  peer_id: string;
  name: string;
  authorized: boolean;
  disconnected?: boolean;
  is_file_transfer: boolean;
  keyboard: boolean;
  clipboard: boolean;
  audio: boolean;
  file: boolean;
  restart: boolean;
  recording: boolean;
}

const PERMS: { key: keyof Client; perm: string; label: string; icon: typeof Keyboard }[] = [
  { key: "keyboard", perm: "keyboard", label: "Control mouse and keyboard", icon: Keyboard },
  { key: "clipboard", perm: "clipboard", label: "Use clipboard", icon: Clipboard },
  { key: "file", perm: "file", label: "Transfer files", icon: FolderSync },
  { key: "audio", perm: "audio", label: "Hear this computer's audio", icon: Mic },
  { key: "restart", perm: "restart", label: "Restart this computer", icon: RotateCcw },
  { key: "recording", perm: "recording", label: "Record the session", icon: Video },
];

function useElapsed(since: number) {
  const [, tick] = useState(0);
  useEffect(() => {
    const t = setInterval(() => tick((x) => x + 1), 1000);
    return () => clearInterval(t);
  }, []);
  const s = Math.floor((Date.now() - since) / 1000);
  return `${String(Math.floor(s / 60)).padStart(2, "0")}:${String(s % 60).padStart(2, "0")}`;
}

export function ConnectionManager() {
  const [clients, setClients] = useState<Client[]>([]);
  const [active, setActive] = useState<number | null>(null);
  const [since] = useState(() => new Map<number, number>());
  const [chat, setChat] = useState<Record<number, { me: boolean; text: string }[]>>({});
  const [consents, setConsents] = useState<ConsentRequest[]>([]);

  const answerConsent = (r: ConsentRequest, allow: boolean) => {
    invoke("cm_record_consent", { connId: r.conn_id, allow });
    setConsents((cs) => cs.filter((c) => c.conn_id !== r.conn_id));
    setClients((cs) => cs.map((c) => (c.id === r.conn_id ? { ...c, recording: allow } : c)));
  };

  useEffect(() => {
    const sub = subscribe("cm", (e) => {
      const p = e.payload;
      if (e.name === "add_connection") {
        if (!since.has(p.id)) since.set(p.id, Date.now());
        setClients((cs) => [...cs.filter((c) => c.id !== p.id), p]);
        setActive(p.id);
      } else if (e.name === "remove_connection") {
        setClients((cs) => cs.filter((c) => c.id !== p.id));
        setConsents((cs) => cs.filter((c) => c.conn_id !== p.id));
      } else if (e.name === "record_consent_request") {
        setConsents((cs) => [...cs.filter((c) => c.conn_id !== p.conn_id), p]);
      } else if (e.name === "chat") {
        // The backend sink restores the window and flashes the taskbar (no focus change).
        setChat((c) => ({ ...c, [p.id]: [...(c[p.id] ?? []), { me: false, text: p.text }] }));
        setActive(p.id);
      }
    });
    (async () => {
      invoke("cm_record_consent_init").catch(() => {});
      await sub.ready;
      const initial = await invoke<Client[]>("cm_clients").catch(() => []);
      if (initial?.length) {
        initial.forEach((c) => since.set(c.id, Date.now()));
        setClients(initial);
        setActive(initial[initial.length - 1].id);
      }
    })().catch(() => {});
    return sub.off;
  }, [since]);

  const c = clients.find((x) => x.id === active) ?? clients[clients.length - 1];
  if (!c) {
    return (
      <div className="flex h-full flex-col">
        <TitleBar>
          <Logo size={20} />
          <span className="text-caption font-semibold">Tether</span>
        </TitleBar>
        <div className="grid flex-1 place-items-center text-caption text-muted">No active connections</div>
      </div>
    );
  }
  return (
    <div className="flex h-full flex-col">
      <TitleBar>
        <Logo size={20} />
        <span className="text-caption font-semibold">{c.authorized ? "Session active" : "Incoming connection"}</span>
      </TitleBar>
      {clients.length > 1 && (
        <div className="flex gap-1 overflow-x-auto px-3 pb-2">
          {clients.map((x) => (
            <button
              key={x.id}
              onClick={() => setActive(x.id)}
              className={`shrink-0 rounded-lg px-2.5 py-1 text-footnote ${x.id === c.id ? "bg-accent/15 text-accent" : "text-muted hover:bg-parchment"}`}
            >
              {x.name || formatId(x.peer_id)}
            </button>
          ))}
        </div>
      )}
      <ClientView key={c.id} c={c} since={since.get(c.id) ?? Date.now()} msgs={chat[c.id] ?? []} onMsg={(m) => setChat((x) => ({ ...x, [c.id]: [...(x[c.id] ?? []), m] }))} />
      {consents[0] && <RecordConsent r={consents[0]} onAnswer={(allow) => answerConsent(consents[0], allow)} />}
    </div>
  );
}

/** Payload of the `record_consent_request` CM event (shared contract with the customer app). */
interface ConsentRequest {
  conn_id: number;
  peer_name: string;
  text: string;
}

/** Nothing is recorded until the user answers; there is no way to dismiss it without answering. */
function RecordConsent({ r, onAnswer }: { r: ConsentRequest; onAnswer: (allow: boolean) => void }) {
  return (
    <Modal width={340}>
      <div className="flex flex-col items-center text-center">
        <div className="grid h-12 w-12 place-items-center rounded-full bg-parchment">
          <Video size={22} className="text-danger" />
        </div>
        <div className="mt-4 text-headline font-semibold">Recording request</div>
        {r.peer_name && <div className="mt-1 text-caption text-muted">{r.peer_name}</div>}
        <p className="mt-3 text-callout">{r.text || "The technician wants to record this session. Allow?"}</p>
        <div className="mt-6 flex w-full gap-2">
          <Button className="flex-1" onClick={() => onAnswer(false)}>
            Deny
          </Button>
          <Button variant="primary" className="flex-1" autoFocus onClick={() => onAnswer(true)}>
            Allow
          </Button>
        </div>
      </div>
    </Modal>
  );
}

function ClientView({ c, since, msgs, onMsg }: { c: Client; since: number; msgs: { me: boolean; text: string }[]; onMsg: (m: { me: boolean; text: string }) => void }) {
  const elapsed = useElapsed(since);
  const [perm, setPerm] = useState(c);
  const [text, setText] = useState("");
  const [tab, setTab] = useState<"perm" | "chat">("perm");
  const [seen, setSeen] = useState(0);
  const end = useRef<HTMLDivElement>(null);
  const incoming = msgs.filter((m) => !m.me).length;
  useEffect(() => {
    setPerm(c);
  }, [c]);
  // Each message from the technician opens the chat (the backend restores the window).
  useEffect(() => {
    if (incoming) setTab("chat");
  }, [incoming]);
  // Braces: scrollIntoView may return a Promise, which React would call as cleanup.
  useEffect(() => {
    end.current?.scrollIntoView({ block: "end" });
  }, [tab, msgs.length]);
  useEffect(() => {
    if (tab === "chat") setSeen(msgs.length);
  }, [tab, msgs.length]);
  const unread = msgs.slice(seen).filter((m) => !m.me).length;
  const initials = (c.name || c.peer_id).replace(/[^A-Za-z0-9]/g, "").slice(0, 2).toUpperCase();

  return (
    <div className="flex min-h-0 flex-1 flex-col px-5 pb-5">
      <div className="animate-rise flex items-center gap-3 rounded-2xl border border-hairline bg-tile p-4">
        <div className="grid h-12 w-12 place-items-center rounded-full bg-accent text-callout font-semibold text-white">{initials}</div>
        <div className="min-w-0 flex-1">
          <div className="truncate text-callout font-semibold" title={c.name || undefined}>
            {c.name || "Unknown"}
          </div>
          <div className="font-mono text-footnote text-muted">{formatId(c.peer_id)}</div>
        </div>
        {c.authorized && <div className="rounded-lg bg-ok/15 px-2 py-1 font-mono text-footnote text-ok">{elapsed}</div>}
      </div>

      {!c.authorized && (
        <div className="mt-3 flex items-start gap-2 rounded-xl bg-warn/10 px-3 py-2.5 text-caption text-ink">
          <ShieldAlert size={16} className="mt-0.5 shrink-0 text-warn" />
          <span>
            {c.is_file_transfer ? "wants to transfer files with this computer." : "wants to see and control this screen."} Only accept if you know this person.
          </span>
        </div>
      )}

      <div className="mt-4 flex justify-center">
        <Segmented
          value={tab}
          onChange={setTab}
          options={[
            { value: "perm", label: "Permissions" },
            { value: "chat", label: `Chat${unread ? ` (${unread} new)` : ""}` },
          ]}
        />
      </div>

      <div className="mt-3 min-h-0 flex-1 overflow-y-auto">
        {tab === "perm" ? (
          <div className="space-y-1">
            {PERMS.map(({ key, perm: name, label, icon: Icon }) => (
              <div key={name} className="flex items-center gap-3 rounded-xl px-2 py-2 hover:bg-parchment">
                <Icon size={16} className="text-muted" />
                <span className="flex-1 text-caption">{label}</span>
                <Toggle
                  label={label}
                  checked={!!perm[key]}
                  onChange={(v) => {
                    setPerm({ ...perm, [key]: v });
                    invoke("cm_permission", { id: c.id, name, enabled: v });
                  }}
                />
              </div>
            ))}
          </div>
        ) : (
          <div className="flex h-full flex-col">
            <div className="flex-1 space-y-2 overflow-y-auto">
              {msgs.length === 0 && (
                <div className="flex flex-col items-center pt-8 text-caption text-muted">
                  <MessageSquare size={20} className="mb-2" />
                  No messages
                </div>
              )}
              {msgs.map((m, i) => (
                <div key={i} className={`flex ${m.me ? "justify-end" : ""}`}>
                  <div className={`max-w-[80%] rounded-2xl px-3 py-1.5 text-caption ${m.me ? "bg-accent text-white" : "bg-parchment"}`}>{m.text}</div>
                </div>
              ))}
              <div ref={end} />
            </div>
            <form
              className="mt-2 flex gap-2"
              onSubmit={(e) => {
                e.preventDefault();
                if (!text.trim()) return;
                invoke("cm_chat", { id: c.id, text: text.trim() });
                onMsg({ me: true, text: text.trim() });
                setText("");
              }}
            >
              <input value={text} onChange={(e) => setText(e.target.value)} placeholder="Message" className="h-9 flex-1 rounded-xl border border-hairline bg-parchment px-3.5 text-caption outline-none focus-visible:border-focus" />
              <button aria-label="Send" className="grid h-9 w-9 place-items-center rounded-full bg-accent text-white">
                <Send size={15} />
              </button>
            </form>
          </div>
        )}
      </div>

      <div className="mt-4 flex gap-2">
        {c.authorized ? (
          <Button variant="danger" size="lg" className="flex-1" onClick={() => invoke("cm_close", { id: c.id })}>
            End session
          </Button>
        ) : (
          <>
            <Button size="lg" className="flex-1" onClick={() => invoke("cm_close", { id: c.id })}>
              Decline
            </Button>
            <Button variant="primary" size="lg" className="flex-1" autoFocus onClick={() => invoke("cm_authorize", { id: c.id })}>
              Accept
            </Button>
          </>
        )}
      </div>
    </div>
  );
}
